//! The window. The chrome (`layouts/<name>.json`) is a tree of the locked
//! primitives; a `{ "tab": "active" }` block shows the selected
//! tab's terminals as it arranges them. Every visible part that isn't a
//! terminal is a mod. Commands (commands.rs) are the one way to change
//! anything: keys, mods, menus and agents all run them.

use crate::hc::Core;
mod floats;
mod movable;
mod pane_drag;
mod titlebar;

use crate::layout::{Axis, Background as Paint, Content, Fit, HeaderContent, Kind, Line, Node, PaneFrame, Size as Span};
use crate::model::{self, Tab, Model};
use crate::mods::{self, Found, Instance, Request};
use crate::settings;
use crate::layout_sync;
use crate::terminal::{TerminalEvent, TerminalView};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_shell::action::ShellAction;
use gpui_shell::{ShellRoot, ShellRuntime, Watcher};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const HEADER_HEIGHT: f32 = 30.0;
const HANDLE: f32 = 6.0;

// The three native buttons plus their trailing breathing room (also used by the tabs mod).
const TRAFFIC_LIGHT_WIDTH: f32 = 64.0;

fn traffic_light_divider_inset(n: &Node, axis: Axis, index: usize, macos: bool, zoom: f32) -> f32 {
    if !macos || axis != Axis::Row || n.titlebar != Some(crate::layout::Titlebar::Layout) {
        return 0.0;
    }
    let Some([x, _]) = n.traffic_lights else { return 0.0 };
    let shown: Vec<_> = n.children().iter().filter(|child| !child.hidden).collect();
    let mut edge = n.padding.map_or(0.0, |padding| padding.axes().1)
        + index as f32 * (n.gap.unwrap_or(0.0) + n.divider.as_ref().map_or(0.0, |line| line.width));
    for child in shown.iter().take(index + 1) {
        let Some(Span::Px { px }) = child.size else { return 0.0 };
        edge += px;
    }
    if edge * zoom < x || edge * zoom > x + TRAFFIC_LIGHT_WIDTH {
        return 0.0;
    }
    // Keep the toolbar continuous above the rail; the divider starts with the content.
    shown.iter().skip(index).take(2).filter_map(|column| {
        let header = column.children().iter().find(|child| !child.hidden)?;
        match header.size {
            Some(Span::Px { px }) if header.window_drag => Some(px),
            _ => None,
        }
    }).reduce(f32::min).unwrap_or(0.0)
}

fn separator_line(row: bool, width: f32, color: Hsla, top: f32) -> Div {
    div().bg(color)
        .when(row, |d| d.w(px(width)))
        .when(row && top == 0.0, |d| d.h_full())
        .when(row && top > 0.0, |d| d.absolute().top(px(top)).bottom_0())
        .when(!row, |d| d.h(px(width)).w_full())
}

fn c(name: &str) -> Hsla {
    settings::color_or_literal(name)
}

fn file(name: &str) -> PathBuf {
    mods::app_dir().join(name)
}

fn is_dark(a: gpui::WindowAppearance) -> bool {
    matches!(a, gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark)
}

/// A layout's theme after picking `name`: it takes the half of its own
/// lightness; the other half stays (a single theme of the other lightness
/// becomes that half).
fn with_theme(was: Option<crate::layout::ThemeChoice>, name: String) -> crate::layout::ThemeChoice {
    use crate::layout::ThemeChoice::{One, Pair};
    let light = settings::theme_is_light(&name);
    let (l, d) = match was {
        Some(Pair { light, dark }) => (light, dark),
        Some(One(x)) if settings::theme_is_light(&x) == light => return One(name),
        Some(One(x)) => (x.clone(), x),
        None => (settings::DEFAULT_LIGHT.into(), settings::DEFAULT_DARK.into()),
    };
    if light { Pair { light: name, dark: d } } else { Pair { light: l, dark: name } }
}

fn mtime(p: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

pub fn shell() -> String {
    if cfg!(windows) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let pwsh = std::env::split_paths(&path).any(|d| d.join("pwsh.exe").is_file());
        return if pwsh { "pwsh.exe" } else { "powershell.exe" }.into();
    }
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

fn unix_agent_argv(sh: String, cmd: Vec<String>, login: bool) -> Vec<String> {
    let fish = std::path::Path::new(&sh).file_name().and_then(|s| s.to_str()) == Some("fish");
    let script = match (fish, login) {
        (true, true) => "command $argv[2..]; exec $argv[1] -li",
        (true, false) => "command $argv[2..]; exec $argv[1] -i",
        (false, true) => "\"$@\"; exec \"$0\" -li",
        (false, false) => "\"$@\"; exec \"$0\" -i",
    };
    let mut run = vec![sh.clone(), if login { "-lic" } else { "-ic" }.into(), script.into(), sh];
    run.extend(cmd);
    run
}

fn launch_failure(record: &Value, output: &str) -> Option<String> {
    let code = record["exit_code"].as_i64().filter(|c| *c != 0)?;
    let elapsed = record["finished_ms"].as_u64()?.checked_sub(record["created_ms"].as_u64()?)?;
    if record["reason"].as_str() != Some("exited") || elapsed > 2000 {
        return None;
    }
    let lines = output.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    Some(if tail.is_empty() { format!("Exit code {code}") } else { format!("Exit code {code}\n{tail}") })
}

/// The shipped layouts (defaults/layouts): `main` is Herdr's window (spaces
/// sidebar, tab bar).
const LAYOUTS: &[(&str, &str)] = &[
    ("main", include_str!("../defaults/layouts/main.json")),
    ("terminals", include_str!("../defaults/layouts/terminals.json")),
    ("win98", include_str!("../defaults/layouts/win98.json")),
];

/// Pictures shipped layouts name as their `preview`, written beside them.
const LAYOUT_PREVIEWS: &[(&str, &str)] = &[("win98.svg", include_str!("../defaults/layouts/win98.svg"))];

pub fn shipped_layouts() -> Vec<&'static str> {
    LAYOUTS.iter().map(|(n, _)| *n).collect()
}

fn shipped_layout(name: &str) -> &'static str {
    LAYOUTS.iter().find(|(n, _)| *n == name).unwrap_or(&LAYOUTS[0]).1
}

/// A layout as shipped (main for names Herdr doesn't ship).
fn default_layout(name: &str) -> Node {
    serde_json::from_str(shipped_layout(name)).expect("defaults/layouts are valid")
}

fn layout_path(name: &str) -> PathBuf {
    file(&format!("layouts/{name}.json"))
}

/// Layout names on disk, in order: main first, then the rest alphabetically.
/// A layout whose mods are gone (Zen after the update) isn't offered;
/// `layout.switch {name}` still reaches it.
pub(crate) fn layout_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(file("layouts"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_owned))
        .filter(|n| missing_mods(n).is_empty())
        .collect();
    names.sort_by_key(|n| (n != "main", n.clone()));
    names
}

fn state_path(name: &str) -> PathBuf {
    file(&format!("layouts/.state/{name}.json"))
}

/// The clicks on a layout (layout_sync.rs), none if unreadable.
fn read_state(name: &str) -> layout_sync::State {
    std::fs::read_to_string(state_path(name)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn state_text(s: &layout_sync::State) -> String {
    serde_json::to_string_pretty(s).unwrap_or_default() + "\n"
}

/// Where to set `path` aside: `<path>.<tag>-<time>`, with `-2`, `-3` ... when
/// that's taken, so an older one is never replaced.
pub(crate) fn aside(path: &std::path::Path, tag: &str) -> PathBuf {
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    (1..).map(|i| path.with_file_name(if i == 1 { format!("{name}.{tag}-{stamp}") } else { format!("{name}.{tag}-{stamp}-{i}") })).find(|p| !p.exists()).unwrap_or_else(|| path.to_path_buf())
}

/// Writes through a temporary file and a rename, so a reader never sees half
/// a file. Says whether it worked.
fn write_atomic(path: &std::path::Path, text: &str) -> bool {
    match crate::persist::write_atomic(path, text) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!("cannot write {}: {error}", path.display());
            false
        }
    }
}

/// The mods a layout file names that aren't in mods/ (frame mods are always there).
fn missing_mods(name: &str) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => o.iter().for_each(|(k, v)| match v {
                Value::String(s) if k == "mod" => out.push(s.clone()),
                _ => walk(v, out),
            }),
            Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let Some(layout) = std::fs::read_to_string(layout_path(name)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return vec![] };
    let mut used = Vec::new();
    walk(&layout, &mut used);
    used.sort();
    used.dedup();
    used.retain(|m| !m.starts_with("frame-") && !mods::mods_dir().join(m).is_dir());
    used
}

fn install_layouts() {
    if let Err(e) = mods::sync_layouts(&mods::app_dir(), LAYOUTS) {
        tracing::warn!("cannot install layouts: {e}");
    }
    for (name, text) in LAYOUT_PREVIEWS {
        let _ = std::fs::write(file(&format!("layouts/{name}")), text);
    }
}

