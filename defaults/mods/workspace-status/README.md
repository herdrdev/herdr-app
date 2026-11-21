# Session and subscription usage

- Click **Usage** in the Herdr status bar, then click **Enable** for each provider you want to track.
- Install Node.js 18 or later on the app’s `PATH`.
- The bar shows local subscriptions and the focused local session.
- Enabled providers refresh once a minute and when terminal focus changes. Missing metrics appear as unavailable.
- Replace `/absolute/path/to/workspace-status` with the absolute path to `mods/workspace-status` in your Herdr app home (normally `~/.config/herdr-app`).

## Codex

- Uses your signed-in `codex app-server` for subscription quotas.
- For focused session tracking, either start directly with `codex resume <UUID>` or set `SESSION_ID` and bind it inside its Herdr terminal.

```bash
node "/absolute/path/to/workspace-status/usage.cjs" bind codex "$SESSION_ID"
```

- Optional Codex `SessionStart` hook reads `session_id` from stdin.

```bash
node "/absolute/path/to/workspace-status/usage.cjs" bind codex
```

## Claude Code

- Reads the default account-matched local cache for quotas automatically.
- For live session and rate-limit updates, add `statusLine` to your Claude settings:

```json
{
  "statusLine": {
    "type": "command",
    "command": "node \"/absolute/path/to/workspace-status/claude-usage.cjs\" status-line"
  }
}
```

- To keep an existing status-line command, add `--passthrough` and pipe to it:

```json
{
  "statusLine": {
    "type": "command",
    "command": "node \"/absolute/path/to/workspace-status/claude-usage.cjs\" status-line --passthrough | existing-command"
  }
}
```

## Agy

- Add `statusLine` with `stack_with_default: true`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "node \"/absolute/path/to/workspace-status/agy-usage.cjs\"",
    "stack_with_default": true
  }
}
```

## Cursor

- Add `sessionStart` and `stop` hooks to `~/.cursor/hooks.json` (`version: 1`). Keep existing entries in both arrays.

```json
{
  "version": 1,
  "hooks": {
    "sessionStart": [
      { "command": "node \"/absolute/path/to/workspace-status/cursor-usage.cjs\"" }
    ],
    "stop": [
      { "command": "node \"/absolute/path/to/workspace-status/cursor-usage.cjs\"" }
    ]
  }
}
```

- **Billing**: Cursor performs no automatic login credential discovery.
  - **Preferred**: Save the JSON from signed-in `https://cursor.com/api/usage-summary` and set `HERDR_CURSOR_USAGE_FILE=/absolute/path/to/usage-summary.json`.
  - **Alternative**: In a trusted app environment, explicitly set `HERDR_CURSOR_USAGE_COOKIE="WorkosCursorSessionToken=..."`.
- Displays session `model` and `turns` when available.
