//! Mods: folders of JavaScript under ~/.config/herdr-app/mods that agents write and
//! Herdr runs live (gpui-shell), plus the `herdr` module they import.

use crate::layout::Node;
use gpui::App;
use gpui_shell::{Capabilities, ExecuteGrant, HostError, HostModule, HostObject, HostValue, NetworkGrant};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

include!(concat!(env!("OUT_DIR"), "/default_mods.rs"));
include!(concat!(env!("OUT_DIR"), "/frame_mods.rs"));

#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod lifecycle_tests;

/// The frame's own mods (welcome, ...): the app's, not the person's. Written
/// from the binary on every start, so no edit or broken mod can break them.
/// Loaded as `frame-<name>`.
pub fn frame_dir() -> PathBuf {
    app_dir().join("frame")
}

pub fn home() -> PathBuf {
    // HOME, or the profile folder on Windows.
    std::env::home_dir().unwrap_or_else(|| "/".into())
}

/// The fonts on this machine (`fonts()`): read when the window opens and
/// again when the home brings new ones.
static FONTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn note_fonts(cx: &gpui::App) {
    // Names starting with "." are the system's private UI fonts (macOS).
    let mut names: Vec<String> = cx.text_system().all_font_names().into_iter().filter(|n| !n.starts_with('.')).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    *FONTS.lock().unwrap() = names;
}

/// The app's home: `$HERDR_APP_HOME` (tests, experiments), else
/// `$XDG_CONFIG_HOME/herdr-app`, else `~/.config/herdr-app`, on every OS.
pub fn app_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    env("HERDR_APP_HOME").unwrap_or_else(|| env("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config")).join("herdr-app"))
}

pub fn mods_dir() -> PathBuf {
    app_dir().join("mods")
}

const AGENTS_MD: &str = include_str!("../defaults/AGENTS.md");

/// Writes the agent guide and API types, and installs or updates the default
/// mods. Shipped mods are replaced whole; edits are first moved to a clone.
pub fn write_workspace() -> std::io::Result<()> {
    let dir = app_dir();
    std::fs::create_dir_all(mods_dir())?;
    write_if_changed(&dir.join("AGENTS.md"), AGENTS_MD)?;
    // Agents read AGENTS.md now; the pointer file we used to write goes (only ours).
    if std::fs::read_to_string(dir.join("CLAUDE.md")).is_ok_and(|s| s == "@AGENTS.md\n") {
        let _ = std::fs::remove_file(dir.join("CLAUDE.md"));
    }
    write_if_changed(&dir.join("herdr.d.ts"), DECLARATIONS)?;
    let types = gpui_shell::type_declarations(&gpui_shell::FrozenComponentRegistry::default());
    write_if_changed(&dir.join("gpui-kit.d.ts"), &types)?;
    let _ = std::fs::remove_dir_all(frame_dir());
    for (path, bytes) in FRAME_MODS {
        let file = frame_dir().join(path);
        std::fs::create_dir_all(file.parent().unwrap())?;
        std::fs::write(&file, bytes)?;
    }
    sync_defaults(&mods_dir(), &dir.join("installed.json"), DEFAULT_MODS)
}

static SYNC_NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn take_sync_notes() -> Vec<String> {
    std::mem::take(&mut *SYNC_NOTES.lock().unwrap())
}

fn edited_note(clone: &str) {
    SYNC_NOTES.lock().unwrap().push(clone.to_owned());
}

fn edited_name(parent: &Path, name: &str, suffix: &str) -> String {
    (1..).map(|i| if i == 1 { format!("{name}-edited") } else { format!("{name}-edited-{i}") })
        .find(|n| !parent.join(format!("{n}{suffix}")).exists()).unwrap()
}

fn sync_defaults(mods: &Path, installed_path: &Path, shipped_mods: &[(&str, &[u8])]) -> std::io::Result<()> {
    let text = std::fs::read_to_string(installed_path).unwrap_or_default();
    let mut installed: Installed = serde_json::from_str(&text).unwrap_or_default();
    let names: std::collections::BTreeSet<&str> = shipped_mods.iter().filter_map(|(p, _)| p.split('/').next()).collect();
    let dropped: Vec<String> = installed.keys().filter(|n| !n.starts_with("layout:") && !names.contains(n.as_str())).cloned().collect();
    for name in dropped {
        let target = mods.join(&name);
        if installed.get(&name).is_some_and(|was| target.exists() && as_shipped(&target, was)) {
            std::fs::remove_dir_all(&target)?;
        }
        installed.remove(&name);
    }
    for name in names {
        let target = mods.join(name);
        let shipped: Files = shipped_mods.iter()
            .filter_map(|(p, bytes)| Some((p.strip_prefix(&format!("{name}/"))?.to_owned(), fingerprint(bytes))))
            .collect();
        if installed.get(name) == Some(&shipped) && target.exists() && as_shipped(&target, &shipped) {
            continue;
        }
        if target.exists() {
            if !as_shipped(&target, installed.get(name).unwrap_or(&shipped)) {
                let clone = edited_name(mods, name, "");
                std::fs::rename(&target, mods.join(&clone))?;
                edited_note(&clone);
            } else {
                std::fs::remove_dir_all(&target)?;
            }
        }
        for (path, bytes) in shipped_mods.iter().filter(|(p, _)| p.starts_with(&format!("{name}/"))) {
            let file = mods.join(path);
            std::fs::create_dir_all(file.parent().unwrap())?;
            std::fs::write(&file, bytes)?;
        }
        installed.insert(name.to_owned(), shipped);
    }
    write_if_changed(installed_path, serde_json::to_string_pretty(&installed).unwrap() + "\n")
}

pub fn sync_layouts(home: &Path, layouts: &[(&str, &str)]) -> std::io::Result<()> {
    let record = home.join("installed.json");
    let mut installed: Installed = serde_json::from_str(&std::fs::read_to_string(&record).unwrap_or_default()).unwrap_or_default();
    let dir = home.join("layouts");
    std::fs::create_dir_all(&dir)?;
    if dir.join(".base").exists() {
        std::fs::remove_dir_all(dir.join(".base"))?;
    }
    let dropped: Vec<String> = installed.keys().filter(|k| k.strip_prefix("layout:").is_some_and(|n| !layouts.iter().any(|(name, _)| *name == n))).cloned().collect();
    for key in dropped {
        let path = dir.join(format!("{}.json", &key["layout:".len()..]));
        if path.exists() && installed[&key].get("json").is_some_and(|hash| std::fs::read_to_string(&path).is_ok_and(|text| layout_fingerprint(&text) == *hash)) {
            std::fs::remove_file(path)?;
            let _ = std::fs::remove_file(dir.join(".state").join(format!("{}.json", &key["layout:".len()..])));
        }
        installed.remove(&key);
    }
    for (name, text) in layouts {
        let key = format!("layout:{name}");
        let hash = layout_fingerprint(text);
        let path = dir.join(format!("{name}.json"));
        if path.exists() {
            let current = layout_fingerprint(&std::fs::read_to_string(&path)?);
            let Some(was) = installed.get(&key).and_then(|f| f.get("json")) else {
                // From 0.1.x, which recorded no layouts: whether it was edited is
                // unknowable, so no rescue copy and no toast; kept aside, quietly.
                if current != hash {
                    std::fs::rename(&path, dir.join(format!("{name}.json.before-{}", env!("CARGO_PKG_VERSION"))))?;
                }
                std::fs::write(&path, text)?;
                installed.insert(key, [("json".into(), hash)].into());
                continue;
            };
            if current != *was && current != hash {
                let clone = edited_name(&dir, name, ".json");
                copy_layout_state(home, name, &clone)?;
                std::fs::rename(&path, dir.join(format!("{clone}.json")))?;
                edited_note(&clone);
            }
        }
        std::fs::write(&path, text)?;
        installed.insert(key, [("json".into(), hash)].into());
    }
    write_if_changed(&record, serde_json::to_string_pretty(&installed).unwrap() + "\n")
}

