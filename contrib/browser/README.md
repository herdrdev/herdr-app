# Chromium browser companion for Herdr App

An experimental, separately installed browser companion. Open a real Chromium
browser in a Herdr terminal tab and let agents control that same page through
the official Playwright MCP tools. A persistent service keeps page references,
connections and event cursors between shell calls.

This fork keeps the agent integration outside Herdr's native application. It
adds no Rust dependency, built-in command, layout variant or shipped mod. The
optional launcher is a personal mod. Native changes can be reviewed separately
if a demonstrated limitation requires them.

## Requirements and installation

Use macOS or Linux, Node.js 22 or later, Herdr App with herdr-core, and
[terminal-browser](https://github.com/zenbu-labs/terminal-browser) 0.15.3.
The current integration is pinned and tested against that terminal-browser
release and the Playwright versions in `package-lock.json`.

```sh
cd contrib/browser
npm ci --ignore-scripts
node src/cli.mjs --help
```

Install terminal-browser using its own instructions. It supplies its Electron /
Chromium runtime. This package attaches through CDP and never downloads a second
browser engine. An installed Chrome can be used through an explicit CDP endpoint;
its application bundle cannot replace terminal-browser's embedded runtime.

## Open a browser tab

Get a local space ID from `herdr-app ls`, then run:

```sh
node src/cli.mjs open --space <space-id> --url http://localhost:3000 --focus
node src/cli.mjs list
```

Use `--binary /path/to/terminal-browser` for an installation outside PATH.
`TERMINAL_BROWSER_BIN` sets the same default. The new tab starts this package's
`run` command, which advertises Herdr App's terminal socket to terminal-browser.
This avoids terminal-browser's existing adapter for the older Herdr TUI.
The adapter supports pane identity, listing, focus and splits using existing
Herdr App commands. It does not advertise clipboard or text delivery operations
that it cannot implement correctly.

Omit `--focus` to request a background tab. Older releases that do not support
background creation reject that request before opening anything. The live app's
command schema determines support, rather than its version number.

The optional launcher opens a URL without editing shipped menus or layouts:

```sh
node src/cli.mjs install-mod
herdr-app check ~/.config/herdr-app/mods/browser-launcher
herdr-app do mod.open --name browser-launcher
```

`install-mod` creates a new personal mod and refuses to overwrite an existing
folder. It records the absolute Node and companion paths. Keep this checkout in
place. `--herdr-home <directory>` selects an isolated app home for development.

## Connect an agent

Use the browser key printed by `list`. An MCP client can start:

```sh
node /absolute/path/contrib/browser/src/cli.mjs mcp --browser <browser-key>
```

This exposes the official Playwright tool definitions, including
`browser_snapshot`, `browser_navigate`, `browser_click`, `browser_type`,
`browser_fill_form`, `browser_evaluate`, `browser_take_screenshot` and
`browser_tabs`. Snapshot and action semantics come from the pinned upstream
implementation. The companion routes tab lifecycle through terminal-browser, so
new tabs are registered with the visible browser. Each selected page has its own
Playwright MCP session. Selection is pinned to a CDP target, including when two
pages have the same URL. Arbitrary `browser_run_code` and `browser_install` are
excluded; this integration attaches to an existing browser.

The definitions are available through `node src/cli.mjs tools --browser <key>`.
The included [agent skill](skill/SKILL.md) explains targeting, events and recovery;
it is not automatically installed into global agent configuration.

For repeated CLI calls, start one service in a private directory:

```sh
mkdir -m 700 /tmp/my-herdr-browser
node src/cli.mjs serve --browser <key> --socket /tmp/my-herdr-browser/browser.sock
```

In another terminal:

```sh
node src/cli.mjs call --socket /tmp/my-herdr-browser/browser.sock browser_snapshot
node src/cli.mjs call --socket /tmp/my-herdr-browser/browser.sock browser_click '{"target":"e14"}'
node src/cli.mjs events --socket /tmp/my-herdr-browser/browser.sock --types navigation,page.error
```

An MCP client can also start `mcp --socket /tmp/my-herdr-browser/browser.sock`.
The service serializes actions; event waits can run concurrently. Use separate
services and browser instances for independent agents. Stop the service with
Ctrl+C or SIGTERM. Its shutdown detaches the automation connection and leaves
the browser running. One-shot `call --browser` is useful for independent
operations; references and cursors across calls require a persistent service or
MCP connection.

## Events and screenshots

`browser_events` returns `{cursor, gap, events}`. Obtain a cursor before an
action, then pass it with `types`, optional `tab` (a stable target ID), and
`waitMs` to await matching events. It keeps 1,024 events, returns at most 500
per request, and reports eviction explicitly. Cursors belong to one service
lifetime; reconnecting to that service preserves them, restarting it does not.
The CLI event command emits NDJSON batches, including cursors and gap markers.

Events cover tab lifecycle, navigation, DOM readiness, page load, console,
page errors, failed requests, HTTP errors, dialogs and downloads.
`browser_observe_dom` watches an explicit CSS subtree and coalesces changes.
Stop observers by their watch IDs; navigation ends them. There is no continuous
full-document DOM export. Currently observers watch the main document, and
at most 32 may be active.

`browser_take_screenshot` supports a viewport, whole page or element.
`browser_screenshot_region` captures an in-viewport rectangle in CSS pixels,
returning a PNG path; `includeImage: true` also returns MCP image content.
The existing `herdr-app shot /path/window.png` captures Herdr's whole window.

## Remote topology

For local rendering and remote networking, use `run --ssh <user@host>` inside
Herdr, or add `--ssh` to `open` in a local space. This uses terminal-browser's
SSH network proxy; browser execution remains local. That proxy path has not yet
been exercised by this companion's automated tests.

For an already running remote Chromium, forward its loopback CDP endpoint over
SSH and supply `--endpoint http://127.0.0.1:<forwarded-port>` instead of
`--browser`. CDP attachment alone does not stream a remote display into Herdr.
Remote rendering, viewer reconnection and unattended browser ownership are
separate follow-up work. There is no silent switch between local and remote
browser hosts, and no WebKit implementation.

## Validation and contribution boundary

```sh
npm run check
npm test
HERDR_BROWSER_TEST_CHANNEL=chromium npm run test:browser
```

The browser test requires a test Chromium installed with
`npx playwright install chromium`, or an installed Chrome (the local default).
It drives real Chromium through the MCP transport, verifies semantic interaction,
DOM events, target identity, native-control lifecycle, screenshots, persistent CLI
connections and detachment. CI runs the tests on macOS and Linux.

`test/smoke-desktop.mjs` is an opt-in visible desktop check using a separate app
home, core session and browser state. It requires explicit native app and browser
paths through `HERDR_BROWSER_DESKTOP_BIN`, `TERMINAL_BROWSER_BIN`, and
`HERDR_BROWSER_TERMINAL_DIST`. It writes screenshots to `.artifacts/` and stops
only the processes and core session it created.

[ARCHITECTURE.md](ARCHITECTURE.md) records the upstream constraints and remaining native
questions. This companion is not bundled into Herdr releases. Herdr's existing
Cargo licenses and distribution inventory are unchanged; npm dependency licenses
remain in their installed packages. Playwright and Playwright MCP use Apache-2.0,
the MCP TypeScript SDK uses MIT, and the separately installed terminal-browser
uses MIT.