fn clone_layout(name: &str, to: &str) -> std::io::Result<()> {
    if !mods::valid_name(name) || !mods::valid_name(to) {
        return Err(std::io::Error::other("names use letters, digits, - or _"));
    }
    let mut node: Node = serde_json::from_str(&std::fs::read_to_string(layout_path(name))?)?;
    node.cloned_from = Some(crate::layout::ClonedFrom { name: name.into(), version: env!("CARGO_PKG_VERSION").into() });
    let text = layout_sync::text_of(&node);
    use std::io::Write;
    let mut dest = std::fs::OpenOptions::new().write(true).create_new(true).open(layout_path(to))?;
    let result = (|| {
        dest.write_all(text.as_bytes())?;
        mods::copy_layout_state(&mods::app_dir(), name, to)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(layout_path(to));
        let _ = std::fs::remove_file(state_path(to));
    }
    result
}

struct Term {
    view: Entity<TerminalView>,
    address: String,
    focus: FocusHandle,
    _sub: Subscription,
}

enum TerminalChange {
    Keep { space: String, tab: String, pane: String, name: String, target: Option<String>, axis: Axis },
    Park,
}

struct PendingTerminalChange {
    change: TerminalChange,
    reply: std::sync::mpsc::Receiver<Result<(), String>>,
}

/// The slot of a terminal shown in a pane in place of its own (`<pane>#swap`).
const SWAP: &str = "#swap";

struct Copy {
    inst: Instance,
    /// Its number for commands (`mods::prepare_load`); 0 if it never loaded.
    id: u64,
    root: Option<Entity<ShellRoot>>,
    _watch: Option<Watcher>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum OverlayKind {
    Dialog,
    Popup,
    Toast,
    /// Stays until closed, moves where it's dropped, comes to the front when
    /// pressed (`movable.rs`). Never holds the keyboard on its own.
    Window,
}

impl OverlayKind {
    /// Dialogs and popups hold the keyboard while open (Esc closes them).
    fn modal_keys(self) -> bool {
        matches!(self, OverlayKind::Dialog | OverlayKind::Popup)
    }
}

struct Overlay {
    id: String,
    kind: OverlayKind,
    slot: String,
    at: Point<Pixels>,
    /// As designed; drawn at the current zoom. Up to 1: a share of the window.
    size: Size<f32>,
    expires: Option<Instant>,
    focus: FocusHandle,
    /// Blocks instead of one mod (terminals too), ids prefixed with the overlay's.
    layout: Option<Node>,
    /// `at: "top"`: centered, this far below the window's top.
    top: Option<f32>,
    /// As tall as its content (size.height is the most).
    fit: bool,
    /// A click beside it doesn't close it (the first-run welcome).
    modal: bool,
    /// `frame: "none"`: no border, corners, shadow or background; the mod draws its own.
    bare: bool,
    /// A window: where its top left is (None: centered) and its name (`key`).
    place: Option<Point<Pixels>>,
    key: Option<String>,
    /// The app's own screen (a `frame-` mod: What's new, the welcome): only
    /// its own close ends it, never a layout switch.
    frame: bool,
}

struct Drag {
    container: String,
    index: usize,
    axis: Axis,
}

#[derive(Default)]
struct Watched {
    settings: Option<SystemTime>,
    keys: Option<SystemTime>,
    programs: Option<SystemTime>,
}

pub struct AppWindow {
    core: Arc<Core>,
    runtime: Option<Rc<ShellRuntime>>,
    chrome: Node,
    /// Which layout `chrome` is (`layouts/<name>.json`).
    layout_name: String,
    /// The agent an `{ "agent": "focused" }` block shows (by address); any core
    /// terminal, so sub-agents outside workspaces work too.
    agent: Option<String>,
    /// The agent view that has the keyboard, so it's only taken on change.
    agent_focused: Option<String>,
    /// Panes showing another terminal for now (pane → address), e.g. a sub-agent
    /// looked at in place of its parent. Not saved; the pane keeps its own.
    swapped: HashMap<String, String>,
    float_drag: Option<floats::FloatDrag>,
    pane_drag: Option<pane_drag::PaneDrag>,
    pane_grip_hover: Option<String>,
    /// Each pane's top row the grip sits centered in: its top header, else a strip.
    pane_grip_rows: RefCell<HashMap<String, f32>>,
    /// A window (`kind: "window"` overlay) being pressed or dragged.
    win_drag: Option<movable::WinDrag>,
    float_anims: HashMap<String, floats::FloatAnim>,
    float_reveal: HashMap<String, floats::Reveal>,
    float_hover: HashSet<String>,
    /// Bumped on every hover change, so a delayed leave knows it's still current.
    float_leave: HashMap<String, u64>,
    model: Model,
    /// Terminal views by block id (panes and chrome terminal blocks).
    terms: HashMap<String, Term>,
    /// Mod copies by slot (block id, `<stack>#header`, or `overlay:<id>`).
    copies: HashMap<String, Copy>,
    overlays: Vec<Overlay>,
    folders: BTreeMap<String, Found>,
    /// Mods placed once; later the layout decides where (or whether) they show.
    seen: BTreeSet<String>,
    bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    drag: Option<Drag>,
    written_layout: String,
    /// The shown layout as its file has it; `chrome` is this plus the clicks
    /// (layout_sync.rs).
    file_chrome: Node,
    written_state: String,
    picked_theme: Option<crate::layout::ThemeChoice>,
    /// Whether the shown layout's file read and parsed (or is simply
    /// missing): a broken one is never written over.
    layout_file_ok: bool,
    written_view: String,
    /// settings.json: zoom and sounds.
    person: settings::Person,
    /// What the last command made or why it didn't (for callers that wait for it).
    reply: Option<Value>,
    /// The `where` label last written per terminal address.
    written_where: HashMap<String, String>,
    /// `herdr.seen` turns written but not yet back from the core.
    written_seen: HashMap<String, u64>,
    /// Agents the person is looking at, and the turn they were shared as seen at.
    looked: HashMap<String, u64>,
    watched: Watched,
    /// Core's terminal records by address.
    live: HashMap<String, Value>,
    listed_once: bool,
    /// Live folders by address (from the shell process).
    cwds: HashMap<String, String>,
    /// When each terminal's repo and branch were last read, and what it was doing.
    folders_checked: HashMap<String, FolderCheck>,
    /// When Herdr started each terminal: a listing already in flight can't know them yet.
    born: RefCell<HashMap<String, Instant>>,
    /// Connection epoch: ignore replies from an old connection.
    core_generation: u64,
    checking_closed: bool,
    documents_watching: bool,
    draining: bool,
    changing_terminals: HashSet<String>,
    /// Closed terminals stay excluded until kill completes and a listing drops them.
    closing_terminals: HashMap<String, bool>,
    pending_terminals: std::collections::BTreeMap<String, PendingTerminalChange>,
    /// Each machine owns a document; the empty key is this core.
    documents: std::collections::BTreeMap<String, crate::mirror_sync::Slot>,
    initial_view: Option<crate::mirror::View>,
    pointer: Point<Pixels>,
    /// Where a press on a `window_drag` node started, until it moves the window.
    drag_from: Option<Point<Pixels>>,
    titlebar_applied: Option<(crate::layout::Titlebar, Option<[f32; 2]>)>,
    focused_pane: Option<String>,
    synced_model_pane: Option<String>,
    window_active: bool,
    /// The system's appearance; a layout with a light and a dark theme follows it.
    system_dark: bool,
    /// The OS tells us when files change (`files.rs`); else the disk is polled.
    file_events: Option<async_channel::Receiver<crate::files::Changed>>,
    polls: u64,
    /// The downloaded update the frame's pill offers, and the one closed for now.
    /// What's waiting, in words (`update_label`).
    update_ready: Option<String>,
    update_dismissed: Option<String>,
    update_success: bool,
    update_status: String,
    focus: FocusHandle,
    _tasks: Vec<Task<()>>,
    /// Gives the keyboard back when what had it leaves the screen (`restore_focus`).
    _focus_lost: Option<Subscription>,
    /// Remembers where the window is (`place.rs`); the write waiting to settle.
    _place: Option<Subscription>,
    place_write: Option<Task<()>>,
    /// The quick terminal's `auto_hide` (quick.rs).
    _quick: Option<Subscription>,
}

impl AppWindow {
    pub fn new(core: Arc<Core>, requests: async_channel::Receiver<Request>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let runtime = ShellRuntime::new(cx).map_err(|e| tracing::error!("script runtime failed, mods are off: {e:#}")).ok();
        mods::note_fonts(cx);
        let seen = std::fs::read_to_string(file("seen.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let mut w = AppWindow {
            core,
            runtime,
            chrome: default_layout("main"),
            layout_name: "main".into(),
            agent: None,
            agent_focused: None,
            swapped: HashMap::new(),
            float_drag: None,
            pane_drag: None,
            pane_grip_hover: None,
            pane_grip_rows: RefCell::default(),
            win_drag: None,
            float_anims: HashMap::new(),
            float_reveal: HashMap::new(),
            float_hover: HashSet::new(),
            float_leave: HashMap::new(),
            model: Model::default(),
            terms: HashMap::new(),
            copies: HashMap::new(),
            overlays: Vec::new(),
            folders: mods::scan().into_iter().map(|f| (f.id.clone(), f)).collect(),
            seen,
            bounds: Default::default(),
            drag: None,
            written_layout: String::new(),
            file_chrome: default_layout("main"),
            written_state: String::new(),
            picked_theme: None,
            layout_file_ok: true,
            written_view: String::new(),
            person: settings::Person::default(),
            reply: None,
            written_where: HashMap::new(),
            written_seen: HashMap::new(),
            looked: HashMap::new(),
            watched: Watched::default(),
            live: HashMap::new(),
            listed_once: false,
            cwds: HashMap::new(),
            folders_checked: HashMap::new(),
            born: RefCell::new(HashMap::new()),
            core_generation: 0,
            checking_closed: false,
            documents_watching: false,
            draining: false,
            changing_terminals: HashSet::new(),
            closing_terminals: HashMap::new(),
            pending_terminals: Default::default(),
            documents: {
                let mut local = crate::mirror_sync::Slot::default();
                local.connection(true, None, None);
                [(String::new(), local)].into()
            },
            initial_view: std::fs::read_to_string(file("view.json")).ok().and_then(|s| serde_json::from_str(&s).ok()),
            pointer: Point::default(),
            drag_from: None,
            titlebar_applied: None,
            focused_pane: None,
            synced_model_pane: None,
            window_active: true,
            system_dark: is_dark(window.appearance()),
            file_events: crate::files::watch(&mods::app_dir()),
            polls: 0,
            update_ready: None,
            update_dismissed: None,
            update_success: false,
            update_status: String::new(),
            focus: cx.focus_handle(),
            _tasks: Vec::new(),
            _focus_lost: None,
            _place: None,
            place_write: None,
            _quick: None,
        };
        w._focus_lost = Some(cx.on_focus_lost(window, |this, window, cx| this.restore_focus(window, cx)));
        w._place = Some(cx.observe_window_bounds(window, |this, window, cx| {
            if let Some(t) = crate::place::changed(window, cx) {
                this.place_write = Some(t);
            }
        }));
        if crate::quick::on() {
            crate::quick::install(window, cx);
            w._quick = Some(cx.observe_window_activation(window, |_, window, cx| crate::quick::focus_changed(window.is_window_active(), cx)));
        }
        let closing = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            // macOS: the red button hides the app instead, so alerts keep ringing;
            // the Dock icon or a new launch brings it back, Cmd+Q quits.
            if cfg!(target_os = "macos") {
                cx.hide();
                return false;
            }
            let _ = closing.update(cx, |this, cx| {
                this.drain_documents(window, cx);
                this.close_mods_now(std::time::Instant::now() + Duration::from_millis(250), window, cx);
            });
            crate::place::save(window, cx);
            true
        });
        settings::install_themes();
        crate::programs::install_icons();
        w.reload_config(window, cx);
        install_layouts();
        let name = mods::shared("layout").and_then(|v| v.as_str().map(str::to_owned)).filter(|n| layout_path(n).exists());
        // A layout whose mods are gone (Zen, T4, ADE after the update took
        // theirs) starts on Herdr's instead of a wall of "did not load".
        let gone = name.as_deref().map(missing_mods).unwrap_or_default();
        w.load_layout(if gone.is_empty() { name.as_deref().unwrap_or("main") } else { "main" });
        w.apply_look(window, cx);
        w.show_layout_notes(window, cx);
        if let (Some(n), false) = (&name, gone.is_empty()) {
            let note = json!({ "title": format!("{n} needs mods that aren't here"), "body": format!("Showing Herdr instead. Missing: {}. Your {n} layout file is still there.", gone.join(", ")), "level": "info", "timeout": 12000 });
            w.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": note }), window, cx);
        }

        w._tasks.push(cx.spawn_in(window, async move |this, cx| {
            loop {
                if this.update_in(cx, |this, window, cx| this.poll(window, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(400)).await;
            }
        }));
        if let Some(events) = w.file_events.clone() {
            w._tasks.push(cx.spawn_in(window, async move |this, cx| {
                while let Ok(mut changed) = events.recv().await {
                    // An editor's save is several events: take them together.
                    cx.background_executor().timer(Duration::from_millis(80)).await;
                    while let Ok(more) = events.try_recv() {
                        changed.extend(more);
                    }
                    if this.update_in(cx, |this, window, cx| this.files_changed(changed, window, cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        w._tasks.push(cx.spawn_in(window, async move |this, cx| {
            while let Ok(req) = requests.recv().await {
                if this.update_in(cx, |this, window, cx| this.handle(req, window, cx)).is_err() {
                    break;
                }
            }
        }));
        w._tasks.push(cx.spawn_in(window, async move |this, cx| {
            // The terminals come from the core's events (a snapshot, then each
            // change), not from listing them all every tick; machines, which
            // send no events, are asked now and then.
            let mut subscription = None;
            let mut subscribed_generation = u64::MAX;
            let mut event_lines = crate::hc::EventLines::default();
            let (mut tick, mut relist) = (0u64, false);
            loop {
                let Ok((core, generation)) = this.update(cx, |this, _| {
                    (this.core.clone(), this.core_generation)
                }) else { break };
                if generation != subscribed_generation {
                    drop(subscription.take());
                    event_lines = crate::hc::EventLines::default();
                    subscribed_generation = generation;
                    let local_core = core.clone();
                    let (sub, result) = cx.background_executor().spawn(async move {
                        let sub = local_core.subscribe().ok();
                        let result = crate::mirror_sync::round(&local_core, crate::mirror_sync::Work {
                            machine: String::new(), version: 0, write: None,
                        });
                        (sub, result)
                    }).await;
                    subscription = sub.map(|(channel, events, snapshot)| {
                        event_lines = crate::hc::EventLines::new(&snapshot);
                        (channel, events)
                    });
                    (tick, relist) = (0, true);
                    if this.update_in(cx, |this, window, cx| {
                        if generation != this.core_generation { return; }
                        this.documents.entry(String::new()).or_default().connection(true, None, None);
                        this.documents_updated(None, vec![result], window, cx);
                        this.sync(window, cx);
                    }).is_err() { break; }
                }
                let ask_machines = tick.is_multiple_of(5);
                tick += 1;
                let relisting = std::mem::take(&mut relist);
                let (sub, machines, refresh, listed, lines) = cx.background_executor().spawn(async move {
                    // The local stream forwards remote doc events too.
                    let mut sub = subscription;
                    let (mut refresh, mut terminals) = (false, relisting);
                    if sub.is_none() && let Ok((channel, events, snapshot)) = core.subscribe() {
                        event_lines = crate::hc::EventLines::new(&snapshot);
                        sub = Some((channel, events));
                        (refresh, terminals) = (true, true);
                    }
                    let mut closed = false;
                    if let Some((channel, events)) = &sub {
                        while let Ok(event) = events.try_recv() {
                            if let crate::hc::Stream::Data(bytes) = event {
                                let pushed = event_lines.push(&bytes);
                                refresh |= pushed.docs;
                                terminals |= pushed.terminals;
                                core.credit(*channel, bytes.len());
                            } else { (refresh, closed) = (true, true); }
                        }
                    }
                    // A stream that ended is subscribed to again, with a fresh snapshot.
                    if closed { sub = None; }
                    let machines = if ask_machines || terminals { core.call("machine.list", json!({})).ok() } else { None };
                    let listed = terminals.then(|| event_lines.live());
                    (sub, machines, refresh, listed, event_lines)
                }).await;
                let watching = sub.is_some();
                subscription = sub;
                event_lines = lines;
                let (seen, resynced) = event_lines.take_seen();
                if this.update_in(cx, |this, window, cx| {
                    if generation != this.core_generation { return; }
                    // Our seen writes the core has applied (or, after a fresh
                    // snapshot, can't tell): its labels decide again.
                    if resynced {
                        this.written_seen.clear();
                    } else {
                        this.written_seen.retain(|a, w| seen.get(a).is_none_or(|s| s < w));
                    }
                    this.documents_watching = watching;
                    if watching { this.documents.entry(String::new()).or_default().connection(true, None, None); }
                    let changed = this.documents_updated(machines, vec![], window, cx);
                    match listed {
                        Some(v) => this.terminals_listed(v, window, cx),
                        None => this.tend_terminals(false, window, cx),
                    }
                    if changed { this.sync(window, cx); }
                    this.flush_documents(refresh, window, cx);
                }).is_err() { break; }
                cx.background_executor().timer(Duration::from_millis(500)).await;
            }
        }));
        w._tasks.push(cx.spawn_in(window, async move |this, cx| {
            // Cores restart (upgrades, crashes) and their programs survive: keep
            // the panes, reconnect, reattach the same terminals.
            loop {
                let Ok(core) = this.update(cx, |this, _| this.core.clone()) else { break };
                core.closed().await;
                tracing::warn!("lost the core; reconnecting");
                crate::telemetry::count("core_lost");
                let fresh = loop {
                    cx.background_executor().timer(Duration::from_millis(1000)).await;
                    if let Ok(core) = cx.background_executor().spawn(async { Core::connect() }).await {
                        break core;
                    }
                };
                if this.update(cx, |this, cx| this.reconnected(fresh, cx)).is_err() {
                    break;
                }
            }
        }));
        Self::note_core(&w.core);
        w.sync(window, cx);
        // The first time the app opens: the frame's welcome (it sets
        // "frame.welcomed" when you start or skip).
        cx.defer_in(window, |this, window, cx| this.open_kept_windows(window, cx));
        // 0.1.0 kept space colors under its own word.
        if mods::shared("space.colors").is_none()
            && let Some(c) = mods::shared("project.colors")
        {
            mods::set_shared("space.colors".into(), c);
        }
        let first_install = mods::shared("frame.welcomed").is_none();
        crate::update_notes::started(first_install);
        if first_install {
            cx.defer_in(window, |this, window, cx| this.open_welcome(window, cx));
        } else if !crate::update_notes::spotlights().is_empty() {
            cx.defer_in(window, |this, window, cx| this.open_whats_new(false, window, cx));
        } else {
            w.update_success = crate::update_notes::changed();
        }
        crate::telemetry::window_open();
        w
    }

    /// What telemetry.rs counts: how much is open, never what's in it.
    fn observe(&self) {
        if !crate::telemetry::collecting() {
            return;
        }
        let m = &self.model;
        let agents: Vec<&model::Agent> = m.agents.iter().filter(|(a, _)| self.live.contains_key(*a) && m.status(a).is_some()).map(|(_, a)| a).collect();
        crate::telemetry::observe(crate::telemetry::Observation {
            foreground: self.window_active,
            agents_working: agents.iter().filter(|a| a.word.as_deref() == Some("working")).count(),
            agents: agents.iter().map(|a| a.app.clone()).collect(),
            spaces: m.spaces.len(),
            tabs: m.spaces.iter().map(|p| p.tabs.len()).sum(),
            terminals: m.spaces.iter().flat_map(|p| &p.tabs).map(|c| model::addresses(&c.root).len()).sum(),
            layout: self.layout_name.clone(),
            mods: self.visible().1.into_iter().map(|(_, i)| i.name).collect(),
            theme: settings::get().theme.clone(),
            welcome: mods::shared("frame.welcomed").and_then(|v| v.as_str().map(str::to_owned)),
        });
    }

    /// After an update: unseen spotlights. Asked for: the pending range, or this version's notes.
    fn open_whats_new(&mut self, asked: bool, window: &mut Window, cx: &mut Context<Self>) {
        let notes = if asked { crate::update_notes::notes(crate::update::ready().as_deref()) } else { crate::update_notes::spotlights() };
        if notes.is_empty() {
            self.run("app.settings", json!({ "section": "updates" }), window, cx);
            return;
        }
        let viewport = window.viewport_size();
        let width = 880.0_f32.min((f32::from(viewport.width) / settings::zoom() - 24.0).max(1.0));
        let height = 760.0_f32.min((f32::from(viewport.height) / settings::zoom() - 24.0).max(1.0));
        let spec = json!({ "mod": "frame-whatsnew", "kind": "window", "width": width, "height": height, "props": { "notes": notes } });
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    fn open_welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A window over the live app (nothing dims), centered and movable:
        // each step changes what's behind it, and a click beside it doesn't end it.
        let spec = json!({ "mod": "frame-welcome", "kind": "window", "width": 660, "height": 600 });
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    /// Tells the updater which core this is and who owns it (off the UI thread).
    fn note_core(core: &Arc<Core>) {
        let core = core.clone();
        std::thread::spawn(move || {
            if let Ok(s) = core.call("server.status", json!({})) {
                let text = |k: &str| s.get(k).and_then(Value::as_str).map(str::to_owned);
                crate::update::set_core(text("version").map(|v| v.trim_start_matches("hc ").to_owned()), text("installed_by"));
            }
        });
    }

    fn reconnected(&mut self, core: Arc<Core>, cx: &mut Context<Self>) {
        tracing::info!("reconnected to the core");
        self.core_generation += 1;
        self.documents_watching = false;
        for (machine, slot) in &mut self.documents {
            slot.available = false;
            slot.connected = machine.is_empty();
            slot.read_attempted = false;
            slot.pending = None;
            slot.blocked = false;
        }
        Self::note_core(&core);
        self.core = core.clone();
        self.publish(cx);
        for t in self.terms.values() {
            t.view.update(cx, |v, cx| v.reattach(core.clone(), cx));
        }
        cx.notify();
    }

    fn report(&self, what: &str, message: &str) {
        tracing::warn!("{what}.json not applied: {message}");
        let _ = std::fs::write(file(&format!("{what}.error")), format!("{what}.json was not applied: {message}\n"));
    }

    // ---- config files ------------------------------------------------------

    /// Changes settings.json from inside the app (the zoom keys). If the file
    /// is broken mid-edit, the change applies for now without touching it.
    fn write_person(&mut self, change: impl FnOnce(&mut settings::Person), window: &mut Window, cx: &mut Context<Self>) {
        let broken = settings::load_person().is_err();
        change(&mut self.person);
        if !broken {
            if let Err(e) = settings::write_person(&self.person) {
                tracing::warn!("cannot write settings.json: {e}");
            }
            self.watched.settings = mtime(&settings::path());
        }
        self.apply_look(window, cx);
    }

    /// Runs with the shown layout's look (its root node) at the person's zoom.
    fn apply_look(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A layout's fonts arrive with its mod: added since the start, they load now.
        if settings::load_home_fonts(cx) {
            mods::note_fonts(cx);
        }
        let s = settings::Settings::compose(&self.person, &self.chrome.look, self.system_dark);
        let mut problems = s.problems();
        if let Err(e) = crate::quick::set_key(s.quick_terminal.as_ref().and_then(|q| q.key.as_deref())) {
            problems.push(e);
        }
        if !problems.is_empty() {
            self.report_layout(&problems.join("; "));
        }
        if settings::set(s) {
            for term in self.terms.values() {
                term.view.read(cx).refresh_theme();
            }
            mods::apply_theme(cx);
            self.refresh_mods(cx);
            window.refresh();
        }
    }

    fn reload_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let s_time = mtime(&settings::path());
        if s_time.is_none() || s_time != self.watched.settings {
            let loaded = settings::load_person();
            crate::telemetry::set_setting(settings::telemetry_setting(&loaded));
            match loaded {
                Ok(p) => {
                    self.person = p;
                    let _ = std::fs::remove_file(file("settings.error"));
                }
                Err(e) => self.report("settings", &e),
            }
            self.watched.settings = mtime(&settings::path());
            self.apply_look(window, cx);
        }
        let k_time = mtime(&crate::keys::path());
        if k_time.is_none() || k_time != self.watched.keys {
            match crate::keys::load() {
                Ok(bindings) => {
                    crate::keys::install(bindings, cx);
                    let _ = std::fs::remove_file(file("keys.error"));
                }
                Err(e) => {
                    self.report("keys", &e);
                    if self.watched.keys.is_none() {
                        crate::keys::install(vec![], cx);
                    }
                }
            }
            self.watched.keys = mtime(&crate::keys::path());
        }
        let p_time = mtime(&crate::programs::path());
        if p_time != self.watched.programs {
            match crate::programs::load() {
                Ok(()) => {
                    let _ = std::fs::remove_file(file("programs.error"));
                }
                Err(e) => self.report("programs", &e),
            }
            self.watched.programs = p_time;
            self.publish(cx);
        }
    }

    /// Makes `name` the shown layout (its file, or its default if missing or
    /// broken), with the person's clicks on it.
    fn load_layout(&mut self, name: &str) {
        self.layout_name = name.to_owned();
        self.written_layout.clear();
        let mut from_file = false;
        // A file that's there but broken or unreadable is shown as shipped for
        // now and never written to (nor its clicks) until it's fixed.
        self.layout_file_ok = true;
        let mut chrome = match std::fs::read_to_string(layout_path(name)) {
            Ok(text) => match serde_json::from_str::<Node>(&text) {
                Ok(n) => {
                    self.written_layout = text;
                    from_file = true;
                    let _ = std::fs::remove_file(file("layout.error"));
                    n
                }
                Err(e) => {
                    self.report_layout(&e.to_string());
                    self.layout_file_ok = false;
                    default_layout(name)
                }
            },
            Err(e) => {
                self.layout_file_ok = e.kind() == std::io::ErrorKind::NotFound;
                default_layout(name)
            }
        };
        if from_file {
            self.keep_ids(&mut chrome);
        } else {
            fill_ids(&mut chrome, &mut self.model);
        }
        self.file_chrome = chrome.clone();
        let mut state = read_state(name);
        if self.layout_file_ok {
            // Clicks on fields the file changed since (an edit while the app
            // was closed or another layout showed, an update) give way.
            state.forget_since_saved(&chrome);
        }
        self.picked_theme = state.theme.clone();
        state.apply(&mut chrome);
        self.chrome = chrome;
        self.written_state.clear();
        if self.layout_file_ok {
            self.write_state(&state);
        }
        mods::set_shared("layout".into(), json!(name));
    }

    fn report_layout(&self, message: &str) {
        let name = &self.layout_name;
        tracing::warn!("layouts/{name}.json not applied: {message}");
        let _ = std::fs::write(file("layout.error"), format!("layouts/{name}.json was not applied: {message}\n"));
    }

    /// Shows the layout `name`, or the next one after the current.
    fn switch_layout(&mut self, name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let names = layout_names();
        let next = match name {
            Some(n) if mods::valid_name(n) && layout_path(n).exists() => n.to_owned(),
            Some(n) => {
                tracing::warn!("no layout {n:?} in layouts/");
                return;
            }
            None => {
                let i = names.iter().position(|n| *n == self.layout_name).map_or(0, |i| i + 1);
                match names.get(i % names.len().max(1)) {
                    Some(n) => n.clone(),
                    None => return,
                }
            }
        };
        self.save();
        self.float_drag = None;
        self.pane_drag = None;
        self.float_anims.clear();
        self.float_reveal.clear();
        self.float_hover.clear();
        let other = next != self.layout_name;
        if other {
            self.close_windows(window, cx);
            self.drop_layout_copies(window, cx);
        }
        self.load_layout(&next);
        self.apply_look(window, cx);
        // Whatever the new layout shows takes the keyboard.
        self.focused_pane = None;
        self.agent_focused = None;
        self.sync(window, cx);
        if other {
            self.open_kept_windows(window, cx);
        }
    }

    fn check_layout_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(text) = std::fs::read_to_string(layout_path(&self.layout_name)) else { return };
        if text == self.written_layout {
            return;
        }
        match serde_json::from_str::<Node>(&text) {
            Ok(mut n) => {
                self.written_layout = text;
                let was_broken = !self.layout_file_ok;
                self.layout_file_ok = true;
                self.keep_ids(&mut n);
                // What the file now sets on purpose wins over a click on it.
                // Coming back from a broken file, the clicks are the saved ones
                // (what showed meanwhile was only the shipped stand-in).
                let state = if was_broken {
                    let mut s = read_state(&self.layout_name);
                    s.forget_since_saved(&n);
                    s
                } else {
                    let mut s = layout_sync::State::between(&self.file_chrome, &self.chrome);
                    s.theme = self.picked_theme.clone();
                    s.forget_edited(&self.file_chrome, &n);
                    s
                };
                self.file_chrome = n.clone();
                self.chrome = n;
                self.picked_theme = state.theme.clone();
                state.apply(&mut self.chrome);
                self.write_state(&state);
                let _ = std::fs::remove_file(file("layout.error"));
                self.apply_look(window, cx);
                self.sync(window, cx);
            }
            Err(e) => {
                self.written_layout = text;
                self.layout_file_ok = false;
                self.report_layout(&e.to_string());
            }
        }
    }

    /// Live folders used for unnamed spaces; not restart state.
    fn note_pane_folders(&mut self) {
        let panes: Vec<(String, String)> =
            self.model.tabs().flat_map(|c| c.root.terminals()).filter_map(|n| Some((n.id.clone(), n.terminal()?.to_owned()))).collect();
        self.model.pane_folders.retain(|p, _| panes.iter().any(|(id, _)| id == p));
        for (pane, addr) in panes {
            if let Some(cwd) = self.cwds.get(&addr) {
                self.model.pane_folders.insert(pane, cwd.clone());
            }
        }
    }

    /// Writes the clicks to the layout's state file and, only when something
    /// else changed (a mod docked, `layout.set`), the layout file.
    fn save(&mut self) {
        // Quitting (also the OS ending the session): programs may end in any
        // order now. Their panes aren't closing; the next start brings them back.
        if QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        self.note_pane_folders();
        let mut state = layout_sync::State::between(&self.file_chrome, &self.chrome);
        state.theme = self.picked_theme.clone();
        if self.layout_file_ok {
            self.write_state(&state);
        }
        let edited = layout_sync::without_clicks(&self.file_chrome, &self.chrome);
        if edited != self.file_chrome && self.layout_file_ok && !shipped_layouts().contains(&self.layout_name.as_str()) {
            let path = layout_path(&self.layout_name);
            // Someone (an agent) changed the file since we read it: theirs
            // wins; the watcher loads it next.
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            if !self.written_layout.is_empty() && on_disk != self.written_layout {
                tracing::info!("layouts/{}.json changed meanwhile: not overwritten", self.layout_name);
            } else {
                let layout = layout_sync::text_of(&edited);
                if write_atomic(&path, &layout) {
                    self.written_layout = layout;
                    self.file_chrome = edited;
                }
            }
        }
        self.save_view();
    }

    fn save_view(&mut self) {
        let mut view = crate::mirror::View::capture(&self.model);
        if let Some(saved) = &self.initial_view {
            view.preserve_unloaded(saved, |machine| self.documents.get(machine.unwrap_or("")).is_some_and(|s| s.document.is_some()));
        }
        let view = serde_json::to_string_pretty(&view).unwrap_or_default() + "\n";
        if self.documents.values().any(|s| s.document.is_some()) && view != self.written_view && write_atomic(&file("view.json"), &view) {
            self.written_view = view;
        }
    }

    /// What installing layouts kept of the person's own, as toasts.
    fn show_layout_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for clone in mods::take_sync_notes() {
            let n = json!({ "title": format!("Your edits are in {clone}"), "body": "Herdr's own layouts and mods can't be changed: clone one to make it yours.", "level": "info", "timeout": 12000 });
            self.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": n }), window, cx);
        }
    }

    /// Writes the clicks, with the file they were made on (so a later change
    /// to the file can win over them).
    fn write_state(&mut self, state: &layout_sync::State) {
        let mut state = state.clone();
        if !state.is_empty() {
            state.file = serde_json::to_value(&self.file_chrome).ok();
        }
        let text = state_text(&state);
        if text == self.written_state {
            return;
        }
        let path = state_path(&self.layout_name);
        let ok = if state.is_empty() {
            std::fs::remove_file(&path).is_ok() || !path.exists()
        } else {
            let _ = std::fs::create_dir_all(file("layouts/.state"));
            write_atomic(&path, &text)
        };
        if ok {
            self.written_state = text;
        }
    }

    /// Gives nodes without an id one and writes it into the file, so clicks
    /// on them (keyed by id) still find them next time.
    fn keep_ids(&mut self, n: &mut Node) {
        let before = n.clone();
        // Never one the tree (or yesterday's generated ids in it) already has.
        let taken: std::collections::HashSet<String> = n.walk().iter().map(|x| x.id.clone()).collect();
        let mut i = 0;
        n.fill_ids(&mut || loop {
            i += 1;
            let id = format!("node-{i}");
            if !taken.contains(&id) {
                break id;
            }
        });
        if *n != before {
            let text = layout_sync::text_of(n);
            if write_atomic(&layout_path(&self.layout_name), &text) {
                self.written_layout = text;
            }
        }
    }

    // ---- making the screen match the model ---------------------------------

    /// Visible slots: terminal blocks (id → address) and mod slots (key → instance).
    fn visible(&self) -> (Vec<(String, String)>, Vec<(String, Instance)>) {
        let mut terms = Vec::new();
        let mut mods = Vec::new();
        let visit = |n: &Node, terms: &mut Vec<(String, String)>, mods: &mut Vec<(String, Instance)>| match n.content() {
            Some(Content::Terminal { terminal }) => terms.push((n.id.clone(), terminal.clone())),
            Some(Content::Mod { name, props }) => mods.push((n.id.clone(), Instance { name: name.clone(), props: props.clone() })),
            _ => {}
        };
        let stacks = |root: &Node, mods: &mut Vec<(String, Instance)>| {
            for n in visible_nodes(root) {
                if let Kind::Stack { header: Some(h), .. } = &n.kind
                    && let HeaderContent::Mod { name, props } = &h.content
                {
                    mods.push((format!("{}#header", n.id), Instance { name: name.clone(), props: props.clone() }));
                }
            }
        };
        for n in self.chrome.visible_blocks() {
            visit(n, &mut terms, &mut mods);
            // An agent block with an agent outside every tab gets its own view.
            if matches!(n.content(), Some(Content::Agent { .. }))
                && let Some(addr) = self.shown_agent()
                && self.model.pane_of(&addr).is_none()
            {
                terms.push((n.id.clone(), addr));
            }
        }
        stacks(&self.chrome, &mut mods);
        for n in visible_nodes(&self.chrome) {
            for f in n.floating.iter().filter(|f| !f.hidden) {
                match &f.content {
                    Content::Mod { name, props } => mods.push(float_mod(&f.id, name, props)),
                    Content::Terminal { terminal } => terms.push((floats::slot(&f.id), terminal.clone())),
                    _ => {}
                }
            }
        }
        if self.shows_tab()
            && let Some(c) = self.model.active_tab()
        {
            let header = self.pane_frame().and_then(|f| f.header.as_ref()).and_then(|h| match &h.content {
                HeaderContent::Mod { name, props } => Some((name.clone(), props.clone())),
                HeaderContent::Builtin(_) => None,
            });
            for n in c.shown().visible_blocks() {
                if let Some((name, props)) = &header
                    && n.is_pane()
                {
                    mods.push(pane_header_mod(n, name, props));
                }
                visit(n, &mut terms, &mut mods);
                if let Some(a) = self.swapped.get(&n.id) {
                    terms.push((format!("{}{SWAP}", n.id), a.clone()));
                }
            }
        }
        for o in &self.overlays {
            if let Some(c) = self.copies.get(&o.slot) {
                mods.push((o.slot.clone(), c.inst.clone()));
            }
            if let Some(l) = &o.layout {
                for n in l.visible_blocks() {
                    visit(n, &mut terms, &mut mods);
                }
            }
        }
        (terms, mods)
    }

    /// Mod slots whose copy may stay alive: every one in the layout (hidden
    /// nodes, inactive stack children, hidden floats), every pane mod of every
    /// tab (zoomed or not shown), and the overlays.
    fn owned_mods(&self) -> HashMap<String, Instance> {
        let mut out = HashMap::new();
        let inst = |name: &String, props: &Value| Instance { name: name.clone(), props: props.clone() };
        for n in self.chrome.walk() {
            if let Some(Content::Mod { name, props }) = n.content() {
                out.insert(n.id.clone(), inst(name, props));
            }
            if let Kind::Stack { header: Some(h), .. } = &n.kind
                && let HeaderContent::Mod { name, props } = &h.content
            {
                out.insert(format!("{}#header", n.id), inst(name, props));
            }
            for f in &n.floating {
                if let Content::Mod { name, props } = &f.content {
                    let (slot, i) = float_mod(&f.id, name, props);
                    out.insert(slot, i);
                }
            }
        }
        // The layout's pane header, shown or not (`pane_frame` says what's drawn now).
        let header = self
            .chrome
            .walk()
            .into_iter()
            .find_map(|n| match n.content() {
                Some(Content::Context { pane: Some(f), .. }) => f.header.as_ref(),
                _ => None,
            })
            .and_then(|h| match &h.content {
                HeaderContent::Mod { name, props } => Some((name.clone(), props.clone())),
                HeaderContent::Builtin(_) => None,
            });
        for c in self.model.tabs() {
            for n in c.root.walk() {
                if let Some(Content::Mod { name, props }) = n.content() {
                    out.insert(n.id.clone(), inst(name, props));
                }
                if let Some((name, props)) = &header
                    && n.is_pane()
                {
                    let (slot, i) = pane_header_mod(n, name, props);
                    out.insert(slot, i);
                }
            }
        }
        for o in &self.overlays {
            if let Some(c) = self.copies.get(&o.slot) {
                out.insert(o.slot.clone(), c.inst.clone());
            }
            if let Some(l) = &o.layout {
                for n in l.visible_blocks() {
                    if let Some(Content::Mod { name, props }) = n.content() {
                        out.insert(n.id.clone(), inst(name, props));
                    }
                }
            }
        }
        out
    }

    /// Another layout: its mods start fresh. Pane mods (they belong to their
    /// tab) and overlays stay.
    fn drop_layout_copies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let gone: Vec<_> = self.copies.keys().filter(|s| {
            self.model.tab_of(s).is_none() && !self.overlays.iter().any(|o| o.slot == **s || o.layout.as_ref().is_some_and(|l| l.find(s).is_some()))
        }).cloned().collect();
        for slot in gone {
            self.remove_copy(&slot, window, cx);
        }
    }

    fn remove_copy(&mut self, slot: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(copy) = self.copies.remove(slot)
            && let (Some(runtime), Some(root)) = (&self.runtime, copy.root)
        {
            runtime.close(&root, std::time::Instant::now() + Duration::from_millis(250), window, cx).detach();
        }
    }

    pub(crate) fn close_mods_now(&self, deadline: std::time::Instant, window: &mut Window, cx: &mut Context<Self>) {
        let Some(runtime) = &self.runtime else { return };
        mods::final_cleanup(self.core.clone(), deadline, || {
            for root in self.copies.values().filter_map(|copy| copy.root.as_ref()) {
                runtime.close_now(root, deadline, window, cx);
            }
        });
    }

    fn shows_tab(&self) -> bool {
        self.chrome.visible_blocks().iter().any(|n| matches!(n.content(), Some(Content::Context { .. })))
    }

    fn agent_block(&self) -> Option<String> {
        self.chrome.visible_blocks().iter().find(|n| matches!(n.content(), Some(Content::Agent { .. }))).map(|n| n.id.clone())
    }

    fn shows_agent(&self) -> bool {
        self.agent_block().is_some()
    }

    /// The mod listing agents chooses navigation order; model order is the fallback.
    fn agent_order(&self) -> Vec<String> {
        let m = &self.model;
        mods::agent_order(m.all_addresses().into_iter().filter(|a| m.status(a).is_some()).collect())
    }

    /// The agent an agent block shows: the chosen one while it lives, else the
    /// focused pane if it's an agent, else the first agent.
    fn shown_agent(&self) -> Option<String> {
        let focused = self.model.focused_pane().and_then(|p| self.model.address_of(&p));
        self.agent
            .clone()
            .filter(|a| self.live.contains_key(a))
            .or_else(|| focused.filter(|a| self.model.status(a).is_some()))
            .or_else(|| self.agent_order().into_iter().next())
    }

    /// The terminal view showing `addr` in an agent block: its pane's, or the block's own.
    fn agent_view(&self, block: &str, addr: &str) -> Option<&Term> {
        self.model.pane_of(addr).and_then(|p| self.terms.get(&p)).or_else(|| self.terms.get(block).filter(|t| t.address == addr))
    }

    /// What's on screen now, for done-vs-seen.
    fn visible_addresses(&self) -> Vec<String> {
        let (terms, _) = self.visible();
        // A swapped-in terminal is drawn instead of its pane's own.
        let covered: HashSet<&str> = terms.iter().filter_map(|(slot, _)| slot.strip_suffix(SWAP)).collect();
        let mut out: Vec<String> = terms.iter().filter(|(slot, _)| !covered.contains(slot.as_str())).map(|(_, a)| a.clone()).collect();
        if self.shows_agent() {
            out.extend(self.shown_agent());
        }
        out
    }

    /// Makes `addr` the shown agent; a pane showing it also becomes the focused pane.
    fn focus_agent(&mut self, addr: &str) {
        if let Some(p) = self.model.pane_of(addr) {
            self.model.focus_pane(&p);
        }
        self.focused_pane = None;
        self.agent = Some(addr.to_owned());
    }

    /// The parent chain's top: the agent a sub-agent belongs to in the agent order.
    fn root_agent(&self, addr: &str) -> String {
        let mut cur = addr.to_owned();
        for _ in 0..32 {
            let parent = self.live.get(&cur).and_then(|r| r.get("parent")).and_then(Value::as_str);
            match parent {
                Some(p) if self.live.contains_key(p) => cur = p.to_owned(),
                _ => break,
            }
        }
        cur
    }

    /// Creates what's visible, drops what isn't, publishes state, saves, focuses.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.order_spaces();
        let (model, live) = (&self.model, &self.live);
        self.swapped.retain(|p, a| model.address_of(p).is_some() && live.contains_key(a));
        let (mut terms, mods_visible) = self.visible();
        // Every tab's terminals stay attached (only visible ones are drawn): tab
        // switches are instant and kitty images survive, which a repaint can't bring back.
        for c in self.model.tabs() {
            for n in c.root.terminals() {
                if let Some(a) = n.terminal()
                    && !terms.iter().any(|(id, _)| *id == n.id)
                {
                    terms.push((n.id.clone(), a.to_owned()));
                }
            }
        }
        let want_terms: HashSet<String> = terms.iter().map(|(id, _)| id.clone()).collect();
        for (id, address) in terms {
            if self.terms.get(&id).is_some_and(|t| t.address == address) {
                continue;
            }
            let rec = self.live.get(&address);
            let dim = |k: &str, d: u64| rec.and_then(|r| r.get(k)).and_then(Value::as_u64).unwrap_or(d) as u16;
            let size_hint = (dim("cols", 120), dim("rows", 40));
            let view = cx.new(|cx| TerminalView::attach(self.core.clone(), address.clone(), size_hint, cx));
            self.tell_term(&view, &address, cx);
            let (block, addr) = (id.clone(), address.clone());
            let sub = cx.subscribe_in(&view, window, move |this, _, ev: &TerminalEvent, window, cx| match ev {
                TerminalEvent::Peek(address) => this.peek_terminal(address, window, cx),
                TerminalEvent::TitleChanged => this.publish(cx),
                TerminalEvent::Ended => this.terminal_ended(&block, window, cx),
                TerminalEvent::ContextMenu(at) => {
                    this.open_menu("pane", json!({ "pane": block.trim_end_matches(SWAP), "terminal": addr }), Some(*at), window, cx)
                }
            });
            let focus = view.read(cx).focus_handle().clone();
            self.terms.insert(id, Term { view, address, focus, _sub: sub });
        }
        let gone: Vec<String> = self.terms.keys().filter(|id| !want_terms.contains(*id)).cloned().collect();
        for id in gone {
            if let Some(t) = self.terms.remove(&id) {
                t.view.read(cx).detach();
            }
        }

        // A mod copy lives while its slot is in the layout, hidden or not (an
        // inactive stack child, a hidden column, a pane in another tab), so it
        // keeps its place; only shown slots start a copy. Gone or changed: dropped.
        let owned = self.owned_mods();
        let gone: Vec<_> = self.copies.iter().filter(|(slot, copy)| owned.get(*slot).is_none_or(|inst| *inst != copy.inst)).map(|(slot, _)| slot.clone()).collect();
        for slot in gone {
            self.remove_copy(&slot, window, cx);
        }
        let want: HashMap<String, Instance> = mods_visible.into_iter().collect();
        for (slot, inst) in want {
            if !self.copies.contains_key(&slot) {
                let copy = self.load(inst, window, cx);
                if slot.starts_with("float:")
                    && let Some(root) = &copy.root
                {
                    root.update(cx, |r, cx| r.set_fit_content(true, cx));
                }
                self.copies.insert(slot, copy);
            }
        }
        self.publish_shown_copies();

        // Keyboard to the focused pane when it changes (never steal it from an open dialog).
        let model_pane = self.model.focused_pane();
        let block = terminal_block(&self.chrome, self.focused_pane.as_deref());
        let focused = if block.is_some() && model_pane == self.synced_model_pane {
            self.focused_pane.clone()
        } else {
            model_pane.clone()
        };
        self.synced_model_pane = model_pane;
        let pane_changed = focused != self.focused_pane;
        self.focused_pane = focused.clone();
        let no_dialog = !self.overlays.iter().any(|o| o.kind.modal_keys());
        if terminal_block(&self.chrome, focused.as_deref()).is_some() {
            if pane_changed && no_dialog && let Some(f) = focused.as_deref().and_then(|p| self.pane_focus(p)) {
                f.focus(window, cx);
            }
        } else if let Some(block) = self.agent_block() {
            let shown = self.shown_agent();
            if shown != self.agent_focused || pane_changed {
                self.agent_focused = shown.clone();
                if no_dialog && let Some(t) = shown.and_then(|a| self.agent_view(&block, &a)) {
                    t.focus.focus(window, cx);
                }
            }
        } else if pane_changed {
            // Outside an agent view, the agent shown later follows the focused pane.
            self.agent = None;
            // A mod pane takes the keyboard on a click; until then the window holds
            // it, so another tab's terminal can't keep it (and pull its tab back).
            if no_dialog {
                focused.and_then(|p| self.pane_focus(&p)).unwrap_or_else(|| self.focus.clone()).focus(window, cx);
            }
        }
        if self.window_active {
            let visible = self.visible_addresses();
            self.model.mark_seen(&visible);
        }
        self.share_seen(cx);
        self.publish(cx);
        self.save();
        cx.notify();
    }

    /// Shares what the person looks at as `herdr.seen` (docs/labels.md): an
    /// agent on screen in the active window, once per turn while it stays
    /// there, so a phone marking it unread isn't undone by a screen nobody
    /// watches. Only ever raises it. True when a status changed.
    fn share_seen(&mut self, cx: &mut Context<Self>) -> bool {
        let looking: HashSet<String> = if self.window_active { self.visible_addresses().into_iter().collect() } else { HashSet::new() };
        self.looked.retain(|a, _| looking.contains(a));
        let mut changed = false;
        for addr in looking {
            let Some(a) = self.model.agents.get(&addr) else { continue };
            let turn = a.turn;
            if a.word.is_none() || !self.live.contains_key(&addr) || self.looked.get(&addr) == Some(&turn) {
                continue;
            }
            self.looked.insert(addr.clone(), turn);
            if a.seen_turn.is_some_and(|s| s >= turn) {
                continue;
            }
            // Shown as seen now, not after the core's round trip.
            changed |= self.model.set_turn(&addr, turn, Some(turn));
            self.written_seen.insert(addr.clone(), turn);
            let core = self.core.clone();
            cx.spawn(async move |this, cx| {
                let params = json!({ "terminal": addr, "labels": { "herdr.seen": turn.to_string() } });
                let Err(e) = cx.background_executor().spawn(async move { core.call("terminal.label", params) }).await else { return };
                tracing::warn!("seen label for {addr}: {e:#}");
                let _ = this.update(cx, |this, _| {
                    if this.written_seen.get(&addr) == Some(&turn) {
                        this.written_seen.remove(&addr);
                        this.looked.remove(&addr);
                    }
                });
            })
            .detach();
        }
        changed
    }

    /// The keyboard never ends up nowhere. When what had it leaves the screen (a
    /// mod hidden but kept alive, a closed pane, a hidden mod focusing itself on a
    /// reload), it goes to the top dialog, else the shown agent or focused pane,
    /// else the window, where the app's keys work.
    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = self.overlays.iter().rev().find(|o| o.kind.modal_keys()).map(|o| o.focus.clone());
        let shown = if terminal_block(&self.chrome, self.focused_pane.as_deref()).is_some() {
            self.focused_pane.as_deref().and_then(|p| self.pane_focus(p))
        } else if let Some(block) = self.agent_block() {
            self.shown_agent().and_then(|a| self.agent_view(&block, &a)).map(|t| t.focus.clone())
        } else if self.shows_tab() {
            self.model.focused_pane().and_then(|p| self.pane_focus(&p))
        } else {
            None
        };
        dialog.or(shown).unwrap_or_else(|| self.focus.clone()).focus(window, cx);
    }

    /// The terminal view a pane shows: the swapped-in one, else its own.
    fn pane_term(&self, pane: &str) -> Option<&Term> {
        self.swapped.get(pane).and_then(|_| self.terms.get(&format!("{pane}{SWAP}"))).or_else(|| self.terms.get(pane))
    }

    /// Peeks, floats and swapped-in views own keyboard focus without changing the model pane.
    fn focused_terminal(&self, window: &Window) -> Option<&Term> {
        self.terms.values().find(|t| t.focus.is_focused(window))
            .or_else(|| self.focused_pane.as_deref().and_then(|p| self.pane_term(p)))
            .or_else(|| self.model.focused_pane().and_then(|p| self.pane_term(&p)))
    }

    fn pane_focus(&self, pane: &str) -> Option<FocusHandle> {
        self.pane_term(pane).map(|t| t.focus.clone())
    }

    fn load(&self, inst: Instance, window: &mut Window, cx: &mut Context<Self>) -> Copy {
        let mut copy = Copy { inst, id: 0, root: None, _watch: None, error: None };
        let (Some(runtime), Some(folder)) = (&self.runtime, self.folders.get(&copy.inst.name)) else {
            copy.error = Some(format!("no mod folder ~/.config/herdr-app/mods/{}", copy.inst.name));
            return copy;
        };
        copy.id = mods::prepare_load(&copy.inst);
        match runtime.try_load(&folder.dir, window, cx) {
            Ok(root) => {
                if self.file_events.is_none() {
                    copy._watch = runtime.watch(&root, window, cx).map_err(|e| tracing::warn!("{}: no hot reload: {e:#}", copy.inst.name)).ok();
                }
                copy.root = Some(root);
            }
            Err(e) => {
                tracing::warn!("mod {} failed to load: {e:#}", copy.inst.name);
                copy.error = Some(format!("{e:#}"));
            }
        }
        copy
    }

    fn refresh_mods(&self, cx: &mut Context<Self>) {
        if let Some(runtime) = &self.runtime {
            for c in self.copies.values() {
                if let Some(root) = &c.root {
                    let _ = runtime.refresh(root, cx);
                }
            }
        }
    }

    /// How a terminal shows (`program` for mods): its agent, else what it runs.
    fn program(&self, addr: &str) -> Value {
        let rec = self.live.get(addr).and_then(Value::as_object);
        program_of(rec.and_then(|r| r.get("report")).unwrap_or(&Value::Null), rec.and_then(running_argv))
    }

    /// Tells mods what changed (they re-render only if something did).
    fn publish(&mut self, cx: &mut Context<Self>) {
        let m = &self.model;
        let home = home();
        let spaces: Vec<Value> = m
            .spaces
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // The branch of the terminal the space shows first.
                let first = p.active_tab().and_then(|c| c.focused_pane()).and_then(|pane| m.address_of(&pane));
                let branch = first.and_then(|a| m.folder_info.get(&a)).and_then(|f| f.branch.clone());
                model::space_json(m, p, i, branch.as_deref(), &home)
            })
            .collect();
        let block = terminal_block(&self.chrome, self.focused_pane.as_deref());
        let focused_pane = if block.is_some() { None } else { m.focused_pane() };
        let mut agents = Vec::new();
        let mut panes = Vec::new();
        for pr in &m.spaces {
            for (ci, ctx) in pr.tabs.iter().enumerate() {
                for p in ctx.root.panes() {
                    let focused = m.active.as_deref() == Some(pr.id.as_str()) && focused_pane.as_deref() == Some(p.id.as_str());
                    let zoomed = ctx.zoomed.as_deref() == Some(p.id.as_str());
                    // A mod pane: what it shows, no terminal.
                    if let Some(Content::Mod { name, .. }) = p.content() {
                        panes.push(json!({ "pane": p.id, "terminal": null, "mod": name, "tab": ctx.id, "title": name, "focused": focused, "zoomed": zoomed, "movable": self.pane_movable(&p.id) }));
                        continue;
                    }
                    let Some(addr) = p.terminal() else { continue };
                    let a = m.agents.get(addr);
                    let title = self.terms.get(&p.id).map(|t| t.view.read(cx).title.clone()).or_else(|| {
                        self.live.get(addr).and_then(|r| r.get("title")).and_then(Value::as_str).map(str::to_owned)
                    });
                    panes.push(json!({
                        "pane": p.id, "terminal": addr, "tab": ctx.id, "title": title.clone().unwrap_or_default(),
                        "cwd": self.cwds.get(addr), "branch": m.folder_info.get(addr).and_then(|f| f.branch.clone()),
                        "status": m.status(addr), "app": a.and_then(|a| a.app.clone()), "program": self.program(addr),
                        "focused": focused, "zoomed": zoomed, "movable": self.pane_movable(&p.id),
                    }));
                    let Some(status) = m.status(addr) else { continue };
                    agents.push(json!({
                        "address": addr, "pane": p.id, "space": pr.id, "space_name": m.space_name(pr, &home),
                        "tab": ctx.id, "tab_index": ci + 1, "tab_name": Model::tab_name(pr, ctx),
                        "status": status, "app": a.and_then(|a| a.app.clone()), "program": self.program(addr), "title": title.unwrap_or_default(),
                        "seq": a.map(|a| a.seq).unwrap_or(0),
                        "pinned": self.live.get(addr).and_then(|r| r.pointer("/labels/herdr.pinned")).and_then(Value::as_str).is_some_and(|v| !v.is_empty()),
                        "focused": m.active.as_deref() == Some(pr.id.as_str()) && focused_pane.as_deref() == Some(p.id.as_str()),
                    }));
                }
            }
        }
        let focused = m.active_space().and_then(|p| {
            let ctx = p.active_tab()?;
            if let Some(block) = block {
                let address = block.terminal()?;
                return Some(json!({ "space": p.id, "tab": ctx.id, "pane": null, "terminal": address,
                    "shown": null, "agent": self.shown_agent(), "cwd": self.cwds.get(address) }));
            }
            let pane = ctx.focused_pane()?;
            let cwd = (if self.shows_agent() { self.shown_agent() } else { m.address_of(&pane) }).and_then(|a| self.cwds.get(&a).cloned());
            Some(json!({ "space": p.id, "tab": ctx.id, "terminal": m.address_of(&pane), "shown": self.swapped.get(&pane),
                "pane": pane, "agent": self.shown_agent(), "cwd": cwd }))
        });
        let machines = self.documents.iter().map(|(name, slot)| json!({
            "name": (!name.is_empty()).then_some(name), "layout": slot.layout(),
        })).collect();
        let mut changed = mods::publish_machines(Value::Array(machines));
        changed |= mods::publish_spaces(Value::Array(spaces));
        changed |= mods::publish_agents(Value::Array(agents));
        changed |= mods::publish_panes(Value::Array(panes));
        changed |= mods::publish_focused(focused.unwrap_or(Value::Null));
        changed |= mods::publish_floats(self.float_placements());
        changed |= mods::publish_layout(&self.chrome);
        if changed {
            self.refresh_mods(cx);
        }
        self.write_where(cx);
    }

    /// Tells the core where each terminal lives, in words: the `where` label,
    /// `<space> / <tab>`, which hc shows in its picker and `ls`.
    /// Sub-agents get their parent's. The ids (`herdr.space`,
    /// `herdr.tab`) are what the app groups by; only this follows renames
    /// and moves. Written only when it changes.
    fn write_where(&mut self, cx: &mut Context<Self>) {
        let m = &self.model;
        let home = home();
        let mut want: HashMap<String, String> = HashMap::new();
        for p in &m.spaces {
            let name = m.space_name(p, &home);
            for c in &p.tabs {
                for a in model::addresses(&c.root) {
                    want.insert(a, format!("{name} / {}", Model::tab_name(p, c)));
                }
            }
        }
        for addr in self.live.keys() {
            if want.contains_key(addr) {
                continue;
            }
            let mut cur = addr.clone();
            for _ in 0..32 {
                let Some(p) = self.live.get(&cur).and_then(|r| r.get("parent")).and_then(Value::as_str) else { break };
                if let Some(place) = want.get(p).cloned() {
                    want.insert(addr.clone(), place);
                    break;
                }
                cur = p.to_owned();
            }
        }
        self.written_where.retain(|a, _| self.live.contains_key(a));
        for (addr, place) in want {
            if self.written_where.get(&addr) == Some(&place) || !self.live.contains_key(&addr) {
                continue;
            }
            // Off the UI thread (a terminal on a slow machine answers late); a
            // failed write is forgotten so the next publish tries again.
            self.written_where.insert(addr.clone(), place.clone());
            let core = self.core.clone();
            cx.spawn(async move |this, cx| {
                let params = json!({ "terminal": addr, "labels": { "where": place } });
                let Err(e) = cx.background_executor().spawn(async move { core.call("terminal.label", params) }).await else { return };
                tracing::warn!("where label for {addr}: {e:#}");
                let _ = this.update(cx, |this, _| {
                    if this.written_where.get(&addr) == Some(&place) {
                        this.written_where.remove(&addr);
                    }
                });
            })
            .detach();
        }
    }

    fn flush_documents(&mut self, refresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.draining || !self.documents_watching { return; }
        let generation = self.core_generation;
        for (machine, slot) in &mut self.documents {
            slot.dirty |= refresh;
            if !slot.ready() { continue; }
            let doc = crate::mirror::Document::capture(&self.model, (!machine.is_empty()).then_some(machine.as_str()));
            let write = (self.drag.is_none() && slot.available && slot.document.as_ref() != Some(&doc)).then_some(doc);
            if write.is_none() && slot.available && !slot.dirty { continue; }
            let (tx, rx) = std::sync::mpsc::channel();
            slot.pending = Some(rx);
            slot.dirty = false;
            let work = crate::mirror_sync::Work { machine: machine.clone(), version: slot.version, write };
            let core = self.core.clone();
            // Quit can block the foreground before the continuation gets a turn.
            let job = cx.background_executor().spawn(async move {
                let (machine, outcome) = crate::mirror_sync::round(&core, work);
                let _ = tx.send(outcome);
                machine
            });
            cx.spawn_in(window, async move |this, cx| {
                let machine = job.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if generation != this.core_generation || this.draining { return; }
                    let result = this.documents.get_mut(&machine).and_then(|s| s.pending.take()).and_then(|rx| rx.try_recv().ok());
                    if let Some(result) = result
                        && this.documents_updated(None, vec![(machine, result)], window, cx) { this.sync(window, cx); }
                });
            }).detach();
        }
    }

    fn layout_notice(&mut self, title: &str, body: &str, window: &mut Window, cx: &mut Context<Self>) {
        let spec = json!({"kind":"toast", "notification":{"title":title,"body":body,"level":"info"}});
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    /// Finish the accepted edit and any newer projection before the window goes.
    pub fn drain_documents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.draining { return; }
        self.save();
        self.draining = true;
        QUITTING.store(true, std::sync::atomic::Ordering::SeqCst);
        while let Some(address) = self.pending_terminals.keys().next().cloned() {
            let pending = self.pending_terminals.remove(&address).unwrap();
            let timeout = Duration::from_secs(if address.contains('/') { 2 } else { 10 });
            let result = pending.reply.recv_timeout(timeout).unwrap_or_else(|e| Err(format!("terminal change did not finish before quit: {e}")));
            self.apply_terminal_change(address, pending.change, result, window, cx);
        }
        let machines = self.documents.keys().cloned().collect::<Vec<_>>();
        for machine in machines {
            let timeout = Duration::from_secs(if machine.is_empty() { 10 } else { 2 });
            let deadline = Instant::now() + timeout;
            let pending = self.documents.get_mut(&machine).and_then(|s| s.pending.take());
            if let Some(pending) = pending {
                match pending.recv_timeout(timeout) {
                    Ok(result) => { self.documents_updated(None, vec![(machine.clone(), result)], window, cx); }
                    Err(e) => {
                        tracing::error!("layout on {machine:?} could not finish saving before quit: {e}");
                        continue;
                    }
                }
            }
            let slot = &self.documents[&machine];
            let doc = crate::mirror::Document::capture(&self.model, (!machine.is_empty()).then_some(machine.as_str()));
            if slot.document.as_ref() == Some(&doc) { continue; }
            if !slot.available {
                tracing::error!("layout on {machine:?} could not save before quit: {}", slot.error.as_deref().unwrap_or("machine unavailable"));
                continue;
            }
            let work = crate::mirror_sync::Work { machine: machine.clone(), version: slot.version, write: Some(doc) };
            let result = crate::mirror_sync::round_timeout(&self.core, work, deadline.saturating_duration_since(Instant::now()));
            self.documents_updated(None, vec![result], window, cx);
        }
        self.save_view();
    }

    fn documents_updated(&mut self, machines: Option<Value>, results: Vec<(String, crate::mirror_sync::Outcome)>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        use crate::mirror_sync::Outcome;
        let mut changed = false;
        // The listing every 500 ms brings the machines alone: redraw only when
        // one's state moved, or the whole window draws twice a second for nothing.
        let shown = |d: &std::collections::BTreeMap<String, crate::mirror_sync::Slot>| {
            d.iter().map(|(m, s)| (m.clone(), s.connected, s.available, s.blocked, s.error.clone(), s.layout())).collect::<Vec<_>>()
        };
        let (listing, was) = (results.is_empty(), shown(&self.documents));
        for (machine, result) in results {
            let Some(slot) = self.documents.get_mut(&machine) else { continue };
            let first = slot.document.is_none();
            slot.read_attempted = true;
            let (document, version, replace, conflict) = match result {
                Outcome::Read(d, v) => {
                    let replace = !slot.available || slot.version != v || slot.document.is_none();
                    let projection = crate::mirror::Document::capture(&self.model, (!machine.is_empty()).then_some(machine.as_str()));
                    let unsent = slot.document.as_ref().is_some_and(|old| old != &projection);
                    let conflict = replace && unsent && projection != d;
                    (d, v, replace, conflict)
                }
                Outcome::Written(d, v) => (d, v, false, false),
                Outcome::Conflict(d, v) => (d, v, true, true),
                error @ (Outcome::Unavailable(_) | Outcome::Undecodable(_) | Outcome::Blocked(_)) => {
                    let blocked = !matches!(&error, Outcome::Unavailable(_));
                    let error = match error {
                        Outcome::Unavailable(e) | Outcome::Undecodable(e) | Outcome::Blocked(e) => e,
                        _ => unreachable!(),
                    };
                    let notify = slot.connected && blocked && slot.notified_error.as_ref() != Some(&error);
                    slot.available = false;
                    slot.blocked = blocked;
                    if slot.connected || slot.error.is_none() { slot.error = Some(error.clone()); }
                    if notify {
                        tracing::warn!("layout on {machine:?} unavailable: {error}");
                        slot.notified_error = Some(error.clone());
                        self.layout_notice("Layout unavailable", &format!("{}: {error}", if machine.is_empty() { "This machine" } else { &machine }), window, cx);
                    }
                    continue;
                }
            };
            slot.version = version;
            slot.available = slot.connected;
            slot.blocked = false;
            if slot.connected { slot.error = None; slot.notified_error = None; }
            slot.document = Some(document.clone());
            if replace {
                let on = (!machine.is_empty()).then_some(machine.as_str());
                document.replace(&mut self.model, on);
                if first && let Some(view) = &self.initial_view { view.apply_machine(&mut self.model, on); }
                changed = true;
            }
            if conflict {
                self.layout_notice("Layout changed elsewhere", "The other screen's layout is shown.", window, cx);
            }
        }
        if let Some(machines) = machines {
            // Temporary machines are agents' short-lived sandboxes: the app leaves them out.
            let kept = || machines["machines"].as_array().into_iter().flatten().filter(|m| m["temporary"] != true);
            let states = kept().filter_map(|m| Some((m["name"].as_str()?, m["state"].as_str()?))).collect::<HashMap<_, _>>();
            for machine in kept() {
                let Some(name) = machine["name"].as_str() else { continue };
                let state = machine["state"].as_str().unwrap_or("unavailable");
                let reason = (state != "connected").then(|| machine["why"].as_str().map(str::to_owned).unwrap_or_else(|| match state {
                    "needs_update" => format!("{name} needs a core update; run hc machine update {name}"),
                    "needs_login" => format!("{name} needs login; run hc machine login {name}"),
                    _ => format!("{name} is {state}"),
                }));
                self.documents.entry(name.to_owned()).or_default().connection(state == "connected", machine["since_ms"].as_u64(), reason);
            }
            if self.model.spaces.iter().any(|s| s.machine.as_deref().is_some_and(|m| !states.contains_key(m))) {
                let mut view = crate::mirror::View::capture(&self.model);
                if let Some(saved) = &self.initial_view {
                    view.preserve_unloaded(saved, |m| self.documents.get(m.unwrap_or("")).is_some_and(|s| s.document.is_some()));
                }
                self.initial_view = Some(view);
            }
            self.documents.retain(|m, _| m.is_empty() || states.contains_key(m.as_str()));
            let before = self.model.spaces.len();
            self.model.spaces.retain(|s| s.machine.as_deref().is_none_or(|m| states.contains_key(m)));
            changed |= before != self.model.spaces.len();
        }
        if !listing || changed || shown(&self.documents) != was {
            self.publish(cx);
            cx.notify();
        }
        changed
    }

    // ---- core facts --------------------------------------------------------

    /// What a terminal view takes from the core's facts: its folder, where right-clicks go.
    fn tell_term(&self, view: &Entity<TerminalView>, addr: &str, cx: &mut Context<Self>) {
        let cwd = self.cwds.get(addr).map(std::path::PathBuf::from);
        let to_program = self.live.get(addr).and_then(|r| r.pointer("/labels/herdr.right-click")).and_then(Value::as_str) == Some("program");
        view.update(cx, |v, _| {
            v.cwd = cwd;
            v.right_click_to_program = to_program;
        });
    }

    fn terminals_listed(&mut self, v: Value, window: &mut Window, cx: &mut Context<Self>) {
        let mut list = v.get("terminals").cloned().unwrap_or(Value::Array(vec![]));
        let mut live = HashMap::new();
        for t in list.as_array_mut().into_iter().flatten() {
            let Some(obj) = t.as_object_mut() else { continue };
            let report = obj.remove("status").unwrap_or(Value::Null);
            let word = report.get("state").cloned().unwrap_or_else(|| if report.is_string() { report.clone() } else { Value::Null });
            obj.insert("status".into(), word);
            obj.insert("program".into(), program_of(&report, running_argv(obj)));
            obj.insert("report".into(), report);
            // As the panes show it: a spinning title would change every listing.
            if let Some(title) = obj.get("title").and_then(Value::as_str).map(|t| crate::terminal::shown_title(t).to_owned()) {
                obj.insert("title".into(), title.into());
            }
            if let Some(a) = obj.get("address").and_then(Value::as_str) {
                live.insert(a.to_owned(), Value::Object(obj.clone()));
            }
        }
        self.live = live;
        self.closing_terminals.retain(|addr, completed| !*completed || self.live.contains_key(addr));
        // A new sub-agent changes only this list; mods that show teams need to redraw.
        let terminals_changed = mods::publish_terminals(list);
        self.tend_terminals(true, window, cx);
        if terminals_changed {
            self.refresh_mods(cx);
        }
    }

    /// Every tick, with or without news from the core: where the terminals go
    /// (a layout that arrived late can place one now), what they mean now (a
    /// finished agent you're looking at is no longer news), their folders, their
    /// streams. `listed`: the list itself changed.
    fn tend_terminals(&mut self, listed: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Place newly labeled terminals first: one that is blocked on its first
        // listing is ours, and must ring.
        let mut changed = self.reconcile(window, cx);
        let visible: HashSet<String> = self.visible_addresses().into_iter().collect();
        let mut alerts = Vec::new();
        for (addr, rec) in &self.live {
            let word = rec.get("status").and_then(Value::as_str).map(str::to_owned);
            let app = rec.get("report").and_then(|r| r.get("app")).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
            let looking = visible.contains(addr) && self.window_active;
            let prev = self.model.agents.get(addr).and_then(|a| a.word.clone());
            let child = rec.get("parent").and_then(Value::as_str).is_some_and(|p| !p.is_empty());
            if let Some(alert) = self.model.alert_for(addr, child, prev.as_deref(), word.as_deref(), looking) {
                alerts.push((addr.clone(), alert, looking));
            }
            changed |= self.model.set_word(addr, word, app, looking);
            let turn = rec.get("turn").and_then(Value::as_u64).unwrap_or(0);
            let label = rec.pointer("/labels/herdr.seen").and_then(Value::as_str).and_then(|s| s.parse::<u64>().ok());
            changed |= self.model.set_turn(addr, turn, label.max(self.written_seen.get(addr).copied()));
        }
        self.written_seen.retain(|a, _| self.live.contains_key(a));
        changed |= self.share_seen(cx);
        // Changes already made before Herdr started aren't news.
        if self.listed_once {
            for (addr, alert, looking) in alerts {
                self.alert(&addr, alert, looking, window, cx);
            }
        }
        self.track_folders(cx);
        for t in self.terms.values() {
            if self.live.get(&t.address).is_some_and(|r| r["pid"].as_u64().is_some()) {
                t.view.update(cx, |v, cx| v.reattach_if_needed(self.core.clone(), cx));
            }
            self.tell_term(&t.view, &t.address, cx);
        }
        self.listed_once = true;
        if changed {
            self.sync(window, cx);
        } else if listed {
            self.publish(cx);
        }
    }

    /// Tells the person an agent needs them or finished: Herdr's sounds, and a
    /// toast (drawn by the `toast` mod; click it to go there) unless they're
    /// looking at that agent.
    fn alert(&mut self, addr: &str, alert: model::Alert, looking: bool, window: &mut Window, cx: &mut Context<Self>) {
        if settings::get().sounds.enabled {
            crate::sound::play(if alert == model::Alert::Blocked { "blocked" } else { "done" });
        }
        // Needing the person brings a settled tab back.
        if let Some(ctx) = self.model.pane_of(&self.root_agent(addr)).and_then(|p| self.model.tab_of(&p)).map(|c| c.id.clone())
            && self.model.unsettle(&ctx)
        {
            self.save();
            self.publish(cx);
        }
        if looking {
            return;
        }
        let rec = self.live.get(addr);
        let app = self.model.agents.get(addr).and_then(|a| a.app.clone());
        let name = rec
            .and_then(|r| r.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned))
            .or_else(|| app.as_deref().map(agent_name))
            .unwrap_or_else(|| "An agent".into());
        // A sub-agent lives where its top agent does.
        let home = self.root_agent(addr);
        let place = self.model.pane_of(&home).and_then(|pane| {
            let (p, c) = (self.model.space_of(&pane)?, self.model.tab_of(&pane)?);
            Some(format!("{} / {}", self.model.space_name(p, &self::home()), Model::tab_name(p, c)))
        });
        let (title, level) = match alert {
            model::Alert::Blocked => (format!("{name} needs you"), "error"),
            model::Alert::Done => (format!("{name} finished"), "info"),
        };
        let notification = json!({
            "title": title, "body": place, "level": level,
            "action": { "name": "agent.focus", "args": { "terminal": addr } },
        });
        self.open_overlay(mods::new_overlay_id(), json!({ "kind": "toast", "notification": notification }), window, cx);
    }

    /// Keeps the model true to core, on every listing: drops panes whose terminal
    /// is gone (ended while hidden, or while the app was closed) and places
    /// terminals labeled `herdr.space` that aren't shown (started by an
    /// agent, a script, or before a restart). Returns whether anything changed.
    fn reconcile(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // No fresh shells while quitting: the next start brings them back.
        if QUITTING.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let missing = self.model.tabs().flat_map(|c| c.root.terminals())
            .filter_map(|n| n.terminal().filter(|a| !self.live.contains_key(*a) && !self.just_born(a)).map(|a| (n.id.clone(), a.to_owned())))
            .collect::<Vec<_>>();
        if !missing.is_empty() && !self.checking_closed {
            self.check_closed(missing, window, cx);
        }
        let mut orphans = self.live.iter()
            .filter(|(a, _)| self.model.pane_of(a).is_none() && !self.changing_terminals.contains(*a) && !self.closing_terminals.contains_key(*a))
            .filter_map(|(a, r)| {
                let machine = a.split_once('/').map(|(m, _)| m);
                if !self.documents.get(machine.unwrap_or("")).is_some_and(|s| s.available) { return None; }
                let labels = r.get("labels")?;
                let space = labels.get("herdr.space")?.as_str().filter(|s| !s.is_empty())?;
                let tab = labels.get("herdr.tab").and_then(Value::as_str).filter(|s| !s.is_empty());
                Some((a.clone(), crate::mirror::qualify(space, machine), tab.map(|t| crate::mirror::qualify(t, machine))))
            }).collect::<Vec<_>>();
        orphans.sort();
        for (addr, space, tab) in orphans {
            self.start_adoption(addr, space, tab, window, cx);
        }
        false
    }

    fn start_adoption(&mut self, addr: String, space: String, tab: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.changing_terminals.insert(addr.clone());
        let core = self.core.clone();
        let generation = self.core_generation;
        cx.spawn_in(window, async move |this, cx| {
            let (address, wanted_space, wanted_tab) = (addr.clone(), space.clone(), tab.clone());
            let ready = cx.background_executor().spawn(async move {
                let record = core.call("terminal.get", json!({"terminal":address}))?;
                let machine = address.split_once('/').map(|(m, _)| m);
                if !adoptable(&record, &wanted_space, wanted_tab.as_deref(), machine) { return Ok(false); }
                if record["keep"].as_bool() != Some(true) {
                    core.call("terminal.keep", json!({"terminal":address,"keep":true}))?;
                }
                anyhow::Ok(true)
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.changing_terminals.remove(&addr);
                if generation != this.core_generation || this.draining { return; }
                if matches!(ready, Ok(true)) && this.model.pane_of(&addr).is_none()
                    && this.require_document(addr.split_once('/').map(|(m, _)| m)).is_ok()
                {
                    this.adopt(&addr, &space, tab.as_deref());
                    this.sync(window, cx);
                    this.flush_documents(false, window, cx);
                }
            });
        }).detach();
    }

    /// Puts a live terminal where its labels say, without changing what's
    /// shown. A space or tab the app doesn't know is made under the label's
    /// id, unnamed (called Herdr's way).
    fn adopt(&mut self, addr: &str, space: &str, ctx: Option<&str>) {
        let machine = addr.split_once('/').map(|(m, _)| m.to_owned());
        let pane = self.model.fresh_id_on("p", machine.as_deref());
        let ctx_id = ctx.map(str::to_owned).unwrap_or_else(|| self.model.fresh_id_on("tb", machine.as_deref()));
        // Found by its labels: no names, so they're called Herdr's way.
        let new_ctx = Tab::new(&ctx_id, "", &pane, addr);
        if self.model.space(space).is_none() {
            // Its live folder may not be read yet: the core's record has it.
            let rec = self.live.get(addr);
            let folder = self
                .cwds
                .get(addr)
                .cloned()
                .or_else(|| rec.and_then(|r| r.get("foreground").and_then(|f| f.get("cwd")).or_else(|| r.get("cwd"))).and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_else(|| mods::home().to_string_lossy().into_owned());
            let active = self.model.active.clone();
            self.model.add_space(space, "", &folder, new_ctx);
            self.model.space_mut(space).unwrap().machine = machine;
            self.model.active = active.or(Some(space.to_owned()));
            return;
        }
        let target = self.model.tab(&ctx_id).and_then(|c| c.root.panes().last().map(|p| p.id.clone()));
        if let Some(target) = target {
            let tab = self.model.tab_of(&target).unwrap();
            let focused = tab.focused.clone();
            let before = tab.before.clone();
            let zoomed = tab.zoomed.clone();
            let id = tab.id.clone();
            self.model.split(&target, Axis::Row, &pane, addr);
            let tab = self.model.tab_mut(&id).unwrap();
            tab.focused = focused;
            tab.before = before;
            tab.zoomed = zoomed;
            return;
        }
        let keep = self.model.space(space).and_then(|p| p.active.clone());
        self.model.add_tab(space, new_ctx);
        if let Some(p) = self.model.space_mut(space) {
            p.active = keep;
        }
    }

    /// Reads each terminal's live folder, and its repo and branch for mods.
    fn track_folders(&mut self, cx: &mut Context<Self>) {
        let home = mods::home().to_string_lossy().into_owned();
        let mut todo: HashMap<(Option<String>, String), Vec<String>> = HashMap::new();
        let mut moved = false;
        for (addr, rec) in &self.live {
            // Core's live folder of what runs there (any machine), else the shell's (local).
            let cwd = rec
                .get("foreground")
                .and_then(|f| f.get("cwd"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                // (a pid on another machine means nothing here)
                .or_else(|| rec.get("pid").and_then(Value::as_u64).filter(|_| !addr.contains('/')).and_then(crate::process_cwd::read).map(|p| p.to_string_lossy().into_owned()))
                .or_else(|| rec.get("cwd").and_then(Value::as_str).map(str::to_owned));
            let Some(cwd) = cwd else { continue };
            let busy = busy(rec);
            let last = self.folders_checked.get_mut(addr);
            if recheck(last.as_deref(), &cwd, busy, self.window_active) {
                moved |= self.cwds.get(addr) != Some(&cwd);
                self.cwds.insert(addr.clone(), cwd.clone());
                let repo = last.as_ref().is_none_or(|l| l.cwd != cwd || l.repo); // until git says otherwise
                self.folders_checked.insert(addr.clone(), FolderCheck { at: Instant::now(), cwd: cwd.clone(), busy, repo });
                // Another machine's terminal (`vps/t7`): git runs there.
                todo.entry((addr.split_once('/').map(|(m, _)| m.to_owned()), cwd)).or_insert_with(Vec::new).push(addr.clone());
            } else if let Some(last) = last {
                last.busy = busy;
            }
        }
        self.folders_checked.retain(|a, _| self.live.contains_key(a));
        // Refresh unnamed spaces when a terminal moves.
        if moved {
            self.save();
        }
        // Once per machine and folder, however many terminals are there.
        for ((on, cwd), addrs) in todo {
            let home = home.clone();
            cx.spawn(async move |this, cx| {
                let asked = cwd.clone();
                let (info, repo) = cx.background_executor().spawn(async move { folder_info(&asked, &home, on.as_deref()) }).await;
                let _ = this.update(cx, |this, cx| {
                    let mut changed = false;
                    // Not one that moved on meanwhile: its own answer is coming.
                    for addr in addrs.into_iter().filter(|a| this.cwds.get(a) == Some(&cwd)) {
                        if let Some(check) = this.folders_checked.get_mut(&addr) {
                            check.repo = repo;
                        }
                        if this.model.folder_info.get(&addr) != Some(&info) {
                            this.model.folder_info.insert(addr, info.clone());
                            changed = true;
                        }
                    }
                    if changed {
                        this.publish(cx);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn terminal_ended(&mut self, block: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = block.strip_suffix(SWAP) {
            self.swapped.remove(pane);
            self.focused_pane = None;
            return self.sync(window, cx);
        }
        if let Some(address) = self.model.address_of(block) {
            self.notify_launch_failure(&address, window, cx);
        }
        if let Some(o) = self.overlays.iter().find(|o| o.layout.as_ref().is_some_and(|l| l.find(block).is_some())) {
            let id = o.id.clone();
            return self.close_overlay(&id, window, cx);
        }
        if self.chrome.find(block).is_some_and(|n| n.terminal().is_some()) {
            self.chrome.remove(block);
            self.sync(window, cx);
        }
        // How its stream ended, for `reconcile` (`hc run --rm` leaves no record).
        if let Some(t) = self.terms.get(block)
            && let Some(why) = t.view.read(cx).ended.clone()
        {
            // The program ended on its own (or was killed): its pane goes now. A
            // stream that just broke (the core died) says neither.
            let closed = matches!(why.as_str(), "exited" | "killed" | "finished");
            if closed && self.model.address_of(block).as_deref() == Some(t.address.as_str()) {
                self.model.remove_pane(block);
                return self.sync(window, cx);
            }
        }
        // Otherwise the next listing checks closure; core owns lost terminals.
    }

    fn notify_launch_failure(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.born.borrow_mut().remove(address).is_none() {
            return;
        }
        let core = self.core.clone();
        let address = address.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let failure = cx.background_executor().spawn(async move {
                let record = core.call("terminal.get", json!({"terminal": address})).ok()?;
                launch_failure(&record, "")?;
                let output = core.call("terminal.read", json!({"terminal": address, "lines": 5})).unwrap_or(Value::Null);
                launch_failure(&record, output["text"].as_str().unwrap_or_default())
            }).await;
            if let Some(body) = failure {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_overlay(mods::new_overlay_id(), json!({"kind":"toast", "notification":{"title":"Terminal failed to start", "body":body, "level":"error"}}), window, cx);
                });
            }
        }).detach();
    }

    // ---- creating terminals ------------------------------------------------

    /// The folder new terminals in `space` start in: where its shown tab's focused
    /// pane is now (Herdr's "follow"), else the folder the space was made in.
    fn folder_for(&self, space: &str) -> String {
        let p = self.model.space(space);
        let here = p.and_then(|p| p.active_tab()).and_then(|c| c.focused_pane()).and_then(|f| self.model.address_of(&f)).and_then(|a| self.cwds.get(&a).cloned());
        here.or_else(|| p.map(|p| p.folder.clone())).unwrap_or_else(home)
    }

    /// The machine a space's terminals run on, None for this one.
    fn require_document(&self, machine: Option<&str>) -> Result<(), String> {
        match self.documents.get(machine.unwrap_or("")) {
            Some(slot) if slot.available => Ok(()),
            Some(slot) => Err(slot.error.clone().unwrap_or_else(|| "This machine's layout is unavailable. Try again when it reconnects.".into())),
            None => Err(match machine {
                Some(m) => format!("Herdr has no machine called {m}. See hc machine ls; machines added with --temp stay out of Herdr."),
                None => "This machine's layout is unavailable. Try again when it reconnects.".into(),
            }),
        }
    }

    fn machine_of(&self, space: &str) -> Option<String> {
        self.model.space(space).and_then(|p| p.machine.clone())
    }

    /// Starts a terminal for `space`, on `on` (another machine) or here. An
    /// empty `cwd`: the machine's home. With no argv, that machine's core
    /// picks its own shell.
    fn spawn(&self, argv: Option<Vec<String>>, cwd: String, space: &str, ctx: &str, on: Option<&str>) -> Result<String, String> {
        let argv = argv.unwrap_or_default();
        let cwd = Some(cwd).filter(|c| !c.is_empty());
        self.require_document(on)?;
        let labels = json!({ "herdr.space": crate::mirror::local(space, on), "herdr.tab": crate::mirror::local(ctx, on), "herdr.seen": "0" });
        let addr = match self.core.run(argv.clone(), cwd.clone(), labels.clone(), on) {
            // An agent on a machine without `sh` (Windows): in its PowerShell instead.
            Err(e) if on.is_some() && argv.first().map(String::as_str) == Some("sh") && format!("{e:#}").contains("not found") => {
                self.core.run(Self::powershell_argv(&argv), cwd, labels, on)
            }
            r => r,
        }
        .map_err(|e| format!("cannot start a terminal: {e:#}"))?;
        self.born.borrow_mut().insert(addr.clone(), Instant::now());
        Ok(addr)
    }

    /// The PowerShell form of a remote agent start (`agent_argv`): the program,
    /// then a shell that stays.
    fn powershell_argv(sh: &[String]) -> Vec<String> {
        let cmd = sh.get(4..).unwrap_or_default();
        vec!["powershell.exe".into(), "-NoLogo".into(), "-NoExit".into(), "-Command".into(), crate::programs::powershell_line(cmd)]
    }

    /// An agent with its first message, in a shell that stays when the agent exits
    /// (on another machine: that machine's shell).
    fn agent_argv(argv: Option<Vec<String>>, task: Option<&str>, on: Option<&str>) -> Option<Vec<String>> {
        let task = task.map(str::trim).filter(|t| !t.is_empty());
        argv.filter(|a| !a.is_empty()).map(|mut cmd| {
            cmd.extend(task.map(str::to_owned));
            if on.is_some() {
                return crate::remote_shell::launch(cmd);
            }
            let sh = shell();
            if cfg!(windows) {
                // PowerShell has no `-ic`: run it, then stay. `&` finds npm's `.cmd` shims too.
                return vec![sh, "-NoExit".into(), "-Command".into(), crate::programs::powershell_line(&cmd)];
            }
            // macOS terminal sessions load login profiles (including Homebrew's PATH).
            unix_agent_argv(sh, cmd, cfg!(target_os = "macos"))
        })
    }

    /// A space with its first tab and terminal, shown at once.
    /// `machine`: another machine (`hc machine ls`); its space starts in that machine's home.
    fn new_space(&mut self, name: &str, folder: Option<String>, ctx_name: Option<&str>, argv: Option<Vec<String>>, task: Option<&str>, machine: Option<String>) -> Result<Value, String> {
        let on = machine.as_deref();
        // The focused pane's folder only means something on its own machine.
        let here = || self.model.focused_pane().and_then(|p| self.model.address_of(&p))
            .filter(|a| a.split_once('/').map(|(m, _)| m) == on)
            .and_then(|a| self.cwds.get(&a).cloned());
        let folder = folder.or_else(here).unwrap_or_else(|| if on.is_some() { String::new() } else { home() });
        let (space, ctx, pane) = (self.model.fresh_id_on("sp", on), self.model.fresh_id_on("tb", on), self.model.fresh_id_on("p", on));
        let addr = self.spawn(Self::agent_argv(argv.clone(), task, on).or(argv), folder.clone(), &space, &ctx, on)?;
        let ctx_name = ctx_name.map(str::trim).unwrap_or_default();
        self.model.add_space(&space, name.trim(), &folder, Tab::new(&ctx, ctx_name, &pane, &addr));
        if let Some(p) = self.model.space_mut(&space) {
            p.machine = machine;
        }
        Ok(json!({ "space": space, "tab": ctx, "terminal": addr }))
    }

    /// A tab in a space (default: the shown one), shown at once.
    fn new_tab(&mut self, space: Option<String>, name: &str, folder: Option<String>, argv: Option<Vec<String>>, task: Option<&str>) -> Result<Value, String> {
        let space = space.or_else(|| self.model.active_space().map(|p| p.id.clone())).ok_or("no space yet: make one first (space.new)")?;
        if self.model.space(&space).is_none() {
            return Err(format!("no space {space:?}"));
        }
        let folder = folder.unwrap_or_else(|| self.folder_for(&space));
        let on = self.machine_of(&space);
        let (ctx, pane) = (self.model.fresh_id_on("tb", on.as_deref()), self.model.fresh_id_on("p", on.as_deref()));
        let addr = self.spawn(Self::agent_argv(argv.clone(), task, on.as_deref()).or(argv), folder, &space, &ctx, on.as_deref())?;
        self.model.add_tab(&space, Tab::new(&ctx, name.trim(), &pane, &addr));
        self.model.active = Some(space.clone());
        Ok(json!({ "space": space, "tab": ctx, "terminal": addr }))
    }

    /// A tab whose only pane is a mod (a file, a board), shown at once.
    fn new_mod_tab(&mut self, space: Option<String>, name: &str, m: String, props: Value) -> Result<Value, String> {
        let space = space.or_else(|| self.model.active_space().map(|p| p.id.clone())).ok_or("no space yet: make one first (space.new)")?;
        if self.model.space(&space).is_none() {
            return Err(format!("no space {space:?}"));
        }
        if !self.folders.contains_key(&m) {
            return Err(format!("no mod {m:?} in mods/"));
        }
        let machine = self.machine_of(&space);
        self.require_document(machine.as_deref())?;
        let (ctx, pane) = (self.model.fresh_id_on("tb", machine.as_deref()), self.model.fresh_id_on("p", machine.as_deref()));
        self.model.add_tab(&space, Tab::with_pane(&ctx, name.trim(), Node::block(pane.clone(), Content::Mod { name: m, props })));
        self.model.active = Some(space.clone());
        Ok(json!({ "space": space, "tab": ctx, "pane": pane }))
    }

    fn split(&mut self, pane: Option<String>, axis: Axis, argv: Option<Vec<String>>, folder: Option<String>) -> Result<Value, String> {
        let target = pane.or_else(|| self.model.focused_pane()).ok_or("no pane to split")?;
        let (Some(p), Some(c)) = (self.model.space_of(&target), self.model.tab_of(&target)) else { return Err(format!("no pane {target:?}")) };
        let (space, ctx, fallback) = (p.id.clone(), c.id.clone(), p.folder.clone());
        let target_addr = self.model.address_of(&target);
        let cwd = folder.unwrap_or_else(|| target_addr.as_ref().and_then(|a| self.cwds.get(a).cloned()).unwrap_or(fallback));
        // A split stays on the machine of the pane it splits (else its space's).
        let on = target_addr.and_then(|a| a.split_once('/').map(|(m, _)| m.to_owned())).or_else(|| self.machine_of(&space));
        let new = self.model.fresh_id_on("p", on.as_deref());
        let addr = self.spawn(argv, cwd, &space, &ctx, on.as_deref())?;
        self.model.split(&target, axis, &new, &addr);
        Ok(json!({ "pane": new, "terminal": addr }))
    }

    /// A mod as a pane beside the target (a viewer next to the shell).
    fn split_mod(&mut self, pane: Option<String>, axis: Axis, name: String, props: Value) -> Result<Value, String> {
        let target = pane.or_else(|| self.model.focused_pane()).ok_or("no pane to split")?;
        if self.model.tab_of(&target).is_none() {
            return Err(format!("no pane {target:?}"));
        }
        if !self.folders.contains_key(&name) {
            return Err(format!("no mod {name:?} in mods/"));
        }
        let machine = self.model.space_of(&target).and_then(|s| s.machine.clone());
        self.require_document(machine.as_deref())?;
        let new = self.model.fresh_id_on("p", machine.as_deref());
        self.model.split_with(&target, axis, Node::block(new.clone(), Content::Mod { name, props }));
        Ok(json!({ "pane": new }))
    }

    /// Ask only whether a missing terminal is closed. Core owns restarting it.
    fn check_closed(&mut self, missing: Vec<(String, String)>, window: &mut Window, cx: &mut Context<Self>) {
        self.checking_closed = true;
        let core = self.core.clone();
        let generation = self.core_generation;
        cx.spawn_in(window, async move |this, cx| {
            let closed = cx.background_executor().spawn(async move {
                missing.into_iter().filter(|(_, addr)| match core.call("terminal.get", json!({"terminal":addr})) {
                    Ok(v) => matches!(v["reason"].as_str(), Some("exited" | "killed")),
                    Err(e) => crate::hc::error_kind(&e) == Some("not_found"),
                }).collect::<Vec<_>>()
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.checking_closed = false;
                if generation != this.core_generation { return; }
                let mut changed = false;
                for (pane, addr) in closed {
                    if this.model.address_of(&pane).as_deref() == Some(&addr) {
                        this.notify_launch_failure(&addr, window, cx);
                        this.model.remove_pane(&pane);
                        changed = true;
                    }
                }
                if changed { this.sync(window, cx); }
            });
        }).detach();
    }

    fn just_born(&self, addr: &str) -> bool {
        let mut born = self.born.borrow_mut();
        // Keep launch facts until hidden panes have passed through `check_closed`.
        born.retain(|_, t| t.elapsed() < Duration::from_secs(30));
        born.get(addr).is_some_and(|t| t.elapsed() < Duration::from_secs(5))
    }

    /// Gives a running terminal without a pane (a sub-agent, say) one in the
    /// shown space and focuses it; already in a pane: focuses that.
    fn keep_terminal(&mut self, addr: &str, place: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, String> {
        if let Some(p) = self.model.pane_of(addr) {
            self.model.focus_pane(&p);
            return Ok(json!({ "pane": p }));
        }
        if !self.live.contains_key(addr) { return Err(format!("no running terminal {addr:?}")); }
        if self.changing_terminals.contains(addr) { return Err("This terminal already has a pane change in progress.".into()); }
        let space = self.model.active_space().map(|p| p.id.clone()).ok_or("no space to keep it in")?;
        let machine = self.machine_of(&space);
        if addr.split_once('/').map(|(m, _)| m) != machine.as_deref() {
            return Err("A terminal's pane must stay on its own machine.".into());
        }
        self.require_document(machine.as_deref())?;
        let target = matches!(place, "right" | "down").then(|| self.model.focused_pane()).flatten();
        let ctx = target.as_deref().and_then(|p| self.model.tab_of(p)).map(|t| t.id.clone())
            .unwrap_or_else(|| self.model.fresh_id_on("tb", machine.as_deref()));
        let pane = self.model.fresh_id_on("p", machine.as_deref());
        let name = self.live.get(addr).and_then(|r| r["name"].as_str()).filter(|s| !s.is_empty()).map(str::to_owned)
            .or_else(|| self.model.agents.get(addr).and_then(|a| a.app.as_deref()).map(agent_name)).unwrap_or_else(|| addr.to_owned());
        let (address, place) = (addr.to_owned(), place.to_owned());
        let reply = json!({"pending":true,"terminal":addr,"pane":pane,"tab":ctx});
        self.start_terminal_change(address, TerminalChange::Keep {
            space, tab: ctx, pane, name, target, axis: if place == "right" { Axis::Row } else { Axis::Column },
        }, window, cx);
        Ok(reply)
    }

    /// Parking changes labels/keep in the background; only a successful reply removes the pane.
    fn park_terminal(&mut self, addr: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<Value, String> {
        self.require_document(addr.split_once('/').map(|(m, _)| m))?;
        if self.changing_terminals.contains(addr) { return Err("This terminal already has a pane change in progress.".into()); }
        self.start_terminal_change(addr.to_owned(), TerminalChange::Park, window, cx);
        Ok(json!({"pending":true,"terminal":addr}))
    }

    fn start_terminal_change(&mut self, address: String, change: TerminalChange, window: &mut Window, cx: &mut Context<Self>) {
        let machine = address.split_once('/').map(|(m, _)| m);
        let (keep, labels) = match &change {
            TerminalChange::Keep { space, tab, .. } => (true, json!({
                "herdr.space":crate::mirror::local(space, machine), "herdr.tab":crate::mirror::local(tab, machine),
            })),
            TerminalChange::Park => (false, json!({"herdr.space":"","herdr.tab":""})),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.changing_terminals.insert(address.clone());
        self.pending_terminals.insert(address.clone(), PendingTerminalChange { change, reply: rx });
        let core = self.core.clone();
        let addr = address.clone();
        // The drain must not need the foreground continuation to start this work.
        let job = cx.background_executor().spawn(async move {
            let result = (|| {
                core.call("terminal.label", json!({"terminal":addr,"labels":labels}))?;
                core.call("terminal.keep", json!({"terminal":addr,"keep":keep}))?;
                anyhow::Ok(())
            })().map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        cx.spawn_in(window, async move |this, cx| {
            job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.draining { return; }
                let Some(pending) = this.pending_terminals.remove(&address) else { return };
                let result = pending.reply.try_recv().unwrap_or_else(|e| Err(format!("terminal change reply missing: {e}")));
                this.apply_terminal_change(address, pending.change, result, window, cx);
            });
        }).detach();
    }

    fn apply_terminal_change(&mut self, address: String, change: TerminalChange, result: Result<(), String>, window: &mut Window, cx: &mut Context<Self>) {
        self.changing_terminals.remove(&address);
        if let Err(error) = result {
            tracing::error!("terminal change for {address} failed: {error}");
            if !self.draining { self.layout_notice("Could not change terminal pane", &error, window, cx); }
            return;
        }
        match change {
            TerminalChange::Keep { space, tab, pane, name, target, axis } => {
                if !self.live.contains_key(&address) { return; }
                if self.model.space(&space).is_none() {
                    if let Err(error) = self.park_terminal(&address, window, cx) {
                        tracing::error!("cannot undo pane placement into closed space: {error}");
                    }
                    return;
                }
                if self.model.pane_of(&address).is_none() {
                    let destination = target.filter(|p| self.model.tab_of(p).is_some_and(|t| t.id == tab))
                        .or_else(|| self.model.tab(&tab).and_then(|t| t.root.panes().first().map(|p| p.id.clone())));
                    if let Some(target) = destination {
                        self.model.split(&target, axis, &pane, &address);
                    } else {
                        self.model.add_tab(&space, Tab::new(&tab, &name, &pane, &address));
                    }
                }
                if let Some(pane) = self.model.pane_of(&address) { self.model.focus_pane(&pane); }
            }
            TerminalChange::Park => {
                if let Some(pane) = self.model.pane_of(&address) { self.model.remove_pane(&pane); }
            }
        }
        if !self.draining {
            self.sync(window, cx);
            self.flush_documents(false, window, cx);
        }
    }

    /// Kills terminals and their sub-agents, all the way down. Core keeps a
    /// sub-agent running when its parent dies; closing one here means done with
    /// its whole tree. Kept sub-agents (given a tab) are the person's: spared.
    fn kill(&mut self, addresses: &[String], cx: &mut Context<Self>) {
        let mut doomed: Vec<String> = addresses.to_vec();
        let mut i = 0;
        while i < doomed.len() {
            let parent = doomed[i].clone();
            for (addr, rec) in &self.live {
                let child = rec.get("parent").and_then(Value::as_str) == Some(parent.as_str());
                let kept = rec.get("labels").and_then(|l| l.get("herdr.space")).and_then(Value::as_str).is_some_and(|w| !w.is_empty());
                if child && !kept && !doomed.contains(addr) {
                    doomed.push(addr.clone());
                }
            }
            i += 1;
        }
        self.closing_terminals.extend(doomed.iter().cloned().map(|a| (a, false)));
        // Off the UI thread: a kill waits for the program to go, and for slow machines.
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            for address in doomed {
                let core = core.clone();
                let addr = address.clone();
                let result = cx.background_executor().spawn(async move {
                    core.call("terminal.kill", json!({ "terminal": addr }))
                }).await;
                let _ = this.update(cx, |this, _| {
                    if result.is_ok() && this.live.contains_key(&address) {
                        // The live snapshot may still predate the kill's reply.
                        this.closing_terminals.insert(address, true);
                    } else {
                        this.closing_terminals.remove(&address);
                    }
                });
            }
        }).detach();
    }

    // ---- commands ----------------------------------------------------------

    /// Runs a keys.json action string: `name`, `name:value` (the value fills
    /// the command's first argument), `send:<text>`, or `<mod>.<command>`.
    fn run_action(&mut self, action: &str, window: &mut Window, cx: &mut Context<Self>) {
        // `send:` is only meaningful in a terminal, which handles it before it gets here.
        if !action.starts_with("send:") {
            self.run(action, Value::Null, window, cx);
        }
    }

    fn argv(args: &Value) -> Option<Vec<String>> {
        args.get("argv").and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    fn arg<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
        args.get(k).and_then(Value::as_str)
    }

    fn owned(args: &Value, k: &str) -> Option<String> {
        Self::arg(args, k).map(str::to_owned)
    }

    fn answer(&mut self, r: Result<Value, String>) {
        if let Err(e) = &r {
            tracing::warn!("{e}");
        }
        self.reply = Some(r.unwrap_or_else(|e| json!({ "error": e })));
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.answer(Err(message.into()));
    }

    /// Runs a command for the person (keys, clicks, menus, mods): a missing
    /// name is asked for.
    fn run(&mut self, name: &str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        // A click or key has nobody to read the reply: show why it failed.
        if let Some(error) = self.run_as(name, args, true, window, cx).get("error").and_then(Value::as_str) {
            let spec = json!({"kind":"toast", "notification":{"title":format!("{name} failed"),"body":error,"level":"error"}});
            self.open_overlay(mods::new_overlay_id(), spec, window, cx);
        }
    }

    /// Runs a command and says what it made or why not. Scripts and agents
    /// (`person: false`) never get a dialog: a missing argument is an error.
    fn run_as(&mut self, name: &str, mut args: Value, person: bool, window: &mut Window, cx: &mut Context<Self>) -> Value {
        // A mod someone edited under 0.1.0 still says project.* / context.*.
        let current = crate::keys::current(name);
        let name = current.as_str();
        // `name:value`, as in keys.json.
        if let Some((n, v)) = name.split_once(':')
            && let Some(c) = crate::commands::find(n)
        {
            if let Err(error) = crate::commands::validate_args(c, &args) {
                return json!({ "error": error });
            }
            if c.args.is_empty() {
                return json!({ "error": format!("{n}: unexpected shorthand argument; valid arguments: none") });
            }
            let mut short = crate::commands::shorthand(c, v);
            if let (Some(s), Some(extra)) = (short.as_object_mut(), args.as_object()) {
                s.extend(extra.clone());
            }
            args = short;
            return self.run_as(n, args, person, window, cx);
        }
        let Some(command) = crate::commands::find(name) else {
            // `<mod>.<command>` goes to that mod's nextCommand().
            if let Some((m, c)) = name.split_once('.')
                && mods::send_command(m, c)
            {
                return json!({ "ok": true });
            }
            tracing::warn!("unknown command {name:?}");
            return json!({ "error": format!("unknown command {name:?}: see `herdr-app commands`; a mod's are <mod>.<command> and need the mod loaded") });
        };
        if let Err(error) = crate::commands::validate_args(command, &args) {
            return json!({ "error": error });
        }
        if terminal_block(&self.chrome, self.focused_pane.as_deref()).is_some() && needs_model_pane(name, &args) {
            return json!({ "error": "a terminal layout block is focused, not a pane; focus a pane or pass an explicit pane id" });
        }
        if person && name.ends_with(".rename") && args.get("name").is_none_or(Value::is_null) {
            self.ask_name(name, args, window, cx);
            return json!({ "asked": "name" });
        }
        if let Some(arg) = crate::commands::missing(command, &args) {
            return json!({ "error": format!("{name} needs {arg}") });
        }
        if person {
            crate::telemetry::command(command.name);
        }
        if matches!(name, "space.new" | "space.close" | "space.rename" | "space.move" | "tab.new" | "tab.close" | "tab.rename" | "tab.move" | "pane.split" | "pane.close" | "pane.swap" | "terminal.keep" | "terminal.park") {
            let node = ["id", "space", "pane"].into_iter().find_map(|k| Self::arg(&args, k));
            let target = node.and_then(|id| self.model.space(id).or_else(|| self.model.space_of(id))).or_else(|| self.model.active_space());
            let machine = if name == "space.new" { Self::arg(&args, "machine") }
                else if name == "terminal.park" { Self::arg(&args, "terminal").and_then(|a| a.split_once('/').map(|(m, _)| m)) }
                else { target.and_then(|s| s.machine.as_deref()) };
            if let Err(error) = self.require_document(machine) { return json!({"error":error}); }
        }
        if name == "layout.resize"
            && let Some(space) = Self::arg(&args, "id").and_then(|id| self.model.space_of(id))
            && let Err(error) = self.require_document(space.machine.as_deref())
        { return json!({"error":error}); }
        self.reply = None;
        self.dispatch(name, &args, person, window, cx);
        self.flush_documents(false, window, cx);
        self.reply.take().unwrap_or_else(|| json!({ "ok": true }))
    }

    /// Asks the person for a new name (space.rename, tab.rename), then runs it with it.
    /// The prompt starts with the name someone gave, empty for an automatic one.
    fn ask_name(&mut self, name: &str, mut args: Value, window: &mut Window, cx: &mut Context<Self>) {
        if !args.is_object() {
            args = json!({});
        }
        let (title, hint, current) = match name {
            "space.rename" => {
                let id = Self::owned(&args, "id").or_else(|| self.model.active.clone());
                let current = id.as_deref().and_then(|i| self.model.space(i)).map(|p| p.name.clone());
                args["id"] = json!(id);
                ("rename space", "empty: named after its folder", current)
            }
            _ => {
                let id = Self::owned(&args, "id").or_else(|| self.model.active_tab().map(|c| c.id.clone()));
                let current = id.as_deref().and_then(|i| self.model.tab(i)).map(|c| c.name.clone());
                args["id"] = json!(id);
                ("rename tab", "empty: its number", current)
            }
        };
        let spec = json!({ "mod": "prompt", "kind": "dialog", "width": 380,
            "props": { "title": title, "value": current.as_deref().unwrap_or(""), "hint": hint, "command": { "name": name, "args": args } } });
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    fn dispatch(&mut self, name: &str, a: &Value, person: bool, window: &mut Window, cx: &mut Context<Self>) {
        let active_ctx = self.model.active_tab().map(|c| c.id.clone());
        match name {
            // ---- spaces
            "space.new" => {
                let name = Self::arg(a, "name").unwrap_or_default();
                let r = self.new_space(name, Self::owned(a, "folder"), Self::arg(a, "tab"), Self::argv(a), Self::arg(a, "task"), Self::owned(a, "machine").filter(|m| !m.is_empty()));
                self.answer(r);
            }
            "space.close" => {
                let Some(id) = Self::owned(a, "id").or_else(|| self.model.active.clone()) else { return self.fail("no space to close") };
                let Some(p) = self.model.space(&id) else { return self.fail(format!("no space {id:?}")) };
                if a.get("confirmed").and_then(Value::as_bool) != Some(true) {
                    if !person {
                        return self.fail("space.close ends its terminals: pass confirmed: true");
                    }
                    let panes: usize = p.tabs.iter().map(|c| c.root.panes().len()).sum();
                    let detail = format!("{} \u{2014} {} tabs, {} terminals", self.model.space_name(p, &home()), p.tabs.len(), panes);
                    self.confirm("Close space?", &detail, "space.close", json!({ "id": id, "confirmed": true }), window, cx);
                    return;
                }
                let addrs = self.model.remove_space(&id);
                self.kill(&addrs, cx);
            }
            "space.rename" => {
                let Some(id) = Self::owned(a, "id").or_else(|| self.model.active.clone()) else { return self.fail("no space to rename") };
                let Some(name) = Self::arg(a, "name") else { return self.fail("space.rename needs name (empty: automatic)") };
                let Some(p) = self.model.space_mut(&id) else { return self.fail(format!("no space {id:?}")) };
                p.name = name.trim().to_owned();
            }
            "space.move" => {
                let (Some(id), Some(i)) = (Self::arg(a, "id"), a.get("index").and_then(Value::as_u64)) else { return self.fail("space.move needs id and index") };
                if !self.model.move_space(id, i as usize) { return self.fail(format!("no space {id:?}")); }
            }
            "space.select" => {
                let id = Self::owned(a, "id").or_else(|| {
                    let i = a.get("index").and_then(Value::as_u64)? as usize;
                    self.model.spaces.get(i.checked_sub(1)?).map(|p| p.id.clone())
                });
                match id.filter(|id| self.model.space(id).is_some()) {
                    Some(id) => self.model.active = Some(id),
                    None => return self.fail("no such space"),
                }
            }
            "space.next" | "space.prev" => {
                let n = self.model.spaces.len();
                if let Some(i) = self.model.active_index().filter(|_| n > 0) {
                    let j = if name == "space.next" { (i + 1) % n } else { (i + n - 1) % n };
                    self.model.active = Some(self.model.spaces[j].id.clone());
                }
            }
            // ---- tabs
            "tab.new" => {
                let name = Self::arg(a, "name").unwrap_or_default();
                let r = match Self::owned(a, "mod") {
                    Some(m) => self.new_mod_tab(Self::owned(a, "space"), name, m, a.get("props").cloned().unwrap_or(Value::Null)),
                    // A person's new tab with no space open starts one, as space.new does.
                    None if person && a.get("space").is_none() && self.model.active_space().is_none() => {
                        self.new_space("", Self::owned(a, "folder"), Some(name), Self::argv(a), Self::arg(a, "task"), None)
                    }
                    None => self.new_tab(Self::owned(a, "space"), name, Self::owned(a, "folder"), Self::argv(a), Self::arg(a, "task")),
                };
                self.answer(r);
            }
            "tab.close" => {
                let Some(id) = Self::owned(a, "id").or(active_ctx) else { return self.fail("no tab to close") };
                let Some(p) = self.model.space_of(&id) else { return self.fail(format!("no tab {id:?}")) };
                // The last tab takes its space with it: ask like closing the space.
                if p.tabs.len() == 1 {
                    let mut args = json!({ "id": p.id });
                    args["confirmed"] = a.get("confirmed").cloned().unwrap_or(Value::Null);
                    return self.dispatch("space.close", &args, person, window, cx);
                }
                let (addrs, _) = self.model.remove_tab(&id);
                self.kill(&addrs, cx);
            }
            "tab.rename" => {
                let Some(id) = Self::owned(a, "id").or(active_ctx) else { return self.fail("no tab to rename") };
                let Some(name) = Self::arg(a, "name") else { return self.fail("tab.rename needs name (empty: its number)") };
                let Some(c) = self.model.tab_mut(&id) else { return self.fail(format!("no tab {id:?}")) };
                c.name = name.trim().to_owned();
            }
            "tab.move" => {
                let (Some(id), Some(i)) = (Self::arg(a, "id"), a.get("index").and_then(Value::as_u64)) else { return self.fail("tab.move needs id and index") };
                let Some(pid) = self.model.space_of(id).map(|p| p.id.clone()) else { return self.fail(format!("no tab {id:?}")) };
                let p = self.model.space_mut(&pid).unwrap();
                let from = p.tabs.iter().position(|c| c.id == id).unwrap();
                let c = p.tabs.remove(from);
                let to = (i as usize).saturating_sub(1).min(p.tabs.len());
                p.tabs.insert(to, c);
            }
            "tab.settle" | "tab.unsettle" => {
                let Some(id) = Self::owned(a, "id").or(active_ctx) else { return self.fail("no tab") };
                let found = if name == "tab.settle" { self.model.settle(&id, model::now_ms()) } else { self.model.tab(&id).is_some() && { self.model.unsettle(&id); true } };
                if !found {
                    return self.fail(format!("no tab {id:?}"));
                }
            }
            "tab.select" => {
                // By position among the tabs the lists show (settled ones are put away).
                let id = Self::owned(a, "id").or_else(|| {
                    let i = a.get("index").and_then(Value::as_u64)? as usize;
                    self.model.active_space()?.tabs.iter().filter(|c| c.settled.is_none()).nth(i.checked_sub(1)?).map(|c| c.id.clone())
                });
                if !id.is_some_and(|id| self.model.select_tab(&id)) {
                    return self.fail("no such tab");
                }
            }
            "tab.next" | "tab.prev" => {
                if let Some(p) = self.model.active_space()
                    && let Some(i) = active_ctx.as_ref().and_then(|c| p.tabs.iter().position(|x| x.id == *c))
                {
                    let n = p.tabs.len();
                    let step = |k: usize| if name == "tab.next" { (i + k) % n } else { (i + n * k - k) % n };
                    if let Some(id) = (1..n).map(|k| &p.tabs[step(k)]).find(|c| c.settled.is_none()).map(|c| c.id.clone()) {
                        self.model.select_tab(&id);
                    }
                }
            }
            // ---- panes
            "pane.split" => {
                let axis = match Self::arg(a, "direction") {
                    Some("right") => Axis::Row,
                    Some("down") => Axis::Column,
                    other => return self.fail(format!("pane.split direction is \"right\" or \"down\", not {other:?}")),
                };
                let r = match Self::owned(a, "mod") {
                    Some(name) => self.split_mod(Self::owned(a, "pane"), axis, name, a.get("props").cloned().unwrap_or(Value::Null)),
                    None => self.split(Self::owned(a, "pane"), axis, Self::argv(a), None),
                };
                self.answer(r);
            }
            "pane.close" => {
                let Some(pane) = Self::owned(a, "pane").or_else(|| self.model.focused_pane()) else { return self.fail("no pane to close") };
                let Some(c) = self.model.tab_of(&pane) else { return self.fail(format!("no pane {pane:?}")) };
                if c.root.find(&pane).is_some_and(|n| !n.closable()) {
                    return;
                }
                if c.root.panes().len() == 1 {
                    let id = c.id.clone();
                    return self.dispatch("tab.close", &json!({ "id": id }), person, window, cx);
                }
                let addr = self.model.address_of(&pane);
                self.model.remove_pane(&pane);
                self.kill(&addr.into_iter().collect::<Vec<_>>(), cx);
            }
            "pane.zoom" => {
                let pane = Self::owned(a, "pane").or_else(|| self.model.focused_pane());
                if let Some(pane) = pane
                    && let Some(ctx) = self.model.tab_of(&pane).map(|c| c.id.clone())
                    && let Some(c) = self.model.tab_mut(&ctx)
                    && c.root.panes().len() > 1
                {
                    c.zoomed = if c.zoomed.as_deref() == Some(pane.as_str()) { None } else { Some(pane) };
                }
            }
            "pane.focus" => {
                if Self::arg(a, "pane").is_none()
                    && let Some(terminal) = Self::arg(a, "terminal")
                    && let Some(block) = self.chrome.visible_blocks().into_iter().find(|n| n.terminal() == Some(terminal))
                {
                    let id = block.id.clone();
                    self.focused_pane = Some(id.clone());
                    self.synced_model_pane = self.model.focused_pane();
                    if !self.overlays.iter().any(|o| o.kind.modal_keys()) && let Some(f) = self.pane_focus(&id) {
                        f.focus(window, cx);
                    }
                    self.publish(cx);
                    cx.notify();
                    return;
                }
                let pane = Self::owned(a, "pane").or_else(|| Self::arg(a, "terminal").and_then(|t| self.model.pane_of(t)));
                let Some(p) = pane.filter(|p| self.model.tab_of(p).is_some()) else { return self.fail("no such pane") };
                self.model.focus_pane(&p);
                self.focused_pane = None;
                self.agent = self.model.address_of(&p);
                self.swapped.remove(&p);
            }
            "pane.go" | "pane.swap" => {
                let dir = Self::arg(a, "direction").unwrap_or_default();
                if !matches!(dir, "left" | "right" | "up" | "down") {
                    return self.fail(format!("{name} direction is left, right, up or down, not {dir:?}"));
                }
                if let Some(from) = self.model.focused_pane()
                    && let Some(to) = self.neighbor(&from, dir)
                {
                    if name == "pane.swap" {
                        if let Some(ctx) = self.model.tab_of(&from).map(|c| c.id.clone())
                            && let Some(c) = self.model.tab_mut(&ctx)
                        {
                            c.root.swap(&from, &to);
                        }
                    } else {
                        self.model.focus_pane(&to);
                        self.focused_pane = None;
                    }
                }
            }
            // ---- terminals
            "terminal.open" => {
                let argv = Self::argv(a);
                let r = match Self::arg(a, "place").unwrap_or("tab") {
                    "right" => self.split(None, Axis::Row, argv, Self::owned(a, "folder")),
                    "down" => self.split(None, Axis::Column, argv, Self::owned(a, "folder")),
                    _ => {
                        let name = Self::owned(a, "name")
                            .or_else(|| argv.as_ref().and_then(|v| v.first()).map(|p| p.rsplit('/').next().unwrap_or(p).to_owned()))
                            .unwrap_or_else(|| "shell".into());
                        self.new_tab(None, &name, Self::owned(a, "folder"), argv, None)
                    }
                };
                self.answer(r);
            }
            "terminal.keep" => {
                let r = self.keep_terminal(Self::arg(a, "terminal").unwrap_or_default(), Self::arg(a, "place").unwrap_or("tab"), window, cx);
                self.answer(r);
            }
            "terminal.park" => {
                let r = self.park_terminal(Self::arg(a, "terminal").unwrap_or_default(), window, cx);
                self.answer(r);
            }
            "terminal.right_click" => {
                let to = match Self::arg(a, "to") {
                    Some("program") => "program",
                    Some("menu") => "",
                    other => return self.fail(format!("terminal.right_click to is \"program\" or \"menu\", not {other:?}")),
                };
                let t = Self::owned(a, "terminal").or_else(|| self.focused_terminal(window).map(|t| t.address.clone()));
                let Some(t) = t.filter(|t| self.live.contains_key(t)) else { return self.fail("no such running terminal") };
                // Off the UI thread: a terminal on a slow machine answers late.
                let core = self.core.clone();
                let params = json!({ "terminal": t, "labels": { "herdr.right-click": to } });
                cx.background_executor()
                    .spawn(async move {
                        if let Err(e) = core.call("terminal.label", params) {
                            tracing::warn!("right-click label: {e:#}");
                        }
                    })
                    .detach();
            }
            "terminal.pin" => {
                let Some(pinned) = a.get("pinned").and_then(Value::as_bool) else { return self.fail("terminal.pin pinned is true or false") };
                let t = Self::arg(a, "terminal").unwrap_or_default().to_owned();
                if !self.live.contains_key(&t) {
                    return self.fail(format!("no running terminal {t:?}"));
                }
                let core = self.core.clone();
                let params = json!({ "terminal": t, "labels": { "herdr.pinned": if pinned { "1" } else { "" } } });
                cx.background_executor()
                    .spawn(async move {
                        if let Err(e) = core.call("terminal.label", params) {
                            tracing::warn!("pin label: {e:#}");
                        }
                    })
                    .detach();
            }
            "terminal.kill" => {
                let t = Self::arg(a, "terminal").unwrap_or_default();
                if !self.live.contains_key(t) {
                    return self.fail(format!("no running terminal {t:?}"));
                }
                self.kill(&[t.to_owned()], cx);
            }
            "terminal.peek" => {
                let t = Self::arg(a, "terminal").unwrap_or_default();
                if !self.live.contains_key(t) {
                    return self.fail(format!("no running terminal {t:?}"));
                }
                self.peek_terminal(t, window, cx);
            }
            "terminal.show" => {
                let pane = Self::owned(a, "pane").or_else(|| self.model.focused_pane());
                let target = Self::arg(a, "terminal").filter(|t| self.live.contains_key(*t)).map(str::to_owned);
                if let Some(p) = pane {
                    match target {
                        // One with a pane of its own is shown there.
                        Some(t) if let Some(own) = self.model.pane_of(&t) => {
                            self.swapped.remove(&p);
                            self.model.focus_pane(&own);
                        }
                        Some(t) => {
                            self.swapped.insert(p.clone(), t);
                            self.model.focus_pane(&p);
                        }
                        None => {
                            self.swapped.remove(&p);
                        }
                    }
                    self.focused_pane = None;
                }
            }
            "terminal.copy" | "terminal.paste" | "terminal.paste_image" => {
                if let Some(t) = self.focused_terminal(window) {
                    t.view.update(cx, |v, cx| match name {
                        "terminal.paste" => v.paste(cx),
                        "terminal.paste_image" => v.paste_image(cx),
                        _ => v.copy(cx),
                    });
                }
                return;
            }
            // ---- agents
            "agent.focus" => {
                let t = Self::arg(a, "terminal").unwrap_or_default().to_owned();
                if !self.live.contains_key(&t) {
                    return self.fail(format!("no running terminal {t:?}"));
                }
                self.focus_agent(&t);
            }
            "agent.next" | "agent.prev" => {
                let order = self.agent_order();
                if order.is_empty() {
                    return;
                }
                let current = self.shown_agent().map(|a| self.root_agent(&a));
                let n = order.len();
                let next = match current.and_then(|c| order.iter().position(|a| *a == c)) {
                    Some(i) if name == "agent.next" => (i + 1) % n,
                    Some(i) => (i + n - 1) % n,
                    None if name == "agent.next" => 0,
                    None => n - 1,
                };
                let addr = order[next].clone();
                self.focus_agent(&addr);
            }
            // ---- layout
            "layout.switch" => {
                self.switch_layout(Self::arg(a, "name"), window, cx);
                return;
            }
            "layout.clone" => {
                self.save();
                let name = Self::owned(a, "name").unwrap_or_else(|| self.layout_name.clone());
                let to = Self::owned(a, "as").unwrap_or_else(|| format!("{name}-copy"));
                if let Err(e) = clone_layout(&name, &to) {
                    return self.fail(format!("cannot clone layout: {e}"));
                }
                self.switch_layout(Some(&to), window, cx);
                return;
            }
            "mod.clone" => {
                let name = Self::arg(a, "name").unwrap_or_default();
                let to = Self::owned(a, "as").unwrap_or_else(|| format!("{name}-copy"));
                if let Err(e) = mods::clone_mod(name, &to) {
                    return self.fail(format!("cannot clone mod: {e}"));
                }
                self.folders = mods::scan().into_iter().map(|f| (f.id.clone(), f)).collect();
            }
            "layout.set" if shipped_layouts().contains(&self.layout_name.as_str()) => {
                let n = match serde_json::from_value::<Node>(a.get("layout").cloned().unwrap_or_default()) {
                    Ok(n) => n,
                    Err(e) => return self.fail(format!("layout: {e}")),
                };
                if layout_sync::without_clicks(&self.chrome, &n) != self.chrome {
                    return self.fail("Only supported click/look fields can change on Herdr's own layouts: clone one for other changes");
                }
                let mut state = layout_sync::State::between(&self.file_chrome, &n);
                if n.look.theme == self.chrome.look.theme {
                    state.theme = self.picked_theme.clone();
                }
                self.chrome = self.file_chrome.clone();
                state.apply(&mut self.chrome);
                self.picked_theme = state.theme;
                self.apply_look(window, cx);
            }
            // A deliberate edit: the file becomes this tree, clicks start over.
            "layout.set" => match serde_json::from_value::<Node>(a.get("layout").cloned().unwrap_or_default()) {
                Ok(mut n) => {
                    fill_ids(&mut n, &mut self.model);
                    let text = layout_sync::text_of(&n);
                    if !write_atomic(&layout_path(&self.layout_name), &text) {
                        return self.fail(format!("cannot write layouts/{}.json", self.layout_name));
                    }
                    self.written_layout = text;
                    self.layout_file_ok = true;
                    self.file_chrome = n.clone();
                    self.chrome = n;
                    self.picked_theme = None;
                    self.write_state(&layout_sync::State::default());
                    self.apply_look(window, cx);
                }
                Err(e) => return self.fail(format!("layout: {e}")),
            },
            "layout.toggle" | "layout.show" | "layout.hide" => {
                let id = Self::arg(a, "id").unwrap_or_default();
                let Some(n) = self.chrome.find_mut(id) else { return self.fail(format!("no node {id:?} in the shown layout")) };
                n.hidden = match name {
                    "layout.toggle" => !n.hidden,
                    "layout.show" => false,
                    _ => true,
                };
            }
            "layout.resize" => {
                let id = Self::arg(a, "id").unwrap_or_default();
                let Some(n) = self.find_node_mut(id) else { return self.fail(format!("no node {id:?}")) };
                if let Some(px) = a.get("px").and_then(Value::as_f64) {
                    n.size = Some(Span::Px { px: px as f32 });
                } else if let Some(w) = a.get("weight").and_then(Value::as_f64) {
                    n.size = Some(Span::Weight { weight: w as f32 });
                }
            }
            "layout.reset" => {
                let name = Self::owned(a, "name").unwrap_or_else(|| self.layout_name.clone());
                let Some((_, text)) = LAYOUTS.iter().find(|(n, _)| *n == name) else {
                    return self.fail(format!("{name:?} isn't a layout Herdr ships"));
                };
                // Theirs is kept under a name nothing else has (never over an older one).
                if layout_path(&name).exists() {
                    if let Err(e) = std::fs::rename(layout_path(&name), aside(&layout_path(&name), "old")) {
                        return self.fail(format!("cannot set layouts/{name}.json aside: {e}"));
                    }
                }
                if !write_atomic(&layout_path(&name), text) {
                    return self.fail(format!("cannot write layouts/{name}.json"));
                }
                let _ = std::fs::remove_file(state_path(&name));
                if name == self.layout_name {
                    self.load_layout(&name);
                    self.apply_look(window, cx);
                }
            }
            "layout.select" => {
                let (s, ch) = (Self::arg(a, "stack").unwrap_or_default(), Self::arg(a, "child").unwrap_or_default());
                if !self.chrome.select(s, ch) {
                    return self.fail(format!("no stack {s:?} with a child {ch:?}"));
                }
            }
            "mod.open" => {
                let name = Self::arg(a, "name").unwrap_or_default();
                let Some(f) = self.folders.get(name) else { return self.fail(format!("no mod folder {name:?}")) };
                let mut spec = mod_overlay(&f.dir);
                spec["mod"] = json!(name);
                // One popup per mod: running mod.open again toggles it, like app.settings.
                let id = format!("mod-{name}");
                if self.overlays.iter().any(|o| o.id == id) {
                    self.close_overlay(&id, window, cx);
                } else {
                    self.open_overlay(id, spec, window, cx);
                }
            }
            "overlay.close" => {
                let id = Self::owned(a, "id").or_else(|| self.overlays.iter().rev().find(|o| o.kind.modal_keys()).map(|o| o.id.clone()));
                if let Some(id) = id {
                    self.close_overlay(&id, window, cx);
                }
                return;
            }
            // ---- the app
            "app.welcome" => {
                self.open_welcome(window, cx);
                return;
            }
            "app.whats_new" => {
                self.open_whats_new(true, window, cx);
                return;
            }
            "app.palette" => {
                // Once: a second press closes the palette instead of stacking another.
                if self.overlays.iter().any(|o| o.id == "palette") {
                    self.close_overlay("palette", window, cx);
                } else {
                    let spec = json!({ "mod": "frame-palette", "kind": "popup", "at": "top", "offset": 60, "width": 620, "height": 0.85 });
                    self.open_overlay("palette".into(), spec, window, cx);
                }
                return;
            }
            "app.feedback" => {
                let from = Self::owned(a, "from").unwrap_or_else(|| "person".into());
                if !matches!(from.as_str(), "person" | "agent") {
                    return self.fail("from must be person or agent");
                }
                // A window, not a dialog: the app stays usable behind it (copy an error, paste it in).
                let spec = json!({ "mod": "frame-feedback", "kind": "window", "key": "feedback", "width": 680,
                    "props": { "message": Self::owned(a, "message").unwrap_or_default(), "from": from, "layout": self.layout_name } });
                self.open_overlay(mods::new_overlay_id(), spec, window, cx);
                return;
            }
            "app.settings" => {
                // Once: a second press doesn't stack another; asking for a section opens it there.
                let section = Self::owned(a, "section");
                let open = self.overlays.iter().any(|o| o.id == "settings");
                if open && section.is_some() {
                    self.close_overlay("settings", window, cx);
                }
                if !open || section.is_some() {
                    let spec = json!({ "mod": "frame-settings", "kind": "dialog", "width": 880, "props": { "section": section } });
                    self.open_overlay("settings".into(), spec, window, cx);
                }
                return;
            }
            "app.check_updates" => crate::update::check_now(),
            "app.update_core" => {
                if crate::update::core().ready.is_none() && crate::update::core().error.is_none() {
                    return self.fail("herdr-core is up to date");
                }
                crate::update::update_core();
            }
            "app.help" => {
                let spec = json!({ "mod": "help", "kind": "dialog", "width": 640, "height": 520 });
                self.open_overlay(mods::new_overlay_id(), spec, window, cx);
                return;
            }
            "app.reset" => self.reset(window, cx),
            "app.theme" => {
                let name = Self::arg(a, "name").unwrap_or_default().to_owned();
                if !settings::theme_exists(&name) {
                    return self.fail(format!("no theme {name:?}"));
                }
                // A layout is a look: the theme goes on the shown layout's root,
                // as its light or its dark half (the other half stays).
                let current = crate::layout::ThemeChoice::Pair {
                    light: settings::get().theme_light.clone(),
                    dark: settings::get().theme_dark.clone(),
                };
                self.chrome.look.theme = Some(with_theme(Some(current), name));
                self.picked_theme = self.chrome.look.theme.clone();
                self.save();
                self.apply_look(window, cx);
            }
            "app.zoom_in" | "app.zoom_out" | "app.zoom_reset" => {
                let zoom = settings::next_zoom(match name {
                    "app.zoom_in" => 1,
                    "app.zoom_out" => -1,
                    _ => 0,
                });
                self.write_person(|p| p.zoom = zoom, window, cx);
            }
            "app.quit" => cx.quit(),
            "app.toggle" => return self.toggle(window, cx),
            "app.update" => {
                if !crate::update::apply() {
                    return self.fail("no update is ready yet");
                }
                self.drain_documents(window, cx);
                crate::update::relaunch();
                cx.quit();
            }
            "window.close" | "window.minimize" | "window.maximize" => return self.window_command(name, window, cx),
            // Hiding apps is macOS's (bound natively there).
            "app.hide" | "app.hide_others" | "app.show_all" => return,
            other => {
                tracing::warn!("command {other:?} is listed but not handled");
                return;
            }
        }
        self.sync(window, cx);
    }

    fn find_node_mut(&mut self, id: &str) -> Option<&mut Node> {
        if self.chrome.find(id).is_some() {
            return self.chrome.find_mut(id);
        }
        self.model.spaces.iter_mut().flat_map(|p| p.tabs.iter_mut()).find_map(|c| c.root.find_mut(id))
    }

    /// The nearest visible pane in a direction, by what's on screen.
    fn neighbor(&self, from: &str, dir: &str) -> Option<String> {
        let bounds = self.bounds.borrow();
        let cur = *bounds.get(from)?;
        let c = self.model.active_tab()?;
        let candidates: Vec<String> = c.shown().visible_blocks().iter().filter(|n| n.is_pane() && n.id != from).map(|n| n.id.clone()).collect();
        let overlap = |a0: Pixels, a1: Pixels, b0: Pixels, b1: Pixels| f32::from(a1.min(b1) - a0.max(b0)).max(0.0);
        candidates
            .into_iter()
            .filter_map(|id| {
                let r = *bounds.get(&id)?;
                let (gap, ov) = match dir {
                    "left" => (f32::from(cur.left() - r.right()), overlap(cur.top(), cur.bottom(), r.top(), r.bottom())),
                    "right" => (f32::from(r.left() - cur.right()), overlap(cur.top(), cur.bottom(), r.top(), r.bottom())),
                    "up" => (f32::from(cur.top() - r.bottom()), overlap(cur.left(), cur.right(), r.left(), r.right())),
                    _ => (f32::from(r.top() - cur.bottom()), overlap(cur.left(), cur.right(), r.left(), r.right())),
                };
                (gap > -2.0 && ov > 0.0).then_some((gap, -ov, id))
            })
            .min_by(|a, b| a.partial_cmp(b).unwrap())
            .map(|(_, _, id)| id)
    }

    // ---- overlays ----------------------------------------------------------

    fn confirm(&mut self, title: &str, detail: &str, command: &str, args: Value, window: &mut Window, cx: &mut Context<Self>) {
        let spec = json!({ "mod": "confirm", "kind": "dialog", "width": 380, "height": 150,
            "props": { "title": title, "detail": detail, "command": { "name": command, "args": args } } });
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn open_menu(&mut self, tab: &str, target: Value, at: Option<Point<Pixels>>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = at {
            self.pointer = p;
        }
        let requested = self.chrome.terminal_menu.as_deref().unwrap_or("menu");
        let name = if self.folders.contains_key(requested) {
            requested
        } else {
            tracing::warn!(mod_name = requested, "terminal menu mod not found; using menu");
            "menu"
        };
        let defaults = self.folders.get(name).map(|f| mod_overlay(&f.dir)).unwrap_or_else(|| json!({}));
        let spec = terminal_menu_spec(name, &defaults, tab, target);
        self.open_overlay(mods::new_overlay_id(), spec, window, cx);
    }

    fn open_overlay(&mut self, id: String, spec: Value, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match spec.get("kind").and_then(Value::as_str) {
            Some("popup") => OverlayKind::Popup,
            Some("toast") => OverlayKind::Toast,
            Some("window") => OverlayKind::Window,
            _ => OverlayKind::Dialog,
        };
        let key = spec.get("key").and_then(Value::as_str).filter(|k| !k.is_empty() && kind == OverlayKind::Window).map(str::to_owned);
        // One window per name: opening it again replaces it, in its place.
        let mut place = key.as_deref().and_then(movable::place_of);
        if let Some(k) = &key
            && let Some(old) = self.overlays.iter().find(|o| o.key.as_ref() == Some(k)).map(|o| (o.id.clone(), o.place))
        {
            place = old.1.or(place);
            self.remove_overlay(&old.0, window, cx);
        }
        let layout = spec.get("layout").filter(|_| kind != OverlayKind::Toast).and_then(|l| match serde_json::from_value::<Node>(l.clone()) {
            Ok(n) => Some(overlay_layout(n, &id)),
            Err(e) => {
                tracing::warn!("overlay layout: {e}");
                None
            }
        });
        let (name, mut props) = if kind == OverlayKind::Toast {
            ("toast".to_owned(), spec.get("notification").cloned().unwrap_or(Value::Null))
        } else {
            (spec.get("mod").and_then(Value::as_str).unwrap_or_default().to_owned(), spec.get("props").cloned().unwrap_or(json!({})))
        };
        if !props.is_object() {
            props = json!({});
        }
        props["overlay"] = json!(id);
        let num = |k: &str, d: f32| spec.get(k).and_then(Value::as_f64).map_or(d, |v| v as f32);
        let size = match kind {
            OverlayKind::Toast => size(340.0, num("height", 72.0)),
            // No size: as big as its content (0 = unset).
            OverlayKind::Window => size(num("width", 0.0), num("height", 0.0)),
            // No height: as tall as the content, up to most of the window.
            _ => size(num("width", 420.0), num("height", 0.85)),
        };
        let at = match spec.get("at") {
            Some(v) if v.get("x").is_some() => point(px(num_in(v, "x")), px(num_in(v, "y"))),
            _ => self.pointer,
        };
        let top = (spec.get("at").and_then(Value::as_str) == Some("top")).then(|| num("offset", 0.0));
        let fit = layout.is_none() && (kind == OverlayKind::Popup || (matches!(kind, OverlayKind::Dialog | OverlayKind::Window) && spec.get("height").is_none()));
        if kind == OverlayKind::Window && place.is_none() && spec.get("at").is_some_and(|v| v.get("x").is_some()) {
            place = Some(at);
        }
        let timeout = props.get("timeout").and_then(Value::as_f64).unwrap_or(match props.get("level").and_then(Value::as_str) {
            Some("error" | "warning") => 8000.0,
            _ => 5000.0,
        });
        let slot = format!("overlay:{id}");
        // The frame takes the keyboard first, so a mod that focuses itself while loading wins.
        let focus = cx.focus_handle();
        if kind.modal_keys() {
            focus.focus(window, cx);
        }
        if layout.is_none() {
            let copy = self.load(Instance { name, props }, window, cx);
            // Popups (menus) are as tall as what's in them; `height` is the most.
            if fit
                && let Some(root) = &copy.root
            {
                root.update(cx, |r, cx| r.set_fit_content(true, cx));
            }
            self.copies.insert(slot.clone(), copy);
        }
        let first_term = layout.as_ref().and_then(|l| l.visible_blocks().into_iter().find(|n| matches!(n.content(), Some(Content::Terminal { .. }))).map(|n| n.id.clone()));
        let has_layout = layout.is_some();
        self.overlays.push(Overlay {
            id,
            kind,
            slot,
            at,
            size,
            expires: (kind == OverlayKind::Toast).then(|| Instant::now() + Duration::from_millis(timeout as u64)),
            focus,
            layout,
            top,
            fit,
            modal: spec.get("modal").and_then(Value::as_bool).unwrap_or(false),
            bare: spec.get("frame").and_then(Value::as_str) == Some("none"),
            place,
            key: key.clone(),
            frame: spec.get("mod").and_then(Value::as_str).is_some_and(|m| m.starts_with("frame-")),
        });
        if let Some(k) = &key {
            if spec.get("keep").and_then(Value::as_bool) == Some(true) {
                movable::keep(&self.layout_name, k, &spec);
            } else {
                movable::forget(&self.layout_name, k);
            }
        }
        if has_layout {
            self.sync(window, cx);
            if kind.modal_keys()
                && let Some(t) = first_term.and_then(|b| self.terms.get(&b))
            {
                t.focus.focus(window, cx);
            }
        } else {
            self.publish_shown_copies();
        }
        cx.notify();
    }

    fn peek_terminal(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.close_overlay("terminal-peek", window, cx);
        self.open_overlay("terminal-peek".into(), json!({
            "kind": "dialog",
            "width": 0.8,
            "height": 0.8,
            "layout": {
                "type": "column",
                "children": [
                    { "type": "block", "size": { "px": 34 }, "content": { "mod": "peek", "props": { "terminal": address } } },
                    { "type": "block", "content": { "terminal": address } }
                ]
            }
        }), window, cx);
    }

    /// Which copies are on screen, for `<mod>.<command>` (`mods::send_command`).
    fn publish_shown_copies(&self) {
        let (_, shown) = self.visible();
        mods::set_shown_copies(shown.iter().filter_map(|(s, i)| self.copies.get(s).filter(|c| c.inst == *i)).map(|c| c.id).collect());
    }

    /// Closes an overlay; a kept window stays closed on the next start too.
    fn close_overlay(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(k) = self.overlays.iter().find(|o| o.id == id).and_then(|o| o.key.clone()) {
            movable::forget(&self.layout_name, &k);
        }
        self.remove_overlay(id, window, cx);
    }

    fn remove_overlay(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.overlays.iter().position(|o| o.id == id) {
            let o = self.overlays.remove(i);
            if self.win_drag.as_ref().is_some_and(|d| d.id == o.id) {
                self.win_drag = None;
            }
            self.bounds.borrow_mut().remove(&o.slot);
            self.remove_copy(&o.slot, window, cx);
            if o.layout.is_some() {
                self.sync(window, cx);
            } else {
                self.publish_shown_copies();
            }
            // The keyboard never ends up nowhere: back to a dialog still open,
            // else the pane, else the window (where the app's keys work). A
            // window in the background had none to give back (one that had it:
            // `restore_focus` runs on its own when its focus goes).
            if o.kind.modal_keys() {
                self.restore_focus(window, cx);
            }
            cx.notify();
        }
    }

    /// The escape hatch: set the files aside and go back to the default screen.
    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = format!("layouts/{}.json", self.layout_name);
        for name in [layout.as_str(), "keys.json"] {
            if file(name).exists() {
                let _ = std::fs::rename(file(name), aside(&file(name), "reset"));
            }
        }
        let _ = std::fs::remove_file(state_path(&self.layout_name));
        tracing::warn!("escape hatch: {layout} and keys.json set aside as *.reset-<time>");
        self.watched.keys = None;
        install_layouts();
        self.drop_layout_copies(window, cx);
        self.load_layout("main");
        self.apply_look(window, cx);
        self.focused_pane = None;
        self.agent_focused = None;
        let ids: Vec<String> = self.overlays.iter().map(|o| o.id.clone()).collect();
        for id in ids {
            self.close_overlay(&id, window, cx);
        }
        self.show_layout_notes(window, cx);
        self.reload_config(window, cx);
    }

    fn handle(&mut self, req: Request, window: &mut Window, cx: &mut Context<Self>) {
        match req {
            Request::Run { name, args } => self.run(&name, args, window, cx),
            Request::OpenOverlay { id, spec } => self.open_overlay(id, spec, window, cx),
            Request::CloseOverlay { id } => self.close_overlay(&id, window, cx),
            Request::Refresh => self.refresh_mods(cx),
            Request::Control { cmd, reply } => {
                let _ = reply.send(self.control(cmd, window, cx));
            }
        }
    }

    /// `app.toggle`: the quick terminal drops down or goes up; else the app
    /// hides or comes to the front.
    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if crate::quick::on() {
            crate::quick::toggle(cx);
        } else if window.is_window_active() {
            // Only macOS hides an app; elsewhere `hide` does nothing.
            if cfg!(target_os = "macos") {
                cx.hide();
            } else {
                window.minimize_window();
            }
        } else {
            cx.activate(true);
            window.activate_window();
        }
    }

    fn control(&mut self, cmd: Value, window: &mut Window, cx: &mut Context<Self>) -> Value {
        if let Some(name) = cmd.get("do").and_then(Value::as_str) {
            let args = cmd.get("args").cloned().unwrap_or(Value::Null);
            return self.run_as(name, args, false, window, cx);
        }
        if let Some(path) = cmd.get("shot").and_then(Value::as_str) {
            return match crate::control::save_shot(window, std::path::Path::new(path)) {
                Ok(()) => json!({ "path": path }),
                Err(error) => json!({ "error": format!("cannot save screenshot: {error:#}") }),
            };
        }
        if cmd.get("commands").is_some() {
            return crate::commands::listing();
        }
        // Herdr was launched again: this window comes to the front instead.
        if cmd.get("raise").is_some() {
            if crate::quick::on() {
                crate::quick::show(cx);
            } else {
                cx.activate(true);
                window.activate_window();
            }
            return json!({ "ok": true });
        }
        if cmd.get("ls").is_some() {
            return self.world();
        }
        if cmd.get("status").is_some() {
            let round = |v: f32| (v as f64 * 100.0).round() / 100.0;
            let errors: Vec<String> = ["settings", "keys", "layout", "programs"]
                .iter()
                .filter_map(|w| std::fs::read_to_string(file(&format!("{w}.error"))).ok())
                .chain(self.copies.values().filter_map(|c| Some(format!("mod {}: {}", c.inst.name, c.error.as_ref()?))))
                .chain(self.documents.iter().filter_map(|(m, s)| s.error.as_ref().map(|e| format!("layout {}: {e}", if m.is_empty() { "local" } else { m }))))
                .collect();
            return json!({
                "zoom": round(settings::zoom()),
                "font_size": round(settings::font_size()),
                "ui_font_size": round(settings::ui_font_size()),
                "space": self.model.active,
                "tab": self.model.active_tab().map(|c| c.id.clone()),
                "focused_pane": self.focused_pane,
                "layout": self.layout_name,
                "agent": self.shown_agent(),
                "floats": self.float_status(),
                "overlays": self.overlays.iter().map(|o| match &o.layout {
                    Some(_) => Some("layout".to_owned()),
                    None => self.copies.get(&o.slot).map(|c| c.inst.name.clone()),
                }).collect::<Vec<_>>(),
                "errors": errors,
            });
        }
        json!({ "error": "expected { do }, { ls }, { commands } or { status }" })
    }

    /// Spaces, their tabs and terminals, for `herdr-app ls`. Sub-agents are listed
    /// under the terminal that started them.
    fn world(&self) -> Value {
        let m = &self.model;
        let children = |addr: &str| -> Vec<String> {
            let mut out: Vec<String> = self
                .live
                .iter()
                .filter(|(_, r)| r.get("parent").and_then(Value::as_str) == Some(addr))
                .map(|(a, _)| a.clone())
                .collect();
            out.sort();
            out
        };
        let terminal = |pane: &str, addr: &str| {
            let rec = self.live.get(addr);
            json!({
                "terminal": addr, "pane": pane, "status": m.status(addr),
                "app": m.agents.get(addr).and_then(|a| a.app.clone()),
                "name": rec.and_then(|r| r.get("name")).cloned(),
                "sub_agents": children(addr),
            })
        };
        let spaces: Vec<Value> = m
            .spaces
            .iter()
            .map(|p| {
                let active = p.active_tab().map(|c| c.id.clone());
                let tabs: Vec<Value> = p
                    .tabs
                    .iter()
                    .map(|c| {
                        let terms: Vec<Value> = c.root.terminals().iter().filter_map(|n| Some(terminal(&n.id, n.terminal()?))).collect();
                        json!({ "id": c.id, "name": Model::tab_name(p, c), "active": Some(&c.id) == active.as_ref(), "settled": c.settled.is_some(), "terminals": terms })
                    })
                    .collect();
                json!({ "id": p.id, "name": m.space_name(p, &home()), "folder": p.folder, "machine": p.machine, "active": m.active.as_deref() == Some(&p.id), "tabs": tabs })
            })
            .collect();
        json!({ "spaces": spaces, "recent": m.recent })
    }

    // ---- mods on disk ----------------------------------------------------

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_documents(false, window, cx);
        let ready = Self::update_label();
        if ready != self.update_ready {
            self.update_ready = ready;
            cx.notify();
        }
        // The settings screen shows versions and progress: redraw mods when they move.
        let status = crate::update::status().to_string();
        if status != self.update_status {
            self.update_status = status;
            self.refresh_mods(cx);
        }
        if let Some(r) = &self.runtime {
            crate::bench::note_mods(&r.read_metrics());
        }
        // With file events the disk is only checked now and then, in case one
        // was missed.
        self.polls += 1;
        if self.polls.is_multiple_of(25) {
            self.observe();
        }
        if self.file_events.is_none() || self.polls.is_multiple_of(25) {
            self.reload_config(window, cx);
            self.check_layout_file(window, cx);
            self.sync_folders(window, cx);
        }
        let now = Instant::now();
        let expired: Vec<String> = self.overlays.iter().filter(|o| o.expires.is_some_and(|e| e <= now)).map(|o| o.id.clone()).collect();
        for id in expired {
            self.close_overlay(&id, window, cx);
        }
        let active = window.is_window_active();
        if active != self.window_active {
            self.window_active = active;
            if active {
                self.sync(window, cx);
            } else {
                self.looked.clear();
            }
        }
        let dark = is_dark(window.appearance());
        if dark != self.system_dark {
            self.system_dark = dark;
            self.apply_look(window, cx);
        }
    }

    /// Files people or agents edited: apply what they change.
    fn files_changed(&mut self, changed: Vec<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        for path in changed_asset_paths(&mods::app_dir(), &changed) {
            invalidate_asset(&path, window, cx);
        }
        let top = |p: &std::path::PathBuf| p.components().next().and_then(|c| c.as_os_str().to_str().map(str::to_owned)).unwrap_or_default();
        if changed.iter().any(|p| matches!(top(p).as_str(), "settings.json" | "keys.json" | "programs.json" | "themes")) {
            self.reload_config(window, cx);
        }
        // The theme showing may be the file that changed.
        if changed.iter().any(|p| top(p) == "themes") {
            self.apply_look(window, cx);
        }
        if changed.iter().any(|p| top(p) == "layouts") {
            self.check_layout_file(window, cx);
        }
        let edited: BTreeSet<String> = changed.iter().filter_map(|p| crate::files::mod_of(p)).collect();
        if edited.is_empty() {
            return;
        }
        // Copies that load now (a new folder, a fixed failed load) already run the
        // saved code: reloading them too would start them twice.
        let loaded: BTreeSet<u64> = self.copies.values().filter(|c| c.root.is_some()).map(|c| c.id).collect();
        self.sync_folders(window, cx);
        let Some(runtime) = self.runtime.clone() else { return };
        for copy in self.copies.values_mut().filter(|c| edited.contains(&c.inst.name) && loaded.contains(&c.id)) {
            let Some(root) = &copy.root else { continue };
            let error = runtime.reload_now(root, window, cx).err().map(|e| format!("{e:#}"));
            if let Some(e) = &error {
                tracing::warn!("mod reload failed: {e}");
            }
            copy.error = error;
        }
    }

    fn sync_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.runtime.is_none() {
            return;
        }
        let found: BTreeMap<String, Found> = mods::scan().into_iter().map(|f| (f.id.clone(), f)).collect();
        let edited: BTreeSet<String> = found.values().filter(|f| self.folders.get(&f.id).is_some_and(|o| o.stamp != f.stamp)).map(|f| f.id.clone()).collect();
        let added = found.keys().any(|k| !self.folders.contains_key(k));
        let removed = self.folders.keys().any(|k| !found.contains_key(k));
        let fresh: Vec<Found> = found.values().filter(|f| !self.seen.contains(&f.id)).cloned().collect();
        self.folders = found;

        // Copies that failed get another try when their folder changes; loaded ones hot-reload.
        let retry: Vec<String> = self.copies.iter().filter(|(_, c)| c.root.is_none() && (edited.contains(&c.inst.name) || added)).map(|(s, _)| s.clone()).collect();
        for slot in &retry {
            self.copies.remove(slot);
        }
        // A mod seen for the first time goes where it asked (unless the layout already shows it).
        for f in fresh {
            if shipped_layouts().contains(&self.layout_name.as_str()) && f.hint.place.as_deref().unwrap_or("right") != "none" {
                continue;
            }
            self.seen.insert(f.id.clone());
            write_atomic(&file("seen.json"), &serde_json::to_string(&self.seen).unwrap_or_default());
            let shown = self.chrome.walk().iter().any(|n| matches!(n.content(), Some(Content::Mod { name, .. }) if *name == f.id));
            let place = f.hint.place.as_deref().unwrap_or("right");
            if !shown && place != "none" && !shipped_layouts().contains(&self.layout_name.as_str()) {
                let mut block = Node::block(self.model.fresh_id("b"), Content::Mod { name: f.id.clone(), props: Value::Null });
                block.size = Some(Span::Px { px: f.hint.size.unwrap_or(320.0) });
                dock(&mut self.chrome, block, place, self.model.fresh_id("r"));
            }
        }
        if !retry.is_empty() || added || removed {
            self.sync(window, cx);
        }
    }

    // ---- rendering ---------------------------------------------------------

    fn focused_path(&self) -> HashSet<String> {
        let mut out = HashSet::new();
        if let Some(p) = &self.focused_pane {
            if let Some(c) = self.model.active_tab()
                && let Some(path) = c.root.path_to(p)
            {
                out.extend(path);
            }
            out.insert(p.clone());
        }
        out
    }

    /// The shown tab block's pane frame, if its panes are framed now: when
    /// the tab is split, or always with `lone`.
    fn pane_frame(&self) -> Option<&PaneFrame> {
        let frame = self.chrome.visible_blocks().into_iter().find_map(|n| match n.content() {
            Some(Content::Context { pane, .. }) => pane.as_ref(),
            _ => None,
        })?;
        let ctx = self.model.active_tab()?;
        (frame.lone || ctx.root.panes().len() > 1).then_some(frame)
    }

    /// A pane in its frame: the look, and the header mod above or below it
    /// (on hover: drawn over the terminal, so it never resizes).
    fn frame_pane(&self, n: &Node, f: &PaneFrame, on: bool, inner: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let line = if on { f.focus.as_ref().or(f.border.as_ref()) } else { f.border.as_ref() };
        let group = SharedString::from(format!("pane-{}", n.id));
        let mut el = div().size_full().relative().flex().flex_col().overflow_hidden().group(group.clone());
        if let Some(r) = f.radius {
            el = el.rounded(settings::z(r));
        }
        if let Some(bg) = &f.background {
            el = paint(el, bg);
        }
        if let Some(l) = line {
            el = el.border(settings::z(l.width)).border_color(c(&l.color));
        }
        el = shadow(el, f.shadow.as_deref());
        let body = div().flex_1().min_h_0().flex().child(inner);
        let Some(h) = &f.header else { return el.child(body).into_any_element() };
        let head = div().flex_none().h(settings::z(h.height.unwrap_or(HEADER_HEIGHT))).w_full().flex()
            .child(self.render_slot(&format!("{}#frame", n.id), cx));
        if h.hover() {
            let head = head.absolute().left_0().right_0().opacity(0.).group_hover(group, |s| s.opacity(1.));
            let head = if h.bottom() { head.bottom_0() } else { head.top_0() };
            return el.child(body).child(head).into_any_element();
        }
        if h.bottom() { el.child(body).child(head) } else { el.child(head).child(body) }.into_any_element()
    }

    /// `frame`: how panes are wrapped here (inside a framed tab), else None.
    fn render_node(&self, n: &Node, focused: &HashSet<String>, frame: Option<&PaneFrame>, cx: &mut Context<Self>) -> AnyElement {
        let mut el = div().relative().overflow_hidden().size_full().flex();
        let bounds_map = self.bounds.clone();
        let id = n.id.clone();
        el = el.child(canvas(move |b, _, _| { bounds_map.borrow_mut().insert(id, b); }, |_, _, _, _| {}).absolute().size_full());
        el = apply_look(el, n, focused.contains(&n.id));
        if n.window_drag {
            el = self.window_drag(el, cx);
        }
        let el = match &n.kind {
            Kind::Block { content } => {
                let inner = self.render_block(n, content, focused, cx);
                match frame.filter(|_| n.is_pane()) {
                    Some(f) => el.child(self.frame_pane(n, f, focused.contains(&n.id), inner, cx)),
                    None => el.child(inner),
                }
            }
            Kind::Row { children } | Kind::Column { children } => {
                let axis = n.axis().unwrap();
                el = if axis == Axis::Row { el.flex_row() } else { el.flex_col() };
                let shown: Vec<&Node> = children.iter().filter(|c| !c.hidden).collect();
                let gap = n.gap.or(frame.and_then(|f| f.gap)).unwrap_or(0.0);
                // Weights are shares of what the fixed-size siblings leave, so they always fill it.
                let total: f32 = shown
                    .iter()
                    .filter_map(|c| match c.size {
                        Some(Span::Px { .. }) => None,
                        Some(Span::Weight { weight }) => Some(weight),
                        None => Some(1.0),
                    })
                    .sum::<f32>()
                    .max(f32::EPSILON);
                for (i, child) in shown.iter().enumerate() {
                    if i > 0 {
                        el = el.child(self.separator(n, axis, gap, i - 1, cx));
                    }
                    // No clipping here: a child's shadow may fall outside it (the child clips itself).
                    let mut slot = div().relative().flex().min_w_0().min_h_0();
                    slot = sized(slot, child, axis, total);
                    el = el.child(slot.child(self.render_node(child, focused, frame, cx)));
                }
                el
            }
            Kind::Stack { header, .. } => {
                el = el.flex_col();
                let head = header.as_ref().map(|h| {
                    let height = h.height.unwrap_or(HEADER_HEIGHT);
                    let head: AnyElement = match &h.content {
                        HeaderContent::Mod { .. } => self.render_slot(&format!("{}#header", n.id), cx),
                        HeaderContent::Builtin(_) => self.builtin_tabs(n, cx),
                    };
                    (h.bottom(), div().flex_none().h(settings::z(height)).w_full().flex().child(head))
                });
                let body = match n.active_child() {
                    Some(child) => self.render_node(child, focused, frame, cx),
                    None => div().into_any_element(),
                };
                let body = div().flex_1().min_h_0().flex().child(body);
                match head {
                    Some((true, head)) => el.child(body).child(head),
                    Some((false, head)) => el.child(head).child(body),
                    None => el.child(body),
                }
            }
        };
        let row = frame.and_then(|f| f.header.as_ref()).filter(|h| !h.bottom() && !h.hover()).map(|h| h.height.unwrap_or(HEADER_HEIGHT));
        el.children(self.render_floats(n, cx)).children(self.pane_grip(&n.id, row, cx)).into_any_element()
    }

    fn render_block(&self, n: &Node, content: &Content, focused: &HashSet<String>, cx: &mut Context<Self>) -> AnyElement {
        match content {
            Content::Terminal { .. } => match self.pane_term(&n.id) {
                Some(t) => {
                    let id = n.id.clone();
                    div()
                        .size_full()
                        .on_any_mouse_down(cx.listener(move |this, _, _, cx| {
                            // Overlay terminals keep their own focus; chrome blocks share the pane focus API.
                            if this.focused_pane.as_deref() != Some(id.as_str())
                                && (this.model.tab_of(&id).is_some() || terminal_block(&this.chrome, Some(&id)).is_some())
                            {
                                this.model.focus_pane(&id);
                                this.focused_pane = Some(id.clone());
                                this.synced_model_pane = this.model.focused_pane();
                                this.publish(cx);
                                this.save();
                                cx.notify();
                            }
                        }))
                        .child(shown_terminal(&t.view, cx))
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
            Content::Mod { .. } => self.render_slot(&n.id, cx),
            Content::Agent { .. } => match self.shown_agent() {
                Some(addr) => match self.agent_view(&n.id, &addr) {
                    Some(t) => div().size_full().child(shown_terminal(&t.view, cx)).into_any_element(),
                    None => div().into_any_element(),
                },
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(c("dim"))
                    .child("no agents yet. start claude, codex or pi in a terminal")
                    .into_any_element(),
            },
            Content::Context { .. } => match self.model.active_tab() {
                Some(c) => self.render_node(c.shown(), focused, self.pane_frame(), cx),
                None if self.documents.get("").is_some_and(|s| s.layout() == "loading") => div().size_full().into_any_element(),
                None => {
                    let key = crate::keys::listing().into_iter().find(|(_, _, a)| a == "space.new").map(|(_, k, _)| format!(" ({k})"));
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(c("dim"))
                        .child(format!("No space open. Make one{}.", key.unwrap_or_default()))
                        .into_any_element()
                }
            },
        }
    }

    fn render_slot(&self, slot: &str, cx: &mut Context<Self>) -> AnyElement {
        let in_overlay = slot.starts_with("overlay:") || self.overlays.iter().any(|o| o.layout.as_ref().is_some_and(|l| l.find(slot).is_some()));
        // A mod that is a pane in a tab: a click makes it the focused pane, and the
        // keyboard is its own (an editor, a board), like a terminal's.
        let in_pane = self.model.tab_of(slot).is_some();
        match self.copies.get(slot) {
            Some(Copy { root: Some(root), inst, .. }) => div()
                .size_full()
                .key_context(format!("Mod mod = {}", inst.name).as_str())
                .when(in_pane, |d| {
                    let id = slot.to_owned();
                    d.on_any_mouse_down(cx.listener(move |this, _, _, cx| {
                        if this.focused_pane.as_deref() != Some(id.as_str()) {
                            this.model.focus_pane(&id);
                            this.focused_pane = Some(id.clone());
                            this.publish(cx);
                            this.save();
                            cx.notify();
                        }
                    }))
                })
                // Bare layout clicks leave the keyboard with the pane, but an input
                // or other focusable control owns the keyboard once it takes focus.
                .when(!in_overlay && !in_pane, |d| {
                    d.capture_any_mouse_down(cx.listener(|_, _, window, cx| {
                        cx.defer_in(window, |this, window, cx| {
                            let dialog = this.overlays.iter().any(|o| o.kind.modal_keys());
                            if !dialog
                                && (window.focused(cx).is_none() || this.focus.is_focused(window))
                                && let Some(f) = this.focused_pane.as_ref().and_then(|p| this.pane_focus(p))
                                && !f.is_focused(window)
                            {
                                f.focus(window, cx);
                            }
                        });
                    }))
                })
                // Popups size to their content, up to the window: a column, so a mod
                // root with `min_h(0)` can shrink and scroll. Layout mods fill their block.
                // Not cached here: ShellRoot caches the mod's view itself and draws its
                // spinners over it, waking only itself (GPUI rebuilds all under a cached
                // view that changed, so a spinner step would rebuild the mod).
                .map(|d| if in_overlay { d.h_auto().min_h_0().flex().flex_col().child(root.clone()) } else { d.child(root.clone()) })
                .into_any_element(),
            Some(Copy { error, inst, .. }) => div()
                .id(SharedString::from(format!("err-{slot}")))
                .size_full()
                .p_2()
                .text_xs()
                .text_color(c("blocked"))
                .overflow_y_scroll()
                .child(format!("{} did not load:\n\n{}\n\nFix the files; Herdr retries on save.", inst.name, error.as_deref().unwrap_or("?")))
                .into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// Plain fallback for `"header": { "content": "tabs" }`.
    fn builtin_tabs(&self, n: &Node, cx: &mut Context<Self>) -> AnyElement {
        let active = n.active_child().map(|c| c.id.clone());
        let stack = n.id.clone();
        let mut row = div().size_full().flex().flex_row().bg(c("surface")).text_xs();
        for (i, ch) in n.children().iter().enumerate() {
            let (s, id) = (stack.clone(), ch.id.clone());
            let on = active.as_deref() == Some(ch.id.as_str());
            row = row.child(
                div()
                    .id(SharedString::from(format!("tab-{}", ch.id)))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_color(if on { c("foreground") } else { c("muted") })
                    .when(on, |d| d.bg(c("raised")))
                    .on_click(cx.listener(move |this, _, w, cx| this.run("layout.select", json!({ "stack": s, "child": id }), w, cx)))
                    .child(ch.title.clone().unwrap_or_else(|| (i + 1).to_string())),
            );
        }
        row.into_any_element()
    }

    fn separator(&self, n: &Node, axis: Axis, gap: f32, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let row = axis == Axis::Row;
        let zoom = settings::zoom();
        let line = n.divider.as_ref().map(|d| (d.width * zoom, c(&d.color)));
        let width = gap * zoom + line.map_or(0.0, |l| l.0);
        let mut sep = div().flex_none().relative().flex().items_center().justify_center();
        sep = if row { sep.w(px(width)).h_full() } else { sep.h(px(width)).w_full() };
        if let Some((w, color)) = line {
            let top = traffic_light_divider_inset(n, axis, index, cfg!(target_os = "macos"), zoom) * zoom;
            if top > 0.0
                && let Some(Paint::Color(background)) = n.children().iter().filter(|child| !child.hidden).nth(index).and_then(|child| child.background.as_ref())
            {
                sep = sep.bg(c(background));
            }
            sep = sep.child(separator_line(row, w, color, top));
        }
        if n.resizable() {
            let container = n.id.clone();
            let hit = HANDLE.max(width);
            sep = sep.child(
                div()
                    .absolute()
                    .when(row, |d| d.top_0().bottom_0().left(px((width - hit) / 2.0)).w(px(hit)).cursor_col_resize())
                    .when(!row, |d| d.left_0().right_0().top(px((width - hit) / 2.0)).h(px(hit)).cursor_row_resize())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            if let Some(space) = this.model.space_of(&container)
                                && this.require_document(space.machine.as_deref()).is_err() { return; }
                            this.drag = Some(Drag { container: container.clone(), index, axis });
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        sep.into_any_element()
    }

    fn drag_to(&mut self, pos: Point<Pixels>) {
        let Some(d) = &self.drag else { return };
        let (container, index, axis) = (d.container.clone(), d.index, d.axis);
        let Some(node) = self.find_node_mut(&container).map(|n| n.clone()) else { return };
        let shown: Vec<&Node> = node.children().iter().filter(|c| !c.hidden).collect();
        let (Some(a), Some(b)) = (shown.get(index), shown.get(index + 1)) else { return };
        let bounds = self.bounds.borrow();
        let (Some(ba), Some(bb)) = (bounds.get(&a.id).copied(), bounds.get(&b.id).copied()) else { return };
        drop(bounds);
        let (start, end, p) = match axis {
            Axis::Row => (f32::from(ba.left()), f32::from(bb.right()), f32::from(pos.x)),
            Axis::Column => (f32::from(ba.top()), f32::from(bb.bottom()), f32::from(pos.y)),
        };
        let (la, lb) = match axis {
            Axis::Row => (f32::from(ba.size.width), f32::from(bb.size.width)),
            Axis::Column => (f32::from(ba.size.height), f32::from(bb.size.height)),
        };
        // Measured on screen; sizes are stored as designed (unzoomed).
        let zoom = settings::zoom();
        let (la, lb, start, end, p) = (la / zoom, lb / zoom, start / zoom, end / zoom, p / zoom);
        let total = la + lb;
        let mut new_a = (p - start).clamp(24.0, (end - start) - 24.0);
        if let Some(min) = a.min {
            new_a = new_a.max(min);
        }
        if let Some(max) = a.max {
            new_a = new_a.min(max);
        }
        let new_a = new_a.min(total - 24.0);
        let (a_id, b_id) = (a.id.clone(), b.id.clone());
        let (sa, sb) = (a.size, b.size);
        let Some(parent) = self.find_node_mut(&container) else { return };
        let Some(children) = parent.children_mut() else { return };
        let set = |children: &mut Vec<Node>, id: &str, s: Span| {
            if let Some(n) = children.iter_mut().find(|n| n.id == id) {
                n.size = Some(s);
            }
        };
        match (sa, sb) {
            (Some(Span::Px { .. }), _) => set(children, &a_id, Span::Px { px: new_a.round() }),
            (_, Some(Span::Px { .. })) => set(children, &b_id, Span::Px { px: (total - new_a).round() }),
            _ => {
                let wa = match sa { Some(Span::Weight { weight }) => weight, _ => 1.0 };
                let wb = match sb { Some(Span::Weight { weight }) => weight, _ => 1.0 };
                let sum = wa + wb;
                set(children, &a_id, Span::Weight { weight: sum * new_a / total });
                set(children, &b_id, Span::Weight { weight: sum * (total - new_a) / total });
            }
        }
    }

    fn render_overlays(&self, viewport: Size<Pixels>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let mut toast_y = 16.0;
        // Drawn last, so it stays clickable over an open dialog.
        let pill = self.render_update_pill(cx);
        if pill.is_some() {
            toast_y += UPDATE_PILL_HEIGHT + 8.0;
        }
        // Windows under dialogs, popups and toasts; each kind in the order opened
        // (a window pressed last is last: on top).
        let order = self.overlays.iter().filter(|o| o.kind == OverlayKind::Window).chain(self.overlays.iter().filter(|o| o.kind != OverlayKind::Window));
        for o in order {
            let id = o.id.clone();
            // Up to 1: a share of the window (terminals in overlays want room, not px).
            let span = |v: f32, full: Pixels| if v <= 1.0 { full * v } else { settings::z(v) };
            let (w, h) = (span(o.size.width, viewport.width), span(o.size.height, viewport.height));
            let sized = |v: f32| v > 0.0 || o.kind != OverlayKind::Window;
            let content = match &o.layout {
                Some(l) => self.render_node(l, &HashSet::new(), None, cx),
                None => self.render_slot(&o.slot, cx),
            };
            let body = div()
                .id(SharedString::from(format!("ov-{}", o.id)))
                .track_focus(&o.focus)
                // Esc stays with terminals (agents use it to interrupt) and windows.
                .when(o.layout.is_none() && o.kind.modal_keys(), |d| d.key_context("Overlay"))
                .when(sized(o.size.width), |d| d.w(w))
                .when(sized(o.size.height), |d| if o.fit { d.max_h(h) } else { d.h(h) })
                .when(!o.bare, |d| d.bg(c("popover")).border_1().border_color(c("border")).rounded(settings::z(8.0)).shadow_lg())
                .overflow_hidden()
                .flex()
                // Scroll, hover and cursor stop here, not on the pane behind.
                .occlude()
                .when(o.kind != OverlayKind::Window, |d| d.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
                .child(content);
            match o.kind {
                OverlayKind::Window => out.push(self.render_window(o, body, viewport, cx)),
                OverlayKind::Dialog => out.push(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(hsla(0.0, 0.0, 0.0, 0.35))
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(!o.modal, |d| d.on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| this.close_overlay(&id, w, cx))))
                        .child(body)
                        .into_any_element(),
                ),
                OverlayKind::Popup => {
                    let at = match o.top {
                        // Centered, hanging from `offset` px below the top (a drop-down from a bar).
                        Some(off) => point((viewport.width - w) / 2.0, settings::z(off)),
                        None => o.at,
                    };
                    out.push(
                        div()
                            .absolute()
                            .inset_0()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, w, cx| this.close_overlay(&id, w, cx)))
                            .on_mouse_down(MouseButton::Right, cx.listener({
                                let id = o.id.clone();
                                move |this, _, w, cx| this.close_overlay(&id, w, cx)
                            }))
                            .child(position_popup(at, body))
                            .into_any_element(),
                    );
                }
                OverlayKind::Toast => {
                    out.push(div().absolute().right(settings::z(16.0)).bottom(settings::z(toast_y)).child(body).into_any_element());
                    toast_y += o.size.height + 8.0;
                }
            }
        }
        if let Some(error) = crate::update::core().error {
            out.push(div().absolute().right(settings::z(16.0)).bottom(settings::z(toast_y))
                .max_w(settings::z(540.0)).p(settings::z(12.0)).rounded_lg().bg(c("popover"))
                .border_1().border_color(c("border")).text_size(settings::z(13.0)).text_color(c("foreground"))
                .id("core-update-error").cursor_pointer()
                .on_click(cx.listener(|this, _, w, cx| this.run("app.settings", json!({ "section": "updates" }), w, cx)))
                .child(format!("herdr-core update failed: {error} · Open Updates")).into_any_element());
        }
        out.extend(pill);
        out
    }
}

const UPDATE_PILL_HEIGHT: f32 = 34.0;

impl AppWindow {
    /// "Herdr 0.2.0 is ready · Restart": the frame's, so no layout or mod can hide it.
    /// What's waiting, in words: the pill shows it until clicked or dismissed.
    fn update_label() -> Option<String> {
        let core = crate::update::core();
        let core = core.ready.filter(|_| !core.updating);
        match (crate::update::ready(), core) {
            (Some(a), Some(c)) => Some(format!("Herdr {a} and herdr-core {c} are ready")),
            (Some(a), None) => Some(format!("Herdr {a} is ready")),
            (None, Some(c)) => Some(format!("herdr-core {c} is ready")),
            (None, None) => None,
        }
    }

    fn render_update_pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let app_version = crate::update::ready();
        let app_ready = app_version.is_some();
        let success = self.update_ready.is_none() && self.update_success;
        let label = self.update_ready.clone().or_else(|| success.then(|| format!("Updated to {}", env!("CARGO_PKG_VERSION"))))?;
        let dismissal = app_version.map(|v| format!("app:{v}")).unwrap_or_else(|| label.clone());
        if self.update_dismissed.as_ref() == Some(&dismissal) { return None; }
        let pill = div()
            .absolute()
            .right(settings::z(16.0))
            .bottom(settings::z(16.0))
            .h(settings::z(UPDATE_PILL_HEIGHT))
            .pl(settings::z(14.0))
            .pr(settings::z(6.0))
            .flex()
            .items_center()
            .gap(settings::z(10.0))
            .bg(c("popover"))
            .border_1()
            .border_color(c("border"))
            .rounded_full()
            .shadow_lg()
            .occlude()
            .text_size(settings::z(13.0))
            .child(div().text_color(c("foreground")).child(label.clone()))
            .when(app_ready, |pill| pill.child(
                div()
                    .id("update-restart")
                    .h(settings::z(24.0))
                    .px(settings::z(10.0))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(c("accent"))
                    .text_color(c("accent_foreground"))
                    .font_weight(FontWeight::SEMIBOLD)
                    .hover(|s| s.opacity(0.9))
                    .child("Restart")
                    .on_click(cx.listener(|this, _, w, cx| this.run("app.update", json!({}), w, cx))),
            ))
            .child(div().id("update-notes").px(settings::z(6.0)).cursor_pointer().text_color(c("accent"))
                .child(if app_ready || success { "What changed" } else { "Open Updates" })
                .on_click(cx.listener(move |this, _, w, cx| {
                    if success { this.update_success = false; }
                    this.run("app.settings", json!({ "section": "updates" }), w, cx);
                })))
            .child(
                div()
                    .id("update-dismiss")
                    .size(settings::z(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .text_color(c("dim"))
                    .hover(|s| s.bg(c("raised")).text_color(c("foreground")))
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.update_dismissed = Some(dismissal.clone());
                        cx.notify();
                    })),
            );
        Some(pill.into_any_element())
    }
}

/// A view drawn again only when it changed: a terminal's output then
/// redraws that terminal, not every mod and pane in the window.
pub(crate) fn cached<V: Render>(view: &Entity<V>) -> AnyElement {
    view.clone().cached(StyleRefinement::default().size_full()).into_any_element()
}

pub(crate) fn shown_terminal(view: &Entity<TerminalView>, cx: &App) -> AnyElement {
    view.read(cx).mark_shown();
    cached(view)
}

/// How people call an agent, from its 7501 `app`.
fn agent_name(app: &str) -> String {
    match app {
        "claude" | "claude-code" => "Claude Code".into(),
        "codex" => "Codex".into(),
        "pi" => "Pi".into(),
        "pig" => "PiG".into(),
        "opencode" => "OpenCode".into(),
        "cursor" | "cursor-agent" => "Cursor".into(),
        "amp" => "Amp".into(),
        "grok" => "Grok".into(),
        other => other.to_owned(),
    }
}

fn position_popup(at: Point<Pixels>, body: Stateful<Div>) -> Anchored {
    anchored()
        .position(point(at.x.max(px(8.0)), at.y.max(px(8.0))))
        .snap_to_window_with_margin(px(8.0))
        .child(body)
}

/// An overlay's blocks get ids of their own (`<overlay>/<id>`), and its mods learn the overlay.
fn overlay_layout(mut n: Node, overlay: &str) -> Node {
    fn walk(n: &mut Node, overlay: &str, seq: &mut usize) {
        *seq += 1;
        n.id = if n.id.is_empty() { format!("ov{overlay}/{seq}") } else { format!("ov{overlay}/{}", n.id) };
        if let Kind::Block { content: Content::Mod { props, .. } } = &mut n.kind {
            if !props.is_object() {
                *props = json!({});
            }
            props["overlay"] = json!(overlay);
        }
        for c in n.children_mut().into_iter().flatten() {
            walk(c, overlay, seq);
        }
    }
    walk(&mut n, overlay, &mut 0);
    n
}

/// What a terminal's folder last said about its repo, and what ran there then.
struct FolderCheck {
    at: Instant,
    cwd: String,
    busy: bool,
    /// A repo, or git couldn't say (a machine unreachable): worth a look again.
    repo: bool,
}

/// Changes made outside Herdr (a commit in another app): looked for this often,
/// in repos, while the window is in front.
const FOLDER_FALLBACK: Duration = Duration::from_secs(60);

/// An agent working, or a program other than the shell in front (an agent
/// between turns isn't, even started from a shell).
fn busy(rec: &Value) -> bool {
    let program = |k: &str| rec["program"][k].as_bool() == Some(true);
    rec["status"] == "working" || !(program("shell") || program("agent"))
}

/// Whether to ask a terminal's folder for its repo and branch again: only when
/// the answer could have changed. It moved, a command or an agent's turn just
/// ended there (a checkout, a commit), or a repo's fallback is due.
fn recheck(last: Option<&FolderCheck>, cwd: &str, busy: bool, active: bool) -> bool {
    let Some(last) = last else { return true };
    last.cwd != cwd || (last.busy && !busy) || (active && last.repo && last.at.elapsed() > FOLDER_FALLBACK)
}

/// Set once the app starts quitting, the OS ending the session included: the
/// saved layout stays as it was.
pub static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The repo a folder is in (a worktree counts as its main checkout's), else the
/// folder itself, and the branch; and whether it is a repo (or git couldn't say).
fn folder_info(cwd: &str, home: &str, on: Option<&str>) -> (model::FolderInfo, bool) {
    let none = model::FolderInfo { top: cwd.into(), branch: None };
    if on.is_none() && cwd == home {
        return (none, false);
    }
    let args = ["-C", cwd, "rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"].map(str::to_owned);
    let ran = match on {
        Some(m) => mods::run_remote(m, "git", &args, None).ok().map(|(code, out, err)| (code == 0, out, err)),
        None => crate::spawn::command("git").args(&args).output().ok().map(|o| (o.status.success(), o.stdout, o.stderr)),
    };
    let Some((true, out, _)) = ran else {
        // Only git's own "no" means no repo; a failed run, a repo without commits
        // or one git won't read are asked again.
        let no = ran.is_some_and(|(_, _, err)| String::from_utf8_lossy(&err).to_lowercase().contains("not a git repository"));
        return (none, !no);
    };
    let out: Vec<String> = String::from_utf8_lossy(&out).lines().map(str::to_owned).collect();
    let [top, head] = out.as_slice() else { return (none, true) };
    let branch = (head != "HEAD").then(|| head.clone());
    (model::FolderInfo { top: top.clone(), branch }, true)
}

fn terminal_block<'a>(chrome: &'a Node, focused: Option<&str>) -> Option<&'a Node> {
    let id = focused?;
    chrome.visible_blocks().into_iter().find(|n| n.id == id && n.terminal().is_some())
}

fn needs_model_pane(name: &str, args: &Value) -> bool {
    match name {
        "pane.split" | "pane.close" | "pane.zoom" | "terminal.show" => AppWindow::arg(args, "pane").is_none(),
        "pane.go" | "pane.swap" => true,
        "terminal.open" | "terminal.keep" => matches!(AppWindow::arg(args, "place"), Some("right" | "down")),
        _ => false,
    }
}

fn home() -> String {
    mods::home().to_string_lossy().into_owned()
}

fn mod_overlay(dir: &std::path::Path) -> Value {
    let meta: Value = std::fs::read_to_string(dir.join("mod.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    meta.get("overlay").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}))
}

fn terminal_menu_spec(name: &str, defaults: &Value, tab: &str, target: Value) -> Value {
    json!({ "mod": name, "kind": "popup",
        "width": defaults.get("width").and_then(Value::as_f64).unwrap_or(220.0),
        "height": defaults.get("height").and_then(Value::as_f64).unwrap_or(260.0),
        "props": { "tab": tab, "target": target } })
}

fn num_in(v: &Value, k: &str) -> f32 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32
}

/// A floating mod's slot and copy (it's told its float id). `visible` and
/// `owned_mods` must build the same, or a kept copy is dropped.
fn float_mod(id: &str, name: &str, props: &Value) -> (String, Instance) {
    let mut props = if props.is_object() { props.clone() } else { json!({}) };
    props["float"] = json!(id);
    (floats::slot(id), Instance { name: name.to_owned(), props })
}

/// A pane header's slot and copy, told which pane it sits on (a mod pane has no terminal).
fn pane_header_mod(n: &Node, name: &str, props: &Value) -> (String, Instance) {
    let mut props = if props.is_object() { props.clone() } else { json!({}) };
    props["pane"] = json!(n.id);
    props["terminal"] = json!(n.terminal());
    (format!("{}#frame", n.id), Instance { name: name.to_owned(), props })
}

/// Every node shown on screen (hidden ones and inactive stack children skipped).
fn visible_nodes(n: &Node) -> Vec<&Node> {
    if n.hidden {
        return vec![];
    }
    let mut out = vec![n];
    match &n.kind {
        Kind::Stack { .. } => {
            if let Some(c) = n.active_child() {
                out.extend(visible_nodes(c));
            }
        }
        _ => {
            for c in n.children() {
                out.extend(visible_nodes(c));
            }
        }
    }
    out
}

fn fill_ids(n: &mut Node, m: &mut Model) {
    n.fill_ids(&mut || m.fresh_id("n"));
}

/// Puts a block along one side of the chrome.
fn dock(root: &mut Node, block: Node, place: &str, wrapper_id: String) {
    let axis = if matches!(place, "top" | "bottom") { Axis::Column } else { Axis::Row };
    let first = matches!(place, "left" | "top");
    if root.axis() == Some(axis) {
        let c = root.children_mut().unwrap();
        if first { c.insert(0, block) } else { c.push(block) }
        return;
    }
    let old = std::mem::replace(root, Node::new("", Kind::Row { children: vec![] }));
    let children = if first { vec![block, old] } else { vec![old, block] };
    let mut wrapper = Node::container(wrapper_id, axis, children);
    wrapper.divider = Some(Line { width: 1.0, color: "border".into() });
    *root = wrapper;
}

fn sized(slot: Div, n: &Node, axis: Axis, total_weight: f32) -> Div {
    let mut slot = slot;
    let row = axis == Axis::Row;
    match n.size {
        Some(Span::Px { px: v }) => {
            slot = slot.flex_none();
            slot = if row { slot.w(settings::z(v)).h_full() } else { slot.h(settings::z(v)).w_full() };
        }
        other => {
            let w = match other {
                Some(Span::Weight { weight }) => weight,
                _ => 1.0,
            };
            let s = slot.style();
            s.flex_grow = Some(w / total_weight);
            s.flex_shrink = Some(1.0);
            s.flex_basis = Some(relative(0.).into());
            slot = if row { slot.h_full() } else { slot.w_full() };
        }
    }
    if let Some(min) = n.min {
        slot = if row { slot.min_w(settings::z(min)) } else { slot.min_h(settings::z(min)) };
    }
    if let Some(max) = n.max {
        slot = if row { slot.max_w(settings::z(max)) } else { slot.max_h(settings::z(max)) };
    }
    slot
}

/// What a terminal runs now (core's `foreground`), else what it started with.
fn running_argv(rec: &serde_json::Map<String, Value>) -> Option<&Value> {
    rec.get("foreground").and_then(|f| f.get("argv")).filter(|a| a.as_array().is_some_and(|a| !a.is_empty())).or_else(|| rec.get("argv"))
}

/// Whether the core's record still puts a terminal in this space and tab and
/// it still runs. A listing taken just before it ended still shows it running,
/// and labels outlive the program: putting it back would show a dead pane.
fn adoptable(record: &Value, space: &str, tab: Option<&str>, machine: Option<&str>) -> bool {
    let label = record.pointer("/labels/herdr.space").and_then(Value::as_str);
    let labeled_tab = record.pointer("/labels/herdr.tab").and_then(Value::as_str).filter(|s| !s.is_empty());
    record["state"] == "live"
        && label == Some(crate::mirror::local(space, machine))
        && labeled_tab == tab.map(|t| crate::mirror::local(t, machine))
}

fn program_of(report: &Value, argv: Option<&Value>) -> Value {
    let app = report.get("app").and_then(Value::as_str);
    let argv: Vec<String> = argv.and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.as_str().map(str::to_owned)).collect();
    crate::programs::of(app, &argv)
}

fn apply_look(el: Div, n: &Node, focused: bool) -> Div {
    let mut el = el;
    let focus = n.focus.as_ref().filter(|_| focused);
    if let Some(bg) = focus.and_then(|f| f.background.as_ref()) {
        el = el.bg(c(bg));
    } else if let Some(bg) = &n.background {
        el = paint(el, bg);
    }
    if let Some(line) = focus.and_then(|f| f.border.as_ref()).or(n.border.as_ref()) {
        el = el.border(settings::z(line.width)).border_color(c(&line.color));
    }
    if let Some(r) = n.radius {
        el = el.rounded(settings::z(r));
    }
    if let Some(p) = n.padding {
        let (y, x) = p.axes();
        el = el.py(settings::z(y)).px(settings::z(x));
    }
    shadow(el, n.shadow.as_deref())
}

/// A color or gradient paints the element; an image is a layer behind
/// whatever comes after it (call before adding content).
fn paint(el: Div, bg: &Paint) -> Div {
    match bg {
        Paint::Color(name) => el.bg(c(name)),
        Paint::Gradient { gradient: [a, b], angle } => el.bg(linear_gradient(*angle, linear_color_stop(c(a), 0.), linear_color_stop(c(b), 1.))),
        Paint::Image { image, fit } => {
            let fit = match fit {
                Fit::Cover => ObjectFit::Cover,
                Fit::Contain => ObjectFit::Contain,
            };
            el.child(img(mods::app_dir().join(image)).absolute().top_0().left_0().size_full().object_fit(fit))
        }
    }
}

fn shadow(el: Div, s: Option<&str>) -> Div {
    match s {
        Some("sm") => el.shadow_sm(),
        Some("md") => el.shadow_md(),
        Some("lg") => el.shadow_lg(),
        _ => el,
    }
}

impl Render for AppWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::bench::count(&crate::bench::WINDOW);
        let _timed = crate::bench::timed(&crate::bench::WINDOW_US);
        crate::bench::frame_start();
        crate::terminal::next_frame();
        window.set_rem_size(px(settings::ui_font_size()));
        // A terminal the person clicked into becomes the focused pane.
        let clicked = self
            .terms
            .iter()
            .find(|(_, t)| t.focus.is_focused(window))
            .map(|(id, _)| id.clone());
        if let Some(id) = clicked
            && self.focused_pane.as_deref() != Some(id.as_str())
            && (self.model.tab_of(&id).is_some() || terminal_block(&self.chrome, Some(&id)).is_some())
        {
            self.model.focus_pane(&id);
            self.focused_pane = Some(id);
            self.synced_model_pane = self.model.focused_pane();
            self.publish(cx);
        }
        self.tick_floats(window);
        self.apply_titlebar(window);
        if mods::publish_window(self.window_json(window)) {
            self.refresh_mods(cx);
        }
        let focused = self.focused_path();
        let tree = self.render_node(&self.chrome, &focused, None, cx);
        let tree = self.window_frame(tree, window, cx);
        let overlays = self.render_overlays(window.viewport_size(), cx);
        let tracker = self.float_tracker(cx);
        let win_tracker = self.window_tracker(cx);
        let pane_drag = self.pane_drag_layer(cx);
        div()
            .id("herdr")
            .track_focus(&self.focus)
            // `layout:<name>` key sections apply while that layout shows.
            .key_context(format!("Herdr layout = {}", self.layout_name).as_str())
            .size_full()
            .relative()
            .bg(c("background"))
            .text_color(c("foreground"))
            .font_family(settings::get().ui_font_family.clone())
            .on_action(cx.listener(|this, a: &ShellAction, w, cx| this.run_action(a.id(), w, cx)))
            // Ctrl (Cmd) alone shows links in the pane under the pointer, focused or not.
            .on_modifiers_changed(cx.listener(|this, _: &ModifiersChangedEvent, _, cx| {
                for t in this.terms.values() {
                    if t.view.read(cx).hovered() {
                        t.view.update(cx, |_, cx| cx.notify());
                    }
                }
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" && this.pane_drag.is_some() {
                    this.cancel_pane_drag(cx);
                    cx.stop_propagation();
                }
            }))
            .capture_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, _, _| this.pointer = ev.position))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                this.drag_window(ev, window);
                if this.drag.is_none() {
                    return;
                }
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.drag = None;
                    this.save();
                    this.publish(cx);
                    return;
                }
                this.drag_to(ev.position);
                cx.notify();
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.drag_from = None;
                if this.drag.take().is_some() {
                    this.save();
                    // Mods see the new sizes (the sidebar picks its rail by width).
                    this.publish(cx);
                    this.flush_documents(false, window, cx);
                }
            }))
            .child(tree)
            .children(overlays)
            .children(tracker)
            .children(win_tracker)
            .child(pane_drag)
            .child(crate::bench::frame_end())
    }
}

