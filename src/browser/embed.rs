//! A native program's own window shown inside a Herdr pane (Windows).
//!
//! The pane asks the program for a new window and adopts that top-level
//! window. VS Code opens it with `--new-window` on a folder, from a process
//! that may already run the person's other windows. Fork gets a process of its
//! own per pane, which opens only the pane's repository. The window moves into
//! a container child window that Herdr owns: the container follows the pane,
//! crops the program's own title bar and frame, and has overlays cut out of it.
//! Closing the pane gives the window back to the desktop and closes it, as its
//! own close button would.
//!
//! Native calls on another process's window send messages to it. They run in
//! foreground tasks between App updates, never inside layout or paint, so a
//! reentrant message cannot meet a borrowed GPUI window.
use super::policy;
use gpui::{AnyElement, App, Bounds, HitboxBehavior, HitboxId, ParentElement, Pixels, Styled, Window, WindowId, canvas, div};
use gpui_shell::{ArgumentDescriptor, ArgumentSchema, ComponentArgument, ComponentDataValue, ComponentDescriptor, ComponentMaterializer, ComponentPayload, ComponentRegistry, ConstructorDescriptor, MaterializeRequest, StateDescriptor, StateMethodDescriptor};
use std::{cell::RefCell, path::PathBuf, rc::{Rc, Weak}, sync::Arc};

type State = Rc<RefCell<NativeApp>>;
thread_local! {
    static APPS: RefCell<Vec<Weak<RefCell<NativeApp>>>> = const { RefCell::new(Vec::new()) };
}

/// What to start and how to recognize its windows.
#[derive(Clone, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Launch {
    program: PathBuf,
    args: Vec<String>,
    /// Win32 class of the program's top-level windows; a trailing `*` matches
    /// any rest (WPF adds a GUID per run).
    class: &'static str,
    /// What the title of a window already showing the target contains as a
    /// " - "-separated part (VS Code: the folder's name, maybe with " [WSL: …]").
    title: String,
    /// Only a window of the started process is the pane's.
    own_process: bool,
    /// Fork: the repository that Fork's settings get as the only one of the
    /// active workspace just before the start.
    fork_repo: Option<String>,
}

/// A rectangle in device pixels, relative to the host window's client area.
type Rect = [i32; 4];

// Program, target and launch details are used by the Windows backend only.
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) struct NativeApp {
    window: WindowId,
    app: String,
    target: String,
    status: String,
    error: Option<String>,
    hitbox: Option<HitboxId>,
    rect: Option<Rect>,
    visible: bool,
    holes: Vec<[i32; 4]>,
    apply_pending: bool,
    /// Bumped by reopen, so an older watcher task ends.
    generation: u64,
    /// The window was closed; only reopen asks for a new one.
    closed: bool,
    /// Keyboard focus was asked for while the pane was hidden: give it when
    /// the pane shows up soon after (a tab selected just before).
    focus_until: Option<std::time::Instant>,
    #[cfg(windows)]
    embedded: Option<win::Embedded>,
    #[cfg(windows)]
    opening: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
fn launch(app: &str, target: &str) -> anyhow::Result<Launch> {
    match app {
        "vscode" => vscode(target),
        "fork" => fork(target),
        other => anyhow::bail!("Unknown native app {other:?}; Herdr knows \"vscode\" and \"fork\""),
    }
}

/// The person's own VS Code: default profile and user data, so a running VS
/// Code opens the window itself. `target` is a folder URI (`file://`,
/// `vscode-remote://wsl+…`) or a local folder path.
#[cfg_attr(not(windows), allow(dead_code))]
fn vscode(target: &str) -> anyhow::Result<Launch> {
    let env = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let candidates = [
        env("HERDR_VSCODE_EXE"),
        env("LOCALAPPDATA").map(|d| d.join("Programs/Microsoft VS Code/Code.exe")),
        env("ProgramFiles").map(|d| d.join("Microsoft VS Code/Code.exe")),
    ];
    let program = candidates.into_iter().flatten().find(|p| p.is_file())
        .ok_or_else(|| anyhow::anyhow!("VS Code is not installed (set HERDR_VSCODE_EXE to Code.exe)"))?;
    let target = target.trim();
    anyhow::ensure!(!target.is_empty(), "No folder to open");
    let mut args = vec!["--new-window".to_owned()];
    if target.contains("://") {
        args.extend(["--folder-uri".to_owned(), target.to_owned()]);
    } else {
        args.push(target.to_owned());
    }
    let name = target.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default();
    let title = url::form_urlencoded::parse(format!("n={name}").as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default();
    Ok(Launch { program, args, class: "Chrome_WidgetWin_1", title, own_process: false, fork_repo: None })
}

/// A Fork (git client) process of the pane's own on a repository. `target`
/// is a local folder, a `file://` URI or a VS Code WSL folder URI
/// (`vscode-remote://wsl+…`).
///
/// Fork started with a folder hands it to a running Fork and exits; started
/// with none, it is a new process that restores the tabs of its active
/// workspace. So the pane writes a workspace of only its repository first.
#[cfg_attr(not(windows), allow(dead_code))]
fn fork(target: &str) -> anyhow::Result<Launch> {
    let env = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let candidates = [
        env("HERDR_FORK_EXE"),
        env("LOCALAPPDATA").map(|d| d.join("Fork/current/Fork.exe")),
        env("LOCALAPPDATA").map(|d| d.join("Fork/Fork.exe")),
    ];
    let program = candidates.into_iter().flatten().find(|p| p.is_file())
        .ok_or_else(|| anyhow::anyhow!("Fork is not installed (set HERDR_FORK_EXE to Fork.exe)"))?;
    let mut folder = windows_folder(target)?;
    while folder.len() > 3 && folder.ends_with(['\\', '/']) { folder.pop(); }
    Ok(Launch { program, args: Vec::new(), class: "HwndWrapper[Fork.exe;*", title: String::new(), own_process: true, fork_repo: Some(folder) })
}

/// The name of the Fork workspace that Herdr's panes start Fork in.
const FORK_WORKSPACE: &str = "Herdr";

/// Give the "Herdr" workspace of Fork's settings
/// (`%LOCALAPPDATA%\Fork\settings.json`) only `folder` and make it active,
/// and mark `folder` safe for Fork's git. Other workspaces and settings stay
/// as they are. Without that file Fork has never run: then it starts empty.
#[cfg_attr(not(windows), allow(dead_code))]
fn prepare_fork(folder: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    let dir = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).context("LOCALAPPDATA is not set")?.join("Fork");
    let path = dir.join("settings.json");
    let settings: Option<serde_json::Value> = match std::fs::read_to_string(&path) {
        Ok(text) => Some(serde_json::from_str(text.trim_start_matches('\u{feff}')).with_context(|| format!("Cannot parse {}", path.display()))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("Cannot read {}", path.display())),
    };
    #[cfg(windows)]
    let trusted = trust_repository(&dir, settings.as_ref(), folder);
    #[cfg(not(windows))]
    let trusted = anyhow::Ok(());
    let Some(mut settings) = settings else { return trusted };
    fork_workspace(&mut settings, folder)?;
    // A running Fork may read the file at any time: replace it whole.
    let temp = dir.join("settings.json.herdr");
    std::fs::write(&temp, serde_json::to_vec_pretty(&settings)?).with_context(|| format!("Cannot write {}", temp.display()))?;
    std::fs::rename(&temp, &path).with_context(|| format!("Cannot replace {}", path.display()))?;
    trusted
}

/// Git refuses a repository that another user owns, as every WSL one is
/// (`\\wsl.localhost\...`), until it is a `safe.directory`. Herdr opens the
/// person's own repositories: add it to the global git config with Fork's
/// git, as Fork's "Mark repository as safe" does.
#[cfg(windows)]
fn trust_repository(fork_dir: &std::path::Path, settings: Option<&serde_json::Value>, folder: &str) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    let Some(entry) = safe_directory(folder) else { return Ok(()) };
    let git = fork_git(fork_dir, settings);
    let config = |args: &[&str]| {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new(&git).arg("config").arg("--global").args(args)
            .stdin(std::process::Stdio::null()).creation_flags(CREATE_NO_WINDOW).output()
    };
    let listed = config(&["--get-all", "safe.directory"])?;
    if String::from_utf8_lossy(&listed.stdout).lines().any(|line| line.trim() == entry || line.trim() == "*") { return Ok(()); }
    let added = config(&["--add", "safe.directory", &entry])?;
    anyhow::ensure!(added.status.success(), "{} cannot mark {folder} safe: {}", git.display(), String::from_utf8_lossy(&added.stderr).trim());
    Ok(())
}

