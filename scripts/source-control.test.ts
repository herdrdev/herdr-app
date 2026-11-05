import { test, expect } from "bun:test";
import { readFile, writeFile, mkdtemp, mkdir, rm, rename } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";

const mod = new URL("../defaults/mods/source-control/", import.meta.url);
const source = async (name: string) => (await readFile(new URL(name, mod), "utf8")).replace(/^import .*;\r?\n/gm, "").replace(/^export /gm, "");
const [gitSource, diffSource, mergeSource] = await Promise.all([source("git.js"), source("diff.js"), source("merge-ui.js")]);
const { parseDiff, selectedPatch } = new Function(`${diffSource}; return { parseDiff, selectedPatch };`)();
const panelSource = (await source("panel.js")).replace("default class SourceControl", "class SourceControl");
const SourceControl = new Function("View", `${panelSource}; return SourceControl;`)(class {});
type Result = { code: number; stdout: string; stderr: string };

test("plain Enter keeps a multiline message; only the commit shortcut submits", () => {
  const handlers = new Map<string, Function>();
  const input = { set_auto_grow() {}, on(name: string, handler: Function) { handlers.set(name, handler); } };
  const Panel = new Function("View", "TextareaState", "VirtualListScrollHandle", "shared", `${panelSource}; return SourceControl;`)(class {}, { new: () => input }, { new: () => ({}) }, () => undefined);
  const panel = new Panel(); let commits = 0, confirmations = 0;
  panel.refresh = () => {}; panel.commit = () => { commits++; panel.dialog = {}; }; panel.submitDialog = () => { confirmations++; };
  const cx = { focus_handle() {}, timer: { every() {} } };
  panel.init({}, cx);
  handlers.get("submit")!({ secondary: false, shift: false }, cx);
  handlers.get("submit")!({ secondary: false, shift: true }, cx);
  expect(commits).toBe(0);
  handlers.get("submit")!({ secondary: true, shift: false }, cx);
  expect(commits).toBe(1); expect(confirmations).toBe(0);
  handlers.get("submit")!({ secondary: true, shift: false }, cx);
  expect(commits).toBe(1); expect(confirmations).toBe(1);
});

function controller() {
  const panel = new SourceControl();
  panel.chosen = { cwd: "/fixture", on: null }; panel.context = panel.key(); panel.message = "message";
  panel.data = { root: "/fixture", head: "head", groups: { staged: [], changes: [], conflicts: [] }, remotes: ["origin"], operation: null,
    branch: { name: "main", upstream: "origin/main", remote: "origin", ahead: 0, behind: 0 } };
  return panel;
}

test("the primary action follows Commit, Push, Sync, and Publish repository states", () => {
  const panel = controller();
  expect(panel.primary()).toMatchObject({ action: "commit", enabled: false });
  panel.data.groups.staged = [{ path: "staged.txt" }];
  expect(panel.primary()).toMatchObject({ action: "commit", enabled: true });
  panel.data.groups.staged = []; panel.data.groups.changes = [{ path: "working.txt" }];
  expect(panel.primary()).toMatchObject({ action: "commit", enabled: true });
  panel.message = ""; expect(panel.primary().enabled).toBe(false); panel.message = "message";
  panel.data.groups.changes = []; panel.data.branch.ahead = 2;
  expect(panel.primary()).toMatchObject({ action: "push", enabled: true });
  panel.data.branch.behind = 1;
  expect(panel.primary()).toMatchObject({ action: "sync", enabled: true });
  panel.data.branch.upstream = "";
  expect(panel.primary()).toMatchObject({ action: "publish", enabled: true });
  panel.data.remotes = []; expect(panel.primary().enabled).toBe(false);
  panel.data.branch = null; expect(panel.primary().enabled).toBe(false);
  panel.data.groups.conflicts = [{ path: "conflict.txt" }]; expect(panel.primary().enabled).toBe(false);
  panel.data.groups.conflicts = []; panel.data.operation = "merge";
  expect(panel.primary()).toMatchObject({ action: "continue", enabled: true });
  panel.busy = "operation"; expect(panel.primary().enabled).toBe(false);
  panel.busy = null; panel.chosen.cwd = "/different"; expect(panel.primary().enabled).toBe(false);
});