fn invalidate_asset(path: &std::path::Path, window: &mut Window, cx: &mut gpui::App) {
    let key = gpui_shell::AppAssets::cache_key(&path.to_string_lossy());
    invalidate_image(&gpui::Resource::Embedded(key.clone()), window, cx);
    window.invalidate_svg(&key);
}

fn invalidate_image(resource: &gpui::Resource, window: &mut Window, cx: &mut gpui::App) {
    if let Some(Ok(old)) = cx.cached_asset::<gpui::ImgResourceLoader>(resource) {
        cx.drop_image(old, Some(window));
    }
    cx.remove_asset::<gpui::ImgResourceLoader>(resource);
}

/// The changed paths, plus the files inside changed mod folders (a folder
/// renamed into place arrives as one event). Runs on the UI thread: skips what
/// the watcher ignores (hidden, `node_modules`) and stops at a mod-sized limit.
fn changed_asset_paths(home: &std::path::Path, changed: &[std::path::PathBuf]) -> Vec<std::path::PathBuf> {
    const MAX_FILES: usize = 2000;
    let mut paths: std::collections::BTreeSet<_> = changed.iter().cloned().collect();
    let mut seen = std::collections::HashSet::new();
    let mut pending: Vec<_> = changed.iter().filter(|p| p.starts_with("mods")).cloned().collect();
    while let Some(path) = pending.pop() {
        // Do not follow symlinks out of the mod folder (or into a cycle).
        if !seen.insert(path.clone()) || !home.join(&path).symlink_metadata().is_ok_and(|m| m.is_dir()) { continue; }
        let Ok(entries) = std::fs::read_dir(home.join(&path)) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" { continue; }
            let child = path.join(&*name);
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                pending.push(child);
            } else if paths.len() < MAX_FILES {
                paths.insert(child);
            } else {
                return paths.into_iter().collect();
            }
        }
    }
    paths.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use gpui::{div, point, px, size, Context, IntoElement, Render, TestAppContext, Window};
    use gpui::prelude::*;

    #[gpui::test]
    fn image_invalidation_drops_cached_atlas_tiles_without_loading_missing_assets(cx: &mut TestAppContext) {
        use gpui::{ImgResourceLoader, Resource};
        let temp = if cfg!(target_os = "linux") { std::path::PathBuf::from("/var/tmp") } else { std::env::temp_dir() };
        let home = temp.join(format!("herdr-image-atlas-test-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let file = home.join("image.gif");
        let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&file).unwrap());
        for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder.encode_frame(image::Frame::new(image::RgbaImage::from_pixel(2, 2, image::Rgba(color)))).unwrap();
        }
        drop(encoder);
        let resource = Resource::Path(file.into());
        cx.update(|cx| { cx.fetch_asset::<ImgResourceLoader>(&resource); });
        cx.background_executor.run_until_parked();
        let old = cx.update(|cx| cx.cached_asset::<ImgResourceLoader>(&resource)).unwrap().unwrap();
        let context = cx.add_empty_window();
        let displayed = old.clone();
        assert_eq!(old.frame_count(), 2);
        context.draw(point(px(0.0), px(0.0)), size(px(20.0), px(20.0)), move |_, _| {
            gpui::canvas(|_, _, _| (), move |bounds, _, window, _| {
                for frame in 0..displayed.frame_count() {
                    window.paint_image(bounds, bounds, gpui::Corners::default(), displayed.clone(), frame, false).unwrap();
                }
            }).size_full()
        });
        context.update(|window, cx| {
            assert!(window.has_image_atlas_entry(&old));
            super::invalidate_image(&resource, window, cx);
            assert!(!window.has_image_atlas_entry(&old));
            assert!(!cx.has_asset::<ImgResourceLoader>(&resource));
            let missing = Resource::Embedded("mods/missing.png".into());
            super::invalidate_image(&missing, window, cx);
            assert!(!cx.has_asset::<ImgResourceLoader>(&missing));
        });
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn directory_changes_include_nested_assets() {
        let temp = if cfg!(target_os = "linux") { std::path::PathBuf::from("/var/tmp") } else { std::env::temp_dir() };
        let home = temp.join(format!("herdr-image-directory-test-{}", std::process::id()));
        std::fs::create_dir_all(home.join("mods/probe/nested")).unwrap();
        std::fs::write(home.join("mods/probe/nested/icon.svg"), "svg").unwrap();
        for ignored in ["node_modules/pkg", ".git"] {
            std::fs::create_dir_all(home.join("mods/probe").join(ignored)).unwrap();
            std::fs::write(home.join("mods/probe").join(ignored).join("logo.svg"), "svg").unwrap();
        }
        let changed = vec![std::path::PathBuf::from("mods/probe")];
        // A repeated root and a covered child are walked once.
        let events = [changed[0].clone(), changed[0].clone(), "mods/probe/nested".into()];
        let paths = super::changed_asset_paths(&home, &events);
        assert!(paths.contains(&std::path::PathBuf::from("mods/probe/nested/icon.svg")));
        assert!(paths.contains(&changed[0]));
        assert!(!paths.iter().any(|p| p.to_string_lossy().contains("node_modules") || p.to_string_lossy().contains(".git")));
        assert_eq!(paths.iter().filter(|p| p.ends_with("icon.svg")).count(), 1);
        std::fs::remove_dir_all(&home).unwrap();
        assert_eq!(super::changed_asset_paths(&home, &changed), changed);
    }

    #[test]
    fn asset_cache_keys_match_file_events_without_requiring_the_file() {
        use gpui_shell::AppAssets;
        let expected = AppAssets::cache_key("mods/probe/icon.svg");
        for alias in ["./mods/probe/icon.svg", "/mods/probe/icon.svg", "mods//probe/./icon.svg"] {
            assert_eq!(AppAssets::cache_key(alias), expected);
        }
        // Traversal remains invalid, rather than becoming a valid cache hit.
        assert_eq!(AppAssets::cache_key("mods/probe/../icon.svg").as_ref(), "mods/probe/../icon.svg");
        #[cfg(windows)]
        assert_eq!(AppAssets::cache_key(r"mods\probe\icon.svg"), expected);
    }

    #[test]
    fn svg_invalidation_removes_every_size_but_preserves_other_assets() {
        use gpui::{AtlasKey, DevicePixels, HeadlessAtlas, PlatformAtlas, RenderSvgParams, Size};
        let atlas = HeadlessAtlas::default();
        let key = |path: &str, size| AtlasKey::Svg(RenderSvgParams {
            path: path.to_owned().into(), size: Size::new(DevicePixels(size), DevicePixels(size)),
        });
        let keys = [key("icons/icon.svg", 16), key("icons/icon.svg", 32), key("icons/nested/icon.svg", 16), key("icons-other/icon.svg", 16), AtlasKey::Image(gpui::RenderImageParams { image_id: gpui::ImageId(1), frame_index: 0 })];
        let mut build = || Ok(Some((Size::new(DevicePixels(1), DevicePixels(1)), std::borrow::Cow::Borrowed(&[255u8][..]))));
        let tiles: Vec<_> = keys.iter().map(|key| atlas.get_or_insert_with(key.clone(), &mut build).unwrap().unwrap()).collect();
        atlas.remove_svg("icons");
        atlas.remove_svg("icons");
        for (index, key) in keys.iter().enumerate() {
            assert_eq!(atlas.contains(key), index >= 3);
            let tile = atlas.get_or_insert_with(key.clone(), &mut build).unwrap().unwrap();
            assert_eq!(tile == tiles[index], index >= 3);
        }
    }

    #[test]
    fn agent_commands_keep_arguments_out_of_shell_source() {
        let cmd = vec!["agent".to_owned(), "a 'quoted' task; $(not-a-command)\nnext line".to_owned(), "".to_owned()];
        for login in [false, true] {
            for sh in ["/bin/bash", "/bin/zsh", "/bin/sh", "/bin/dash", "/bin/fish"] {
                let run = super::unix_agent_argv(sh.into(), cmd.clone(), login);
                assert_eq!(run[0], sh);
                assert_eq!(run[1], if login { "-lic" } else { "-ic" });
                assert_eq!(run[3], sh);
                assert_eq!(run[4..], cmd);
                let expected = match (sh.ends_with("/fish"), login) {
                    (true, true) => "command $argv[2..]; exec $argv[1] -li",
                    (true, false) => "command $argv[2..]; exec $argv[1] -i",
                    (false, true) => "\"$@\"; exec \"$0\" -li",
                    (false, false) => "\"$@\"; exec \"$0\" -i",
                };
                assert_eq!(run[2], expected);
            }
        }
    }

    #[test]
    fn early_launch_failures_show_exit_code_and_last_output_lines() {
        let mut record = serde_json::json!({"reason":"exited", "exit_code":127, "created_ms":1000, "finished_ms":3000});
        assert_eq!(super::launch_failure(&record, "old\none\ntwo\nthree\nfour\nfive\n\n").as_deref(), Some("Exit code 127\none\ntwo\nthree\nfour\nfive"));
        assert_eq!(super::launch_failure(&record, "").as_deref(), Some("Exit code 127"));
        record["exit_code"] = 0.into();
        assert!(super::launch_failure(&record, "").is_none());
        record["exit_code"] = 1.into();
        record["finished_ms"] = 3001.into();
        assert!(super::launch_failure(&record, "").is_none());
        record["finished_ms"] = 999.into();
        assert!(super::launch_failure(&record, "").is_none());
        record["finished_ms"] = 2000.into();
        record["reason"] = "killed".into();
        assert!(super::launch_failure(&record, "").is_none());
        assert!(super::launch_failure(&serde_json::Value::Null, "").is_none());
    }

    #[gpui::test]
    fn traffic_light_dividers_start_below_the_toolbar(cx: &mut TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        let mut layout: crate::layout::Node = serde_json::from_str(include_str!("../defaults/layouts/main.json")).unwrap();
        let context = cx.add_empty_window();
        for (macos, titlebar, width, zoom, expected) in [
            (true, crate::layout::Titlebar::Layout, 44.0, 1.0, 42.0),
            (true, crate::layout::Titlebar::Layout, 90.0, 0.75, 42.0),
            (true, crate::layout::Titlebar::Layout, 60.0, 1.5, 0.0),
            (true, crate::layout::Titlebar::Layout, 60.0, 1.0, 42.0),
            (true, crate::layout::Titlebar::Layout, 76.0, 1.0, 42.0),
            (true, crate::layout::Titlebar::Layout, 268.0, 1.0, 0.0),
            (false, crate::layout::Titlebar::Layout, 44.0, 1.0, 0.0),
            (true, crate::layout::Titlebar::System, 44.0, 1.0, 0.0),
        ] {
            layout.titlebar = Some(titlebar);
            layout.children_mut().unwrap()[0].size = Some(crate::layout::Size::Px { px: width });
            let top = super::traffic_light_divider_inset(&layout, crate::layout::Axis::Row, 0, macos, zoom);
            assert_eq!(top, expected);
            let bounds = Rc::new(Cell::new(gpui::Bounds::default()));
            let observed = bounds.clone();
            context.draw(point(px(0.0), px(0.0)), size(px(1.0), px(100.0)), move |_, _| {
                div().size_full().relative().flex().items_center().justify_center().child(
                    super::separator_line(true, 1.0, gpui::rgb(0xffffff).into(), top).child(
                        gpui::canvas(move |bounds, _, _| observed.set(bounds), |_, _, _, _| {}).size_full()
                    )
                )
            });
            assert_eq!(bounds.get().origin.y, px(expected));
            assert_eq!(bounds.get().size.height, px(100.0 - expected));
        }
        layout.traffic_lights = None;
        assert_eq!(super::traffic_light_divider_inset(&layout, crate::layout::Axis::Row, 0, true, 1.0), 0.0);
    }

    #[gpui::test]
    fn fitted_popups_use_their_rendered_size_at_the_window_edge(cx: &mut TestAppContext) {
        struct Popup {
            height: f32,
            edge: bool,
        }
        impl Render for Popup {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let viewport = window.viewport_size();
                let at = if self.edge {
                    point(viewport.width - px(40.0), viewport.height - px(40.0))
                } else {
                    point(px(100.0), px(200.0))
                };
                div().size_full().child(super::position_popup(
                    at,
                    div()
                        .id("popup")
                        .debug_selector(|| "popup".into())
                        .w(px(220.0))
                        .max_h(viewport.height * 0.85)
                        .child(div().h(px(self.height))),
                ))
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Popup { height: 72.0, edge: false });
        for (height, edge) in [(72.0, false), (72.0, true), (300.0, true)] {
            view.update(cx, |view, cx| {
                view.height = height;
                view.edge = edge;
                cx.notify();
            });
            cx.run_until_parked();
            let bounds = cx.debug_bounds("popup").unwrap();
            assert_eq!(bounds.size, size(px(220.0), px(height)));
            if edge {
                let viewport = cx.update(|window, _| window.viewport_size());
                assert_eq!(bounds.bottom_right(), point(viewport.width - px(8.0), viewport.height - px(8.0)));
            } else {
                assert_eq!(bounds.origin, point(px(100.0), px(200.0)));
            }
        }
    }

    #[test]
    fn terminal_menu_is_a_root_layout_choice_that_roundtrips() {
        let mut root = super::default_layout("main");
        assert_eq!(root.terminal_menu, None);
        assert!(serde_json::to_value(&root).unwrap().get("terminal_menu").is_none());
        root.terminal_menu = Some("personal-menu".into());
        let restored: super::Node = serde_json::from_value(serde_json::to_value(&root).unwrap()).unwrap();
        assert_eq!(restored.terminal_menu.as_deref(), Some("personal-menu"));
    }

    #[test]
    fn terminal_menu_preserves_clicked_target_and_uses_mod_dimensions() {
        use serde_json::json;
        let target = json!({ "pane": "p2", "terminal": "t8" });
        let stock = super::terminal_menu_spec("menu", &json!({}), "pane", target.clone());
        assert_eq!(stock["width"], 220.0);
        assert_eq!(stock["height"], 260.0);
        let custom = super::terminal_menu_spec("personal-menu", &json!({"kind": "dialog", "width": 310, "height": 120}), "pane", target.clone());
        assert_eq!(custom["mod"], "personal-menu");
        assert_eq!(custom["kind"], "popup");
        assert_eq!(custom["width"], 310.0);
        assert_eq!(custom["height"], 120.0);
        assert_eq!(custom["props"], json!({"tab": "pane", "target": target}));
        let partial = super::terminal_menu_spec("personal-menu", &json!({"width": 0.4}), "pane", json!({}));
        assert_eq!(partial["width"], 0.4);
        assert_eq!(partial["height"], 260.0);
    }

    #[test]
    fn composition_focus_only_uses_visible_terminal_blocks() {
        let chrome: super::Node = serde_json::from_value(serde_json::json!({
            "id": "composition", "type": "row", "children": [
                { "id": "left", "type": "block", "content": {"terminal": "t1"} },
                { "id": "right", "type": "block", "content": {"terminal": "other/t2"} },
                { "id": "hidden", "type": "block", "hidden": true, "content": {"terminal": "t3"} },
                { "id": "tabs", "type": "block", "content": {"tab": "active"} }
            ]
        })).unwrap();
        assert_eq!(super::terminal_block(&chrome, Some("left")).and_then(|n| n.terminal()), Some("t1"));
        assert_eq!(super::terminal_block(&chrome, Some("right")).and_then(|n| n.terminal()), Some("other/t2"));
        for id in [None, Some("hidden"), Some("tabs"), Some("source-pane"), Some("overlay:term")] {
            assert!(super::terminal_block(&chrome, id).is_none());
        }
    }

    #[test]
    fn composition_commands_never_default_to_a_source_pane() {
        use serde_json::json;
        for name in ["pane.split", "pane.close", "pane.zoom", "pane.go", "pane.swap", "terminal.show"] {
            assert!(super::needs_model_pane(name, &json!({})), "{name}");
            assert!(super::needs_model_pane(name, &json!({"pane": null})), "{name}");
        }
        for name in ["pane.split", "pane.close", "pane.zoom", "terminal.show"] {
            assert!(!super::needs_model_pane(name, &json!({"pane": "explicit-pane"})), "{name}");
        }
        for name in ["terminal.open", "terminal.keep"] {
            for place in ["right", "down"] {
                assert!(super::needs_model_pane(name, &json!({"place": place})), "{name} {place}");
            }
            assert!(!super::needs_model_pane(name, &json!({})), "{name}");
            assert!(!super::needs_model_pane(name, &json!({"place": "tab"})), "{name}");
        }
        for name in ["pane.focus", "terminal.copy", "terminal.paste", "terminal.paste_image", "terminal.right_click"] {
            assert!(!super::needs_model_pane(name, &json!({})), "{name}");
        }
    }

    #[test]
    fn a_folder_is_asked_again_only_when_its_answer_could_have_changed() {
        use super::{FOLDER_FALLBACK, FolderCheck, recheck};
        use std::time::{Duration, Instant};
        let checked = |busy, repo, ago: Duration| FolderCheck { at: Instant::now() - ago, cwd: "/src".into(), busy, repo };
        let (now, later) = (Duration::ZERO, FOLDER_FALLBACK + Duration::from_secs(1));
        assert!(recheck(None, "/src", false, false), "never asked");
        assert!(!recheck(Some(&checked(false, true, now)), "/src", false, true), "nothing happened");
        assert!(recheck(Some(&checked(false, true, now)), "/elsewhere", false, false), "it moved");
        assert!(recheck(Some(&checked(true, true, now)), "/src", false, false), "a command or turn ended");
        assert!(!recheck(Some(&checked(false, true, now)), "/src", true, true), "one started");
        assert!(recheck(Some(&checked(false, true, later)), "/src", false, true), "a repo, a minute on, in front");
        assert!(!recheck(Some(&checked(false, true, later)), "/src", false, false), "not while the window is behind");
        assert!(!recheck(Some(&checked(false, false, later)), "/src", false, true), "not a repo: never on a timer");
    }

    #[test]
    fn busy_is_an_agent_working_or_a_program_in_front_of_the_shell() {
        let rec = |status: &str, shell: bool, agent: bool| {
            serde_json::json!({ "status": status, "program": { "shell": shell, "agent": agent } })
        };
        assert!(super::busy(&rec("working", false, true)), "an agent's turn");
        assert!(!super::busy(&rec("done", false, true)), "an agent started from a shell, its turn over");
        assert!(super::busy(&rec("", false, false)), "vim in front of the shell");
        assert!(!super::busy(&rec("", true, false)), "the shell's prompt");
    }

    #[test]
    fn shipped_layouts_parse_and_show_what_they_promise() {
        let main = super::default_layout("main");
        assert!(main.find("main").is_some() && main.find("sidebar").is_some());
        assert!(main.walk().iter().any(|n| matches!(n.content(), Some(super::Content::Context { .. }))));
        let terminals = super::default_layout("terminals");
        assert!(terminals.find("tabs").is_some());
        assert!(matches!(terminals.find("main").and_then(|n| n.content()), Some(super::Content::Context { .. })));
    }

    #[test]
    fn a_finished_terminal_is_not_put_back_in_its_tab() {
        let record = |state: &str| serde_json::json!({ "state": state, "labels": { "herdr.space": "sp1", "herdr.tab": "tb1" } });
        assert!(super::adoptable(&record("live"), "sp1", Some("tb1"), None));
        assert!(!super::adoptable(&record("finished"), "sp1", Some("tb1"), None));
        assert!(!super::adoptable(&record("live"), "sp2", Some("tb1"), None));
        assert!(!super::adoptable(&record("live"), "sp1", Some("tb2"), None));
    }

}
