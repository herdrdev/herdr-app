# Local setup and recovery

Run project commands from the repository containing this skill, or the configured
`HERDR_VM_PROJECT`. The existing guest's identity, disk, credentials and SSH key
live in `.local/vm`. Preserve that directory when updating the app.

## Start the existing VM

`scripts/vm/run` launches `.local/Herdr VM.app` using a separate app home and
`HC_SESSION=vmproto`. Select its macOS VM tab if the VM has not started yet.
It can remain hidden after boot. This is distinct from the installed Herdr.

The default guest account is `agent`; the authoritative password is in
`.local/vm/config.json`. Read it only when authentication is needed and avoid
printing it into routine diagnostics. After changing it, the guest may
require an explicit login rather than logging in automatically.

`scripts/vm/guest connect` installs the private SSH public key using the saved
guest credentials. It discovers the NAT lease by the VM's MAC address. If SSH
is not ready, inspect boot status and retry after the guest has booted. A stale
or unavailable lease is not permission to connect to an unrelated machine.

## Build or provision

`scripts/vm/build` compiles the feature-gated native app and signs the local
bundle with the virtualization entitlement. Stop the guest and quit only this
experimental app before replacing its executable or native library.

`scripts/vm/prepare` downloads Apple's supported restore image and installs a
fresh VM when none exists; it does not reset an installed guest. It needs
substantial disk space and download time. The prepared local configuration is
8 CPUs, 16 GiB RAM, and a 128 GiB sparse disk on Apple Silicon. Automated
account/SSH provisioning uses macOS 27 APIs. Put Rust and Zig on PATH (or set ZIG).

The repository's `AGENTS.md` requires a maintainer discussion before upstream
core/API growth. Local prototype work is not an approved upstream extension API.

## Cua permissions and connection

The guest installation is `/Applications/CuaDriver.app`, invoked through
`~/.local/bin/cua-driver`. Use the signed app's own daemon so macOS attributes
permissions to CuaDriver. Check with `guest exec`:

```sh
scripts/vm/guest exec '~/.local/bin/cua-driver doctor'
scripts/vm/guest exec '~/.local/bin/cua-driver permissions status --json'
scripts/vm/guest exec '~/.local/bin/cua-driver permissions grant'
```

The grant command opens the guest's system prompts. Complete Device Control /
Accessibility, Screen Recording, and the explicit direct-capture consent. The
last one may still be needed after both basic status booleans are true. Respect
the active computer-use tool's confirmation rules when changing these settings.
Use the guest UI or hand that step to the person; permission databases and
disabled permission gates are not substitutes for a grant.

When a prompt requires host interaction, the host Computer Use skill controls
the experimental app. Click inside the VM before typing. Host automation must send physical key
events (`press_key`); generic Unicode/AX `type_text` does not pass through
`VZVirtualMachineView`. Use guest-driver `type_text` for ordinary text entry. Ordinary operation
then returns to the guest driver, including when the host tab is hidden.

For Codex or Claude, register a stdio MCP server whose command is the absolute
project path to `scripts/vm/guest`, with `args = ["mcp"]`. The CLI `guest tool`
transport works immediately without changing the agent's global MCP settings.
The driver version used for the local proof was 0.38.0; use its `describe`
output rather than copying schemas from a different release.

## Keyboard and shutdown

The development guest was tested with U.S. input only. To configure a new guest via
the supported macOS input-source APIs:

```sh
xcrun swiftc -framework AppKit -framework Carbon native/vm/GuestKeyboard.swift -o .local/vm/artifacts/guest-keyboard
codesign --force --sign - .local/vm/artifacts/guest-keyboard
scripts/vm/guest exec '"/Volumes/My Shared Files/artifacts/guest-keyboard"'
```

Native typing requires the VM view to retain first responder across Herdr
redraws. System-wide shortcuts remain with macOS. Click host chrome to return
focus to Herdr. Host synthetic Command shortcuts through the native view are
not verified; send agent shortcuts through the guest driver. The native host defers hiding until it knows the pane is absent; hiding
and showing the native view every frame breaks that focus. Test both native
typing and direct guest-driver typing when changing the view integration.

For a clean shutdown, run `scripts/vm/guest shutdown` and wait for
`.local/vm/status.json` to report `stopped` before quitting Herdr. The toolbar's
ACPI shutdown request may require confirmation inside macOS. Quitting Herdr requests guest shutdown; if it has not completed after 30 seconds,
quitting is cancelled. Guest shutdown prompts may still need attention. Forced
process termination or a host crash cannot be intercepted.

## Reproduce the deployment test

`scripts/vm/build-smoke-app` builds a small test app. Deploy
`.local/vm/artifacts/Herdr VM Test.app`. It reports the Metal device and a
completed Metal command in `results/deployment.json`; typing writes
`results/typing-result.txt`, and its button writes `results/click-result.txt`.
Verify those outcomes alongside screenshots, including with another Herdr tab
selected. These files provide independent evidence of actual guest input.
