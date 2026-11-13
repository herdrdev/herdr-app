#!/bin/sh
# Shared by startup core discovery and the Settings preflight check.
check() {
    for f do
        # WSL interop can run a Windows hc.exe; it is not a guest core.
        case "$f" in *.[eE][xX][eE]) continue ;; esac
        [ -f "$f" ] && [ -x "$f" ] || continue
        v=$("$f" --version 2>/dev/null) || continue
        case "$v" in 'hc '*) printf '%s\n' "$f"; exit 0 ;; esac
    done
}
case "$1" in [a-zA-Z]:*|\\\\*) set -- "$(wslpath -u "$1" 2>/dev/null)" ;; esac
case "$1" in \~/*) check "$HOME/${1#\~/}" ;; ?*) check "$1" ;; esac
s=${SHELL:-/bin/sh}
# Split command-v output on newlines only, without expanding glob characters.
set -f
IFS='
'
for f in $("$s" -lic 'command -v herdr-core; command -v hc' 2>/dev/null) $("$s" -lc 'command -v herdr-core; command -v hc' 2>/dev/null); do check "$f"; done
for d in "$HOME/.local/bin" "$HOME/.cargo/bin" /usr/local/bin /usr/bin /home/linuxbrew/.linuxbrew/bin "$HOME/.nix-profile/bin" /run/current-system/sw/bin "$HOME/.local/share/mise/shims"; do check "$d/herdr-core" "$d/hc"; done
IFS=:
for d in $PATH; do [ -n "$d" ] && check "$d/herdr-core" "$d/hc"; done
# Distinguish a completed search with no guest core from a failed WSL launch.
exit 3
