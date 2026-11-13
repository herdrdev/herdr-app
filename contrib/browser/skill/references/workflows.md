# Browser workflows

Run shell examples from the installed skill directory, or use an absolute path
to `scripts/herdr-browser`. Replace angle-bracket placeholders with recorded
identities, references or paths. Browser tool examples are MCP calls with JSON
arguments; they also work through `call --socket <path> <tool> '<json>'`.

## Start a service

First open or discover a browser using the skill's Open or attach steps. Record
its browser key, content target and containing Herdr tab. For direct MCP, configure
the harness to start the skill wrapper with these arguments:

```json
{
  "command": "/absolute/path/to/skill/scripts/herdr-browser",
  "args": ["mcp", "--browser", "<browser-key>", "--target", "<content-target>"]
}
```

The harness receives the tool definitions through MCP. To share a persistent
connection with shell calls, start a service instead:

```sh
browser_state=$(mktemp -d /tmp/herdr-agent.XXXXXX)
scripts/herdr-browser serve --browser <browser-key> --target <content-target> \
  --socket "$browser_state/browser.sock" --output-dir "$browser_state/artifacts" \
  --lean >"$browser_state/service.log" 2>&1 &
browser_service_pid=$!
```

Record the actual state directory and service PID. The short private directory
also avoids macOS Unix-socket path-length limits. Readiness means the socket
exists and `tools --socket <actual-socket>` succeeds. If startup fails, inspect
the service log before launching another process. After readiness:

```sh
scripts/herdr-browser call --socket "$browser_state/browser.sock" browser_snapshot '{}'
```

To connect an MCP harness to this service, use the wrapper with arguments
`["mcp", "--socket", "<actual-socket>"]`. It inherits the service's mode.
Omit `--lean` at service creation if automatic action snapshots and generated
code are useful. Change modes by restarting only the service you own, then
refreshing references and cursors; the browser can stay open.

To detach, send SIGTERM to the recorded service PID. Completion means that
process has exited and the browser is still listed. Preserve artifacts needed
as evidence; remove only task-owned temporary files after they are no longer
needed.

## Multiple pages

One service can control several visible browser pages, one selected page at a
time. Keep each selection and its following actions in order. An independent
workflow uses a separate service and explicit target; two services selecting the
same page still act on the same DOM and share that page's state.

1. Call `browser_tabs {"action":"list"}` and record the original stable `target`.
2. Call `browser_tabs {"action":"new","url":"https://example.com"}`. Record the
   selected target returned by the call. Page creation is complete when that
   target appears in the returned list; wait separately for the required page
   content to load.
3. Call `browser_snapshot {}` on the selected page. Use references from that
   page for `browser_click`, `browser_type` or `browser_fill_form`.
4. To return to a previous page, list pages again and find its target's current
   index. If the list maps it to index 0, call
   `browser_tabs {"action":"select","index":0}`. Confirm the returned selected
   target, then inspect that page before acting.
5. To close a page, find its current index by target, call `browser_tabs` with
   `action: close`, then confirm its absence and the presence of pages you kept.

Indices can change when a page closes; equal URLs can identify different pages.
References from one page are not transferable to another. After navigation,
take a new snapshot even if the browser target stayed the same. Creating a new
page shares the browser profile's session; it is not a fresh authentication
context. Use the skill's closure steps when the request concerns a containing
Herdr tab rather than a browser page.

## Read and act efficiently

Take `browser_snapshot {}` first to identify the page and relevant section. A
focused snapshot keeps the tool's semantic targets while reducing irrelevant
context. For an observed dialog:

```text
browser_snapshot {"target":"[role=dialog]"}
browser_fill_form {"fields":[{"target":"<field-ref>","name":"Email","type":"textbox","value":"agent@example.test"}]}
browser_click {"target":"<button-ref>"}
```

Use a selector that uniquely identifies the section. `depth` can limit tree
depth; request a deeper or full snapshot if the required control is absent.
Use `browser_evaluate` for precise DOM measurements such as bounding rectangles
or counts, and return only the values needed for the current check.