test("dialogs cannot submit after the repository, HEAD, branch, or operation changes", () => {
  for (const change of [(p: any) => p.chosen.cwd = "/other", (p: any) => p.data.head = "new head", (p: any) => p.data.branch.name = "other", (p: any) => p.data.operation = "rebase"]) {
    const panel = controller(); let submitted = false;
    panel.dialog = { context: panel.key(), stamp: panel.stamp(), fields: [], submit() { submitted = true; } };
    change(panel); panel.submitDialog({ notify() {} });
    expect(submitted).toBe(false); expect(panel.dialog).toBeNull(); expect(panel.note).toContain("changed");
  }
});

test("commit then push stops if the active branch changes after committing", async () => {
  const panel = controller(), calls: string[] = [], jobs: Promise<unknown>[] = [];
  const snapshot = structuredClone(panel.data);
  let reads = 0, inputValue = "message";
  panel.repo = {
    read: async () => ++reads === 1 ? structuredClone(snapshot) : { ...snapshot, head: "committed", branch: { ...snapshot.branch, name: "other" } },
    run: async (action: string) => { calls.push(action); },
  };
  panel.refresh = () => {}; panel.input = { set_value(value: string) { inputValue = value; } };
  const cx: any = { notify() {}, spawn(fn: Function) { jobs.push(Promise.resolve().then(() => fn(cx))); } };
  panel.perform("Commit", "commit", { message: "message" }, cx, "push");
  while (jobs.length) await jobs.shift();
  expect(calls).toEqual(["commit"]);
  expect(panel.note).toContain("Commit completed; push failed");
  expect(panel.message).toBe(""); expect(inputValue).toBe("");
});

function renderedPanel(savedHeight: unknown, platform = "windows", zoom = 1) {
  const values = new Map<string, unknown>([["source-control.upper-height", savedHeight]]), writes: unknown[][] = [];
  let notifications = 0;
  const element = (kind: string, id?: string): any => {
    const node = { kind, attrs: { id: [id] } as Record<string, any[]>, items: [] as any[] };
    const builder: any = new Proxy(node, { get(target: any, name: string) {
      if (name in target) return target[name];
      return (...args: any[]) => {
        if (name === "child") node.items.push(args[0]);
        else if (name === "children") node.items.push(...args[0]);
        else if (name === "when") { if (args[0]) return args[1](builder); }
        else node.attrs[name] = args;
        return builder;
      };
    } });
    return builder;
  };
  const scrolls: any[] = [], jobs: Promise<unknown>[] = [], diffs: any[] = [];
  let panel: any, repository: any;
  const globals = { View: class {}, div: () => element("div"), svg: (path: string) => element("svg", path), h_flex: () => element("row"), v_flex: () => element("column"),
    v_resizable: (id: string) => element("vertical-split", id), resizable_panel: () => element("panel"),
    Textarea: { new: () => element("textarea") }, theme: () => ({}), spaces: () => [{ id: "fixture", active: true }],
    window: () => ({ platform }), settings: () => ({ zoom }), shared: (key: string) => values.get(key),
    setShared: (key: string, value: unknown) => { values.set(key, value); writes.push([key, value]); },
    renderGraph: (_rows: any, _theme: any, options: any) => element("graph").props(options),
    graphIndex: (rows: any[], hash: string) => rows.findIndex(row => row.hash === hash),
    parseGraph: (text: string) => JSON.parse(text),
    openOverlay: (options: any) => diffs.push(options),
    createGit: () => repository || panel.repo,
  };
  const Panel = new Function(...Object.keys(globals), `${panelSource}; return SourceControl;`)(...Object.values(globals));
  panel = Object.assign(new Panel(), controller(), { collapsed: {}, history: [], limit: 200, input: { set_value() {} }, expanded: {}, directories: {}, fileView: "list",
    graphFilter: { mode: "auto", refs: [] }, graphScroll: { scroll_to_item(...args: any[]) { scrolls.push(args); } }, handle: { focus() {} },
  });
  Object.assign(panel.data, { refs: [], graphRefs: [], graphIncludesHead: true });
  const cx: any = { notify() { notifications++; }, stop_propagation() {}, spawn(fn: Function) { jobs.push(Promise.resolve().then(() => fn(cx))); } };
  const nodes = (node: any): any[] => typeof node === "object" && node ? [node, ...node.items.flatMap(nodes)] : [];
  const render = () => nodes(panel.render(cx));
  const measure = (height = 700) => {
    render()[0].attrs.on_size_change[0]({ width: 368, height }, cx);
    return render();
  };
  return { panel, cx, values, writes, render, measure, scrolls, diffs, setRepository: (value: any) => { repository = value; }, drain: async () => { while (jobs.length) await jobs.shift(); }, notifications: () => notifications };
}