fn layout_fingerprint(text: &str) -> String {
    let canonical = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok().map(|mut v| {
        crate::layout_sync::normalize(&mut v);
        v.sort_all_objects();
        v.to_string()
    });
    fingerprint(canonical.as_deref().unwrap_or(text).as_bytes())
}

pub fn copy_layout_state(home: &Path, name: &str, to: &str) -> std::io::Result<()> {
    let dir = home.join("layouts/.state");
    match std::fs::copy(dir.join(format!("{name}.json")), dir.join(format!("{to}.json"))) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !dir.join(format!("{name}.json")).exists() => {
            match std::fs::remove_file(dir.join(format!("{to}.json"))) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn clone_mod(name: &str, to: &str) -> std::io::Result<()> {
    clone_mod_at(&mods_dir(), name, to)
}

fn clone_mod_at(mods: &Path, name: &str, to: &str) -> std::io::Result<()> {
    if !valid_name(name) || !valid_name(to) {
        return Err(std::io::Error::other("names use letters, digits, - or _"));
    }
    let source = mods.join(name);
    let target = mods.join(to);
    if !source.is_dir() {
        return Err(std::io::Error::other("no such mod"));
    }
    std::fs::create_dir(&target)?;
    fn copy(source: &Path, target: &Path, from: &str, to: &str) -> std::io::Result<()> {
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let dest = target.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                std::fs::create_dir(&dest)?;
                copy(&entry.path(), &dest, from, to)?;
            } else {
                let bytes = std::fs::read(entry.path())?;
                std::fs::copy(entry.path(), &dest)?;
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let mut text = text.replace(&format!("mods/{from}/"), &format!("mods/{to}/"))
                        .replace(&format!("\"mods/{from}\""), &format!("\"mods/{to}\""))
                        .replace(&format!("'mods/{from}'"), &format!("'mods/{to}'"));
                    for key in ["mod", "\"mod\"", "'mod'"] {
                        for space in ["", " "] {
                            for quote in ["\"", "'"] {
                                text = text.replace(&format!("{key}:{space}{quote}{from}{quote}"), &format!("{key}:{space}{quote}{to}{quote}"));
                            }
                        }
                    }
                    std::fs::write(dest, text)?;
                }
            }
        }
        Ok(())
    }
    let result = (|| {
        copy(&source, &target, name, to)?;
        let path = target.join("mod.json");
        let mut meta: Value = if path.exists() { serde_json::from_str(&std::fs::read_to_string(&path)?)? } else { json!({}) };
        let Some(object) = meta.as_object_mut() else {
            return Err(std::io::Error::other("mod.json must be an object"));
        };
        object.insert("cloned_from".into(), json!({ "name": name, "version": env!("CARGO_PKG_VERSION") }));
        std::fs::write(path, serde_json::to_string_pretty(&meta)?)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(target);
    }
    result
}

/// Mods in use by kind, for telemetry: the shipped ones as shipped (by
/// name), how many shipped ones were edited, how many are the person's own
/// (never their names).
pub fn kinds(names: &std::collections::BTreeSet<String>) -> (Vec<String>, usize, usize) {
    let text = std::fs::read_to_string(app_dir().join("installed.json")).unwrap_or_default();
    let installed: Installed = serde_json::from_str(&text).unwrap_or_default();
    let shipped = |n: &str| DEFAULT_MODS.iter().any(|(p, _)| p.split('/').next() == Some(n));
    let (mut builtin, mut edited, mut custom) = (Vec::new(), 0, 0);
    for name in names {
        if !shipped(name) {
            custom += 1;
        } else if installed.get(name).is_some_and(|files| as_shipped(&mods_dir().join(name), files)) {
            builtin.push(name.clone());
        } else {
            edited += 1;
        }
    }
    (builtin, edited, custom)
}

/// Exactly as installed: every file unchanged and none added.
fn as_shipped(dir: &Path, files: &Files) -> bool {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = e.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, dir, &mut found);
    // gpui-shell writes these editor helpers when it loads a mod.
    found.retain(|f| !matches!(f.as_str(), "gpui-kit.d.ts" | "jsconfig.json") || files.contains_key(f));
    found.len() == files.len() && found.iter().all(|f| files.contains_key(f)) && untouched(dir, files)
}

/// File path (inside the mod) to content fingerprint.
type Files = std::collections::BTreeMap<String, String>;
/// Default mods Herdr installed, with the files as installed.
type Installed = std::collections::BTreeMap<String, Files>;

/// FNV-1a: stable across builds, unlike std's hasher.
fn fingerprint(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3));
    format!("{h:016x}")
}

/// Whether every file Herdr installed is still there as installed.
fn untouched(dir: &Path, files: &Files) -> bool {
    files.iter().all(|(path, want)| std::fs::read(dir.join(path)).is_ok_and(|b| fingerprint(&b) == *want))
}

fn write_if_changed(path: &Path, content: impl AsRef<[u8]>) -> std::io::Result<()> {
    let content = content.as_ref();
    if std::fs::read(path).ok().as_deref() != Some(content) {
        std::fs::write(path, content)?;
    }
    Ok(())
}

/// Where a mod would like to go the first time it appears (mod.json).
#[derive(Deserialize, Default, Clone)]
pub struct Hint {
    #[serde(default)]
    pub place: Option<String>,
    #[serde(default)]
    pub size: Option<f32>,
    /// `pane.go` can give it the keyboard.
    #[serde(default)]
    pub focusable: bool,
}

#[derive(Clone)]
pub struct Found {
    pub id: String,
    pub dir: PathBuf,
    pub hint: Hint,
    pub stamp: SystemTime,
}

/// Every mod folder that has a main.js.
pub fn scan() -> Vec<Found> {
    let mut found = scan_dir(&mods_dir(), "");
    found.extend(scan_dir(&frame_dir(), "frame-"));
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

fn scan_dir(root: &Path, prefix: &str) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.path().join("main.js").is_file())
        .filter_map(|e| {
            let id = format!("{prefix}{}", e.file_name().to_str()?);
            if id.starts_with('.') {
                return None;
            }
            let dir = e.path();
            let hint = std::fs::read_to_string(dir.join("mod.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            let stamp = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|f| f.metadata().ok()?.modified().ok())
                .max()
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some(Found { id, dir, hint, stamp })
        })
        .collect()
}

/// What mods ask of the window.
pub enum Request {
    Run { name: String, args: Value },
    OpenOverlay { id: String, spec: Value },
    CloseOverlay { id: String },
    Refresh,
    /// From `herdr-app do` / `herdr-app status` (control.rs); answer on `reply`.
    Control { cmd: Value, reply: std::sync::mpsc::Sender<Value> },
}

static REQUESTS: OnceLock<async_channel::Sender<Request>> = OnceLock::new();

thread_local! {
    static FINAL_KILLS: RefCell<Option<(std::sync::Arc<crate::hc::Core>, std::time::Instant)>> = const { RefCell::new(None) };
}

/// The event loop stops before queued commands run at quit. Send final terminal
/// kills to the core now, bounded by the same deadline as the mod's cleanup.
pub fn final_cleanup(core: std::sync::Arc<crate::hc::Core>, deadline: std::time::Instant, body: impl FnOnce()) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) { FINAL_KILLS.with_borrow_mut(|slot| { slot.take(); }); }
    }
    FINAL_KILLS.with_borrow_mut(|slot| {
        assert!(slot.is_none(), "nested final cleanup");
        *slot = Some((core, deadline));
    });
    let _reset = Reset;
    body();
}

