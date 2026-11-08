import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Exercise the shipped view and its event handlers without a window or a core.
// Size callbacks use physical pixels, while element dimensions use zoomed units.
class Element {
  nodes: Element[] = [];
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
const source = await readFile(new URL("../defaults/mods/picker/main.js", import.meta.url), "utf8");
const element = () => new Element();
const commands: unknown[][] = [];
// What the herdr module reports, and the search line.
const empty = { agents: [], spaces: [], terminals: [], panes: [] };
let state: Record<string, any[]> = empty;
let typed = "";
const handlers: Record<string, Function> = {};
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, v_flex: element, h_flex: element,
  Input: { new: element }, InputState: { new: () => ({ on(event: string, f: Function) { handlers[event] = f; }, focus() {}, value: () => typed }) },
  agents: () => state.agents, spaces: () => state.spaces, terminals: () => state.terminals, panes: () => state.panes, theme: () => ({}),
  settings: () => ({ zoom: 1.2 }), props: () => ({}), shared: () => ({}),
  setShared() {}, closeOverlay() {}, openOverlay() {}, run(...args: unknown[]) { commands.push(args); }, hc() {},
};
const Picker = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Picker", "return class Picker"))(...Object.values(values));
const cx = { notify() {}, stop_propagation() {}, timer: { after() {} }, focus_handle() {}, spawn() {} };
function picker(machines = 12, world: Record<string, any[]> = empty) {
  state = world;
  typed = "";
  const view = new Picker();
  view.init({}, cx);
  view.machines = Array.from({ length: machines }, (_, i) => `machine-${i}`);
  return view;
}
function find(root: Element, id: string): Element {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return undefined as any;
}
function measure(view: any, height: number) {
  const list = find(view.render(), "pk-list");
  expect(list).toBeDefined();
  list.calls.on_size_change[0]({ width: 500, height: height * 1.2 }, cx);
}

test("picker reveals keyboard selection in its measured, constrained list", () => {
  for (const [machines, height] of [[12, 144], [0, 48]]) {
    const view = picker(machines);
    measure(view, height);
    let tree = view.render();
    tree.calls.on_key_down[0]({ key: "up" }, cx);
    tree = view.render();
    const selected = find(tree, machines ? `pk-space-machine-${machines - 1}` : "pk-space-here");
    expect(selected.calls.top[0]).toBe(height - 32);
    expect(selected.calls.bottom[0]).toBe(0);
    const list = find(tree, "pk-list");
    expect(list.calls.h[0]).toBeLessThanOrEqual(320);
    list.calls.on_scroll_wheel[0]({ delta: { y: -1000 } }, cx);
    expect(find(view.render(), selected.calls.id[0])).toBeDefined();
    tree.calls.on_key_down[0]({ key: "down" }, cx);
    const first = find(view.render(), "pk-new");
    expect(first.calls.top[0]).toBeGreaterThanOrEqual(0);
    expect(first.calls.bottom[0]).toBeGreaterThanOrEqual(0);
  }
});

test("picker space choices explicitly target this computer or the selected machine", () => {
  const view = picker();
  commands.length = 0;
  view.go({ kind: "space", machine: null });
  view.go({ kind: "space", machine: "cloud" });
  expect(commands).toEqual([
    ["space.new", { machine: "" }],
    ["space.new", { machine: "cloud" }],
  ]);
});

test("picker preserves small pixel touchpad deltas", () => {
  const view = picker();
  const list = find(view.render(), "pk-list");
  for (const delta of [-1, -3.9, -4]) {
    view.scroll = 0;
    list.calls.on_scroll_wheel[0]({ delta: { y: delta } }, cx);
    expect(view.scroll).toBe(-delta);
  }
});

