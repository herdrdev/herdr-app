//! Proof of concept: a native program's own window shown inside a Herdr pane.
//!
//! On Windows the pane starts the program in a job object that it owns, finds
//! the program's top-level window, and reparents it as a Win32 child of the
//! Herdr window. Closing the pane (or quitting Herdr) ends the whole process
//! tree. Other platforms report that embedding is unsupported.
//!
//! Native calls on the embedded window send messages to another process.
//! They run in foreground tasks between App updates, never inside layout or
//! paint, so a reentrant message cannot meet a borrowed GPUI window.
use super::policy;
use gpui::{AnyElement, App, Bounds, HitboxBehavior, HitboxId, ParentElement, Pixels, Styled, Window, WindowId, canvas, div};
use gpui_shell::{ArgumentDescriptor, ArgumentSchema, ComponentArgument, ComponentDataValue, ComponentDescriptor, ComponentMaterializer, ComponentPayload, ComponentRegistry, ConstructorDescriptor, MaterializeRequest, StateDescriptor, StateMethodDescriptor};
use std::{cell::RefCell, path::PathBuf, rc::{Rc, Weak}, sync::Arc};

type State = Rc<RefCell<NativeApp>>;
thread_local! {
    static APPS: RefCell<Vec<Weak<RefCell<NativeApp>>>> = const { RefCell::new(Vec::new()) };
}

/// What to start and how to recognize its main window.
#[derive(Clone, Debug)]
struct Launch {
    program: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    /// Win32 class of the main window, when the program has a known one.
    #[cfg_attr(not(windows), allow(dead_code))]
    class: Option<&'static str>,
}

/// A rectangle in physical pixels, relative to the host window's client area.
type Rect = [i32; 4];

pub(super) struct NativeApp {
    window: WindowId,
    app: String,
    folder: String,
    status: String,
    error: Option<String>,
    hitbox: Option<HitboxId>,
    rect: Option<Rect>,
    visible: bool,
    /// What the native window last received.
    applied: Option<(Rect, bool)>,
    apply_pending: bool,
    /// Bumped by restart, so an older watcher task ends.
    generation: u64,
    /// The program exited by itself; only restart starts it again.
    stopped: bool,
    /// A redraw is scheduled to retry a start that had to wait.
    retry_pending: bool,
    #[cfg(windows)]
    process: Option<win::Process>,
}

fn launch(app: &str, folder: &str) -> anyhow::Result<Launch> {
    match app {
        "vscode" => vscode(folder),
        other => anyhow::bail!("Unknown native app {other:?}; this proof of concept knows \"vscode\""),
    }
}

/// VS Code with its own user-data folder, so it runs as a separate main
/// process that this pane owns instead of handing the request to an already
/// running VS Code. Extensions stay in the shared `~/.vscode/extensions`.
fn vscode(folder: &str) -> anyhow::Result<Launch> {
    let env = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    let candidates = [
        env("HERDR_VSCODE_EXE"),
        env("LOCALAPPDATA").map(|d| d.join("Programs/Microsoft VS Code/Code.exe")),
        env("ProgramFiles").map(|d| d.join("Microsoft VS Code/Code.exe")),
    ];
    let program = candidates.into_iter().flatten().find(|p| p.is_file())
        .ok_or_else(|| anyhow::anyhow!("VS Code is not installed (set HERDR_VSCODE_EXE to Code.exe)"))?;
    let data = crate::mods::app_dir().join("apps").join("vscode");
    seed_vscode_settings(&data);
    let mut args = vec!["--user-data-dir".to_owned(), data.to_string_lossy().into_owned(), "--new-window".to_owned()];
    let folder = folder.trim();
    let cwd = (!folder.is_empty() && std::path::Path::new(folder).is_dir()).then(|| PathBuf::from(folder));
    if let Some(cwd) = &cwd { args.push(cwd.to_string_lossy().into_owned()); }
    Ok(Launch { program, args, cwd, class: Some("Chrome_WidgetWin_1") })
}

