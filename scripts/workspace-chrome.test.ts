import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// The sidebar's top bar, at expanded, collapsed, and legacy/narrow widths.
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
const element = () => new Element();
const source = await readFile(new URL("../defaults/mods/workspace-chrome/main.js", import.meta.url), "utf8");

function find(root: any, id: string): any {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return undefined;
}
function ids(root: any, out: string[] = []): string[] {
  if (root?.calls?.id) out.push(root.calls.id[0]);
  (root?.nodes ?? []).forEach((child: any) => ids(child, out));
  return out;
}

function harness({ platform = "linux", fullscreen = false, width = 268, saved = 268, footer = false } = {}) {
  const runs: any[][] = [];
  const writes: any[][] = [];
  const values = {
    View: class {}, div: element, image: element, svg: element, h_flex: element, v_flex: element,
    theme: () => ({}),
    run: (...args: any[]) => runs.push(args),
    props: () => footer ? { footer: true } : {},
    window: () => ({ platform, fullscreen }),
    shared: (key: string) => key === "sidebar.width" ? saved : undefined,
    setShared: (...args: any[]) => writes.push(args),
    updates: () => ({ app: { ready: false } }),
    layout: () => ({ id: "root", children: [{ id: "sidebar", size: { px: width } }] }),
  };
  const Chrome = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class WorkspaceChrome", "return class WorkspaceChrome"))(...Object.values(values));
  return { root: new Chrome().render(), runs, writes };
}

const RAILS = [["linux", false, 72], ["macos", false, 140], ["macos", true, 72]] as const;

test("expanded bars keep search then sidebar, and collapse to the platform rail", () => {
  for (const [platform, fullscreen, rail] of RAILS) {
    const h = harness({ platform, fullscreen });
    expect(ids(h.root)).toEqual(["search", "sidebar-toggle"]);
    expect(h.runs).toEqual([]);
    find(h.root, "sidebar-toggle").calls.on_click[0]();
    expect(h.writes).toEqual([["sidebar.width", 268], ["sidebar.collapsed", true]]);
    expect(h.runs).toEqual([["layout.resize", { id: "sidebar", px: rail }]]);
  }
});

test("collapsed bars restore the saved expanded width", () => {
  for (const [platform, fullscreen, rail] of RAILS) {
    const h = harness({ platform, fullscreen, width: rail, saved: 300 });
    expect(ids(h.root)).toEqual(["search", "sidebar-toggle"]);
    expect(h.runs).toEqual([]);
    find(h.root, "sidebar-toggle").calls.on_click[0]();
    expect(h.writes).toEqual([["sidebar.collapsed", false]]);
    expect(h.runs).toEqual([["layout.resize", { id: "sidebar", px: 300 }]]);
  }
});

test("legacy, dragged, and fullscreen-transition rails are widened to fit both controls", () => {
  for (const [platform, fullscreen, width, rail] of [
    ["linux", false, 44, 72], // the old Linux rail
    ["macos", false, 76, 140], // the old macOS rail
    ["macos", true, 44, 72], // too narrow in fullscreen
    ["macos", false, 72, 140], // back out of fullscreen
  ] as const) {
    const h = harness({ platform, fullscreen, width });
    expect(ids(h.root)).toEqual(["search", "sidebar-toggle"]);
    expect(h.runs).toEqual([["layout.resize", { id: "sidebar", px: rail }]]);
    find(h.root, "sidebar-toggle").calls.on_click[0]();
    expect(h.runs.at(-1)).toEqual(["layout.resize", { id: "sidebar", px: 268 }]);
  }
});

test("the footer keeps settings and add machine, without another toggle", () => {
  const h = harness({ footer: true, width: 72 });
  expect(ids(h.root)).toEqual(["settings", "machine"]);
});
