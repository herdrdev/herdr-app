# Cursor Agent CLI status

Cursor Agent CLI (`cursor-agent`) doesn't report its status with OSC 7501, so Herdr shows a Cursor pane as a program, not as an agent. `status-hook.sh` (macOS and Linux) and `status-hook.ps1` (Windows) report it for Cursor from Cursor's own hooks:

| Cursor hook | Status |
|---|---|
| `sessionStart` | `idle` |
| `beforeSubmitPrompt` | `working` |
| `stop` | `done` (`error` if it errored, `idle` if aborted) |
| `sessionEnd` | cleared |

It needs a core with `hc report` ([herdr-core#81](https://github.com/herdrdev/herdr-core/pull/81)). With an older core it reports nothing and Cursor runs as before. There's no `blocked` state: Cursor has no hook for "waiting on your approval".

Once Cursor reports OSC 7501 itself, remove the hook.

## Install

Herdr doesn't install it. You add it to Cursor yourself, or ask your agent to.

### macOS and Linux

1. Copy `status-hook.sh` somewhere it stays, for example `~/.cursor/herdr-status-hook.sh`.
2. Add these entries to `~/.cursor/hooks.json`, beside any hooks already there:

   ```json
   {
     "version": 1,
     "hooks": {
       "sessionStart":       [{"command": "sh ~/.cursor/herdr-status-hook.sh start",  "timeout": 5}],
       "beforeSubmitPrompt": [{"command": "sh ~/.cursor/herdr-status-hook.sh prompt", "timeout": 5}],
       "stop":               [{"command": "sh ~/.cursor/herdr-status-hook.sh stop",   "timeout": 5}],
       "sessionEnd":         [{"command": "sh ~/.cursor/herdr-status-hook.sh end",    "timeout": 5}]
     }
   }
   ```

3. Start a new Cursor session.

Cursor runs hooks with its own environment, so `hc` must be on that `PATH`. If it isn't, put `HERDR_HC=/path/to/hc ` in front of each command. Outside a Herdr terminal the hook does nothing.

It needs `sh`, and `jq` to tell a finished turn from an error or an abort (without it, every turn ends `done`).

### Windows

1. Copy `status-hook.ps1` somewhere it stays, for example `%USERPROFILE%\.cursor\herdr-status-hook.ps1`.
2. Add these entries to `%USERPROFILE%\.cursor\hooks.json`, beside any hooks already there, with your own path to the script:

   ```json
   {
     "version": 1,
     "hooks": {
       "sessionStart":       [{"command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\Users\\you\\.cursor\\herdr-status-hook.ps1 start",  "timeout": 5}],
       "beforeSubmitPrompt": [{"command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\Users\\you\\.cursor\\herdr-status-hook.ps1 prompt", "timeout": 5}],
       "stop":               [{"command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\Users\\you\\.cursor\\herdr-status-hook.ps1 stop",   "timeout": 5}],
       "sessionEnd":         [{"command": "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\Users\\you\\.cursor\\herdr-status-hook.ps1 end",    "timeout": 5}]
     }
   }
   ```

3. Start a new Cursor session.

`hc` must be on Cursor's `PATH`; if it isn't, set the user environment variable `HERDR_HC` to the full path of `hc.exe`. It is written for Windows PowerShell 5.1 and PowerShell 7 (`pwsh` in place of `powershell` works too) and needs nothing else. Its test has run with PowerShell 7 only.

## Test

```sh
sh integrations/cursor/status-hook.test.sh
pwsh -NoProfile -File integrations/cursor/status-hook.test.ps1
```

On Windows: `powershell -NoProfile -ExecutionPolicy Bypass -File integrations\cursor\status-hook.test.ps1`.
