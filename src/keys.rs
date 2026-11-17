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
//!
//! A key can also be a double tap, `"shift shift"`: a modifier pressed and
//! released twice within 300 ms, nothing else between (taps.rs).

pub mod taps;

use gpui::{App, KeyBinding, KeyBindingContextPredicate, NoAction};
use std::rc::Rc;
use gpui_shell::action::ShellAction;
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

pub fn path() -> std::path::PathBuf {
    crate::mods::app_dir().join("keys.json")
}

/// The GPUI key tab a section applies in. `global` is the window itself,
/// not "no tab": GPUI ranks a binding without one above every other, so
/// it would beat `layout:` (same depth, added earlier) and only lose to
/// `terminal` and `mod:` by order.
fn tab(section: &str) -> Result<String, String> {
    match section {
        "global" => Ok("Herdr".into()),
        "terminal" => Ok("Terminal".into()),
        s => match (s.strip_prefix("mod:"), s.strip_prefix("layout:")) {
            (Some(p), _) if !p.is_empty() => Ok(format!("mod == {p}")),
            // While layouts/<name>.json is shown (the window's tab says which).
            (_, Some(l)) if !l.is_empty() => Ok(format!("layout == {l}")),
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
        let ctx = match tab(section) {
            Ok(c) => c,
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
            let predicate = match KeyBindingContextPredicate::parse(&ctx) {
                Ok(p) => Some(Rc::new(p)),
                Err(e) => {
                    problems.push(format!("{section}: {e}"));
                    continue;
                }
            };
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
            let binding = KeyBinding::load(keys, action, predicate, false, None, &gpui::DummyKeyboardMapper);
            match binding {
                Ok(b) => match taps::double_tap(&b) {
                    Some(Err(e)) => problems.push(format!("{section}.{keys}: {e}")),
                    _ => out.push(b),
                },
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
/// Double taps go to a keymap of their own (taps.rs).
pub fn install(bindings: Vec<KeyBinding>, cx: &mut App) {
    if !cx.has_global::<KitKeys>() {
        let kit = cx.key_bindings().borrow().bindings().cloned().collect();
        cx.set_global(KitKeys(kit));
    }
    let kit = cx.global::<KitKeys>().0.clone();
    let (double_taps, bindings): (Vec<_>, Vec<_>) = bindings.into_iter().partition(|b| matches!(taps::double_tap(b), Some(Ok(_))));
    taps::install(double_taps, cx);
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
            let bound = bindings[0].action().as_any().downcast_ref::<gpui_shell::action::ShellAction>().unwrap();
            assert_eq!(bound.id(), action);
            let (name, payload) = bound.id().split_once(':').unwrap();
            assert_eq!(crate::commands::shorthand(crate::commands::find(name).unwrap(), payload), json!({"name":value}));
        }
        assert_eq!(crate::commands::shorthand(crate::commands::find("tab.select").unwrap(), "3"), json!({"index":3}));
    }

    #[test]
    fn double_taps_are_one_modifier_twice() {
        use super::taps::double_tap;
        use serde_json::json;
        for (keys, tap) in [
            ("shift shift", Some("shift")), ("Shift Shift", Some("shift")),
            ("ctrl ctrl", Some("control")), ("alt alt", Some("alt")),
            ("cmd cmd", Some("platform")), ("super super", Some("platform")), ("win win", Some("platform")),
            ("fn fn", Some("function")),
            // One modifier alone, or with another key, stays GPUI's.
            ("shift", None), ("shift f1", None), ("ctrl-shift-t", None), ("a a", None),
        ] {
            for section in ["global", "terminal", "layout:main", "mod:editor"] {
                let bindings = super::parse(&json!({ section: { keys: "app.palette" } }).to_string()).unwrap();
                assert_eq!(bindings.len(), 1, "{section}.{keys}");
                assert_eq!(double_tap(&bindings[0]).map(Result::unwrap), tap, "{section}.{keys}");
            }
        }
        for keys in ["shift ctrl", "shift shift shift", "ctrl-shift ctrl-shift", "alt shift alt"] {
            let error = super::parse(&json!({ "terminal": { keys: "app.palette", "ctrl-shift-t": "tab.new" } }).to_string()).unwrap_err();
            assert_eq!(error, format!("terminal.{keys}: a double tap is one modifier twice, like \"shift shift\" or \"ctrl ctrl\""));
        }
        // An unknown action on a double tap is reported as on any key.
        assert_eq!(super::parse(r#"{"global":{"shift shift":"nope"}}"#).unwrap_err(), r#"global.shift shift: unknown action "nope""#);
    }

    #[gpui::test]
    fn double_taps_stay_out_of_gpui_and_follow_section_rules(cx: &mut gpui::TestAppContext) {
        use gpui::KeyContext;
        let bindings = super::parse(r#"{
            "global": { "shift shift": "app.palette", "ctrl ctrl": "app.help", "ctrl-shift-t": "tab.new" },
            "layout:main": { "shift shift": "tab.new" },
            "mod:editor": { "shift shift": "pane.zoom" },
            "terminal": { "shift shift": "terminal", "ctrl ctrl": "pane.zoom" }
        }"#).unwrap();
        cx.update(|cx| {
            super::install(bindings, cx);
            let keymap = cx.key_bindings();
            assert!(keymap.borrow().bindings().all(|b| super::taps::double_tap(b).is_none()));
            assert!(keymap.borrow().bindings().any(|b| b.match_keystrokes(&[gpui::Keystroke::parse("ctrl-shift-t").unwrap()]) == Some(false)));
            let run = |modifier: &str, contexts: &[&str], cx: &gpui::App| {
                let stack: Vec<_> = contexts.iter().map(|c| KeyContext::parse(c).unwrap()).collect();
                super::taps::action(modifier, &stack, cx)
                    .map(|a| a.as_any().downcast_ref::<gpui_shell::action::ShellAction>().unwrap().id().to_owned())
            };
            let other = "Herdr layout = other";
            let main = "Herdr layout = main";
            assert_eq!(run("shift", &[other], cx).as_deref(), Some("app.palette"));
            assert_eq!(run("control", &[other], cx).as_deref(), Some("app.help"));
            assert_eq!(run("alt", &[other], cx), None);
            // The shown layout's section over global; a focused terminal's over both.
            assert_eq!(run("shift", &[main], cx).as_deref(), Some("tab.new"));
            assert_eq!(run("shift", &[main, "Terminal"], cx), None);
            assert_eq!(run("control", &[main, "Terminal"], cx).as_deref(), Some("pane.zoom"));
            assert_eq!(run("shift", &[other, "Mod mod = editor"], cx).as_deref(), Some("pane.zoom"));
            assert_eq!(run("shift", &[other, "Mod mod = files"], cx).as_deref(), Some("app.palette"));
            // A keys.json without them takes them away.
            super::install(vec![], cx);
            assert_eq!(run("shift", &[other], cx), None);
        });
    }

    #[test]
    fn shipped_keys_load() {
        for keys in [include_str!("../defaults/keys.json"), include_str!("../defaults/keys.macos.json")] {
            assert_eq!(super::parse(keys).err(), None);
        }
    }
}