test("Source Control split restores safe heights through native panels on both platforms", () => {
  for (const platform of ["windows", "macos"]) {
    for (const [saved, expected] of [[undefined, 207], [null, 207], ["360", 207], [NaN, 207], [Infinity, 207], [80, 160], [360.5, 360.5]]) {
      const view = renderedPanel(saved, platform), nodes = view.measure();
      const splits = nodes.filter(node => node.kind === "vertical-split");
      expect(splits).toHaveLength(1);
      expect(splits[0].items.map((node: any) => node.kind)).toEqual(["panel", "panel"]);
      expect(splits[0].items[0].attrs.size).toEqual([expected]);
      expect(splits[0].items[0].attrs.size_range).toEqual([160, 582]);
      expect(splits[0].items[1].attrs.size_range).toEqual([80]);
      expect(nodes.some(node => node.kind === "graph")).toBe(true);
      expect(view.writes).toEqual([]);
    }
  }
});

test("Source Control split stores unzoomed heights and restores them at a different zoom", () => {
  const view = renderedPanel(undefined, "windows", 1.2), split = view.measure().find(node => node.kind === "vertical-split");
  expect(split.items[0].attrs.size[0]).toBeCloseTo(248.4);
  expect(split.items[0].attrs.size_range[0]).toBeCloseTo(192);
  expect(split.items[0].attrs.size_range[1]).toBeCloseTo(558.4);
  expect(split.items[1].attrs.size_range[0]).toBeCloseTo(96);
  const notified = view.notifications();
  split.attrs.on_resize[0]([377.4, 277], view.cx);
  expect(view.writes).toEqual([["source-control.upper-height", 314.5]]);
  expect(view.notifications()).toBe(notified + 1);
  const reopened = renderedPanel(view.values.get("source-control.upper-height"), "macos", 1.5);
  const restored = reopened.measure().find(node => node.kind === "vertical-split");
  expect(restored.items[0].attrs.size[0]).toBeCloseTo(471.75);
  expect(reopened.writes).toEqual([]);
});

test("Source Control split measures a smaller window before mounting a saved height", () => {
  const view = renderedPanel(582);
  expect(view.render().some(node => node.kind === "vertical-split")).toBe(false);
  expect(view.writes).toEqual([]);
  const split = view.measure(400).find(node => node.kind === "vertical-split");
  expect(split.items[0].attrs.size).toEqual([282]);
  expect(split.items[0].attrs.size_range).toEqual([160, 282]);
  expect(split.items[1].attrs.size_range).toEqual([80]);
  expect(view.values.get("source-control.upper-height")).toBe(582);
  expect(view.writes).toEqual([]);
  split.attrs.on_resize[0]([200, 162], view.cx);
  expect(view.writes).toEqual([["source-control.upper-height", 200]]);
});

test("Source Control split preserves its size and identity while Graph is collapsed", () => {
  const view = renderedPanel(320), initial = view.measure();
  const split = initial.find(node => node.kind === "vertical-split");
  const toggle = (nodes: any[]) => nodes.find(node => node.attrs.id[0] === "graph-heading").attrs.on_click[0]({}, view.cx);
  toggle(initial);
  const collapsed = view.render(), hiddenSplit = collapsed.find(node => node.kind === "vertical-split");
  expect(collapsed.some(node => node.kind === "graph")).toBe(false);
  expect(hiddenSplit.attrs.id).toEqual(split.attrs.id);
  expect(hiddenSplit.items).toHaveLength(2);
  expect(hiddenSplit.items[0].attrs.size).toEqual([320]);
  toggle(collapsed);
  const expanded = view.render(), restored = expanded.find(node => node.kind === "vertical-split");
  expect(expanded.some(node => node.kind === "graph")).toBe(true);
  expect(restored.attrs.id).toEqual(split.attrs.id);
  expect(restored.items[0].attrs.size).toEqual([320]);
  expect(view.values.get("source-control.upper-height")).toBe(320);
  expect(view.writes).toEqual([]);
});

test("Graph toolbar follows VS Code order and dispatches fetch-all, pull, push, and publish", () => {
  const view = renderedPanel(undefined), calls: any[] = [];
  view.panel.network = (...args: any[]) => calls.push([args[0], args[2]]);
  const ids = ["graph-filter", "graph-current", "fetch", "graph-pull", "push", "graph-refresh", "graph-more-actions"];
  const buttons = view.measure().filter(node => ids.includes(node.attrs.id[0]));
  expect(buttons.map(node => node.attrs.id[0])).toEqual(ids);
  const click = (id: string) => view.render().find(node => node.attrs.id[0] === id).attrs.on_click[0]({ stop_propagation() {} }, view.cx);
  click("fetch"); click("graph-pull"); click("push");
  expect(calls).toEqual([["fetch", { all: true }], ["pull", undefined], ["push", undefined]]);
  view.panel.data.graphIncludesHead = false; click("graph-pull"); click("push");
  expect(calls).toHaveLength(3);
  view.panel.data.branch.upstream = ""; click("push");
  expect(calls.at(-1)[0]).toBe("publish");
  expect(view.render().find(node => node.attrs.id[0] === "push").attrs.tooltip).toEqual(["Publish Branch…"]);
});

