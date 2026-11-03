import { test, expect } from "bun:test";
import { mkdtemp, mkdir, writeFile, readFile, readdir, realpath, rm, chmod } from "node:fs/promises";
import { existsSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";

const source = await readFile(new URL("../defaults/mods/worktree/main.js", import.meta.url), "utf8");
const part = (from: string, to: string) => source.slice(source.indexOf(from), source.indexOf(to));
const realRm = Bun.which("rm");

// What each exec saw: whether the checkout was still there, and what was next to it.
const calls: { argv: string[]; there: boolean; beside: string[] }[] = [];
let at = { top: "", bin: "" };
const removeAside = new Function("exec", `${part("async function git(", "\n// The submodule repos")}\n${part("async function removeAside(", "\n// The repo a folder")}; return removeAside;`)(
  async (command: string, args: string[] = [], options?: { cwd?: string }) => {
    calls.push({ argv: [command, ...args], there: existsSync(at.top), beside: readdirSync(dirname(at.top)) });
    const p = Bun.spawn([command, ...args], { cwd: options?.cwd, stdout: "pipe", stderr: "pipe", env: { ...process.env, PATH: `${at.bin}:${process.env.PATH}` } });
    const [code, stdout, stderr] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    return { code, stdout, stderr };
  },
);

function git(cwd: string, ...args: string[]) {
  const p = Bun.spawnSync(["git", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args], { cwd });
  if (p.exitCode !== 0) throw new Error(`git ${args.join(" ")}: ${p.stderr}`);
  return p.stdout.toString().trim();
}

// A repo with one linked worktree holding ignored dependencies, and a slow rm
// on PATH: if the background delete kept exec's pipes open, exec would wait for it.
async function checkout() {
  const root = await realpath(await mkdtemp(join(process.platform === "linux" ? "/var/tmp" : tmpdir(), "herdr-worktree-")));
  const main = join(root, "repo"), top = join(root, "feat"), bin = join(root, "bin");
  await mkdir(main);
  git(main, "init", "-q");
  await writeFile(join(main, ".gitignore"), "node_modules/\n");
  git(main, "add", ".gitignore");
  git(main, "commit", "-qm", "init");
  git(main, "worktree", "add", "-q", "-b", "feat", top, "HEAD");
  for (let i = 0; i < 20; i++) {
    await mkdir(join(top, "node_modules", `pkg${i}`), { recursive: true });
    await writeFile(join(top, "node_modules", `pkg${i}`, "index.js"), "");
  }
  await mkdir(bin);
  await writeFile(join(bin, "rm"), `#!/bin/sh\ntouch "${root}/slow-rm-ran"\nsleep 3\nexec "${realRm}" "$@"\n`);
  await chmod(join(bin, "rm"), 0o755);
  at = { top, bin };
  calls.length = 0;
  return { root, main, top };
}

const checkouts = (main: string) => git(main, "worktree", "list", "--porcelain").split("\n").filter((l) => l.startsWith("worktree "));
const leftovers = async (root: string) => (await readdir(root)).filter((name) => name.startsWith("feat."));

test.skipIf(process.platform === "win32")("Remove moves the checkout aside first, so git only forgets it and the files go in the background", async () => {
  const { root, main, top } = await checkout();
  try {
    const started = performance.now();
    expect(await removeAside(main, top, ["worktree", "remove", top])).toBe(true);
    expect(performance.now() - started).toBeLessThan(2000);
    expect(calls.find((c) => c.argv.join(" ").includes("worktree remove"))?.there).toBe(false);
    // Next to itself, so a rename on the same disk rather than a copy.
    expect(calls.find((c) => c.argv.join(" ").includes("rm -rf"))?.beside.filter((name) => name.startsWith("feat.removing-"))).toHaveLength(1);
    expect(checkouts(main)).toEqual([`worktree ${main}`]);
    expect(existsSync(top)).toBe(false);
    for (let i = 0; i < 100 && (await leftovers(root)).length; i++) await Bun.sleep(100);
    expect(await leftovers(root)).toEqual([]);
    expect(existsSync(join(root, "slow-rm-ran"))).toBe(true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}, 15000);

test.skipIf(process.platform === "win32")("Without --force, what git would refuse is left for the plain remove", async () => {
  const { root, main, top } = await checkout();
  try {
    await writeFile(join(top, "notes.md"), "draft");
    expect(await removeAside(main, top, ["worktree", "remove", top])).toBe(false);
    expect(existsSync(join(top, "notes.md"))).toBe(true);
    await rm(join(top, "notes.md"));
    // A repo checked out inside, with history of its own: git won't remove it unforced.
    await mkdir(join(top, "lib"));
    git(join(top, "lib"), "init", "-q");
    git(join(top, "lib"), "commit", "-q", "--allow-empty", "-m", "local only");
    git(top, "add", "lib");
    git(top, "commit", "-qm", "add lib");
    expect(await removeAside(main, top, ["worktree", "remove", top])).toBe(false);
    expect(existsSync(join(top, "lib", ".git"))).toBe(true);
    expect(checkouts(main)).toHaveLength(2);
    expect(await leftovers(root)).toEqual([]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test.skipIf(process.platform === "win32")("The main checkout is never moved, even for a moment", async () => {
  const { root, main } = await checkout();
  try {
    at.top = main;
    expect(await removeAside(main, main, ["worktree", "remove", main])).toBe(false);
    expect(calls.every((c) => c.there)).toBe(true);
    expect((await readdir(root)).filter((name) => name.startsWith("repo."))).toEqual([]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test.skipIf(process.platform === "win32")("A worktree git won't remove is put back where it was", async () => {
  const { root, main, top } = await checkout();
  try {
    git(main, "worktree", "lock", "--reason", "on a usb disk", top);
    expect(await removeAside(main, top, ["worktree", "remove", "--force", top])).toBe(false);
    expect(existsSync(join(top, "node_modules", "pkg0", "index.js"))).toBe(true);
    expect(git(top, "rev-parse", "--abbrev-ref", "HEAD")).toBe("feat");
    expect(checkouts(main)).toHaveLength(2);
    expect(await leftovers(root)).toEqual([]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
