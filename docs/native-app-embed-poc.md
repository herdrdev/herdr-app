# Proof of concept: VS Code inside a Herdr tab (Windows)

Branch `poc/vscode-embed`. Not for release.

## What it does

The `vscode` mod opens a tab that starts VS Code as a Herdr subprocess and shows
VS Code's own native window inside the pane. It is the real editor, not a web
build: the same `Code.exe`, the same extensions folder (`~/.vscode/extensions`)
and a separate user-data folder under `$HERDR_APP_HOME/apps/vscode`, seeded once
with the person's `settings.json` and `keybindings.json`.

```sh
herdr-app do tab.new '{"name":"VS Code","mod":"vscode","props":{"folder":"C:\\path\\to\\project"}}'
```

Without `props.folder` the tab opens the focused terminal's folder.

## How it works (`src/browser/embed.rs`)

- **Process.** Code.exe starts in a job object with `KILL_ON_JOB_CLOSE`. Closing
  the tab sends `WM_CLOSE` (VS Code keeps unsaved editors through hot exit), then
  ends the whole tree. If Herdr crashes or is killed, Windows ends the tree.
- **Window.** A watcher finds the job's visible top-level `Chrome_WidgetWin_1`
  window, strips its frame and reparents it with `SetParent` as a child of the
  Herdr window. Herdr's layout pass gives it the pane's rectangle in physical
  pixels; native calls run in foreground tasks, not inside paint.
- **Visibility.** The same rule as the native browser: hidden while its tab is
  inactive, its pane is clipped, or a Herdr overlay is open.
- **Keyboard focus.** Chromium takes focus on activation of its top-level
  window, which a child never gets. The Herdr window is subclassed: a click on
  the embedded window (`WM_PARENTNOTIFY`) gives it focus, a click on Herdr's own
  surface takes it back, and reactivating Herdr returns focus to the embedded
  window that had it.
- **Restart.** VS Code is single-instance per user-data folder. A new Code.exe
  started while the old one is closing hands its request to the dying process
  and hangs, so a restart waits until the previous tree has ended.

The component is registered as `NativeAppState(app, folder)` / `NativeApp(state)`
in the `herdr-browser` module. Adding another program means adding a launch
entry (path, arguments, window class) to `launch()`.

## Verified on Windows 11 (ZinPC)

- The tab shows a working VS Code with the project folder open.
- Mouse: clicks in the explorer and the editor.
- Keyboard: typing in the editor; `Ctrl+Shift+P` and `Ctrl+Shift+X` reach VS
  Code when it has focus, and Herdr's command palette when Herdr has focus.
- Focus returns to the Herdr terminal after switching tabs or clicking it.
- An extension installed with the regular `code --install-extension` appears in
  the embedded editor.
- The window follows tab switches, a split beside a terminal, Herdr overlays and
  window resizes.
- Restart starts a fresh main process; VS Code restores its editors.
- Closing the tab ends all VS Code processes of that tab at once. Killing
  Herdr with `Stop-Process -Force` also ends them; the person's own VS Code is
  untouched.

## Known limits

- **One VS Code main process per user-data folder.** A second VS Code tab is
  expected to hand its window to the first tab's process (not tested), so plan
  on one tab for now. Options: a
  user-data folder per tab (separate state and settings), or one shared process
  whose windows Herdr distributes to tabs.
- **VS Code's title bar stays.** Its custom title bar (menu, layout controls,
  close button) is drawn inside the pane. Its close button ends the process; the
  tab then offers Restart. `window.titleBarStyle` or a trimmed layout could hide
  it.
- **Workspace trust.** The new user-data folder starts in Restricted Mode for
  every folder until it is trusted once.
- **Herdr shortcuts while VS Code has focus.** All keys go to VS Code. Shortcuts
  that must always reach Herdr (switching tabs) need a keyboard hook or an
  accelerator list.
- **Focus edge cases.** Not yet handled: drag-and-drop between Herdr and VS
  Code, IME, a VS Code dialog (they are separate top-level windows owned by the
  embedded one), several Herdr windows, DPI changes while moving between
  monitors.
- **Rendering.** Herdr already runs with `GPUI_DISABLE_DIRECT_COMPOSITION=true`
  for WebView2, which is also what lets a child HWND draw over the GPUI surface.
- Linux and macOS report that embedding is unsupported.
