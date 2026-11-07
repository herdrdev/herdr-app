import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/source-control/graph.js", import.meta.url), "utf8"))
  .replace(/^import .*;\r?\n/gm, "").replace(/^export /gm, "");
const exported = `${source}; return { parseGraph, graphBadges, graphItems, graphIndex, renderGraph };`;
const { parseGraph, graphBadges, graphItems, graphIndex } = new Function(exported)();
const head = "a".repeat(40), older = "b".repeat(40);
const commit = (hash = head, refs = "HEAD -> main, origin/main") => ({ graph: "* ", hash, refs, subject: "Change", author: "Fixture", date: "2026-01-01", height: 22 });
const ref = (type: string, name: string, hash = head, symbolicTarget?: string) => ({
  type, name, hash, symbolicTarget, ref: `refs/${type === "branch" ? "heads" : type === "remote" ? "remotes" : "tags"}/${name}`,
});
const data = { head, branch: { name: "main", upstreamRef: "refs/remotes/origin/main" }, refs: [ref("branch", "main"), ref("remote", "origin/main")] };

test("graph parsing keeps full commit IDs and topology-only rows", () => {
  const rows = parseGraph(`* \x1f${head}\x1fHEAD -> main\x1f한글 change\x1fAuthor\x1f2026-01-01\r\n|\\\r\n| * \x1f${older}\x1f\x1fOlder\x1fAuthor\x1f2025-12-31\r\n`);
  expect(rows.map((row: any) => [row.graph, row.hash, row.height])).toEqual([["* ", head, 22], ["|\\", undefined, 6], ["| * ", older, 22]]);
  expect(rows[0].subject).toBe("한글 change");
});

test("current local, upstream remote, and tag badges coexist with grouped names", () => {
  const snapshot = { ...data, refs: [ref("branch", "feature"), ref("branch", "main"), ref("remote", "origin/feature"), ref("remote", "origin/main"),
    ref("remote", "origin/HEAD", head, "refs/remotes/origin/main"), ref("tag", "v2"), ref("tag", "v1")] };
  const result = graphBadges(commit(), snapshot);
  expect(result.head).toBe(true);
  expect(result.badges.map((badge: any) => [badge.icon, badge.label, badge.count])).toEqual([["target", "main", 2], ["cloud", "", 2], ["tag", "", 2]]);
  expect(result.badges[0].names).toEqual(["main", "feature"]);
  expect(result.badges[1].names).toEqual(["origin/main", "origin/feature"]);
  expect(result.badges[2].names).toEqual(["v1", "v2"]);
});

test("HEAD highlighting uses exact identity and ignores origin/HEAD decorations", () => {
  expect(graphBadges(commit(head, "origin/HEAD, origin/main"), { ...data, head: older }).head).toBe(false);
  expect(graphBadges(commit(head, "origin/HEAD, origin/main")).head).toBe(false);
  expect(graphBadges(commit(head, "HEAD -> main"), { ...data, head: older }).head).toBe(false);
  expect(graphBadges(commit(head, "HEAD -> main"), { ...data, head: null }).head).toBe(false);
  expect(graphBadges(commit(head.slice(0, 7)), data)).toMatchObject({ head: false, badges: [] });
  expect(graphBadges(commit(head, "HEAD"), { head, branch: null, refs: [] })).toMatchObject({ head: true, badges: [] });
});

test("a configured base ref receives the visible name without dropping other ref types", () => {
  const result = graphBadges(commit(), { head: older, branch: { name: "elsewhere" }, baseRef: "refs/heads/release", refs: [ref("remote", "origin/main"), ref("branch", "feature"), ref("branch", "release")] });
  expect(result.badges.map((badge: any) => [badge.icon, badge.label])).toEqual([["branch", "release"], ["cloud", ""]]);
  expect(result.badges[0].names).toEqual(["release", "feature"]);
});

const rows = [commit(), { graph: "|/", refs: "", height: 6 }, commit(older, "")];
const files = [{ path: "src/a.ts", status: "M" }, { path: "src/lib/b.ts", status: "A" }, { path: "README.md", status: "R", oldPath: "readme.txt" }];
const expanded = { [head]: { files } };

test("list and tree views have distinct rows and preserve file identity", () => {
  const list = graphItems(rows, { expanded, fileView: "list" });
  expect(list.filter((item: any) => item.kind === "file").map((item: any) => [item.name, item.depth])).toEqual(files.map(file => [file.path, 0]));
  const tree = graphItems(rows, { expanded, fileView: "tree" });
  expect(tree.filter((item: any) => ["file", "directory"].includes(item.kind)).map((item: any) => [item.kind, item.name, item.depth])).toEqual([
    ["directory", "src", 0], ["directory", "lib", 1], ["file", "b.ts", 2], ["file", "a.ts", 1], ["file", "README.md", 0],
  ]);
  expect(tree.find((item: any) => item.name === "README.md").file).toBe(files[2]);
  expect(new Set(tree.map((item: any) => item.key)).size).toBe(tree.length);
});