test("Graph reference selection stays open for multiple refs and saves only on Apply", () => {
  const view = renderedPanel(undefined), { panel, cx } = view;
  panel.refresh = () => {};
  panel.data.refs = [
    { ref: "refs/heads/main", name: "main", hash: "head", type: "branch" },
    { ref: "refs/remotes/origin/main", name: "origin/main", hash: "head", type: "remote" },
    { ref: "refs/remotes/origin/HEAD", name: "origin/HEAD", hash: "head", type: "remote", symbolicTarget: "refs/remotes/origin/main" },
  ];
  panel.data.graphRefs = ["refs/heads/main"];
  panel.graphFilterMenu(cx);
  const dialog = panel.dialog;
  expect(dialog.items.map((item: any) => item.label)).toEqual(["Auto", "All", "Apply Selected References", "main", "origin/main"]);
  const remote = view.render().find(node => node.attrs.id[0] === "menu-4");
  remote.attrs.on_click[0]({}, cx);
  expect(panel.dialog).toBe(dialog); expect(dialog.items[4].checked).toBe(true);
  expect(view.writes).toEqual([]);
  view.render().find(node => node.attrs.id[0] === "menu-2").attrs.on_click[0]({}, cx);
  expect(panel.dialog).toBeNull();
  expect(panel.graphFilter).toEqual({ mode: "refs", refs: ["refs/heads/main", "refs/remotes/origin/main"] });
  expect(view.values.get("source-control.graph-filters")[panel.key()]).toEqual(panel.graphFilter);
  expect(view.scrolls).toEqual([[0]]);
});

test("Graph filters follow the repository root across terminal subdirectories and separate remote machines", async () => {
  const view = renderedPanel(undefined), { panel, cx } = view, snapshot = { ...panel.data, graph: "[]" };
  const repo = { read: async () => structuredClone(snapshot) };
  view.setRepository(repo);
  panel.chosen.cwd = "/fixture/src";
  panel.repo = repo; panel.refresh(cx); await view.drain();
  const filter = { mode: "refs", refs: ["refs/heads/topic"] };
  panel.setGraphFilter(filter, cx); await view.drain();
  expect(view.values.get("source-control.graph-filters")).toEqual({ [JSON.stringify({ cwd: "/fixture", on: null })]: filter });
  panel.chosen.cwd = "/fixture/tests"; panel.repo = repo; panel.refresh(cx); await view.drain();
  expect(panel.graphFilter).toEqual(filter);
  panel.chosen.on = "other-machine"; panel.repo = repo; panel.refresh(cx); await view.drain();
  expect(panel.graphFilter).toEqual({ mode: "auto", refs: [] });
});

test("Go to Current History Item loads beyond the first page and avoids stale repository results", async () => {
  for (const stale of [false, true]) {
    const view = renderedPanel(undefined), { panel, cx } = view, requests: number[] = [];
    const snapshot = structuredClone(panel.data);
    panel.history = Array.from({ length: 200 }, (_, index) => ({ hash: String(index) }));
    panel.collapsed.graph = true;
    panel.repo = { read: async (limit: number) => { requests.push(limit); if (stale) panel.chosen.cwd = "/other"; return { ...snapshot, graph: JSON.stringify([...panel.history, { hash: "head" }]) }; } };
    panel.revealCurrent(cx); await view.drain();
    expect(requests).toEqual([400]);
    expect(view.scrolls).toEqual(stale ? [] : [[200, "center"]]);
    if (!stale) { expect(panel.selected).toBe("head"); expect(panel.collapsed.graph).toBe(false); }
    expect(panel.loading).toBe(false);
  }
});

