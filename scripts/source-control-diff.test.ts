import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const mod = new URL("../defaults/mods/source-control/", import.meta.url);
const source = async (name: string) => (await readFile(new URL(name, mod), "utf8"))
  .replace(/^import .*;\r?\n/gm, "").replace(/^export /gm, "");
const [diffSource, panelSource] = await Promise.all([source("diff.js"), source("diff-ui.js")]);
const { parseDiff, selectedPatch } = new Function(`${diffSource}; return { parseDiff, selectedPatch };`)();
const patch = "diff --git a/example.txt b/example.txt\nindex 1234567..abcdef0 100644\n--- a/example.txt\n+++ b/example.txt\n@@ -1,2 +1,2 @@\n first\n-old\n+new\n";

async function fixture(props = {}) {
  let deferred = false, notifications = 0;
  const lists: any[] = [], jobs: Promise<unknown>[] = [];
  const element = () => {
    const events: Record<string, Function> = {};
    const node: any = new Proxy({ events }, { get(target, key) {
      if (key === "events") return events;
      return (...args: any[]) => {
        // GPUI forbids handler registration during virtual-list layout.
        if (String(key).startsWith("on_")) {
          if (deferred) throw new Error(`Cannot register ${String(key)} inside a virtual-list renderer`);
          events[String(key)] = args[0];
        } else if (key === "when" && args[0]) args[1](node);
        return node;
      };
    } });
    return node;
  };
  const globals = {
    div: element, h_flex: element, v_flex: element,
    Scrollbar: { vertical: element }, theme: () => new Proxy({}, { get: () => "#808080" }),
    v_virtual_list: (_id: string, count: number, _size: number, key: Function, render: Function) => {
      const node = element(); lists.push({ node, count, key, render }); return node;
    },
    parseDiff, selectedPatch, createGit: () => ({ diff: async () => patch, commitDiff: async () => patch }),
  };
  const DiffPanel = new Function(...Object.keys(globals), `${panelSource}; return DiffPanel;`)(...Object.values(globals));
  const cx: any = { notify() { notifications++; }, spawn(fn: Function) { jobs.push(Promise.resolve().then(() => fn(cx))); } };
  const panel = new DiffPanel({ root: "/fixture", file: { path: "example.txt" }, group: "changes", ...props }, cx);
  await Promise.all(jobs); panel.render();
  const list = lists.at(-1);
  return { panel, notifications: () => notifications,
    click(index: number) {
      expect(typeof list.node.events.on_item_click).toBe("function");
      list.node.events.on_item_click(list.key(index), cx);
    },
    layout() {
      deferred = true;
      try { return list.render({ start: 0, end: list.count }, cx); }
      finally { deferred = false; }
    },
  };
}

test("mutable diff rows render without registering handlers during deferred layout", async () => {
  const f = await fixture();
  expect(f.layout()).toHaveLength(8);
});

test("diff list clicks select and clear changed lines, ignoring headers and context", async () => {
  const f = await fixture();
  f.click(0); f.click(5); expect([...f.panel.selected]).toEqual([]);
  f.click(7); expect([...f.panel.selected]).toEqual([2]);
  f.click(6); expect([...f.panel.selected].sort()).toEqual([1, 2]);
  f.click(7); expect([...f.panel.selected]).toEqual([1]);
});

test("diff list clicks select and clear every changed line in a hunk", async () => {
  const f = await fixture();
  f.click(4); expect([...f.panel.selected]).toEqual([1, 2]);
  f.click(4); expect([...f.panel.selected]).toEqual([]);
});

test("diff list clicks cannot select busy, failed, or read-only diffs", async () => {
  for (const state of [{ busy: true }, { error: "unavailable" }, { props: { hash: "1234567" } }, { props: { on: "remote" } }, { props: { group: "conflicts" } }]) {
    const f = await fixture(state.props);
    Object.assign(f.panel, Object.fromEntries(Object.entries(state).filter(([key]) => key !== "props")));
    f.click(4); f.click(7); expect([...f.panel.selected]).toEqual([]);
  }
});

test("a queued diff click cannot select rows after refresh", async () => {
  const f = await fixture();
  f.panel.setPatch(patch.replace("+new\n", "+replacement\n"));
  const notifications = f.notifications();
  f.click(4); f.click(7);
  expect([...f.panel.selected]).toEqual([]);
  expect(f.notifications()).toBe(notifications);
});
