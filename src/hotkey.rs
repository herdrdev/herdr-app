//! The system-wide key (settings.json `hotkey`): it shows or hides Herdr from
//! any app by running `app.toggle`. macOS registers a Carbon hot key (no
//! Accessibility permission), Windows `RegisterHotKey` on a thread of its own.
//! Linux has no portable one (Wayland gives apps none): people bind
//! `herdr-app do app.toggle` in their desktop's keyboard settings instead.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The key `"hotkey": true` uses: Cmd+` on macOS, Ctrl+` elsewhere.
pub const DEFAULT: &str = if cfg!(target_os = "macos") { "cmd-`" } else { "ctrl-`" };

/// settings.json `hotkey`: off (absent or `false`), on with [`DEFAULT`]
/// (`true`), or a key in keys.json's form (`"ctrl-alt-space"`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Setting {
    #[default]
    Off,
    Default,
    Key(String),
}

impl Setting {
    pub fn is_off(&self) -> bool {
        *self == Setting::Off
    }

    /// The key to register, if any.
    pub fn key(&self) -> Option<&str> {
        match self {
            Setting::Off => None,
            Setting::Default => Some(DEFAULT),
            Setting::Key(k) => Some(k),
        }
    }
}

impl Serialize for Setting {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Setting::Off => s.serialize_bool(false),
            Setting::Default => s.serialize_bool(true),
            Setting::Key(k) => s.serialize_str(k),
        }
    }
}

impl<'de> Deserialize<'de> for Setting {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Null | serde_json::Value::Bool(false) => Ok(Setting::Off),
            serde_json::Value::Bool(true) => Ok(Setting::Default),
            serde_json::Value::String(k) => Ok(Setting::Key(k)),
            _ => Err(serde::de::Error::custom("hotkey must be true, false or a key like \"ctrl-alt-space\"")),
        }
    }
}

/// A system-wide key: its modifiers and the key's name as keys.json writes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    pub name: String,
}

pub fn parse(text: &str) -> Result<Key, String> {
    let bad = || format!("{text:?} is not a key like \"{DEFAULT}\" or \"ctrl-alt-space\"");
    let k = gpui::Keystroke::parse(text).map_err(|_| bad())?;
    let m = k.modifiers;
    if m.function {
        return Err(format!("{text:?}: fn can't be part of a system-wide key"));
    }
    let f_key = k.key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n));
    // A bare key (or shift and one) would stop that key from typing in every app.
    if !(m.control || m.alt || m.platform || f_key) {
        return Err(format!("{text:?} needs ctrl, alt or cmd (a key alone would stop typing it in every app)"));
    }
    if mac_code(&k.key).is_none() && win_vk(&k.key).is_none() {
        return Err(bad());
    }
    Ok(Key { ctrl: m.control, alt: m.alt, shift: m.shift, cmd: m.platform, name: k.key })
}

/// Registers `key` system-wide in place of the one before (none: no key).
/// Setting the same key again does nothing.
pub fn set(key: Option<&str>) -> Result<(), String> {
    let key = key.map(parse).transpose()?;
    platform::set(key.as_ref())
}

/// The key was pressed: the window shows or hides itself.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn pressed() {
    crate::mods::request(crate::mods::Request::Run { name: "app.toggle".into(), args: serde_json::Value::Null });
}

/// macOS virtual key codes (Events.h `kVK_*`): positions on a US (ANSI)
/// keyboard, so on other layouts a key is the one in that place.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_code(name: &str) -> Option<u32> {
    const LETTERS: [u32; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F, 0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20,
        0x09, 0x0D, 0x07, 0x10, 0x06,
    ];
    const DIGITS: [u32; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    const F_KEYS: [u32; 20] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71, 0x6A, 0x40, 0x4F, 0x50, 0x5A,
    ];
    if let Some(i) = single(name, b'a', b'z') {
        return Some(LETTERS[i]);
    }
    if let Some(i) = single(name, b'0', b'9') {
        return Some(DIGITS[i]);
    }
    if let Some(n) = f_number(name) {
        return F_KEYS.get(n - 1).copied();
    }
    Some(match name {
        "`" => 0x32,
        "-" => 0x1B,
        "=" => 0x18,
        "[" => 0x21,
        "]" => 0x1E,
        "\\" => 0x2A,
        ";" => 0x29,
        "'" => 0x27,
        "," => 0x2B,
        "." => 0x2F,
        "/" => 0x2C,
        "space" => 0x31,
        "tab" => 0x30,
        "enter" => 0x24,
        "escape" => 0x35,
        "backspace" => 0x33,
        "delete" => 0x75,
        "home" => 0x73,
        "end" => 0x77,
        "pageup" => 0x74,
        "pagedown" => 0x79,
        "left" => 0x7B,
        "right" => 0x7C,
        "down" => 0x7D,
        "up" => 0x7E,
        _ => return None,
    })
}

