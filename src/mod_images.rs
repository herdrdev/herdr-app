//! `setImage`: pictures a mod hands over from memory, drawn wherever
//! `image(path)` names their path, without writing a file.
//!
//! A mod sets paths in its own folder only. Raw pixels are reordered and
//! encoded ones decoded on GPUI's background pool; the picture is installed
//! back on the UI thread, where the store lives, and only if the call is still
//! its path's newest one. A failed call leaves the picture before it in place.
//! The store is `gpui_shell::live_image`, which also drops a picture once the
//! application that set it is retired (reloaded, restarted, closed); this adds
//! which loaded copy set each path, so a copy clears only its own.

use gpui::{RenderImage, SharedString};
use gpui_shell::host_modules::HostFuture;
use gpui_shell::live_image::{self, Owner};
use gpui_shell::{HostArguments, HostError, HostValue};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Widest and tallest picture, in pixels.
pub const MAX_SIDE: u32 = 4096;
/// Largest encoded picture, in bytes.
pub const MAX_ENCODED: usize = 32 << 20;
/// Paths showing (or about to show) a picture from memory, across all mods.
pub const MAX_PATHS: usize = 256;
/// Decoded pixels held for pictures from memory, across all mods: the ones
/// shown and the calls still converting. A 4K frame fits with its
/// replacement; the largest picture fits alone. (Inputs are a passing copy
/// of bytes the script VM already counts.)
pub const MAX_BYTES: usize = 64 << 20;

struct Shown {
    copy: u64,
    owner: Owner,
    bytes: usize,
}

struct Pending {
    copy: u64,
    call: u64,
    /// Set once a newer call or a clear replaces this one, so its conversion
    /// is skipped if it hasn't started.
    superseded: Arc<AtomicBool>,
}

thread_local! {
    static SHOWN: RefCell<HashMap<SharedString, Shown>> = RefCell::new(HashMap::new());
    static PENDING: RefCell<HashMap<SharedString, Pending>> = RefCell::new(HashMap::new());
    /// Bytes held by calls that haven't finished.
    static RESERVED: Cell<usize> = const { Cell::new(0) };
    static CALLS: Cell<u64> = const { Cell::new(0) };
}

/// Forgets the pictures a loaded copy of a mod set, so `image(path)` shows
/// their files again: when the window drops the copy. (Its `herdr` module can
/// outlive it in the window's script VM, so this can't hang off the module.)
pub fn forget(copy: u64) {
    // A window dropped as the thread ends finds the stores already gone.
    let _ = PENDING.try_with(|pending| {
        pending.borrow_mut().retain(|_, p| {
            let keep = p.copy != copy;
            if !keep {
                p.superseded.store(true, Ordering::Relaxed);
            }
            keep
        })
    });
    let _ = SHOWN.try_with(|shown| {
        shown.borrow_mut().retain(|key, s| {
            let keep = s.copy != copy;
            if !keep {
                live_image::clear(key);
            }
            keep
        })
    });
}

enum Pixels {
    Raw { width: u32, height: u32, channels: usize, bytes: Vec<u8> },
    Encoded { bytes: Vec<u8>, format: image::ImageFormat },
}

