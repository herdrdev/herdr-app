---
name: herdr-vm
description: "Set up, recover, and operate a local macOS VM in Herdr. Use for VM provisioning, unattended login and SSH, guest computer use, or app deployment and testing, including with the VM tab hidden."
---

# Herdr VM

Operate the guest through its Cua Driver over SSH. Herdr's tab is a native
`VZVirtualMachineView` for the person to watch or interact with; agents do not
need to select, capture, or click that host tab. The VM and its desktop remain
available when the tab is hidden. Quitting Herdr requests guest shutdown; a timeout cancels quitting. Keep guest work separate from the person's host desktop.

## Set up or recover

For a new VM, missing configuration, interrupted installation, login/SSH problems,
guest permissions, a dark/idle display, or driver startup, read
[setup.md](references/setup.md). Complete the relevant
stages and verify their outcomes; preserve existing guests. Use that workflow
when asked to make the VM passwordless: SSH keys, automatic desktop login, and
administrator authentication are separate settings.

## Locate and check

Resolve this skill through any installation symlink. Its project root is two
directories above the skill folder (`skills/herdr-vm`), or `HERDR_VM_PROJECT`
when explicitly configured. Select the VM directory from the app's launch
configuration (`HERDR_VM_HOME`); `.local/vm` is only the lab default. Export the
same absolute `HERDR_VM_HOME` for all helpers. If the app and helper targets are
unclear, resolve them using the setup reference before connecting. Run commands
from the project root:

```sh
cat "$HERDR_VM_HOME/status.json"
scripts/vm/guest exec 'id; sw_vers'
scripts/vm/guest exec '~/.local/bin/cua-driver permissions status --json'
```

Proceed when the VM is running, SSH works, and the guest is logged in with the
driver's capture/control permissions enabled.
An unavailable guest is not a reason to substitute host computer use.

## Deploy an app

Build an Apple Silicon macOS `.app` on the host, then:

```sh
scripts/vm/guest deploy '/absolute/path/My App.app'
```

This copies the bundle into the host artifacts folder, copies it from VirtioFS
to the guest's `~/Applications`, and launches it there. Quit the previous guest
instance before replacing a running build. Verify the new instance through
guest window discovery and its visible state or test output.

| Host path under `HERDR_VM_HOME` | Guest path | Guest access |
| --- | --- | --- |
| `artifacts/` | `/Volumes/My Shared Files/artifacts/` | Read-only |
| `results/` | `/Volumes/My Shared Files/results/` | Read/write |

Use `results/` for requested deliverables, logs, and test reports; routine
computer-use screenshots are transient, not saved artifacts. Host paths and guest
paths are different: commands after `guest exec` run inside macOS in the VM.
The guest's `localhost` is also separate from the host's.

## Communication and call efficiency

Keep routine computer use quiet. Perform clicks, double-clicks, typing,
screenshots, and straightforward recovery without narrating each action or
explaining tool names, coordinates, or transport mechanics. Follow the client's
required progress-update cadence with brief outcome-focused updates. Report
completed results, meaningful blockers, consequential mistakes, and decisions
that need the user. Explain interaction details when the user asks for debugging
or a walkthrough; otherwise omit the step-by-step play-by-play.

Reuse known tool schemas and valid app/window IDs within the task. Discover
again when the target changes, disappears, or an error invalidates that state.
Use the connected guest MCP tools directly; do not duplicate their calls through
the CLI. Request observations that inform the next action or verify an outcome,
using accessibility-only diffs when sufficient. Keep required visual checks
and authorization steps. The client controls tool-call visibility; this skill
cannot hide its tool UI.

## See and interact

Use the connected guest MCP server for inline images. Its stdio command is
the absolute project path to `scripts/vm/guest` with argument `mcp` and the
selected `HERDR_VM_HOME`. If it is missing, follow **Connect the agent to guest
MCP** in [setup.md](references/setup.md): register it in the intended client,
verify connectivity, and reload that client's tools as needed. The skill alone
does not connect a server. For diagnosis or a temporary fallback, use the CLI:

```sh
scripts/vm/guest tools
scripts/vm/guest describe get_window_state
scripts/vm/guest tool get_accessibility_tree '{"session":"my-task"}'
```

For connected MCP tools, **omit `session` by default**: the authenticated
connection owns an implicit lifecycle session. Omit it on `end_session` too.
Named sessions are transport-owned, not global labels that can be carried
between CLI calls and MCP connections. If a named MCP session is needed, use a
fresh label and keep its lifecycle on that same connection. On “session is not
available to this transport”, return to the implicit session; do not try to
revive or end a label owned by another connection. Take fresh snapshots after
switching sessions rather than reusing old element tokens or capture IDs.

For the CLI fallback, choose a task-specific label and reuse it on calls that
accept `session`, including cleanup. Discover guest PIDs/window IDs from the
response. Read the relevant tool schema with `describe` before using unfamiliar
arguments.

For a discovered window, request `get_window_state` with its `pid` and
`window_id`, following the transport-specific session rules above. **Omit `screenshot_out_file` by default.** Inspect the inline image
and accessibility tree returned through guest MCP. If using a tool bridge,
forward image content to the model's image renderer; printing base64 to a
terminal does not let the model see it.

Use `include_screenshot:false` for follow-up checks answerable from the
accessibility tree, with `since:"latest"` for changes. Request an inline image
when visual verification or coordinate targeting is needed. Capture errors
are failures to see the window, not evidence that it is blank.

If only the CLI is available and the harness cannot render its inline image,
use one task-owned temporary capture path in the shared `results/` directory.
Reuse/overwrite that path for sequential captures, inspect each image through
the harness's image viewer, and delete the temporary file after inspection
(including cleanup on failure). Concurrent tasks must have separate paths.
Do not create numbered or timestamped screenshot collections. Persist images
only when the user requests screenshots, a recording, or other visual evidence;
ordinary verification does not imply permission to retain a screenshot archive.

Use current `element_token` values for accessible controls. For canvas or other
surfaces absent from the tree, ground coordinates in the current inspected
screenshot and its capture metadata, using the coordinate space required by the
action tool. Pair coordinates with that capture's ID. When image dimensions,
window size, or layout change, obtain a fresh reference; never reuse a scale
factor from an earlier image or guess at a conversion. Resolve any ambiguity
from the tool schema and capture metadata before clicking. Read state again after acting, using `since:"latest"`; the returned
snapshot ID supersedes older element tokens. Confirm text, navigation, and
click effects from the new state or application outputs. A tool reporting
`effect:"unverifiable"` requires inspection before retrying.

Guest foreground delivery, when necessary, changes focus inside the VM. It
does not require bringing Herdr or its VM tab to the front. Browser work uses
the guest's browser; discover its window and use the driver's supported tools.
This skill does not itself authorize external submissions or account changes.

For long tests, verify the guest stays logged in and awake using the setup
reference’s idle settings or a task-scoped sleep assertion. Coordinate
concurrent agents sharing this one guest desktop; a different session label
does not create a separate desktop. End your driver session when finished
using `end_session` according to its schema. Leave the VM running for the
person unless shutdown is part of the task.

## Scope and verified behavior

This is a feature-gated Herdr capability proposed for review, not a released extension API. It
currently hosts one VM. Native display uses no VNC. The guest exposes an Apple
paravirtual Metal device; this is not unrestricted physical GPU passthrough.
Deployment, a Metal command, shared-folder access, and guest screenshot/type/
click operations with the Herdr VM tab inactive have been exercised locally.
