import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/agents/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\r?\n/gm, "").replace("export default class Agents", "class Agents");

function harness(pins: string[], quota: unknown = {}) {
  const data = new Map([["pins", JSON.stringify(pins)]]);
  const writes: unknown[] = [];
  const storage = { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => data.set(k, v), removeItem: (k: string) => data.delete(k) };
  const Agents = new Function("View", "localStorage", "props", "run", "shared", `${source}; return Agents;`)(
    class {}, storage, () => ({}), (...args: unknown[]) => writes.push(args), () => quota,
  );
  const agents = new Agents();
  agents.init({}, { timer: { every() {} } });
  return { agents, data, writes };
}

test("legacy pins migrate once listed, retaining offline pins until their labels arrive", () => {
  const { agents, data, writes } = harness(["t1", "offline/t2"]);
  agents.migratePins([]);
  expect(JSON.parse(data.get("pins")!)).toEqual(["t1", "offline/t2"]);
  const local = { address: "t1", state: "live", labels: {} };
  agents.migratePins([local]);
  agents.migratePins([local]);
  expect(writes).toEqual([["terminal.pin", { terminal: "t1", pinned: true }]]);
  expect(JSON.parse(data.get("pins")!)).toEqual(["t1", "offline/t2"]);
  agents.migratePins([{ ...local, labels: { "herdr.pinned": "1" } }]);
  expect(JSON.parse(data.get("pins")!)).toEqual(["offline/t2"]);
  const remote = { address: "offline/t2", state: "live", labels: {} };
  agents.migratePins([remote]);
  expect(writes).toHaveLength(2);
  agents.migratePins([{ ...remote, labels: { "herdr.pinned": "1" } }]);
  expect(data.has("pins")).toBe(false);
});

test("quota badges label default accounts and omit remote, stale, failed or reset windows", () => {
  const now = 2000000000000;
  const result = { status: "ok", checkedAt: now, windows: [{ remaining: 47, resetsAt: now + 100000 }] };
  const local = { address: "t1", program: { key: "codex" } };
  expect(harness([], { codex: result }).agents.quota(local, now)).toBe("Default 47%");
  expect(harness([], { codex: result }).agents.quota({ ...local, address: "vps/t1" }, now)).toBeNull();
  for (const changed of [{ ...result, status: "unavailable" }, { ...result, checkedAt: now - 300001 }, { ...result, windows: [] }]) {
    expect(harness([], { codex: changed }).agents.quota(local, now)).toBeNull();
  }
  expect(harness([], { codex: result }).agents.quota(local, now + 100000)).toBeNull();
});

test("already shared pins need no write and pending writes survive restart", () => {
  expect(harness([]).data.has("pins")).toBe(false);
  const { agents, data, writes } = harness(["t1"]);
  agents.migratePins([{ address: "t1", state: "live", labels: { "herdr.pinned": "1" } }]);
  expect(writes).toHaveLength(0);
  expect(data.has("pins")).toBe(false);
  const retry = harness(["t2"]);
  retry.agents.migratePins([{ address: "t2", state: "live", labels: {} }]);
  expect(JSON.parse(retry.data.get("pins")!)).toEqual(["t2"]);
  const restarted = harness(JSON.parse(retry.data.get("pins")!));
  restarted.agents.migratePins([{ address: "t2", state: "live", labels: {} }]);
  expect(restarted.writes).toHaveLength(1);
});