test("Graph commits expand inline, open historical diffs, and switch list/tree without Git mutations", async () => {
  const view = renderedPanel(undefined), { panel, cx } = view;
  const file = { path: "src/app.js", status: "M" };
  let reads = 0;
  panel.repo = { showCommit: async () => { reads++; return { files: [file] }; } };
  panel.toggleCommit({ hash: "head" }, cx); await view.drain();
  expect(panel.expanded.head.files).toEqual([file]); expect(panel.dialog).toBeUndefined();
  const graph = view.measure().find(node => node.kind === "graph").attrs.props[0];
  graph.onFile(file, "head", cx);
  expect(view.diffs[0].props).toMatchObject({ file, group: "history", hash: "head" });
  for (const path of ["src", "toString", "__proto__"]) {
    graph.onToggleDirectory("head", path, cx); expect(panel.directories.head[path]).toBe(true);
    graph.onToggleDirectory("head", path, cx); expect(panel.directories.head[path]).toBe(false);
  }
  panel.graphViewMenu(cx); panel.dialog.items[1].run(cx);
  expect(panel.fileView).toBe("tree"); expect(view.values.get("source-control.graph-file-view")).toBe("tree");
  expect(panel.expanded.head.files).toEqual([file]); expect(reads).toBe(1);
  panel.toggleCommit({ hash: "head" }, cx); expect(panel.expanded.head).toBeUndefined();
});

