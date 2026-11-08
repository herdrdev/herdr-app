import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Exercise the shipped tab strip's scroll handler without a window or a core.
const source = await readFile(new URL("../defaults/mods/tabs/main.js", import.meta.url), "utf8");
const commands: string[] = [];
const values: Record<string, any> = {
  View: class {}, div() {}, svg() {}, image() {}, h_flex() {},
  spaces: () => [], panes: () => [], openOverlay() {}, theme: () => ({}), window: () => null,
  shared: () => null, layout: () => null, settings: () => ({}),
  run(command: string) { commands.push(command); },
};
const Tabs = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Tabs", "return class Tabs"))(...Object.values(values));

type Phase = "started" | "moved" | "ended" | "cancelled";
const pixels = (phase: Phase, x: number, y: number) => ({ touch_phase: phase, delta: { x, y } });
const lines = (x: number, y: number) => ({ touch_phase: "moved", delta: { x: x * 20, y: y * 20 }, delta_lines: { x, y } });
const repeat = (count: number, event: object) => Array.from({ length: count }, () => event);
function scroll(...events: object[]) {
  const view = new Tabs();
  commands.length = 0;
  for (const event of events) view.wheel(event);
  return [...commands];
}

test("a trackpad swipe switches one tab and ignores the momentum after it", () => {
  const swipe = [pixels("started", 0, 0), ...repeat(30, pixels("moved", 0, 4)), pixels("ended", 0, 0)];
  expect(scroll(...swipe, ...repeat(30, pixels("moved", 0, 3)))).toEqual(["tab.prev"]);
  expect(scroll(pixels("started", 0, 0), ...repeat(30, pixels("moved", 0, -4)))).toEqual(["tab.next"]);
});

test("each trackpad gesture switches again", () => {
  const swipe = [pixels("started", 0, 0), pixels("moved", 0, 30), pixels("ended", 0, 0)];
  expect(scroll(...swipe, ...swipe)).toEqual(["tab.prev", "tab.prev"]);
});

test("a short or sideways trackpad swipe does not switch tabs", () => {
  expect(scroll(pixels("started", 0, 0), pixels("moved", 0, 10), pixels("ended", 0, 0))).toEqual([]);
  expect(scroll(pixels("started", 0, 0), ...repeat(20, pixels("moved", -8, 2)))).toEqual([]);
});

test("each mouse wheel notch switches one tab", () => {
  expect(scroll(lines(0, 1), lines(0, 1), lines(0, -1))).toEqual(["tab.prev", "tab.prev", "tab.next"]);
  expect(scroll(lines(1, 0))).toEqual([]);
});

test("pixel scrolling without gesture phases switches once per stretch", () => {
  expect(scroll(...repeat(12, pixels("moved", 0, 4)))).toEqual(["tab.prev", "tab.prev"]);
});
