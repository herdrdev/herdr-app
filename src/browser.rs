//! A native HTTP(S) view for trusted mods. Web pages get no Herdr IPC bridge.
mod policy;
mod embed;
#[cfg(windows)]
mod win32;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::create;

use gpui::{HitboxId, HitboxBehavior, AnyElement, App, Bounds, IntoElement, ParentElement, Pixels, Styled, Window, WindowId, canvas, div};
use gpui_shell::{ArgumentDescriptor, ArgumentSchema, ComponentArgument, ComponentDataValue, ComponentDescriptor, ComponentMaterializer, ComponentPayload, ComponentRegistry, ConstructorDescriptor, FrozenComponentRegistry, MaterializeRequest, StateDescriptor, StateMethodDescriptor, COMPONENT_REGISTRY_API_VERSION};
use std::{cell::RefCell, rc::{Rc, Weak}, sync::Arc};

type State = Rc<RefCell<Browser>>;
thread_local! {
    static VIEWS: RefCell<Vec<Weak<RefCell<Browser>>>> = const { RefCell::new(Vec::new()) };
    /// What overlays cover this frame, collected by `cover()` during prepaint.
    static COVERS: RefCell<Vec<(WindowId, Bounds<Pixels>)>> = const { RefCell::new(Vec::new()) };
}