/// The first start copies the person's VS Code settings and keybindings, so
/// the embedded editor looks and behaves like theirs. Later edits stay apart.
fn seed_vscode_settings(data: &std::path::Path) {
    let Some(source) = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Code").join("User")) else { return };
    let target = data.join("User");
    if std::fs::create_dir_all(&target).is_err() { return; }
    for file in ["settings.json", "keybindings.json"] {
        let to = target.join(file);
        if !to.exists() {
            let _ = std::fs::copy(source.join(file), to);
        }
    }
}

impl NativeApp {
    fn layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window) {
        self.hitbox = Some(window.insert_hitbox(bounds, HitboxBehavior::Normal).id);
        let scale = window.scale_factor();
        let px = |v: Pixels| (f32::from(v) * scale).round() as i32;
        self.rect = Some([px(bounds.origin.x), px(bounds.origin.y), px(bounds.size.width), px(bounds.size.height)]);
    }

    fn update_visibility(&mut self, window: &Window, overlay: bool) {
        let hitbox = self.hitbox.and_then(|id| window.prepainted_hitbox(id));
        let unclipped = hitbox.is_some_and(|hitbox| policy::is_unclipped_at_scale(&hitbox.bounds, &hitbox.content_mask.bounds, window.scale_factor()));
        self.visible = policy::should_show(hitbox.is_some(), unclipped, overlay);
    }

    /// The native state differs from the latest layout.
    fn needs_apply(&self) -> bool {
        #[cfg(windows)]
        if self.process.as_ref().is_some_and(|p| p.hwnd.is_some()) {
            return match self.rect { Some(rect) => self.applied != Some((rect, self.visible)), None => false };
        }
        false
    }
}

