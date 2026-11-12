//! `herdr-app do <command> [--arg value ... | json]`, `herdr-app ls`, `herdr-app commands`
//! and `herdr-app status`: drive a running window from a shell (agents, tests)
//! over `$HERDR_APP_HOME/herdr-app.sock`, one JSON line each way.

use crate::mods::{self, Request};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

fn socket() -> PathBuf {
    mods::app_dir().join("herdr-app.sock")
}

/// Serves the socket for this window. A live socket means another window owns this home.
pub fn listen(tx: async_channel::Sender<Request>) {
    let path = socket();
    if UnixStream::connect(&path).is_ok() {
        tracing::warn!("another Herdr is serving {}; `herdr-app do` goes there", path.display());
        return;
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => return tracing::warn!("cannot serve {}: {e}", path.display()),
    };
    let _ = std::thread::Builder::new().name("herdr-app-control".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || serve(stream, tx));
        }
    });
}

fn serve(stream: UnixStream, tx: async_channel::Sender<Request>) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Value>(&line) {
        Ok(cmd) => {
            let timeout = Duration::from_secs(if cmd.get("shot").is_some() { 30 } else { 5 });
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            let _ = tx.send_blocking(Request::Control { cmd, reply: reply_tx });
            reply_rx.recv_timeout(timeout).unwrap_or_else(|_| json!({ "error": "the window did not answer" }))
        }
        Err(e) => json!({ "error": format!("bad request: {e}") }),
    };
    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
}

/// One Herdr per home: a second launch brings the running window to the
/// front and quits. True when one is running (even one too busy to answer:
/// a second window would ring every alert twice).
pub fn raise_running() -> bool {
    raise_running_at(&socket())
}

fn raise_running_at(path: &std::path::Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(path) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    if stream.write_all(format!("{}\n", json!({ "raise": true })).as_bytes()).is_ok() {
        let _ = BufReader::new(&stream).read_line(&mut String::new());
    }
    true
}

/// The client side; returns the process exit code.
pub fn client(args: &[String]) -> i32 {
    if args == ["commands"] {
        println!("{}", serde_json::to_string_pretty(&crate::commands::listing()).unwrap());
        return 0;
    }
    let cmd = match args {
        [verb, rest @ ..] if verb == "shot" && rest.len() <= 1 => {
            let path = PathBuf::from(rest.first().map(String::as_str).unwrap_or("herdr-shot.png"));
            let path = match std::path::absolute(path) {
                Ok(path) => path,
                Err(e) => return fail(&format!("cannot resolve screenshot path: {e}")),
            };
            if path.extension().is_none_or(|ext| !ext.eq_ignore_ascii_case("png")) {
                return fail("screenshot path must end in .png");
            }
            json!({ "shot": path })
        },
        [verb] if verb == "status" => json!({ "status": true }),
        [verb] if verb == "ls" => json!({ "ls": true }),
        [verb, message] if verb == "feedback" => json!({ "do": "app.feedback", "args": { "message": message, "from": "agent" } }),
        [verb, command, rest @ ..] if verb == "do" => match parse_args(rest) {
            Ok(a) => json!({ "do": command, "args": a }),
            Err(e) => return fail(&e),
        },
        _ => return fail("usage: herdr-app ls | herdr-app commands | herdr-app status | herdr-app shot [file.png] | herdr-app do <command> [--arg value ... | json]"),
    };
    let mut stream = match UnixStream::connect(socket()) {
        Ok(s) => s,
        Err(_) if args.first().is_some_and(|v| v == "feedback") => {
            let body = crate::feedback::draft(&args[1], "", 0, "agent", "other");
            println!("{}", serde_json::to_string_pretty(&body).unwrap_or_default());
            eprintln!("Herdr must be open to review and send this feedback. Nothing was sent; core version and layout will reflect the open window.");
            return 1;
        }
        Err(e) => return fail(&format!("no Herdr window at {}: {e}", socket().display())),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(35)));
    let mut line = String::new();
    if stream.write_all(format!("{cmd}\n").as_bytes()).is_err() || BufReader::new(&stream).read_line(&mut line).is_err() {
        return fail("the window hung up");
    }
    let reply: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
    if args.first().is_some_and(|v| v == "shot") {
        if let Some(error) = reply.get("error") {
            return fail(error.as_str().unwrap_or("screenshot failed"));
        }
        let Some(path) = reply.get("path").and_then(Value::as_str) else {
            return fail("the window returned no screenshot path");
        };
        if let Some(why) = reply.get("left_out").and_then(Value::as_str) {
            eprintln!("herdr-app: the screenshot leaves out the native windows: {why}");
        }
        println!("{path}");
    } else {
        println!("{}", serde_json::to_string_pretty(&reply).unwrap_or_default());
    }
    if reply.get("error").is_some() { 1 } else { 0 }
}

