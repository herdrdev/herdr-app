# Herdr for RHEL 8 (x86_64)

Built inside AlmaLinux 8, so both binaries run on glibc 2.28 and newer. The
tarball carries the app and its core: there is no separate install step.

## Run

    ./herdr-app-rhel8

That starts the app with `GPUI_X11_SCALE_FACTOR=1` for this build. RHEL 8 X11
desktops often report `Xft.dpi: 192` on a 1080p screen, which makes GPUI lay
the window out at 2x and render more pixels than the screen.

To use the detected scale instead, run `./herdr-app` directly, or set
`GPUI_X11_SCALE_FACTOR=2` (or any positive number) before launching:

    GPUI_X11_SCALE_FACTOR=2 ./herdr-app-rhel8

`herdr-core` and the `hc` symlink sit next to the app; it finds them there.
Set `HERDR_APP_HOME` to keep settings, layouts and mods in a different folder.

## Files

| File | What it is |
|---|---|
| `herdr-app` | the desktop app |
| `herdr-app-rhel8` | launcher: the app with the 1x X11 scale default |
| `herdr-core` | the core the app connects to |
| `hc` | symlink to `herdr-core` |
| `herdr-app.svg` | the icon |
| `LICENSE`, `THIRD-PARTY-NOTICES` | licenses |
