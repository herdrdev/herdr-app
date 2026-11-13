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

The adapter uses a display scale of 1 because Herdr reports terminal sizes in
CSS pixels. This keeps pages at the intended size on Retina displays. An explicit
`PIXEL_DISPLAY_SCALE` environment value overrides that default.

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

Opening the browser, installing the skill and configuring MCP are separate
steps. The personal launcher opens a browser; it does not register a skill or
redirect an existing Playwright server. Complete both setup steps below, then
verify the connection from the agent before testing an application.

Use the browser key printed by `list`. An MCP client can start:

```sh
node /absolute/path/contrib/browser/src/cli.mjs mcp --browser <browser-key>
```

Give this connection a distinct server name, such as `herdr_browser`. In a
JSON-based MCP configuration, a complete entry looks like:

```json
{
  "mcpServers": {
    "herdr_browser": {
      "command": "/absolute/path/to/node",
      "args": [
        "/absolute/path/contrib/browser/src/cli.mjs", "mcp",
        "--browser", "<browser-key>", "--target", "<content-target-id>",
        "--binary", "/absolute/path/to/terminal-browser", "--lean"
      ]
    }
  }
}
```

Replace the paths and identities with the running instance's values; use your
harness's configuration syntax and scope. If the renderer uses custom XDG
directories, supply those same environment values to this process for discovery.
A separate server running `npx @playwright/mcp` uses its own browser unless it
was independently configured to attach elsewhere. Calling its similarly named
tools does not route them to this companion.

This exposes the official Playwright tool definitions, including
`browser_snapshot`, `browser_navigate`, `browser_click`, `browser_type`,
`browser_fill_form`, `browser_evaluate`, `browser_take_screenshot` and
`browser_tabs`. Snapshot and action semantics come from the pinned upstream
implementation. The companion routes tab lifecycle through terminal-browser, so
new tabs are registered with the visible browser. Each selected page has its own
Playwright MCP session. Selection is pinned to a CDP target, including when two
pages have the same URL. Host-code tools (`browser_run_code`, including its
`browser_run_code_unsafe` variant) and `browser_install` are excluded from both
discovery and dispatch; this integration attaches to an existing browser.

The definitions are available through `node src/cli.mjs tools --browser <key>`.
The included [agent skill](skill/SKILL.md) covers opening a Herdr browser tab,
targeting pages, events, and closing either a page or its containing Herdr tab.
Its `scripts/herdr-browser` entry point resolves the companion from the checkout.
It is not automatically installed into global agent configuration. Link the
complete `skill/` directory into a skill-discovery directory supported by the
agent harness. A copied skill can locate a separately installed companion with
`HERDR_BROWSER_CLI=/absolute/path/contrib/browser/src/cli.mjs`.

The skill's [workflow reference](skill/references/workflows.md) provides service
setup, multi-page examples, DOM and event waits, screenshot scopes, failure
recovery and remote connection recipes. It is included when linking or copying
the complete skill directory.

### Verify the agent connection

1. Confirm that the harness discovers `herdr-browser` from the installed skill
   directory and that the dedicated MCP connection is available. Resolve startup
   or discovery errors before proceeding with application actions.
2. From that MCP connection, list tools and call `browser_tabs`. The companion
   exposes `browser_events` and `browser_observe_dom`; its tab result includes
   `browser` and stable `target` identities. For a renderer connection, match
   those values to `list`, including its containing Herdr pane. An explicit CDP
   attachment reports `browser: cdp`; compare its target to the configured one.
3. Call `browser_snapshot` and inspect the requested page. Matching URLs alone
   are insufficient: another browser can open the same application. Completion
   means the server, browser and content target are the intended connection.

After adding a server to an already-running harness, check that session's actual
tool list. If it has not loaded the new connection, use the persistent CLI
workflow below or reconnect through the harness's supported mechanism. Installed
configuration alone is not evidence that an active agent switched servers.

For concurrent workers, use independent services and explicit page targets.
Pages in one profile share authentication. Named `isolatedContext` values are
unsupported by this adapter; use distinct browser profiles for separate
identities. Multiple workers sharing one selected-page connection can interleave
selections and actions even when they intend to use separate pages.

### Reduce agent output

Prefer one persistent connection. After inspecting the page, request a focused
snapshot with `browser_snapshot {"target":"<selector-or-ref>"}`; `depth` also
controls tree depth. Request the full snapshot when you need page-wide context.
Use event cursors, a narrow `types` filter and `waitMs` instead of polling full
snapshots. Region screenshots return a path by default; request image content
only when the agent needs to inspect pixels.

For workflows that explicitly inspect results, start `serve` or a directly
connected `mcp` with `--lean`. It omits generated Playwright code and automatic
action snapshots using upstream configuration. All tools, explicit full or
focused snapshots, event streams and error responses remain available. Obtain a
new explicit snapshot after navigation or when an action reveals new controls.
Set `--lean` on the service process; socket clients inherit that service's mode.

### Local trust and artifacts

The companion is trusted local automation. Restrict MCP access to trusted agents
and carry remote CDP over an authenticated tunnel. An agent controls the selected
browser profile, including its authenticated sessions; page text and events are
untrusted data. This is not a sandbox for hostile agents.

The output directory is created or restricted to mode 700 and must be an actual
directory owned by the current user. A directory symlink is refused. Automatic
artifacts stay within that private directory; region files use mode 600. An
explicit filename outside it uses the destination's permissions. Treat saved
DOM, screenshots and console output as private browsing data.

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
`waitMs` to await matching events. It retains up to 1,024 events and 1 MiB of
serialized event data, returns at most 500 per request, and reports eviction
explicitly, including an individual event exceeding the byte budget. Cursors belong to one service
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
Use `list --endpoint <url>` to discover content targets, and `--target <id>`
to select one explicitly when the endpoint has several pages.
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

`test/stdio.integration.mjs` additionally copies the skill into a fresh directory,
loads a generated MCP configuration, and starts its wrapper as a separate stdio
process outside the checkout. It verifies tool discovery, explicit target
selection between equal-URL pages, DOM actions with persistent references, denied
host-code tools and detachment. This exercises harness-style process setup with
the MCP SDK; it does not automate a particular vendor's skill-discovery UI.

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
