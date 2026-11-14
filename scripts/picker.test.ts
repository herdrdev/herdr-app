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
let closed = 0;
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, v_flex: element, h_flex: element,
  Input: { new: element }, InputState: { new: () => ({ handlers: {} as Record<string, any>, on(name: string, handler: any) { this.handlers[name] = handler; }, focus() {} }) },
  agents: () => [], spaces: () => testSpaces, terminals: () => [], theme: () => ({ hover: "hover" }),
  settings: () => ({ zoom: 1.2 }), props: () => ({}), shared: () => ({}),
  setShared() {}, closeOverlay() { closed++; }, openOverlay() {}, run(...args: unknown[]) { commands.push(args); }, hc() {},
};
const Picker = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Picker", "return class Picker"))(...Object.values(values));
const cx = { notify() {}, stop_propagation() {}, timer: { after() {} }, focus_handle() {}, spawn() {} };
function picker(machines = 12) {
  testSpaces = [];
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
