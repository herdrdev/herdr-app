import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

class Element {
  nodes: unknown[] = [];
  calls: Record<string, unknown[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key) => {
        if (typeof key !== "string" || key in target) return Reflect.get(target, key);
        return (...args: unknown[]) => {
          if (key === "child") target.nodes.push(args[0]);
          else if (key === "children" && Array.isArray(args[0])) target.nodes.push(...args[0]);
          else if (key === "when") {
            if (args[0] && typeof args[1] === "function") args[1](proxy);
          } else target.calls[key] = args;
          return proxy;
        };
      },
    });
    return proxy;
  }
}

const source = await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8");
const element = () => new Element();
let choices: { name: string; light: boolean; colors: Record<string, string> }[] = [];
let currentLayout = "main";
let themeReads = 0;
let layoutReads = 0;
let notified = 0;
const commands: unknown[][] = [];
const values = {
  View: class {}, div: element, svg: element, image: element, v_flex: element, h_flex: element,
  Input: { new: element }, Popup: { new: element }, window: () => ({ platform: "macos" }),
  settings: () => ({ theme_light: "paper", theme_dark: "dark-0", zoom: 1, font_size: 14 }),
  themes: () => { themeReads++; return choices; },
  layouts: () => { layoutReads++; return ["main", "win98"]; },
  shared: () => currentLayout,
  run: (...args: unknown[]) => { commands.push(args); },
};
const Settings = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Settings", "return class Settings"))(...Object.values(values));
const cx = { spawn() {}, notify() { notified++; } };

function settings() {
  themeReads = 0; layoutReads = 0; notified = 0; commands.length = 0;
  currentLayout = "main";
  choices = Array.from({ length: 150 }, (_, i) => ({
    name: `${i % 2 ? "light" : "dark"}-${i}`, light: Boolean(i % 2), colors: {},
  }));
  choices.push({ name: "main", light: false, colors: {} }, { name: "win98", light: true, colors: {} });
  const view = new Settings();
  view.person = {}; view.previews = {}; view.previewsOf = ""; view.picking = null;
  return view;
}

function cards(root: unknown): Element[] {
  if (!(root instanceof Element)) return [];
  const id = root.calls.id?.[0];
  return [
    ...(typeof id === "string" && id.startsWith("st-t-") ? [root] : []),
    ...root.nodes.flatMap(cards),
  ];
}

test("settings enumerates themes once and layouts independently of the theme count", () => {
  const view = settings();
  const shown = cards(view.look({}, cx));
  expect(themeReads).toBe(1);
  expect(layoutReads).toBe(2); // One for layout previews, one for theme filtering.
  expect(shown.map((card) => card.calls.id[0])).toEqual(
    choices.filter((choice) => !choice.light).map((choice) => `st-t-${choice.name}`)
      .concat(choices.filter((choice) => choice.light && choice.name !== "win98").map((choice) => `st-t-${choice.name}`)),
  );
  const selected = shown.find((card) => card.calls.id[0] === "st-t-dark-0")!;
  const click = selected.calls.on_click[0];
  if (typeof click !== "function") throw new Error("Missing theme click handler");
  click({}, cx);
  expect(commands).toEqual([["app.theme", { name: "dark-0" }]]);
  expect(notified).toBe(1);
});

test("settings refreshes theme choices and layout ownership on the next render", () => {
  const view = settings();
  view.look({}, cx);
  choices.push({ name: "new-light", light: true, colors: {} });
  currentLayout = "win98";
  const shown = cards(view.look({}, cx)).map((card) => card.calls.id[0]);
  expect(shown).toContain("st-t-new-light");
  expect(shown).toContain("st-t-win98");
  expect(shown).not.toContain("st-t-main");
  expect(themeReads).toBe(2);
  expect(layoutReads).toBe(4);
});