/// The git Fork runs: its `GitInstancePath` setting, else its newest bundled
/// git, else the one on PATH.
#[cfg(windows)]
fn fork_git(fork_dir: &std::path::Path, settings: Option<&serde_json::Value>) -> PathBuf {
    let chosen = settings.and_then(|s| s.get("GitInstancePath")).and_then(serde_json::Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from);
    if let Some(path) = chosen.into_iter().flat_map(|p| [p.join("bin/git.exe"), p]).find(|p| p.is_file()) { return path; }
    std::fs::read_dir(fork_dir.join("gitInstance")).into_iter().flatten().flatten()
        .map(|entry| entry.path().join("bin/git.exe")).filter(|p| p.is_file())
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
        .unwrap_or_else(|| PathBuf::from("git"))
}

/// The `safe.directory` entry git asks for a UNC folder (WSL folders are
/// `\\wsl.localhost\...`); none for a local drive, which the person owns.
#[cfg_attr(not(windows), allow(dead_code))]
fn safe_directory(folder: &str) -> Option<String> {
    let rest = folder.strip_prefix(r"\\")?;
    Some(format!("%(prefix)///{}", rest.replace('\\', "/")))
}

/// Fork's settings JSON with the "Herdr" workspace holding only `folder`,
/// made active.
#[cfg_attr(not(windows), allow(dead_code))]
fn fork_workspace(settings: &mut serde_json::Value, folder: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    use serde_json::{Value, json};
    let root = settings.as_object_mut().context("Fork's settings are not a JSON object")?;
    let workspaces = root.entry("Workspaces").or_insert_with(|| json!({}))
        .as_object_mut().context("Fork's Workspaces setting is not an object")?;
    {
        let all = workspaces.entry("All").or_insert_with(|| json!([]))
            .as_array_mut().context("Fork's workspace list is not an array")?;
        let index = match all.iter().position(|w| w.get("Name").and_then(Value::as_str) == Some(FORK_WORKSPACE)) {
            Some(index) => index,
            None => { all.push(json!({ "Name": FORK_WORKSPACE })); all.len() - 1 }
        };
        let workspace = all[index].as_object_mut().context("A Fork workspace is not an object")?;
        workspace.insert("Repositories".into(), json!([folder]));
        workspace.insert("ActiveRepository".into(), json!(folder));
    }
    workspaces.insert("ActiveWorkspace".into(), json!(FORK_WORKSPACE));
    Ok(())
}

/// A window class against a `Launch.class` pattern.
#[cfg_attr(not(windows), allow(dead_code))]
fn class_matches(pattern: &str, class: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => class.starts_with(prefix),
        None => class == pattern,
    }
}

/// A Windows path for a folder given as a path or a folder URI.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_folder(target: &str) -> anyhow::Result<String> {
    let target = target.trim();
    anyhow::ensure!(!target.is_empty(), "No folder to open");
    // %XX escapes only: '+' stays itself.
    let decode = |s: &str| url::form_urlencoded::parse(format!("n={}", s.replace('+', "%2B")).as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_default();
    let Some((scheme, rest)) = target.split_once("://") else { return Ok(target.to_owned()) };
    let (authority, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
    let path = decode(path).replace('/', "\\");
    match scheme {
        "file" if authority.is_empty() => Ok(path),
        "file" => Ok(format!(r"\\{authority}\{path}")),
        "vscode-remote" => match decode(authority).strip_prefix("wsl+") {
            Some(distro) if !distro.is_empty() => Ok(format!(r"\\wsl.localhost\{distro}\{path}")),
            _ => anyhow::bail!("Only local and WSL folders open on Windows, not {target}"),
        },
        _ => anyhow::bail!("Only local and WSL folders open on Windows, not {target}"),
    }
}

impl Drop for NativeApp {
    fn drop(&mut self) {
        // A mod that restarts (a reload) makes a new state at once: it takes
        // the window back within a moment, or the window closes.
        #[cfg(windows)]
        if let Some(embedded) = self.embedded.take() { win::orphan(&self.app, &self.target, embedded); }
    }
}

impl NativeApp {
    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window) {
        self.hitbox = Some(window.insert_hitbox(bounds, HitboxBehavior::Normal).id);
        let scale = window.scale_factor();
        let px = |v: Pixels| (f32::from(v) * scale).round() as i32;
        self.rect = Some([px(bounds.origin.x), px(bounds.origin.y), px(bounds.size.width), px(bounds.size.height)]);
    }

    fn update_visibility(&mut self, window: &Window, dragging: bool, covers: &[Bounds<Pixels>]) {
        let hitbox = self.hitbox.and_then(|id| window.prepainted_hitbox(id));
        let scale = window.scale_factor();
        // The parts outside the pane's clip mask are cut out like overlays.
        let mut covers = covers.to_vec();
        if let Some(hitbox) = hitbox.as_ref() {
            covers.extend(policy::outside_mask(&hitbox.bounds, &hitbox.content_mask.bounds, scale));
        }
        let covered = dragging || hitbox.is_some_and(|hitbox| policy::fully_covered(&hitbox.bounds, &covers));
        self.visible = policy::should_show(hitbox.is_some(), true, covered);
        self.holes = hitbox.map(|hitbox| policy::holes(&hitbox.bounds, &covers, scale)).unwrap_or_default();
    }

    /// The native state differs from the latest layout.
    fn needs_apply(&self) -> bool {
        #[cfg(windows)]
        if let (Some(embedded), Some(rect)) = (&self.embedded, self.rect) {
            return embedded.applied.as_ref() != Some(&(rect, self.visible, self.holes.clone()));
        }
        false
    }
}