async function fixture(body: (f: any) => Promise<void>) {
  const sandbox = await mkdtemp(join(tmpdir(), "herdr-source-control-")), root = join(sandbox, "work");
  const calls: string[][] = [];
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_")));
  Object.assign(env, {
    GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(sandbox, "gitconfig"), GIT_TERMINAL_PROMPT: "0", GIT_ALLOW_PROTOCOL: "file",
    GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid",
  });
  const inside = (path: string) => {
    const child = relative(sandbox, resolve(path));
    if (isAbsolute(child) || child === ".." || child.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) || !child) throw new Error("Expected a path inside the fixture");
  };
  const run = async (command: string, args: string[], options: { cwd: string }, input?: string): Promise<Result> => {
    expect(command).toBe("git"); inside(options.cwd); calls.push(args);
    const p = Bun.spawn([command, ...args], { cwd: options.cwd, env, windowsHide: true, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
    if (input !== undefined) p.stdin.write(input);
    p.stdin.end();
    const [code, stdout, stderr] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    return { code, stdout, stderr };
  };
  const git = async (args: string[], cwd = root, input?: string) => {
    const result = await run("git", args, { cwd }, input);
    if (result.code !== 0) throw new Error(`${args.join(" ")}: ${result.stderr || result.stdout}`);
    return result.stdout;
  };
  const reader = async (path: string) => {
    inside(path);
    try { return await readFile(path, "utf8"); }
    catch (error: any) { if (error.code === "ENOENT") return null; throw error; }
  };
  const createGit = new Function("exec", "readTextFile", `${gitSource}; return createGit;`)(run, reader);
  try {
    await mkdir(root); await writeFile(join(sandbox, "gitconfig"), "");
    await git(["init", "-b", "main"]); await git(["config", "core.autocrlf", "false"]);
    await body({ sandbox, root, git, run, reader, calls, createGit, repo: createGit(root),
      write: (name: string, text: string, cwd = root) => writeFile(join(cwd, name), text),
      patch: async (text: string) => { const path = join(sandbox, "selection.patch"); await writeFile(path, text); return path; },
    });
  } finally {
    // Only remove the directory this fixture created, never a repository supplied by a caller.
    expect(dirname(resolve(sandbox))).toBe(resolve(tmpdir()));
    await rm(sandbox, { recursive: true, force: true });
  }
}

test("Source Control unstages an unborn index and commits only staged content", () => fixture(async f => {
  const name = "literal [x] 한글.txt";
  await f.write(name, "staged\n"); await f.repo.run("stage", { file: name });
  await f.repo.run("unstage", { all: true });
  expect(await f.git(["ls-files"])).toBe("");
  expect(await f.reader(join(f.root, name))).toBe("staged\n");
  await f.repo.run("stage", { file: name });
  await f.write(name, "working only\n"); await f.write("untracked.txt", "not committed\n");
  await f.repo.run("commit", { message: "staged only" });
  expect(await f.git(["show", `HEAD:${name}`])).toBe("staged\n");
  expect(await f.git(["ls-tree", "--name-only", "HEAD"])).not.toContain("untracked.txt");
  const state = await f.repo.read(1);
  expect(state.groups.staged).toHaveLength(0);
  expect(state.groups.changes.map((file: any) => file.path).sort()).toEqual([name, "untracked.txt"].sort());
  await expect(f.repo.run("stage", { file: "../outside.txt" })).rejects.toThrow();
}), 120000);

test("a branch and tag sharing a name do not change the branch rename target", () => fixture(async f => {
  await f.write("sample.txt", "base\n"); await f.git(["add", "."]); await f.git(["commit", "-m", "base"]); await f.git(["tag", "main"]);
  const state = await f.repo.read(1);
  expect(state.branch.name).toBe("main");
  expect(state.refs).toContainEqual(expect.objectContaining({ ref: "refs/heads/main", name: "main", type: "branch" }));
  expect(state.refs).toContainEqual(expect.objectContaining({ ref: "refs/tags/main", name: "main", type: "tag" }));
  await f.repo.run("branchRename", { name: state.branch.name, newName: "renamed" });
  expect((await f.git(["symbolic-ref", "HEAD"])).trim()).toBe("refs/heads/renamed");
  expect((await f.git(["rev-parse", "refs/tags/main"])).trim()).toBe(state.head);
}), 120000);

test("selected hunks stage, unstage, and discard without touching other changes", () => fixture(async f => {
  const before = Array.from({ length: 45 }, (_, index) => `line ${index + 1}`), after = [...before];
  after.splice(2, 0, "inserted"); after[35] = "replacement";
  await f.write("sample.txt", before.join("\n") + "\n"); await f.git(["add", "."]); await f.git(["commit", "-m", "base"]);
  await f.write("sample.txt", after.join("\n") + "\n");
  const diff = parseDiff(await f.repo.diff("sample.txt"));
  expect(diff.unsupported).toBeNull(); expect(diff.hunks.length).toBeGreaterThan(1);
  const selected = diff.hunks[0].lines.filter((line: any) => ["+", "-"].includes(line.kind)).map((line: any) => line.index);
  await f.repo.run("applyPatch", { patchFile: await f.patch(selectedPatch(diff, selected)) });
  const staged = [...before]; staged.splice(2, 0, "inserted");
  expect(await f.git(["show", ":sample.txt"])).toBe(staged.join("\n") + "\n");
  expect(await f.reader(join(f.root, "sample.txt"))).toBe(after.join("\n") + "\n");
  const cached = parseDiff(await f.repo.diff("sample.txt", "staged"));
  const all = cached.hunks.flatMap((h: any) => h.lines.filter((line: any) => ["+", "-"].includes(line.kind)).map((line: any) => line.index));
  await f.repo.run("applyPatch", { patchFile: await f.patch(selectedPatch(cached, all, true)) });
  expect(await f.git(["show", ":sample.txt"])).toBe(before.join("\n") + "\n");
  await f.repo.run("applyPatch", { patchFile: await f.patch(selectedPatch(diff, selected, true)), cached: false });
  const remaining = [...before]; remaining[34] = "replacement";
  expect(await f.reader(join(f.root, "sample.txt"))).toBe(remaining.join("\n") + "\n");
  expect(await f.git(["diff", "--cached"])).toBe("");
}), 120000);

test("selected patches preserve UTF-8, CRLF, and unterminated final lines", () => fixture(async f => {
  for (const [before, after] of [["첫 줄\r\n이전\r\n끝\r\n", "첫 줄\r\n변경\r\n끝\r\n"], ["a\nold", "a\nnew"]]) {
    await f.write("sample.txt", before); await f.git(["add", "sample.txt"]); await f.write("sample.txt", after);
    const diff = parseDiff(await f.repo.diff("sample.txt"));
    const all = diff.hunks.flatMap((h: any) => h.lines.filter((line: any) => ["+", "-"].includes(line.kind)).map((line: any) => line.index));
    await f.repo.run("applyPatch", { patchFile: await f.patch(selectedPatch(diff, all)) });
    expect(await f.git(["show", ":sample.txt"])).toBe(after);
    await f.repo.run("applyPatch", { patchFile: await f.patch(selectedPatch(diff, all, true)) });
    expect(await f.git(["show", ":sample.txt"])).toBe(before);
    expect(() => selectedPatch(diff, [])).toThrow();
    expect(() => selectedPatch(diff, [99999])).toThrow();
  }
  await f.write("sample.txt", "a\nold"); await f.git(["add", "sample.txt"]); await f.write("sample.txt", "a\nnew");
  const diff = parseDiff(await f.repo.diff("sample.txt"));
  const additions = diff.hunks.flatMap((h: any) => h.lines.filter((line: any) => line.kind === "+").map((line: any) => line.index));
  await f.repo.run("applyPatch", { patchFile: await f.patch(selectedPatch(diff, additions)) });
  expect(await f.git(["show", ":sample.txt"])).toBe("a\nold\nnew");
}), 120000);

test("partial patches reject unsupported file changes and truncated hunks", () => {
  const samples = [
    "diff --git a/a b/a\nindex 123..456 100644\nBinary files a/a and b/a differ\n",
    "diff --git a/a b/b\nsimilarity index 100%\nrename from a\nrename to b\n",
    "diff --git a/a b/a\nold mode 100644\nnew mode 100755\n",
    "diff --git a/a b/a\nnew file mode 100644\n--- /dev/null\n+++ b/a\n@@ -0,0 +1 @@\n+hi\n",
    "diff --git a/a b/a\ndeleted file mode 100644\n--- a/a\n+++ /dev/null\n@@ -1 +0,0 @@\n-hi\n",
    "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n-hi\n+new\n",
  ];
  for (const text of samples) { const diff = parseDiff(text); expect(diff.unsupported).toBeTruthy(); expect(() => selectedPatch(diff, [0])).toThrow(); }
});

test("publish, push, and sync update only the selected local remote", () => fixture(async f => {
  const remote = join(f.sandbox, "remote.git"), peer = join(f.sandbox, "peer");
  await mkdir(remote); await f.git(["init", "--bare", "-b", "main"], remote);
  await f.write("base.txt", "base\n"); await f.git(["add", "."]); await f.git(["commit", "-m", "base"]);
  await f.repo.run("remoteAdd", { name: "origin", url: remote });
  await f.repo.run("publish", { branch: { name: "main" }, remote: "origin" });
  const branch = { name: "main", upstream: "origin/main", remote: "origin", target: "refs/heads/main" };
  expect((await f.git(["for-each-ref", "--format=%(upstream:short)", "refs/heads/main"])).trim()).toBe("origin/main");
  await f.write("base.txt", "updated\n"); await f.git(["add", "."]); await f.git(["commit", "-m", "outgoing"]);
  await f.repo.run("push", { branch });
  expect((await f.git(["rev-parse", "main"], remote)).trim()).toBe((await f.git(["rev-parse", "HEAD"])).trim());
  await f.git(["clone", "--", remote, peer]);
  await f.write("peer.txt", "incoming\n", peer); await f.git(["add", "."], peer); await f.git(["commit", "-m", "incoming"], peer); await f.git(["push"], peer);
  await f.write("local.txt", "local\n"); await f.git(["add", "."]); await f.git(["commit", "-m", "local"]);
  await f.repo.run("sync", { branch });
  expect(await f.reader(join(f.root, "peer.txt"))).toBe("incoming\n");
  expect((await f.git(["rev-parse", "main"], remote)).trim()).toBe((await f.git(["rev-parse", "HEAD"])).trim());
}), 240000);

test("force push leases reject a moved remote and stale branch before overwriting", () => fixture(async f => {
  const remote = join(f.sandbox, "remote.git");
  await mkdir(remote); await f.git(["init", "--bare", "-b", "main"], remote);
  await f.write("base.txt", "base\n"); await f.git(["add", "."]); await f.git(["commit", "-m", "base"]);
  await f.git(["remote", "add", "origin", remote]); await f.git(["push", "-u", "origin", "main"]);
  const expected = (await f.git(["rev-parse", "HEAD"])).trim();
  const tree = (await f.git(["rev-parse", "HEAD^{tree}"])).trim();
  const branch = { name: "main", upstream: "origin/main", remote: "origin", target: "refs/heads/main", remoteHead: expected };
  // Advance the bare repository without updating this checkout's tracking ref.
  const remoteHead = (await f.git(["commit-tree", tree, "-p", expected, "-m", "remote moved"], remote)).trim();
  await f.git(["update-ref", "refs/heads/main", remoteHead], remote);
  await f.git(["commit", "--amend", "-m", "rewritten local commit"]);
  await expect(f.repo.run("forcePush", { branch })).rejects.toThrow();
  expect((await f.git(["rev-parse", "main"], remote)).trim()).toBe(remoteHead);
  await f.repo.run("forcePush", { branch, expected: remoteHead });
  expect((await f.git(["rev-parse", "main"], remote)).trim()).toBe((await f.git(["rev-parse", "HEAD"])).trim());
  await f.git(["switch", "-c", "other"]);
  await expect(f.repo.run("push", { branch })).rejects.toThrow();
  expect(f.calls.some((args: string[]) => args.includes("--force"))).toBe(false);
}), 240000);

test("merge resolution detects stale text, conflict OIDs, and HEAD before writing or staging", () => fixture(async f => {
  const file = join(f.root, "sample.txt"), markers = "<<<<<<< HEAD\ncurrent\n=======\nincoming\n>>>>>>> feature\n";
  const hashes = await Promise.all(["base\n", "current\n", "incoming\n"].map(async text => (await f.git(["hash-object", "-w", "--stdin"], f.root, text)).trim()));
  const conflict = async () => {
    await f.git(["update-index", "--force-remove", "--", "sample.txt"]);
    await f.git(["update-index", "--index-info"], f.root, hashes.map((hash, index) => `100644 ${hash} ${index + 1}\tsample.txt\n`).join(""));
    await f.write("sample.txt", markers);
  };
  const jobs: Promise<unknown>[] = [], writes: string[] = [];
  let afterWrite: (() => Promise<void>) | undefined;
  const cx: any = { notify() {}, spawn(fn: Function) { jobs.push(Promise.resolve().then(() => fn(cx))); } };
  const drain = async () => { while (jobs.length) await jobs.shift(); };
  const globals = {
    exec: f.run, readTextFile: f.reader, createGit: f.createGit, theme: () => ({}), settings: () => ({}), setShared() {}, closeOverlay() {},
    EditorState: { new(options: any) { let value = options.value; return { value: () => value, set_value(next: string) { value = next; }, on() {}, release() {} }; } },
    writeTextFile: async (path: string, value: string, expected: string) => {
      expect(resolve(path)).toBe(resolve(file));
      if (await f.reader(path) !== expected) throw new Error("stale snapshot");
      await writeFile(`${path}.save`, value); await rename(`${path}.save`, path); writes.push(value); await afterWrite?.();
    },
  };
  const MergePanel = new Function(...Object.keys(globals), `${mergeSource}; return MergePanel;`)(...Object.values(globals));
  const open = async () => { const panel = new MergePanel({ root: f.root, on: null, file: { path: "sample.txt" } }, cx); await drain(); expect(panel.error).toBeNull(); return panel; };
  await conflict(); let panel = await open();
  panel.accept("current", cx); expect(await f.reader(file)).toBe(markers); expect(writes).toHaveLength(0);
  panel.editors.result.set_value(markers); panel.save(true, cx); await drain(); expect(writes).toHaveLength(0); expect(panel.error).toBeTruthy();
  panel.editors.result.set_value("resolved\n"); panel.save(false, cx); await drain(); expect(panel.original).toBe("resolved\n");
  expect(await f.git(["ls-files", "-u"])).not.toBe("");
  panel.save(true, cx); await drain(); expect(await f.git(["ls-files", "-u"])).toBe(""); expect(await f.git(["show", ":sample.txt"])).toBe("resolved\n"); panel.close();
  const count = writes.length, stages = () => f.calls.filter((args: string[]) => args.includes("add")).length, stageCount = stages();
  await conflict(); panel = await open(); await f.write("sample.txt", "external text\n"); panel.editors.result.set_value("mine\n"); panel.save(true, cx); await drain();
  expect(writes).toHaveLength(count); expect(stages()).toBe(stageCount); expect(await f.reader(file)).toBe("external text\n"); panel.close();
  await conflict(); panel = await open(); await f.git(["update-index", "--index-info"], f.root, `100644 ${hashes[2]} 2\tsample.txt\n`);
  panel.editors.result.set_value("mine\n"); panel.save(true, cx); await drain(); expect(writes).toHaveLength(count); expect(stages()).toBe(stageCount); expect(await f.reader(file)).toBe(markers); panel.close();
  const tree = (await f.git(["hash-object", "-t", "tree", "-w", "--stdin"], f.root, "")).trim();
  const first = (await f.git(["commit-tree", tree, "-m", "first"])).trim(), second = (await f.git(["commit-tree", tree, "-m", "second"])).trim();
  await f.git(["update-ref", "HEAD", first]); await conflict(); panel = await open(); await f.git(["update-ref", "HEAD", second]);
  panel.editors.result.set_value("mine\n"); panel.save(true, cx); await drain(); expect(writes).toHaveLength(count); expect(stages()).toBe(stageCount); expect(await f.reader(file)).toBe(markers); panel.close();
  await conflict(); panel = await open();
  afterWrite = () => f.git(["update-index", "--index-info"], f.root, `100644 ${hashes[2]} 2\tsample.txt\n`).then(() => {});
  panel.editors.result.set_value("saved before index changed\n"); panel.save(true, cx); await drain();
  expect(writes).toHaveLength(count + 1); expect(stages()).toBe(stageCount); expect(panel.error).toContain("saved, but staging failed"); panel.close();
}), 240000);
