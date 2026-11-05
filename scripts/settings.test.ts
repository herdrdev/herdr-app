import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8");

// The panel choice: the list the tab bar reads, after one panel is turned on or off.
const choice = source.slice(source.indexOf("function panelsAfter("), source.indexOf("\nexport default class"));
const panelsAfter = new Function(`${choice}; return panelsAfter;`)() as (
  current: string[],
  id: string,
  wasOn: boolean,
) => string[];

test("Turning a panel off takes it out of the choice", () => {
  expect(panelsAfter(["files", "changes"], "files", true)).toEqual(["changes"]);
  expect(panelsAfter(["files"], "files", true)).toEqual([]);
});

test("Turning a panel on adds it to the choice", () => {
  expect(panelsAfter(["changes"], "files", false)).toEqual(["changes", "files"]);
  expect(panelsAfter([], "files", false)).toEqual(["files"]);
});

test("Turning a panel on never repeats it", () => {
  expect(panelsAfter(["files", "changes"], "files", false)).toEqual(["changes", "files"]);
});
