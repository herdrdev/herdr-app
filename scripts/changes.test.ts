import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../defaults/mods/changes/main.js", import.meta.url), "utf8");
const diffRows = new Function(`${source.slice(source.indexOf("const countLines"), source.indexOf("\nexport default class"))}; return diffRows;`)();

test("Changes keeps tab indentation in diff rows", () => {
  const rows = diffRows(["@@ -1,3 +1,4 @@", " func tabs() {", "-\tif i > 0 {", "+\t\tprintln(i)", " \t}", "+x\tyz\tw", "+\t", "+", "+    spaces"]);
  expect(rows.map((r: { text: string }) => r.text)).toEqual([
    "@@ -1,3 +1,4 @@", "func tabs() {", "  if i > 0 {", "    println(i)", "  }", "x yz  w", "  ", " ", "    spaces",
  ]);
  expect(rows.map((r: { number?: number }) => r.number)).toEqual([undefined, 1, 2, 2, 3, 4, 5, 6, 7]);
});

test("Changes expands long lines in one pass", () => {
  // The preview's limits: 1,500 lines of up to 4,000 characters.
  const long = "x".repeat(4000), rows = diffRows(Array.from({ length: 1500 }, (_, i) => i % 2 ? `+${long}` : `+\t${long}`));
  expect(rows[0].text).toBe(`  ${long}`);
  expect(rows[1].text).toBe(long);
});
