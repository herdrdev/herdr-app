# Local macOS VM

This opt-in capability places a macOS desktop inside a Herdr tab using Apple's
Virtualization framework. It is a proposal for review, not an upstream release.
The first version intentionally manages one local VM. There is no VNC server,
network video stream, or change to herdr-core.

## Try it

Use Apple Silicon, Xcode with the macOS SDK, Rust and Zig as described in the
root README, Python 3, and herdr-core (`hc` on PATH). Full automatic guest
account/SSH provisioning requires a macOS 27 host and SDK. Older SDKs compile
the view without that provisioning API; guest setup must then be completed
manually. The tested configuration is macOS 27.2 on Apple Silicon.

```sh
git submodule update --init --recursive
scripts/vm/prepare
scripts/vm/build
scripts/vm/run
scripts/vm/guest connect
```

Preparation downloads Apple's supported restore image and creates an 8-core,
16 GiB VM with a 128 GiB sparse disk. Allow space for the restore image and
guest filesystem. The private directory defaults to `.local/vm`; select a
different absolute directory with `HERDR_VM_HOME` for prepare/build/guest.
CPU and memory settings live in its `config.json`; edit only while stopped.
The framework validates the resulting hardware configuration on start.

The account is `agent`, with a random password in that private configuration.
Automatic login and SSH are provisioned on supported hosts. Complete any
remaining login/setup screens in the guest. No Apple account is required for
this development workflow. Apple's macOS software license applies.

The lab application has a separate bundle identifier, app home and core session.
It does not replace an installed Herdr. `HERDR_VM_APP_HOME` overrides the lab
app home at build/run time. Rebuild after changing the bundled launch defaults.
Normal builds do not link Virtualization or require its entitlement.

In a VM-enabled build, click **+ → macOS VM** to open the native pane,
then use **Start** to boot the prepared guest. This entry is only visible in local workspaces on VM-enabled builds.
The underlying native capability always targets the desktop client’s Mac.

## Native capability and mod API

The shipped `virtual-machine` mod declares:

```json
{"place":"none","focusable":true,"native":"macos-vm"}
```

The capability is selected by metadata, not the mod's name. It applies to tab
panes; unsupported builds, unknown native providers, and non-pane placements
use the mod's JavaScript renderer. The serialized layout stays an ordinary mod
node. This is always a VM on the desktop client's Mac, even when its selected
space belongs to a remote core.

Mods may import `vmStatus()`, `vmStart()`, and `vmShutdown()` from `herdr`.
Status reports build support, configuration and the last persisted session
report (including pid/time); it does not expose passwords or disk contents.
Start queues a boot and is idempotent during startup/running. Shutdown requests
normal guest shutdown and never forces power off. Configuration/start errors
appear in the pane or session status. Check the result rather than treating
acceptance of a request as completion.

## Display and session lifetime

`native/NativePaneHost.swift` handles AppKit placement, clipping to the pane's
visible rectangle, focus, and hiding when overlays or other tabs are shown. System-wide
shortcuts remain with macOS; click another Herdr pane or tab to return
keyboard ownership to the host.
`native/vm/` owns the VM configuration, display and lifecycle. `src/native_vm.rs`
provides the small Rust/Swift bridge; `src/window.rs` only routes opted-in panes.

Hiding or closing a pane leaves the VM running. Reopening its pane reattaches
the native view. One VM desktop surface is supported in this first version;
avoid displaying duplicate VM panes simultaneously. The VM exposes Apple's
paravirtual Metal device, not unrestricted physical GPU passthrough.

Quit requests normal guest shutdown before Herdr's own shutdown hooks run. If
the guest hasn't stopped in 30 seconds, quitting is cancelled and the status
explains why. Finish any guest shutdown prompts and try again. The red window
button retains Herdr's existing hide behavior. A forced process termination,
host shutdown deadline, or crash may still abruptly stop the VM; this version
does not implement saved-state suspend/resume.

## Deploy and automate

```sh
scripts/vm/guest deploy '/absolute/path/Example.app'
scripts/vm/guest exec 'sw_vers'
```

Quit an old instance before deploying its replacement. Artifacts are shared
read-only at `/Volumes/My Shared Files/artifacts`; the deploy helper copies
the bundle into guest `~/Applications` before launching. The guest can write
screenshots/logs back through `/Volumes/My Shared Files/results`, visible on
the host at `.local/vm/results`. Only these dedicated directories are shared.
Guest and host `localhost` are distinct.

Computer use is optional. Install the signed [Cua Driver](https://cua.ai/docs/cua-driver)
**inside the guest**, grant its requested guest permissions, and verify:

```sh
scripts/vm/guest exec '~/.local/bin/cua-driver permissions grant'
scripts/vm/guest exec '~/.local/bin/cua-driver permissions status --json'
```

Configure an agent's stdio MCP server with the absolute path to
`scripts/vm/guest` and argument `mcp`. Alternatively use `guest tools`,
`guest describe TOOL`, and `guest tool TOOL JSON`. The driver runs in the guest:
screenshots, clicks, typing and the agent cursor work while the host tab is
hidden. The guest must remain logged in and awake. For host-side automation of the
native view (for example login), use physical key events; generic host Unicode
text injection does not translate into guest keystrokes. Ordinary agent typing
uses the guest driver. Host automation of Command shortcuts through the native
view is not verified; guest-driver shortcuts are the supported agent route.
No host desktop permissions
are needed by this SSH transport. Installation and privacy grants are explicit
setup steps; the feature does not silently install an automation agent.

The colored, named cursor is the driver's agent overlay; the white pointer is
the guest's ordinary mouse cursor. End the driver session when done to remove
its overlay.

The [agent skill](../skills/herdr-vm/SKILL.md) documents this workflow. It can be
symlinked into Codex's or Claude's skills directory. Its paths resolve from the
repository so a fork checkout can be installed without user-specific edits.

## Validation

`just test` checks the normal build. On Apple Silicon, `just vm-test` additionally
links the native feature and runs AppKit focus/visibility/reattachment and VM
disk-lock tests. `just docs-check` checks documentation examples.

For an end-to-end test, build and deploy the included sample:

```sh
scripts/vm/build-smoke-app
scripts/vm/guest deploy '.local/vm/artifacts/Herdr VM Test.app'
```

It writes Metal device/command results to `results/deployment.json`, entered
text to `results/typing-result.txt`, and button actions to `results/click-result.txt`.
Inspect screenshots and those outputs while another Herdr tab is selected.
Then test pane reattachment and native typing. Shut down cleanly with
`scripts/vm/guest shutdown`, waiting for `status.json` to report `stopped`.

## Boundaries for review

This proposes a built-in native VM capability with a narrow mod API. It does
not introduce arbitrary native plugin loading or a general public surface API.
Multi-VM selection, remote display, snapshots, clipboard synchronization,
automatic driver installation and saved-state restoration are outside this PR.
The AppKit host is kept separate so a future native browser can reuse the
placement/input work after its own design review.
