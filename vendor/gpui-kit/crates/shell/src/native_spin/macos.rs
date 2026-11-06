//! Spinners as Core Animation layers over the window's Metal layer.
//!
//! One flipped overlay layer per window holds a clip layer per spinner (its
//! anchor's bounds, so it is clipped like GPUI would clip it) with the icon
//! inside, turning on a `transform.rotation.z` animation that repeats forever.
//! The window server runs the animation; the app only touches a layer when
//! its spinner moves, changes, is covered or goes, inside one transaction
//! per frame with implicit animations off.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use gpui::{App, Bounds, DevicePixels, EntityId, Pixels, RenderSvgParams, Size, Window, WindowId};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_core_foundation::{CFData, CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
};
use objc2_foundation::{NSNumber, NSString};
use objc2_quartz_core::{CABasicAnimation, CALayer, CAMediaTiming, CATransaction};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::Spinner;

struct Entry {
    window: WindowId,
    /// The view to build again when the spinner changes hands between GPUI and
    /// its layer.
    root: EntityId,
    spinner: Spinner,
    /// Something was painted over it, so GPUI draws it instead of its layer.
    covered: bool,
    native: Option<Native>,
}

/// A spinner's layers while it has them.
struct Native {
    clip: Retained<CALayer>,
    icon: Retained<CALayer>,
    /// What the icon's image and animation were made from.
    made: Option<Look>,
    /// The image's size: GPUI keeps an icon's aspect and centers it.
    image_size: Size<Pixels>,
    /// Where the layers were last put.
    placed: Option<(Bounds<Pixels>, Bounds<Pixels>)>,
}

/// What a spinner's image and animation depend on.
#[derive(PartialEq)]
struct Look {
    path: gpui::SharedString,
    color: gpui::Hsla,
    ms: f64,
    size: Size<Pixels>,
    scale: f32,
}

impl Drop for Native {
    fn drop(&mut self) {
        self.clip.removeFromSuperlayer();
    }
}

thread_local! {
    static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    /// Each window's overlay, or `None` where native spinners are off.
    static WINDOWS: RefCell<HashMap<WindowId, Option<Retained<CALayer>>>> =
        RefCell::new(HashMap::new());
    static WATCHING_CLOSES: Cell<bool> = const { Cell::new(false) };
}

/// Whether `window` turns spinners natively. The first call sets the window
/// up: its overlay layer and the hook that syncs layers after each frame.
/// `HERDR_NATIVE_SPINNERS=0` turns it off, to compare against GPUI's.
pub(crate) fn enabled(window: &mut Window, cx: &mut App) -> bool {
    let id = Window::window_handle(window).window_id();
    if let Some(on) = WINDOWS.with(|w| w.borrow().get(&id).map(Option::is_some)) {
        return on;
    }
    if !WATCHING_CLOSES.with(|w| w.replace(true)) {
        cx.on_window_closed(|_, id| {
            WINDOWS.with(|w| w.borrow_mut().remove(&id));
            ENTRIES.with(|e| e.borrow_mut().retain(|_, entry| entry.window != id));
        })
        .detach();
    }
    let off = std::env::var_os("HERDR_NATIVE_SPINNERS").is_some_and(|v| v == "0");
    let overlay = if off { None } else { overlay(window) };
    let on = overlay.is_some();
    WINDOWS.with(|w| w.borrow_mut().insert(id, overlay));
    if on {
        window.on_frame_painted(move |window, cx| sync(id, window, cx));
    }
    on
}

/// Whether GPUI should draw `key`'s spinner: something was painted over it.
pub(crate) fn covered(key: u64) -> bool {
    ENTRIES.with(|e| e.borrow().get(&key).is_some_and(|e| e.covered))
}