/// Navigation order set by the mod that lists agents, with stale entries removed and new agents appended.
pub fn agent_order(available: Vec<String>) -> Vec<String> {
    AGENT_ORDER.with(|s| {
        let mut order = Vec::new();
        for addr in s.borrow().iter().chain(available.iter()) {
            if available.contains(addr) && !order.contains(addr) {
                order.push(addr.clone());
            }
        }
        order
    })
}

pub fn shared(key: &str) -> Option<Value> {
    with_shared(|s| s.values.get(key).cloned())
}

/// Sets a value every mod can read (persisted in shared.json); mods re-render.
pub fn set_shared(key: String, value: Value) {
    if with_shared(|s| s.set(key, value)) {
        request(Request::Refresh);
    }
}

fn with_shared<R>(f: impl FnOnce(&mut SharedStore) -> R) -> R {
    SHARED.with_borrow_mut(|slot| f(slot.get_or_insert_with(|| SharedStore::load(app_dir().join("shared.json")))))
}

/// Called after mod close handlers, so their last changes reach disk before exit.
pub fn finish_shared() {
    SHARED.with_borrow_mut(|slot| {
        if let Some(store) = slot {
            store.finish();
        }
    });
}

struct SharedStore {
    values: serde_json::Map<String, Value>,
    path: PathBuf,
    writable: bool,
    writer: Option<(std::sync::mpsc::Sender<String>, std::thread::JoinHandle<()>)>,
}

impl SharedStore {
    fn load(path: PathBuf) -> Self {
        let mut writable = true;
        let values = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(values) => values,
                Err(error) => {
                    let backup = crate::window::aside(&path, "corrupt");
                    match std::fs::rename(&path, &backup) {
                        Ok(()) => tracing::warn!("{} is invalid ({error}); kept in {}", path.display(), backup.display()),
                        Err(e) => {
                            writable = false;
                            tracing::warn!("{} is invalid ({error}); cannot keep it aside: {e}; shared persistence disabled", path.display());
                        }
                    }
                    serde_json::Map::new()
                }
            },
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    writable = false;
                    tracing::warn!("cannot read {}: {error}; shared persistence disabled", path.display());
                }
                serde_json::Map::new()
            }
        };
        Self { values, path, writable, writer: None }
    }

    fn set(&mut self, key: String, value: Value) -> bool {
        if self.values.get(&key) == Some(&value) {
            return false;
        }
        self.values.insert(key, value);
        if self.writable {
            let (writer, _) = self.writer.get_or_insert_with(|| shared_writer(self.path.clone()));
            if let Err(error) = writer.send(serde_json::to_string(&self.values).expect("shared values encode as JSON")) {
                tracing::warn!("shared writer stopped: {error}");
            }
        }
        true
    }

    fn finish(&mut self) {
        if let Some((sender, worker)) = self.writer.take() {
            drop(sender);
            if worker.join().is_err() {
                tracing::warn!("shared writer stopped before saving finished");
            }
        }
    }
}

fn shared_writer(path: PathBuf) -> (std::sync::mpsc::Sender<String>, std::thread::JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let worker = std::thread::spawn(move || {
        while let Ok(mut text) = rx.recv() {
            // A burst needs only its newest snapshot. Never let an older write land last.
            for newer in rx.try_iter() {
                text = newer;
            }
            if let Err(error) = crate::persist::write_atomic(&path, text) {
                tracing::warn!("cannot write {}: {error}", path.display());
            }
        }
    });
    (tx, worker)
}

pub fn request_sender() -> Option<async_channel::Sender<Request>> {
    REQUESTS.get().cloned()
}

pub fn request(r: Request) {
    if let Some(tx) = REQUESTS.get() {
        let _ = tx.try_send(r);
    }
}

thread_local! {
    /// Snapshots the window publishes; mods read them synchronously and
    /// re-render when they change.
    static SNAPSHOT: RefCell<Snapshot> = RefCell::new(Snapshot::default());
    static AGENT_ORDER: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static SHARED: RefCell<Option<SharedStore>> = const { RefCell::new(None) };
    /// Where key commands for each loaded copy go: (copy, mod, sender).
    static COMMANDS: RefCell<Vec<(u64, String, async_channel::Sender<String>)>> = const { RefCell::new(Vec::new()) };
    /// The copies on screen: only they get commands (a hidden copy kept alive must
    /// not act on the person's keys).
    static SHOWN_COPIES: RefCell<std::collections::HashSet<u64>> = RefCell::new(std::collections::HashSet::new());
    static COPY_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static OVERLAY_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub struct Snapshot {
    pub layout: Value,
    pub spaces: Value,
    pub machines: Value,
    pub agents: Value,
    /// Every pane: { pane, terminal, tab, title, cwd, branch, status, app, focused, zoomed }.
    pub panes: Value,
    pub terminals: Value,
    pub focused: Value,
    /// Floating blocks by id: { edge, align, vertical }.
    pub floats: Value,
    /// { platform, titlebar, buttons, maximized, fullscreen, active }.
    pub window: Value,
}

impl Default for Snapshot {
    /// Lists start empty, never null, so mods can map over them right away.
    fn default() -> Self {
        Snapshot { layout: Value::Null, spaces: json!([]), machines: json!([]), agents: json!([]), panes: json!([]), terminals: json!([]), focused: Value::Null, floats: json!({}), window: Value::Null }
    }
}

/// Replaces one part of the snapshot; returns whether it changed.
pub fn publish(set: impl FnOnce(&mut Snapshot) -> (Value, Value)) -> bool {
    SNAPSHOT.with(|s| {
        let mut s = s.borrow_mut();
        let (old, new) = set(&mut s);
        old != new
    })
}

pub fn publish_layout(layout: &Node) -> bool {
    let v = serde_json::to_value(layout).unwrap_or(Value::Null);
    publish(|s| (std::mem::replace(&mut s.layout, v.clone()), v))
}
pub fn publish_machines(v: Value) -> bool {
    publish(|s| (std::mem::replace(&mut s.machines, v.clone()), v))
}
pub fn publish_spaces(v: Value) -> bool {
    publish(|s| (std::mem::replace(&mut s.spaces, v.clone()), v))
}
pub fn publish_agents(v: Value) -> bool {
    publish(|s| (std::mem::replace(&mut s.agents, v.clone()), v))
}
pub fn publish_panes(v: Value) -> bool {
    publish(|s| (std::mem::replace(&mut s.panes, v.clone()), v))
}
pub fn publish_terminals(v: Value) -> bool {
    publish(|s| (std::mem::replace(&mut s.terminals, v.clone()), v))
}
pub fn terminal_address(word: &str, source: &str) -> Option<String> {
    SNAPSHOT.with(|s| {
        let s = s.borrow();
        let live = s.terminals.as_array()?;
        crate::links::terminal_address(word, source, live.iter().filter_map(|t| t["address"].as_str()))
    })
}

pub fn publish_floats(v: Value) -> bool {
    publish(|s| (std::mem::replace(&mut s.floats, v.clone()), v))
}
pub fn publish_window(v: Value) -> bool {
    publish(|s| (std::mem::replace(&mut s.window, v.clone()), v))
}
pub fn publish_focused(v: Value) -> bool {
    publish(|s| (std::mem::replace(&mut s.focused, v.clone()), v))
}

fn read(f: impl FnOnce(&Snapshot) -> &Value) -> HostValue {
    SNAPSHOT.with(|s| to_host(f(&s.borrow())))
}

/// Delivers `command` to every shown copy of `mod`. Returns whether any listens.
pub fn send_command(name: &str, command: &str) -> bool {
    let shown = SHOWN_COPIES.with(|s| s.borrow().clone());
    COMMANDS.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|(_, _, tx)| !tx.is_closed());
        let mut any = false;
        for (copy, p, tx) in c.iter() {
            if p == name && shown.contains(copy) {
                any |= tx.try_send(command.to_owned()).is_ok();
            }
        }
        any
    })
}