/// Windows virtual-key codes, US layout (Windows itself maps punctuation to
/// the person's layout first, see `platform::vk`).
#[cfg_attr(not(windows), allow(dead_code))]
fn win_vk(name: &str) -> Option<u16> {
    if single(name, b'a', b'z').is_some() || single(name, b'0', b'9').is_some() {
        return Some(name.as_bytes()[0].to_ascii_uppercase() as u16);
    }
    if let Some(n) = f_number(name) {
        return (n <= 24).then_some(0x6F + n as u16);
    }
    Some(match name {
        "`" => 0xC0,
        "-" => 0xBD,
        "=" => 0xBB,
        "[" => 0xDB,
        "]" => 0xDD,
        "\\" => 0xDC,
        ";" => 0xBA,
        "'" => 0xDE,
        "," => 0xBC,
        "." => 0xBE,
        "/" => 0xBF,
        "space" => 0x20,
        "tab" => 0x09,
        "enter" => 0x0D,
        "escape" => 0x1B,
        "backspace" => 0x08,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        _ => return None,
    })
}

/// The index of a one-character name in `first..=last`.
fn single(name: &str, first: u8, last: u8) -> Option<usize> {
    match name.as_bytes() {
        [c] if (first..=last).contains(c) => Some((c - first) as usize),
        _ => None,
    }
}

