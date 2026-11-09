#!/bin/sh
# Cursor Agent CLI hooks for Herdr: report Cursor's state as an OSC 7501 root
# record through `hc report`, so the app lists a Cursor pane as an agent
# (working / done / idle) the way it lists Claude Code. Cursor prints no 7501
# itself, and it runs hooks detached, with no terminal to print one to, hence
# `hc report` (herdr-core#81).
#
#   status-hook.sh start   sessionStart        -> idle
#   status-hook.sh prompt  beforeSubmitPrompt  -> working
#   status-hook.sh stop    stop                -> done (error: error, aborted: idle)
#   status-hook.sh end     sessionEnd          -> the record is cleared
#
# No `blocked`: Cursor has no hook for "waiting on your approval", and its
# permission hooks (beforeShellExecution, ...) block the action on any bad output.
#
# Install by hand, see README.md (~/.cursor/hooks.json, beside any hooks already there):
#   "sessionStart":       [{"command": "sh /path/to/status-hook.sh start", "timeout": 5}],
#   "beforeSubmitPrompt": [{"command": "sh /path/to/status-hook.sh prompt", "timeout": 5}],
#   "stop":               [{"command": "sh /path/to/status-hook.sh stop", "timeout": 5}],
#   "sessionEnd":         [{"command": "sh /path/to/status-hook.sh end", "timeout": 5}]
#
# Outside a Herdr terminal (no HC_TERMINAL_ID) it reports nothing. It never
# fails a hook: Cursor gets its answer whatever hc does.
# Env: HERDR_HC (the hc to run, default hc on PATH).
set -u
event=${1:-}
input=$(cat 2>/dev/null || true)

case $event in
  start) body="state=idle:app=cursor" ;;
  prompt) body="state=working:app=cursor" ;;
  stop)
    status=$(printf '%s' "$input" | jq -r '.status // empty' 2>/dev/null || true)
    case $status in
      error) body="state=error:app=cursor" ;;
      aborted) body="state=idle:app=cursor" ;;
      *) body="state=done:app=cursor" ;;
    esac
    ;;
  end) body="state=clear" ;;
  *) body= ;;
esac

if [ -n "$body" ] && [ -n "${HC_TERMINAL_ID:-}" ]; then
  "${HERDR_HC:-hc}" report "$body" > /dev/null 2>&1 || true
fi

# beforeSubmitPrompt must say whether the prompt goes on; the others want an object.
if [ "$event" = prompt ]; then echo '{"continue":true}'; else echo '{}'; fi