/// Delivers `command` to one shown copy (the block given the keyboard, not
/// every copy of its mod).
pub fn send_command_to(copy: u64, command: &str) -> bool {
    if !SHOWN_COPIES.with(|s| s.borrow().contains(&copy)) {
        return false;
    }
    COMMANDS.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|(_, _, tx)| !tx.is_closed());
        c.iter().any(|(id, _, tx)| *id == copy && tx.try_send(command.to_owned()).is_ok())
    })
}

/// The copies on screen now (`prepare_load` numbers them).
pub fn set_shown_copies(copies: std::collections::HashSet<u64>) {
    SHOWN_COPIES.with(|s| *s.borrow_mut() = copies);
}

pub fn new_overlay_id() -> String {
    OVERLAY_SEQ.with(|s| {
        s.set(s.get() + 1);
        format!("o{}", s.get())
    })
}

/// One loaded copy of a mod: which folder, with which props.
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    pub name: String,
    pub props: Value,
}

impl Instance {
    pub fn key(&self) -> String {
        if self.props.is_null() { self.name.clone() } else { format!("{}{}", self.name, self.props) }
    }
}

pub fn to_host(v: &Value) -> HostValue {
    match v {
        Value::Null => HostValue::Null,
        Value::Bool(b) => HostValue::Bool(*b),
        Value::Number(n) => HostValue::Number(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => HostValue::Str(s.clone()),
        Value::Array(a) => HostValue::Array(a.iter().map(to_host).collect()),
        Value::Object(o) => HostValue::Object(o.iter().map(|(k, v)| (k.clone(), to_host(v))).collect()),
    }
}

pub fn from_host(v: &HostValue) -> Value {
    match v {
        HostValue::Null => Value::Null,
        HostValue::Bool(b) => Value::Bool(*b),
        HostValue::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => Value::from(*n as i64),
        HostValue::Number(n) => serde_json::Number::from_f64(*n).map_or(Value::Null, Value::Number),
        HostValue::Str(s) => Value::String(s.clone()),
        HostValue::Array(a) => Value::Array(a.iter().map(from_host).collect()),
        HostValue::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), from_host(v))).collect()),
    }
}

pub const DECLARATIONS: &str = include_str!("../defaults/herdr.d.ts");

fn strings(v: Option<&HostValue>) -> Vec<String> {
    v.and_then(HostValue::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

fn opt_str(v: Option<&HostValue>, key: &str) -> Option<String> {
    v.and_then(|o| o.get(key)).and_then(HostValue::as_str).map(str::to_owned)
}

const EXEC_LIMIT: Duration = Duration::from_secs(30);
const EXEC_OUTPUT: u64 = 8 << 20;

fn run_blocking(command: &str, args: &[String], cwd: Option<String>) -> Result<(i32, String, String), String> {
    use std::io::Read;
    let mut child = crate::spawn::command(command)
        .args(args)
        .current_dir(cwd.map(PathBuf::from).unwrap_or_else(home))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {command}: {e}"))?;
    // Bounded: a chatty command can't fill memory; the rest is drained and dropped.
    let drain = |r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut r = r;
            let _ = (&mut r).take(EXEC_OUTPUT).read_to_end(&mut buf);
            let _ = std::io::copy(&mut r, &mut std::io::sink());
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = drain(Box::new(child.stdout.take().unwrap()));
    let err = drain(Box::new(child.stderr.take().unwrap()));
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > EXEC_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{command} took longer than 30 s and was stopped"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok((status.code().unwrap_or(-1), out.join().unwrap_or_default(), err.join().unwrap_or_default()))
}

/// Runs a program on another machine through the core's exec: its exact output.
pub fn run_remote(machine: &str, command: &str, args: &[String], cwd: Option<String>) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    use base64::Engine;
    let mut hc_args: Vec<String> = vec!["exec".into(), "--json".into(), "--on".into(), machine.into(), "--timeout".into(), EXEC_LIMIT.as_secs().to_string()];
    if let Some(c) = cwd {
        hc_args.extend(["--cwd".into(), c]);
    }
    hc_args.push("--".into());
    hc_args.push(command.into());
    hc_args.extend(args.iter().cloned());
    let (_, out, err) = run_blocking("hc", &hc_args, None)?;
    let v: Value = serde_json::from_str(out.trim()).or_else(|_| serde_json::from_str(err.trim())).map_err(|_| format!("hc exec: {}", err.trim()))?;
    if let Some(e) = v.get("error") {
        return Err(e.get("message").and_then(Value::as_str).unwrap_or("hc exec failed").to_owned());
    }
    if v.get("timed_out").and_then(Value::as_bool) == Some(true) {
        return Err(format!("{command} took longer than {} s on {machine} and was stopped", EXEC_LIMIT.as_secs()));
    }
    let bytes = |k: &str| base64::engine::general_purpose::STANDARD.decode(v.get(k).and_then(Value::as_str).unwrap_or_default()).map_err(|e| e.to_string());
    let code = v.get("code").and_then(Value::as_i64).map_or(-1, |c| c as i32);
    Ok((code, bytes("stdout")?, bytes("stderr")?))
}

/// A program's output here, if it ran (for the app's own checks).
pub fn run_here(command: &str, args: &[String]) -> Option<String> {
    run_blocking(command, args, None).ok().map(|(_, out, _)| out)
}

/// A program's output on another machine, if it ran.
pub fn run_on(machine: &str, command: &str, args: &[String]) -> Option<String> {
    run_remote(machine, command, args, None).ok().map(|(_, out, _)| String::from_utf8_lossy(&out).into_owned())
}

/// The agents a machine can start (`knownAgents()`), asked at most once a minute.
fn known_agents(on: Option<String>, list: Vec<Value>) -> Value {
    type InstalledCache = std::collections::HashMap<Option<String>, (std::time::Instant, Vec<String>, Vec<String>)>;
    static SEEN: std::sync::Mutex<Option<InstalledCache>> = std::sync::Mutex::new(None);
    let commands: Vec<String> = list.iter().filter_map(|a| a["run"][0].as_str().map(str::to_owned)).collect();
    let cached = SEEN.lock().ok().and_then(|s| s.as_ref()?.get(&on).filter(|(at, checked, _)| at.elapsed().as_secs() < 60 && checked == &commands).map(|(_, _, v)| v.clone()));
    let have = cached.unwrap_or_else(|| {
        let found = crate::programs::installed(&commands, on.as_deref());
        if let Ok(mut s) = SEEN.lock() {
            s.get_or_insert_with(std::collections::HashMap::new).insert(on.clone(), (std::time::Instant::now(), commands, found.clone()));
        }
        found
    });
    Value::Array(
        list.into_iter()
            .map(|mut a| {
                let run = a["run"][0].as_str().unwrap_or_default().to_owned();
                a["installed"] = json!(have.contains(&run));
                a
            })
            .collect(),
    )
}

/// A text file on another machine: `cat`, or `type` where there is none (Windows).
fn read_remote(machine: &str, path: &str) -> Result<Option<String>, String> {
    let (code, out, _) = match run_remote(machine, "cat", &["--".into(), path.into()], None) {
        Err(e) if e.contains("cannot start") => run_remote(machine, "cmd", &["/d".into(), "/c".into(), "type".into(), path.replace('/', "\\")], None)?,
        r => r?,
    };
    if code != 0 {
        return Ok(None);
    }
    if out.len() as u64 > crate::mod_files::LIMIT {
        return Err("Text file exceeds 1 MiB".into());
    }
    Ok(Some(String::from_utf8_lossy(&out).into_owned()))
}

fn exec_future(command: String, args: Vec<String>, cwd: Option<String>, on: Option<String>) -> impl std::future::Future<Output = Result<HostValue, HostError>> + Send {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let r = match on {
            Some(m) => run_remote(&m, &command, &args, cwd).map(|(c, o, e)| (c, String::from_utf8_lossy(&o).into_owned(), String::from_utf8_lossy(&e).into_owned())),
            None => run_blocking(&command, &args, cwd),
        };
        let _ = tx.send_blocking(r);
    });
    async move {
        let (code, stdout, stderr) = rx.recv().await.map_err(|_| HostError::new("the command was lost"))?.map_err(HostError::new)?;
        Ok(HostObject::new().field("code", code as f64).field("stdout", stdout).field("stderr", stderr).into())
    }
}