In lean mode, explicitly snapshot after navigation or when controls change.
Completion means the page shows the intended value, state or result, not merely
that a click returned successfully. For save or submission workflows, inspect
the success or failure state before issuing another mutation.

## Wait for a result

For navigation, get an event cursor before acting, wait for `navigation` using
that cursor and page target, then inspect the loaded page. A navigation event
alone does not prove the requested application content finished loading.

For a DOM result whose selector you have inspected, such as `#status`:

```text
browser_observe_dom {"selector":"#status","debounceMs":100}
browser_events {}
  -> record cursor and the observer's watch ID
browser_click {"target":"<action-ref>"}
browser_events {"cursor":"<cursor>","tab":"<content-target>","types":["dom.changed","page.error"],"waitMs":5000,"limit":50}
browser_snapshot {"target":"#status"}
browser_observe_dom {"stop":"<watch-id>"}
```

Check the matching watch ID and the observed result. Each event consumer keeps
its own cursor and advances using the returned cursor, including when reading
another page of a batch. A filtered read advances past events outside that
filter: consumers needing other event types keep separate cursors.

A timeout means no matching event arrived within the wait. Inspect the current
DOM or error state; a DOM event may already have happened if the cursor was
obtained too late. Navigation ends existing watches, so create a new watch in
the new document when needed. If the buffer reports `gap`, or the service says
`event_session_changed`, take a fresh snapshot and obtain a new cursor before
continuing. Stop a watch when its result is verified.

## Screenshots

Use semantic snapshots for navigation. Capture pixels for visual verification,
canvas content or a visual defect. Choose the scope needed:

```text
browser_take_screenshot {"scale":"css"}
browser_take_screenshot {"fullPage":true,"scale":"css"}
browser_take_screenshot {"target":"<element-ref>","scale":"css"}
browser_screenshot_region {"x":0,"y":0,"width":400,"height":200}
```

The region coordinates are CSS pixels relative to the current viewport. Inspect
the viewport dimensions first; the rectangle must fit within it. The region
tool returns an artifact path by default. Add `includeImage: true` when the
agent needs image content in its response. `herdr-app shot <path.png>` captures
the entire native Herdr window rather than just the page. Capture is complete
when the returned file exists; a visual assertion additionally requires viewing
it. Save evidence in a private task directory and keep sensitive content out
of public reports.

## Recover from a failure

| Observed failure | Next step and completion criterion |
|---|---|
| Stale or missing reference | Confirm the selected target, take a snapshot, and resolve the intended control before acting again. |
| Expected content absent just after page creation | Read a snapshot to attach the session, wait for the expected content, then verify it. A successful page-open result confirms registration, not application readiness. |
| Connection lost after a mutation | Reconnect to the same target and inspect the outcome. Repeat a submission only after establishing that it was not applied. |
| Browser registration or socket-path failure | Inspect the created Herdr terminal and service log; use a short private runtime path. Open another instance only after identifying what happened to the first. |
| Background creation unsupported | Use `--focus` if foreground creation is acceptable. The failed background request creates no tab. |
| Event gap, changed session or ended watch | Recover with a fresh snapshot/cursor; create a new watch only in the current document. |
| Screenshot rectangle rejected | Measure the current viewport and choose an in-bounds rectangle, then verify the returned image. |

If a mutation's outcome remains uncertain after inspection, report that state
before attempting another submission.

## Remote connections

Choose between these topologies before opening or attaching:

- **Local display, remote network:** run
  `open --space <local-space> --ssh <host> --url <url> --focus`, or
  `run --ssh <host>` inside a local Herdr terminal. The browser is local; the
  SSH host supplies networking. Confirm the expected application in a snapshot.
- **Existing remote Chromium:** forward its loopback CDP endpoint through an
  authenticated SSH tunnel. Discover with `list --endpoint <forwarded-CDP-url>`,
  select a content target, then connect with
  `mcp --endpoint <forwarded-CDP-url> --target <target-id>`. Completion means the
  snapshot belongs to the intended remote page. This attaches automation;
  displaying that browser requires a separate viewer.

The network host resolves `localhost` in page URLs. Use the same browser host
and content target when reconnecting; shared URLs are not identity evidence.