fn f_number(name: &str) -> Option<usize> {
    let n: usize = name.strip_prefix('f')?.parse().ok()?;
    (n >= 1).then_some(n)
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn taken(key: &Key) -> String {
    let mut parts = Vec::new();
    let cmd = if cfg!(target_os = "macos") { "cmd" } else { "win" };
    for (on, name) in [(key.ctrl, "ctrl"), (key.alt, "alt"), (key.shift, "shift"), (key.cmd, cmd)] {
        if on {
            parts.push(name);
        }
    }
    parts.push(&key.name);
    format!("{} is taken by another app; pick another key", parts.join("-"))
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Key;
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::ptr::null_mut;

    type OSStatus = i32;
    type Handler = extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> OSStatus;

    #[repr(C)]
    struct EventTypeSpec {
        class: u32,
        kind: u32,
    }

    #[repr(C)]
    struct EventHotKeyID {
        signature: u32,
        id: u32,
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> *mut c_void;
        fn InstallEventHandler(
            target: *mut c_void,
            handler: Handler,
            count: usize,
            types: *const EventTypeSpec,
            data: *mut c_void,
            out: *mut *mut c_void,
        ) -> OSStatus;
        fn RegisterEventHotKey(code: u32, modifiers: u32, id: EventHotKeyID, target: *mut c_void, options: u32, out: *mut *mut c_void) -> OSStatus;
        fn UnregisterEventHotKey(key: *mut c_void) -> OSStatus;
    }

    const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const HOT_KEY_PRESSED: u32 = 5;
    const SIGNATURE: u32 = u32::from_be_bytes(*b"hrdr");
    // Carbon's modifier bits (cmdKey, shiftKey, optionKey, controlKey).
    const CMD: u32 = 1 << 8;
    const SHIFT: u32 = 1 << 9;
    const OPTION: u32 = 1 << 11;
    const CONTROL: u32 = 1 << 12;
    const EXISTS: OSStatus = -9878; // eventHotKeyExistsErr

    thread_local! {
        // Carbon hot keys live on the main thread, where settings are applied.
        static CURRENT: RefCell<Option<(Key, *mut c_void)>> = const { RefCell::new(None) };
        static HANDLER: Cell<bool> = const { Cell::new(false) };
    }

    extern "C" fn pressed(_: *mut c_void, _: *mut c_void, _: *mut c_void) -> OSStatus {
        super::pressed();
        0
    }

    pub fn set(key: Option<&Key>) -> Result<(), String> {
        CURRENT.with(|current| {
            let mut current = current.borrow_mut();
            if current.as_ref().map(|(k, _)| k) == key {
                return Ok(());
            }
            if let Some((_, old)) = current.take() {
                unsafe { UnregisterEventHotKey(old) };
            }
            let Some(key) = key else { return Ok(()) };
            let code = super::mac_code(&key.name).ok_or_else(|| format!("{:?} is not a key on a Mac keyboard", key.name))?;
            if !HANDLER.get() {
                let spec = EventTypeSpec { class: KEYBOARD, kind: HOT_KEY_PRESSED };
                let status = unsafe { InstallEventHandler(GetApplicationEventTarget(), pressed, 1, &spec, null_mut(), null_mut()) };
                if status != 0 {
                    return Err(format!("macOS refused the key handler (error {status})"));
                }
                HANDLER.set(true);
            }
            let modifiers = [(key.cmd, CMD), (key.shift, SHIFT), (key.alt, OPTION), (key.ctrl, CONTROL)]
                .iter()
                .filter(|(on, _)| *on)
                .fold(0, |m, (_, bit)| m | bit);
            let mut out = null_mut();
            let id = EventHotKeyID { signature: SIGNATURE, id: 1 };
            match unsafe { RegisterEventHotKey(code, modifiers, id, GetApplicationEventTarget(), 0, &mut out) } {
                0 => {
                    *current = Some((key.clone(), out));
                    Ok(())
                }
                EXISTS => Err(super::taken(key)),
                status => Err(format!("macOS refused the key (error {status})")),
            }
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::Key;
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey, VkKeyScanW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT, WM_USER};

    const ALREADY_REGISTERED: u32 = 1409; // ERROR_HOTKEY_ALREADY_REGISTERED

    /// The thread that holds the key: a hot key belongs to the thread that
    /// registered it, and arrives in that thread's message queue.
    struct Holder {
        key: Key,
        thread: u32,
        join: std::thread::JoinHandle<()>,
    }

    static CURRENT: Mutex<Option<Holder>> = Mutex::new(None);

    /// The key and whether it needs Shift: punctuation as the person's
    /// keyboard layout types it (`/` is Shift+7 on German), else US.
    fn vk(name: &str) -> Option<(u32, bool)> {
        let mut chars = name.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && !c.is_ascii_alphanumeric()
            && (c as u32) < 0x10000
        {
            let scan = unsafe { VkKeyScanW(c as u16) } as u16;
            // High byte: 1 Shift, 2 Ctrl, 4 Alt. Ctrl+Alt is AltGr, which a
            // hot key can't hold apart from the modifiers asked for: use US.
            if scan != 0xffff && (scan >> 8) & 6 == 0 {
                return Some(((scan & 0xff) as u32, (scan >> 8) & 1 == 1));
            }
        }
        super::win_vk(name).map(|vk| (u32::from(vk), false))
    }

    pub fn set(key: Option<&Key>) -> Result<(), String> {
        let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().map(|h| &h.key) == key {
            return Ok(());
        }
        if let Some(old) = current.take() {
            unsafe { PostThreadMessageW(old.thread, WM_QUIT, 0, 0) };
            let _ = old.join.join();
        }
        let Some(key) = key else { return Ok(()) };
        let (vk, shifted) = vk(&key.name).ok_or_else(|| format!("{:?} is not a key on this keyboard", key.name))?;
        let modifiers = [(key.alt, MOD_ALT), (key.ctrl, MOD_CONTROL), (key.shift || shifted, MOD_SHIFT), (key.cmd, MOD_WIN)]
            .iter()
            .filter(|(on, _)| *on)
            .fold(MOD_NOREPEAT, |m, (_, bit)| m | bit);
        let (tx, rx) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("herdr-hotkey".into())
            .spawn(move || {
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                // Makes this thread's message queue, so WM_QUIT can reach it.
                unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), WM_USER, WM_USER, PM_NOREMOVE) };
                if unsafe { RegisterHotKey(std::ptr::null_mut(), 1, modifiers, vk) } == 0 {
                    let _ = tx.send(Err(unsafe { GetLastError() }));
                    return;
                }
                let _ = tx.send(Ok(unsafe { GetCurrentThreadId() }));
                while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                    if msg.message == WM_HOTKEY {
                        super::pressed();
                    }
                }
                unsafe { UnregisterHotKey(std::ptr::null_mut(), 1) };
            })
            .map_err(|e| format!("cannot start the key's thread: {e}"))?;
        match rx.recv() {
            Ok(Ok(thread)) => {
                *current = Some(Holder { key: key.clone(), thread, join });
                Ok(())
            }
            Ok(Err(ALREADY_REGISTERED)) => Err(super::taken(key)),
            Ok(Err(code)) => Err(format!("Windows refused the key (error {code})")),
            Err(_) => Err("the key's thread ended".into()),
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::Key;

    pub fn set(key: Option<&Key>) -> Result<(), String> {
        match key {
            None => Ok(()),
            Some(_) => Err("works on macOS and Windows; on Linux, bind `herdr-app do app.toggle` to a key in your desktop's keyboard settings".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_is_off_unless_turned_on_and_round_trips() {
        assert_eq!(Setting::default(), Setting::Off);
        assert_eq!(Setting::Off.key(), None);
        assert_eq!(Setting::Default.key(), Some(DEFAULT));
        for (json, setting) in [
            (serde_json::json!(false), Setting::Off),
            (serde_json::json!(true), Setting::Default),
            (serde_json::json!("ctrl-alt-space"), Setting::Key("ctrl-alt-space".into())),
        ] {
            assert_eq!(serde_json::from_value::<Setting>(json.clone()).unwrap(), setting);
            assert_eq!(serde_json::to_value(&setting).unwrap(), json);
        }
        assert_eq!(serde_json::from_value::<Setting>(serde_json::Value::Null).unwrap(), Setting::Off);
        let error = serde_json::from_value::<Setting>(serde_json::json!(3)).unwrap_err().to_string();
        assert!(error.contains("hotkey must be true, false or a key"), "{error}");
    }

    #[test]
    fn the_default_is_cmd_backtick_on_macos_and_ctrl_backtick_elsewhere() {
        let key = parse(DEFAULT).unwrap();
        assert_eq!(key.name, "`");
        assert_eq!((key.cmd, key.ctrl), (cfg!(target_os = "macos"), !cfg!(target_os = "macos")));
        assert_eq!(mac_code("`"), Some(0x32));
        assert_eq!(win_vk("`"), Some(0xC0));
    }

    #[test]
    fn keys_parse_like_keys_json_and_need_a_modifier() {
        assert_eq!(parse("ctrl-alt-space").unwrap(), Key { ctrl: true, alt: true, shift: false, cmd: false, name: "space".into() });
        assert_eq!(parse("cmd-shift-.").unwrap(), Key { ctrl: false, alt: false, shift: true, cmd: true, name: ".".into() });
        assert!(parse("win-`").unwrap().cmd);
        assert_eq!(parse("f12").unwrap().name, "f12");
        for bad in ["`", "shift-a", "space", "fn-f1", "ctrl-", "ctrl-nosuchkey", "ctrl-f25", "cmd-a-b"] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
        assert!(parse("`").unwrap_err().contains("needs ctrl, alt or cmd"));
    }

    #[test]
    fn every_key_has_a_code_on_both_systems() {
        let names: Vec<String> = ('a'..='z')
            .chain('0'..='9')
            .map(String::from)
            .chain((1..=20).map(|n| format!("f{n}")))
            .chain(
                ["`", "-", "=", "[", "]", "\\", ";", "'", ",", ".", "/", "space", "tab", "enter", "escape", "backspace", "delete"]
                    .map(String::from),
            )
            .chain(["home", "end", "pageup", "pagedown", "left", "right", "up", "down"].map(String::from))
            .collect();
        for name in &names {
            assert!(mac_code(name).is_some(), "no macOS code for {name:?}");
            assert!(win_vk(name).is_some(), "no Windows code for {name:?}");
        }
        let mut codes: Vec<u32> = names.iter().filter_map(|n| mac_code(n)).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), names.len(), "two keys share a macOS code");
        let mut vks: Vec<u16> = names.iter().filter_map(|n| win_vk(n)).collect();
        vks.sort();
        vks.dedup();
        assert_eq!(vks.len(), names.len(), "two keys share a Windows code");
        assert_eq!(win_vk("f24"), Some(0x87));
        assert_eq!(win_vk("insert"), Some(0x2D));
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn linux_says_how_to_bind_the_command_instead() {
        assert_eq!(set(None), Ok(()));
        assert!(set(Some("ctrl-`")).unwrap_err().contains("herdr-app do app.toggle"));
        assert!(set(Some("`")).unwrap_err().contains("needs ctrl, alt or cmd"));
    }
}
