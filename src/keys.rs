//! ~/.config/herdr-app/keys.json: string or object actions, reloaded live.
//!
//! ```json
//! {
//!   "global":   { "ctrl-shift-t": "tab.new", "ctrl-1": "tab.select:1" },
//!   "terminal": { "ctrl-left": "send:\u001bb", "ctrl-shift-t": "terminal" },
//!   "layout:terminals": { "ctrl-]": "space.next" },
//!   "mod:editor": { "ctrl-s": "editor.save" }
//! }
//! ```
//!
//! An action is a command (`pane.split:right`: the value fills its first
//! argument), an object (`{"command":"layout.resize","args":{"id":"sidebar","px":240}}`),
//! `send:<text>` (types into the focused terminal),
//! `<mod>.<command>` (delivered to that mod's `nextCommand()`), or `terminal`
//! (the program gets the key: the binding is switched off where this section
//! applies).

use gpui::{App, KeyBinding, KeyBindingContextPredicate, NoAction};
use std::rc::Rc;
use herdr_shell::action::ShellAction;
use std::collections::BTreeMap;

/// The reserved shortcut that restores the default layout and keys.
pub const ESCAPE_HATCH: &str = "ctrl-alt-shift-escape";

/// The shipped keys: prefix-free; plain ctrl+letter always stays with the
/// program. Herdr uses ctrl+shift (defaults/keys.json), or cmd on macOS
/// (defaults/keys.macos.json, with Mac terminal editing keys).
#[cfg(not(target_os = "macos"))]
pub const DEFAULTS: &str = include_str!("../defaults/keys.json");
#[cfg(target_os = "macos")]
pub const DEFAULTS: &str = include_str!("../defaults/keys.macos.json");

// Exact originals from release tags, deduplicated across v0.1.4–v0.1.15.
// Keep these when defaults change: only untouched shipped files may migrate.
#[cfg(any(not(target_os = "macos"), test))]
const SHIPPED_KEYS: &[&str] = &[
    include_str!("keys/shipped/v0.1.4-keys.json"),
    include_str!("keys/shipped/v0.1.5-keys.json"),
    include_str!("keys/shipped/v0.1.12-keys.json"),
];
#[cfg(any(target_os = "macos", test))]
const SHIPPED_MACOS_KEYS: &[&str] = &[
    include_str!("keys/shipped/v0.1.4-keys.macos.json"),
    include_str!("keys/shipped/v0.1.5-keys.macos.json"),
    include_str!("keys/shipped/v0.1.15-keys.macos.json"),
];

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Binding {
    String(String),
    Object(serde_json::Map<String, serde_json::Value>),
}

type Sections = BTreeMap<String, BTreeMap<String, Binding>>;

/// Kept separate from string actions so colon payloads always stay literal.
#[derive(Clone, PartialEq, gpui::Action, serde::Deserialize)]
#[action(no_json)]
pub struct CommandBinding {
    pub value: serde_json::Map<String, serde_json::Value>,
}

impl CommandBinding {
    pub fn command(&self) -> Result<(String, serde_json::Value), String> {
        use serde_json::{Value, json};
        if let Some(key) = self.value.keys().find(|k| !matches!(k.as_str(), "command" | "args")) {
            return Err(format!("unknown key binding field {key:?}; use command and args"));
        }
        let name = self.value.get("command").and_then(Value::as_str)
            .ok_or("key binding command must be a string")?;
        let name = current(name);
        let command = crate::commands::find(&name).ok_or_else(|| format!("unknown key binding command {name:?}"))?;
        let args = self.value.get("args").cloned().unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return Err(format!("{name}: key binding args must be an object"));
        }
        crate::commands::validate_args(command, &args)?;
        Ok((name, args))
    }
}

fn defaults() -> Sections {
    serde_json::from_str(DEFAULTS).expect("defaults/keys.json is valid")
}

