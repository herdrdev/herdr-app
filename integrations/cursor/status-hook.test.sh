#!/bin/sh
# sh integrations/cursor/status-hook.test.sh  (from the repo root)
# Runs status-hook.sh with a fake hc that records what it was asked to report.
set -u
here=$(cd "$(dirname "$0")" && pwd)
hook="$here/status-hook.sh"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/hc" <<'FAKE'
#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_LOG"
FAKE
chmod +x "$tmp/hc"
export FAKE_LOG="$tmp/log" HERDR_HC="$tmp/hc"
fails=0

# check <name> <event> <stdin> <expected hc args, or - for none> <expected stdout>
check() {
  : > "$FAKE_LOG"
  out=$(printf '%s' "$3" | sh "$hook" "$2")
  got=$(cat "$FAKE_LOG"); [ -n "$got" ] || got=-
  if [ "$got" != "$4" ] || [ "$out" != "$5" ]; then
    echo "FAIL $1: hc '$got' (want '$4'), stdout '$out' (want '$5')"; fails=$((fails + 1))
  else
    echo "ok   $1"
  fi
}

export HC_TERMINAL_ID=t7
check "session start: an idle agent"     start  '{"session_id":"s"}'      "report state=idle:app=cursor"    '{}'
check "prompt: working, and lets it go"  prompt '{"prompt":"hi"}'         "report state=working:app=cursor" '{"continue":true}'
check "stop completed: done"             stop   '{"status":"completed"}'  "report state=done:app=cursor"    '{}'
check "stop error: error"                stop   '{"status":"error"}'      "report state=error:app=cursor"   '{}'
check "stop aborted: idle"               stop   '{"status":"aborted"}'    "report state=idle:app=cursor"    '{}'
check "stop without a status: done"      stop   '{}'                      "report state=done:app=cursor"    '{}'
check "stop with junk input: done"       stop   'not json'                "report state=done:app=cursor"    '{}'
check "session end: clears"              end    '{"reason":"completed"}'  "report state=clear"              '{}'
check "unknown event: nothing"           what   '{}'                      -                                 '{}'

# A failing hc must never block Cursor: the prompt still goes through.
printf '#!/bin/sh\nexit 1\n' > "$tmp/hc"
check "hc fails: prompt still continues" prompt '{}'                      -                                 '{"continue":true}'

unset HC_TERMINAL_ID
printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "$FAKE_LOG"\n' > "$tmp/hc"
check "outside Herdr: nothing reported"  prompt '{}'                      -                                 '{"continue":true}'

[ "$fails" -eq 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
