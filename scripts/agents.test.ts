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

test("Pi status records appear beneath their owning terminal without inventing terminals", () => {
  const { agents, writes } = harness([], { h_flex: element, div: element, svg: element, image: element });
  const owner = { address: "remote/t1", state: "live", program: { name: "Pi" }, records: {
    "subagents/active": { app: "pi-subagents", state: "working", title: "worker", msg: "Reading" },
    "subagents/done": { app: "pi-subagents", state: "done", title: "reviewer" },
    "subagents/error": { app: "pi-subagents", state: "error", title: "failed" },
    "other": { app: "pi", state: "working" },
    "subagents/foreign": { app: "other", state: "working" },
    "subagents/invalid": { app: "pi-subagents", state: "unexpected" },
    "subagents/null": null,
  } };
  const child = { address: "remote/t2", parent: owner.address, state: "live" };
  const tree = agents.tree([owner, child], owner.address);
  expect(tree.map((s: any) => s.t.address)).toEqual([
    "remote/t2", "remote/t1#subagents/active", "remote/t1#subagents/done", "remote/t1#subagents/error",
  ]);
  expect(tree[1].t.message).toBe("Reading");
  expect(tree[3].lines).toEqual(["└"]);
  const kidStatus = new Function("View", `${source}; return kidStatus;`)(class {});
  expect(kidStatus(tree[2].t, new Map())).toBe("idle");
  expect(kidStatus(tree[3].t, new Map())).toBe("blocked");
  expect(agents.tree([{ ...owner, records: {} }], owner.address)).toEqual([]);
  agents.listed = new Map([[owner.address, "idle"]]);
  const row = agents.subRow({ raised: "selected" }, tree[1], owner.address, false).node;
  expect(textOf(row)).toContain("worker · Reading");
  expect(row.bg).toBeUndefined(); // Focusing the parent doesn't select every worker.
  row.click();
  expect(writes).toEqual([["pane.focus", { terminal: "remote/t1" }]]);
});

for (const sort of ["priority", "grouped"]) test(`${sort}: a headless Pi worker renders under its idle parent`, () => {
  const parent = { address: "t1", pane: "p1", tab: "tab1", space: "s1", space_name: "Project",
    program: { name: "Pi", key: "pi", agent: true }, status: "idle", seq: 1 };
  let records: Record<string, unknown> = { "subagents/test": { app: "pi-subagents", state: "working", title: "worker", msg: "Inspecting" } };
  const { agents } = harness([], {
    agents: () => [parent], terminals: () => [{ ...parent, state: "live", records }],
    panes: () => [{ ...parent, terminal: parent.address }], focused: () => null,
    spaces: () => [{ id: "s1", tabs: [{ id: "tab1", name: "Pi", named: true }] }],
    layout: () => ({ id: "sidebar", size: { px: 268 } }), shared: () => sort,
    theme: () => ({}), setAgentOrder() {},
    div: element, v_flex: element, h_flex: element, svg: element, image: element,
  });
  expect(textOf(agents.render({}).node)).toContain("worker · Inspecting");
  if (sort === "priority") expect(textOf(agents.render({}).node)).toContain("Working");
  records = { "subagents/test": { app: "pi-subagents", state: "error", title: "worker", msg: "Failed" } };
  expect(textOf(agents.render({}).node)).toContain("worker · Failed");
  expect(textOf(agents.render({}).node)).toContain("Needs you");
  records = { "subagents/test": { app: "pi-subagents", state: "done", title: "worker", msg: "Finished" } };
  expect(textOf(agents.render({}).node)).toContain("worker · Finished");
  records = {};
  expect(textOf(agents.render({}).node)).not.toContain("worker");
});

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

test("an empty OSC title published as the terminal address keeps the tab name", () => {
  const title = new Function("View", `${source}; return title;`)(class {});
  for (const address of ["t2", "remote/t2"]) {
    for (const identity of [{ terminal: address }, { address }]) {
      const lead = { ...identity, program: { name: "Claude Code", key: "claude" }, title: address };
      expect(title({ name: "2", named: false }, lead)).toBe("2");
      expect(title({ name: "Claude Code", named: true }, lead)).toBe("Claude Code");
      expect(title({ name: "fix login", named: true }, lead)).toBe("fix login");
      expect(title({ name: "2", named: false }, { ...lead, title: "renamed-task" })).toBe("renamed-task");
    }
  }
});

