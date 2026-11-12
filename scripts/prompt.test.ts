import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The prompt's two submit paths, without a window or a core.
const source = await readFile(new URL("../defaults/mods/prompt/main.js", import.meta.url), "utf8");

function harness(props: any, stored: Record<string, string> = {}) {
  const runs: unknown[][] = [];
  const writes: unknown[][] = [];
  const closed: string[] = [];
  const input = { on() {}, focus() {}, set_selected_range() {} };
  const values = {
    View: class {}, div: () => ({}), v_flex: () => ({}), h_flex: () => ({}),
    InputState: { new: () => input }, Input: { new: () => ({}) },
    run: (...args: unknown[]) => runs.push(args),
    props: () => props,
    closeOverlay: (id: string) => closed.push(id),
    theme: () => ({}), shared: () => stored,
    setShared: (...args: unknown[]) => writes.push(args),
  };
  const Prompt = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Prompt", "return class Prompt"))(...Object.values(values));
  const view = new Prompt();
  view.init({}, { timer: { after() {} }, focus_handle: () => ({}) });
  return { view, runs, writes, closed };
}

test("a shared prompt trims the text and keeps other ids", () => {
  const h = harness({ overlay: "o1", shared: { key: "space.emojis", id: "s1" } }, { s1: "🚀", s2: "🐛" });
  h.view.text = " 🦀 ";
  h.view.submit();
  expect(h.closed).toEqual(["o1"]);
  expect(h.writes).toEqual([["space.emojis", { s1: "🦀", s2: "🐛" }]]);
  expect(h.runs).toEqual([]);
});

test("an empty shared prompt removes its id", () => {
  const h = harness({ overlay: "o1", shared: { key: "space.emojis", id: "s1" } }, { s1: "🚀", s2: "🐛" });
  h.view.text = " ";
  h.view.submit();
  expect(h.closed).toEqual(["o1"]);
  expect(h.writes).toEqual([["space.emojis", { s2: "🐛" }]]);
  expect(h.runs).toEqual([]);
});

test("a command prompt still runs its command", () => {
  const h = harness({ overlay: "o1", command: { name: "space.rename", args: { id: "s1" } } });
  h.view.text = "New name";
  h.view.submit();
  expect(h.closed).toEqual(["o1"]);
  expect(h.runs).toEqual([["space.rename", { id: "s1", name: "New name" }]]);
  expect(h.writes).toEqual([]);
});