test("HEAD navigation includes expanded files, connector rows, and collapsed folders", () => {
  expect(graphIndex(rows, older)).toBe(2);
  expect(graphIndex(rows, older, { expanded, fileView: "list" })).toBe(5);
  expect(graphIndex(rows, older, { expanded, fileView: "tree" })).toBe(7);
  const options = { expanded, fileView: "tree", directories: { [head]: { src: true } } };
  expect(graphIndex(rows, older, options)).toBe(4);
  expect(graphItems(rows, options).some((item: any) => item.name === "b.ts")).toBe(false);
  expect(graphIndex(rows, "missing", options)).toBe(-1);
});

test("file-to-directory changes and prototype-like paths remain visible in a tree", () => {
  const items = graphItems([commit()], { fileView: "tree", directories: { [head]: {} }, expanded: { [head]: { files: [
    { path: "src", status: "D" }, { path: "src/new.ts", status: "A" }, { path: "toString/test.ts", status: "A" },
  ] } } });
  expect(items.filter((item: any) => item.kind === "file").map((item: any) => item.file.path).sort()).toEqual(["src", "src/new.ts", "toString/test.ts"]);
  expect(new Set(items.map((item: any) => item.key)).size).toBe(items.length);
});

test("loading, errors, and empty commits occupy one navigable message row", () => {
  for (const state of [{ loading: true }, { error: "Git unavailable" }, { files: [] }]) {
    const options = { expanded: { [head]: state } }, items = graphItems(rows, options);
    expect(items[1]).toMatchObject({ kind: "message", hash: head, height: 22 });
    expect(graphIndex(rows, older, options)).toBe(3);
  }
});

function renderer() {
  const lists: any[] = [];
  let layingOut = false;
  const node = (kind: string, asset?: string): any => {
    const value: any = { kind, asset, kids: [], ops: {} };
    let proxy: any;
    proxy = new Proxy(value, { get(target, name: string) {
      if (name in target) return target[name];
      return (...args: any[]) => {
        if (layingOut && name.startsWith("on_")) throw new Error("Virtual rows cannot register event handlers during layout");
        if (name === "child") target.kids.push(args[0]);
        else if (name === "children") target.kids.push(...args[0]);
        else if (name === "when") { if (args[0]) args[1](proxy); }
        else target.ops[name] = args;
        return proxy;
      };
    } });
    return proxy;
  };
  const globals = {
    div: () => node("div"), svg: (path: string) => node("svg", path), h_flex: () => node("row"), v_flex: () => node("column"),
    PathBuilder: { stroke() { return { move_to() { return this; }, line_to() { return this; }, build() { return []; } }; } },
    window: { paint_path: () => node("path") }, Scrollbar: { vertical: () => node("scrollbar") },
    v_virtual_list: (id: string, count: number, sizes: number[], key: Function, render: Function) => {
      const element = node("virtual");
      lists.push({ id, count, sizes, key, element, render: (range: any) => {
        layingOut = true;
        try { return render(range); } finally { layingOut = false; }
      } });
      return element;
    },
  };
  return { ...new Function(...Object.keys(globals), exported)(...Object.values(globals)), lists };
}

test("virtual rendering uses the shared item model and forwards file and folder actions", () => {
  const { renderGraph, lists } = renderer(), scroll = {}, actions: any[] = [];
  renderGraph(rows, {}, { data, expanded, fileView: "tree", scroll,
    onSelect: (row: any) => actions.push(["commit", row.hash]),
    onContext: (row: any) => actions.push(["context", row.hash]),
    onFile: (file: any, hash: string) => actions.push(["file", file, hash]),
    onToggleDirectory: (hash: string, path: string) => actions.push(["directory", path, hash]),
  });
  const list = lists[0], items = graphItems(rows, { expanded, fileView: "tree" });
  expect(list.count).toBe(items.length);
  expect(list.sizes).toEqual(items.map((item: any) => item.height));
  expect(list.element.ops.track_scroll).toEqual([scroll]);
  expect(list.key(7)).toBe(`commit:${older}`);
  const visible = list.render({ start: 1, end: 4 });
  expect(visible).toHaveLength(3);
  const cx = { stop_propagation() {} };
  const click = list.element.ops.on_item_click[0], secondary = list.element.ops.on_item_secondary_click[0];
  click(list.key(1), cx); click(list.key(3), cx); click(`commit:${older}`, cx);
  secondary(`commit:${head}`, {}, cx); secondary(list.key(3), {}, cx); click("removed-key", cx);
  expect(actions).toEqual([["directory", "src", head], ["file", files[1], head], ["commit", older], ["context", head]]);
  const commitRow = list.render({ start: 0, end: 1 })[0];
  const assets = (node: any): string[] => typeof node === "object" ? [node.asset, ...node.kids.flatMap(assets)].filter(Boolean) : [];
  expect(assets(commitRow)).toContain("mods/source-control/icons/target.svg");
  expect(assets(commitRow)).toContain("mods/source-control/icons/cloud.svg");
});