test("a title that is a folder never replaces the tab's name", () => {
  const title = new Function("View", "localStorage", "props", "run", `${source}; return title;`)(class {}, {}, () => ({}), () => {});
  const claude = { program: { name: "claude", key: "claude" }, title: "Herdr session names" };
  expect(title({ name: "3", named: false }, { ...claude, title: "~/src/api" })).toBe("3");
  expect(title({ name: "3", named: false }, { ...claude, title: "/tmp" })).toBe("3");
  expect(title({ name: "3", named: false }, { ...claude, title: "\u2733 ~/src/api" })).toBe("3");
  expect(title({ name: "3", named: false }, { ...claude, title: "C:\\src\\api" })).toBe("3");
  expect(title({ name: "3", named: false }, { ...claude, title: "\u2733 Herdr session names" })).toBe("Herdr session names");
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
      if (method === "bg") (node as any).bg = args[0];
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

test("a failed agent check keeps cached launchable agents and shows an error instead of missing agents", async () => {
  const menuSource = (await readFile(new URL("../defaults/mods/new-menu/main.js", import.meta.url), "utf8"))
    .replace(/^import .*;\n/gm, "").replace("export default class NewMenu", "class NewMenu");
  const cached = [
    { key: "claude", name: "Claude Code", run: ["claude"], installed: true },
    { key: "codex", name: "Codex", run: ["codex"], installed: false },
  ];
  for (const agents of [[], cached]) {
    const writes: unknown[] = [];
    const globals = {
      View: class {}, spaces: () => [], shared: () => ({ "": agents }), setShared: (...args: unknown[]) => writes.push(args),
      knownAgents: async () => { throw Error("shell failed"); },
      theme: () => ({}), props: () => ({}), keys: () => [], window: () => ({ platform: "macos" }), panes: () => [],
      div: element, svg: element, image: element, h_flex: element, v_flex: element,
    };
    const NewMenu = new Function(...Object.keys(globals), `${menuSource}; return NewMenu;`)(...Object.values(globals));
    const menu = new NewMenu();
    let task: Promise<void>;
    menu.init({}, { focus_handle: () => ({ focus() {} }), spawn: (run: (cx: unknown) => Promise<void>) => { task = run({ notify() {} }); } });
    await task!;
    const text = textOf(menu.render().node);
    expect(text).toContain("Could not check installed agents");
    expect(text).not.toContain("not installed");
    expect(text).not.toContain("Codex");
    if (agents.length) expect(text).toContain("Claude Code");
    expect(writes).toEqual([]);
  }
});

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

test("grandchild records nest under their owning terminal, not under other records", () => {
  const { agents } = harness([]);
  const record = { app: "pi-subagents", state: "working", title: "worker" };
  const root = { address: "remote/t1", state: "live", records: { "subagents/root": record } };
  const child = { address: "remote/t2", parent: root.address, state: "live", records: {
    "subagents/child": record, "subagents/grandchild": record,
  } as Record<string, typeof record> };
  const grandchild = { address: "remote/t3", parent: child.address, state: "live" };
  expect(agents.tree([root, child, grandchild], root.address).map((s: any) => [s.t.address, s.lines])).toEqual([
    ["remote/t2", ["├"]],
    ["remote/t3", ["│", "├"]],
    ["remote/t2#subagents/child", ["│", "├"]],
    ["remote/t2#subagents/grandchild", ["│", "└"]],
    ["remote/t1#subagents/root", ["└"]],
  ]);
  child.records = {};
  expect(agents.tree([root, child, grandchild], root.address).map((s: any) => s.t.address)).toEqual([
    "remote/t2", "remote/t3", "remote/t1#subagents/root",
  ]);
});

// A kept sub-agent: started by t1 (`hc run`), then given a pane (`terminal.keep`).
function keptRender(sort: string, {
  width = 268,
  kept = [{ address: "t3", parent: "t1" }],
  paneless = [] as { address: string; parent: string }[],
  pinned = [] as string[],
  records = {} as Record<string, Record<string, unknown>>,
  status = "working",
} = {}) {
  const program = { name: "Claude", key: "claude", agent: true };
  const row = (address: string, seq: number) => ({ address, pane: `p-${address}`, tab: "tab1", space: "s1", space_name: "Project", program, title: `Task ${address}`, status, seq, pinned: pinned.includes(address) });
  const list = [row("t1", 1), ...kept.map((k, i) => row(k.address, i + 2))];
  const live = (address: string, parent: string | null) => ({ address, parent, state: "live", status, program, labels: {}, records: records[address] });
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

for (const sort of ["priority", "grouped"]) for (const state of ["working", "error"]) for (const keptOwner of [false, true]) test(`${sort}: a ${state} grandchild opens its ${keptOwner ? "kept" : "pane-less"} owning terminal`, () => {
  const { ids, texts, clicks, writes, order } = keptRender(sort, {
    status: "idle",
    paneless: keptOwner ? [] : [{ address: "t3", parent: "t1" }],
    kept: [...(keptOwner ? [{ address: "t3", parent: "t1" }] : []), { address: "t4", parent: "t3" }],
    records: { t3: { "subagents/test": { app: "pi-subagents", state, title: "grandchild", msg: "Checking" } } },
  });
  expect(ids.filter(id => id === "sub-t3#subagents/test")).toHaveLength(1);
  expect(ids.indexOf("sub-t3")).toBeLessThan(ids.indexOf("sub-t3#subagents/test"));
  expect(ids).toContain("sub-t4"); // Terminal-backed grandchild still appears.
  expect(ids).not.toContain("ag-t4");
  expect(texts.get("sub-t3#subagents/test")).toContain("grandchild · Checking");
  expect(order).toEqual(["t1"]);
  if (sort === "priority") expect(texts.get("ag-list")).toContain(state === "error" ? "Needs you" : "Working");
  clicks.get("sub-t3#subagents/test")!();
  expect(writes).toEqual([[keptOwner ? "pane.focus" : "terminal.peek", { terminal: "t3" }]]);
  clicks.get("sub-t4")!();
  expect(writes.at(-1)).toEqual(["pane.focus", { terminal: "t4" }]);
});

for (const sort of ["priority", "grouped"]) test(`${sort}: a kept sub-agent keeps its own row where no tree shows it`, () => {
  expect(keptRender(sort, { width: 44 }).ids).toContain("ag-t3"); // the rail draws no trees
  const pinned = keptRender(sort, { pinned: ["t3"] });
  expect(pinned.ids).toContain("ag-t3");
  expect(pinned.texts.get("ag-t1")).toContain("Task t1");
  expect(pinned.texts.get("ag-t3")).toContain("Task t3");
  expect(keptRender(sort, { kept: [{ address: "t3", parent: "t9" }] }).ids).toContain("ag-t3"); // its parent ended
});

for (const width of [268, 44]) test(`the focused agent's row is marked as selected ${width < 160 ? "in the rail" : "in the wide list"}`, () => {
  const program = { name: "Claude", key: "claude", agent: true };
  const list = ["t1", "t2"].map((address, i) => ({
    address, pane: `p-${address}`, tab: address, space: "s1", space_name: "Project",
    program, title: address, status: "idle", seq: i, focused: address === "t2",
  }));
  const { agents } = harness([], {
    agents: () => list, terminals: () => list.map(a => ({ ...a, state: "live", labels: {} })),
    panes: () => list.map(a => ({ ...a, terminal: a.address })), focused: () => ({ agent: "t2" }),
    spaces: () => [{ tabs: list.map(a => ({ id: a.tab, name: a.address, named: true })) }],
    layout: () => ({ id: "sidebar", size: { px: width } }), shared: () => "grouped",
    theme: () => ({ selected: "selected", hover: "hover" }), setAgentOrder() {},
    div: element, v_flex: element, h_flex: element, svg: element, image: element,
  });
  const fills = new Map<string, unknown>();
  const walk = (node: any) => {
    if (node?.id?.startsWith("ag-t")) fills.set(node.id, node.bg);
    (node?.children ?? []).forEach(walk);
  };
  walk(agents.render({}).node);
  expect([...fills]).toEqual([["ag-t1", undefined], ["ag-t2", "selected"]]);
});

test("a title that opens with a bracket keeps it; each agent's own prefix still goes", () => {
  const title = new Function("View", "localStorage", "props", "run", `${source}; return title;`)(class {}, {}, () => ({}), () => {});
  const clean = new Function("View", "localStorage", "props", "run", `${source}; return clean;`)(class {}, {}, () => ({}), () => {});
  const claude = { program: { name: "claude", key: "claude" }, title: "" };
  // The reported bug: a Claude Code session renamed "[herdr] config" showed as "herdr] config".
  expect(title({ name: "3", named: false }, { ...claude, title: "[herdr] config" })).toBe("[herdr] config");
  // Titles as each agent writes them (from its source).
  const titles: [string, string][] = [
    ["[herdr] config", "[herdr] config"], // Claude Code: the session name
    ["⠋ [herdr] config | herdr-app", "[herdr] config | herdr-app"], // Codex working
    ["[ ! ] Action Required | [herdr] config | herdr-app", "Action Required | [herdr] config | herdr-app"], // Codex needs you
    ["[ . ] Action Required | [herdr] config | herdr-app", "Action Required | [herdr] config | herdr-app"], // ... its other blink phase
    ["● [ ! ] Action Required | herdr-app", "Action Required | herdr-app"], // ... with the microphone on
    ["✦  [herdr] config (herdr-app)", "[herdr] config (herdr-app)"], // Gemini working
    ["◇  Ready (herdr-app)", "Ready (herdr-app)"], // Gemini idle
    ["OC | [herdr] config", "OC | [herdr] config"], // OpenCode
    ["π - [herdr] config - herdr-app", "π - [herdr] config - herdr-app"], // Pi
    ["⠋ - Thinking - [herdr] config - grok", "Thinking - [herdr] config - grok"], // Grok: the spinner's " - " goes too
  ];
  for (const [raw, shown] of titles) expect(clean(raw)).toBe(shown);
});
