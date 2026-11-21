// Invoked with Node outside the mod's render process.
const { execFileSync } = require("node:child_process");

const providers = new Set(["codex", "cursor", "agy", "claude"]);
const UUID = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;

async function main([action, provider, terminal, sessionId]) {
  if (!providers.has(provider)) throw Error("Unknown usage provider");
  if (action === "bind") {
    const address = process.env.HC_TERMINAL_ID;
    if (!terminal && !address) return; // A global hook also runs outside Herdr.
    const id = terminal || (await require("./usage-hook.cjs").readHookInput())?.session_id;
    if (!address || typeof id !== "string" || !UUID.test(id)) {
      throw Error("Run bind PROVIDER SESSION_ID inside its Herdr terminal");
    }
    execFileSync("hc", ["label", address, "workspace.usage=" + provider + "/" + id]);
    return;
  }
  if (action !== "read") throw Error("Expected read or bind");
  const { collect } = require("./" + provider + "-usage.cjs");
  const appHome = require("node:path").resolve(__dirname, "../..");
  const result = await collect({ appHome, terminal: terminal || null, sessionId: sessionId || null });
  process.stdout.write(JSON.stringify(result));
}

main(process.argv.slice(2)).catch(() => {
  process.stderr.write("Usage data unavailable\n");
  process.exitCode = 1;
});
