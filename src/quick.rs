//! The quick terminal (macOS): with `quick_terminal` in settings.json the
//! window drops down from the top of the screen under the pointer, on every
//! Space, and a system-wide key shows and hides it from any app. Read once at
//! start: a window's kind can't change while it's open. The native side is in
//! gpui-pre-macos (`quick.rs`).

use std::cell::RefCell;
use std::sync::OnceLock;

use gpui::{App, Keystroke, Window, WindowKind, WindowOptions};

static ON: OnceLock<bool> = OnceLock::new();

thread_local! {
    /// The key registered last, and how that went.
    static KEY: RefCell<Option<(Option<String>, Result<(), String>)>> = const { RefCell::new(None) };
}

#[cfg(target_os = "macos")]
thread_local! {
    static PANEL: std::cell::Cell<Option<gpui_macos::QuickPanel>> = const { std::cell::Cell::new(None) };
}

/// Settles, at start, whether this run's window is the drop-down.
pub fn start(wanted: bool) {
    let _ = ON.set(cfg!(target_os = "macos") && wanted);
}

pub fn on() -> bool {
    ON.get().copied().unwrap_or(false)
}

/// The drop-down's window: a panel that opens hidden; `install` places it.
pub fn options(app_id: String) -> WindowOptions {
    WindowOptions {
        kind: WindowKind::PopUp,
        titlebar: None,
        show: false,
        focus: false,
        is_movable: false,
        app_id: Some(app_id),
        ..Default::default()
    }
}

/// Makes the window just opened the drop-down, and drops it down.
pub fn install(window: &Window, cx: &App) {
    #[cfg(target_os = "macos")]
    if let Some(panel) = gpui_macos::QuickPanel::new(window) {
        PANEL.set(Some(panel));
        cx.foreground_executor()
            .spawn(async move {
                panel.style();
                down(panel);
            })
            .detach();
    }
    let _ = (window, cx);
}

/// `app.toggle`: up when it's down and has the keys, else down.
pub fn toggle(cx: &App) {
    #[cfg(target_os = "macos")]
    later(cx, |panel| if panel.shown_and_key() { panel.hide(wanted().slide) } else { down(panel) });
    let _ = cx;
}

pub fn show(cx: &App) {
    #[cfg(target_os = "macos")]
    later(cx, down);
    let _ = cx;
}

pub fn hide(cx: &App) {
    #[cfg(target_os = "macos")]
    later(cx, |panel| panel.hide(wanted().slide));
    let _ = cx;
}

/// The window lost or got the keys: `auto_hide` puts it away.
pub fn focus_changed(active: bool, cx: &App) {
    if !active && crate::settings::get().quick_terminal.as_ref().is_some_and(|q| q.auto_hide) {
        hide(cx);
    }
}

/// Native window calls wait for a task of their own: a frame change or
/// makeKey runs GPUI callbacks, which can't run inside an update.
#[cfg(target_os = "macos")]
fn later(cx: &App, f: fn(gpui_macos::QuickPanel)) {
    if let Some(panel) = PANEL.get() {
        cx.foreground_executor().spawn(async move { f(panel) }).detach();
    }
}

/// Down (or to its new size, when it's down already) as settings.json says.
#[cfg(target_os = "macos")]
fn down(panel: gpui_macos::QuickPanel) {
    let q = wanted();
    panel.show(q.height.clamp(0.1, 1.0) as f64, q.fullscreen, q.slide);
}

#[cfg(target_os = "macos")]
fn wanted() -> crate::settings::QuickTerminal {
    crate::settings::get().quick_terminal.clone().unwrap_or_default()
}

/// Makes `key` the system-wide key that runs `app.toggle` (none: no key).
/// A key that isn't one is `problems()`' to report; this says when macOS
/// won't take it.
pub fn set_key(key: Option<&str>) -> Result<(), String> {
    KEY.with(|k| {
        let mut k = k.borrow_mut();
        if let Some((last, result)) = k.as_ref()
            && last.as_deref() == key
        {
            return result.clone();
        }
        let result = register(key.and_then(keycode));
        *k = Some((key.map(str::to_owned), result.clone()));
        result
    })
}

