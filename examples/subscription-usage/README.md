# Subscription usage — a personal Herdr mod

View Claude Code and Codex subscription quota, remaining percentages and reset
times in one dialog. Local agent sessions appear below their provider; clicking
one focuses it. The quota belongs to the **local default CLI account**, shared
by sessions using that account, not to each agent separately. Session account
matching is not verified.

![Subscription usage with synthetic demonstration data](preview.png)

This folder is an independently installable mod, outside `defaults/` and the
application bundle. It uses existing APIs in Herdr app 0.1.11; no app/core rebuild
or unmerged PR (including #247's proposed mod buttons) is required. It does not
modify the shipped Agents mod, personal layouts or keybindings.

## Install and open

Requires Node.js 18+ on Herdr's PATH and an existing Claude Code and/or Codex CLI
subscription login. API-key-only accounts may not expose subscription limits.
Review [RECIPE.md](RECIPE.md) and the files before installing: the mod runs Node,
reads Claude's local OAuth credentials (or the default macOS keychain entry),
requests Anthropic usage over HTTPS, and runs a temporary Codex app-server.

From a downloaded or checked-out copy of **this folder**:

```sh
node install.mjs
herdr-app check "$HOME/.config/herdr-app/mods/subscription-usage"
herdr-app do mod.open --name subscription-usage
```

These examples use the default app home. The installer prints the actual
absolute path to pass to `herdr-app check` when a different home is configured.
On Windows, if `herdr-app` is not on PATH, use PowerShell:

```powershell
& "$env:LOCALAPPDATA\Programs\herdr-app\herdr-app.exe" check "$HOME\.config\herdr-app\mods\subscription-usage"
& "$env:LOCALAPPDATA\Programs\herdr-app\herdr-app.exe" do mod.open --name subscription-usage
```

It installs only `mods/subscription-usage/` in `HERDR_APP_HOME`, or
`$XDG_CONFIG_HOME/herdr-app`, or `~/.config/herdr-app` (also on Windows). For a
different app home, use `node install.mjs --home "path/to/app home"` and set
`HERDR_APP_HOME` to that same path when running the check/open commands. Herdr
discovers the folder while running; no restart is needed. If Herdr is closed,
open it normally first. The installer does not start, stop or restart it.

For a name collision, install alongside the existing copy:
`node install.mjs --name subscription-usage-new`. Open/check using the new name.
Existing files are never overwritten; the helper path is adjusted for a renamed
copy. To update, install under a new name, check it, switch your shortcut, then
remove the previous mod folder after preserving any personal edits.

For keyboard access, optionally merge the single entry from [keys.json](keys.json)
into the app home's `keys.json`: **Ctrl+Shift+F8** toggles the dialog (some keyboards
also need Fn). Keep all existing sections and bindings. Choose another free key
if this combination is already used; the installer never changes keys. If you
renamed the mod, change the command to `mod.open:<your-name>` too. No menu button
is added by this standalone package; the command and optional shortcut work now.

To uninstall, close the dialog and remove only the installed mod folder and any
shortcut you added. Normalized cached values may remain under the three
`subscription-usage.*` keys in `shared.json`; they contain no credentials. Do not
delete the whole shared file or edit it while Herdr is running.

## What the numbers mean

- Claude: reads `CLAUDE_CONFIG_DIR/.credentials.json` (default `~/.claude`) or
  the default macOS `Claude Code-credentials` keychain entry. Custom macOS
  keychain services are unsupported. Only the access token is used for
  `GET https://api.anthropic.com/api/oauth/usage`; no token refresh is attempted.
  This endpoint is used by Claude Code but is not a stable public API.
- Codex: starts a short-lived `codex app-server`, initializes it, requests
  [`account/rateLimits/read`](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt),
  then terminates only that helper. CLI-managed authentication stays in the CLI.
  Multiple quota buckets are shown separately when reported. No conversation,
  turn or inference request is created.
- Both inherit Herdr's process environment. Terminal-specific config/account
  overrides and remote agents cannot be matched to these values. Other providers
  and numeric token/message allowances are not inferred from percentages.

The UI receives only normalized quota data, cached in Herdr's `shared.json`.
It never stores or logs tokens, account identifiers, raw responses or raw errors.
Requests have deadlines. Failed checks clear old values; missing/malformed
windows are unavailable, not 100% remaining. Data older than five minutes or past
its reset time is labeled stale; the bar does not refill until a successful
request confirms it. Refresh runs every two minutes while the dialog is open,
with a shared 30-second manual cooldown. Renamed copies share this cache/cooldown.
If a reload or closing the dialog cancels a pending check, its lock expires after
35 seconds; an open dialog retries within the next five-second UI tick. The
standalone `herdr-app check` command does not query providers or set a busy lock.

If a provider is unavailable, check the corresponding CLI login and connection;
if Node is unavailable, ensure Herdr can resolve it on PATH. A macOS keychain
access prompt can require your approval. A denied prompt, expired login, 429 or
unsupported response remains unavailable instead of showing a fabricated quota.

## Verify

```sh
node --test install.test.mjs quota.test.mjs view.test.mjs
```

Tests use synthetic credentials and responses: no real accounts, keychain access
or network requests. They cover parsing, failures/timeouts, the read-only Codex
RPC sequence, stale rendering, session focus, cooldown/late results, and safe
installation including existing-file preservation and paths with spaces.
`preview.png` is a native Windows Herdr 0.1.11 screenshot with synthetic data.
