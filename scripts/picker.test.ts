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
let machineList: { name: string | null; layout: string }[] = [];
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, v_flex: element, h_flex: element,
  Input: { new: element }, InputState: { new: () => ({ on(event: string, handler: any) { (this as any)[event] = handler; }, focus() {}, value: () => "" }) },
  agents: () => [], spaces: () => [], terminals: () => [], theme: () => ({}),
  machines: () => machineList,
  settings: () => ({ zoom: 1.2 }), props: () => ({}), shared: () => ({}),
  setShared() {}, closeOverlay() {}, openOverlay() {}, run(...args: unknown[]) { commands.push(args); },
};
const Picker = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Picker", "return class Picker"))(...Object.values(values));
const cx = { notify() {}, stop_propagation() {}, timer: { after() {} }, focus_handle() {} };
function picker(machines = 12) {
  machineList = Array.from({ length: machines }, (_, i) => ({ name: `machine-${i}`, layout: "ready" }));
  const view = new Picker();
  view.init({}, cx);
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

test("picker updates ready machines live and follows the selected machine", () => {
  const view = picker(2);
  const tree = view.render();
  tree.calls.on_key_down[0]({ key: "up" }, cx);
  view.render();
  expect(view.pickable[view.at].machine).toBe("machine-1");
  machineList = [
    { name: null, layout: "ready" },
    { name: "cloud", layout: "ready" },
    ...machineList,
    { name: "loading", layout: "loading" },
    { name: "offline", layout: "unavailable" },
  ];
  view.render();
  expect(view.pickable.filter((r: any) => r.kind === "space").map((r: any) => r.machine)).toEqual([null, "cloud", "machine-0", "machine-1"]);
  expect(view.at).toBe(4);
  expect(view.pickable[view.at].machine).toBe("machine-1");
  // Enter also reconciles a machine change that arrived before the next render.
  machineList = machineList.filter(m => m.name !== "cloud");
  commands.length = 0;
  view.input.submit();
  expect(commands).toEqual([["space.new", { machine: "machine-1" }]]);
  machineList = [];
  view.render();
  expect(view.pickable[view.at]).toEqual({ kind: "space", machine: null });
});

test("changing the filter resets selection instead of preserving the previous row", () => {
  const view = picker(2);
  view.render().calls.on_key_down[0]({ key: "up" }, cx);
  view.render();
  view.input.change({}, cx);
  view.render();
  expect(view.pickable[view.at]).toEqual({ kind: "new" });
});

test("a growing picker draws new rows before its next size callback", () => {
  // Fractional zoom can round the measured height slightly below the request.
  for (const height of [68, 67.9]) {
    const view = picker(0);
    measure(view, height);
    machineList = [{ name: "cloud", layout: "ready" }];
    const tree = view.render();
    expect(find(tree, "pk-list").calls.h[0]).toBe(100);
    expect(find(tree, "pk-space-cloud")).toBeDefined();
  }
});

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
