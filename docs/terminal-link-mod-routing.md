# Terminal link activations for personal mods (proposal)

This is a boundary/design review request, not an implemented or released API.
It follows the core-change discussion requirement in `AGENTS.md`. Related:
[#228](https://github.com/herdrdev/herdr-app/issues/228),
[draft #412](https://github.com/herdrdev/herdr-app/pull/412), and
[#452](https://github.com/herdrdev/herdr-app/issues/452).

## What we cannot build with today's mod API

Our personal ADE layout has a central source editor, project-aware file tabs,
LSP navigation, and a terminal panel. A developer should be able to Cmd-click
(Ctrl-click on Linux/Windows) a source path printed by a terminal or coding
agent and open that file in the ADE editor. URLs that the mod does not handle
should keep opening through the operating system.

A terminal file-list picker is a useful workaround, but does not receive the
link the user actually clicked. A wrapper around the OS opener also loses the
clicked terminal's machine, cwd, and owning project. Polling terminal output
cannot reconstruct the click reliably.

## What was checked

- Installed `herdr-app 0.1.16` and its generated `herdr.d.ts`.
- Upstream master at `8e1208deeff2a24441a192974e648c6fa0c5053c`
  (2026-11-15).
- `TerminalView::link_at` in `src/terminal.rs`: Ghostty hit testing, OSC 8,
  source cwd resolution, and terminal-address recognition happen here.
- The modifier-click branch in `TerminalView::render` directly calls
  `links::open` for external targets. Its other branch emits `Peek` for live
  terminal addresses. External link clicks do not emit a mod-facing event.
- `TerminalEvent` and the subscriber in `src/window.rs` expose context menus,
  selections, scroll changes, and terminal peek, but no external-link event.
- The current layout's `terminal_menu` replaces right-click menus; it is not
  a modifier-click interception API.

Draft #412 already proposes `terminal_links` and passes clicked-link props to
an overlay mod. This proposal is not asking for a second competing setting or
for the shipped Files mod to gain ADE-specific behavior. We would prefer to
build on that work after the maintainer chooses the boundary.

## The three core-boundary questions

1. **Does the app itself act on it?** Yes. It currently consumes the click and
   decides whether to use the system opener or peek at another terminal.
2. **Can only the app see it?** Yes. The exact hit/span, OSC 8 target, source
   terminal identity, and modifier-click are native terminal state.
3. **Can mods not do it correctly, even with care?** Yes. A mod cannot observe
   this click or prevent the existing system opener with the documented API.

File loading, editor tabs, language-server navigation, errors, and presentation
remain the personal mod's responsibility. No herdr-core change is proposed.

## Requested behavior

- Routing is opt-in for a personal layout. An unchanged layout keeps today's
  behavior. Reuse #412's `terminal_links` direction if that is the accepted API.
- Activate only after a real link-modifier click; no launch on hover, terminal
  output, restored shared state, focus changes, or project selection.
- Capture the clicked terminal context at activation. Do not ask the receiving
  mod to infer the source from `focused()` after an asynchronous task resumes.
- A handler can open an existing editor panel directly, without a mandatory
  popup flash for every file click. A picker remains a mod-owned option.
- A missing or unavailable handler keeps the existing system-opening behavior.
  A running handler needs an explicit way to decline an unsupported target and
  hand it back to the system. This must not open the same click twice.
- Preserve terminal-address peek behavior, including qualified remote addresses.
- Do not persist an activation for replay when a mod or project is restored.

## Context needed by an ADE handler

The exact public names are intentionally left for review. The information we
need for a clicked link is:

| Information | Why it matters |
| --- | --- |
| Original clicked target | Retain file suffixes such as `:42:7` and OSC 8 targets |
| Resolved file path or URL, when available | Open local source files or decline URLs without reinterpreting host resolution |
| Source terminal address and machine | Local and remote paths must not be confused |
| Source cwd at the click | Relative paths belong to the clicked terminal |
| Source space/tab/pane when available | A visible terminal block can differ from the focused model pane; absent IDs should stay absent |
| A transient activation identity | Route/consume one user action once, including any explicit system fallback |

The hook could be a routed mod command, an activation-props entry point, or
another small host API. Merely broadcasting to every shown copy would need a
single-consumer policy: an ADE layout has several copies of the same mod for
its editor, tool panels, status bar, and overlays.

## Scope and open decisions

A first implementation can cover the external URL and locally resolved file
links that the app already recognizes. Preserve raw input now rather than
claiming support for unresolved remote paths. Forwarding remote/unresolved
file candidates before the local existence check needs a separate explicit
boundary decision; #412 currently does not do that.

Before touching the core implementation, could Can confirm:

1. Should this continue through #412's layout-scoped `terminal_links` hook?
2. What is the preferred non-popup delivery and explicit-decline mechanism?
3. Should the first version stay with today's resolved targets or also pass
   unresolved/remote file candidates to the handler?

## Acceptance checks for an approved implementation

- Existing local path, file URL, HTTP URL, and OSC 8 clicks retain their default
  behavior with no handler configured.
- A configured personal handler receives the clicked source context even when
  another pane is focused, including terminal blocks, peek, and floating views.
- A handled local file opens the mod's editor once, not the system application.
- Unsupported, missing-handler, and explicit-decline paths use the default
  opener once; an asynchronous reply cannot duplicate an already completed action.
- Existing `:line:column` text and non-ASCII/space-containing paths survive in
  the raw payload. Resolution must not silently use a different pane's cwd.
- Terminal-address peek and unmodified terminal mouse handling are unchanged.
- Terminal output, layout restoration, and project switches emit no activation.
- Validate modifier behavior on macOS and Linux/Windows; document any platform
  checks that have not been run.

No default layout/mod, command contract, or Rust implementation changes are
included in this proposal. Implementation should follow maintainer approval.
