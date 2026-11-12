import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/workspace-chrome/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "")
  .replace("export default class WorkspaceChrome", "class WorkspaceChrome")
  .replace(/^export function/gm, "function");

function harness(px: number, platform = "macos", fullscreen = false) {
  const runs: unknown[] = [];
  const layout = { id: "root", children: [{ id: "sidebar", size: { px } }] };
  const mod = new Function("View", "run", "layout", "window", "shared", "setShared", `${source}; return { WorkspaceChrome, snapWidth, brandFits };`)(
    class {}, (...args: unknown[]) => runs.push(args), () => layout, () => ({ platform, fullscreen }), () => undefined, () => {},
  );
  return { ...mod, runs, layout };
}

test("a sidebar let go between the rail and the full sidebar snaps to the nearer one", () => {
  const { snapWidth } = harness(268);
  expect(snapWidth(44, 76)).toBe(76);
  expect(snapWidth(100, 76)).toBe(76);
  expect(snapWidth(117, 76)).toBe(76);
  expect(snapWidth(118, 76)).toBe(160);
  expect(snapWidth(159, 76)).toBe(160);
  expect(snapWidth(80, 44)).toBe(44);
  expect(snapWidth(120, 44)).toBe(160);
  for (const [px, rail] of [[76, 76], [44, 44], [160, 76], [268, 76], [480, 44], [Infinity, 76]]) expect(snapWidth(px, rail)).toBeNull();
});

test("the brand row drops the name, then the logo, never its buttons", () => {
  const { brandFits } = harness(268);
  expect(brandFits(268, 76)).toEqual({ logo: true, name: true });
  expect(brandFits(190, 76)).toEqual({ logo: true, name: false });
  expect(brandFits(160, 76)).toEqual({ logo: false, name: false });
  expect(brandFits(160, 14)).toEqual({ logo: true, name: true });
});

test("the brand row snaps once per width, so a resize that doesn't land isn't retried every frame", () => {
  const { WorkspaceChrome, runs, layout } = harness(100);
  const chrome = new WorkspaceChrome();
  chrome.snap();
  chrome.snap();
  expect(runs).toEqual([["layout.resize", { id: "sidebar", px: 76 }]]);
  layout.children[0].size.px = 76;
  chrome.snap();
  expect(runs.length).toBe(1);
  layout.children[0].size.px = 140;
  chrome.snap();
  expect(runs.at(-1)).toEqual(["layout.resize", { id: "sidebar", px: 160 }]);
});

test("off macOS the rail is 44px wide", () => {
  const { WorkspaceChrome, runs } = harness(30, "linux");
  new WorkspaceChrome().snap();
  expect(runs).toEqual([["layout.resize", { id: "sidebar", px: 44 }]]);
});