/// `--name api --argv '["pi"]'` (values that parse as JSON are JSON), or one JSON object.
fn parse_args(rest: &[String]) -> Result<Value, String> {
    match rest {
        [] => Ok(Value::Null),
        [raw] if raw.trim_start().starts_with('{') => serde_json::from_str(raw).map_err(|e| format!("args are not JSON: {e}")),
        _ => {
            let mut out = serde_json::Map::new();
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                let key = flag.strip_prefix("--").ok_or_else(|| format!("expected --<arg>, got {flag:?}"))?;
                let value = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
                let v = serde_json::from_str::<Value>(value).ok().filter(|v| !v.is_string()).unwrap_or_else(|| Value::String(value.clone()));
                out.insert(key.to_owned(), v);
            }
            Ok(Value::Object(out))
        }
    }
}

/// `herdr-app shot`: the window's last frame, with the native windows shown
/// in it (on Windows: adopted programs and browser panes), saved as a PNG.
/// The frame is taken here; a thread of its own captures the native windows,
/// writes the file and answers `reply`.
pub fn shot(window: &gpui::Window, path: PathBuf, reply: std::sync::mpsc::Sender<Value>) {
    let image = match window.render_to_image() {
        Ok(image) => image,
        Err(error) => {
            let _ = reply.send(json!({ "error": format!("cannot save screenshot: {error:#}") }));
            return;
        }
    };
    #[cfg(windows)]
    let host = native_views::host(window);
    let answer_failure = reply.clone();
    let spawned = std::thread::Builder::new().name("herdr-shot".into()).spawn(move || {
        #[allow(unused_mut)]
        let mut image = image;
        #[cfg(windows)]
        let left_out = host.and_then(|host| native_views::add(host, &mut image));
        #[cfg(not(windows))]
        let left_out: Option<String> = None;
        let answer = match (save_png(&image, &path), left_out) {
            (Ok(()), Some(why)) => json!({ "path": path, "left_out": why }),
            (Ok(()), None) => json!({ "path": path }),
            (Err(error), _) => json!({ "error": format!("cannot save screenshot: {error:#}") }),
        };
        let _ = reply.send(answer);
    });
    if let Err(error) = spawned {
        let _ = answer_failure.send(json!({ "error": format!("cannot start the screenshot thread: {error}") }));
    }
}

