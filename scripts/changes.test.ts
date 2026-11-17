import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../defaults/mods/changes/main.js", import.meta.url), "utf8");
const rowsSource = source.slice(source.indexOf("const untab ="), source.indexOf("\nexport default class"));
const diffRows = new Function(`${rowsSource}; return diffRows;`)();

test("Changes keeps tab indentation, to the next 4-column stop", () => {
  const rows = diffRows(["@@ -1,2 +1,2 @@", "-\tif x {", "+\t\tif x {", " a\tb", "+    spaces"]);
  expect(rows.map((r: { text: string }) => r.text)).toEqual(["@@ -1,2 +1,2 @@", "    if x {", "        if x {", "a   b", "    spaces"]);
});

test("Changes keeps its folder while a mod pane has focus", () => {
  const state: { panes: any[]; focused: any } = { panes: [], focused: null };
  const locationSource = source.slice(source.indexOf("// A mod pane has no folder"), source.indexOf("const key ="));
  const space = { id: "s1", active: true, folder: "/tmp", machine: null };
  const location = new Function("spaces", "panes", "focused", `${locationSource}; return location;`)(() => [space], () => state.panes, () => state.focused);
  state.panes = [{ pane: "p1", terminal: "t1", cwd: "/home/me/repo" }, { pane: "p2", terminal: null, mod: "viewer", cwd: null }];
  state.focused = { space: "s1", pane: "p1", terminal: "t1", cwd: null };
  expect(location().cwd).toBe("/home/me/repo");
  state.focused = { space: "s1", pane: "p2", terminal: null, cwd: null };
  expect(location().cwd).toBe("/home/me/repo");
});