#[cfg(target_os = "macos")]
fn register(key: Option<(u32, u32)>) -> Result<(), String> {
    gpui_macos::set_hotkey(key, || crate::mods::request(crate::mods::Request::Run { name: "app.toggle".into(), args: serde_json::Value::Null }))
        .map_err(|e| format!("quick_terminal.key: {e}"))
}

#[cfg(not(target_os = "macos"))]
fn register(_: Option<(u32, u32)>) -> Result<(), String> {
    Ok(())
}

/// A key as keys.json writes it (`cmd-.`), as macOS registers it: the
/// virtual key code and Carbon's modifier bits.
pub fn keycode(key: &str) -> Option<(u32, u32)> {
    let k = Keystroke::parse(key).ok()?;
    let m = k.modifiers;
    if m.function {
        return None;
    }
    let code = KEYS.iter().find(|(name, _)| *name == k.key)?.1;
    let bits = [(m.platform, 1 << 8), (m.shift, 1 << 9), (m.alt, 1 << 11), (m.control, 1 << 12)];
    Some((code, bits.iter().filter(|(held, _)| *held).map(|(_, bit)| bit).sum()))
}

/// Key names to macOS virtual key codes (where the key sits on a US keyboard).
const KEYS: &[(&str, u32)] = &[
    ("a", 0x00), ("s", 0x01), ("d", 0x02), ("f", 0x03), ("h", 0x04), ("g", 0x05), ("z", 0x06), ("x", 0x07),
    ("c", 0x08), ("v", 0x09), ("b", 0x0b), ("q", 0x0c), ("w", 0x0d), ("e", 0x0e), ("r", 0x0f), ("y", 0x10),
    ("t", 0x11), ("1", 0x12), ("2", 0x13), ("3", 0x14), ("4", 0x15), ("6", 0x16), ("5", 0x17), ("=", 0x18),
    ("9", 0x19), ("7", 0x1a), ("-", 0x1b), ("8", 0x1c), ("0", 0x1d), ("]", 0x1e), ("o", 0x1f), ("u", 0x20),
    ("[", 0x21), ("i", 0x22), ("p", 0x23), ("enter", 0x24), ("l", 0x25), ("j", 0x26), ("'", 0x27), ("k", 0x28),
    (";", 0x29), ("\\", 0x2a), (",", 0x2b), ("/", 0x2c), ("n", 0x2d), ("m", 0x2e), (".", 0x2f), ("tab", 0x30),
    ("space", 0x31), ("`", 0x32), ("backspace", 0x33), ("escape", 0x35),
    ("f1", 0x7a), ("f2", 0x78), ("f3", 0x63), ("f4", 0x76), ("f5", 0x60), ("f6", 0x61), ("f7", 0x62), ("f8", 0x64),
    ("f9", 0x65), ("f10", 0x6d), ("f11", 0x67), ("f12", 0x6f), ("f13", 0x69), ("f14", 0x6b), ("f15", 0x71),
    ("f16", 0x6a), ("f17", 0x40), ("f18", 0x4f), ("f19", 0x50), ("f20", 0x5a),
    ("home", 0x73), ("pageup", 0x74), ("delete", 0x75), ("end", 0x77), ("pagedown", 0x79),
    ("left", 0x7b), ("right", 0x7c), ("down", 0x7d), ("up", 0x7e),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_turn_into_macos_codes_and_modifiers() {
        assert_eq!(keycode("cmd-."), Some((0x2f, 256)));
        assert_eq!(keycode("ctrl-alt-space"), Some((0x31, 4096 + 2048)));
        assert_eq!(keycode("cmd-shift-t"), Some((0x11, 256 + 512)));
        assert_eq!(keycode("cmd--"), Some((0x1b, 256)));
        assert_eq!(keycode("f12"), Some((0x6f, 0)));
        // keys.json's form only.
        assert_eq!(keycode("cmd+."), None);
        assert_eq!(keycode("cmd-hyper"), None);
        assert_eq!(keycode("fn-f1"), None);
        assert_eq!(keycode(""), None);
    }

    #[test]
    fn every_key_name_is_listed_once() {
        let mut names: Vec<&str> = KEYS.iter().map(|(n, _)| *n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), KEYS.len());
        let mut codes: Vec<u32> = KEYS.iter().map(|(_, c)| *c).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), KEYS.len());
    }
}
