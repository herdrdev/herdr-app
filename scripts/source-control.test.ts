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
  const Panel = new Function("View", "TextareaState", `${panelSource}; return SourceControl;`)(class {}, { new: () => input });
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
  panel.data = { head: "head", groups: { staged: [], changes: [], conflicts: [] }, remotes: ["origin"], operation: null,
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
