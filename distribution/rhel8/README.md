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

## Files

| File | What it is |
|---|---|
| `herdr-app` | the desktop app |
| `herdr-app-rhel8` | launcher: the app with the 1x X11 scale and 1 rasterizer thread defaults |
| `herdr-core` | the core the app connects to |
| `hc` | symlink to `herdr-core` |
| `herdr-app.svg` | the icon |
| `LICENSE`, `THIRD-PARTY-NOTICES` | licenses |
