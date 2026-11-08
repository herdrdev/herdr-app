import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/agents/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Agents", "class Agents");

function harness(pins: string[], globals: Record<string, unknown> = {}) {
  const data = new Map([["pins", JSON.stringify(pins)]]);
  const writes: unknown[] = [];
  const storage = { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => data.set(k, v), removeItem: (k: string) => data.delete(k) };
  const Agents = new Function("View", "localStorage", "props", "run", ...Object.keys(globals), `${source}; return Agents;`)(
    class {}, storage, () => ({}), (...args: unknown[]) => writes.push(args),
    ...Object.values(globals),
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

test("an unnamed tab's row reads its terminal's title, not the tab number", () => {
  const title = new Function("View", "localStorage", "props", "run", `${source}; return title;`)(class {}, {}, () => ({}), () => {});
  const claude = { program: { name: "claude", key: "claude" }, title: "Herdr session names" };
  expect(title({ name: "3", named: false }, claude)).toBe("Herdr session names");
  expect(title({ name: "claude", named: true }, claude)).toBe("Herdr session names");
  expect(title({ name: "fix login", named: true }, claude)).toBe("fix login");
  expect(title({ name: "3", named: false }, { ...claude, title: "claude" })).toBe("3");
});

test("tab labels prefer the first agent, then focused terminal, then first terminal", () => {
  const leadPanes = new Function("View", `${source}; return leadPanes;`)(class {});
  const pane = (tab: string, terminal: string, agent = false, focused = false) => ({ tab, terminal, program: { agent }, focused });
  const all = [
    { tab: "agent", focused: true }, // A mod pane has no terminal.
    pane("agent", "shell", false, true), pane("agent", "first-agent", true), pane("agent", "second-agent", true, true),
    pane("focused", "first-shell"), pane("focused", "focused-shell", false, true),
    pane("first", "first"), pane("first", "second"),
  ];
  const leads = leadPanes(all);
  expect([...leads].map(([tab, p]: any) => [tab, p.terminal])).toEqual([
    ["agent", "first-agent"], ["focused", "focused-shell"], ["first", "first"],
  ]);
});

// Lightweight element descriptions: execute conditional builders and retain text,
// without running input callbacks or pretending to exercise native GPUI layout.
function element() {
  const node = { id: "", children: [] as any[] };
  const el: any = new Proxy(node, { get: (_, method) => {
    if (method === "node") return node;
    return (...args: any[]) => {
      if (method === "id") node.id = args[0];
      if (method === "on_click") (node as any).click = args[0];
      if (method === "child") node.children.push(args[0]?.node ?? args[0]);
      if (method === "children") node.children.push(...args[0].map((child: any) => child?.node ?? child));
      if (method === "when" && args[0]) args[1](el);
      return el;
    };
  } });
  return el;
}
function textOf(node: any): string {
  return typeof node === "string" ? node : (node?.children ?? []).map(textOf).join(" ");
}

for (const sort of ["priority", "grouped"]) test(`${sort} render reads panes once and refreshes labels on the next render`, () => {
  const list = Array.from({ length: 64 }, (_, i) => ({
    address: `t${i}`, pane: `p${i}`, tab: `tab${i}`, space: "s1", space_name: "Project",
    program: { name: "Claude", key: "claude", agent: true }, title: "Fallback", status: "idle", seq: i,
  }));
  const snapshot = list.map((a, i) => ({ tab: a.tab, terminal: a.address, program: a.program, title: `Task ${i}` }));
  let reads = 0;
  const orders: string[][] = [];
  const { agents } = harness([], {
    agents: () => list, terminals: () => list.map(a => ({ ...a, state: "live" })),
    panes: () => { reads++; return structuredClone(snapshot); }, focused: () => null,
    spaces: () => [{ tabs: list.map(a => ({ id: a.tab, name: "Claude", named: true })) }],
    layout: () => ({ id: "sidebar", size: { px: 268 } }), shared: () => sort,
    theme: () => ({}), setAgentOrder: (order: string[]) => orders.push(order),
    div: element, v_flex: element, h_flex: element, svg: element, image: element,
  });
  const first = textOf(agents.render({}).node);
  expect(reads).toBe(1);
  for (let i = 0; i < list.length; i++) expect(first).toContain(`Task ${i}`);
  expect(orders[0]).toHaveLength(list.length);
  snapshot[63].title = "Updated task";
  const second = textOf(agents.render({}).node);
  expect(reads).toBe(2);
  expect(second).toContain("Updated task");
  expect(second).not.toContain("Task 63");
  expect(orders[1]).toEqual(orders[0]);
});

for (const sort of ["priority", "grouped"]) for (const named of [false, true]) test(`${sort}: agents sharing a ${named ? "named" : "numbered"} tab keep their own titles`, () => {
  const program = { name: "Claude", key: "claude", agent: true };
  let list = ["Alpha task", "Beta task", "Solo task"].map((title, i) => ({
    address: `t${i}`, pane: `p${i}`, tab: i < 2 ? "shared" : "solo", space: "s1", space_name: "Project",
    program, title, status: "idle", seq: i,
  }));
  const { agents } = harness([], {
    agents: () => list, terminals: () => list.map(a => ({ ...a, state: "live" })),
    panes: () => list.map(a => ({ ...a, terminal: a.address })), focused: () => null,
    spaces: () => [{ tabs: [{ id: "shared", name: named ? "Named tab" : "1", named }, { id: "solo", name: "Solo tab", named: true }] }],
    layout: () => ({ id: "sidebar", size: { px: 268 } }), shared: () => sort,
    theme: () => ({}), setAgentOrder() {},
    div: element, v_flex: element, h_flex: element, svg: element, image: element,
  });
  const rows = () => {
    const result = new Map<string, string>();
    const walk = (node: any) => {
      if (node?.id?.startsWith("ag-t")) result.set(node.id, textOf(node));
      (node?.children ?? []).forEach(walk);
    };
    walk(agents.render({}).node);
    return result;
  };
  const shared = rows();
  expect(shared.get("ag-t0")).toContain("Alpha task");
  expect(shared.get("ag-t1")).toContain("Beta task");
  expect(shared.get("ag-t1")).not.toContain("Alpha task");
  expect(shared.get("ag-t0")).not.toContain("Named tab");
  expect(shared.get("ag-t2")).toContain("Solo tab");
  expect(shared.get("ag-t2")).not.toContain("Solo task");
  list = list.filter(a => a.address !== "t1");
  expect(rows().get("ag-t0")).toContain(named ? "Named tab" : "Alpha task");
});

// A kept sub-agent: started by t1 (`hc run`), then given a pane (`terminal.keep`).
function keptRender(sort: string, { width = 268, kept = [{ address: "t3", parent: "t1" }], paneless = [] as { address: string; parent: string }[], pinned = [] as string[] } = {}) {
  const program = { name: "Claude", key: "claude", agent: true };
  const row = (address: string, seq: number) => ({ address, pane: `p-${address}`, tab: "tab1", space: "s1", space_name: "Project", program, title: address, status: "working", seq, pinned: pinned.includes(address) });
  const list = [row("t1", 1), ...kept.map((k, i) => row(k.address, i + 2))];
  const live = (address: string, parent: string | null) => ({ address, parent, state: "live", status: "working", program, labels: {} });
  const orders: string[][] = [];
  const { agents, writes } = harness([], {
    agents: () => list,
    terminals: () => [live("t1", null), ...paneless.map(k => live(k.address, k.parent)), ...kept.map(k => live(k.address, k.parent))],
    panes: () => list.map(a => ({ tab: a.tab, terminal: a.address, program, title: a.title })), focused: () => null,
    spaces: () => [{ tabs: [{ id: "tab1", name: "Project", named: true }] }],
    layout: () => ({ id: "sidebar", size: { px: width } }), shared: () => sort,
    theme: () => ({}), setAgentOrder: (order: string[]) => orders.push(order),
    div: element, v_flex: element, h_flex: element, svg: element, image: element,
  });
  const ids: string[] = [];
  const clicks = new Map<string, () => void>();
  const texts = new Map<string, string>();
  const walk = (node: any) => {
    if (node?.id) { ids.push(node.id); texts.set(node.id, textOf(node)); if (node.click) clicks.set(node.id, node.click); }
    (node?.children ?? []).forEach(walk);
  };
  walk(agents.render({}).node);
  return { ids, clicks, writes, texts, order: orders[orders.length - 1] };
}

for (const sort of ["priority", "grouped"]) test(`${sort}: a kept sub-agent shows once, under its parent, and its row focuses its pane`, () => {
  const { ids, clicks, writes, texts, order } = keptRender(sort);
  expect(texts.get("ag-t1")).toContain("Project");
  expect(texts.get("ag-t1")).not.toContain("t1");
  expect(texts.get("sub-t3")).toContain("Claude");
  expect(texts.get("sub-t3")).not.toContain("Project");
  expect(ids).toContain("ag-t1");
  expect(ids).toContain("sub-t3");
  expect(ids).not.toContain("ag-t3");
  expect(order).toEqual(["t1"]);
  clicks.get("sub-t3")!();
  expect(writes).toContainEqual(["pane.focus", { terminal: "t3" }]);
});

for (const sort of ["priority", "grouped"]) test(`${sort}: a kept sub-agent under a pane-less one shows once in the tree`, () => {
  const { ids } = keptRender(sort, { paneless: [{ address: "t3", parent: "t1" }], kept: [{ address: "t4", parent: "t3" }] });
  expect(ids).toContain("sub-t3");
  expect(ids).toContain("sub-t4");
  expect(ids).not.toContain("ag-t4");
});

for (const sort of ["priority", "grouped"]) test(`${sort}: a kept sub-agent keeps its own row where no tree shows it`, () => {
  expect(keptRender(sort, { width: 44 }).ids).toContain("ag-t3"); // the rail draws no trees
  const pinned = keptRender(sort, { pinned: ["t3"] });
  expect(pinned.ids).toContain("ag-t3");
  expect(pinned.texts.get("ag-t1")).toContain("t1");
  expect(pinned.texts.get("ag-t3")).toContain("t3");
  expect(keptRender(sort, { kept: [{ address: "t3", parent: "t9" }] }).ids).toContain("ag-t3"); // its parent ended
});