/// Open the program's window on first use. Runs between App updates on Windows.
fn start(state: &State, window: &mut Window, cx: &mut App) {
    #[cfg(windows)]
    win::start(state, window, cx);
    #[cfg(not(windows))]
    {
        let _ = (window, cx);
        let mut app = state.borrow_mut();
        if app.error.is_none() {
            app.error = Some("Embedding native apps is only available on Windows.".into());
        }
    }
}

fn schedule_apply(state: &State, window: &mut Window, cx: &mut App) {
    {
        let mut app = state.borrow_mut();
        if app.apply_pending || !app.needs_apply() { return; }
        app.apply_pending = true;
    }
    #[cfg(windows)]
    {
        let weak = Rc::downgrade(state);
        window.spawn(cx, async move |_| {
            if let Some(state) = weak.upgrade() { win::apply(&state); }
        }).detach();
    }
    #[cfg(not(windows))]
    let _ = (window, cx);
}

/// Called from the browser's end-of-frame canvas: show, hide or cut each
/// embedded window for overlays, clipping and inactive tabs.
pub(super) fn end_frame(dragging: bool, covers: &[Bounds<Pixels>], window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let apps: Vec<State> = APPS.with_borrow_mut(|apps| {
        apps.retain(|weak| weak.strong_count() > 0);
        apps.iter().filter_map(Weak::upgrade).filter(|app| app.borrow().window == id).collect()
    });
    for app in &apps {
        app.borrow_mut().update_visibility(window, dragging, covers);
    }
    for app in &apps {
        schedule_apply(app, window, cx);
    }
}

struct NativeAppElement;
impl ComponentMaterializer for NativeAppElement {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> anyhow::Result<AnyElement> {
        let argument = request.payload().downcast_ref::<ComponentArgument>().unwrap().clone();
        let state = request.with_state::<State, _>(&argument, Clone::clone)?;
        request.with_window_app(|window, cx| {
            start(&state, window, cx);
            Ok(())
        })?;
        let element = div().size_full().child(canvas(move |bounds, window, cx| {
            state.borrow_mut().layout(bounds, window);
            schedule_apply(&state, window, cx);
        }, |_, _, _, _| {}).size_full());
        request.finish(element)
    }
}

pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register_state(StateDescriptor::new(
        "NativeAppState", "HerdrNativeAppState",
        vec![ArgumentDescriptor::new("app", ArgumentSchema::String), ArgumentDescriptor::new("target", ArgumentSchema::String)],
        |args, window, _| {
            let (ComponentArgument::String(app), ComponentArgument::String(target)) = (&args[0], &args[1]) else { unreachable!() };
            let state = Rc::new(RefCell::new(NativeApp {
                window: window.window_handle().window_id(), app: app.clone(), target: target.clone(),
                status: "Opening…".into(), error: None, hitbox: None, rect: None, visible: false, holes: Vec::new(),
                apply_pending: false, generation: 0, closed: false, focus_until: None,
                #[cfg(windows)] embedded: None,
                #[cfg(windows)] opening: false,
            }));
            APPS.with_borrow_mut(|apps| apps.push(Rc::downgrade(&state)));
            Ok(Box::new(state))
        },
    ).with_documentation("A program's own window shown in this pane: \"vscode\" (the person's VS Code on a folder URI or path) or \"fork\" (a Fork process of this pane's own, showing only one repository: a local or WSL folder). Closing the pane closes that window.")
    .with_methods(vec![
        StateMethodDescriptor::new::<State>("status", "(): string", |state, _, _, _| {
            Ok(ComponentDataValue::String(state.borrow().status.clone()))
        }).with_readonly(true),
        StateMethodDescriptor::new::<State>("error", "(): string | null", |state, _, _, _| {
            Ok(state.borrow().error.clone().map(ComponentDataValue::String).unwrap_or(ComponentDataValue::Null))
        }).with_readonly(true),
        StateMethodDescriptor::new::<State>("shown", "(): boolean", |state, _, _, _| {
            #[cfg(windows)]
            return Ok(ComponentDataValue::Boolean(state.borrow().embedded.is_some()));
            #[cfg(not(windows))]
            { let _ = state; Ok(ComponentDataValue::Boolean(false)) }
        }).with_readonly(true),
        StateMethodDescriptor::new::<State>("reopen", "(): void", |state, _, _, _| {
            let mut app = state.borrow_mut();
            if app.closed || app.error.is_some() {
                app.generation += 1;
                app.closed = false;
                app.error = None;
                app.status = "Opening…".into();
            }
            Ok(ComponentDataValue::Null)
        }),
        StateMethodDescriptor::new::<State>("focus", "(): void", |state, _, _, _| {
            // Every copy of a mod gets its commands: only a pane shown within a
            // second takes the keyboard. The next frame gives it, outside this
            // App update (SetFocus sends messages to Herdr's window at once).
            let mut app = state.borrow_mut();
            app.focus_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(1));
            #[cfg(windows)]
            if let Some(embedded) = app.embedded.as_mut() { embedded.applied = None; }
            Ok(ComponentDataValue::Null)
        }),
    ])).unwrap();
    registry.register(ComponentDescriptor::new("NativeApp", Arc::new(NativeAppElement))
        .with_documentation("The window of a NativeAppState, filling this element. Retain the state in init.")
        .with_constructors(vec![ConstructorDescriptor::new("NativeApp", vec![ArgumentDescriptor::new("state", ArgumentSchema::Entity("HerdrNativeAppState"))], |args| Ok(ComponentPayload::new(args[0].clone())))])
        .with_methods(vec![])).unwrap();
}

