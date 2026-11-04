---
name: herdr-vm
description: "Deploy and test macOS apps or browse with computer use inside the local Herdr VM, including while its tab is hidden. Use for guest screenshots, input, shared files, or VM setup; this targets the experimental native VM build."
---

# Herdr VM

Operate the guest through its Cua Driver over SSH. Herdr's tab is a native
`VZVirtualMachineView` for the person to watch or interact with; agents do not
need to select, capture, or click that host tab. The VM and its desktop remain
available when the tab is hidden. Quitting Herdr requests guest shutdown; a timeout cancels quitting. Keep guest work separate from the person's host desktop.

## Locate and check

Resolve this skill through any installation symlink. Its project root is two
directories above the skill folder (`skills/herdr-vm`), or `HERDR_VM_PROJECT`
when explicitly configured. Run commands from that project root:

```sh
cat .local/vm/status.json
scripts/vm/guest exec 'id; sw_vers'
scripts/vm/guest exec '~/.local/bin/cua-driver permissions status --json'
```

Proceed when the VM is running, SSH works, and the guest is logged in with the
driver's capture/control permissions enabled. For installation, boot/login,
permissions, keyboard layout, or SSH problems, read [setup.md](references/setup.md).
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

| Host path under `.local/vm` | Guest path | Guest access |
| --- | --- | --- |
| `artifacts/` | `/Volumes/My Shared Files/artifacts/` | Read-only |
| `results/` | `/Volumes/My Shared Files/results/` | Read/write |

Use `results/` for screenshots, logs, and test reports. Host paths and guest
paths are different: commands after `guest exec` run inside macOS in the VM.
The guest's `localhost` is also separate from the host's.

## See and interact

Prefer a connected guest MCP server when available. Its stdio command is
the absolute project path to `scripts/vm/guest` with argument `mcp`.
Otherwise use the tested CLI transport:

```sh
scripts/vm/guest tools
scripts/vm/guest describe get_window_state
scripts/vm/guest tool get_accessibility_tree '{"session":"my-task"}'
```

Choose a task-specific session label and reuse it on calls that accept
`session`. Discover guest PIDs/window IDs from the response. Read the relevant
tool schema with `describe` before using unfamiliar arguments.

For a discovered window, request `get_window_state` with its `pid`, `window_id`,
and `session`. Set `screenshot_out_file` to a unique guest path such as
`/Volumes/My Shared Files/results/my-task-before.png`. Inspect the returned
accessibility tree and the corresponding host file `.local/vm/results/my-task-before.png`.
Capture errors are failures to see the window, not evidence that it is blank.

Use current `element_token` values for accessible controls. For canvas or other
surfaces absent from the tree, use screenshot coordinates and the returned
capture ID. Read state again after acting, using `since:"latest"`; the returned
snapshot ID supersedes older element tokens. Confirm text, navigation, and
click effects from the new state or application outputs. A tool reporting
`effect:"unverifiable"` requires inspection before retrying.

Guest foreground delivery, when necessary, changes focus inside the VM. It
does not require bringing Herdr or its VM tab to the front. Browser work uses
the guest's browser; discover its window and use the driver's supported tools.
This skill does not itself authorize external submissions or account changes.

For long tests, keep the guest logged in and awake for that task. Coordinate
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