/// `setImage(path, bytes, options?)`: resolves `true` once the picture shows
/// from the next frame on, or `false` if a newer call, a clear or a reload got
/// there first. `setImage(path, null)` clears this copy's picture.
pub fn set_image(copy: u64, name: &str, a: &HostArguments) -> Result<HostFuture, HostError> {
    let path = a.string(0)?;
    let key = own_key(name, path)?;
    let owner = Owner::current().ok_or_else(|| HostError::new("setImage: only a mod's view can set pictures"))?;
    let bytes = match a.get(1) {
        Some(HostValue::Null) => {
            clear(copy, &key);
            return Ok(Box::pin(async { Ok(HostValue::Bool(false)) }));
        }
        Some(HostValue::Bytes(bytes)) => bytes,
        None => return Err(HostError::new("setImage: pass the bytes, or null to clear")),
        Some(other) => {
            return Err(HostError::new(format!("setImage: bytes must be a Uint8Array or null, not {}", other.describe())));
        }
    };
    let pixels = pixels(bytes, a.get(2))?;
    let (width, height) = match &pixels {
        Pixels::Raw { width, height, .. } => (*width, *height),
        Pixels::Encoded { bytes, format } => header(bytes, *format)?,
    };
    let reserve = width as usize * height as usize * 4;

    prune();
    let shown: usize = SHOWN.with_borrow(|shown| shown.values().map(|s| s.bytes).sum());
    let replaced = SHOWN.with_borrow(|shown| shown.get(&key).map_or(0, |s| s.bytes));
    if shown - replaced + RESERVED.get() + reserve > MAX_BYTES {
        return Err(HostError::new(format!(
            "setImage: pictures from memory would hold more than {} MiB; wait for earlier calls, or clear paths with setImage(path, null)",
            MAX_BYTES >> 20
        )));
    }
    let paths = SHOWN.with_borrow(|shown| {
        PENDING.with_borrow(|pending| pending.keys().filter(|k| !shown.contains_key(*k)).count() + shown.len())
    });
    let known = SHOWN.with_borrow(|s| s.contains_key(&key)) || PENDING.with_borrow(|p| p.contains_key(&key));
    if !known && paths >= MAX_PATHS {
        return Err(HostError::new(format!(
            "setImage: {MAX_PATHS} paths already show pictures from memory; reuse a path, or clear one with setImage(path, null)"
        )));
    }

    let call = CALLS.with(|c| {
        c.set(c.get() + 1);
        c.get()
    });
    let superseded = Arc::new(AtomicBool::new(false));
    PENDING.with_borrow_mut(|pending| {
        let newer = Pending { copy, call, superseded: superseded.clone() };
        if let Some(older) = pending.insert(key.clone(), newer) {
            older.superseded.store(true, Ordering::Relaxed);
        }
    });
    RESERVED.set(RESERVED.get() + reserve);

    let (done, result) = async_channel::bounded(1);
    let installed = key.clone();
    let started = gpui_shell::with_current_app(|cx| {
        let background = cx.background_executor().clone();
        cx.foreground_executor()
            .spawn(async move {
                let skip = superseded.clone();
                let converted = background
                    .spawn(async move { if skip.load(Ordering::Relaxed) { None } else { Some(convert(pixels)) } })
                    .await;
                // Back on the UI thread, where the stores are.
                RESERVED.set(RESERVED.get() - reserve);
                let _ = done.try_send(commit(&installed, copy, call, owner, converted));
            })
            .detach();
    });
    if started.is_none() {
        PENDING.with_borrow_mut(|pending| pending.remove(&key));
        RESERVED.set(RESERVED.get() - reserve);
        return Err(HostError::new("setImage: called outside a script call"));
    }
    Ok(Box::pin(async move {
        result.recv().await.map_err(|_| HostError::new("setImage: the picture was lost"))?.map(HostValue::Bool).map_err(HostError::new)
    }))
}

/// Installs a finished call's picture if it is still its path's newest call
/// and its application is live; true if it did.
fn commit(
    key: &SharedString,
    copy: u64,
    call: u64,
    owner: Owner,
    converted: Option<Result<Arc<RenderImage>, String>>,
) -> Result<bool, String> {
    let newest = PENDING.with_borrow_mut(|pending| {
        let newest = pending.get(key).is_some_and(|p| p.call == call);
        if newest {
            pending.remove(key);
        }
        newest
    });
    let Some(converted) = converted else { return Ok(false) };
    let image = converted?;
    if !newest || !owner.is_live() {
        return Ok(false);
    }
    let size = image.size(0);
    let bytes = size.width.0 as usize * size.height.0 as usize * 4;
    SHOWN.with_borrow_mut(|shown| shown.insert(key.clone(), Shown { copy, owner: owner.clone(), bytes }));
    live_image::set(key.clone(), image, owner);
    Ok(true)
}

/// Clears a path's picture and pending call, if this copy set them.
fn clear(copy: u64, key: &SharedString) {
    PENDING.with_borrow_mut(|pending| {
        if pending.get(key).is_some_and(|p| p.copy == copy)
            && let Some(p) = pending.remove(key)
        {
            p.superseded.store(true, Ordering::Relaxed);
        }
    });
    SHOWN.with_borrow_mut(|shown| {
        if shown.get(key).is_some_and(|s| s.copy == copy) {
            shown.remove(key);
            live_image::clear(key);
        }
    });
}

/// Drops pictures whose application was retired without the copy going.
fn prune() {
    SHOWN.with_borrow_mut(|shown| shown.retain(|_, s| s.owner.is_live()));
}

