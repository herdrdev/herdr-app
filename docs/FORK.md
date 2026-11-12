# Personal browser fork

This fork follows `herdrdev/herdr-app` and adds a native HTTP(S) browser component
for mods. Plannotator reviews open in their own tab in the requesting Pi terminal's space.
Pages have no Herdr command bridge. Existing layouts, terminal storage and core
protocols are unchanged.

## Build and install

Use the [build requirements](../README.md#build-from-source) and install the
[Linux runtime dependencies](../README.md#linux-and-macos) when running a Linux
binary. Windows needs the WebView2 runtime. The browser uses the system webview;
it does not bundle Chromium.

Build and install with `just install`. Quit and reopen the app yourself to use the
new binary. Installing a build must not stop the core or other people's agents.
On Linux, start the app on X11/XWayland (`env -u WAYLAND_DISPLAY herdr-app`).
Native Wayland embedding is not supported by this adapter.

Check the binary before configuring an agent:

```sh
herdr-app --browser-api
# {"module":"herdr-browser","version":1}
```

## Install a release

Every master push that changes the app builds a Windows x64 release on GitHub
(`.github/workflows/fork-release.yml`); nothing is signed. Install or update
from PowerShell, without building:

```powershell
powershell -c "irm https://github.com/alexshpunt/herdr-app/releases/latest/download/install.ps1 | iex"
```

The installer verifies the build's SHA-256, replaces `%LOCALAPPDATA%\Programs\herdr-app`
(a running app keeps running until it quits) and installs herdr-core only if
there is none. These builds do not update themselves, because a release build
of upstream's updater would replace the fork with the upstream app; run the
installer again for a newer build.

Upstream's `release.yml` publishes only for its maintainer and is disabled in
this fork (`gh workflow disable release.yml`). Do not re-enable it.

## Follow upstream

`sync-upstream.yml` merges `herdrdev/herdr-app` into master every day, runs CI
on the merge, fast-forwards master and starts the fork release.

When the merge conflicts, `scripts/fork-resolve-conflicts` has a cheap model
(`opencode-go/deepseek-v4.1-flash` through pi, the `OPENCODE_API_KEY` repository
secret) edit the conflicted files. `scripts/fork-sync.py` rejects the result if
any conflict marker is left, and the candidate still has to pass the whole CI
before master moves, so a wrong resolution fails instead of landing. When the
resolver or CI fails, the workflow opens an issue "Upstream sync is failing";
resolve it by merging `upstream/master` into a branch by hand and landing it
through a PR (merge commit, not squash).

## Connect Pi and Plannotator

Install Plannotator and this checkout's integration into each Pi environment:

```sh
pi install npm:@plannotator/pi-extension
pi install /absolute/path/to/herdr-app/integrations/plannotator
```

Reload Pi after installing. An explicitly configured `PLANNOTATOR_BROWSER` keeps
its existing behavior. Otherwise the integration supplies a browser helper that
finds the native app and its requesting terminal. It uses the terminal's
`herdr.space` label, not the currently selected agent. An unavailable app, an
older app without the browser API, or an ambiguous origin opens the system
browser instead. Closing the review closes only the browser it created. If the
user has split the review tab, other panes are left alone. Each Pi runtime owns
a private local socket (a named pipe on Windows). The browser hook pairs with
Plannotator's review ID over that socket; a reused URL or port does not reuse a
closed review's lifetime. Cancelling a review while its tab is opening also
closes the browser once it appears. Archive and other unmatched
browser requests still open in the system browser.

The Windows app can show a review served by Pi in local WSL through localhost.
The helper resolves Windows PowerShell by absolute path when it is absent from
WSL's PATH. For arbitrary remote servers, configure an appropriate browser route
and tunnel separately; this integration does not expose a remote port publicly.

Herdr can also start a review in a Pi, as the person typing the command there
would. In a herdr-core terminal the integration labels the terminal
`herdr.plannotator` with its process ID, session, socket, token, Node binary,
`request.mjs` and the path of `hc`. `node request.mjs <terminal> review|last
[hc]`, run on that
machine, reads the label with `hc get`, checks that its Pi is still the
terminal's foreground process, and asks that Pi over its socket to run
`/plannotator-review` or `/plannotator-last`. Pi runs the command at once, even
while the agent works, and the submitted annotations reach that session as with
the typed command. A Pi without Plannotator answers with an error instead.

Environment overrides:

- `HERDR_APP_BIN`: the app executable to control, including a WSL path to the
  Windows executable.
- `HERDR_PLANNOTATOR_MODE=external`: always use the system browser.
- `HERDR_PLANNOTATOR_MODE=disabled`: leave Plannotator's browser settings alone.
- `HERDR_WINDOWS_POWERSHELL`: override the Windows PowerShell executable path.

Ask AI uses Plannotator's original in-process Pi bridge. The integration does not
send text through terminal keystrokes. WSL connected through SSH to its Windows
host keeps that bridge enabled when the native fork is available, unless the
user explicitly selected `PLANNOTATOR_REMOTE`. A review opened before Pi was
reloaded or replaced can have an expired bridge; open a new review from the
current session in that case.

## Folders reported by programs

A terminal's folder follows what its program reports with OSC 7
(`file://host/path`), OSC 9;9 or OSC 1337 CurrentDir. Pi's Teleport sends it when a
session starts and after every move, so an agent moved to another folder or worktree
appears there in the sidebar, the tab bar and the project context. This also covers
Windows, where herdr-core sees only the PowerShell that Pi runs in. A report holds
while the program that was in front when it arrived stays in front, and only when its
host is empty, `localhost` or the terminal machine's own hostname (asked once per
machine with `hostname`); a shell inside `ssh` therefore does not move a local
terminal. In a terminal of this machine, the path must also be in its system's form:
a Unix path from WSL started in a Windows terminal is ignored. Otherwise the folder of
the process in front applies, as before. The core replays the last report when the app
reattaches, so it survives an app restart.

## Native view limits

The page keeps its state when its tab is inactive. Native pixels always draw
above Herdr's own, so on Windows the areas of open overlays (toasts, menus,
dialogs) and the parts outside the container's clip are cut out of the page,
which stays visible around them; a dialog's dimmed backdrop does not dim it.
Elsewhere the page hides while an overlay is open or its container is partly
clipped. It hides while a pane is dragged. Back, Reload and Open externally
stay in native chrome.
Downloads and new-window requests are blocked. Initial URLs and subsequent
navigation accept HTTP(S) without embedded credentials.

Headless Linux compositors without a working EGL/DRI path can need
`WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1` in the test launcher.
These are rendering options, not webview sandbox overrides. They are not applied
to ordinary installations automatically.

## Upstream synchronization

The `sync upstream` workflow runs daily at 04:23 UTC and can be started manually.
It merges upstream into a temporary `automation/upstream-*` branch, then runs the
same Windows, Linux and macOS CI against that exact commit. Master advances only
after those checks pass. A merge conflict, failed check, changed candidate or
concurrent master change leaves master untouched. The workflow never force-pushes.

Source builds do not install upstream's official app updates. Synchronization
updates this repository; install a checked fork build to update the app. CI keeps
a Windows test executable with its license notices as the `herdr-browser-windows`
artifact for seven days.
