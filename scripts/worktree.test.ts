import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/worktree/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Worktree", "class Worktree");

// Execute conditional builders and retain text, without native GPUI layout.
function element() {
  const node = { id: "", children: [] as any[], handlers: {} as Record<string, (...args: any[]) => any> };
  const el: any = new Proxy(node, { get: (_, method) => {
    if (method === "node") return node;
    return (...args: any[]) => {
      if (method === "id") node.id = args[0];
      if (method === "on_click" || method === "on_key_down") node.handlers[method] = args[0];
      if (method === "child") node.children.push(args[0]?.node ?? args[0]);
      if (method === "children") node.children.push(...args[0].map((child: any) => child?.node ?? child));
      if (method === "when" && args[0]) args[1](el);
      return el;
    };
  } });
  return el;
}
function textOf(node: any): string {
  return typeof node === "string" ? node : (node?.children ?? []).map(textOf).join(" ");
}
function rowsOf(node: any): any[] {
  return node?.id?.match(/^wt-\d+$/) ? [node] : (node?.children ?? []).flatMap(rowsOf);
}

function harness(focusedMachine: string | null = null) {
  let spaceList: any[] = [];
  let paneList: any[] = [];
  const reads = { spaces: 0, panes: 0 };
  const writes: unknown[] = [];
  const destinations: (string | null)[] = [];
  const executions: any[] = [];
  const properties: any = { overlay: "overlay" };
  const closed: string[] = [];
  const globals = {
    View: class {}, div: element, v_flex: element, h_flex: element,
    Input: { new: element }, Scrollbar: { vertical: element }, theme: () => ({}),
    spaces: () => { reads.spaces++; return structuredClone(spaceList); },
    panes: () => { reads.panes++; return structuredClone(paneList); },
    props: () => properties, closeOverlay: (id: string) => { closed.push(id); },
    run: (command: string, args: any) => {
      writes.push([command, args]);
      if (command === "space.new") destinations.push(args.machine === "" ? null : args.machine ?? focusedMachine);
    },
    exec: async (command: string, args: string[], options: any) => {
      executions.push({ command, args, ...options });
      return { code: 0, stdout: "", stderr: "" };
    },
  };
  const Worktree = new Function(...Object.keys(globals), `${source}; return Worktree;`)(...Object.values(globals));
  const worktree = new Worktree();
  Object.assign(worktree, { opening: true, text: "", at: 0, repo: { name: "repo" }, userHome: "/home/user" });
  return { worktree, reads, writes, destinations, executions, properties, closed, snapshot: (spaces: any[], panes: any[]) => { spaceList = spaces; paneList = panes; } };
}

function find(node: any, id: string): any {
  return node?.id === id ? node : (node?.children ?? []).map((child: any) => find(child, id)).find(Boolean);
}

function removalHarness() {
  const h = harness();
  h.properties.action = "remove";
  Object.assign(h.worktree, {
    opening: false, repo: { top: "/repo/worktree", main: "/repo", linked: true },
    dirty: false, unpushed: [], busy: false, error: "",
  });
  return h;
}

test("remove confirmation offers clickable actions with the existing keyboard shortcuts", () => {
  const { worktree, closed } = removalHarness();
  let started = 0, stopped = 0;
  const cx = { spawn: () => { started++; }, stop_propagation: () => { stopped++; } };
  const tree = worktree.render(cx).node;
  const remove = find(tree, "wt-remove"), cancel = find(tree, "wt-cancel");
  expect(textOf(remove)).toBe("Remove ↵");
  expect(textOf(cancel)).toBe("Cancel Esc");
  remove.handlers.on_click({}, cx);
  expect(started).toBe(1);
  tree.handlers.on_key_down({ key: "enter" }, cx);
  expect(started).toBe(2);
  cancel.handlers.on_click({}, cx);
  tree.handlers.on_key_down({ key: "escape" }, cx);
  expect(closed).toEqual(["overlay", "overlay"]);
  expect(stopped).toBe(4);
});

for (const state of ["loading", "unavailable", "main checkout", "busy"]) {
  test(`remove clicks and Enter cannot submit while ${state}`, () => {
    const { worktree, closed } = removalHarness();
    if (state === "loading") worktree.repo = undefined;
    if (state === "unavailable") worktree.repo = null;
    if (state === "main checkout") worktree.repo.linked = false;
    if (state === "busy") worktree.busy = true;
    let started = 0;
    const cx = { spawn: () => { started++; }, stop_propagation() {} };
    const tree = worktree.render(cx).node;
    expect(find(tree, "wt-remove").handlers.on_click).toBeUndefined();
    tree.handlers.on_key_down({ key: "enter" }, cx);
    expect(started).toBe(0);
    find(tree, "wt-cancel").handlers.on_click({}, cx);
    expect(closed).toEqual(["overlay"]);
  });
}

test("a stale Remove click rechecks the busy state and preserves removal warnings", () => {
  const { worktree } = removalHarness();
  worktree.dirty = true;
  worktree.unpushed = ["submodule"];
  let started = 0;
  const cx = { spawn: () => { started++; }, stop_propagation() {} };
  const tree = worktree.render(cx).node;
  expect(textOf(tree)).toContain("It has uncommitted changes: they will be lost.");
  expect(textOf(tree)).toContain("Submodule submodule has commits that aren't pushed anywhere: they will be lost.");
  worktree.busy = true;
  find(tree, "wt-remove").handlers.on_click({}, cx);
  expect(started).toBe(0);
});