// Two local spaces and one on "vps". t7 is t1's sub-agent: no pane, so it
// belongs to t1's tab.
const tab = (id: string, name: string) => ({ id, name, active: false, status: null });
const term = (address: string, name: string | null = null, parent: string | null = null) => ({ address, name, parent, state: "live" });
const world = {
  spaces: [
    { id: "s1", name: "herdr", status: "working", active: true, tabs: [tab("tab1", "editor"), tab("tab2", "build"), tab("tab4", "docs")] },
    { id: "s2", name: "lab", status: null, tabs: [tab("tab5", "gpu reserve"), tab("tab6", "server")] },
    { id: "vps/s3", name: "api", status: null, tabs: [tab("vps/tab3", "web")] },
  ],
  agents: [{ address: "t1", tab: "tab1", program: { name: "Claude Code" } }],
  terminals: [term("t1"), term("t12", "logs"), term("t2", "worker"), term("t21"), term("t7", "reviewer", "t1"), term("vps/t1wc")],
  panes: [
    { pane: "p1", terminal: "t1", tab: "tab1" },
    { pane: "p2", terminal: "t12", tab: "tab1" },
    { pane: "p3", terminal: "t2", tab: "tab2" },
    { pane: "p4", terminal: "t21", tab: "tab4" },
    { pane: "vps/p5", terminal: "vps/t1wc", tab: "vps/tab3" },
  ],
};
function search(query: string) {
  const view = picker(0, world);
  typed = query;
  handlers.change({}, cx);
  const tabs = view.rows().filter((r: any) => r.kind === "ctx");
  commands.length = 0;
  handlers.submit();
  return { view, tabs: tabs.map((r: any) => r.ctx.name), terms: tabs.map((r: any) => r.term?.x.address ?? null), commands: [...commands] };
}
function texts(root: any): string[] {
  return typeof root === "string" ? [root] : (root?.nodes ?? []).flatMap(texts);
}

test("picker finds a terminal by its exact address and focuses its pane", () => {
  const found = search("t2");
  // t21 starts with "t2" too, but the exact address wins Enter.
  expect(found.tabs).toEqual(["build", "docs"]);
  expect(found.commands).toEqual([["tab.select", { id: "tab2" }], ["pane.focus", { pane: "p3" }]]);
});

test("picker matches addresses by prefix, not anywhere in them", () => {
  const found = search("t1");
  // t21 holds a 1 after a t, but doesn't start with t1.
  expect(found.tabs).toEqual(["editor", "web"]);
  expect(found.terms).toEqual(["t1", "vps/t1wc"]);
});

test("picker finds another machine's terminal by its qualified address", () => {
  const found = search("vps/t1wc");
  expect(found.tabs).toEqual(["web"]);
  expect(found.commands).toEqual([["tab.select", { id: "vps/tab3" }], ["pane.focus", { pane: "vps/p5" }]]);
});

test("picker finds a terminal by name and shows which one matched", () => {
  const found = search("worker");
  expect(found.tabs).toEqual(["build"]);
  expect(texts(found.view.render())).toContain("t2 worker");
});

test("picker finds a sub-agent in its parent's tab and peeks at it", () => {
  for (const query of ["t7", "review"]) {
    const found = search(query);
    expect(found.tabs).toEqual(["editor"]);
    expect(found.commands).toEqual([["tab.select", { id: "tab1" }], ["terminal.peek", { terminal: "t7" }]]);
  }
});

test("picker ranks names fuzzily, as the palette does", () => {
  // Both hold s, r, v in order; "server" has them at word starts and in a run.
  const found = search("srv");
  expect(found.tabs).toEqual(["server", "gpu reserve"]);
  expect(found.commands).toEqual([["tab.select", { id: "tab6" }]]);
});

test("picker matches a space and tab name typed together", () => {
  expect(search("herdr editor").commands).toEqual([["tab.select", { id: "tab1" }]]);
});

test("picker shows only new work when nothing matches", () => {
  const found = search("zzz");
  expect(found.tabs).toEqual([]);
  expect(found.view.rows().map((r: any) => r.kind)).toEqual(["new", "space"]);
  expect(found.commands).toEqual([["tab.new"]]);
});
