# Browser companion contribution design

The proposed first contribution is a Chromium agent companion that works with
released Herdr App APIs. The native host and agent control plane have separate
ownership and can be reviewed independently. The companion is experimental and
opt-in; no default interface or core model changes are included.

## Upstream constraints

Herdr's [AGENTS.md](../../AGENTS.md) asks contributors to keep the app core small
and obtain Can's agreement before adding model fields, machine layout data,
commands or shared mod APIs. [CONTRIBUTING.md](../../CONTRIBUTING.md) directs new
features into personal mods and asks for discussion before bigger changes.

In [issue 218](https://github.com/herdrdev/herdr-app/issues/218#issuecomment-6536660100),
Can said that a browser was on the roadmap and would probably be an official
mod. That is a direction, not an approved specification for a native-view API.
The issue also documents a compatibility problem: introducing an unrecognized
Content variant can make an older client reject a machine's entire layout.

[Issue 425](https://github.com/herdrdev/herdr-app/issues/425) describes an external
native-view prototype and event API, but was closed as not planned. It is not an
approved API. [PR 235](https://github.com/herdrdev/herdr-app/pull/235) is a draft
macOS WebKit pane and is not the engine chosen for this contribution.

Can's [herdr-browser experiment](https://github.com/ogulcancelik/herdr-browser)
now recommends [terminal-browser](https://github.com/zenbu-labs/terminal-browser).
This contribution follows that existing Chromium renderer instead of adding
another engine or a new native pane type.

## Boundaries

```text
Herdr App tab -> herdr-core terminal -> terminal-browser -> Chromium
                     ^                      ^
                     | terminal socket      | CDP + browser control socket
                     |                      |
                companion adapter     persistent agent service
                                            |
                                 official Playwright MCP tools
                                  events / targeted DOM watches
```

The adapter implements terminal-browser's existing terminal socket protocol with
Herdr App's existing control socket. All commands use explicit pane or terminal
identities. The adapter refuses cross-tab targets and leaves unsupported
operations out of its capability list. A split gets a new adapter scoped to its
new terminal. Herdr's shipped mods, keys and layouts are never edited.

The browser service connects once, routes tab creation/closure through the
renderer, and gives the upstream Playwright MCP implementation one selected
content page at a time. It does not target by URL or expose an Electron utility
page by default. A cursor identifies an event collector lifetime and sequence;
eviction is visible, action calls are serialized and event waits are concurrent.
The CLI and MCP can share that same service.

The service is trusted local automation, not a sandbox. Its socket is restricted
to a private directory and user-only access. A CDP endpoint is explicit and must
be carried over an authenticated transport for remote use. Failed action
delivery is not automatically retried: reconnect, observe the same page and
verify the outcome first.

## Native work remains separate

A native proposal is warranted only after demonstrating a gap that the companion
cannot solve through existing APIs. Candidate questions include rendering
throughput, viewer transport for remote browser hosts, and independent browser
lifetime after a renderer disconnects. The related performance proposal is
[PR 704](https://github.com/herdrdev/herdr-app/pull/704); this fork does not duplicate
that patch. Remote port reachability also has an open discussion in
[issue 853](https://github.com/herdrdev/herdr-app/issues/853).

Any future native change needs the maintainer's API agreement, compatibility
with older clients, and checks on the affected platforms. Chromium is the chosen
browser family on macOS and Linux. Its runtime must still be provisioned
explicitly: a user's installed Chrome is useful for CDP attachment but is not a
portable native embedding SDK.