/// Start the program on first use. Runs between App updates on Windows.
fn start(state: &State, window: &mut Window, cx: &mut App) {
    #[cfg(windows)]
    win::start(state, window, cx);
    #[cfg(not(windows))]
    {
        let _ = (window, cx);
        let mut app = state.borrow_mut();
        if app.error.is_none() {
            app.error = Some("Embedding native apps is only implemented on Windows in this proof of concept.".into());
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

/// Called from the browser's end-of-frame canvas: show or hide each embedded
/// window like a native browser, for overlays, clipping and inactive tabs.
pub(super) fn end_frame(overlay: bool, window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let apps: Vec<State> = APPS.with_borrow_mut(|apps| {
        apps.retain(|weak| weak.strong_count() > 0);
        apps.iter().filter_map(Weak::upgrade).filter(|app| app.borrow().window == id).collect()
    });
    for app in apps {
        app.borrow_mut().update_visibility(window, overlay);
        schedule_apply(&app, window, cx);
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
        let message = {
            let app = state.borrow();
            app.error.clone().unwrap_or_else(|| app.status.clone())
        };
        let element = div().relative().size_full()
            .child(div().absolute().size_full().p(gpui::px(16.)).child(message))
            .child(canvas(move |bounds, window, cx| {
                state.borrow_mut().layout(bounds, window);
                schedule_apply(&state, window, cx);
            }, |_, _, _, _| {}).size_full());
        request.finish(element)
    }
}

pub(super) fn register(registry: &mut ComponentRegistry) {
    registry.register_state(StateDescriptor::new(
        "NativeAppState", "HerdrNativeAppState",
        vec![ArgumentDescriptor::new("app", ArgumentSchema::String), ArgumentDescriptor::new("folder", ArgumentSchema::String)],
        |args, window, _| {
            let (ComponentArgument::String(app), ComponentArgument::String(folder)) = (&args[0], &args[1]) else { unreachable!() };
            let state = Rc::new(RefCell::new(NativeApp {
                window: window.window_handle().window_id(), app: app.clone(), folder: folder.clone(),
                status: "Starting…".into(), error: None, hitbox: None, rect: None, visible: false,
                applied: None, apply_pending: false, generation: 0, stopped: false, retry_pending: false,
                #[cfg(windows)] process: None,
            }));
            APPS.with_borrow_mut(|apps| apps.push(Rc::downgrade(&state)));
            Ok(Box::new(state))
        },
    ).with_documentation("Proof of concept: a native program (\"vscode\") started in folder and shown in this pane. Its process tree ends with the state.")
    .with_methods(vec![
        StateMethodDescriptor::new::<State>("status", "(): string", |state, _, _, _| {
            Ok(ComponentDataValue::String(state.borrow().status.clone()))
        }).with_readonly(true),
        StateMethodDescriptor::new::<State>("error", "(): string | null", |state, _, _, _| {
            Ok(state.borrow().error.clone().map(ComponentDataValue::String).unwrap_or(ComponentDataValue::Null))
        }).with_readonly(true),
        StateMethodDescriptor::new::<State>("restart", "(): void", |state, _, _, _| {
            let mut app = state.borrow_mut();
            app.generation += 1;
            app.stopped = false;
            app.error = None;
            app.status = "Starting…".into();
            app.applied = None;
            #[cfg(windows)]
            { app.process = None; }
            Ok(ComponentDataValue::Null)
        }),
        StateMethodDescriptor::new::<State>("focus", "(): void", |state, _, _, _| {
            #[cfg(windows)]
            if let Some(hwnd) = state.borrow().process.as_ref().and_then(|p| p.hwnd) { win::focus(hwnd); }
            #[cfg(not(windows))]
            let _ = state;
            Ok(ComponentDataValue::Null)
        }),
    ])).unwrap();
    registry.register(ComponentDescriptor::new("NativeApp", Arc::new(NativeAppElement))
        .with_documentation("Proof of concept: the native window of a NativeAppState. Retain the state in init.")
        .with_constructors(vec![ConstructorDescriptor::new("NativeApp", vec![ArgumentDescriptor::new("state", ArgumentSchema::Entity("HerdrNativeAppState"))], |args| Ok(ComponentPayload::new(args[0].clone())))])
        .with_methods(vec![])).unwrap();
}

#[cfg(windows)]
mod win {
    use super::{Launch, NativeApp, Rect, State};
    use gpui::{App, Window};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{os::windows::io::AsRawHandle, rc::Rc, time::Duration};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT},
        Graphics::Gdi::MapWindowPoints,
        System::JobObjects::*,
        UI::WindowsAndMessaging::*,
    };

    /// Process trees still closing. VS Code is single-instance per user-data
    /// folder: a new Code.exe started while the old one shuts down hands its
    /// window request to the dying process and then waits forever.
    static CLOSING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    pub(super) struct Process {
        /// Taken by Drop, which hands it to the closing thread.
        child: Option<std::process::Child>,
        job: HANDLE,
        host: HWND,
        pub(super) hwnd: Option<HWND>,
    }

    impl Drop for Process {
        fn drop(&mut self) {
            // Ask the program to close like its own close button would, so VS
            // Code keeps unsaved editors (hot exit). Only posted messages here:
            // this can run inside an App update. Hide it at once; the tab is gone.
            if let Some(hwnd) = self.hwnd {
                unsafe {
                    ShowWindowAsync(hwnd, SW_HIDE);
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            }
            let job = self.job as usize;
            let graceful = self.hwnd.is_some();
            let Some(mut child) = self.child.take() else {
                unsafe { CloseHandle(self.job) };
                return;
            };
            // Then the whole tree: VS Code's renderers, extension hosts,
            // terminals. If Herdr quits first, closing the job's last handle
            // ends the tree too (KILL_ON_JOB_CLOSE).
            CLOSING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let closer = std::thread::Builder::new().name("native-app-close".into()).spawn(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(if graceful { 5 } else { 0 });
                while std::time::Instant::now() < deadline {
                    if let Ok(Some(_)) = child.try_wait() { break; }
                    std::thread::sleep(Duration::from_millis(100));
                }
                unsafe {
                    TerminateJobObject(job as HANDLE, 1);
                    CloseHandle(job as HANDLE);
                }
                let _ = child.wait();
                // The job's last process may outlive the launcher briefly.
                std::thread::sleep(Duration::from_millis(300));
                CLOSING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            });
            if closer.is_err() { CLOSING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst); }
        }
    }

    fn spawn(launch: &Launch, host: HWND) -> anyhow::Result<Process> {
        let mut command = std::process::Command::new(&launch.program);
        command.args(&launch.args);
        if let Some(cwd) = &launch.cwd { command.current_dir(cwd); }
        // Herdr may itself run inside VS Code or Electron: never pass their
        // IPC variables on, or Code.exe acts as a CLI or Node instead.
        for (key, _) in std::env::vars_os() {
            let key_str = key.to_string_lossy();
            if key_str.starts_with("VSCODE_") || key_str.starts_with("ELECTRON_") { command.env_remove(&key); }
        }
        let child = command.spawn().map_err(|e| anyhow::anyhow!("cannot start {}: {e}", launch.program.display()))?;
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        anyhow::ensure!(!job.is_null(), "cannot create a job object: {}", std::io::Error::last_os_error());
        // SAFETY: a plain C struct; all-zero means no limits.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let mut process = Process { child: None, job, host, hwnd: None };
        unsafe {
            if SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const limits).cast(), size_of_val(&limits) as u32) == 0
                || AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0
            {
                let error = std::io::Error::last_os_error();
                let mut child = child;
                let _ = child.kill();
                anyhow::bail!("cannot own the process tree: {error}");
            }
        }
        process.child = Some(child);
        Ok(process)
    }

    pub(super) fn start(state: &State, window: &mut Window, cx: &mut App) {
        let (generation, app, folder) = {
            let app = state.borrow();
            if app.process.is_some() || app.error.is_some() || app.stopped { return; }
            (app.generation, app.app.clone(), app.folder.clone())
        };
        if CLOSING.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            let mut app = state.borrow_mut();
            app.status = "Waiting for the previous window to close…".into();
            if !app.retry_pending {
                app.retry_pending = true;
                let weak = Rc::downgrade(state);
                window.spawn(cx, async move |cx| {
                    cx.background_executor().timer(Duration::from_millis(200)).await;
                    if let Some(state) = weak.upgrade() { state.borrow_mut().retry_pending = false; }
                    let _ = cx.update(|window, _| window.refresh());
                }).detach();
            }
            return;
        }
        let host = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get() as HWND,
            _ => { state.borrow_mut().error = Some("No Win32 host window".into()); return; }
        };
        let launched = super::launch(&app, &folder).and_then(|launch| spawn(&launch, host).map(|p| (p, launch.class)));
        let class = match launched {
            Ok((process, class)) => {
                let mut app = state.borrow_mut();
                app.status = "Waiting for its window…".into();
                app.process = Some(process);
                class
            }
            Err(error) => { state.borrow_mut().error = Some(format!("{error:#}")); return; }
        };
        let weak = Rc::downgrade(state);
        window.spawn(cx, async move |cx| {
            let mut found = false;
            loop {
                let delay = if found { 250 } else { 100 };
                cx.background_executor().timer(Duration::from_millis(delay)).await;
                let Some(state) = weak.upgrade() else { return };
                if state.borrow().generation != generation { return; }
                let changed = watch(&state, class);
                found = state.borrow().process.as_ref().is_some_and(|p| p.hwnd.is_some());
                apply(&state);
                if changed {
                    let _ = cx.update(|window, _| window.refresh());
                }
                if state.borrow().process.is_none() { return; }
            }
        }).detach();
    }

    /// One watcher step: notice an exit, find or lose the window, keep the
    /// geometry the program may have changed itself. Returns whether to redraw.
    fn watch(state: &State, class: Option<&'static str>) -> bool {
        let (job, host, hwnd) = {
            let mut app = state.borrow_mut();
            let Some(process) = app.process.as_mut() else { return false };
            if let Some(Ok(Some(status))) = process.child.as_mut().map(|child| child.try_wait()) {
                app.process = None;
                app.stopped = true;
                app.status = format!("Exited ({status}). Restart to open it again.");
                return true;
            }
            (process.job, process.host, process.hwnd)
        };
        match hwnd {
            Some(hwnd) if unsafe { IsWindow(hwnd) } == 0 => {
                let mut app = state.borrow_mut();
                if let Some(p) = app.process.as_mut() { p.hwnd = None; }
                app.applied = None;
                app.status = "Waiting for its window…".into();
                true
            }
            Some(hwnd) => {
                // The program may move or resize its own window (restoring its
                // saved bounds, a double-click on its title bar). Put it back.
                let expected = state.borrow().applied.map(|(rect, _)| rect);
                if let Some(expected) = expected {
                    if current_rect(hwnd, host) != Some(expected) { state.borrow_mut().applied = None; }
                }
                false
            }
            None => {
                let Some(found) = find_window(&job_pids(job), class) else { return false };
                embed(found, host);
                let mut app = state.borrow_mut();
                if let Some(p) = app.process.as_mut() { p.hwnd = Some(found); }
                app.applied = None;
                app.status = "Hidden while its pane is covered.".into();
                true
            }
        }
    }

    fn current_rect(hwnd: HWND, host: HWND) -> Option<Rect> {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            if GetWindowRect(hwnd, &mut r) == 0 { return None; }
            let mut points = [windows_sys::Win32::Foundation::POINT { x: r.left, y: r.top }, windows_sys::Win32::Foundation::POINT { x: r.right, y: r.bottom }];
            MapWindowPoints(std::ptr::null_mut(), host, points.as_mut_ptr(), 2);
            Some([points[0].x, points[0].y, points[1].x - points[0].x, points[1].y - points[0].y])
        }
    }

    fn job_pids(job: HANDLE) -> Vec<u32> {
        #[repr(C)]
        struct List { assigned: u32, listed: u32, ids: [usize; 256] }
        let mut list = List { assigned: 0, listed: 0, ids: [0; 256] };
        let ok = unsafe { QueryInformationJobObject(job, JobObjectBasicProcessIdList, (&raw mut list).cast(), size_of::<List>() as u32, std::ptr::null_mut()) };
        if ok == 0 { return Vec::new(); }
        list.ids[..list.listed as usize].iter().map(|&id| id as u32).collect()
    }

    /// A visible, unowned top-level window of a process in the job.
    fn find_window(pids: &[u32], class: Option<&str>) -> Option<HWND> {
        struct Search<'a> { pids: &'a [u32], class: Option<&'a str>, found: Option<HWND> }
        unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> windows_sys::core::BOOL {
            let search = unsafe { &mut *(data as *mut Search) };
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if !search.pids.contains(&pid) || unsafe { IsWindowVisible(hwnd) } == 0 { return 1; }
            if !unsafe { GetWindow(hwnd, GW_OWNER) }.is_null() || unsafe { GetWindowTextLengthW(hwnd) } == 0 { return 1; }
            if let Some(class) = search.class {
                let mut name = [0u16; 64];
                let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
                if String::from_utf16_lossy(&name[..len.max(0) as usize]) != class { return 1; }
            }
            search.found = Some(hwnd);
            0
        }
        if pids.is_empty() { return None; }
        let mut search = Search { pids, class, found: None };
        unsafe { EnumWindows(Some(visit), (&raw mut search) as LPARAM) };
        search.found
    }

    /// Turn the program's top-level window into a borderless child of `host`.
    fn embed(hwnd: HWND, host: HWND) {
        unsafe {
            if IsZoomed(hwnd) != 0 || IsIconic(hwnd) != 0 { ShowWindow(hwnd, SW_RESTORE); }
            ShowWindow(hwnd, SW_HIDE);
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            let style = (style & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU)) | WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
            SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let ex = ex & !(WS_EX_APPWINDOW | WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_DLGMODALFRAME | WS_EX_NOPARENTNOTIFY);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
            SetParent(hwnd, host);
            focus::adopt(host, hwnd);
            SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_NOACTIVATE);
        }
    }

    /// Give the embedded window the latest geometry and visibility.
    pub(super) fn apply(state: &State) {
        let (hwnd, host, rect, visible) = {
            let mut app = state.borrow_mut();
            app.apply_pending = false;
            if !app.needs_apply() { return; }
            let NativeApp { process: Some(process), rect: Some(rect), visible, .. } = &*app else { return };
            (process.hwnd.unwrap(), process.host, *rect, *visible)
        };
        unsafe {
            if !visible {
                // Typing must not keep going to a window nobody sees.
                focus::release(host, hwnd);
            }
            let show = if visible { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
            SetWindowPos(hwnd, HWND_TOP, rect[0], rect[1], rect[2].max(1), rect[3].max(1), SWP_NOACTIVATE | show);
        }
        let mut app = state.borrow_mut();
        app.applied = Some((rect, visible));
        if visible { app.status = "Running.".into(); } else if app.process.is_some() { app.status = "Hidden while its pane is covered.".into(); }
    }

    pub(super) fn focus(hwnd: HWND) {
        focus::give(hwnd);
    }

    /// Keyboard focus between Herdr and embedded windows.
    ///
    /// A Chromium window takes keyboard focus when its top-level window is
    /// activated, which a child window never is. Herdr's window is subclassed
    /// instead: a click on an embedded window (WM_PARENTNOTIFY) gives it focus,
    /// a click on Herdr's own surface takes focus back, and activating Herdr
    /// returns focus to the embedded window that had it. Reparenting attached
    /// both processes' input queues, so SetFocus works across them.
    mod focus {
        use std::cell::{Cell, RefCell};
        use windows_sys::Win32::{
            Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
            UI::Input::KeyboardAndMouse::{GetFocus, SetFocus},
            UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            UI::WindowsAndMessaging::*,
        };

        const ID: usize = 0x4845_5244; // "HERD"

        thread_local! {
            /// Embedded windows, by host.
            static EMBEDDED: RefCell<Vec<(HWND, HWND)>> = const { RefCell::new(Vec::new()) };
            /// The embedded window that should have focus while Herdr is active.
            static ACTIVE: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
        }

        pub fn adopt(host: HWND, hwnd: HWND) {
            let first = EMBEDDED.with_borrow_mut(|list| {
                list.retain(|&(_, h)| unsafe { IsWindow(h) } != 0);
                let first = !list.iter().any(|&(o, _)| o == host);
                list.push((host, hwnd));
                first
            });
            if first { unsafe { SetWindowSubclass(host, Some(proc), ID, 0) }; }
        }

        /// The window is hidden: its typing goes back to Herdr.
        pub fn release(host: HWND, hwnd: HWND) {
            if ACTIVE.get() == hwnd { ACTIVE.set(std::ptr::null_mut()); }
            let focus = unsafe { GetFocus() };
            if focus == hwnd || unsafe { IsChild(hwnd, focus) } != 0 { unsafe { SetFocus(host) }; }
        }

        pub fn give(hwnd: HWND) {
            ACTIVE.set(hwnd);
            unsafe { SetFocus(hwnd) };
        }

        fn embedded_at(host: HWND, point: POINT) -> Option<HWND> {
            let child = unsafe { ChildWindowFromPointEx(host, point, CWP_SKIPINVISIBLE) };
            EMBEDDED.with_borrow(|list| list.iter().find(|&&(o, h)| o == host && h == child).map(|&(_, h)| h))
        }

        unsafe extern "system" fn proc(host: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
            match msg {
                WM_PARENTNOTIFY => {
                    let event = (wparam & 0xffff) as u32;
                    if matches!(event, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_POINTERDOWN) {
                        let mut point = POINT { x: (lparam & 0xffff) as i16 as i32, y: ((lparam >> 16) & 0xffff) as i16 as i32 };
                        // Pointer notifications carry screen coordinates; mouse ones the host's client coordinates.
                        if event == WM_POINTERDOWN { unsafe { windows_sys::Win32::Graphics::Gdi::ScreenToClient(host, &mut point) }; }
                        if let Some(hwnd) = embedded_at(host, point) { give(hwnd); }
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
                    EMBEDDED.with_borrow_mut(|list| list.retain(|&(o, _)| o != host));
                    unsafe { RemoveWindowSubclass(host, Some(proc), ID) };
                }
                _ => {}
            }
            unsafe { DefSubclassProc(host, msg, wparam, lparam) }
        }
    }
}
