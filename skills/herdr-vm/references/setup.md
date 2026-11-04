# Set up or resume a local Herdr VM

Use the existing helpers for Apple installation and SSH transport; adapt guest
setup to the detected macOS and driver versions. This workflow manages one VM
per Herdr process. A new tab does not create another machine.

## Establish the target and resume point

Resolve the checkout as described in SKILL.md. Inspect `uname -m`, `sw_vers`,
`xcodebuild -version`, `xcrun swift --version`, free disk space, and available
Rust/Zig/Python/hc tools. Read the checkout's `docs/local-vm.md` for build
requirements. Apple Silicon is required. Automatic account provisioning is
compiled with Swift 6.4+ and used on macOS 27+; older supported hosts require
manual guest Setup Assistant. Do not promise unattended setup on those hosts.

Inspect the intended app bundle's `Info.plist` / `LSEnvironment` and its launch
method. `HERDR_VM_HOME` selects the private VM directory; `HERDR_APP_HOME`
selects the GUI configuration; `HC_SESSION` selects the core. Export the same
absolute VM path for all helpers. Shell environment changes do not reconfigure
an already running app or necessarily override a Finder-launched bundle.

- `scripts/vm/build` and `scripts/vm/run` are the isolated lab route, with
  `.local/Herdr VM.app`, a separate app home and `HC_SESSION=vmproto`.
- An installed VM-enabled app can use the person's normal workspace. Use its
  configured VM path; do not launch the lab as a substitute for that app.
- Select an existing configured guest when present. Choosing another directory
  requires a coordinated app restart; simultaneous multi-VM tabs are unsupported.

Inspect only non-secret config fields (installed, CPU, RAM, disk, username,
MAC address), file presence, and status. Read passwords only for authentication
or when the person asks for them. Status includes a timestamp and PID: confirm
process identity and fresh SSH state instead of trusting an old `running` file.

| Observed state | Next action |
| --- | --- |
| No configuration or disk | Provision a new private directory. |
| Partial restore download | Resume `prepare`; it resumes the partial download. |
| Valid config with `installed:false` | Inspect installer output and disk ownership, then resume `prepare` on that directory. |
| Installed guest | Skip installation; boot or connect. |
| Disk/identity files but missing or invalid config | Preserve them; diagnose or choose a separate new directory. Never recreate over those files. |
| Another process owns the VM | Identify it and use that session; do not remove the lock or compete for its disk. |

Record completed stages and remaining blockers in a private `setup-notes.md`
inside the VM directory, without credentials. Recheck outcomes on resumption;
the notes are not proof the guest is still reachable.

## Prepare and boot

