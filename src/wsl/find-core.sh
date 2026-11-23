#!/bin/sh
mode=${2:-find}
check() {
    for f do
        # WSL interop can run a Windows hc.exe; it is not a guest core.
        case "$f" in *.[eE][xX][eE]) continue ;; esac
        case "$f" in */*) ;; *) f=$(command -v -- "$f") || continue ;; esac
        [ -f "$f" ] && [ -x "$f" ] || continue
        # Inspect the target bytes before executing it: interop also accepts renamed PE files.
        magic=$(LC_ALL=C command -p od -An -N2 -tx1 < "$f") || continue
        case "$magic" in *'4d 5a') continue ;; esac
        v=$("$f" --version 2>/dev/null) || continue
        case "$v" in 'hc '*)
            if [ "$mode" = version ]; then printf '%s\n' "$v"; else printf '%s\n' "$f"; fi
            exit 0 ;;
        esac
    done
}
if [ "$mode" = installed ]; then
    check "${HC_INSTALL_DIR:-$HOME/.local/bin}/herdr-core"
    exit 3
fi
case "$1" in [a-zA-Z]:*|\\\\*) set -- "$(wslpath -u "$1" 2>/dev/null)" ;; esac
case "$1" in \~/*) check "$HOME/${1#\~/}" ;; ?*) check "$1" ;; esac
[ "$mode" = version ] && exit 3
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
