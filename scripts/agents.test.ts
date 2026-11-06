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

// Lightweight element descriptions: retain text, SVG source and animation calls,
// without running input callbacks or pretending to exercise native GPUI layout.
function element(src?: string) {
  const node = { id: "", src, spin: undefined as number | undefined, children: [] as any[] };
  const el: any = new Proxy(node, { get: (_, method) => {
    if (method === "node") return node;
    return (...args: any[]) => {
      if (method === "id") node.id = args[0];
      if (method === "spin") node.spin = args[0];
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
  const walk = (node: any) => {
    if (node?.id) { ids.push(node.id); if (node.click) clicks.set(node.id, node.click); }
    (node?.children ?? []).forEach(walk);
  };
  walk(agents.render({}).node);
  return { ids, clicks, writes, order: orders[orders.length - 1] };
}

for (const sort of ["priority", "grouped"]) test(`${sort}: a kept sub-agent shows once, under its parent, and its row focuses its pane`, () => {
  const { ids, clicks, writes, order } = keptRender(sort);
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
  expect(keptRender(sort, { pinned: ["t3"] }).ids).toContain("ag-t3");
  expect(keptRender(sort, { kept: [{ address: "t3", parent: "t9" }] }).ids).toContain("ag-t3"); // its parent ended
});

const summaryAgent = (address = "t1", status = "idle") => ({
  address, status, pane: `p-${address}`, tab: "tab1", space: "s1", space_name: "Project",
  program: { name: "Generic worker", key: "generic-worker", agent: true },
  seq: 1, focused: false, pinned: false,
});
const summaryTerminal = (address = "t1", records?: Record<string, unknown>) => ({
  address, records, state: "live", status: "idle", report: { state: "idle" }, parent: null as string | null,
});
const workingRecords = () => ({ task: { state: "working", app: "generic-worker" } });

function summaryRender(list = [summaryAgent()], all = [summaryTerminal("t1", workingRecords())], { sort = "priority", width = 268 } = {}) {
  const orders: string[][] = [];
  const { agents, writes } = harness([], {
    agents: () => list, terminals: () => all, panes: () => [], focused: () => null,
    spaces: () => [{ tabs: [{ id: "tab1", name: "Task", named: true }] }],
    layout: () => ({ id: "sidebar", size: { px: width } }), shared: () => sort,
    theme: () => ({}), setAgentOrder: (order: string[]) => orders.push(order),
    div: element, v_flex: element, h_flex: element, svg: element, image: element,
  });
  return { view: agents, writes, orders, render: () => agents.render({}).node };
}
function nodes(node: any): any[] {
  return [node, ...(node?.children ?? []).flatMap(nodes)];
}
const byId = (tree: any, id: string) => nodes(tree).find(node => node?.id === id);
const spinners = (tree: any) => nodes(tree).filter(node => node?.src === "mods/workspace-chrome/spinner.svg" && node.spin === 900);

for (const sort of ["priority", "grouped"]) test(`${sort}: settled roots with working records show a spinning row in Working or their space`, () => {
  const list = [summaryAgent("t1", "idle"), summaryAgent("t2", "done")];
  const all = [summaryTerminal("t1", workingRecords()), summaryTerminal("t2", {
    ...workingRecords(), second: { state: "working" }, waiting: { state: "blocked" }, failed: { state: "error" },
  })];
  const { render, view, writes } = summaryRender(list, all, { sort });
  const tree = render();
  for (const a of list) {
    expect(spinners(byId(tree, `ag-${a.address}`))).toHaveLength(1);
    expect(view.listed.get(a.address)).toBe("working");
  }
  const header = byId(tree, sort === "priority" ? "sec-2" : "sp-s1");
  expect(textOf(header)).toContain(sort === "priority" ? "Working" : "Project");
  expect(byId(tree, "sec-3")).toBeUndefined();
  expect(byId(tree, "sec-1")).toBeUndefined();
  expect(nodes(tree).filter(node => node?.id?.startsWith("sub-"))).toHaveLength(0);
  expect(writes).toEqual([]);
});

test("working and blocked roots keep precedence, including a raw root error", () => {
  for (const raw of ["working", "blocked", "error"]) {
    const status = raw === "error" ? "blocked" : raw;
    const terminal = { ...summaryTerminal("t1", workingRecords()), status: raw, report: { state: raw } };
    const { render, view } = summaryRender([summaryAgent("t1", status)], [terminal]);
    const tree = render();
    expect(view.listed.get("t1")).toBe(status);
    expect(textOf(byId(tree, raw === "working" ? "sec-2" : "sec-4"))).toContain(raw === "working" ? "Working" : "Needs you");
    expect(spinners(byId(tree, "ag-t1"))).toHaveLength(raw === "working" ? 1 : 0);
  }
});

for (const status of ["idle", "done"]) test(`${status}: only a nonempty record ID with direct working state promotes`, () => {
  const ignored = [undefined, {}, { "": { state: "working" } }, { task: { status: { state: "working" } } },
    ...["idle", "done", "blocked", "error"].map(state => ({ task: { state } }))];
  for (const records of ignored) {
    const { render, view } = summaryRender([summaryAgent("t1", status)], [summaryTerminal("t1", records)]);
    const tree = render();
    expect(view.listed.get("t1")).toBe(status);
    expect(byId(tree, status === "done" ? "sec-3" : "sec-1")).toBeDefined();
    expect(spinners(tree)).toHaveLength(0);
  }
});

for (const status of ["idle", "done"]) test(`${status}: clearing or settling the last working record restores the root on the same view`, () => {
  const terminal = summaryTerminal("t1", workingRecords());
  const { render, view } = summaryRender([summaryAgent("t1", status)], [terminal]);
  const settled = [undefined, {}, ...["done", "idle", "error", "blocked"].map(state => ({ task: { state } }))];
  for (const records of settled) {
    terminal.records = workingRecords();
    expect(spinners(byId(render(), "ag-t1"))).toHaveLength(1);
    expect(view.changed.t1.s).toBe("working");
    terminal.records = records;
    const tree = render();
    expect(spinners(tree)).toHaveLength(0);
    expect(byId(tree, status === "done" ? "sec-3" : "sec-1")).toBeDefined();
    expect(view.listed.get("t1")).toBe(status);
    expect(view.changed.t1.s).toBe("idle");
  }
});

test("focused Done hold yields to working records and resumes normal Done/Idle handling after clear", () => {
  const agent = { ...summaryAgent("t1", "done"), focused: true };
  const terminal = summaryTerminal();
  const { render } = summaryRender([agent], [terminal]);
  expect(byId(render(), "sec-3")).toBeDefined();
  agent.status = "idle";
  expect(byId(render(), "sec-3")).toBeDefined(); // Seen Done stays until left.
  terminal.records = workingRecords();
  expect(byId(render(), "sec-2")).toBeDefined();
  terminal.records = {};
  expect(byId(render(), "sec-1")).toBeDefined(); // Not a stale Done hold or spinner.
  agent.status = "done";
  expect(byId(render(), "sec-3")).toBeDefined();
  agent.status = "idle";
  expect(byId(render(), "sec-3")).toBeDefined();
  agent.focused = false;
  expect(byId(render(), "sec-1")).toBeDefined();
});

test("records join by exact local/remote address without mutating snapshots or issuing commands", () => {
  const list = ["t1", "host-a/t1", "host-b/t1", "host-c/t1"].map(address => summaryAgent(address));
  const all = [summaryTerminal("t1"), summaryTerminal("host-a/t1"), summaryTerminal("host-b/t1")];
  const { render, view, writes } = summaryRender(list, all);
  for (const owner of all) {
    for (const terminal of all) terminal.records = terminal === owner ? workingRecords() : {};
    const before = structuredClone({ list, all });
    const tree = render();
    for (const agent of list) {
      expect(view.listed.get(agent.address)).toBe(agent.address === owner.address ? "working" : "idle");
      expect(spinners(byId(tree, `ag-${agent.address}`))).toHaveLength(agent.address === owner.address ? 1 : 0);
    }
    expect({ list, all }).toEqual(before);
    expect(writes).toEqual([]);
  }
});

for (const sort of ["priority", "grouped"]) test(`${sort}: narrow rail and pinned rows keep the summary and navigation order`, () => {
  const list = [summaryAgent("idle"), summaryAgent("worker", "done"), summaryAgent("done", "done"),
    summaryAgent("blocked", "blocked"), { ...summaryAgent("pinned"), pinned: true }];
  const all = list.map(a => summaryTerminal(a.address, ["worker", "pinned"].includes(a.address) ? workingRecords() : {}));
  for (const width of [268, 44]) {
    const { render, orders } = summaryRender(list, all, { sort, width });
    const tree = render();
    expect(orders.at(-1)).toEqual(sort === "priority"
      ? ["pinned", "blocked", "done", "worker", "idle"] : ["pinned", "idle", "worker", "done", "blocked"]);
    expect(spinners(byId(tree, "ag-worker"))).toHaveLength(1);
    expect(spinners(byId(tree, "ag-pinned"))).toHaveLength(1);
    expect(Boolean(byId(tree, "pinned"))).toBe(width === 268);
  }
});

test("folded space header reflects working records and restores idle after clear", () => {
  const terminal = summaryTerminal("t1", workingRecords());
  const { render, view } = summaryRender([summaryAgent()], [terminal], { sort: "grouped" });
  view.folded.add("sp-s1");
  const tree = render();
  expect(byId(tree, "ag-t1")).toBeUndefined();
  expect(textOf(byId(tree, "sp-s1"))).toContain("Project 1");
  expect(spinners(byId(tree, "sp-s1"))).toHaveLength(1);
  terminal.records = {};
  expect(spinners(byId(render(), "sp-s1"))).toHaveLength(0);
});

test("separate blocked child terminals still put a record-working parent in Needs you", () => {
  const all = [summaryTerminal("t1", workingRecords()), { ...summaryTerminal("child"), parent: "t1", status: "blocked" }];
  const { render, orders } = summaryRender([summaryAgent("t1", "done")], all);
  const tree = render();
  expect(textOf(byId(tree, "sec-4"))).toContain("Needs you");
  expect(spinners(byId(tree, "ag-t1"))).toHaveLength(1);
  expect(textOf(byId(tree, "sub-child"))).toContain("!");
  expect(orders.at(-1)).toEqual(["t1"]);
});
