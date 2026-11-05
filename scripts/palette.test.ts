import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Exercise the shipped palette's handler with the pixel deltas from GPUI.
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
const source = await readFile(new URL("../defaults/frame/palette/main.js", import.meta.url), "utf8");
const element = () => new Element();
let platform = "linux";
const icons: string[] = [];
const values = { View: class {}, div: element, svg: (path: string) => { icons.push(path); return element(); }, v_flex: element, h_flex: element, Input: { new: element }, theme: () => ({}), keys: () => [], win: () => ({ platform }) };
const Palette = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Palette", "return class Palette"))(...Object.values(values));
function find(root: Element, id: string): Element | undefined {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
}

test("palette follows small pixel movements and converted wheel deltas", () => {
  for (const delta of [-1, -3.9, -4, -48]) {
    const view = new Palette();
    view.query = "command"; view.at = 0; view.scroll = 0; view.input = {}; view.handle = {};
    view.rows = () => Array.from({ length: 50 }, (_, i) => ({ e: { name: "test", title: `Command ${i}`, args: {} } }));
    const list = find(view.render(), "palette-rows");
    expect(list).toBeDefined();
    list!.calls.on_scroll_wheel[0]({ delta: { y: delta } }, { notify() {} });
    expect(view.scroll).toBe(-delta);
  }
});

test("palette input shows the command key only on macos", () => {
  for (const [os, icon] of [["macos", "frame/palette/command.svg"], ["windows", "mods/workspace-chrome/search.svg"], ["linux", "mods/workspace-chrome/search.svg"]]) {
    platform = os; icons.length = 0;
    const view = new Palette();
    view.query = ""; view.at = 0; view.scroll = 0; view.input = {}; view.handle = {};
    view.rows = () => [];
    view.render();
    expect(icons).toEqual([icon]);
  }
});

test("palette scrolls past group titles with small movements", () => {
  for (const delta of [-3, -16, -25]) {
    const view = new Palette();
    view.query = ""; view.at = 0; view.scroll = 0; view.input = {}; view.handle = {};
    view.rows = () => Array.from({ length: 30 }, (_, i) => ({ head: `Group ${Math.floor(i / 6)}`, e: { name: "test", title: `Command ${i}`, args: {} } }));
    for (let step = 0; step < 400 && (step === 0 || view.scroll < view.maxScroll); step++) {
      const list = find(view.render(), "palette-rows");
      const before = view.scroll;
      list!.calls.on_scroll_wheel[0]({ delta: { y: delta } }, { notify() {} });
      view.render();
      expect(view.scroll).toBe(Math.min(before - delta, view.maxScroll));
    }
    expect(view.scroll).toBe(view.maxScroll);
  }
});