/// Every binding as `(section, keys, action)`, for the keybind help.
pub fn listing() -> Vec<(String, String, String)> {
    let text = std::fs::read_to_string(path()).unwrap_or_default();
    let sections: Sections = serde_json::from_str(&text).unwrap_or_else(|_| defaults());
    let mut out = Vec::new();
    for (s, keys) in sections {
        for (k, a) in keys {
            match a {
                Binding::String(a) => {
                    let a = current(&a);
                    if !gone_mod(&a) {
                        out.push((s.clone(), k, a));
                    }
                }
                Binding::Object(a) => out.push((s.clone(), k, serde_json::Value::Object(a).to_string())),
            }
        }
    }
    out.push(("global".into(), ESCAPE_HATCH.into(), "app.reset".into()));
    out
}

type Listing = Vec<(String, String, String)>;

/// `listing()`, read again only when keys.json changes: tooltips ask for it on every render.
pub fn listing_cached() -> Rc<Listing> {
    thread_local! {
        static CACHE: std::cell::RefCell<Option<(std::time::SystemTime, Rc<Listing>)>> = const { std::cell::RefCell::new(None) };
    }
    let modified = std::fs::metadata(path()).and_then(|m| m.modified()).ok();
    CACHE.with_borrow_mut(|cache| {
        if let (Some(now), Some((then, list))) = (modified, cache.as_ref())
            && now == *then
        {
            return list.clone();
        }
        let list = Rc::new(listing());
        *cache = modified.map(|m| (m, list.clone()));
        list
    })
}

/// `label` and the key bound to `action`, for a tooltip: "Split right  ⌘D".
/// The shown layout's section wins over `global`; unbound, just `label`.
pub fn with_shortcut(listing: &[(String, String, String)], label: &str, action: &str, layout: Option<&str>, mac: bool) -> String {
    let in_section = |section: &str| listing.iter().find(|(s, k, a)| s == section && a == action && !k.trim().is_empty());
    let bound = layout.and_then(|name| in_section(&format!("layout:{name}"))).or_else(|| in_section("global"));
    match bound {
        Some((_, keys, _)) => format!("{label}  {}", display(keys, mac)),
        None => label.to_owned(),
    }
}

