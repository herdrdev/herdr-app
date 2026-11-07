#!/usr/bin/env python3
"""Switch this machine from the official Herdr app to this checkout's custom build.

The official installer (distribution/install.sh) puts the app in ~/.local/share/herdr-app and
links ~/.local/bin/herdr-app to it. This script:

  1. optionally builds the release binary (--build: Zig 0.16 + sccache when present),
  2. moves ~/.local/share/herdr-app aside to ~/.local/share/herdr-app.official-<time>
     (or deletes it with --purge),
  3. links ~/.local/bin/herdr-app to <checkout>/target/release/herdr-app,
  4. writes the launcher entry and icon, as scripts/install-linux does.

herdr-core (`hc`) and ~/.config/herdr-app are left alone: settings, layouts, mods and spaces
carry over, and terminals keep running. A source build never updates itself.
Quit and reopen Herdr afterwards to run the custom build.

  use-custom-build.py [--build] [--purge] [--dry-run]
  use-custom-build.py --restore      put the newest official backup back
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

HOME = Path.home()
DATA = Path(os.environ.get("XDG_DATA_HOME") or HOME / ".local/share")
BIN_LINK = HOME / ".local/bin/herdr-app"
OFFICIAL = DATA / "herdr-app"
CHECKOUT = Path(os.path.abspath(__file__)).parent.parent  # keep ~/Projects/herdr-app, not the resolved path
CUSTOM = CHECKOUT / "target/release/herdr-app"
ZIG = HOME / ".local/opt/zig-x86_64-linux-0.16.0/zig"
SCCACHE = HOME / ".local/bin/sccache"
DESKTOP = DATA / "applications/herdr-app.desktop"
ICON = DATA / "icons/hicolor/scalable/apps/herdr-app.svg"

dry = False


def step(msg: str) -> None:
    print(("[dry-run] " if dry else "") + msg)


def build() -> None:
    env = dict(os.environ)
    if ZIG.exists():
        env.setdefault("ZIG", str(ZIG))
    if SCCACHE.exists():
        env.setdefault("RUSTC_WRAPPER", str(SCCACHE))
    step(f"cargo build --locked --release in {CHECKOUT}")
    if not dry:
        subprocess.run(["cargo", "build", "--locked", "--release"], cwd=CHECKOUT, env=env, check=True)


def link(target: Path) -> None:
    step(f"link {BIN_LINK} -> {target}")
    if not dry:
        BIN_LINK.parent.mkdir(parents=True, exist_ok=True)
        if BIN_LINK.is_symlink() or BIN_LINK.exists():
            BIN_LINK.unlink()
        BIN_LINK.symlink_to(target)


def launcher(icon_src: Path) -> None:
    step(f"write {DESKTOP} and icon")
    if dry:
        return
    ICON.parent.mkdir(parents=True, exist_ok=True)
    DESKTOP.parent.mkdir(parents=True, exist_ok=True)
    if icon_src.exists():
        shutil.copyfile(icon_src, ICON)
    DESKTOP.write_text(f"""[Desktop Entry]
Type=Application
Name=Herdr
Comment=Terminals and coding agents that keep running
Exec={BIN_LINK}
Icon=herdr-app
Terminal=false
Categories=Development;
StartupWMClass=herdr-app
""")
    subprocess.run(["update-desktop-database", str(DESKTOP.parent)], capture_output=True)
    cache = DATA / "icons/hicolor/icon-theme.cache"
    if cache.exists():
        subprocess.run(["gtk-update-icon-cache", "-f", "-t", str(DATA / "icons/hicolor")], capture_output=True)


def use_custom(purge: bool, do_build: bool) -> None:
    if do_build:
        build()
    if not CUSTOM.exists() and not dry:
        sys.exit(f"No custom build at {CUSTOM}. Run with --build, or build it first.")
    if OFFICIAL.is_dir():
        if purge:
            step(f"delete official app {OFFICIAL}")
            if not dry:
                shutil.rmtree(OFFICIAL)
        else:
            backup = OFFICIAL.with_name(f"herdr-app.official-{time.strftime('%Y%m%d-%H%M%S')}")
            step(f"move official app {OFFICIAL} -> {backup}")
            if not dry:
                OFFICIAL.rename(backup)
    else:
        step(f"no official app at {OFFICIAL} (already switched?)")
    link(CUSTOM)
    launcher(CHECKOUT / "assets/icons/herdr-ui-icon-clean.svg")
    print(f"\nUsing the custom build: {CUSTOM}\nQuit and reopen Herdr to run it.")


def restore() -> None:
    backups = sorted(DATA.glob("herdr-app.official-*"))
    if not backups:
        sys.exit(f"No official backup in {DATA} (herdr-app.official-*). Reinstall: "
                 "curl -fsSL https://herdr.dev/install.sh | sh")
    if OFFICIAL.exists():
        sys.exit(f"{OFFICIAL} already exists; move it away first.")
    step(f"move {backups[-1]} -> {OFFICIAL}")
    if not dry:
        backups[-1].rename(OFFICIAL)
    link(OFFICIAL / "herdr-app")
    launcher(OFFICIAL / "herdr-app.svg")
    print("\nBack on the official app (it updates itself again). Quit and reopen Herdr.")


def main() -> None:
    global dry
    p = argparse.ArgumentParser(description=(__doc__ or "").split("\n\n")[0])
    p.add_argument("--build", action="store_true", help="build the release binary first")
    p.add_argument("--purge", action="store_true", help="delete the official app instead of keeping a backup")
    p.add_argument("--restore", action="store_true", help="go back to the newest official backup")
    p.add_argument("--dry-run", action="store_true", help="print what would happen")
    a = p.parse_args()
    dry = a.dry_run
    restore() if a.restore else use_custom(a.purge, a.build)


if __name__ == "__main__":
    main()
