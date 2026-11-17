# Terminal link handler: local experiment and upstream proposal

A working local prototype implements the layout-selected handler and non-recursive system opener below. The native patch is proposed as a small draft for architectural review; this is not an approved or stable shared API. The SSH/browser implementation stays in a personal mod.

## The three core questions

1. **Does the app itself act on it? Yes.** The terminal currently invokes the OS opener directly. A handler would change what happens when someone activates a URL.
2. **Can only the app see it? Yes.** The native terminal knows the exact link target (including OSC 8 links), clicked terminal, and its machine. Focus may refer to a different terminal, or may change during asynchronous work.
3. **Can mods not do it correctly? Yes.** No current API supplies terminal URL-click events. Screen scraping, overriding the system browser, or looking at focused() cannot reliably recover that source context.

Browser choice, forwarding policy, ownership presentation, and cleanup should remain in a personal mod/helper. This prototype demonstrates that part independently.

## Current source path

- `src/terminal.rs`: `TerminalEvent` is already the native-to-window event channel.
- The URL click branch now emits `TerminalEvent::OpenUrl`; file targets still use `links::open` directly.
- `src/window.rs`: the `TerminalEvent` subscription already captures `addr`, so it can route the **clicked** source without consulting the currently focused pane.
- `src/mirror.rs`: existing machine qualification helpers can recover the source machine from that terminal's app-qualified address.
- `src/layout.rs`: the local patch adds root `terminal_link_handler`, preserving it when docking wraps the root.
- `src/mods.rs`: the local patch exposes `openUrlSystem(url)` for non-recursive pass-through; it has no effects during `checking()`.

## Suggested minimal shape (for discussion)

An optional root layout choice, tentatively:

```json
{"terminal_link_handler": "remote-links"}
```

For URL activation only, emit a native event and deliver this immutable context to the selected handler mod:

```js
{ url: "http://127.0.0.1:3000/foo", terminal: "dev/t16", machine: "dev" }
```

`machine` is null for a local terminal. File opens and terminal-address peek links retain their existing behavior. Automatic URL matches and OSC 8 targets should take the same path.

Without a configured handler, or if its folder is absent, use the existing system opener. Keep all released behavior unchanged by default. The handler must have an explicit, non-recursive way to decline/pass through a link; a narrow `openUrlSystem(url)` host function is one option. Avoid a generic command registry or global browser override for this one event.

The local implementation uses a short-lived dialog copy with explicit props (also `activated: true` and `overlay`). The personal handler passes ordinary URLs to `openUrlSystem`, starts its async helper for remote loopback URLs, closes the dialog on success, and keeps errors/retry visible on failure. A missing handler uses the existing opener; a broken configured handler never silently falls back. Stable delivery/lifecycle API semantics still need maintainer discussion before any upstream contribution.

The remote-links handler policy:

1. Local terminal / ordinary public URL: hand to the normal system opener.
2. Remote HTTP(S) loopback URL: start/reuse the local browser/SOCKS helper with the captured context.
3. Connection failure: show an actionable error, never accidentally visit the local computer's same-numbered service.

Cloning a shipped layout is required before configuring a personal handler. Update `defaults/AGENTS.md`, generated type declarations, relevant docs and tests in the same native implementation change.

## Possible core follow-up, separate approval

A future `hc tunnel` or `hc socks5` could reuse the existing authenticated machine transport, avoiding a second SSH login and supporting custom --via machines. Its listener must be on the app computer; the remote side performs the destination connection. Core should own lifecycle and expose enough identity to associate that service with a remote originating terminal.

Current `hc exec --on` is not that API: it has no terminal record and a maximum ten-minute lifetime. Current `hc run --parent dev/t16` for a local process is rejected because its parent is on another machine. This prototype uses explicit origin labels rather than pretending either limitation is solved.

## Native acceptance tests after approval

- Local URLs and public URLs retain the normal opener behavior with no handler configured.
- A click in an unfocused remote pane uses its machine even if focus changes before the helper starts.
- OSC 8 links use the actual target, not their displayed text.
- File and terminal-address links retain their existing actions.
- Missing/broken handlers, declined URLs, and startup failures have explicit tested behavior.
- Remote localhost, 127.0.0.1 and [::1] work; lookalike domains are not treated as loopback.
- Concurrent clicks create one browser/proxy per machine; different machines/app homes are isolated.
- Cross-port redirects and WebSockets work; no local fallback on SSH loss.
- Isolated Linux sandbox test, full Rust tests, and native macOS/Windows validation for any promised platform behavior.