/// Paints `key`'s anchor at `spinner`. Call while painting, inside the
/// spinner's content mask, after GPUI drew it if it's [`covered`].
pub(crate) fn place(key: u64, root: EntityId, spinner: Spinner, window: &mut Window) {
    let id = Window::window_handle(window).window_id();
    let bounds = spinner.bounds;
    ENTRIES.with(|entries| {
        let mut entries = entries.borrow_mut();
        let entry = entries.entry(key).or_insert_with(|| Entry {
            window: id,
            root,
            spinner: spinner.clone(),
            covered: false,
            native: None,
        });
        if entry.window != id {
            // Its layers belong to the other window's overlay.
            entry.native = None;
            entry.window = id;
        }
        entry.root = root;
        entry.spinner = spinner;
    });
    window.paint_native_anchor(key, bounds);
}

fn ns_view(window: &Window) -> Option<&NSView> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    unsafe { (handle.ns_view.as_ptr() as *const NSView).as_ref() }
}

/// A layer over the window's Metal layer, flipped so it shares GPUI's
/// top-left origin and points.
fn overlay(window: &Window) -> Option<Retained<CALayer>> {
    let host = ns_view(window)?.layer()?;
    let overlay = CALayer::new();
    overlay.setGeometryFlipped(true);
    host.addSublayer(&overlay);
    Some(overlay)
}

/// After a frame painted: layers follow the anchors it has.
fn sync(id: WindowId, window: &mut Window, cx: &mut App) {
    crate::materialize::reset_native_slots();
    let Some(overlay) = WINDOWS.with(|w| w.borrow().get(&id).cloned().flatten()) else {
        return;
    };
    let anchors = window.native_anchors();
    let has_entries = ENTRIES.with(|e| e.borrow().values().any(|entry| entry.window == id));
    if anchors.is_empty() && !has_entries {
        return;
    }
    let scale = window.scale_factor();
    let mut rebuild = HashSet::new();

    CATransaction::begin();
    CATransaction::setDisableActions(true);
    if let Some(host) = overlay.superlayer() {
        overlay.setFrame(host.bounds());
    }
    ENTRIES.with(|entries| {
        let mut entries = entries.borrow_mut();
        let mut seen = HashSet::new();
        for (key, clip, is_covered) in anchors {
            let Some(entry) = entries.get_mut(&key) else {
                continue;
            };
            seen.insert(key);
            // A spinner changes hands between GPUI and its layer on the next
            // build; until then the layer stays hidden, so it's never drawn twice.
            let drawn_by_gpui = entry.covered;
            if is_covered != drawn_by_gpui {
                entry.covered = is_covered;
                rebuild.insert(entry.root);
            }
            if drawn_by_gpui || is_covered {
                if let Some(native) = &entry.native {
                    native.clip.setHidden(true);
                }
            } else {
                show(entry, &overlay, clip, scale, window, cx);
            }
        }
        // A spinner no frame paints any more is gone; dropping its layers
        // removes them.
        entries.retain(|key, entry| entry.window != id || seen.contains(key));
    });
    CATransaction::commit();

    for root in rebuild {
        window.on_next_frame(move |_, cx| cx.notify(root));
    }
}

fn show(
    entry: &mut Entry,
    overlay: &CALayer,
    clip: Bounds<Pixels>,
    scale: f32,
    window: &Window,
    cx: &App,
) {
    let native = entry.native.get_or_insert_with(|| {
        let clip = CALayer::new();
        clip.setMasksToBounds(true);
        let icon = CALayer::new();
        clip.addSublayer(&icon);
        overlay.addSublayer(&clip);
        Native {
            clip,
            icon,
            made: None,
            image_size: Size::default(),
            placed: None,
        }
    });
    let spinner = &entry.spinner;

    let look = Look {
        path: spinner.path.clone(),
        color: spinner.color,
        ms: spinner.ms,
        size: spinner.bounds.size,
        scale,
    };
    if native.made.as_ref() != Some(&look) {
        native.made = Some(look);
        native.placed = None;
        let image = image(spinner, scale, window_colorspace(window).as_deref(), cx);
        native.image_size = image.as_ref().map_or(Size::default(), |(_, size)| *size);
        let contents = image.as_ref().map(|(image, _)| unsafe {
            &*(CFRetained::as_ptr(image).as_ptr() as *const AnyObject)
        });
        unsafe { native.icon.setContents(contents) };
        native.icon.setContentsScale(scale as f64);
        native
            .icon
            .addAnimation_forKey(&turn(spinner.ms), Some(&NSString::from_str("herdr.spin")));
    }

    // The icon at its image's size, centered on the spinner, relative to the clip.
    let center = spinner.bounds.center();
    let icon = Bounds {
        origin: gpui::point(
            center.x - native.image_size.width / 2.0 - clip.origin.x,
            center.y - native.image_size.height / 2.0 - clip.origin.y,
        ),
        size: native.image_size,
    };
    if native.placed != Some((clip, icon)) {
        native.placed = Some((clip, icon));
        native.clip.setFrame(rect(clip));
        native.icon.setFrame(rect(icon));
    }
    native.clip.setHidden(false);
}