test("new and open worktree footers are unchanged", () => {
  const { worktree, properties } = removalHarness();
  properties.action = "new";
  const tree = worktree.render({}).node;
  expect(find(tree, "wt-remove")).toBeUndefined();
  expect(find(tree, "wt-cancel")).toBeUndefined();
  expect(textOf(tree)).toContain("↵ create · esc cancel");
  properties.action = "open";
  worktree.opening = true;
  worktree.list = [];
  expect(textOf(worktree.render({}).node)).toContain("↑↓ choose · ↵ open · esc close");
});

test("open render reads spaces and panes once and refreshes both on the next render", () => {
  const { worktree, reads, snapshot } = harness();
  worktree.list = Array.from({ length: 80 }, (_, i) => ({ path: `/repo/wt-${i}`, branch: `branch-${i}`, at: 0 }));
  const spaces = worktree.list.map((c: any, i: number) => ({ id: `s${i}`, tabs: [{ id: `tab${i}` }] }));
  const panes = worktree.list.map((c: any, i: number) => ({ tab: `tab${i}`, cwd: `${c.path}/src` }));
  snapshot(spaces, panes);
  const first = rowsOf(worktree.render({}).node);
  expect(reads).toEqual({ spaces: 1, panes: 1 });
  expect(first).toHaveLength(80);
  for (let i = 0; i < first.length; i++) {
    expect(textOf(first[i])).toContain(`branch-${i}`);
    expect(textOf(first[i])).toContain(" open ");
  }
  snapshot(spaces.slice(0, -1), panes.map((p: any, i: number) => i === 0 ? { ...p, cwd: "/elsewhere" } : p));
  const second = rowsOf(worktree.render({}).node);
  expect(reads).toEqual({ spaces: 2, panes: 2 });
  expect(textOf(second[0])).not.toContain(" open ");
  expect(textOf(second[79])).not.toContain(" open ");
  expect(textOf(second[1])).toContain(" open ");
});

test("open clicks read fresh snapshots once and select an existing space or create one", () => {
  const { worktree, reads, writes, snapshot } = harness();
  const checkout = { path: "/repo/worktree", branch: "branch", at: 0 };
  worktree.list = [checkout];
  snapshot([], []);
  worktree.render({});
  snapshot([{ id: "existing", tabs: [{ id: "tab" }] }], [{ tab: "tab", cwd: checkout.path }]);
  worktree.open(checkout);
  expect(reads).toEqual({ spaces: 2, panes: 2 });
  expect(writes).toEqual([["space.select", { id: "existing" }]]);
  snapshot([], []);
  worktree.open(checkout);
  expect(reads).toEqual({ spaces: 3, panes: 3 });
  expect(writes[1]).toEqual(["space.new", { folder: checkout.path, machine: "" }]);
  worktree.open(undefined);
  expect(reads).toEqual({ spaces: 3, panes: 3 });
});

for (const on of [undefined, null, "server"]) {
  for (const focused of [null, "server"]) {
    test(`create and open stay on ${on ?? "local"} while ${focused ?? "local"} is focused`, async () => {
      const { worktree, destinations, executions } = harness(focused);
      Object.assign(worktree, {
        on, text: "feature", repo: { name: "repo", main: "/repo", top: "/repo" }, list: [],
      });
      expect(await worktree.create()).toBe("");
      expect(executions).toHaveLength(2);
      expect(executions.every(e => (e.on ?? null) === (on ?? null))).toBe(true);
      expect(executions[1].args).toEqual(["worktree", "add", "/home/user/.herdr/worktrees/repo/feature", "feature"]);
      worktree.open({ path: "/repo/existing" });
      expect(destinations).toEqual([on ?? null, on ?? null]);
    });
  }
}

test("space matching keeps deepest checkout, machine, live-folder and fallback rules", () => {
  const { worktree, reads, writes, snapshot } = harness();
  const paths = ["C:/repo", "C:/repo/nested", "C:/repo/fallback", "C:/repo-other"];
  worktree.list = paths.map(path => ({ path }));
  snapshot([
    { id: "remote", machine: "server", folder: paths[0] },
    { id: "nested", folder: paths[0], tabs: [{ id: "tab" }] },
    { id: "fallback", folder: paths[2] },
    { id: "other", folder: paths[3] },
  ], [
    { tab: "tab" },
    { tab: "tab", cwd: "C:\\repo\\nested\\src" },
    { tab: "tab", cwd: paths[0] },
  ]);
  for (const checkout of worktree.list) worktree.open(checkout);
  expect(writes).toEqual([
    ["space.new", { folder: paths[0], machine: "" }],
    ["space.select", { id: "nested" }],
    ["space.select", { id: "fallback" }],
    ["space.select", { id: "other" }],
  ]);
  worktree.on = "server";
  worktree.open(worktree.list[0]);
  expect(writes[4]).toEqual(["space.select", { id: "remote" }]);
  expect(reads).toEqual({ spaces: 5, panes: 5 });
});