fn module(inst: Instance, copy: u64) -> HostModule {
    let props = inst.props.clone();
    let feedback_frame = inst.name == "frame-feedback";
    let feedback_from = inst.props.get("from").and_then(Value::as_str).unwrap_or("person").to_owned();
    let feedback_layout = inst.props.get("layout").and_then(Value::as_str).unwrap_or("other").to_owned();
    let (cmd_tx, commands) = async_channel::unbounded::<String>();
    COMMANDS.with(|c| c.borrow_mut().push((copy, inst.name.clone(), cmd_tx)));
    HostModule::new("herdr")
        .declarations(DECLARATIONS)
        .async_function("exec", |a| {
            let command = a.string(0)?.to_owned();
            Ok(exec_future(command, strings(a.get(1)), opt_str(a.get(2), "cwd"), opt_str(a.get(2), "on")))
        })
        .async_function("readTextFile", |a| {
            let path = a.string(0)?.to_owned();
            let on = opt_str(a.get(1), "on");
            Ok(crate::mod_files::read_with(move || match on {
                Some(m) => read_remote(&m, &path),
                None => crate::mod_files::read(Path::new(&path)),
            }))
        })
        .async_function("writeTextFile", |a| {
            let expected = a.get(2).and_then(|v| match v { HostValue::Str(s) => Some(s.clone()), _ => None });
            Ok(crate::mod_files::write_text(a.string(0)?.to_owned(), a.string(1)?.to_owned(), expected))
        })
        .async_function("hc", |a| Ok(exec_future("hc".into(), strings(a.get(0)), None, None)))
        .function("home", |_| Ok(home().to_string_lossy().into_owned().into()))
        .function("appHome", |_| Ok(app_dir().to_string_lossy().into_owned().into()))
        .function("fonts", |_| Ok(to_host(&json!(*FONTS.lock().unwrap()))))
        .function("updates", |_| Ok(to_host(&crate::update::status())))
        .function("presentUpdateNotes", |a| {
            crate::update_notes::presented(&strings(a.get(0)));
            Ok(HostValue::Null)
        })
        .function("telemetry", |_| Ok(to_host(&crate::telemetry::status())))
        .function("feedbackPreview", move |a| {
            let images = a.get(2).and_then(|v| match v { HostValue::Number(n) => Some(*n as usize), _ => None }).unwrap_or(0);
            let body = crate::feedback::draft(a.string(0)?, a.string(1)?, images, &feedback_from, &feedback_layout);
            Ok(to_host(&json!({ "body": body, "available": crate::feedback::available() })))
        })
        .function("feedbackPasteImage", move |_| {
            if !feedback_frame { return Err(HostError::new("feedback screenshots are frame-only")); }
            let bytes = gpui_shell::with_current_app(|cx| {
                cx.read_from_clipboard()?.entries().iter().find_map(|e| match e {
                    gpui::ClipboardEntry::Image(i) => Some(i.bytes.clone()),
                    _ => None,
                })
            }).flatten();
            Ok(match bytes {
                None => HostValue::Null,
                Some(b) => to_host(&match crate::feedback::attach(&b) { Ok(path) => json!({ "path": path }), Err(e) => json!({ "error": e }) }),
            })
        })
        .function("feedbackAttachImages", move |a| {
            if !feedback_frame { return Err(HostError::new("feedback screenshots are frame-only")); }
            let paths = strings(a.get(0)).into_iter().map(std::path::PathBuf::from).collect::<Vec<_>>();
            Ok(to_host(&json!(crate::feedback::attach_files(&paths))))
        })
        .async_function("feedbackPickImages", move |_| {
            if !feedback_frame { return Err(HostError::new("feedback screenshots are frame-only")); }
            let picked = gpui_shell::with_current_app(|cx| cx.prompt_for_paths(gpui::PathPromptOptions {
                files: true, directories: false, multiple: true, prompt: Some("Add".into()),
            })).ok_or_else(|| HostError::new("no window to pick from"))?;
            Ok(async move {
                let paths = picked.await.ok().and_then(Result::ok).flatten().unwrap_or_default();
                let out = crate::feedback::attach_files(&paths);
                Ok(to_host(&Value::Array(out)))
            })
        })
        .async_function("knownAgents", |a| {
            let on = opt_str(a.get(0), "on");
            // The table is loaded on the window thread; workers have only defaults.
            let list = crate::programs::startable();
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let _ = tx.send_blocking(known_agents(on, list));
            });
            Ok(async move { Ok(to_host(&rx.recv().await.unwrap_or(Value::Array(Vec::new())))) })
        })
        .async_function("pickFolder", |a| {
            let prompt = a.get(0).map(from_host).and_then(|v| v.get("prompt").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| "Open".into());
            let picked = gpui_shell::with_current_app(|cx| cx.prompt_for_paths(gpui::PathPromptOptions {
                files: false, directories: true, multiple: false, prompt: Some(prompt.into()),
            })).ok_or_else(|| HostError::new("no window to pick from"))?;
            Ok(async move {
                let path = picked.await.ok().and_then(Result::ok).flatten().and_then(|p| p.into_iter().next());
                Ok(path.map(|p| HostValue::Str(p.to_string_lossy().into_owned())).unwrap_or(HostValue::Null))
            })
        })
        .function("feedbackClearImages", move |_| {
            if feedback_frame { crate::feedback::clear_shots(); }
            Ok(HostValue::Null)
        })
        .async_function("sendFeedback", move |a| {
            if !feedback_frame { return Err(HostError::new("sending feedback is frame-only")); }
            let body = from_host(a.value(0)?);
            let shots: Vec<String> = from_host(a.value(1).unwrap_or(&HostValue::Null)).as_array()
                .map(|l| l.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            Ok(async move {
                Ok(match crate::feedback::send(body, shots).await {
                    Ok(()) => HostValue::Null,
                    Err(why) => HostValue::Str(why),
                })
            })
        })
        .function("themes", |_| {
            let list: Vec<Value> = crate::settings::themes().into_iter().map(|(name, colors)| json!({ "light": crate::settings::theme_is_light(&name), "name": name, "colors": colors })).collect();
            Ok(to_host(&Value::Array(list)))
        })
        .function("playSound", |a| {
            crate::sound::play(a.string(0)?);
            Ok(HostValue::Null)
        })
        .function("run", |a| {
            let name = a.string(0)?.to_owned();
            let args = a.get(1).map(from_host).unwrap_or(Value::Null);
            if let Some(command) = crate::commands::find(&name) {
                crate::commands::validate_args(command, &args).map_err(HostError::new)?;
            }
            let final_kill = (name == "terminal.kill").then(|| FINAL_KILLS.with_borrow(Clone::clone)).flatten();
            if let Some((core, deadline)) = final_kill {
                if let Err(error) = core.call_timeout(&name, args, deadline.saturating_duration_since(std::time::Instant::now())) {
                    tracing::warn!("final terminal.kill: {error:#}");
                }
            } else {
                request(Request::Run { name, args });
            }
            Ok(HostValue::Null)
        })
        .function("spaces", |_| Ok(read(|s| &s.spaces)))
        .function("machines", |_| Ok(read(|s| &s.machines)))
        .function("agents", |_| Ok(read(|s| &s.agents)))
        .function("panes", |_| Ok(read(|s| &s.panes)))
        .function("setAgentOrder", |a| {
            AGENT_ORDER.with(|s| *s.borrow_mut() = strings(a.get(0)));
            Ok(HostValue::Null)
        })
        .function("terminals", |_| Ok(read(|s| &s.terminals)))
        .function("focused", |_| Ok(read(|s| &s.focused)))
        .function("layout", |_| Ok(read(|s| &s.layout)))
        .function("layouts", |_| Ok(to_host(&json!(crate::window::layout_names()))))
        .function("commands", |_| {
            let list: Vec<Value> = crate::commands::COMMANDS
                .iter()
                .map(|c| {
                    let args: Vec<Value> = c.args.iter().map(|a| json!({ "name": a.name, "required": a.required })).collect();
                    json!({ "name": c.name, "args": args, "about": c.about })
                })
                .collect();
            Ok(to_host(&Value::Array(list)))
        })
        .function("window", |_| Ok(read(|s| &s.window)))
        .function("theme", |_| Ok(to_host(&serde_json::to_value(&*crate::settings::theme()).unwrap_or(Value::Null))))
        .function("settings", |_| Ok(to_host(&serde_json::to_value(&*crate::settings::get()).unwrap_or(Value::Null))))
        .function("keys", |_| {
            let list: Vec<Value> = crate::keys::listing().into_iter().map(|(s, k, a)| json!({ "section": s, "keys": k, "action": a })).collect();
            Ok(to_host(&Value::Array(list)))
        })
        .function("openOverlay", |a| {
            let mut spec = from_host(a.value(0)?);
            if spec.get("mod").is_none()
                && let Some(old) = spec.get("piece").cloned()
            {
                spec["mod"] = old;
            }
            if spec.get("mod").and_then(Value::as_str).is_none() && !spec.get("layout").is_some_and(Value::is_object) {
                return Err(HostError::new("openOverlay needs { mod: \"<folder>\" } or { layout: {...} }"));
            }
            let id = new_overlay_id();
            request(Request::OpenOverlay { id: id.clone(), spec });
            Ok(id.into())
        })
        .function("closeOverlay", |a| {
            request(Request::CloseOverlay { id: a.string(0)?.to_owned() });
            Ok(HostValue::Null)
        })
        .function("notify", |a| {
            let id = new_overlay_id();
            let n = from_host(a.value(0)?);
            request(Request::OpenOverlay { id: id.clone(), spec: json!({ "kind": "toast", "notification": n }) });
            Ok(id.into())
        })
        .function("placement", {
            let float = props.get("float").and_then(Value::as_str).map(str::to_owned);
            move |_| Ok(float.as_ref().map_or(HostValue::Null, |id| read(|s| s.floats.get(id).unwrap_or(&Value::Null))))
        })
        .function("props", move |_| Ok(to_host(&props)))
        .function("shared", |a| {
            let k = a.string(0)?;
            Ok(with_shared(|s| s.values.get(k).map(to_host).unwrap_or(HostValue::Null)))
        })
        .function("setShared", |a| {
            set_shared(a.string(0)?.to_owned(), from_host(a.value(1)?));
            Ok(HostValue::Null)
        })
        .async_function("watchFolder", |a| {
            let path = a.string(0)?.to_owned();
            Ok(async move { crate::files::next_change(path).await.map(|v| to_host(&json!(v))).map_err(HostError::new) })
        })
        .async_function("nextCommand", move |_| {
            let rx = commands.clone();
            Ok(async move { rx.recv().await.map(HostValue::Str).map_err(|_| HostError::new("herdr-app is shutting down")) })
        })
}

