import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Click the shipped sidebar row's search button without a window or a core.
class Element {
  nodes: Element[] = [];
  calls: Record<string, unknown[]> = {};
  findById(id: string): Element | undefined {
    if (this.calls.id?.[0] === id) return this;
    for (const child of this.nodes) {
      const found = child?.findById?.(id);
      if (found) return found;
    }
  }
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? Reflect.get(target, key) : (...args: unknown[]) => {
        if (key === "child") target.nodes.push(args[0] as Element);
        else if (key === "when") { if (args[0]) (args[1] as (el: Element) => void)(proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}
const source = await readFile(new URL("../defaults/mods/workspace-chrome/main.js", import.meta.url), "utf8");
const element = () => new Element();
function search(blockProps: unknown) {
  const commands: [string, unknown][] = [];
  const values: Record<string, unknown> = {
    View: class {}, div: element, image: element, svg: element, h_flex: element, v_flex: element, theme: () => ({}),
    run(command: string, args: unknown) { commands.push([command, args]); }, props: () => blockProps,
    window: () => ({ platform: "linux" }), shared: () => null, setShared() {}, updates: () => ({ app: {} }),
    layout: () => ({ id: "root", children: [{ id: "sidebar", size: { px: 268 } }] }),
  };
  const Chrome = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class WorkspaceChrome", "return class WorkspaceChrome"))(...Object.values(values));
  const button = (new Chrome().render() as Element).findById("search")!;
  (button.calls.on_click[0] as () => void)();
  return commands;
}

test("the search button opens the shipped picker", () => {
  expect(search(null)).toEqual([["mod.open", { name: "picker" }]]);
  expect(search({})).toEqual([["mod.open", { name: "picker" }]]);
});

test("the search button opens the picker its block names", () => {
  expect(search({ picker: "my-picker" })).toEqual([["mod.open", { name: "my-picker" }]]);
});
