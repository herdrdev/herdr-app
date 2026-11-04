import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/agents/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Agents", "class Agents");

function harness(pins: string[], values: Record<string, unknown> = {}) {
  const data = new Map([["pins", JSON.stringify(pins)]]);
  const writes: unknown[] = [];
  const storage = { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => data.set(k, v), removeItem: (k: string) => data.delete(k) };
  const globals = { View: class {}, localStorage: storage, props: () => ({}), run: (...args: unknown[]) => writes.push(args), ...values };
  const Agents = new Function(...Object.keys(globals), `${source}; return Agents;`)(...Object.values(globals));
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

// Render the shipped view with snapshots of the existing herdr API. Keep each
// element's children and handlers so labels and click targets are tested together.
class Element {
  nodes: (Element | string)[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child") target.nodes.push(args[0]);
        else if (key === "children") target.nodes.push(...args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}
function find(root: Element | string, id: string): Element | undefined {
  if (typeof root === "string") return;
  if (root.calls.id?.[0] === id) return root;
  for (const node of root.nodes) {
    const found = find(node, id);
    if (found) return found;
  }
}
const text = (root: Element | string): string[] => typeof root === "string" ? [root] : root.nodes.flatMap(text);

function sidebar(sort = "priority") {
  const program = { key: "claude", name: "Claude Code", agent: true };
  const tab = { id: "tab1", name: "1", named: false };
  const tabs = [tab];
  const list = ["t1", "t2"].map((address, i) => ({
    address, pane: `p${i + 1}`, tab: tab.id, space: "s1", space_name: "repo", tab_name: "1", tab_index: 1,
    program, title: `Task ${i + 1}`, status: "idle", seq: i, focused: false, pinned: false,
  }));
  const snapshot = list.map(a => ({ pane: a.pane, terminal: a.address, tab: a.tab, name: null as string | null, title: a.title, program, focused: false }));
  const terminals: any[] = [];
  const orders: string[][] = [];
  const element = () => new Element();
  const { agents: view, writes } = harness([], {
    div: element, svg: element, image: element, h_flex: element, v_flex: element,
    agents: () => list, panes: () => snapshot, terminals: () => terminals,
    spaces: () => [{ id: "s1", tabs }], focused: () => null,
    theme: () => ({}), layout: () => null, shared: () => sort,
    setAgentOrder: (order: string[]) => orders.push(order),
  });
  const render = () => view.render({});
  const row = (tree: Element, address: string) => {
    const found = find(tree, `ag-${address}`);
    expect(found).toBeDefined();
    return found!;
  };
  return { tab, tabs, list, snapshot, terminals, orders, writes, render, row };
}

test("agents in one tab show their own pane names in priority and grouped views", () => {
  for (const sort of ["priority", "grouped"]) {
    const h = sidebar(sort);
    h.tab.name = "Shared tab";
    h.tab.named = true;
    h.snapshot[0].name = "API";
    h.snapshot[1].name = "UI";
    h.snapshot.reverse(); // Pane identity, not snapshot order or the tab's lead.
    const tree = h.render();
    for (const [address, name] of [["t1", "API"], ["t2", "UI"]]) {
      expect(text(h.row(tree, address))).toEqual(sort === "priority" ? ["repo", `· ${name}`, "<1m"] : [name, "<1m"]);
    }
  }
});

test("an explicitly numeric pane name is not hidden as an automatic tab number", () => {
  for (const sort of ["priority", "grouped"]) {
    const h = sidebar(sort);
    h.list.splice(1);
    h.snapshot[0].name = "42";
    const tree = h.render();
    expect(text(h.row(tree, "t1"))).toEqual(sort === "priority" ? ["repo", "· 42", "<1m"] : ["42", "<1m"]);
  }
});

test("Chinese and duplicate pane names preserve terminal aliases, navigation order and click targets", () => {
  for (const sort of ["priority", "grouped"]) {
    const h = sidebar(sort);
    h.terminals.push(...h.list.map(a => ({ address: a.address, name: `${a.address}-alias`, state: "live" })));
    for (const names of [["中文 · 接口 🧪", "🧭 前端"], ["同名", "同名"]]) {
      h.snapshot.forEach((p, i) => { p.name = names[i]; });
      const tree = h.render();
      for (const [i, a] of h.list.entries()) {
        const row = h.row(tree, a.address);
        expect(text(row)).toEqual(sort === "priority" ? ["repo", `· ${names[i]}`, "<1m"] : [names[i], "<1m"]);
        row.calls.on_click[0]();
      }
      expect(h.orders.at(-1)).toEqual(sort === "priority" ? ["t2", "t1"] : ["t1", "t2"]);
    }
    expect(h.writes).toEqual(Array(2).fill([
      ["pane.focus", { pane: "p1" }], ["pane.focus", { pane: "p2" }],
    ]).flat());
    expect(h.terminals.map(t => [t.address, t.name])).toEqual([["t1", "t1-alias"], ["t2", "t2-alias"]]);
  }
});

test("renaming and clearing a pane updates its row and restores the named tab fallback", () => {
  const h = sidebar();
  h.tab.name = "fix login";
  h.tab.named = true;
  h.snapshot[0].name = "接口";
  h.snapshot[1].name = "前端";
  expect(text(h.row(h.render(), "t1"))).toContain("· 接口");
  h.snapshot[0].name = "接口 v2";
  expect(text(h.row(h.render(), "t1"))).toContain("· 接口 v2");
  // pane.rename clears to null; an old snapshot may still contain an empty string.
  for (const cleared of [null, ""]) {
    h.snapshot[0].name = cleared;
    const tree = h.render();
    expect(text(h.row(tree, "t1"))).toEqual(["repo", "· fix login", "<1m"]);
    expect(text(h.row(tree, "t2"))).toEqual(["repo", "· 前端", "<1m"]);
  }
});

test("unnamed panes read their own live titles without stealing the first or focused pane's title", () => {
  const h = sidebar();
  for (const tab of [{ name: "1", named: false }, { name: "Claude Code", named: true }]) {
    Object.assign(h.tab, tab);
    h.snapshot[0].title = "✳ Fix login";
    h.snapshot[1].title = "✳ Write tests";
    for (const focused of [0, 1]) {
      h.snapshot.forEach((p, i) => { p.focused = i === focused; });
      const tree = h.render();
      expect(text(h.row(tree, "t1"))).toEqual(["repo", "· Fix login", "<1m"]);
      expect(text(h.row(tree, "t2"))).toEqual(["repo", "· Write tests", "<1m"]);
    }
  }
  h.snapshot[1].title = "✳ Tests passed";
  expect(text(h.row(h.render(), "t2"))).toContain("· Tests passed");
});

test("program, Windows path and shell titles keep the existing automatic tab-number fallback", () => {
  const h = sidebar();
  for (const title of ["Claude Code", "claude", "C:\\work\\repo", "user@host:repo"]) {
    h.snapshot.forEach(p => { p.title = title; });
    const tree = h.render();
    // With two otherwise identical rows the automatic tab number stays visible.
    for (const a of h.list) expect(text(h.row(tree, a.address))).toEqual(["repo", "· 1", "<1m"]);
  }
  h.list.splice(1);
  expect(text(h.row(h.render(), "t1"))).toEqual(["repo", "<1m"]);
});

test("missing pane entries retain the legacy lead fallback and missing tabs retain agent titles", () => {
  const h = sidebar();
  h.snapshot.splice(1);
  expect(text(h.row(h.render(), "t2"))).toContain("· Task 1");
  h.tabs.splice(0);
  h.snapshot[0].name = "命名仍有效";
  let tree = h.render();
  expect(text(h.row(tree, "t1"))).toContain("· 命名仍有效");
  expect(text(h.row(tree, "t2"))).toContain("· Task 2");
  h.list[1].title = "";
  tree = h.render();
  expect(text(h.row(tree, "t2"))).toContain("· Claude Code");
});

test("background sub-agents keep their own labels and peek targets under a named parent pane", () => {
  const h = sidebar();
  h.snapshot[0].name = "主代理";
  h.terminals.push({ address: "t3", parent: "t1", state: "live", name: "后台检索", status: "working" });
  const tree = h.render();
  expect(text(h.row(tree, "t1"))).toContain("· 主代理");
  const sub = find(tree, "sub-t3");
  expect(sub).toBeDefined();
  expect(text(sub!)).toEqual(["后台检索"]);
  sub!.calls.on_click[0]();
  h.row(tree, "t1").calls.on_click[0]();
  expect(h.writes).toEqual([["terminal.peek", { terminal: "t3" }], ["pane.focus", { pane: "p1" }]]);
  expect(h.orders.at(-1)?.sort()).toEqual(["t1", "t2"]);
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
