# OpenBSD feasibility check

Checked locally on 2026-10-29, against upstream commit
`863b270b9914e073ad60b33d1cc69d015bdd3bbf` (app version 0.1.7).

## Result

The desktop app does not currently provide an OpenBSD build path. Getting
it running requires a port of its dependencies, plus a separate installation
of the new herdr-core. The existing Herdr terminal application is not that
core and was not replaced or reconfigured during this investigation.

No desktop executable or working app window has been produced. The observations
below distinguish executed probes from source inspection.

## Local environment: executed probes

- `uname -a`: OpenBSD 8.0, amd64.
- `rustc --version`: 1.99.1; `cargo --version`: 1.99.1.
- Xorg and an Xfce session are running on display `:0`.
  This agent does not inherit `DISPLAY`.
- Vulkan headers and loader are installed; Mesa ICD manifests are under
  `/usr/X11R6/share/vulkan/icd.d`. The GPU is an AMD Cezanne integrated GPU.
- A minimal Vulkan instance probe using Python ctypes returned
  `VK_ERROR_INCOMPATIBLE_DRIVER` (-9) with default environment discovery.
  With `VK_DRIVER_FILES=/usr/X11R6/share/vulkan/icd.d/radeon_icd.x86_64.json`,
  `vkCreateInstance` and `vkEnumeratePhysicalDevices` both returned success,
  and one physical device was found. This establishes instance/device
  enumeration only, not GPUI rendering, presentation, or application startup.
- `/usr/local/bin/herdr --version`: `herdr 0.9.3`.
  Neither `hc` nor `herdr-core` is on this agent's PATH.
- Zig and just are absent. The configured package repository lists
  `zig-0.16.0`, matching upstream's requested Zig version; it was not installed.
- The repository and its pinned Ghostty submodule were cloned successfully.
- The unmodified `cargo check --locked` exited with status 101. The first
  compiler error was a missing
  `quickjs-jit-…/sys/src/bindings/x86_64-unknown-openbsd.rs` file.
  The build itself recommended the `bindgen` feature. The local log is
  `/tmp/herdr-app-openbsd-check.log`.

## Binding-generation experiment

An experimental OpenBSD-only QuickJS dependency enabled `bindgen` for the
same pinned QuickJS revision already used by gpui-shell. No dependency versions
were upgraded. This experimental change was removed after the isolated probe
below failed; this branch retains the investigation report only.

The follow-up command uses the installed libclang explicitly:

```sh
LIBCLANG_PATH=/usr/local/llvm22/lib cargo check
```

The follow-up exited with status 101 at the Ghostty build script:

```text
ghostty-sys: no Zig target for x86_64-unknown-openbsd
```

Its local log is `/tmp/herdr-app-openbsd-bindgen-check.log`.

An additional isolated check with the experimental manifest,
`LIBCLANG_PATH=/usr/local/llvm22/lib cargo check --locked -p quickjs-jit-sys`,
also exited with status 101. Bindgen wrote an effectively empty bindings file;
the Rust compiler then reported missing types such as `JSJitRuntimeAPI` and
`JSValue`. Its log is `/tmp/herdr-app-openbsd-quickjs-check.log`. Enabling
`bindgen` alone is therefore insufficient with the tested toolchain. The
underlying cause of the empty output has not been established.

`cargo metadata --locked --format-version 1 --no-deps` and
`git diff --check` passed. These checks do not establish build success.

## Remaining porting requirements

### GPUI platform selection

`gpui-pre-platform` 0.3.8 has branches for macOS, Windows, Linux/FreeBSD,
and WebAssembly in `current_platform`. It has no OpenBSD branch. Its
`gpui_linux` dependency is also gated to Linux/FreeBSD.

The vendored `vendor/gpui-pre-linux/src/gpui_linux.rs` gates the entire crate
to Linux/FreeBSD, and the crate's dependency table uses the same condition.
The vendored base GPUI crate has corresponding platform gates. Adding an OS
name only in the application's Cargo.toml cannot make these crates work.

An initial port should target X11, then check the event loop, platform
dispatcher, font system, clipboard, notifications, renderer and frame capture.
Linux-specific assumptions need individual review; for example,
`vendor/gpui-pre-linux/src/linux/x11/client.rs` constructs `/proc/self/fd/…`
paths. Whether that path is exercised by the app remains untested.

### Ghostty VT library

`crates/ghostty-sys/build.rs::zig_target` has an exhaustive mapping of supported
Rust triples. `x86_64-unknown-openbsd` falls into
`panic!("ghostty-sys: no Zig target for {other}")` before Zig is invoked.

This panic was also observed in the follow-up build. The target mapping must
be extended, Zig 0.16 installed, and the pinned
Ghostty VT library built and linked on OpenBSD. A mapping change alone does
not establish that Ghostty's Zig dependencies support this target.

### JavaScript runtime

`vendor/quickjs-jit-runtime/src/platform/mod.rs` selects native executable
memory backends for Linux, macOS and Windows; OpenBSD selects `unsupported`.
The runtime also has interpreter paths. Verify interpreter initialization and
mod execution before deciding whether a native JIT port is necessary.
No JIT failure has been observed at runtime in this investigation.

### Separate herdr-core

`src/core_bin.rs::version` accepts binaries reporting a version starting with
`hc `. The installed `herdr 0.9.3` does not satisfy that check.
`distribution/core-version` requires core 0.1.11 or newer, and `src/hc.rs`
requires API generation 1 and the listed terminal/document/event methods.

Build or port herdr-core separately, install it alongside the existing
`herdr`, and test with an isolated core and `HERDR_APP_HOME`. Do not substitute
a symlink from `hc` to the existing `herdr` binary.

## Suggested validation order

1. Build the standalone Ghostty VT library on OpenBSD using Zig 0.16.
2. Add an OpenBSD X11 path across GPUI platform/base/Linux dependencies and
   resolve the first compiler errors without changing other platforms.
3. Verify the JavaScript interpreter and shipped mods on OpenBSD.
4. Build a compatible herdr-core alongside the existing Herdr installation.
5. Start the app with an isolated home/core and a private test display.
   Verify drawing, keyboard input, terminal output, resize and reconnection.
6. Add OpenBSD installation instructions only after the runtime checks pass.

Upstream documents desktop releases for Linux, macOS and Windows:
<https://github.com/herdrdev/herdr-app#install>.
