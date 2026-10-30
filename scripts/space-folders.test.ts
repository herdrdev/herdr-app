import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const read = (mod: string) => readFile(new URL(`../defaults/mods/${mod}/main.js`, import.meta.url), "utf8");
const spaces = await read("spaces");
const dialog = await read("space-folder");
const block = (source: string) => source.slice(source.indexOf("// --- Folders:"), source.indexOf("// --- end folders"));
const shownOrderSource = spaces.slice(spaces.indexOf("function shownOrder("), spaces.indexOf("\n}\n", spaces.indexOf("function shownOrder(")) + 2);

type Folder = { name: string; collapsed: boolean; ids: string[]; parent: string | null };

// The helpers over a store of their own.
function load(list: Folder[]) {
  const store: { list: Folder[] } = { list };
  const api = new Function("shared", "setShared", `${block(spaces)}; ${shownOrderSource}; return { moveToFolder, moveFolder, lineage, prune, shownOrder };`)(
    () => store.list,
    (_key: string, value: Folder[]) => { store.list = value; },
  );
  return { ...api, store };
}
const folder = (name: string, ids: string[], parent: string | null = null): Folder => ({ name, collapsed: false, ids, parent });

test("the spaces and space-folder mods share one copy of the folder helpers", () => {
  expect(block(spaces).length).toBeGreaterThan(0);
  expect(block(dialog)).toBe(block(spaces));
});

test("a space moves into a folder, case-insensitively, and an emptied folder goes", () => {
  const f = load([folder("Work", ["a"]), folder("home", ["b"])]);
  f.moveToFolder("b", "work");
  expect(f.store.list).toEqual([folder("Work", ["a", "b"])]);
  f.moveToFolder("a", "new");
  expect(f.store.list).toEqual([folder("Work", ["b"]), folder("new", ["a"])]);
  f.moveToFolder("b", "");
  expect(f.store.list).toEqual([folder("new", ["a"])]);
});

test("a folder nests, never inside itself, and goes to the top with no name", () => {
  const f = load([folder("a", ["1"]), folder("b", ["2"], "a")]);
  f.moveFolder("a", "b");
  expect(f.store.list).toEqual([folder("a", ["1"]), folder("b", ["2"], "a")]);
  f.moveFolder("b", null);
  expect(f.store.list).toEqual([folder("a", ["1"]), folder("b", ["2"])]);
  f.moveFolder("b", "fresh");
  expect(f.store.list).toEqual([folder("a", ["1"]), folder("b", ["2"], "fresh"), folder("fresh", [], null)]);
});

test("a folder holding only folders stays; a loop of empty folders goes", () => {
  const f = load([]);
  expect(f.prune([folder("outer", []), folder("inner", ["1"], "outer")])).toEqual([folder("outer", []), folder("inner", ["1"], "outer")]);
  expect(f.prune([folder("x", [], "y"), folder("y", [], "x")])).toEqual([]);
  expect(f.prune([folder("x", ["1"], "y"), folder("y", [], "x")])).toEqual([folder("x", ["1"])]);
});

test("the shown order keeps worktrees under their checkout and folders together", () => {
  const f = load([]);
  const row = (id: string, tree: string | null = null, machine: string | null = null) => ({ w: { id, machine }, tree });
  const list = [folder("F", ["a", "c"]), folder("G", ["e"], "F")];
  expect(f.shownOrder([row("a"), row("a1", "└─"), row("b"), row("c"), row("d"), row("e")], list)).toEqual(["a", "a1", "c", "e", "b", "d"]);
  // A folder with spaces on two machines gathers them in each machine's block.
  expect(f.shownOrder([row("a"), row("b"), row("r/c", null, "r"), row("r/d", null, "r"), row("r/e", null, "r")], [folder("F", ["a", "r/c", "r/e"])]))
    .toEqual(["a", "b", "r/c", "r/e", "r/d"]);
});
