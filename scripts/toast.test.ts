import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Exercise the shipped toast and its click handlers without a window or a core.
class Element {
  nodes: Element[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child") target.nodes.push(args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}
const source = await readFile(new URL("../defaults/mods/toast/main.js", import.meta.url), "utf8");
const element = () => new Element();
const commands: unknown[][] = [];
const closed: unknown[] = [];
let testProps: any = {};
const values: Record<string, any> = {
  View: class {}, div: element, svg: element, image: element, v_flex: element, h_flex: element,
  theme: () => new Proxy({}, { get: (_t, key) => String(key) }),
  props: () => testProps,
  closeOverlay(id: unknown) { closed.push(id); },
  run(...args: unknown[]) { commands.push(args); },
};
const Toast = new Function(...Object.keys(values), source.replace(/^import .*;\n/gm, "").replace("export default class Toast", "return class Toast"))(...Object.values(values));
function find(root: Element, id: string): Element {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return undefined as any;
}
function render() {
  commands.length = 0;
  closed.length = 0;
  testProps = { title: "Agent needs you", overlay: 7, action: { name: "agent.focus", args: { id: "a1" } } };
  return new Toast().render({});
}

test("the toast's × closes it without running its action", () => {
  const tree = render();
  const close = find(tree, "toast-close");
  expect(close).toBeDefined();
  let stopped = 0;
  close.calls.on_click[0]({}, { stop_propagation() { stopped++; } });
  expect(stopped).toBe(1);
  expect(closed).toEqual([7]);
  expect(commands).toEqual([]);
});

test("clicking the toast still runs its action and closes it", () => {
  const tree = render();
  tree.calls.on_click[0]({}, { stop_propagation() {} });
  expect(commands).toEqual([["agent.focus", { id: "a1" }]]);
  expect(closed).toEqual([7]);
});