/// Encode before opening the destination: GPU errors must not truncate a good shot.
fn save_png(image: &image::RgbaImage, path: &std::path::Path) -> anyhow::Result<()> {
    // Two shots of one process can be written at once.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        encoder.write_header()?.write_image_data(image.as_raw())?;
    }
    // A sibling temporary file keeps publication on the same filesystem.
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = path.with_file_name(format!(".herdr-shot-{}-{n}.png", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Copy the `rects` (`[x, y, width, height]`, clipped to the frame) of a
/// captured top-down BGRA picture of the frame's size onto the RGBA frame.
#[cfg_attr(not(windows), allow(dead_code))]
fn paste_bgra(frame: &mut [u8], width: u32, height: u32, captured: &[u8], rects: &[[i32; 4]]) {
    let (w, h) = (width as i32, height as i32);
    for &[x, y, rw, rh] in rects {
        let (left, right) = (x.max(0), (x + rw).min(w));
        for row in y.max(0)..(y + rh).min(h) {
            for column in left..right {
                let i = ((row * w + column) * 4) as usize;
                frame[i] = captured[i + 2];
                frame[i + 1] = captured[i + 1];
                frame[i + 2] = captured[i];
                frame[i + 3] = 255;
            }
        }
    }
}

/// Native windows inside Herdr's window draw themselves: the GPU frame has
/// holes where they are. A picture of Herdr's window that the desktop
/// composes (`PrintWindow` with `PW_RENDERFULLCONTENT`, which also works
/// while other windows cover it) fills them.
#[cfg(windows)]
mod native_views {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::time::Duration;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT},
        Graphics::Gdi::*,
        Storage::Xps::{PW_CLIENTONLY, PrintWindow},
        UI::WindowsAndMessaging::{EnumChildWindows, GetClientRect, GetParent, GetWindowRect, IsIconic, IsWindowVisible, PW_RENDERFULLCONTENT},
    };

    /// How long the desktop may take to draw the picture.
    const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

    pub fn host(window: &gpui::Window) -> Option<usize> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as usize),
            _ => None,
        }
    }

    /// Paste the shown native windows into `frame`. Returns why they are left
    /// out, if they are.
    pub fn add(host: usize, frame: &mut image::RgbaImage) -> Option<String> {
        let rects = shown_children(host as HWND);
        if rects.is_empty() {
            return None;
        }
        if unsafe { IsIconic(host as HWND) } != 0 {
            return Some("Herdr is minimized, so its native windows are not drawn".into());
        }
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(host as HWND, &mut client) };
        let (width, height) = (frame.width(), frame.height());
        if (client.right, client.bottom) != (width as i32, height as i32) {
            return Some(format!("the window measures {}x{} pixels, its last frame {width}x{height}", client.right, client.bottom));
        }
        // PrintWindow can wait on the programs that own the windows: a busy
        // one must not hold the reply.
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("herdr-shot-capture".into()).spawn(move || {
            let _ = tx.send(capture(host as HWND, width as i32, height as i32));
        });
        if spawned.is_err() {
            return Some("cannot start the thread that captures the native windows".into());
        }
        let captured = match rx.recv_timeout(CAPTURE_TIMEOUT) {
            Ok(Some(captured)) => captured,
            Ok(None) => return Some("Windows did not draw the native windows".into()),
            Err(_) => return Some(format!("the native windows were not drawn within {} s", CAPTURE_TIMEOUT.as_secs())),
        };
        super::paste_bgra(frame, width, height, &captured, &rects);
        None
    }

    /// The window's client area as the desktop composes it, in top-down BGRA rows.
    fn capture(host: HWND, width: i32, height: i32) -> Option<Vec<u8>> {
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width;
            info.bmiHeader.biHeight = -height;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB;
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            let mut out = None;
            if !bitmap.is_null() {
                let previous = SelectObject(dc, bitmap);
                if PrintWindow(host, dc, PW_CLIENTONLY | PW_RENDERFULLCONTENT) != 0 {
                    GdiFlush();
                    out = Some(std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize).to_vec());
                }
                SelectObject(dc, previous);
                DeleteObject(bitmap);
            }
            DeleteDC(dc);
            out
        }
    }

    /// The visible parts of the host's shown child windows, in its client
    /// pixels: each window's rectangle, or the rectangles of its region.
    fn shown_children(host: HWND) -> Vec<[i32; 4]> {
        struct Found {
            host: HWND,
            rects: Vec<[i32; 4]>,
        }
        unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> windows_sys::core::BOOL {
            let found = unsafe { &mut *(data as *mut Found) };
            if unsafe { GetParent(hwnd) } != found.host || unsafe { IsWindowVisible(hwnd) } == 0 {
                return 1;
            }
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetWindowRect(hwnd, &mut r) };
            let mut origin = POINT { x: r.left, y: r.top };
            unsafe { ScreenToClient(found.host, &mut origin) };
            let parts = region_rects(hwnd, r.right - r.left, r.bottom - r.top);
            found.rects.extend(parts.into_iter().map(|[x, y, w, h]| [origin.x + x, origin.y + y, w, h]));
            1
        }
        let mut found = Found { host, rects: Vec::new() };
        unsafe { EnumChildWindows(host, Some(visit), (&raw mut found) as LPARAM) };
        found.rects
    }

    /// A window's visible rectangles relative to itself: all of it, or the
    /// rectangles of the region that cuts holes for Herdr's overlays.
    fn region_rects(hwnd: HWND, width: i32, height: i32) -> Vec<[i32; 4]> {
        unsafe {
            let region = CreateRectRgn(0, 0, 0, 0);
            let kind = GetWindowRgn(hwnd, region);
            let rects = if kind == NULLREGION as i32 {
                Vec::new()
            } else if kind == SIMPLEREGION as i32 || kind == COMPLEXREGION as i32 {
                let size = GetRegionData(region, 0, std::ptr::null_mut());
                // RGNDATA holds RECTs: align the buffer for them.
                let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
                let data = buffer.as_mut_ptr() as *mut RGNDATA;
                if size == 0 || GetRegionData(region, size, data) == 0 {
                    vec![[0, 0, width, height]]
                } else {
                    let first = (*data).Buffer.as_ptr() as *const RECT;
                    let parts = std::slice::from_raw_parts(first, (*data).rdh.nCount as usize);
                    parts.iter().map(|r| [r.left, r.top, r.right - r.left, r.bottom - r.top]).collect()
                }
            } else {
                // No region: all of the window shows.
                vec![[0, 0, width, height]]
            };
            DeleteObject(region);
            rects
        }
    }
}