#[cfg(windows)]
mod win {
    use super::{Launch, Rect, State};
    use gpui::{App, Window};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{cell::Cell, rc::Rc, time::{Duration, Instant}};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        System::{LibraryLoader::GetModuleHandleW, Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW}},
        UI::Input::KeyboardAndMouse::SetFocus,
        UI::WindowsAndMessaging::*,
    };

    /// An adopted window in its container.
    pub(super) struct Embedded {
        pub(super) child: HWND,
        container: HWND,
        /// The window's own styles, restored when it goes back to the desktop.
        style: isize,
        ex_style: isize,
        /// Device pixels of the window cut off at each side: its title bar and frame.
        crop: Crop,
        pub(super) applied: Option<(Rect, bool, Vec<[i32; 4]>)>,
    }

    #[derive(Clone, Copy, Default, PartialEq, Debug)]
    pub(super) struct Crop { left: i32, top: i32, right: i32, bottom: i32 }

    impl Drop for Embedded {
        fn drop(&mut self) {
            focus::forget(self.container);
            // Another process's window: give it back to the desktop and close
            // it from a helper thread, so its sent messages never reenter an
            // App update. A clean window closes at once; one with unsaved
            // changes shows its own save prompt on the desktop.
            let (child, container, style, ex_style) = (self.child as usize, self.container as usize, self.style, self.ex_style);
            let screen = screen_rect(self.container);
            let _ = std::thread::Builder::new().name("native-app-release".into()).spawn(move || {
                release(child as HWND, style, ex_style, screen);
                unsafe {
                    PostMessageW(child as HWND, WM_CLOSE, 0, 0);
                    // DestroyWindow must run on the container's own thread.
                    PostMessageW(container as HWND, WM_CLOSE, 0, 0);
                }
            });
        }
    }

    fn screen_rect(hwnd: HWND) -> RECT {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(hwnd, &mut r) };
        r
    }

    /// The window back on the desktop, where the pane showed it.
    fn release(child: HWND, style: isize, ex_style: isize, at: RECT) {
        unsafe {
            if IsWindow(child) == 0 { return; }
            SetWindowLongPtrW(child, GWL_STYLE, style & !(WS_VISIBLE as isize) & !(WS_MAXIMIZE as isize));
            SetWindowLongPtrW(child, GWL_EXSTYLE, ex_style);
            SetParent(child, std::ptr::null_mut());
            let (w, h) = (at.right - at.left, at.bottom - at.top);
            if w > 0 && h > 0 {
                SetWindowPos(child, HWND_TOP, at.left, at.top, w, h, SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW);
            } else {
                ShowWindow(child, SW_SHOWNOACTIVATE);
            }
        }
    }

    thread_local! {
        static QUIT_HOOKED: Cell<bool> = const { Cell::new(false) };
        /// Windows of dropped states, by program and target, until a new
        /// state takes them or their time is up.
        static ORPHANS: std::cell::RefCell<Vec<(String, String, Embedded, Instant)>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// How long a dropped state's window waits for a restarted mod.
    const ORPHAN_GRACE: Duration = Duration::from_secs(2);

    /// Keep a dropped state's window, hidden, for a state that takes it soon.
    /// Runs inside App updates: posted calls only.
    pub(super) fn orphan(app: &str, target: &str, embedded: Embedded) {
        unsafe { ShowWindowAsync(embedded.container, SW_HIDE) };
        ORPHANS.with_borrow_mut(|o| o.push((app.to_owned(), target.to_owned(), embedded, Instant::now() + ORPHAN_GRACE)));
    }

    pub(super) fn start(state: &State, window: &mut Window, cx: &mut App) {
        let (generation, app, target) = {
            let mut app = state.borrow_mut();
            if app.embedded.is_some() || app.opening || app.error.is_some() || app.closed { return; }
            app.opening = true;
            (app.generation, app.app.clone(), app.target.clone())
        };
        if !QUIT_HOOKED.get() {
            QUIT_HOOKED.set(true);
            // Quitting Herdr closes every adopted window as its own close
            // button would. Only posted messages: this runs inside an App update.
            cx.on_app_quit(|_| {
                let close = |e: Embedded| {
                    let e = std::mem::ManuallyDrop::new(e);
                    unsafe { PostMessageW(e.child, WM_CLOSE, 0, 0) };
                };
                super::APPS.with_borrow(|apps| {
                    for app in apps.iter().filter_map(std::rc::Weak::upgrade) {
                        if let Some(e) = app.borrow_mut().embedded.take() { close(e); }
                    }
                });
                for (_, _, e, _) in ORPHANS.take() { close(e); }
                async {}
            }).detach();
            // Close the windows nobody took back.
            cx.spawn(async |cx| loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                let now = Instant::now();
                let expired: Vec<_> = ORPHANS.with_borrow_mut(|o| o.extract_if(.., |(_, _, _, until)| *until <= now).collect());
                drop(expired);
            }).detach();
        }
        let host = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get() as HWND,
            _ => { fail(state, "No Win32 host window".into()); return; }
        };
        let launch = match super::launch(&app, &target) {
            Ok(launch) => launch,
            Err(error) => { fail(state, format!("{error:#}")); return; }
        };
        // A pane whose mod restarted (a mod reload, a layout change) gets the
        // window its earlier copy still holds for the same target: VS Code
        // would only bring that window forward instead of opening another.
        if let Some(embedded) = take_over(state, &app, &target, host) {
            let child = embedded.child as usize;
            show(state, embedded);
            let weak = Rc::downgrade(state);
            window.spawn(cx, async move |cx| {
                let _ = cx.update(|window, _| window.refresh());
                watch_loop(weak, generation, child, cx).await;
            }).detach();
            return;
        }
        let weak = Rc::downgrade(state);
        window.spawn(cx, async move |cx| {
            // Fork reads its settings when it starts: one start at a time,
            // each with its own repository there.
            let starting = match &launch.fork_repo {
                Some(folder) => {
                    while STARTING.get() {
                        let Some(state) = wanted(&weak, generation) else { return };
                        state.borrow_mut().status = "Waiting for another Fork to start…".into();
                        drop(state);
                        cx.background_executor().timer(Duration::from_millis(100)).await;
                    }
                    // A start whose pane went away meanwhile left its window.
                    if let Some(embedded) = take_orphan(&app, &target) {
                        let Some(state) = wanted(&weak, generation) else { orphan(&app, &target, embedded); return };
                        let child = embedded.child as usize;
                        show(&state, moved_to(embedded, host));
                        drop(state);
                        let _ = cx.update(|window, _| window.refresh());
                        watch_loop(weak, generation, child, cx).await;
                        return;
                    }
                    let starting = Starting::new();
                    let folder = folder.clone();
                    if let Err(error) = cx.background_executor().spawn(async move { super::prepare_fork(&folder) }).await {
                        tracing::warn!(error = format!("{error:#}"), "cannot give Fork the pane's repository");
                    }
                    Some(starting)
                }
                None => None,
            };
            let Some(state) = wanted(&weak, generation) else { return };
            let before = program_windows(&launch);
            let foreground = unsafe { GetForegroundWindow() };
            let mut child = match run(&launch) {
                Ok(child) => child,
                Err(error) => {
                    fail(&state, format!("Cannot start {}: {error}", launch.program.display()));
                    let _ = cx.update(|window, _| window.refresh());
                    return;
                }
            };
            let pid = child.id();
            state.borrow_mut().status = "Waiting for its window…".into();
            drop(state);
            let opened = Instant::now();
            // A pane that goes away (a mod restarting) leaves a started
            // process of its own: its window waits for the pane's next state.
            let mut abandoned = false;
            // Adopt the new window.
            let found = loop {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                if !abandoned && wanted(&weak, generation).is_none() {
                    if !launch.own_process { return; }
                    abandoned = true;
                }
                let mut now = program_windows(&launch);
                if launch.own_process { now.retain(|&hwnd| window_pid(hwnd) == pid); }
                if let Some(&hwnd) = now.iter().find(|h| !before.contains(h)) { break hwnd; }
                // A program that already shows this folder brings that window
                // to the front instead of opening another. Windows may not let
                // it come forward: then it is found by its title.
                let front = unsafe { GetForegroundWindow() };
                if opened.elapsed() > Duration::from_secs(2) && front != foreground && now.contains(&front) { break front; }
                if opened.elapsed() > Duration::from_secs(3) && let Some(hwnd) = showing(&now, &launch.title) { break hwnd; }
                let failed = match child.try_wait() {
                    Ok(Some(status)) => launch.own_process || !status.success(),
                    _ => false,
                };
                if failed || opened.elapsed() > Duration::from_secs(20) {
                    if !abandoned && let Some(state) = weak.upgrade() {
                        let mut app = state.borrow_mut();
                        app.opening = false;
                        app.error = Some(format!("{} did not open a window", launch.program.file_name().unwrap_or_default().to_string_lossy()));
                        let _ = cx.update(|window, _| window.refresh());
                    }
                    return;
                }
            };
            let mut embedded = embed(found, host);
            let child = found as usize;
            // The container stays hidden until the crop is known.
            embedded.crop = cx.background_executor().spawn(async move { measure(child as HWND) }).await;
            let state = if abandoned { None } else { wanted(&weak, generation) };
            let Some(state) = state else { orphan(&app, &target, embedded); return };
            drop(starting);
            show(&state, embedded);
            let _ = cx.update(|window, _| window.refresh());
            apply(&state);
            focus::give(found);
            // Only the pane holds its state: closing the pane drops it, and
            // with it the window.
            drop(state);
            watch_loop(weak, generation, child, cx).await;
        }).detach();
    }

    thread_local! {
        /// A program that reads its settings at start (Fork) is starting.
        static STARTING: Cell<bool> = const { Cell::new(false) };
    }

    /// Holds `STARTING` until dropped.
    struct Starting;
    impl Starting {
        fn new() -> Self { STARTING.set(true); Starting }
    }
    impl Drop for Starting {
        fn drop(&mut self) { STARTING.set(false); }
    }

    fn window_pid(hwnd: HWND) -> u32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        pid
    }

    fn run(launch: &Launch) -> std::io::Result<std::process::Child> {
        let mut command = std::process::Command::new(&launch.program);
        command.args(&launch.args)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        // Herdr may run inside VS Code or another Electron app: never pass
        // their IPC variables on, or Code.exe acts as a CLI or Node instead.
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("VSCODE_") || name.starts_with("ELECTRON_") { command.env_remove(&key); }
        }
        command.spawn()
    }

    fn fail(state: &State, error: String) {
        let mut app = state.borrow_mut();
        app.opening = false;
        app.error = Some(error);
    }

    /// The state, while it still wants this start's window. A state that
    /// moved on stops opening.
    fn wanted(weak: &std::rc::Weak<std::cell::RefCell<super::NativeApp>>, generation: u64) -> Option<State> {
        let state = weak.upgrade()?;
        if state.borrow().generation != generation {
            state.borrow_mut().opening = false;
            return None;
        }
        Some(state)
    }

    fn show(state: &State, embedded: Embedded) {
        let mut app = state.borrow_mut();
        app.opening = false;
        app.embedded = Some(embedded);
        app.status = "Hidden while its pane is covered.".into();
    }

    /// The window of the same program and target that a dropped state left or
    /// another state holds, moved into this host. That state ends as closed.
    fn take_over(state: &State, app: &str, target: &str, host: HWND) -> Option<Embedded> {
        let embedded = take_orphan(app, target).or_else(|| held(state, app, target))?;
        Some(moved_to(embedded, host))
    }

    fn take_orphan(app: &str, target: &str) -> Option<Embedded> {
        ORPHANS.with_borrow_mut(|o| o.iter().position(|(a, t, _, _)| a == app && t == target).map(|i| o.remove(i).2))
    }

    fn moved_to(mut embedded: Embedded, host: HWND) -> Embedded {
        embedded.applied = None;
        unsafe {
            if GetParent(embedded.container) != host { SetParent(embedded.container, host); }
        }
        embedded
    }

    /// The window another live state of the same program and target holds,
    /// taken from it. One pane per project: the newest one shows its window.
    fn held(state: &State, app: &str, target: &str) -> Option<Embedded> {
        let other = super::APPS.with_borrow(|apps| {
            apps.iter().filter_map(std::rc::Weak::upgrade).find(|other| {
                if Rc::ptr_eq(other, state) { return false; }
                let o = other.borrow();
                o.app == app && o.target == target && o.embedded.is_some()
            })
        })?;
        let mut o = other.borrow_mut();
        let embedded = o.embedded.take()?;
        o.generation += 1;
        o.closed = true;
        o.status = "Moved to another pane.".into();
        Some(embedded)
    }

    /// Watch an adopted window: closed by its own button, moved by its title
    /// bar, or a title bar that changed height.
    async fn watch_loop(weak: std::rc::Weak<std::cell::RefCell<super::NativeApp>>, generation: u64, child: usize, cx: &mut gpui::AsyncWindowContext) {
        for tick in 1u64.. {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let Some(state) = weak.upgrade() else { return };
            if state.borrow().generation != generation { return; }
            // Zoom or the command center can change the title bar's height.
            if tick % 8 == 0 {
                let crop = cx.background_executor().spawn(async move { measure(child as HWND) }).await;
                let mut app = state.borrow_mut();
                if let Some(e) = app.embedded.as_mut().filter(|e| e.crop != crop) {
                    tracing::debug!(?crop, "native app crop changed");
                    e.crop = crop;
                    e.applied = None;
                }
            }
            if watch(&state) {
                let _ = cx.update(|window, _| window.refresh());
            }
            if state.borrow().embedded.is_none() { return; }
        }
    }

    /// One watcher step. Returns whether to redraw.
    fn watch(state: &State) -> bool {
        let mut app = state.borrow_mut();
        let Some(embedded) = app.embedded.as_mut() else { return false };
        if unsafe { IsWindow(embedded.child) } == 0 || unsafe { GetParent(embedded.child) } != embedded.container {
            // Closed by the program: nothing to give back.
            let e = std::mem::ManuallyDrop::new(app.embedded.take().unwrap());
            app.closed = true;
            app.status = "Closed.".into();
            drop(app);
            focus::forget(e.container);
            unsafe { DestroyWindow(e.container) };
            return true;
        }
        // Its own minimize button: show it again in the pane (posted, so no
        // message reaches this process while the state is borrowed).
        if unsafe { IsIconic(embedded.child) } != 0 {
            unsafe { ShowWindowAsync(embedded.child, SW_SHOWNOACTIVATE) };
            embedded.applied = None;
        }
        // The program may move or resize its own window: put it back.
        if let Some((rect, _, _)) = &embedded.applied {
            let expected = child_rect(rect[2], rect[3], embedded.crop);
            if current(embedded.child) != expected { embedded.applied = None; }
        }
        drop(app);
        apply(state);
        false
    }

    /// The child's place inside a container of `width` x `height`.
    fn child_rect(width: i32, height: i32, crop: Crop) -> [i32; 4] {
        [-crop.left, -crop.top, width + crop.left + crop.right, height + crop.top + crop.bottom]
    }

    /// The window's rectangle in its parent's client coordinates.
    fn current(hwnd: HWND) -> [i32; 4] {
        let r = screen_rect(hwnd);
        let mut origin = POINT { x: r.left, y: r.top };
        unsafe {
            let parent = GetParent(hwnd);
            let mut zero = POINT { x: 0, y: 0 };
            ClientToScreen(parent, &mut zero);
            origin.x -= zero.x;
            origin.y -= zero.y;
        }
        [origin.x, origin.y, r.right - r.left, r.bottom - r.top]
    }

    fn image_path(pid: u32) -> Option<String> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() { return None; }
            let mut buffer = [0u16; 1024];
            let mut len = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len);
            CloseHandle(process);
            (ok != 0).then(|| String::from_utf16_lossy(&buffer[..len as usize]))
        }
    }

    /// The one window whose title has `part` as a " - "-separated part.
    fn showing(windows: &[HWND], part: &str) -> Option<HWND> {
        if part.is_empty() { return None; }
        let matches: Vec<HWND> = windows.iter().copied().filter(|&hwnd| {
            let mut text = [0u16; 512];
            let len = unsafe { GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32) };
            let title = String::from_utf16_lossy(&text[..len.max(0) as usize]);
            title.split(" - ").any(|p| p == part || p.strip_prefix(part).is_some_and(|rest| rest.starts_with(" [")))
        }).collect();
        (matches.len() == 1).then(|| matches[0])
    }

    /// Visible, unowned top-level windows of the launch's program.
    fn program_windows(launch: &Launch) -> Vec<HWND> {
        struct Search<'a> { class: &'a str, program: String, found: Vec<HWND>, checked: Vec<(u32, bool)> }
        unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> windows_sys::core::BOOL {
            let search = unsafe { &mut *(data as *mut Search) };
            if unsafe { IsWindowVisible(hwnd) } == 0 || !unsafe { GetWindow(hwnd, GW_OWNER) }.is_null() { return 1; }
            // The class first: a title read can send a message to a window
            // of this process, such as Herdr's own.
            let mut name = [0u16; 128];
            let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
            if !super::class_matches(search.class, &String::from_utf16_lossy(&name[..len.max(0) as usize])) { return 1; }
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if pid == std::process::id() || unsafe { GetWindowTextLengthW(hwnd) } == 0 { return 1; }
            let ours = match search.checked.iter().find(|(p, _)| *p == pid) {
                Some(&(_, ours)) => ours,
                None => {
                    let ours = image_path(pid).is_some_and(|path| path.eq_ignore_ascii_case(&search.program));
                    search.checked.push((pid, ours));
                    ours
                }
            };
            if ours { search.found.push(hwnd); }
            1
        }
        let program = std::fs::canonicalize(&launch.program).map(|p| p.to_string_lossy().trim_start_matches(r"\\?\").to_owned())
            .unwrap_or_else(|_| launch.program.to_string_lossy().into_owned());
        let mut search = Search { class: launch.class, program, found: Vec::new(), checked: Vec::new() };
        unsafe { EnumWindows(Some(visit), (&raw mut search) as LPARAM) };
        search.found
    }

    /// Move the program's top-level window into a new container in `host`.
    fn embed(hwnd: HWND, host: HWND) -> Embedded {
        unsafe {
            let container = container::create(host);
            if IsIconic(hwnd) != 0 || IsZoomed(hwnd) != 0 { ShowWindow(hwnd, SW_RESTORE); }
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            // A borderless child. WS_MAXIMIZE makes Chromium treat its edges as
            // a maximized window's: no resize borders to drag.
            let child_style = (style as u32 & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU))
                | WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | WS_MAXIMIZE;
            SetWindowLongPtrW(hwnd, GWL_STYLE, child_style as isize);
            let child_ex = ex_style as u32 & !(WS_EX_APPWINDOW | WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_DLGMODALFRAME | WS_EX_NOPARENTNOTIFY | WS_EX_TOPMOST);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, child_ex as isize);
            // Herdr's flip-model swap chain covers child windows that draw
            // into a redirection surface (WPF, as in Fork). A layered
            // container gets its own DWM surface above it. Programs with
            // their own composition (Chromium) need none.
            if ex_style as u32 & WS_EX_NOREDIRECTIONBITMAP == 0 {
                SetWindowLongPtrW(container, GWL_EXSTYLE, GetWindowLongPtrW(container, GWL_EXSTYLE) | WS_EX_LAYERED as isize);
                SetLayeredWindowAttributes(container, 0, 255, LWA_ALPHA);
            }
            SetParent(hwnd, container);
            SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            focus::adopt(host, container, hwnd);
            Embedded { child: hwnd, container, style, ex_style, crop: Crop::default(), applied: None }
        }
    }

    /// Give the container the pane's latest geometry, visibility and holes.
    ///
    /// Runs outside App updates, and makes its native calls without any
    /// borrow: SetFocus and SetWindowPos deliver messages to Herdr's own
    /// window at once, which can draw a frame that reads this state again.
    pub(super) fn apply(state: &State) {
        let (child, container, crop, rect, visible, holes, give) = {
            let mut app = state.borrow_mut();
            app.apply_pending = false;
            if !app.needs_apply() { return; }
            let (rect, visible, holes) = (app.rect.unwrap(), app.visible, app.holes.clone());
            let give = visible && app.focus_until.take().is_some_and(|until| std::time::Instant::now() <= until);
            app.status = if visible { "Shown.".into() } else { "Hidden while its pane is covered.".into() };
            let embedded = app.embedded.as_mut().unwrap();
            embedded.applied = Some((rect, visible, holes.clone()));
            (embedded.child, embedded.container, embedded.crop, rect, visible, holes, give)
        };
        let width = rect[2].max(1);
        let height = rect[3].max(1);
        unsafe {
            if !visible { focus::release(container, child); }
            let show = if visible { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
            SetWindowPos(container, HWND_TOP, rect[0], rect[1], width, height, SWP_NOACTIVATE | show);
        }
        super::super::win32::set_holes(container, width, height, &holes);
        let [x, y, w, h] = child_rect(width, height, crop);
        unsafe { SetWindowPos(child, std::ptr::null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
        if give { focus::give(child); }
    }

    /// The window's frame: what lies outside its client area. Everything the
    /// program draws stays, including VS Code's title bar and the few pixels
    /// at its edges where it reports resize borders. A resize from there is
    /// undone by the watcher. No messages are sent to the other process.
    fn measure(hwnd: HWND) -> Crop {
        unsafe {
            let window = screen_rect(hwnd);
            let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetClientRect(hwnd, &mut client);
            let mut origin = POINT { x: 0, y: 0 };
            ClientToScreen(hwnd, &mut origin);
            Crop {
                left: origin.x - window.left,
                top: origin.y - window.top,
                right: window.right - (origin.x + client.right),
                bottom: window.bottom - (origin.y + client.bottom),
            }
        }
    }

    /// Herdr's container windows for adopted windows.
    mod container {
        use super::*;
        use std::sync::Once;

        const CLASS: &str = "HerdrNativeAppHost";

        fn class_name() -> Vec<u16> { CLASS.encode_utf16().chain([0]).collect() }

        pub fn create(host: HWND) -> HWND {
            static REGISTER: Once = Once::new();
            let name = class_name();
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                REGISTER.call_once(|| {
                    let class = WNDCLASSEXW {
                        cbSize: size_of::<WNDCLASSEXW>() as u32,
                        lpfnWndProc: Some(proc),
                        hInstance: instance,
                        lpszClassName: name.as_ptr(),
                        ..std::mem::zeroed()
                    };
                    RegisterClassExW(&class);
                });
                CreateWindowExW(0, name.as_ptr(), std::ptr::null(), WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                    0, 0, 1, 1, host, std::ptr::null_mut(), instance, std::ptr::null())
            }
        }

        unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            match msg {
                // Keyboard focus passes through to the adopted window.
                WM_SETFOCUS => {
                    let child = unsafe { GetWindow(hwnd, GW_CHILD) };
                    if !child.is_null() { unsafe { SetFocus(child) }; }
                    0
                }
                // The adopted window paints everything; no flash of a background.
                WM_ERASEBKGND => 1,
                _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
            }
        }
    }

    /// Keyboard focus between Herdr and adopted windows.
    ///
    /// A Chromium window takes keyboard focus when its top-level window is
    /// activated, which a child window never is. Herdr's window is subclassed
    /// instead: a click on an adopted window (WM_PARENTNOTIFY) gives it focus,
    /// a click on Herdr's own surface takes focus back, and activating Herdr
    /// returns focus to the adopted window that had it. Reparenting attached
    /// both processes' input queues, so SetFocus works across them.
    mod focus {
        use std::cell::{Cell, RefCell};
        use windows_sys::Win32::{
            Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
            Graphics::Gdi::ScreenToClient,
            UI::Input::KeyboardAndMouse::{GetFocus, SetFocus},
            UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            UI::WindowsAndMessaging::*,
        };

        const ID: usize = 0x4845_5244; // "HERD"

        thread_local! {
            /// (host, container, adopted window).
            static EMBEDDED: RefCell<Vec<(HWND, HWND, HWND)>> = const { RefCell::new(Vec::new()) };
            /// The adopted window that should have focus while Herdr is active.
            static ACTIVE: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
        }

        pub fn adopt(host: HWND, container: HWND, hwnd: HWND) {
            let first = EMBEDDED.with_borrow_mut(|list| {
                let first = !list.iter().any(|&(h, _, _)| h == host);
                list.push((host, container, hwnd));
                first
            });
            if first { unsafe { SetWindowSubclass(host, Some(proc), ID, 0) }; }
        }

        pub fn forget(container: HWND) {
            EMBEDDED.with_borrow_mut(|list| {
                if let Some(&(_, _, hwnd)) = list.iter().find(|&&(_, c, _)| c == container) {
                    if ACTIVE.get() == hwnd { ACTIVE.set(std::ptr::null_mut()); }
                }
                list.retain(|&(_, c, _)| c != container);
            });
        }

        /// The window is hidden: its typing goes back to Herdr.
        pub fn release(container: HWND, hwnd: HWND) {
            if ACTIVE.get() == hwnd { ACTIVE.set(std::ptr::null_mut()); }
            let focus = unsafe { GetFocus() };
            if focus == container || focus == hwnd || unsafe { IsChild(container, focus) } != 0 {
                unsafe { SetFocus(GetParent(container)) };
            }
        }

        pub fn give(hwnd: HWND) {
            ACTIVE.set(hwnd);
            unsafe { SetFocus(hwnd) };
        }

        fn adopted_at(host: HWND, point: POINT) -> Option<HWND> {
            let child = unsafe { ChildWindowFromPointEx(host, point, CWP_SKIPINVISIBLE) };
            EMBEDDED.with_borrow(|list| list.iter().find(|&&(h, c, _)| h == host && c == child).map(|&(_, _, w)| w))
        }

        unsafe extern "system" fn proc(host: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
            match msg {
                WM_PARENTNOTIFY => {
                    let event = (wparam & 0xffff) as u32;
                    if matches!(event, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_POINTERDOWN) {
                        let mut point = POINT { x: (lparam & 0xffff) as i16 as i32, y: ((lparam >> 16) & 0xffff) as i16 as i32 };
                        // Pointer notifications carry screen coordinates; mouse ones the host's client coordinates.
                        if event == WM_POINTERDOWN { unsafe { ScreenToClient(host, &mut point) }; }
                        if let Some(hwnd) = adopted_at(host, point) { give(hwnd); }
                    }
                }
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_NCLBUTTONDOWN => {
                    // Herdr's own surface: its keys go to Herdr again.
                    ACTIVE.set(std::ptr::null_mut());
                    if unsafe { GetFocus() } != host { unsafe { SetFocus(host) }; }
                }
                WM_SETFOCUS => {
                    let result = unsafe { DefSubclassProc(host, msg, wparam, lparam) };
                    let active = ACTIVE.get();
                    if !active.is_null() && unsafe { IsWindow(active) } != 0 && unsafe { IsWindowVisible(active) } != 0 {
                        unsafe { SetFocus(active) };
                    }
                    return result;
                }
                WM_NCDESTROY => {
                    EMBEDDED.with_borrow_mut(|list| list.retain(|&(h, _, _)| h != host));
                    unsafe { RemoveWindowSubclass(host, Some(proc), ID) };
                }
                _ => {}
            }
            unsafe { DefSubclassProc(host, msg, wparam, lparam) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{class_matches, fork_workspace, safe_directory, windows_folder};
    use serde_json::json;

    #[test]
    fn fork_starts_in_a_workspace_of_one_repository() {
        let mut settings = json!({
            "Theme": 1,
            "Workspaces": {
                "All": [
                    { "Name": "Work", "Repositories": [r"C:\work"], "ActiveRepository": r"C:\work" },
                    { "Name": "Herdr", "Repositories": [r"C:\old", r"C:\other"], "ActiveRepository": r"C:\old", "Color": 3 },
                ],
                "ActiveWorkspace": "Work",
                "ShowInTitle": false,
            },
        });
        fork_workspace(&mut settings, r"C:\new").unwrap();
        assert_eq!(settings, json!({
            "Theme": 1,
            "Workspaces": {
                "All": [
                    { "Name": "Work", "Repositories": [r"C:\work"], "ActiveRepository": r"C:\work" },
                    { "Name": "Herdr", "Repositories": [r"C:\new"], "ActiveRepository": r"C:\new", "Color": 3 },
                ],
                "ActiveWorkspace": "Herdr",
                "ShowInTitle": false,
            },
        }));

        let mut fresh = json!({ "Theme": 1 });
        fork_workspace(&mut fresh, r"\\wsl.localhost\Ubuntu\root\app").unwrap();
        assert_eq!(fresh["Workspaces"], json!({
            "All": [{ "Name": "Herdr", "Repositories": [r"\\wsl.localhost\Ubuntu\root\app"], "ActiveRepository": r"\\wsl.localhost\Ubuntu\root\app" }],
            "ActiveWorkspace": "Herdr",
        }));
        assert!(fork_workspace(&mut json!([]), r"C:\new").is_err());
    }

    #[test]
    fn wsl_repositories_become_safe_directories() {
        assert_eq!(safe_directory(r"\\wsl.localhost\Ubuntu\root\dev\herdr-app").as_deref(), Some("%(prefix)///wsl.localhost/Ubuntu/root/dev/herdr-app"));
        assert_eq!(safe_directory(r"\\server\share\app").as_deref(), Some("%(prefix)///server/share/app"));
        assert_eq!(safe_directory(r"C:\dev\app"), None);
    }

    #[test]
    fn folders_become_windows_paths() {
        assert_eq!(windows_folder(r"C:\dev\app").unwrap(), r"C:\dev\app");
        assert_eq!(windows_folder("file:///C:/dev/my%20app").unwrap(), r"C:\dev\my app");
        assert_eq!(windows_folder("file://server/share/app").unwrap(), r"\\server\share\app");
        assert_eq!(windows_folder("vscode-remote://wsl+Ubuntu/root/dev/c++").unwrap(), r"\\wsl.localhost\Ubuntu\root\dev\c++");
        assert_eq!(windows_folder("vscode-remote://wsl%2BUbuntu-24.04/root").unwrap(), r"\\wsl.localhost\Ubuntu-24.04\root");
        assert!(windows_folder("vscode-remote://ssh-remote+vps/root/app").is_err());
        assert!(windows_folder("  ").is_err());
    }

    #[test]
    fn classes_match_exactly_or_by_prefix() {
        assert!(class_matches("Chrome_WidgetWin_1", "Chrome_WidgetWin_1"));
        assert!(!class_matches("Chrome_WidgetWin_1", "Chrome_WidgetWin_10"));
        assert!(class_matches("HwndWrapper[Fork.exe;*", "HwndWrapper[Fork.exe;;f99ed41f-8634-42e2-ab0d-34b527cb1e44]"));
        assert!(!class_matches("HwndWrapper[Fork.exe;*", "HwndWrapper[Other.exe;;f99ed41f]"));
    }
}