/// Marks its parent's area as drawn over native views this frame: a toast,
/// a menu, a dialog's backdrop. Native views leave that area to GPUI.
pub fn cover() -> impl IntoElement {
    canvas(
        |bounds, window, _| COVERS.with_borrow_mut(|covers| covers.push((window.window_handle().window_id(), bounds))),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

struct Browser {
    window: WindowId,
    url: String,
    native: Option<wry::WebView>,
    #[cfg(not(target_os = "linux"))]
    context: Option<wry::WebContext>,
    error: Option<String>,
    hitbox: Option<HitboxId>,
    bounds: Option<wry::Rect>,
    visible: bool,
    #[cfg(windows)]
    creating: bool,
    /// Wry's container window, which holds the WebView2 windows.
    #[cfg(windows)]
    hwnd: Option<isize>,
    /// What overlays cut out of it now.
    #[cfg(windows)]
    holes: Vec<[i32; 4]>,
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(view) = &self.native {
            let _ = view.set_visible(false);
        }
    }
}

#[cfg(not(windows))]
fn create(state: &State, window: &mut Window, cx: &mut App) {
    let mut browser = state.borrow_mut();
    if let Err(error) = browser.create(window, cx) { browser.error = Some(error.to_string()); }
}

impl Browser {
    fn builder<'a>(url: &str, context: &'a mut wry::WebContext) -> wry::WebViewBuilder<'a> {
        wry::WebViewBuilder::new_with_web_context(context)
            .with_url(url)
            .with_visible(false)
            .with_navigation_handler(|url| policy::allows_navigation(&url))
            // No popups, downloads, custom protocols, initialization scripts or native IPC.
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
    }

    #[cfg(not(windows))]
    fn create(&mut self, window: &mut Window, cx: &mut App) -> anyhow::Result<()> {
        if self.native.is_some() || self.error.is_some() { return Ok(()); }
        #[cfg(target_os = "linux")]
        let view = linux::build(&self.url, window, cx)?;
        #[cfg(not(target_os = "linux"))]
        let view = {
            let mut context = wry::WebContext::new(Some(crate::mods::app_dir().join("browser")));
            let view = Self::builder(&self.url, &mut context).build_as_child(window)?;
            self.context = Some(context);
            view
        };
        // Wry's X11 host is installed after its initial visibility is applied.
        // Hide once more after construction so its child window is hidden too.
        view.set_visible(false)?;
        self.native = Some(view);
        Ok(())
    }

    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window) {
        self.hitbox = Some(window.insert_hitbox(bounds, HitboxBehavior::Normal).id);
        // GTK expects physical pixels. WebView2 and WKWebView use logical coordinates.
        let x = f64::from(f32::from(bounds.origin.x));
        let y = f64::from(f32::from(bounds.origin.y));
        let width = f64::from(f32::from(bounds.size.width));
        let height = f64::from(f32::from(bounds.size.height));
        let rect = if cfg!(target_os = "linux") {
            let scale = f64::from(window.scale_factor());
            wry::Rect {
                position: wry::dpi::PhysicalPosition::new(x * scale, y * scale).into(),
                size: wry::dpi::PhysicalSize::new(width * scale, height * scale).into(),
            }
        } else {
            wry::Rect {
                position: wry::dpi::LogicalPosition::new(x, y).into(),
                size: wry::dpi::LogicalSize::new(width, height).into(),
            }
        };
        // Creation may still be pending on Windows. Keep the latest requested
        // rectangle even when this canvas is cached before the native view arrives.
        self.bounds = Some(rect);
        let Some(view) = &self.native else { return };
        if let Err(error) = view.set_bounds(rect) {
            self.error = Some(error.to_string());
        }
    }

    fn show(&mut self, window: &Window, dragging: bool, covers: &[Bounds<Pixels>]) {
        let hitbox = self.hitbox.and_then(|id| window.prepainted_hitbox(id));
        // Windows cuts overlays out of the native view; elsewhere it hides.
        let overlay = dragging || hitbox.is_some_and(|hitbox| {
            policy::fully_covered(&hitbox.bounds, covers) || (!cfg!(windows) && !policy::holes(&hitbox.bounds, covers, 1.).is_empty())
        });
        #[cfg(windows)]
        if let (Some(hwnd), Some(hitbox)) = (self.hwnd, hitbox.as_ref()) {
            let scale = window.scale_factor();
            let holes = policy::holes(&hitbox.bounds, covers, scale);
            if holes != self.holes {
                let size = |v: Pixels| (f32::from(v) * scale).round() as i32;
                win32::set_holes(hwnd as _, size(hitbox.bounds.size.width), size(hitbox.bounds.size.height), &holes);
                self.holes = holes;
            }
        }
        let unclipped = hitbox.is_some_and(|hitbox| {
            // WKWebView keeps a floating-point logical frame. Windows/GTK
            // children use integer device pixels, like GPUI's rendered mask.
            if cfg!(target_os = "macos") {
                policy::is_unclipped(&hitbox.bounds, &hitbox.content_mask.bounds)
            } else {
                policy::is_unclipped_at_scale(&hitbox.bounds, &hitbox.content_mask.bounds, window.scale_factor())
            }
        });
        let show = policy::should_show(hitbox.is_some(), unclipped, overlay) && self.error.is_none();
        tracing::trace!(geometry = ?hitbox.map(|hitbox| (hitbox.bounds, hitbox.content_mask.bounds)), overlay, unclipped, show, visible = self.visible, "native browser visibility");
        if show == self.visible { return; }
        if let Some(view) = &self.native {
            if !show {
                if let Err(error) = view.focus_parent() {
                    tracing::warn!("browser focus could not return to its parent: {error}");
                }
            }
            match view.set_visible(show) {
                Ok(()) => {
                    self.visible = show;
                    // GTK can restore its default size when a hidden child is shown.
                    #[cfg(target_os = "linux")]
                    if show {
                        if let Some(bounds) = self.bounds {
                            if let Err(error) = view.set_bounds(bounds) { self.error = Some(error.to_string()); }
                        }
                    }
                },
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }
}

/// Native children follow cache-aware frame geometry, not render callbacks:
/// a cached view can still be on screen without executing its canvas again.
/// Hide children for overlays, clipping, or an absent (inactive-tab) hitbox.
pub fn end_frame(dragging: bool) -> impl IntoElement {
    canvas(|_, _, _| (), move |_, _, window, cx| {
        let id = window.window_handle().window_id();
        let covers: Vec<Bounds<Pixels>> = COVERS.with_borrow_mut(|covers| {
            let mine = covers.iter().filter(|(w, _)| *w == id).map(|(_, b)| *b).collect();
            covers.retain(|(w, _)| *w != id);
            mine
        });
        VIEWS.with_borrow_mut(|views| {
            views.retain(|weak| {
                let Some(view) = weak.upgrade() else { return false };
                let mut view = view.borrow_mut();
                if view.window == id { view.show(window, dragging, &covers); }
                true
            });
        });
        embed::end_frame(dragging, &covers, window, cx);
    })
}

struct BrowserElement;
impl ComponentMaterializer for BrowserElement {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> anyhow::Result<AnyElement> {
        let argument = request.payload().downcast_ref::<ComponentArgument>().unwrap().clone();
        let state = request.with_state::<State, _>(&argument, Clone::clone)?;
        request.with_window_app(|window, cx| {
            create(&state, window, cx);
            Ok(())
        })?;
        let message = state.borrow().error.clone().unwrap_or_else(|| "Browser hidden while its surface is clipped or covered.".into());
        let element = div().relative().size_full()
            .child(div().absolute().size_full().p(gpui::px(16.)).child(message))
            .child(canvas(move |bounds, window, _| state.borrow_mut().layout(bounds, window), |_, _, _, _| {}).size_full());
        request.finish(element)
    }
}

pub fn components() -> FrozenComponentRegistry {
    let mut registry = ComponentRegistry::new(COMPONENT_REGISTRY_API_VERSION, "herdr-browser").unwrap();
    registry.register_state(StateDescriptor::new(
        "BrowserState", "HerdrBrowserState", vec![ArgumentDescriptor::new("url", ArgumentSchema::String)],
        |args, window, _| {
            let ComponentArgument::String(url) = &args[0] else { unreachable!() };
            if !policy::allows_navigation(url) { return Err("Expected an HTTP(S) URL without credentials".into()); }
            let state = Rc::new(RefCell::new(Browser { window: window.window_handle().window_id(), url: url.clone(), native: None, #[cfg(not(target_os = "linux"))] context: None, error: None, hitbox: None, bounds: None, visible: false,
                #[cfg(windows)] creating: false, #[cfg(windows)] hwnd: None, #[cfg(windows)] holes: Vec::new() }));
            VIEWS.with_borrow_mut(|views| views.push(Rc::downgrade(&state)));
            Ok(Box::new(state))
        },
    ).with_documentation("A retained native browser. Pages have no Herdr IPC. HTTP(S) only; no popups or downloads.")
    .with_methods(vec![
        StateMethodDescriptor::new::<State>("navigate", "(url: string): void", |state, args, _, _| {
            let Some(ComponentDataValue::String(url)) = args.first() else { anyhow::bail!("Expected a URL") };
            anyhow::ensure!(policy::allows_navigation(url), "Expected an HTTP(S) URL without credentials");
            let mut state = state.borrow_mut();
            if let Some(view) = &state.native { view.load_url(url)?; }
            state.url = url.clone();
            state.error = None;
            Ok(ComponentDataValue::Null)
        }),
        StateMethodDescriptor::new::<State>("reload", "(): void", |state, _, _, _| {
            let mut state = state.borrow_mut();
            if let Some(view) = &state.native { view.reload()?; }
            state.error = None;
            Ok(ComponentDataValue::Null)
        }),
        StateMethodDescriptor::new::<State>("back", "(): void", |state, _, _, _| {
            if let Some(view) = &state.borrow().native { view.evaluate_script("history.back()")?; }
            Ok(ComponentDataValue::Null)
        }),
        StateMethodDescriptor::new::<State>("url", "(): string", |state, _, _, _| {
            let state = state.borrow();
            let url = match &state.native { Some(view) => view.url()?, None => state.url.clone() };
            Ok(ComponentDataValue::String(url))
        }).with_readonly(true),
        StateMethodDescriptor::new::<State>("error", "(): string | null", |state, _, _, _| {
            Ok(state.borrow().error.clone().map(ComponentDataValue::String).unwrap_or(ComponentDataValue::Null))
        }).with_readonly(true),
    ])).unwrap();
    registry.register(ComponentDescriptor::new("Browser", Arc::new(BrowserElement))
        .with_documentation("Native browser element. Retain BrowserState in init and render Browser(state).")
        .with_constructors(vec![ConstructorDescriptor::new("Browser", vec![ArgumentDescriptor::new("state", ArgumentSchema::Entity("HerdrBrowserState"))], |args| Ok(ComponentPayload::new(args[0].clone())))])
        .with_methods(vec![])).unwrap();
    embed::register(&mut registry);
    registry.freeze().unwrap()
}