/// The runtime's base layer, themed from settings.json.
pub fn init(cx: &mut App) {
    gpui_shell::init(cx);
    apply_theme(cx);
}

/// Maps the theme onto the tokens mods read through `cx.theme().colors`.
pub fn apply_theme(cx: &mut App) {
    let t = crate::settings::theme();
    let g = |k: &str| t.get(k).cloned().unwrap_or_default();
    let palette = json!({
        "background": g("background"), "foreground": g("foreground"),
        "surface": g("surface"), "surface_foreground": g("foreground"),
        "primary": g("accent"), "primary_foreground": g("accent_foreground"),
        "secondary": g("raised"), "secondary_foreground": g("foreground"),
        "muted": g("raised"), "muted_foreground": g("muted"),
        "accent": g("hover"), "accent_foreground": g("foreground"),
        "destructive": g("blocked"), "destructive_foreground": g("background"),
        "border": g("border"), "input": g("input"), "ring": g("ring"),
        "selection": g("selection"),
    });
    match serde_json::from_value::<gpui_base::ColorTokens>(palette) {
        Ok(colors) => {
            let theme = gpui_base::Theme::global_mut(cx);
            theme.appearance = if crate::settings::get().dark { gpui_base::ThemeAppearance::Dark } else { gpui_base::ThemeAppearance::Light };
            theme.tokens.colors = colors;
        }
        Err(e) => tracing::error!("theme did not map onto mod tokens: {e}"),
    }
}

fn capabilities() -> Capabilities {
    Capabilities::new()
        .read_roots([home()])
        .write_roots([app_dir()])
        .execute(ExecuteGrant::Unrestricted)
        .network(NetworkGrant::Unrestricted)
        .storage(true)
        .clipboard_read(true)
        .clipboard_write(true)
}

/// What mods may touch, and the `herdr` module they import. Also used by `herdr-app check`.
pub fn install_policy() -> async_channel::Receiver<Request> {
    gpui_shell::policy::update_default(|p| {
        p.with_capabilities(capabilities())
            .with_host_module(module(Instance { name: String::new(), props: Value::Null }, 0))
            .expect("herdr module is valid")
    });
    let (tx, rx) = async_channel::unbounded();
    let _ = REQUESTS.set(tx);
    rx
}