/// How a binding reads: `cmd-shift-d` is ⌘⇧D on macOS, `ctrl-shift-d` is Ctrl+Shift+D elsewhere.
pub fn display(keys: &str, mac: bool) -> String {
    const ORDER: [&str; 5] = ["ctrl", "alt", "shift", "cmd", "fn"];
    let names: [&str; 5] = if mac { ["⌃", "⌥", "⇧", "⌘", "fn"] } else { ["Ctrl", "Alt", "Shift", "Super", "Fn"] };
    keys.split_whitespace()
        .map(|stroke| {
            let (held, key) = match stroke.strip_suffix("--") {
                Some(held) => (held, "-"),
                None => stroke.rsplit_once('-').filter(|(_, k)| !k.is_empty()).unwrap_or(("", stroke)),
            };
            let mut on: Vec<&str> = held
                .split('-')
                .filter(|m| !m.is_empty())
                .map(|m| match m.to_ascii_lowercase().as_str() {
                    "secondary" if mac => "cmd",
                    "secondary" => "ctrl",
                    "super" | "win" | "cmd" => "cmd",
                    "option" | "alt" => "alt",
                    "control" | "ctrl" => "ctrl",
                    "shift" => "shift",
                    "fn" => "fn",
                    _ => "",
                })
                .collect();
            if key.len() == 1 && key.chars().all(|c| c.is_ascii_uppercase()) {
                on.push("shift");
            }
            let mut chars = key.chars();
            let key: String = chars.next().map(|first| first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect()).unwrap_or_default();
            let mut parts: Vec<String> = ORDER.iter().zip(names).filter(|(m, _)| on.contains(m)).map(|(_, n)| n.to_owned()).collect();
            parts.push(key);
            parts.join(if mac { "" } else { "+" })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn path() -> std::path::PathBuf {
    crate::mods::app_dir().join("keys.json")
}

/// The GPUI key tab a section applies in. `global` is the window itself,
/// not "no tab": GPUI ranks a binding without one above every other, so
/// it would beat `layout:` (same depth, added earlier) and only lose to
/// `terminal` and `mod:` by order.
fn tab(section: &str) -> Result<KeyBindingContextPredicate, String> {
    use KeyBindingContextPredicate::{Equal, Identifier};
    match section {
        "global" => Ok(Identifier("Herdr".into())),
        "terminal" => Ok(Identifier("Terminal".into())),
        s => match (s.strip_prefix("mod:"), s.strip_prefix("layout:")) {
            (Some(p), _) if !p.is_empty() => Ok(Equal("mod".into(), p.to_owned().into())),
            // While layouts/<name>.json is shown (the window's tab says which).
            (_, Some(l)) if !l.is_empty() => Ok(Equal("layout".into(), l.to_owned().into())),
            _ => Err(format!("unknown section {s:?}: use \"global\", \"terminal\", \"layout:<name>\" or \"mod:<name>\"")),
        },
    }
}

/// 0.1.0's words in a keys.json from then (installed once, so still theirs):
/// projects became spaces, contexts tabs, the start popup the new tab.
pub(crate) fn current(action: &str) -> String {
    if action == "mod.open:start" {
        return "tab.new".into();
    }
    let command = |a: &str| crate::commands::find(a.split_once(':').map_or(a, |(n, _)| n)).is_some();
    for (old, new) in [("project.", "space."), ("context.", "tab.")] {
        if let Some(rest) = action.strip_prefix(old) {
            let renamed = format!("{new}{rest}");
            // Only a command that was renamed: a mod named `project` keeps its own.
            if command(&renamed) && !command(action) {
                return renamed;
            }
        }
    }
    action.to_owned()
}

/// A mod's command whose mod is gone (0.1.x's `ribbon.next`): bound to nothing.
fn gone_mod(action: &str) -> bool {
    let name = action.split_once(':').map_or(action, |(n, _)| n);
    crate::commands::find(name).is_none()
        && name.split_once('.').is_some_and(|(m, _)| !m.starts_with("frame-") && !crate::mods::mods_dir().join(m).is_dir())
}

fn valid_action(action: &str) -> bool {
    let name = action.split_once(':').map_or(action, |(n, _)| n);
    crate::commands::find(name).is_some()
        || action == "terminal"
        || action.starts_with("send:")
        || name.split_once('.').is_some_and(|(m, c)| !m.is_empty() && !c.is_empty())
}

/// Reads keys.json, installing or updating untouched defaults, or says what's wrong.
pub fn load() -> Result<Vec<KeyBinding>, String> {
    #[cfg(not(target_os = "macos"))]
    let originals = SHIPPED_KEYS;
    #[cfg(target_os = "macos")]
    let originals = SHIPPED_MACOS_KEYS;
    parse(&load_text(&path(), DEFAULTS, originals)?)
}

fn load_text(path: &std::path::Path, defaults: &str, originals: &[&str]) -> Result<String, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(path, defaults).map_err(|e| e.to_string())?;
            return Ok(defaults.to_owned());
        }
        Err(e) => return Err(e.to_string()),
    };
    if text != defaults && originals.contains(&text.as_str()) {
        std::fs::write(path, defaults).map_err(|e| e.to_string())?;
        return Ok(defaults.to_owned());
    }
    Ok(text)
}

