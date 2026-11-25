# Herdr for RHEL 8 (x86_64)

Built inside AlmaLinux 8, so both binaries run on glibc 2.28 and newer. The
tarball carries the app and its core: there is no separate install step.

## Run

    ./herdr-app-rhel8

That starts the app with two defaults for this build:

- `GPUI_X11_SCALE_FACTOR=1`: RHEL 8 X11 desktops often report `Xft.dpi: 192`
  on a 1080p screen, which makes GPUI lay the window out at 2x and render more
  pixels than the screen.
- `LP_NUM_THREADS=1`: Mesa's CPU rasterizer (llvmpipe/lavapipe) otherwise uses
  one thread per core and competes with the compositor and the core. One thread
  keeps typing responsive on a small VM; raise it on a machine with cores to
  spare.

To use the detected scale instead, run `./herdr-app` directly, or set
`GPUI_X11_SCALE_FACTOR=2` (or any positive number) before launching. Any value
you set yourself wins, so these also work:

    GPUI_X11_SCALE_FACTOR=2 ./herdr-app-rhel8
    LP_NUM_THREADS=4 ./herdr-app-rhel8

`herdr-core` and the `hc` symlink sit next to the app; it finds them there.
Set `HERDR_APP_HOME` to keep settings, layouts and mods in a different folder.

## When it is still slow

The app renders with whatever wGPU finds. On a machine with no GPU driver
(llvmpipe/lavapipe), this build already:

- presents with `Immediate` and one queued frame instead of waiting for vsync
  (`Fifo`, two frames), so a late frame is not held back;
- rasterizes paths with 1x MSAA instead of 4x (the path pass writes a
  full-window intermediate texture, so this is a real saving on a CPU);
- keeps subpixel text and everything else as before.

`Immediate` can tear while typing. These variables override the defaults, so
you can A/B one at a time:

| Variable | What it does |
|---|---|
| `HERDR_APP_PRESENT_MODE=fifo\|mailbox\|immediate` | force a present mode (`fifo` restores vsync) |
| `HERDR_APP_FRAME_LATENCY=1..4` | frames the driver may queue; default is 1 with `Immediate`, else 2 |
| `HERDR_APP_PATH_MSAA=1\|2\|4` | path antialiasing; default is 1 on a CPU adapter |
| `GPUI_WGPU_BACKEND=vulkan\|gl` | use only one wGPU backend; comma-separate to allow both |
| `GPUI_WGPU_FRAME_LOG=1` | log frames/s and acquire/render/present milliseconds once a second |
| `HERDR_APP_LOG=info` | show the adapter, backend, present mode, MSAA and X11 refresh rate at startup |

Example, forcing OpenGL (llvmpipe) and logging frame times:

    GPUI_WGPU_BACKEND=gl GPUI_WGPU_FRAME_LOG=1 HERDR_APP_LOG=info ./herdr-app-rhel8

If the window is fast when small and slow when maximized, the cost is the
number of pixels; if it is slow either way, the X11 compositor or the core is
the limit and these variables will not fix it. `x11: compositor present: true`
in the startup log means every frame is copied again by the desktop.

## Files

| File | What it is |
|---|---|
| `herdr-app` | the desktop app |
| `herdr-app-rhel8` | launcher: the app with the 1x X11 scale and 1 rasterizer thread defaults |
| `herdr-core` | the core the app connects to |
| `hc` | symlink to `herdr-core` |
| `herdr-app.svg` | the icon |
| `LICENSE`, `THIRD-PARTY-NOTICES` | licenses |
