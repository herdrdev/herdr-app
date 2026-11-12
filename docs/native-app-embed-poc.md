# VS Code, Fork and Unity inside a Herdr tab (Windows)

The `vscode` mod shows the person's own VS Code window in a tab. Herdr asks
VS Code for a new window on a folder and adopts it: the same profile, settings,
extensions and sign-in as any other VS Code window.

```sh
herdr-app do tab.new '{"name":"VS Code","mod":"vscode","props":{"uri":"vscode-remote://wsl+Ubuntu/root/app"}}'
```

`props.uri` is a VS Code folder URI; `props.folder` a local folder. Without
either, the tab opens the focused terminal's folder. The tab records itself in
`shared("vscode.tabs")` by URI, so a launcher can select an open project's tab
instead of opening another.

The `fork` mod does the same with Fork, the git client, and records its tabs
in `shared("fork.tabs")`:

```sh
herdr-app do tab.new '{"name":"Fork","mod":"fork","props":{"uri":"vscode-remote://wsl+Ubuntu/root/app"}}'
```

Fork takes a local folder, a `file://` URI or a WSL folder URI, which it opens
as `\\wsl.localhost\<distro>\<path>`. Other remote folders are refused.

The `unity` mod shows the Unity Editor on a local Unity project:

```sh
herdr-app do tab.new '{"name":"Unity","mod":"unity","props":{"folder":"C:\\dev\\Unity\\Game"}}'
```

It starts `<root>\<version>\Editor\Unity.exe -projectPath <folder>`, where
`<version>` is `m_EditorVersion` of the project's
`ProjectSettings/ProjectVersion.txt` and `<root>` is Unity Hub's install folder
(`%APPDATA%\UnityHub\secondaryInstallPath.json`) or its default
`C:\Program Files\Unity\Hub\Editor`. A project whose version is not installed
shows an error in the pane. The first start of a project imports all its
assets: the pane waits for the window for up to 30 minutes.

## How it works (`src/browser/embed.rs`)

- **Window.** Herdr runs `Code.exe --new-window` on the target and adopts the
  new top-level window of VS Code's process. When VS Code already shows that
  folder, it is adopted instead, found as the foreground window or by its title.
  One pane shows a folder: a new pane on a folder that another pane shows
  takes that window over.
- **A Fork of each pane's own.** `Fork.exe <folder>` hands the folder to a
  running Fork and exits, so each Fork pane starts `Fork.exe` without a folder:
  a new process, which restores the tabs of Fork's active workspace. Before
  the start, Herdr makes the "Herdr" workspace in
  `%LOCALAPPDATA%\Fork\settings.json` hold only the pane's repository and makes
  it active; other workspaces and settings stay. Fork starts one at a time, and
  a pane adopts only a window of the process it started. A Fork opened outside
  Herdr afterwards also starts in the "Herdr" workspace; its workspace menu
  switches back.
- **Unity.** Unity opens a project in one editor only. A pane adopts an editor
  window already showing the project (found by the project folder's name in
  its title) instead of starting another, and two panes on one project never
  start two editors: the later one waits and takes the window. Unity's main
  window has a Win32 menu bar, which a child window cannot have, so it stays
  a captionless popup inside the container, and the menu bar is not cropped.
  A popup shares no input queue with Herdr, so Herdr cannot give it focus;
  it takes the keyboard as the foreground window (see Keyboard focus).
  Unity's main thread is often busy for many seconds (loading, compiling,
  importing), so Herdr never waits on it: the restyling and reparenting run
  on a background thread and its moves are posted (`SWP_ASYNCWINDOWPOS`).
  Unity's floating windows (its other windows of the main window's class) are
  hidden while the pane is hidden and come back with it.
- **Safe repositories.** Windows git treats every WSL repository as owned by
  another user and refuses it. Before the start Herdr adds a UNC repository
  (`\\wsl.localhost\...`) to the global `safe.directory` list with Fork's own
  git, as Fork's "Mark repository as safe" does.
- **Mod restarts.** A dropped pane state (a mod reload, a layout change) keeps
  its window hidden for two seconds. The new copy of the pane takes it over;
  otherwise it closes. A Fork still starting when its pane goes away leaves its
  window the same way.
- **Container.** The window becomes a borderless child of a Herdr-owned container
  window. The container follows the pane and cuts away only the window frame
  outside its client area; everything VS Code draws stays. A move, resize or
  minimize from inside is undone within a quarter second.
- **Layered container.** Herdr draws through a flip-model swap chain, which
  covers child windows that draw into a redirection surface, such as WPF
  windows (Fork). For a window without `WS_EX_NOREDIRECTIONBITMAP` the
  container becomes an opaque layered window, which DWM composes on its own
  surface above the swap chain. Chromium windows compose themselves and keep a
  plain container.
- **Overlays.** Toasts, popups and dialogs mark their area with
  `browser::cover()`. Native views (VS Code and the browser) cut those areas
  out of themselves with a window region and stay visible around them. A
  dialog marks only itself, not its dimmed backdrop, so a native view stays
  shown around it, undimmed. The parts of a view outside its pane's clip are
  cut out the same way instead of hiding it; it hides only while a pane is
  dragged or nothing of it is left.
- **Keyboard focus.** The Herdr window is subclassed: a click on an adopted
  window gives it focus, a click on Herdr's own surface takes it back, and
  reactivating Herdr returns focus to the adopted window that had it. The
  `focus` mod command gives a shown VS Code tab the keyboard. Unity's popup
  gets no click notification and no focus from Herdr: a low-level mouse hook
  on a thread of its own sees a press inside its pane (outside overlay holes,
  where Herdr's window is on top) and Herdr makes the popup the foreground
  window. Hiding the pane gives the foreground back to Herdr.
- **Closing.** Closing the tab gives the window back to the desktop and closes it
  as its own close button would; unsaved changes prompt there. Quitting Herdr
  closes adopted windows the same way. VS Code itself keeps running; the
  pane's own Fork process exits with its window, and so does Unity.
- Native calls run outside App updates and without borrowed state: SetFocus and
  SetWindowPos deliver messages to Herdr's own window at once.
- **Screenshots** (`src/control.rs`). Adopted windows draw themselves, so the
  GPU frame of `herdr-app shot` has holes where they are. The shot fills them
  from `PrintWindow(PW_CLIENTONLY | PW_RENDERFULLCONTENT)` of Herdr's window,
  which the desktop composes even while other windows cover it: only the
  visible parts of Herdr's shown child windows (their window regions, which
  keep the holes cut for overlays) are taken from it. The capture runs off
  the window thread with a 5 s limit. A minimized window, a frame of another
  size or a failed capture leaves the native windows out; the reply names
  why in `left_out`.

## Mod hints

`"frameless": true` in `mod.json` drops the layout's pane header for that mod.

## Known limits

- Herdr shortcuts do not reach Herdr while VS Code has keyboard focus.
- Not yet handled: drag and drop between Herdr and VS Code, several Herdr
  windows, DPI changes while moving between monitors.
- Linux and macOS report that embedding is unsupported.
