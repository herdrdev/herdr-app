import { test, expect } from "bun:test";
import { readFile, writeFile, mkdtemp, mkdir, rm, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";

const source = (await readFile(new URL("../defaults/mods/source-control/git.js", import.meta.url), "utf8"))
  .replace(/^import .*;\r?\n/gm, "").replace(/^export /gm, "");
const hashes = (state: any): string[] => state.graph.split("\n").map((line: string) => line.split("\x1f")[1]).filter(Boolean);

async function fixture(body: (f: any) => Promise<void>) {
  const temporary = await realpath(tmpdir()), sandbox = await mkdtemp(join(temporary, "herdr-history-")), root = join(sandbox, "work");
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")));
  Object.assign(env, { GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(sandbox, "gitconfig"), GIT_TERMINAL_PROMPT: "0", GIT_ALLOW_PROTOCOL: "file",
    GIT_AUTHOR_NAME: "Fixture", GIT_AUTHOR_EMAIL: "fixture@example.invalid", GIT_COMMITTER_NAME: "Fixture", GIT_COMMITTER_EMAIL: "fixture@example.invalid" });
  const calls: string[][] = [];
  const inside = (path: string) => {
    const child = relative(sandbox, resolve(path));
    if (!child || isAbsolute(child) || child === ".." || child.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`)) throw new Error("Path outside history fixture");
  };
  const run = async (command: string, args: string[], options: { cwd: string }, input?: string) => {
    expect(command).toBe("git"); inside(options.cwd); calls.push(args);
    const process = Bun.spawn([command, ...args], { cwd: options.cwd, env, windowsHide: true, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
    if (input !== undefined) process.stdin.write(input);
    process.stdin.end();
    const [code, stdout, stderr] = await Promise.all([process.exited, new Response(process.stdout).text(), new Response(process.stderr).text()]);
    return { code, stdout, stderr };
  };
  const git = async (args: string[], cwd = root, input?: string) => {
    const result = await run("git", args, { cwd }, input);
    if (result.code) throw new Error(`${args.join(" ")}: ${result.stderr || result.stdout}`);
    return result.stdout.trim();
  };
  const reader = async (path: string) => {
    inside(path);
    try { return await readFile(path, "utf8"); }
    catch (error: any) { if (error.code === "ENOENT") return null; throw error; }
  };
  const createGit = new Function("exec", "readTextFile", `${source}; return createGit;`)(run, reader);
  try {
    await mkdir(root); await writeFile(join(sandbox, "gitconfig"), "");
    await git(["init", "-b", "main"]);
    const tree = await git(["hash-object", "-t", "tree", "-w", "--stdin"], root, "");
    await body({ sandbox, root, git, calls, repo: createGit(root),
      commit: (subject: string, ...parents: string[]) => git(["commit-tree", tree, ...parents.flatMap(parent => ["-p", parent]), "-m", subject]),
    });
  } finally {
    expect(dirname(resolve(sandbox))).toBe(temporary);
    await rm(sandbox, { recursive: true, force: true });
  }
}

test("history Auto follows current, upstream, and base while All and custom refs stay distinct", () => fixture(async f => {
  const base = await f.commit("base"), main = await f.commit("main advanced", base), fork = await f.commit("feature fork", base);
  const local = await f.commit("local only", fork), remote = await f.commit("remote only", fork), unrelated = await f.commit("unrelated root");
  await f.git(["update-ref", "refs/heads/main", main]);
  await f.git(["update-ref", "refs/heads/feature", local]);
  await f.git(["update-ref", "refs/heads/unrelated", unrelated]);
  await f.git(["symbolic-ref", "HEAD", "refs/heads/feature"]);
  const bare = join(f.sandbox, "remote.git"); await mkdir(bare); await f.git(["init", "--bare", "-b", "main"], bare);
  await f.git(["remote", "add", "origin", bare]);
  await f.git(["push", "origin", "refs/heads/main", "refs/heads/unrelated", `${remote}:refs/heads/feature`]);
  await f.git(["fetch", "origin"]);
  await f.git(["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
  await f.git(["config", "branch.feature.remote", "origin"]); await f.git(["config", "branch.feature.merge", "refs/heads/feature"]);
  const config = await readFile(join(f.root, ".git", "config"), "utf8"), refsBefore = await f.git(["show-ref"]);
  const start = f.calls.length, auto = await f.repo.read();
  expect(auto.baseRef).toBe("refs/remotes/origin/main");
  expect(auto.graphRefs).toEqual(["refs/heads/feature", "refs/remotes/origin/feature", "refs/remotes/origin/main"]);
  expect(auto.graphIncludesHead).toBe(true); expect(auto.branch).toMatchObject({ ahead: 1, behind: 1, remoteHead: remote });
  expect(new Set(hashes(auto))).toEqual(new Set([base, main, fork, local, remote]));
  expect(auto.refs).toContainEqual(expect.objectContaining({ ref: "refs/remotes/origin/HEAD", symbolicTarget: "refs/remotes/origin/main", hash: main }));
  const all = await f.repo.read(200, { mode: "all" });
  expect(hashes(all)).toContain(unrelated); expect(all.graphIncludesHead).toBe(true);
  const current = await f.repo.read(200, { mode: "refs", refs: ["refs/heads/feature"] });
  expect(current.graphRefs).toEqual(["refs/heads/feature"]); expect(current.graphIncludesHead).toBe(true);
  expect(new Set(hashes(current))).toEqual(new Set([local, fork, base]));
  const alias = await f.repo.read(200, { mode: "refs", refs: ["refs/remotes/origin/HEAD", "refs/remotes/origin/main"] });
  expect(alias.graphRefs).toEqual(["refs/remotes/origin/main"]); expect(alias.graphIncludesHead).toBe(false);
  expect(new Set(hashes(alias))).toEqual(new Set([main, base]));
  expect(f.calls.slice(start).some((args: string[]) => args.includes("fetch") || args.includes("push"))).toBe(false);
  expect(await readFile(join(f.root, ".git", "config"), "utf8")).toBe(config);
  expect(await f.git(["show-ref"])).toBe(refsBefore);
  // Moving fixture refs exercises both one-sided states without changing any user's repository.
  await f.git(["update-ref", "refs/heads/feature", fork]);
  expect((await f.repo.read(1)).branch).toMatchObject({ ahead: 0, behind: 1 });
  await f.git(["update-ref", "refs/heads/feature", local]);
  await f.git(["update-ref", "refs/heads/feature", fork], bare); await f.git(["fetch", "origin"]);
  expect((await f.repo.read(1)).branch).toMatchObject({ ahead: 1, behind: 0 });
  await f.git(["config", "branch.feature.vscode-merge-base", "origin/unrelated"]);
  const configured = await f.repo.read();
  expect(configured.baseRef).toBe("refs/remotes/origin/unrelated"); expect(hashes(configured)).toContain(unrelated);
  await f.git(["config", "branch.feature.vscode-merge-base", "refs/remotes/origin/deleted"]);
  expect((await f.repo.read()).baseRef).toBe("refs/remotes/origin/main");
  await f.git(["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
  expect((await f.repo.read()).baseRef).toBe("refs/remotes/origin/main");
}), 240000);

test("history resolves annotated tags and full-ref collisions without broadening invalid selections", () => fixture(async f => {
  const current = await f.commit("current"), tagged = await f.commit("tagged separate root");
  await f.git(["update-ref", "refs/heads/main", current]);
  await f.git(["tag", "-a", "main", "-m", "annotated", tagged]);
  await f.git(["tag", "-a", "nested", "-m", "nested", "refs/tags/main"]);
  const blob = await f.git(["hash-object", "-w", "--stdin"], f.root, "not a commit");
  await f.git(["tag", "blob", blob]);
  const auto = await f.repo.read();
  expect(auto.branch.name).toBe("main"); expect(hashes(auto)).toEqual([current]);
  expect(auto.refs.find((ref: any) => ref.ref === "refs/tags/main")).toMatchObject({ type: "tag", hash: tagged });
  expect(auto.refs.find((ref: any) => ref.ref === "refs/tags/main").objectHash).not.toBe(tagged);
  expect(auto.refs.find((ref: any) => ref.ref === "refs/tags/nested").hash).toBe(tagged);
  expect(auto.refs.find((ref: any) => ref.ref === "refs/tags/blob").hash).toBeNull();
  const tagOnly = await f.repo.read(200, { mode: "refs", refs: ["refs/tags/main"] });
  expect(tagOnly.graphRefs).toEqual(["refs/tags/main"]); expect(tagOnly.graphIncludesHead).toBe(false); expect(hashes(tagOnly)).toEqual([tagged]);
  const withHead = await f.repo.read(200, { mode: "refs", refs: ["refs/tags/main"], includeHead: true });
  expect(withHead.graphIncludesHead).toBe(true); expect(new Set(hashes(withHead))).toEqual(new Set([tagged, current]));
  const invalid = await f.repo.read(200, { mode: "refs", refs: ["main", "HEAD", "--all", "refs/heads/missing", "refs/heads/main..refs/tags/main", "refs/tags/blob", null] });
  expect(invalid.graphRefs).toEqual([]); expect(invalid.graph).toBe(""); expect(invalid.graphIncludesHead).toBe(false);
  await f.git(["tag", "-d", "main"]);
  const deleted = await f.repo.read(200, { mode: "refs", refs: ["refs/tags/main"] });
  expect(deleted.graph).toBe(""); expect(deleted.graphRefs).toEqual([]);
  expect(new Set(hashes(await f.repo.read(200, { mode: "all" })))).toEqual(new Set([current, tagged]));
}), 240000);

test("history handles detached HEAD and unborn branches without implicitly selecting all refs", () => fixture(async f => {
  const empty = await f.repo.read();
  expect(empty.head).toBeNull(); expect(empty.graph).toBe(""); expect(empty.graphRefs).toEqual([]); expect(empty.graphIncludesHead).toBe(false);
  expect((await f.repo.read(200, { mode: "all" })).graph).toBe("");
  const main = await f.commit("main"), detached = await f.commit("detached root");
  await f.git(["update-ref", "refs/heads/main", main]);
  await f.git(["update-ref", "refs/heads/null", main]);
  await f.git(["update-ref", "--no-deref", "HEAD", detached]);
  const auto = await f.repo.read();
  expect(auto.branch).toBeNull(); expect(auto.baseRef).toBeNull(); expect(auto.graphRefs).toEqual([]);
  expect(auto.graphIncludesHead).toBe(true); expect(hashes(auto)).toEqual([detached]);
  expect(new Set(hashes(await f.repo.read(200, { mode: "all" })))).toEqual(new Set([main, detached]));
  const selected = await f.repo.read(200, { mode: "refs", refs: ["refs/heads/null"] });
  expect(selected.graphIncludesHead).toBe(false); expect(hashes(selected)).toEqual([main]);
  expect(new Set(hashes(await f.repo.read(200, { mode: "refs", refs: ["refs/heads/main"], includeHead: true })))).toEqual(new Set([main, detached]));
  await f.git(["symbolic-ref", "HEAD", "refs/heads/unborn"]);
  const unborn = await f.repo.read();
  expect(unborn.head).toBeNull(); expect(unborn.branch.name).toBe("unborn"); expect(unborn.graphIncludesHead).toBe(false);
  expect(unborn.baseRef).toBe("refs/heads/main"); expect(hashes(unborn)).toEqual([main]);
}), 240000);
