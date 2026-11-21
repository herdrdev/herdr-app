const fs = require("node:fs/promises");
const { createHash, randomUUID } = require("node:crypto");
const { homedir } = require("node:os");
const path = require("node:path");

function snapshotPath(provider, terminal, appHome) {
  const root = appHome || process.env.HERDR_APP_HOME ||
    path.join(process.env.XDG_CONFIG_HOME || path.join(homedir(), ".config"), "herdr-app");
  const key = terminal == null ? "quota" : createHash("sha256").update(terminal).digest("hex");
  return path.join(root, "usage", provider, key + ".json");
}

async function readSnapshot(provider, terminal, appHome) {
  try {
    return JSON.parse(await fs.readFile(snapshotPath(provider, terminal, appHome), "utf8"));
  } catch {
    return null;
  }
}

async function writeSnapshot(provider, terminal, data, appHome) {
  const file = snapshotPath(provider, terminal, appHome);
  await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  const tmp = file + "." + randomUUID();
  try {
    await fs.writeFile(tmp, JSON.stringify(data), { flag: "wx", mode: 0o600 });
    await fs.rename(tmp, file);
  } finally {
    await fs.rm(tmp, { force: true });
  }
}

async function readHookInput(passthrough = false) {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    if (passthrough) process.stdout.write(chunk);
    bytes += chunk.length;
    if (bytes <= 2 ** 20) chunks.push(chunk);
  }
  return bytes <= 2 ** 20 ? JSON.parse(Buffer.concat(chunks).toString("utf8")) : null;
}

// Cache only derived counters. Large, unchanged transcripts are common in a long-running tab.
async function summarizeLog(provider, filename, id, appHome, summarize) {
  const file = await fs.open(filename);
  try {
    const stat = await file.stat();
    if (!stat.isFile() || !stat.size) return null;
    const key = "summary/" + id;
    const signature = JSON.stringify([filename, stat.ino, stat.size, stat.mtimeMs, stat.ctimeMs]);
    const cached = await readSnapshot(provider, key, appHome);
    if (cached?.signature === signature) return cached.summary;
    const stream = file.createReadStream({ autoClose: false, end: stat.size - 1 });
    const lines = require("node:readline").createInterface({ input: stream, crlfDelay: Infinity });
    try {
      const summary = await summarize(lines);
      await writeSnapshot(provider, key, { signature, summary }, appHome);
      return summary;
    } finally {
      lines.close();
      stream.destroy();
    }
  } finally {
    await file.close();
  }
}

module.exports = { readSnapshot, writeSnapshot, readHookInput, summarizeLog };