/// One turn every `ms`, forever, in step with GPUI's spinners (see
/// `loop_clock_ms`): they all share one clock.
fn turn(ms: f64) -> Retained<CABasicAnimation> {
    let animation =
        CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("transform.rotation.z")));
    unsafe {
        animation.setFromValue(Some(&NSNumber::new_f64(0.0)));
        animation.setToValue(Some(&NSNumber::new_f64(std::f64::consts::TAU)));
    }
    animation.setDuration(ms / 1000.0);
    animation.setRepeatCount(f32::INFINITY);
    animation.setTimeOffset((crate::materialize::loop_clock_ms() % ms) / 1000.0);
    animation.setRemovedOnCompletion(false);
    animation
}

/// GPUI's Metal layer is untagged, so the window server shows its pixels in
/// the screen's color space; the icon is tagged with that space to match.
fn window_colorspace(window: &Window) -> Option<Retained<CGColorSpace>> {
    ns_view(window)?.window()?.colorSpace()?.CGColorSpace()
}

/// The icon drawn as GPUI draws it: its alpha mask in the spinner's color,
/// premultiplied, and its size in window pixels.
fn image(
    spinner: &Spinner,
    scale: f32,
    colorspace: Option<&CGColorSpace>,
    cx: &App,
) -> Option<(CFRetained<CGImage>, Size<Pixels>)> {
    let size = Size {
        width: DevicePixels((spinner.bounds.size.width.as_f32() * scale).round() as i32),
        height: DevicePixels((spinner.bounds.size.height.as_f32() * scale).round() as i32),
    };
    if size.width.0 <= 0 || size.height.0 <= 0 {
        return None;
    }
    let params = RenderSvgParams {
        path: spinner.path.clone(),
        size,
    };
    let (size, mask) = cx.svg_renderer().render_alpha_mask(&params, None).ok()??;
    let rgb = spinner.color.to_rgb();
    let mut pixels = Vec::with_capacity(mask.len() * 4);
    for coverage in mask {
        let a = coverage as f32 / 255.0 * rgb.a;
        let channel = |c: f32| (c * a * 255.0).round().clamp(0.0, 255.0) as u8;
        pixels.extend([
            channel(rgb.r),
            channel(rgb.g),
            channel(rgb.b),
            (a * 255.0).round() as u8,
        ]);
    }
    let (width, height) = (size.width.0 as usize, size.height.0 as usize);
    let data = unsafe { CFData::new(None, pixels.as_ptr(), pixels.len() as isize) }?;
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    let fallback;
    let colorspace = match colorspace {
        Some(space) => space,
        None => {
            fallback = CGColorSpace::new_device_rgb()?;
            &fallback
        }
    };
    let image = unsafe {
        CGImage::new(
            width,
            height,
            8,
            32,
            width * 4,
            Some(colorspace),
            CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0),
            Some(&provider),
            std::ptr::null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }?;
    let points = Size {
        width: gpui::px(width as f32 / scale),
        height: gpui::px(height as f32 / scale),
    };
    Some((image, points))
}

fn rect(bounds: Bounds<Pixels>) -> CGRect {
    CGRect {
        origin: CGPoint {
            x: bounds.origin.x.into(),
            y: bounds.origin.y.into(),
        },
        size: CGSize {
            width: bounds.size.width.into(),
            height: bounds.size.height.into(),
        },
    }
}
