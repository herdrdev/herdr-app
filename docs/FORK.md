# Personal browser fork

This fork follows `herdrdev/herdr-app` and adds a native HTTP(S) browser component
for mods. Plannotator reviews can open beside the Pi terminal that requested them.
Pages have no Herdr command bridge. Existing layouts, terminal storage and core
protocols are unchanged.

## Build and install

Use the upstream [build requirements](../README.md#build-from-source), plus
WebKitGTK 4.1 and GTK 3 development packages on Linux. Windows needs the WebView2
runtime. The browser uses the system webview; it does not bundle Chromium.

Build and install with `just install`. Quit and reopen the app yourself to use the
new binary. Installing a build must not stop the core or other people's agents.
On Linux, start the app on X11/XWayland (`env -u WAYLAND_DISPLAY herdr-app`).
Native Wayland embedding is not supported by this adapter.

Check the binary before configuring an agent:

```sh
herdr-app --browser-api
# {"module":"herdr-browser","version":1}
```

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
browser instead. Closing the review closes only the browser pane it created.
Each Pi runtime owns a private local socket (a named pipe on Windows). The
browser hook pairs with Plannotator's review ID over that socket; a reused URL
or port does not reuse a closed review's lifetime. Cancelling a review while its
pane is opening also closes that eventual pane. Archive and other unmatched
browser requests still open in the system browser.

The Windows app can show a review served by Pi in local WSL through localhost.
The helper resolves Windows PowerShell by absolute path when it is absent from
WSL's PATH. For arbitrary remote servers, configure an appropriate browser route
and tunnel separately; this integration does not expose a remote port publicly.

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

## Native view limits

The page keeps its state when its tab is inactive. It hides while a Herdr overlay
is open or its container is partly clipped, so native browser pixels cannot
cover a menu or dialog. Back, Reload and Open externally stay in native chrome.
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
