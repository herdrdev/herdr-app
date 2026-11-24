#!/bin/sh
# Installs the Herdr app, and herdr-core if there's none yet:
#
#   curl -fsSL https://releases.herdr.dev/app/install.sh | sh
#
# Linux: the app into ~/.local/share/herdr-app, `herdr-app` in ~/.local/bin,
# a launcher entry and icon. macOS: Herdr.app into ~/Applications, keeping
# its release signature intact (ad hoc for unsigned/dev builds). The core, with its
# own installer, into ~/.local/bin/herdr-core (and `hc`) only if none is
# there yet; the app
# keeps it current after that. Running it again updates the app.
set -eu

releases=${HERDR_RELEASES_URL:-https://releases.herdr.dev}
bin="$HOME/.local/bin"

say() { printf '%s\n' "$*"; }
fail() { say "herdr: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "needs $1"; }
need curl
need tar

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  MINGW* | MSYS* | CYGWIN*) fail "this installer is for Linux and macOS; on Windows, run this in PowerShell: irm $releases/app/install.ps1 | iex" ;;
  *) fail "no build for $(uname -s)" ;;
esac
if [ "$os" = macos ]; then
  # This installer is published alone: keep in sync with .cargo/config.toml.
  min_macos=13.0
  macos_version=$(SYSTEM_VERSION_COMPAT=0 sw_vers -productVersion) || fail "cannot determine macOS version"
  case $macos_version in
    '' | *[!0-9.]*) fail "cannot determine macOS version: $macos_version" ;;
  esac
  if ! awk -v have="$macos_version" -v need="$min_macos" 'BEGIN {
    if (have !~ /^[0-9]+\.[0-9]+(\.[0-9]+)?$/) exit 1
    split(have, h, "."); split(need, n, ".")
    for (i = 1; i <= 3; i++) {
      if (h[i] + 0 > n[i] + 0) exit 0
      if (h[i] + 0 < n[i] + 0) exit 1
    }
    exit 0
  }'; then
    fail "requires macOS $min_macos or newer (found $macos_version)"
  fi
