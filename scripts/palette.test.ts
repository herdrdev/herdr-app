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
let dark = true;
const shipped = [["graphite", false], ["ink", false], ["moss", false], ["paper", true], ["atom-one-dark", false], ["mine-light", true], ["win98", true]];
const values = {
  View: class {}, div: element, svg: (path: string) => { icons.push(path); return element(); }, v_flex: element, h_flex: element, Input: { new: element }, theme: () => ({}), keys: () => [], win: () => ({ platform }),
  commands: () => [{ name: "app.theme", args: [{ name: "name", required: true }], about: "Use a theme." }], layouts: () => ["main", "win98"], shared: () => "main",
  themes: () => shipped.map(([name, light]) => ({ name, light })), settings: () => ({ dark }),
};
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

test("palette offers only the themes that can show in the current appearance", () => {
  const offered = () => {
    const view = new Palette();
    view.query = ""; view.recent = [];
    return view.rows().map((r: any) => r.e).filter((e: any) => e.name === "app.theme").map((e: any) => e.args.name);
  };
  dark = true;
  expect(offered()).toEqual(["graphite", "ink", "moss", "atom-one-dark"]);
  dark = false;
  // win98 is the Herdr 98 layout's own theme, so it isn't offered here.
  expect(offered()).toEqual(["paper", "mine-light"]);
});

test("palette moves with ctrl-n and ctrl-p as with down and up", () => {
  const view = new Palette();
  view.query = "command"; view.at = 0; view.scroll = 0; view.input = {}; view.handle = {};
  view.rows = () => Array.from({ length: 5 }, (_, i) => ({ e: { name: "test", title: `Command ${i}`, args: {} } }));
  const press = (key: string, keystroke: string) => {
    let stopped = false;
    find(view.render(), "palette")!.calls.on_key_down[0]({ key, keystroke }, { notify() {}, stop_propagation() { stopped = true; } });
    return stopped;
  };
  expect(press("n", "ctrl-n")).toBe(true);
  expect(press("n", "ctrl-n")).toBe(true);
  expect(view.at).toBe(2);
  expect(press("p", "ctrl-p")).toBe(true);
  expect(view.at).toBe(1);
  // Plain letters are typing, not movement.
  expect(press("n", "n")).toBe(false);
  expect(press("p", "p")).toBe(false);
  expect(view.at).toBe(1);
});
