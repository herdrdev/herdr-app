# Subscription usage

Open **Agents → Usage** for remaining subscription quota and reset times.
Requires Node.js 18+ on PATH and an existing Claude Code and/or Codex login.
It does not create conversations, run inference, or change login settings.

Limits belong to the CLI's local default account, shared by all sessions using
that account. The panel does not inspect terminal environments: a terminal with
an account/config override may use another quota. Remote agents and other
providers are deliberately not assigned these values. Numeric message/token
allowances are not invented when a provider only reports a percentage.

- Claude: reads `CLAUDE_CONFIG_DIR/.credentials.json` (default `~/.claude`) or
  the default macOS `Claude Code-credentials` keychain entry. Custom macOS
  keychain services are unsupported. Only the access token is used for
  `GET https://api.anthropic.com/api/oauth/usage`; no token refresh is attempted.
  This endpoint is used by Claude Code but is not a stable public API.
- Codex: starts a short-lived `codex app-server`, initializes it, requests
  [`account/rateLimits/read`](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt),
  then terminates only that helper. CLI-managed authentication stays in the CLI.
  Multiple quota buckets are shown separately when reported.

The UI receives normalized quota data only, cached in Herdr's `shared.json` so
agent badges can read it. It never stores or logs tokens, account identifiers,
raw service responses or raw errors. Requests
have deadlines. Failed checks clear old values; missing/malformed windows are
unknown, not 100% remaining. A reset crossing does not refill the bar locally:
the next successful request must confirm it. Checks run every two minutes while
the panel is open, with a shared 30-second manual refresh cooldown.
