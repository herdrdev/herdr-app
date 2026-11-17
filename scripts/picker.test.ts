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
let testSpaces: any[] = [];
let testAgents: any[] = [];
let closed = 0;
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, v_flex: element, h_flex: element,
  Input: { new: element }, InputState: { new: () => ({ handlers: {} as Record<string, any>, on(name: string, handler: any) { this.handlers[name] = handler; }, focus() {} }) },
  agents: () => testAgents, spaces: () => testSpaces, terminals: () => [], theme: () => ({ hover: "hover", selected: "selected" }),
  settings: () => ({ zoom: 1.2 }), props: () => ({}), shared: () => ({}),
  setShared() {}, closeOverlay() { closed++; }, openOverlay() {}, run(...args: unknown[]) { commands.push(args); }, hc() {},
};
const Picker = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Picker", "return class Picker"))(...Object.values(values));
const cx = { notify() {}, stop_propagation() {}, timer: { after() {} }, focus_handle() {}, spawn() {} };
function picker(machines = 12) {
  testSpaces = [];
  testAgents = [];
  closed = 0;
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

test("picker arrows reach earlier and Enter unfolds without closing or losing selection", () => {
  const view = picker(0);
  testSpaces = [
    { id: "busy", name: "busy", status: "working", tabs: [{ id: "busy-tab", name: "busy tab", active: true }], active: true },
    { id: "quiet", name: "quiet", tabs: [{ id: "quiet-tab", name: "quiet tab" }] },
  ];
  commands.length = 0;
  let tree = view.render();
  expect(view.at).toBe(0);
  tree.calls.on_key_down[0]({ key: "down" }, cx);
  tree = view.render();
  expect(find(tree, "pk-earlier").calls.bg).toEqual(["hover"]);
  view.input.handlers.submit({}, cx);
  tree = view.render();
  expect(view.showEarlier).toBe(true);
  expect(closed).toBe(0);
  expect(commands).toEqual([]);
  expect(find(tree, "pk-earlier").calls.bg).toEqual(["hover"]);
  tree.calls.on_key_down[0]({ key: "down" }, cx);
  tree = view.render();
  expect(find(tree, "pk-quiet-tab").calls.bg).toEqual(["hover"]);
  tree.calls.on_key_down[0]({ key: "up" }, cx);
  view.render();
  view.input.handlers.submit({}, cx);
  tree = view.render();
  expect(view.showEarlier).toBe(false);
  expect(find(tree, "pk-earlier").calls.bg).toEqual(["hover"]);
  tree.calls.on_key_down[0]({ key: "down" }, cx);
  view.render();
  view.input.handlers.submit({}, cx);
  expect(commands).toEqual([["tab.new"]]);
  expect(closed).toBe(1);
});

test("picker clicking earlier keeps selection on the toggle when rows shift", () => {
  const view = picker(0);
  testSpaces = [{ id: "quiet", name: "quiet", tabs: [{ id: "quiet-tab", name: "quiet tab" }] }];
  view.render();
  view.at = 2;
  find(view.render(), "pk-earlier").calls.on_click[0]({}, cx);
  let tree = view.render();
  expect(view.showEarlier).toBe(true);
  expect(find(tree, "pk-earlier").calls.bg).toEqual(["hover"]);
  expect(closed).toBe(0);
  tree.calls.on_key_down[0]({ key: "down" }, cx);
  tree = view.render();
  expect(find(tree, "pk-quiet-tab").calls.bg).toEqual(["hover"]);
});

test("picker search selects the first matching tab before earlier", () => {
  for (const busyMatches of [false, true]) {
    const view = picker(0);
    testSpaces = [
      { id: "busy", name: "busy", status: "working", tabs: [{ id: "busy-tab", name: busyMatches ? "match busy" : "busy tab" }] },
      { id: "quiet", name: "quiet", tabs: [{ id: "quiet-tab", name: "match quiet" }] },
    ];
    commands.length = 0;
    view.render();
    view.input.value = () => "match";
    view.input.handlers.change({}, cx);
    const tree = view.render();
    const id = busyMatches ? "busy-tab" : "quiet-tab";
    expect(find(tree, `pk-${id}`).calls.bg).toEqual(["hover"]);
    view.input.handlers.submit({}, cx);
    expect(commands).toEqual([["tab.select", { id }]]);
    expect(closed).toBe(1);
  }
});

test("picker search still lets arrows select and unfold earlier", () => {
  const view = picker(0);
  testSpaces = [{ id: "quiet", name: "quiet", tabs: [{ id: "quiet-tab", name: "quiet tab" }] }];
  commands.length = 0;
  view.input.value = () => "quiet tab";
  view.input.handlers.change({}, cx);
  let tree = view.render();
  expect(find(tree, "pk-quiet-tab").calls.bg).toEqual(["hover"]);
  tree.calls.on_key_down[0]({ key: "up" }, cx);
  tree = view.render();
  expect(find(tree, "pk-earlier").calls.bg).toEqual(["hover"]);
  view.input.handlers.submit({}, cx);
  tree = view.render();
  expect(view.showEarlier).toBe(true);
  expect(find(tree, "pk-earlier").calls.bg).toEqual(["hover"]);
  expect(closed).toBe(0);
  expect(commands).toEqual([]);
  tree.calls.on_key_down[0]({ key: "down" }, cx);
  view.render();
  view.input.handlers.submit({}, cx);
  expect(commands).toEqual([["tab.select", { id: "quiet-tab" }]]);
  expect(closed).toBe(1);
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

function strings(root: any): string[] {
  if (typeof root === "string") return [root];
  return (root?.nodes ?? []).flatMap(strings);
}
function statusPicker() {
  const view = picker(0);
  testSpaces = [
    { id: "a", name: "alpha", status: "blocked", tabs: [
      { id: "a1", name: "one", status: "blocked" },
      { id: "a2", name: "two", status: "working" },
      { id: "a3", name: "three", status: "idle" },
    ] },
    { id: "b", name: "beta", status: "done", tabs: [{ id: "b1", name: "four", status: "done" }] },
    { id: "q", name: "quiet", tabs: [{ id: "q1", name: "five" }] },
  ];
  testAgents = [
    { address: "t1", tab: "a1", status: "blocked" },
    { address: "t2", tab: "a1", status: "idle" },
    { address: "t3", tab: "a2", status: "working" },
    { address: "t4", tab: "a3", status: "idle" },
    { address: "t5", tab: "b1", status: "done" },
  ];
  commands.length = 0;
  return view;
}
function shown(view: any): string[] {
  return view.rows().filter((r: any) => r.kind === "ctx").map((r: any) => r.ctx.id);
}
function press(view: any, key: string, control = true) {
  view.render().calls.on_key_down[0]({ key, modifiers: { control } }, cx);
}

test("ctrl+b/w/f/i show tabs with an agent in that status, and the same key shows all again", () => {
  const view = statusPicker();
  press(view, "b");
  expect(shown(view)).toEqual(["a1"]);
  press(view, "w");
  expect(shown(view)).toEqual(["a2"]);
  press(view, "i");
  expect(shown(view)).toEqual(["a1", "a3"]);
  press(view, "f");
  expect(shown(view)).toEqual(["b1"]);
  expect(view.rows().some((r: any) => r.kind === "earlier")).toBe(false);
  press(view, "f");
  expect(view.status).toBe(null);
  expect(shown(view)).toEqual(["a1", "a2", "a3", "b1"]);
  press(view, "b", false);
  expect(view.status).toBe(null);
});

test("a status filter selects its first tab and Enter goes there", () => {
  const view = statusPicker();
  press(view, "i");
  press(view, "down");
  expect(find(view.render(), "pk-a3").calls.bg).toEqual(["hover"]);
  view.input.handlers.submit({}, cx);
  expect(commands).toEqual([["tab.select", { id: "a3" }]]);
  const again = statusPicker();
  press(again, "w");
  expect(find(again.render(), "pk-a2").calls.bg).toEqual(["hover"]);
});

test("status chips count the searched tabs and click to filter", () => {
  const view = statusPicker();
  let tree = view.render();
  expect(strings(find(tree, "pk-status-all"))).toEqual(["all"]);
  expect(strings(find(tree, "pk-status-blocked"))).toEqual(["●", "blocked", "1"]);
  expect(strings(find(tree, "pk-status-working"))).toEqual(["working", "1"]);
  expect(strings(find(tree, "pk-status-done"))).toEqual(["●", "finished", "1"]);
  expect(strings(find(tree, "pk-status-idle"))).toEqual(["○", "idle", "2"]);
  view.input.value = () => "beta";
  view.input.handlers.change({}, cx);
  tree = view.render();
  expect(strings(find(tree, "pk-status-idle"))).toEqual(["○", "idle", "0"]);
  expect(strings(find(tree, "pk-status-done"))).toEqual(["●", "finished", "1"]);
  view.input.value = () => "";
  view.input.handlers.change({}, cx);
  find(view.render(), "pk-status-working").calls.on_click[0]({}, cx);
  tree = view.render();
  expect(shown(view)).toEqual(["a2"]);
  expect(find(tree, "pk-status-working").calls.bg).toEqual(["selected"]);
  expect(find(tree, "pk-status-all").calls.bg).toBeUndefined();
  find(tree, "pk-status-all").calls.on_click[0]({}, cx);
  expect(view.status).toBe(null);
  expect(shown(view)).toEqual(["a1", "a2", "a3", "b1"]);
});

test("search and a status filter combine, and ctrl+d still closes the selected tab", () => {
  const view = statusPicker();
  view.input.value = () => "three";
  view.input.handlers.change({}, cx);
  press(view, "i");
  expect(shown(view)).toEqual(["a3"]);
  press(view, "d");
  expect(commands).toEqual([["tab.close", { id: "a3" }]]);
});