fi
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) fail "no build for $(uname -m)" ;;
esac
if [ "$os" = macos ] && [ "$arch" = x86_64 ]; then
  # Rosetta reports x86_64 even on Apple Silicon.
  if [ "$(/usr/sbin/sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ] ||
     [ "$(/usr/sbin/sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
    arch=aarch64
  fi
fi
target="$os-$arch"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

tmp=$(mktemp -d)
stage= lock= install_dest= old_id= new_id=
complete=false
tree_id() {
  if [ "$os" = macos ]; then /usr/bin/stat -f '%d:%i' "$1"
  else stat -c '%d:%i' "$1"; fi
}
cleanup() {
  result=$?
  trap - EXIT HUP INT TERM
  set +e
  keep=false
  if [ "$complete" != true ] && [ -n "$stage" ] && { [ -e "$stage/previous" ] || [ -L "$stage/previous" ]; }; then
    # Move aside only the exact new directory we placed, never a concurrent
    # replacement. The previous app stays recoverable if restoration fails.
    if [ -e "$install_dest" ] || [ -L "$install_dest" ]; then
      if [ "$(tree_id "$install_dest")" = "$new_id" ]; then
        mv "$install_dest" "$stage/failed-new"
      fi
    fi
    if [ ! -e "$install_dest" ] && [ ! -L "$install_dest" ] &&
       mv "$stage/previous" "$install_dest" && [ "$(tree_id "$install_dest")" = "$old_id" ]; then
      say "herdr: previous app restored at $install_dest" >&2
    else
      say "herdr: cannot verify restoration; inspect $install_dest and $stage/previous. Recovery lock: $lock. Do not retry before recovering the previous app." >&2
      keep=true
      result=1
    fi
  fi
  if [ "$keep" != true ]; then
    [ -z "$stage" ] || rm -rf "$stage"
    [ -z "$lock" ] || rmdir "$lock"
  fi
  rm -rf "$tmp"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

install_tree() {
  source=$1
  install_dest=$2
  parent=$(dirname "$install_dest")
  # The lock and staging directory live beside the destination: all swaps
  # are same-filesystem renames, even when TMPDIR is on another volume.
  candidate_lock="$parent/.herdr-app-install.lock"
  mkdir "$candidate_lock" 2>/dev/null || fail "installation locked at $candidate_lock; check for another installer or unfinished recovery"
  lock=$candidate_lock
  stage=$(mktemp -d "$parent/.herdr-install.XXXXXXXX")
  if [ "$os" = macos ]; then /usr/bin/ditto "$source" "$stage/new"
  else cp -R "$source" "$stage/new"; fi
  new_id=$(tree_id "$stage/new")
  if [ -e "$install_dest" ] || [ -L "$install_dest" ]; then
    old_id=$(tree_id "$install_dest")
    [ "${old_id%%:*}" = "${new_id%%:*}" ] || fail "previous app is on another filesystem; it was not replaced"
    mv "$install_dest" "$stage/previous" || fail "cannot move previous app aside; it was not replaced"
  fi
  mv "$stage/new" "$install_dest" || fail "cannot put the new app in place"
  [ "$(tree_id "$install_dest")" = "$new_id" ] || fail "cannot verify installed app directory"
}

curl -fsSL "$releases/app/latest.json" -o "$tmp/latest.json" || fail "cannot reach $releases"
# Read without jq: the manifest is flat enough for sed once spaces are gone.
entry=$(tr -d '\n ' < "$tmp/latest.json" | sed -n "s/.*\"$target\":{\([^}]*\)}.*/\1/p")
url=$(printf '%s' "$entry" | sed -n 's/.*"url":"\([^"]*\)".*/\1/p')
want=$(printf '%s' "$entry" | sed -n 's/.*"sha256":"\([^"]*\)".*/\1/p')
version=$(tr -d '\n ' < "$tmp/latest.json" | sed -n 's/^{"version":"\([^"]*\)".*/\1/p')
[ -n "$url" ] && [ -n "$want" ] || fail "no build for $target yet"

# Tells the release server's download counter a new install from a reinstall;
# the file is the same either way. Only plain URLs on the release server in
# use are marked: one with a query or fragment (a mirror's signed URL) is
# fetched exactly as the manifest gives it.
src=install
for have in "${XDG_DATA_HOME:-$HOME/.local/share}/herdr-app" "$HOME/Applications/Herdr.app" /Applications/Herdr.app; do
  if [ -e "$have" ]; then src=reinstall; fi
done
case $url in
  *[?#]*) download=$url ;;
  "$releases"/*) download="$url?src=$src" ;;
  *) download=$url ;;
esac

say "Installing Herdr ${version:-} for $target..."
curl -fsSL "$download" -o "$tmp/app.tar.gz" || fail "download failed"
[ "$(sha256 "$tmp/app.tar.gz")" = "$want" ] || fail "download is damaged (checksum mismatch); try again"
tar -xzf "$tmp/app.tar.gz" -C "$tmp"
mkdir -p "$bin"

if [ "$os" = linux ]; then
  data=${XDG_DATA_HOME:-$HOME/.local/share}
  home="$data/herdr-app"
  mkdir -p "$data"
  install_tree "$tmp/herdr-app" "$home"
  ln -sfn "$home/herdr-app" "$bin/herdr-app"
  mkdir -p "$data/applications" "$data/icons/hicolor/scalable/apps"
  cp "$home/herdr-app.svg" "$data/icons/hicolor/scalable/apps/herdr-app.svg"
  cat > "$data/applications/herdr-app.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Herdr
Comment=Terminals and coding agents that keep running
Exec=$bin/herdr-app
Icon=herdr-app
Terminal=false
Categories=Development;
StartupWMClass=herdr-app
EOF
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$data/applications" 2>/dev/null || true
  [ -f "$data/icons/hicolor/icon-theme.cache" ] && gtk-update-icon-cache -f -t "$data/icons/hicolor" >/dev/null 2>&1 || true
  # Plasma remembers a missing icon until it is told the theme changed,
  # so a fresh install shows an empty page in the taskbar.
  command -v dbus-send >/dev/null 2>&1 &&
    dbus-send --session --type=signal /KIconLoader org.kde.KIconLoader.iconChanged int32:0 2>/dev/null || true
else
  apps="$HOME/Applications"
  app="$apps/Herdr.app"
  mkdir -p "$apps"
  # Prepared in the temp folder, so Spotlight never indexes a half-made
  # bundle under a temporary name in Applications.
  # Never change a certificate-signed bundle: macOS permissions follow its
  # signing identity. Only unsigned/ad-hoc development builds need this PATH.
  expected_arch=$arch
  [ "$arch" != aarch64 ] || expected_arch=arm64
  actual_arch=$(/usr/bin/lipo -archs "$tmp/Herdr.app/Contents/MacOS/Herdr") || fail "cannot read app architecture"
  [ "$actual_arch" = "$expected_arch" ] || fail "expected $expected_arch app, found $actual_arch; previous app was not replaced"
  signed=false
  if signature=$(/usr/bin/codesign -dvv "$tmp/Herdr.app" 2>&1); then
    if printf '%s\n' "$signature" | grep -q '^Authority=Developer ID Application:'; then
      signed=true
    elif ! printf '%s\n' "$signature" | grep -qx 'Signature=adhoc'; then
      fail "cannot determine app signature: $signature"
    fi
  elif ! printf '%s\n' "$signature" | grep -q ': code object is not signed at all$'; then
    fail "cannot determine app signature: $signature"
  fi
  if [ "$signed" = true ]; then
    /usr/bin/codesign --verify --deep --strict "$tmp/Herdr.app" || fail "invalid app signature"
  else
    /usr/libexec/PlistBuddy -c "Add :LSEnvironment:PATH string $bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin" "$tmp/Herdr.app/Contents/Info.plist"
    /usr/bin/codesign --force --sign - "$tmp/Herdr.app" >/dev/null 2>&1
  fi
  install_tree "$tmp/Herdr.app" "$app"
  ln -sfn "$app/Contents/MacOS/Herdr" "$bin/herdr-app"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app" 2>/dev/null || true
fi

# The core: installed only if there's none yet. One that's already there
# (yours, or from a package manager) is left alone; the app updates it.
if [ ! -e "$bin/herdr-core" ] && [ ! -L "$bin/herdr-core" ] && ! command -v herdr-core >/dev/null 2>&1; then
  # POSIX sh has no pipefail: never execute an empty or partial download.
  if ! curl -fsSL "$releases/core/install.sh" -o "$tmp/install-core.sh"; then
    say "herdr: herdr-core download failed; the app is installed. Open Herdr to install Core, or retry this installer when the download is available." >&2
  elif ! HC_INSTALL_DIR="$bin" HERDR_RELEASES_URL="$releases" sh "$tmp/install-core.sh"; then
    say "herdr: herdr-core didn't install; open Herdr to retry its Core setup." >&2
  elif [ ! -e "$bin/herdr-core" ] && ! command -v herdr-core >/dev/null 2>&1; then
    say "herdr: herdr-core is still missing; open Herdr to retry its Core setup." >&2
  fi
fi

complete=true
say "Herdr is installed. Open it from your apps, or run: herdr-app"
case ":$PATH:" in
  *":$bin:"*) ;;
  *) say "Add $bin to your PATH to use herdr-app and hc from a terminal." ;;
esac
