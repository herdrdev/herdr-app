//! The quick terminal (macOS): a system-wide key in every layout, and a drop-down window for layouts with `"dropdown": true`.
//! The drop-down is settled once at start: a window's kind can't change while it's open.

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

/// The window server's blur behind the see-through drop-down, at `blur` px.
/// After GPUI's own (which sets the system's radius) and on every drop-down.
pub fn blur(cx: &App) {
    #[cfg(target_os = "macos")]
    later(cx, apply_blur);
    let _ = cx;
}

#[cfg(target_os = "macos")]
fn apply_blur(panel: gpui_macos::QuickPanel) {
    let s = crate::settings::get();
    if s.glass && s.blur > 0.0 {
        panel.blur(s.blur.round() as i64);
    }
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
    panel.show(q.height.clamp(0.1, 1.0), q.fullscreen, q.slide);
    apply_blur(panel);
}

/// settings.json's height or fullscreen changed: a drop-down that's down takes it now.
pub fn refit(cx: &App) {
    #[cfg(target_os = "macos")]
    later(cx, |panel| {
        let q = wanted();
        panel.refit(q.height.clamp(0.1, 1.0), q.fullscreen);
    });
    let _ = cx;
}

/// Whether an edge of the drop-down is being dragged.
pub fn resizing() -> bool {
    #[cfg(target_os = "macos")]
    {
        PANEL.get().is_some_and(|panel| panel.resizing())
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The height to write to settings.json after the drop-down was resized, or None when it already says so.
pub fn resized_height() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        let q = wanted();
        let share = PANEL.get()?.height_share()?;
        height_to_save(share, q.height, q.fullscreen)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// A dragged share in hundredths, the way people write it; None for a slide, a move to another screen, or full screen.
fn height_to_save(share: f64, saved: f64, fullscreen: bool) -> Option<f64> {
    let height = ((share * 100.0).round() / 100.0).clamp(0.1, 1.0);
    (!fullscreen && (height - saved).abs() > 0.005).then_some(height)
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
    // macOS reports shift with a symbol key as the symbol it types (cmd-shift-. arrives as `cmd->`), so a recorded key comes in that form.
    let (name, shift) = match SHIFTED.iter().find(|(typed, _)| *typed == k.key) {
        Some((_, base)) => (*base, true),
        None => (k.key.as_str(), m.shift),
    };
    // Every app loses this key: one that types (shift-a, ?) would toggle Herdr while typing, and ctrl-alt-shift-escape is Herdr's own reset.
    let f_key = name.strip_prefix('f').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let types = !(m.platform || m.control || m.alt) && name != "escape" && !f_key;
    if types || (name == "escape" && m.control && m.alt && shift && !m.platform) {
        return None;
    }
    let code = KEYS.iter().find(|(key, _)| *key == name)?.1;
    let bits = [(m.platform, 1 << 8), (shift, 1 << 9), (m.alt, 1 << 11), (m.control, 1 << 12)];
    Some((code, bits.iter().filter(|(held, _)| *held).map(|(_, bit)| bit).sum()))
}

/// What shift types on a US keyboard's symbol keys, and the key underneath.
const SHIFTED: &[(&str, &str)] = &[
    ("!", "1"), ("@", "2"), ("#", "3"), ("$", "4"), ("%", "5"), ("^", "6"), ("&", "7"), ("*", "8"), ("(", "9"), (")", "0"),
    ("_", "-"), ("+", "="), ("{", "["), ("}", "]"), ("|", "\\"), (":", ";"), ("\"", "'"), ("<", ","), (">", "."), ("?", "/"), ("~", "`"),
];

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
        assert_eq!(keycode("cmd->"), Some((0x2f, 256 + 512)));
        assert_eq!(keycode("cmd-!"), keycode("cmd-shift-1"));
        assert_eq!(keycode("shift-escape"), Some((0x35, 512)));
        assert_eq!(keycode("?"), None);
        assert_eq!(keycode("shift-a"), None);
        assert_eq!(keycode("a"), None);
        assert_eq!(keycode("f"), None);
        assert_eq!(keycode("shift-f5"), Some((0x60, 512)));
        assert_eq!(keycode("ctrl-alt-shift-escape"), None);
        assert_eq!(keycode("ctrl-alt-escape"), Some((0x35, 4096 + 2048)));
        // keys.json's form only.
        assert_eq!(keycode("cmd+."), None);
        assert_eq!(keycode("cmd-hyper"), None);
        assert_eq!(keycode("fn-f1"), None);
        assert_eq!(keycode(""), None);
    }

    #[test]
    fn a_drag_saves_the_new_height_in_hundredths_only_when_it_changed() {
        assert_eq!(height_to_save(0.3712, 0.5, false), Some(0.37));
        assert_eq!(height_to_save(1.0, 0.5, false), Some(1.0));
        assert_eq!(height_to_save(0.04, 0.5, false), Some(0.1));
        // A slide or another screen of a different size keeps the share.
        assert_eq!(height_to_save(0.5004, 0.5, false), None);
        assert_eq!(height_to_save(0.37, 0.37, false), None);
        assert_eq!(height_to_save(0.38, 0.37, false), Some(0.38));
        // Full screen has no height of its own to save.
        assert_eq!(height_to_save(0.8, 0.5, true), None);
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
        assert!(SHIFTED.iter().all(|(_, base)| KEYS.iter().any(|(key, _)| key == base)));
    }
}
