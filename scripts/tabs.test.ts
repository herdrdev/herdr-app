import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/tabs/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Tabs", "class Tabs");

test("middle-click closes the pointed tab without selecting it", () => {
  const elements = new Map<string, any>();
  const calls: unknown[] = [];
  const makeElement = () => {
    const handlers: Record<string, Function> = {};
    const element: any = new Proxy({}, { get: (_, method: string) => (...args: any[]) => {
      if (method === "id") elements.set(args[0], handlers);
      if (method.startsWith("on_mouse_")) handlers[`${method}:${args[0]}`] = args[1];
      if (method === "when" && args[0]) args[1](element);
      return element;
    } });
    return element;
  };
  const tabs = [{ id: "active", name: "one", active: true }, { id: "inactive", name: "two", active: false }];
  const Tabs = new Function("View", "div", "svg", "image", "h_flex", "spaces", "panes", "run", "openOverlay", "theme", "window", "shared", "layout", "settings", `${source}; return Tabs;`)(
    class {}, makeElement, makeElement, makeElement, makeElement, () => [{ id: "space", active: true, tabs }], () => [],
    (name: string, args: unknown) => calls.push([name, args]), () => {}, () => ({}), () => ({}), () => [], () => ({}), () => ({}),
  );
  const view = new Tabs();
  view.panels = {};
  view.render();
  for (const tab of tabs) {
    let stopped = false;
    const handler = elements.get(`tab-${tab.id}`)?.["on_mouse_down:middle"];
    expect(handler).toBeFunction();
    handler({}, { stop_propagation() { stopped = true; } });
    expect(stopped).toBe(true);
  }
  expect(calls).toEqual([["tab.close", { id: "active" }], ["tab.close", { id: "inactive" }]]);
});