/// Gives the next mod copy to load a policy of its own: its `herdr` module
/// (its props, its identity) and its localStorage file. The shell hands every
/// view a copy of the default that *shares* modules and storage, so each copy
/// needs a fresh default rather than an edit of the old one.
/// Readies the next load of `inst`; returns the copy's number (for commands).
pub fn prepare_load(inst: &Instance) -> u64 {
    let copy = COPY_SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let dir = app_dir().join("data").join(&inst.name);
    let _ = std::fs::create_dir_all(&dir);
    let file = if inst.props.is_null() {
        "store.json".to_owned()
    } else {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        inst.props.to_string().hash(&mut h);
        format!("store-{:016x}.json", h.finish())
    };
    let policy = gpui_shell::policy::Policy::new()
        .with_application(inst.key())
        .with_capabilities(capabilities())
        .with_storage_path(dir.join(file))
        .with_host_module(module(inst.clone(), copy))
        .expect("herdr module is valid");
    gpui_shell::policy::set_default(policy);
    copy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_focus_is_opt_in_and_commands_only_reach_the_shown_copy() {
        for metadata in [json!({}), json!({"focusable": false})] {
            assert!(!serde_json::from_value::<Hint>(metadata).unwrap().focusable);
        }
        assert!(serde_json::from_value::<Hint>(json!({"focusable": true})).unwrap().focusable);

        let (first_tx, first) = async_channel::unbounded();
        let (second_tx, second) = async_channel::unbounded();
        let (hidden_tx, hidden) = async_channel::unbounded();
        COMMANDS.with_borrow_mut(|c| *c = vec![
            (1, "same-mod".into(), first_tx),
            (2, "same-mod".into(), second_tx),
            (3, "same-mod".into(), hidden_tx),
        ]);
        set_shown_copies([1, 2].into());
        assert!(send_command_to(2, "focus"));
        assert_eq!(second.try_recv().unwrap(), "focus");
        assert!(first.is_empty());
        assert!(hidden.is_empty());
        assert!(!send_command_to(3, "focus"));
        assert!(!send_command_to(99, "focus"));
        // Existing mod key bindings still broadcast to every shown copy.
        assert!(send_command("same-mod", "existing-key"));
        assert_eq!(first.try_recv().unwrap(), "existing-key");
        assert_eq!(second.try_recv().unwrap(), "existing-key");
        assert!(hidden.is_empty());
        COMMANDS.with_borrow_mut(Vec::clear);
        set_shown_copies(Default::default());
    }


    #[cfg(unix)]
    #[test]
    fn known_agents_uses_snapshot_and_refreshes_changed_commands() {
        let snapshot = vec![json!({"key": "claude", "run": ["herdr_issue57_not_installed"]})];
        let first = std::thread::spawn(move || known_agents(None, snapshot)).join().unwrap();
        assert_eq!(first[0]["run"], json!(["herdr_issue57_not_installed"]));
        assert_eq!(first[0]["installed"], false);
        let snapshot = vec![json!({"key": "claude", "run": ["printf"]}), json!({"key": "claude-alt", "run": ["printf"]})];
        let next = std::thread::spawn(move || known_agents(None, snapshot)).join().unwrap();
        assert_eq!(next.as_array().unwrap().len(), 2);
        assert_eq!(next[0]["installed"], true);
        assert_eq!(next[1]["key"], "claude-alt");
        assert_eq!(next[1]["installed"], true);
    }

    #[test]
    fn shared_bursts_persist_only_complete_snapshots_and_finish_at_final_state() {
        let root = if cfg!(target_os = "linux") { PathBuf::from("/var/tmp") } else { std::env::temp_dir() };
        let dir = root.join(format!("herdr-shared-stress-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared.json");
        let reading = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let reader = {
            let path = path.clone();
            let reading = reading.clone();
            std::thread::spawn(move || {
                while reading.load(std::sync::atomic::Ordering::Relaxed) {
                    match std::fs::read(&path) {
                        Ok(bytes) => { serde_json::from_slice::<Value>(&bytes).unwrap(); }
                        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
                    }
                    std::thread::yield_now();
                }
            })
        };
        let expected = std::thread::spawn(move || {
            SHARED.with_borrow_mut(|s| *s = Some(SharedStore::load(path)));
            for i in 0..2000 {
                set_shared("changing".into(), json!("x".repeat(if i % 2 == 0 { 16000 } else { 3 })));
                set_shared("revision".into(), json!(i));
            }
            set_shared("final".into(), json!(true));
            finish_shared();
            with_shared(|s| s.values.clone())
        }).join().unwrap();
        reading.store(false, std::sync::atomic::Ordering::Relaxed);
        reader.join().unwrap();
        let actual: serde_json::Map<String, Value> = serde_json::from_slice(&std::fs::read(dir.join("shared.json")).unwrap()).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_shared_file_is_kept_before_starting_empty() {
        let root = if cfg!(target_os = "linux") { PathBuf::from("/var/tmp") } else { std::env::temp_dir() };
        let dir = root.join(format!("herdr-shared-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shared.json");
        let corrupt = b"{\"kept\":true} old snapshot tail";
        std::fs::write(&path, corrupt).unwrap();
        let store = SharedStore::load(path.clone());
        assert!(store.values.is_empty());
        assert!(store.writable);
        assert!(!path.exists());
        let backup = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        assert!(backup.file_name().unwrap().to_string_lossy().starts_with("shared.json.corrupt-"));
        assert_eq!(std::fs::read(&backup).unwrap(), corrupt);
        crate::persist::write_atomic(&path, "{}").unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), corrupt);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn navigation_follows_mod_order() {
        let available = || vec!["t1".into(), "t2".into(), "t3".into()];
        AGENT_ORDER.with(|s| s.borrow_mut().clear());
        assert_eq!(agent_order(available()), available());
        AGENT_ORDER.with(|s| *s.borrow_mut() = vec!["t3".into(), "gone".into(), "t1".into(), "t3".into()]);
        assert_eq!(agent_order(available()), vec!["t3", "t1", "t2"]);
        assert!(agent_order(Vec::new()).is_empty());
        AGENT_ORDER.with(|s| *s.borrow_mut() = vec!["t2".into(), "t1".into(), "t3".into()]);
        assert_eq!(agent_order(available()), vec!["t2", "t1", "t3"]);
        AGENT_ORDER.with(|s| s.borrow_mut().clear());
    }

    #[test]
    fn an_added_file_makes_a_shipped_mod_edited() {
        let dir = std::env::temp_dir().join(format!("herdr-app-shipped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("icons")).unwrap();
        std::fs::write(dir.join("main.js"), "a").unwrap();
        std::fs::write(dir.join("icons/x.svg"), "b").unwrap();
        let files: Files = [("main.js".to_owned(), fingerprint(b"a")), ("icons/x.svg".to_owned(), fingerprint(b"b"))].into();
        assert!(as_shipped(&dir, &files));
        std::fs::write(dir.join("gpui-kit.d.ts"), "generated").unwrap();
        std::fs::write(dir.join("jsconfig.json"), "generated").unwrap();
        assert!(as_shipped(&dir, &files));
        std::fs::write(dir.join(".DS_Store"), "finder").unwrap();
        std::fs::create_dir(dir.join(".vscode")).unwrap();
        std::fs::write(dir.join(".vscode/settings.json"), "{}").unwrap();
        assert!(as_shipped(&dir, &files));
        std::fs::write(dir.join("mine.js"), "c").unwrap();
        assert!(!as_shipped(&dir, &files));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cloned_mods_rewrite_assets_and_self_overlays_and_reject_nonobject_metadata() {
        let root = std::env::temp_dir().join(format!("herdr-mod-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("original")).unwrap();
        std::fs::write(root.join("original/mod.json"), "{}").unwrap();
        std::fs::write(root.join("original/main.js"), r#"const ASSETS = "mods/original"; const icon = 'mods/original/icon.svg'; openOverlay({ mod: "original" });"#).unwrap();
        std::fs::write(root.join("original/icon.svg"), "<svg/>").unwrap();
        clone_mod_at(&root, "original", "copy").unwrap();
        let text = std::fs::read_to_string(root.join("copy/main.js")).unwrap();
        assert!(text.contains("\"mods/copy\"") && text.contains("'mods/copy/icon.svg'") && text.contains("mod: \"copy\""));
        assert_eq!(std::fs::read_to_string(root.join("copy/icon.svg")).unwrap(), "<svg/>");
        assert!(clone_mod_at(&root, "original", "copy").is_err());
        for invalid in ["[]", "42", "true", "null", "\"text\""] {
            std::fs::write(root.join("original/mod.json"), invalid).unwrap();
            assert!(clone_mod_at(&root, "original", "bad").is_err());
            assert!(!root.join("bad").exists());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layout_formatting_is_not_an_edit_and_rescued_clicks_still_apply() {
        use crate::layout::{Node, Size};
        use crate::layout_sync::State;
        let root = std::env::temp_dir().join(format!("herdr-layout-click-rescue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("layouts");
        std::fs::create_dir_all(dir.join(".state")).unwrap();
        std::fs::create_dir_all(dir.join(".base")).unwrap();
        let shipped = r#"{"id":"root","type":"block","content":{"mod":"files"},"size":{"px":360}}"#;
        // No record: formatting, BOM and whole-number floats are not edits.
        std::fs::write(dir.join("main.json"), "\u{feff}{\r\n \"type\":\"block\", \"id\":\"root\", \"size\":{\"px\":360.0}, \"content\":{\"mod\":\"files\"}}").unwrap();
        sync_layouts(&root, &[("main", shipped)]).unwrap();
        assert!(!dir.join("main-edited.json").exists());
        assert!(!dir.join(".base").exists());
        let mut file: Node = serde_json::from_str(shipped).unwrap();
        file.size = Some(Size::Px { px: 400.0 });
        std::fs::write(dir.join("main.json"), crate::layout_sync::text_of(&file)).unwrap();
        let mut shown = file.clone();
        shown.size = Some(Size::Px { px: 500.0 });
        let mut state = State::between(&file, &shown);
        state.file = serde_json::to_value(&file).ok();
        std::fs::write(dir.join(".state/main.json"), serde_json::to_string(&state).unwrap()).unwrap();
        sync_layouts(&root, &[("main", shipped)]).unwrap();
        let mut rescued: Node = serde_json::from_str(&std::fs::read_to_string(dir.join("main-edited.json")).unwrap()).unwrap();
        let mut clicks: State = serde_json::from_str(&std::fs::read_to_string(dir.join(".state/main-edited.json")).unwrap()).unwrap();
        clicks.forget_since_saved(&rescued);
        clicks.apply(&mut rescued);
        assert_eq!(rescued.size, Some(Size::Px { px: 500.0 }));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_0_1_x_layout_is_kept_aside_quietly_not_rescued() {
        let root = std::env::temp_dir().join(format!("herdr-layout-0-1-x-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("layouts");
        std::fs::create_dir_all(&dir).unwrap();
        // 0.1.x wrote no record of what it installed.
        std::fs::write(dir.join("main.json"), r#"{"id":"root","type":"block","content":{"mod":"old"}}"#).unwrap();
        take_sync_notes();
        sync_layouts(&root, &[("main", r#"{"id":"root","type":"block","content":{"mod":"new"}}"#)]).unwrap();
        assert!(std::fs::read_to_string(dir.join("main.json")).unwrap().contains("new"));
        let aside = dir.join(format!("main.json.before-{}", env!("CARGO_PKG_VERSION")));
        assert!(std::fs::read_to_string(aside).unwrap().contains("old"));
        assert!(!dir.join("main-edited.json").exists());
        assert!(take_sync_notes().is_empty(), "no toast for a file nobody can say was edited");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layouts_restore_preserve_edits_clicks_and_drop_only_untouched() {
        let root = std::env::temp_dir().join(format!("herdr-layout-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("layouts");
        sync_layouts(&root, &[("a", "a1"), ("b", "b1"), ("c", "c1")]).unwrap();
        std::fs::create_dir_all(dir.join(".state")).unwrap();
        std::fs::write(dir.join(".state/a.json"), "clicks").unwrap();
        std::fs::write(dir.join("a.json"), "mine").unwrap();
        std::fs::write(dir.join("b.json"), "kept").unwrap();
        sync_layouts(&root, &[("a", "a2")]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "a2");
        assert_eq!(std::fs::read_to_string(dir.join("a-edited.json")).unwrap(), "mine");
        assert_eq!(std::fs::read_to_string(dir.join("b.json")).unwrap(), "kept");
        assert!(!dir.join("c.json").exists());
        assert_eq!(std::fs::read_to_string(dir.join(".state/a.json")).unwrap(), "clicks");
        std::fs::write(dir.join("a.json"), "again").unwrap();
        sync_layouts(&root, &[("a", "a2")]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a-edited-2.json")).unwrap(), "again");
        assert_eq!(std::fs::read_to_string(dir.join("a-edited.json")).unwrap(), "mine");
        std::fs::remove_file(dir.join("a.json")).unwrap();
        sync_layouts(&root, &[("a", "a3")]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.json")).unwrap(), "a3");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_mods_restore_shipped_and_move_edits_to_clones() {
        let root = std::env::temp_dir().join(format!("herdr-app-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (mods, record) = (root.join("mods"), root.join("installed.json"));
        let read = |p: &str| std::fs::read_to_string(mods.join(p)).unwrap_or_default();

        let v1: &[(&str, &[u8])] = &[("a/main.js", b"a1"), ("a/old.js", b"x"), ("b/main.js", b"b1"), ("c/main.js", b"c1")];
        sync_defaults(&mods, &record, v1).unwrap();
        assert_eq!(read("a/main.js"), "a1");
        let before = std::fs::metadata(mods.join("a/main.js")).unwrap().modified().unwrap();
        sync_defaults(&mods, &record, v1).unwrap();
        assert_eq!(std::fs::metadata(mods.join("a/main.js")).unwrap().modified().unwrap(), before, "untouched mods are not rewritten");

        std::fs::write(mods.join("b/main.js"), "mine").unwrap();
        std::fs::remove_dir_all(mods.join("c")).unwrap();
        let v2: &[(&str, &[u8])] = &[("a/main.js", b"a2"), ("b/main.js", b"b2"), ("c/main.js", b"c2")];
        sync_defaults(&mods, &record, v2).unwrap();
        assert_eq!(read("a/main.js"), "a2", "untouched mods update");
        assert!(!mods.join("a/old.js").exists(), "files the update dropped go");
        assert_eq!(read("b/main.js"), "b2", "edited mods are restored");
        assert_eq!(read("b-edited/main.js"), "mine", "edits become a clone");
        assert_eq!(read("c/main.js"), "c2", "deleted mods are restored");
        std::fs::write(mods.join("b/main.js"), "changed again").unwrap();

        // No longer shipped: untouched ones go, edited ones stay, and so do
        // ones someone added a file to.
        let v3: &[(&str, &[u8])] = &[("c/main.js", b"c2"), ("d/main.js", b"d1")];
        sync_defaults(&mods, &record, v3).unwrap();
        std::fs::write(mods.join("d/notes.md"), "my notes").unwrap();
        let v4: &[(&str, &[u8])] = &[("c/main.js", b"c2")];
        sync_defaults(&mods, &record, v4).unwrap();
        assert!(!mods.join("a").exists(), "untouched dropped defaults go");
        assert_eq!(read("b/main.js"), "changed again", "edited dropped defaults stay");
        assert_eq!(read("b-edited/main.js"), "mine", "clones stay untouched");
        assert_eq!(read("d/notes.md"), "my notes", "a dropped default with an added file stays");

        std::fs::remove_dir_all(&root).unwrap();
    }
}