/// The store key for `path`, which must be inside `mods/<name>/`.
fn own_key(name: &str, path: &str) -> Result<SharedString, HostError> {
    let refuse = || HostError::new(format!("setImage: `{path}` isn't a path in this mod's folder, mods/{name}/"));
    let key = live_image::key(path).ok_or_else(refuse)?;
    let mut parts = std::path::Path::new(key.as_ref()).components();
    let inside = parts.next().is_some_and(|p| p.as_os_str() == "mods")
        && parts.next().is_some_and(|p| p.as_os_str() == name)
        && parts.next().is_some();
    if inside { Ok(key) } else { Err(refuse()) }
}

fn pixels(bytes: &[u8], options: Option<&HostValue>) -> Result<Pixels, HostError> {
    let options = match options {
        None | Some(HostValue::Null) => None,
        Some(HostValue::Object(fields)) => Some(fields),
        Some(other) => return Err(HostError::new(format!("setImage: options must be an object, not {}", other.describe()))),
    };
    let Some(options) = options else {
        if bytes.len() > MAX_ENCODED {
            return Err(HostError::new(format!("setImage: an encoded picture is at most {MAX_ENCODED} bytes; this one is {}", bytes.len())));
        }
        let format = match image::guess_format(bytes) {
            Ok(format @ (image::ImageFormat::Png | image::ImageFormat::Jpeg)) => format,
            _ => return Err(HostError::new("setImage: bytes without options must be a PNG or JPEG")),
        };
        return Ok(Pixels::Encoded { bytes: bytes.to_vec(), format });
    };
    if let Some((name, _)) = options.iter().find(|(name, _)| !matches!(name.as_str(), "width" | "height" | "format")) {
        return Err(HostError::new(format!("setImage: unknown option `{name}`; raw pixels take width, height and format")));
    }
    let option = |name: &str| options.iter().find(|(n, _)| n == name).map(|(_, v)| v);
    let side = |name: &str| -> Result<u32, HostError> {
        match option(name).and_then(HostValue::as_number) {
            Some(n) if n.fract() == 0.0 && (1.0..=MAX_SIDE as f64).contains(&n) => Ok(n as u32),
            _ => Err(HostError::new(format!("setImage: raw pixels need a whole `{name}` from 1 to {MAX_SIDE}"))),
        }
    };
    let (width, height) = (side("width")?, side("height")?);
    let channels = match option("format").and_then(HostValue::as_str) {
        Some("rgb") => 3,
        Some("rgba") => 4,
        _ => return Err(HostError::new("setImage: raw pixels need `format`: \"rgb\" or \"rgba\"")),
    };
    let expected = width as usize * height as usize * channels;
    if bytes.len() != expected {
        return Err(HostError::new(format!(
            "setImage: {width}×{height} {} is {expected} bytes; got {}",
            if channels == 3 { "rgb" } else { "rgba" },
            bytes.len()
        )));
    }
    Ok(Pixels::Raw { width, height, channels, bytes: bytes.to_vec() })
}

/// An encoded picture's size from its header, before any decoding.
fn header(bytes: &[u8], format: image::ImageFormat) -> Result<(u32, u32), HostError> {
    let (width, height) = image::ImageReader::with_format(std::io::Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|e| HostError::new(format!("setImage: {e}")))?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(HostError::new(format!("setImage: a picture is 1 to {MAX_SIDE} pixels a side; this one is {width}×{height}")));
    }
    Ok((width, height))
}

/// To GPUI's BGRA (straight alpha), as its own image decoder does.
fn convert(pixels: Pixels) -> Result<Arc<RenderImage>, String> {
    let buffer = match pixels {
        Pixels::Raw { width, height, channels: 4, mut bytes } => {
            for px in bytes.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            image::RgbaImage::from_raw(width, height, bytes)
        }
        Pixels::Raw { width, height, bytes, .. } => {
            let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
            for px in bytes.chunks_exact(3) {
                bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
            image::RgbaImage::from_raw(width, height, bgra)
        }
        Pixels::Encoded { bytes, format } => {
            let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_SIDE);
            limits.max_image_height = Some(MAX_SIDE);
            reader.limits(limits);
            let mut rgba = reader.decode().map_err(|e| format!("setImage: {e}"))?.into_rgba8();
            for px in rgba.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            Some(rgba)
        }
    }
    .ok_or("setImage: the pixels don't fill the picture")?;
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])))
}
