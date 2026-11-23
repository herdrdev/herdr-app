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
// nextCommand() resolves with what the next key binding sends.
const commandQueue: ((c: string) => void)[] = [];
const nextCommand = () => new Promise<string>((resolve) => commandQueue.push(resolve));
const storage = { getItem: () => null, setItem() {} };
const inputState = { new: () => ({ on() {}, focus() {}, value: () => "" }) };
const values = {
  View: class {}, div: element, svg: (path: string) => { icons.push(path); return element(); }, v_flex: element, h_flex: element, Input: { new: element }, theme: () => ({}), keys: () => [], win: () => ({ platform }),
  commands: () => [{ name: "app.theme", args: [{ name: "name", required: true }], about: "Use a theme." }], layouts: () => ["main", "win98"], shared: () => "main",
  themes: () => shipped.map(([name, light]) => ({ name, light })), settings: () => ({ dark }),
  nextCommand, localStorage: storage, InputState: inputState,
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

test("palette moves with the commands keys.json sends it, and arrows still move it", async () => {
  const view = new Palette();
  const tasks: Promise<void>[] = [];
  let notified = 0;
  const cx = { spawn: (f: any) => tasks.push(f({ notify: () => notified++ })), timer: { after() {} }, focus_handle: () => ({}) };
  view.init({}, cx);
  view.rows = () => Array.from({ length: 25 }, (_, i) => ({ e: { name: "test", title: `Command ${i}`, args: {} } }));
  const send = async (c: string) => { commandQueue.shift()!(c); await new Promise((r) => setTimeout(r, 0)); };
  await send("down");
  await send("down");
  expect(view.at).toBe(2);
  await send("up");
  expect(view.at).toBe(1);
  await send("page_down");
  expect(view.at).toBe(11);
  await send("page_down");
  await send("page_down");
  expect(view.at).toBe(24);
  await send("page_up");
  expect(view.at).toBe(14);
  // Unknown commands (and prototype names) leave the selection alone and don't redraw.
  const before = notified;
  await send("toString");
  await send("nope");
  expect(view.at).toBe(14);
  expect(notified).toBe(before);
  expect(notified).toBe(7);

  const press = (key: string) => {
    let stopped = false;
    view.query = "command"; view.input = {};
    find(view.render(), "palette")!.calls.on_key_down[0]({ key }, { notify() {}, stop_propagation() { stopped = true; } });
    return stopped;
  };
  expect(press("down")).toBe(true);
  expect(view.at).toBe(15);
  expect(press("pageup")).toBe(true);
  expect(view.at).toBe(5);
  // Letters are typing, not movement.
  expect(press("n")).toBe(false);
  expect(view.at).toBe(5);
});