pub fn parse(text: &str) -> Result<Vec<KeyBinding>, String> {
    let sections: Sections = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for (section, keys) in &sections {
        let predicate = match tab(section) {
            Ok(c) => Some(Rc::new(c)),
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        for (keys, action) in keys {
            let string = match action {
                Binding::String(action) => Some(current(action)),
                Binding::Object(_) => None,
            };
            if let Some(action) = &string {
                if gone_mod(action) {
                    continue;
                }
                if !valid_action(action) {
                    problems.push(format!("{section}.{keys}: unknown action {action:?}"));
                    continue;
                }
            }
            if keys == ESCAPE_HATCH {
                problems.push(format!("{ESCAPE_HATCH} is reserved for resetting Herdr"));
                continue;
            }
            let action: Box<dyn gpui::Action> = if let Binding::Object(value) = action {
                Box::new(CommandBinding { value: value.clone() })
            } else if string.as_deref() == Some("terminal") {
                // Off where this section applies; globally, simply not bound.
                if section == "global" {
                    continue;
                }
                Box::new(NoAction)
            } else {
                let action = string.as_ref().expect("string binding");
                #[cfg(target_os = "macos")]
                let native = crate::macos::action(action);
                #[cfg(not(target_os = "macos"))]
                let native: Option<Box<dyn gpui::Action>> = None;
                native.unwrap_or_else(|| Box::new(ShellAction::new(action.clone())))
            };
            let binding = KeyBinding::load(keys, action, predicate.clone(), false, None, &gpui::DummyKeyboardMapper);
            match binding {
                Ok(b) => out.push(b),
                Err(e) => problems.push(format!("{section}.{keys}: {e}")),
            }
        }
    }
    if problems.is_empty() { Ok(out) } else { Err(problems.join("; ")) }
}

/// The UI kit's own keys (text editing in inputs, lists): bound at its init,
/// kept across every reload of keys.json.
struct KitKeys(Vec<KeyBinding>);
impl gpui::Global for KitKeys {}

/// Restores kit bindings, installs user bindings, then adds reserved bindings.
pub fn install(bindings: Vec<KeyBinding>, cx: &mut App) {
    if !cx.has_global::<KitKeys>() {
        let kit = cx.key_bindings().borrow().bindings().cloned().collect();
        cx.set_global(KitKeys(kit));
    }
    let kit = cx.global::<KitKeys>().0.clone();
    cx.clear_key_bindings();
    cx.bind_keys(kit);
    cx.bind_keys(bindings);
    cx.bind_keys([
        KeyBinding::new(ESCAPE_HATCH, ShellAction::new("app.reset"), None),
        KeyBinding::new("escape", ShellAction::new("overlay.close"), Some("Overlay")),
    ]);
    // The menu bar shows the keys it finds when it's built.
    #[cfg(target_os = "macos")]
    crate::macos::refresh_menus(cx);
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_byte_identical_shipped_keys_are_upgraded() {
        let dir = std::env::temp_dir().join(format!("herdr-keys-migration-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("keys.json");
        for (defaults, originals) in [
            (include_str!("../defaults/keys.json"), super::SHIPPED_KEYS),
            (include_str!("../defaults/keys.macos.json"), super::SHIPPED_MACOS_KEYS),
        ] {
            for original in originals {
                std::fs::write(&path, original).unwrap();
                assert_eq!(super::load_text(&path, defaults, originals).unwrap(), defaults);
                assert_eq!(std::fs::read_to_string(&path).unwrap(), defaults);

                // Even a whitespace-only edit is personal, not an untouched original.
                for edited in [format!("{original}\n"), original.replace("tab.new", "pane.zoom")] {
                    std::fs::write(&path, &edited).unwrap();
                    assert_eq!(super::load_text(&path, defaults, originals).unwrap(), edited);
                    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
                }
            }
            std::fs::write(&path, defaults).unwrap();
            assert_eq!(super::load_text(&path, defaults, originals).unwrap(), defaults);
            std::fs::remove_file(&path).unwrap();
            assert_eq!(super::load_text(&path, defaults, originals).unwrap(), defaults);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), defaults);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keys_from_0_1_0_mean_the_same_now() {
        assert_eq!(super::current("project.select:2"), "space.select:2");
        assert_eq!(super::current("context.new"), "tab.new");
        assert_eq!(super::current("mod.open:start"), "tab.new");
        assert_eq!(super::current("pane.zoom"), "pane.zoom");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_shifted_punctuation_matches_existing_keys_files() {
        use gpui::{KeybindingKeystroke, Keystroke};
        let bindings = super::parse(include_str!("../defaults/keys.macos.json")).unwrap();
        for (base, shifted) in [("]", "}"), ("[", "{")] {
            let event = Keystroke {
                unshifted_key: Some(base.into()),
                ..Keystroke::parse(&format!("cmd-{shifted}")).unwrap()
            };
            assert!(bindings.iter().any(|binding| binding.match_keystrokes(&[event.clone()]) == Some(false)));
        }
        // Layout-derived aliases, not a US punctuation lookup table.
        for (base, shifted) in [("=", "+"), ("-", "_"), ("ö", "Ö"), ("č", "Č"), ("7", "/")] {
            let event = Keystroke {
                unshifted_key: Some(base.into()),
                ..Keystroke::parse(&format!("cmd-{shifted}")).unwrap()
            };
            for keys in [format!("cmd-shift-{base}"), format!("cmd-{shifted}")] {
                let target = KeybindingKeystroke::from_keystroke(Keystroke::parse(&keys).unwrap());
                assert!(event.should_match(&target), "{keys}");
            }
        }
        assert!(Keystroke::parse("cmd-shift-]").unwrap().unshifted_key.is_none());
        assert!(Keystroke::default().unshifted_key.is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_help_binding_matches_shifted_slash() {
        // Linux reports Ctrl+Shift+/ as Ctrl+? without the Shift modifier.
        let event = gpui::Keystroke::parse("ctrl-?").unwrap();
        let defaults: serde_json::Value = serde_json::from_str(super::DEFAULTS).unwrap();
        let help = defaults["global"].as_object().unwrap().iter()
            .filter(|(_, action)| action.as_str() == Some("app.help"))
            .map(|(keys, action)| (keys.clone(), action.clone()))
            .collect::<serde_json::Map<_, _>>();
        let bindings = super::parse(&serde_json::json!({ "global": help }).to_string()).unwrap();
        assert!(bindings.iter().any(|binding| binding.match_keystrokes(&[event.clone()]) == Some(false)));
    }

    #[test]
    fn object_bindings_pass_named_typed_arguments() {
        use serde_json::json;
        for (name, args) in [
            ("layout.resize", json!({"id":"sidebar", "px":240})),
            ("tab.new", json!({"mod":"editor", "props":{"enabled":true, "items":[1,null,"a:b"]}})),
            ("app.help", json!({})),
        ] {
            let bindings = super::parse(&json!({"global":{"f6":{"command":name,"args":args}}}).to_string()).unwrap();
            assert_eq!(bindings.len(), 1);
            let action = bindings[0].action().as_any().downcast_ref::<super::CommandBinding>().unwrap();
            assert_eq!(action.command().unwrap(), (name.into(), args));
        }
        let bindings = super::parse(r#"{"global":{"f6":{"command":"app.help"}}}"#).unwrap();
        let action = bindings[0].action().as_any().downcast_ref::<super::CommandBinding>().unwrap();
        assert_eq!(action.command().unwrap(), ("app.help".into(), json!({})));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn object_bindings_resolve_native_app_actions_after_validation() {
        use serde_json::json;
        for name in ["app.quit", "app.hide", "app.hide_others", "app.show_all", "app.settings"] {
            let bindings = super::parse(&json!({"global":{"f6":{"command":name}}}).to_string()).unwrap();
            let action = bindings[0].action().as_any().downcast_ref::<super::CommandBinding>().unwrap();
            let (command, args) = action.command().unwrap();
            assert_eq!(args, json!({}));
            let native = crate::macos::action(&command).unwrap();
            let expected = crate::macos::action(name).unwrap();
            assert!(native.partial_eq(expected.as_ref()));
            let bad = super::CommandBinding { value: json!({"command":name,"args":{"typo":true}}).as_object().unwrap().clone() };
            assert!(bad.command().is_err());
        }
    }

    #[test]
    fn bad_object_bindings_stay_bound_for_error_toasts() {
        use serde_json::json;
        for value in [
            json!({}), json!({"command":3}), json!({"command":"unknown"}),
            json!({"command":"space.new:{work}"}),
            json!({"command":"app.help","extra":true}),
            json!({"command":"app.help","args":null}),
            json!({"command":"app.help","args":[]}),
            json!({"command":"app.help","args":"{}"}),
            json!({"command":"layout.resize","args":{"typo":240}}),
            json!({"command":"layout.resize","args":{"id":123,"px":240}}),
        ] {
            let bindings = super::parse(&json!({"terminal":{"f6":value}}).to_string()).unwrap();
            assert_eq!(bindings.len(), 1, "{value}");
            assert_eq!(bindings[0].match_keystrokes(&[gpui::Keystroke::parse("f6").unwrap()]), Some(false));
            let action = bindings[0].action().as_any().downcast_ref::<super::CommandBinding>().unwrap();
            assert!(action.command().is_err(), "{value}");
        }
    }

    #[test]
    fn colon_payloads_stay_literal_including_valid_and_invalid_json() {
        use serde_json::json;
        for value in ["{work}", "{}", "{", "  {bad}", r#"{"name":"work"}"#,
            r#"{"name":"work",}"#, r#"{"name":"work"} trailing"#,
            "hello:world", "", "3", "-1", "true", "null", "[]", "[broken", r#""name""#] {
            let action = format!("space.new:{value}");
            let bindings = super::parse(&json!({"global":{"f6":action}}).to_string()).unwrap();
            let bound = bindings[0].action().as_any().downcast_ref::<herdr_shell::action::ShellAction>().unwrap();
            assert_eq!(bound.id(), action);
            let (name, payload) = bound.id().split_once(':').unwrap();
            assert_eq!(crate::commands::shorthand(crate::commands::find(name).unwrap(), payload), json!({"name":value}));
        }
        assert_eq!(crate::commands::shorthand(crate::commands::find("tab.select").unwrap(), "3"), json!({"index":3}));
    }

    #[test]
    fn shipped_keys_load() {
        for keys in [include_str!("../defaults/keys.json"), include_str!("../defaults/keys.macos.json")] {
            assert_eq!(super::parse(keys).err(), None);
        }
    }

    #[test]
    fn bindings_read_as_each_platform_writes_them() {
        use super::display;
        for (keys, mac, linux) in [
            ("cmd-shift-d", "⇧⌘D", "Shift+Super+D"),
            ("ctrl-shift-t", "⌃⇧T", "Ctrl+Shift+T"),
            ("cmd--", "⌘-", "Super+-"),
            ("shift-f2", "⇧F2", "Shift+F2"),
            ("cmd-shift-enter", "⇧⌘Enter", "Shift+Super+Enter"),
            ("ctrl-k ctrl-t", "⌃K ⌃T", "Ctrl+K Ctrl+T"),
            ("secondary-t", "⌘T", "Ctrl+T"),
            ("SUPER-Shift-t", "⇧⌘T", "Shift+Super+T"),
            ("T", "⇧T", "Shift+T"),
            ("-", "-", "-"),
        ] {
            assert_eq!(display(keys, true), mac, "{keys} on macOS");
            assert_eq!(display(keys, false), linux, "{keys} elsewhere");
        }
    }

    #[test]
    fn a_tooltip_shows_the_key_the_shown_layout_uses() {
        use super::with_shortcut;
        let row = |s: &str, k: &str, a: &str| (s.to_owned(), k.to_owned(), a.to_owned());
        let listing = [row("global", "ctrl-shift-d", "pane.split:right"), row("layout:terminals", "ctrl-alt-d", "pane.split:right"), row("terminal", "ctrl-shift-z", "pane.zoom")];
        assert_eq!(with_shortcut(&listing, "Split right", "pane.split:right", Some("main"), false), "Split right  Ctrl+Shift+D");
        assert_eq!(with_shortcut(&listing, "Split right", "pane.split:right", Some("terminals"), false), "Split right  Ctrl+Alt+D");
        assert_eq!(with_shortcut(&listing, "Split down", "pane.split:down", Some("main"), false), "Split down");
        assert_eq!(with_shortcut(&listing, "Zoom", "pane.zoom", None, false), "Zoom", "a terminal-only key is not the button's");
    }
}