fn fail(message: &str) -> i32 {
    eprintln!("herdr-app: {message}");
    2
}

#[cfg(test)]
mod shot_tests {
    use super::paste_bgra;

    #[test]
    fn native_windows_fill_only_their_rectangles_clipped_to_the_frame() {
        // A 3x2 frame of Herdr's own pixels (all 1s) and a capture whose BGRA
        // pixel at (column, row) is [column, row, 9, 0].
        let (width, height) = (3u32, 2u32);
        let mut frame = vec![1u8; 3 * 2 * 4];
        let captured: Vec<u8> = (0..2).flat_map(|row| (0..3).flat_map(move |column| [column, row, 9, 0])).collect();
        // One rectangle at (1, 0) runs past the right and bottom edges; one
        // starts left of the frame and is empty after clipping.
        paste_bgra(&mut frame, width, height, &captured, &[[1, 0, 5, 5], [-2, 0, 1, 2]]);
        let pixel = |column: usize, row: usize| frame[(row * 3 + column) * 4..][..4].to_vec();
        assert_eq!(pixel(0, 0), [1, 1, 1, 1]);
        assert_eq!(pixel(0, 1), [1, 1, 1, 1]);
        // RGBA from BGRA, opaque.
        assert_eq!(pixel(1, 0), [9, 0, 1, 255]);
        assert_eq!(pixel(2, 1), [9, 1, 2, 255]);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_second_launch_raises_the_running_window() {
        let dir = std::env::temp_dir().join(format!("herdr-app-raise-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("herdr-app.sock");
        // Nothing running: this launch opens its own window.
        assert!(!raise_running_at(&path));
        // One running: it gets asked to come to the front.
        let listener = UnixListener::bind(&path).unwrap();
        let asked = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            (&stream).write_all(b"{\"ok\":true}\n").unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        });
        assert!(raise_running_at(&path));
        assert_eq!(asked.join().unwrap(), json!({ "raise": true }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
