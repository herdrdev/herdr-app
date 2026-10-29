# OpenBSD platform patches

The application already vendors GPUI. The added `gpui-pre-platform` copy is
the crates.io 0.3.8 source, extended to select the existing BSD backend on
OpenBSD and to provide headless capture there. Existing GPUI base/Linux gates
include OpenBSD alongside FreeBSD; portable queue and Wayland protocol types
are enabled where needed. No Linux-only logind/DRM behavior is enabled globally.

`wgpu`, `wgpu-core` and `wgpu-core-deps-windows-linux-android` are crates.io
29.0.4 sources. Their only source changes add OpenBSD to the existing FreeBSD
backend/platform-feature conditions in `build.rs` and/or `Cargo.toml`.
The underlying wgpu-hal Vulkan backend is unmodified.

These copies retain their upstream licenses. Keep their versions synchronized
with GPUI's wgpu requirements. Remove the copies and Cargo patches when the
upstream releases in use include equivalent OpenBSD platform gates, then rerun
the native build, tests, pixel capture and isolated terminal smoke described
in `docs/openbsd-investigation.md`.
