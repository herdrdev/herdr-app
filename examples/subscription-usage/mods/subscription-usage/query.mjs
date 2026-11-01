// Runs in Node, outside the UI VM. Only normalized quota data leaves this process.
// Never print provider errors, credentials, account identifiers, or raw responses.
import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { spawn, execFile } from "node:child_process";
import { pathToFileURL } from "node:url";
import { claudeQuota, codexQuota } from "./quota.js";

const failure = (message) => ({ status: "unavailable", message, windows: [] });
const success = (windows) => windows.length ? { status: "ok", windows, checkedAt: Date.now() } : failure("No subscription limits reported");

export async function readClaude({ env = process.env, platform = process.platform, read = readFile, request = fetch, keychain = execFile } = {}) {
  try {
    let raw;
    const root = env.CLAUDE_CONFIG_DIR || join(homedir(), ".claude");
    try { raw = await read(join(root, ".credentials.json"), "utf8"); } catch {
      // Custom keychain services are not guessed; the CLI may use another account.
      if (platform !== "darwin" || env.CLAUDE_CONFIG_DIR) return failure("Sign in with Claude Code (file credentials required)");
      raw = await new Promise((resolve, reject) => keychain("security", ["find-generic-password", "-s", "Claude Code-credentials", "-w"], { timeout: 10000, maxBuffer: 1048576 }, (err, stdout) => err ? reject(err) : resolve(stdout)));
    }
    const token = JSON.parse(raw)?.claudeAiOauth?.accessToken;
    if (typeof token !== "string" || !token) return failure("Claude subscription login required");
    const response = await request("https://api.anthropic.com/api/oauth/usage", {
      headers: { Authorization: `Bearer ${token}`, "anthropic-beta": "oauth-2025-04-20", "User-Agent": "herdr-subscription-usage" },
      signal: AbortSignal.timeout(12000), redirect: "error",
    });
    if (!response.ok) return failure(response.status === 401 || response.status === 403 ? "Claude login expired or usage access denied" : response.status === 429 ? "Claude usage temporarily rate limited" : "Claude usage service unavailable");
    return success(claudeQuota(await response.json()));
  } catch { return failure("Claude usage unavailable; check login and connection"); }
}

// No thread/turn is created: this requests quota only and never runs inference.
export function readCodex({ start = spawn, platform = process.platform, deadline = 18000 } = {}) {
  return new Promise(resolve => {
    let child, timer, finished = false, pending = "";
    const finish = result => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      child?.stdin?.end();
      // Only our short-lived app-server (and its Windows command wrapper).
      if (child?.pid && child.exitCode === null && child.signalCode === null) {
        if (platform === "win32") execFile("taskkill.exe", ["/pid", String(child.pid), "/t", "/f"], { windowsHide: true }, () => {});
        else child.kill();
      }
      resolve(result);
    };
    const send = value => child.stdin.write(`${JSON.stringify(value)}\n`);
    try {
      // npm installs a .cmd on Windows. Arguments are fixed, never user input.
      child = platform === "win32"
        ? start(process.env.ComSpec || "cmd.exe", ["/d", "/s", "/c", "codex app-server"], { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] })
        : start("codex", ["app-server"], { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
      timer = setTimeout(() => finish(failure("Codex usage request timed out")), deadline);
      child.on("error", () => finish(failure("Install Codex CLI and sign in")));
      child.on("exit", () => finish(failure("Codex usage unavailable; check CLI login")));
      child.stdin.on("error", () => finish(failure("Codex usage connection closed")));
      child.stderr.on("data", () => {});
      child.stdout.setEncoding("utf8");
      child.stdout.on("data", chunk => {
        if (finished) return;
        pending += chunk;
        if (pending.length > 1048576) return finish(failure("Codex usage response too large"));
        let newline;
        while ((newline = pending.indexOf("\n")) >= 0) {
          const line = pending.slice(0, newline); pending = pending.slice(newline + 1);
          let message;
          try { message = JSON.parse(line); } catch { continue; }
          if (message.id === 1) {
            if (message.error) return finish(failure("Codex CLI initialization failed"));
            send({ method: "initialized" });
            send({ id: 2, method: "account/rateLimits/read" });
          } else if (message.id === 2) {
            return finish(message.error ? failure("Codex subscription login or quota unavailable") : success(codexQuota(message.result)));
          }
        }
      });
      send({ id: 1, method: "initialize", params: { clientInfo: { name: "herdr_subscription_usage", title: "Herdr subscription usage", version: "1.0.0" } } });
    } catch { finish(failure("Install Codex CLI and sign in")); }
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [claude, codex] = await Promise.all([readClaude(), readCodex()]);
  process.stdout.write(JSON.stringify({ claude, codex }));
}