Reuse the person's stated preferences. For a fresh setup, clarify only relevant
unknowns: intended app/VM location, resource needs, automatic login, and whether
to install computer-use tooling. State the selected defaults before the large
download. The helper currently creates 8 CPUs, 16 GiB RAM, and a 128 GiB sparse
disk (or Apple's higher minimum CPU/RAM). CPU/RAM may be adjusted in config
while the app has released the guest. Disk resizing and custom pre-install
account options are not helper interfaces; do not claim they are supported.

Run from the checkout with the selected `HERDR_VM_HOME`:

```sh
scripts/vm/prepare
```

Finish when installation succeeds and config says `installed:true`. Reusing an
installed directory is a no-op. The helper generates a random account password
and preserves machine identity. Keep config private (0600) and the directory
private (0700). Reserve sufficient real disk space for the restore image and
guest data, even though the disk image is sparse.

Build only if a suitable native-vm app is missing or outdated. Stop the guest
cleanly and quit that app before replacing its executable/library; preserve
its launch settings and unrelated Herdr/core processes. For the lab, use
`scripts/vm/build` then `scripts/vm/run`. For an existing installed app, open
**+ → macOS VM** in a local workspace. The first display attempts boot;
a stopped session has a Start button. An unprepared pane offers setup guidance
and Retry without owning the disk, allowing `prepare` to run separately.

On supported hosts the first boot requests an `agent` account, automatic login,
and Remote Login. Otherwise complete Setup Assistant in the guest and enable
Remote Login there; align the private config's username/password with the
actual guest account. Editing config alone does not change a guest password.

## Unattended access and login

Treat the three forms of “passwordless” separately:

1. **SSH:** run `scripts/vm/guest connect` to install a per-VM public key using
   the saved guest credentials. Then `guest exec 'id; sw_vers'` must succeed
   without prompting. The helper discovers the NAT lease by MAC address and
   keeps host keys under the VM directory. Verify identity before resolving a
   host-key mismatch; a stale lease is not permission to use another machine.
2. **Desktop:** configure automatic login through the guest's supported System
   Settings or the detected version's documented account-management tool. Keep
   a real account password for administrator prompts. Read the guest tool's
   help rather than assuming host flags. Provisioning requests automatic login,
   but that is not proof it works, especially after changing the password.
3. **Administrator commands:** keep password-authenticated sudo by default;
   the shutdown helper supplies the saved password privately. If the user
   specifically requests passwordless sudo, configure a guest-only sudoers
   drop-in, validate it with `visudo`, and test the requested command with
   `sudo -n`. This does not enable desktop automatic login or grant UI permissions.

When changing a guest password, use its supported account tools and update the
private config only after success. Avoid passwords in shell history, process
arguments, or diagnostic output. A simple requested development password is a
user choice, not the skill's default for every guest.

To claim unattended desktop access, perform a coordinated clean shutdown and
boot, then verify the guest reached the desktop without entering credentials.
SSH success alone is insufficient. If macOS rejects automatic-login setup,
report that specific blocker and the remaining manual-login step; do not edit
credential databases or fabricate success.

## Guest computer use

Install Cua Driver **inside the guest**, following its current official
[quickstart](https://cua.ai/docs/cua-driver/quickstart). Inspect the installer
before executing it in the guest. Preserve an existing working installation;
use its CLI help to resolve version/path differences. The tested installation
uses `/Applications/CuaDriver.app` and `~/.local/bin/cua-driver`.

Through `scripts/vm/guest exec`, run the installed CLI's `doctor`,
`permissions status --json`, and, when needed, `permissions grant`. The signed
app's daemon must own the grants. Complete guest Accessibility, Screen Recording,
and any direct-capture consent, accepting a driver restart when requested.
Use the guest UI or hand the specific step to the person; follow the active
computer-use tool's permission rules. A missing grant remains a setup blocker.

Before guest control is available, the person can interact with the native
pane. If an available host Computer Use tool is used for setup, follow its
skill. Native input requires physical key events; generic Unicode/AX text
injection does not pass through VZVirtualMachineView. Once ready, return to
SSH plus the guest driver for ordinary work, including hidden-tab operation.

For U.S.-only input when requested, compile `native/vm/GuestKeyboard.swift`
with AppKit/Carbon, sign it ad hoc, place it in the selected VM's `artifacts`,
and execute `/Volumes/My Shared Files/artifacts/guest-keyboard` inside the guest.
It uses supported input-source APIs. Host synthetic Command shortcuts remain
unverified; test physical keyboard input separately from guest-driver input.

The CLI `guest tool` transport needs no agent-global MCP changes. If MCP is
wanted, register the absolute `scripts/vm/guest` command with argument `mcp`
and an explicit `HERDR_VM_HOME` environment value. Keep the driver in the guest;
a direct host cua-driver registration would control the wrong desktop.

## Verify and hand over

Verify each requested capability with fresh evidence:

- Boot reaches a usable desktop; SSH authenticates with the key.
- Guest reads `artifacts` and writes a unique test result into `results` that
  appears on the host. Only these dedicated directories are shared.
- Driver discovers guest windows, captures an inspected screenshot, types text
  and clicks a control with an observed result. Repeat with the Herdr tab hidden
  when computer use is requested; end the test driver session afterward.
- For app testing, run `scripts/vm/build-smoke-app`, deploy the produced
  `.local/vm/artifacts/Herdr VM Test.app` using `guest deploy`, and inspect fresh
  `deployment.json`, `typing-result.txt`, and `click-result.txt` under the
  selected VM's results directory. Distinguish new outputs from stale ones.
- For unattended login, verify the clean reboot described above. Leave the VM
  running unless the person asked for shutdown.

Report the app and VM paths, verified capabilities, and precise remaining
manual steps. Persist those results in the private setup notes. Installed or
booted is not equivalent to ready for computer use.

## Lifecycle and recovery

Hidden/closed tabs leave the guest running. All VM tabs target the same guest;
reuse its existing tab. Herdr owns the VM process lifetime, independently of
herdr-core's terminal sessions. Quitting requests shutdown and cancels after
30 seconds if the guest has not stopped. No saved-state suspend exists.

`scripts/vm/guest shutdown` requests authenticated normal OS shutdown. Wait
for a fresh `stopped` status before replacing the app, changing VM files, or
switching configurations. The pane's shutdown request can leave a guest prompt
pending. Even a stopped VM may remain owned by the app until it quits; closing
its tab does not release the disk lock. Forced termination can interrupt guest
writes. Preserve disks and identity on recovery; deletion/recreation is a
separate destructive action, not an automatic repair step.
