import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../defaults/mods/changes/main.js", import.meta.url), "utf8");
const filesSource = await readFile(new URL("../defaults/mods/files/main.js", import.meta.url), "utf8");
const rowsSource = source.slice(source.indexOf("const untab ="), source.indexOf("\nexport default class"));
const { diffRows, untab } = new Function(`${rowsSource}; return { diffRows, untab };`)() as {
  diffRows: (lines: string[]) => { text: string }[];
  untab: (text: string) => string;
};

test("Changes keeps tab indentation, to the next 4-column stop", () => {
  const rows = diffRows(["@@ -1,2 +1,2 @@", "-\tif x {", "+\t\tif x {", " a\tb", "+    spaces"]);
  expect(rows.map((r: { text: string }) => r.text)).toEqual(["@@ -1,2 +1,2 @@", "    if x {", "        if x {", "a   b", "    spaces"]);
});

const oldUntab = (text: string) => text.replace(/[^\t]*\t/g, (s) => s.slice(0, -1).padEnd((s.length - 1) - (s.length - 1) % 4 + 4));

test("untab matches the regex it replaced", () => {
  const long = "x".repeat(4000);
  const inputs = ["", "abc", "\t", "\tx", "\t\tx", "a\tb", "ab\tc", "abc\tx", "abcd\tx", "abcde\tx", "a\tb\tc\td", "tail\t", "\t\t\t",
    "日本\tx", "😀\tx", long, `${long}\t${long}`, `\t${long}`, `${long.slice(0, 3999)}\t`, ("ab\t" + "c".repeat(5)).repeat(500)];
  for (const input of inputs) expect(untab(input)).toBe(oldUntab(input));
});

// The same total length as short and as long lines: linear work takes about as long both ways, quadratic work
// about ten times longer on the long lines. A ratio holds on a slow machine, where a fixed time limit would not.
test("untab takes linear time on long lines without a tab", () => {
  const time = (length: number, times: number) => {
    const line = "x".repeat(length);
    const start = performance.now();
    for (let i = 0; i < times; i++) untab(line);
    return performance.now() - start;
  };
  time(4000, 200);
  const short = time(4000, 1500), long = time(40000, 150);
  expect(long).toBeLessThan(Math.max(short, 1) * 4);
});

// A timer turn runs every promise continuation that is ready.
const settled = () => new Promise(resolve => setTimeout(resolve));
const held = () => { let release!: () => void; const wait = new Promise<void>(resolve => { release = resolve; }); return { wait, release }; };
const never = new Promise<void>(() => {});
type Answer<T> = T & { wait?: Promise<void>; fail?: boolean };

// The panel on a machine with git repos /repo-a and /repo-b; any other folder is not a repo.
// Each git status in a repo, and each read of a file, takes the next answer queued for it, after its
// wait; without one, a repo has one untracked file (a.txt, b.txt) and a file reads "text".
async function panel(statuses: Record<string, Answer<{ files: string[] }>[]> = {}, reads: Record<string, Answer<{ text: string }>[]> = {}, both = false, startDelay = 50) {
  const at = { cwd: "/repo-a", active: true };
  const directoryEntries = new Set(["a.txt"]);
  const asked: string[] = [], vm = {};
  let now = 1;
  const sleepers: { at: number, wake: () => void }[] = [];
  const watches = new Set<(paths: string[]) => void>();
  const define = (source: string) => new Function("View", "InputState", "spaces", "panes", "focused", "exec", "readTextFile", "watchFolder", "window", "globalThis", "Date",
    source.replace(/^import .*;\n/gm, "").replace("export default class", "return class"))(
    class {},
    { new: () => ({ on() {}, value: () => "", set_value() {} }) },
    () => [{ active: true, folder: at.cwd, machine: null }], () => [], () => null,
    async (command: string, args: string[], { cwd }: { cwd: string }) => {
      if (command === "sh") return { code: 0, stdout: [...directoryEntries].map(entry => `f\0${entry}\0`).join(""), stderr: "" };
      const repo = /^\/repo-[ab]/.exec(cwd)?.[0];
      if (!repo) return { code: 128, stdout: "", stderr: "not a git repository" };
      if (args.includes("--show-toplevel")) return { code: 0, stdout: `${repo}\n`, stderr: "" };
      if (args.includes("status")) {
        asked.push(repo);
        const answer = statuses[repo]?.shift() ?? { files: [`${repo.slice(-1)}.txt`] };
        await answer.wait;
        if (answer.fail) return { code: 128, stdout: "", stderr: "fatal: bad object HEAD" };
        return { code: 0, stdout: answer.files.map(f => `?? ${f}\0`).join(""), stderr: "" };
      }
      if (args.includes("HEAD")) return { code: 128, stdout: "", stderr: "no HEAD" };
      return { code: 0, stdout: "", stderr: "" };
    },
    async (path: string) => {
      const answer = reads[path]?.shift() ?? { text: "text\n" };
      await answer.wait;
      return answer.text;
    },
    () => {
      let deliver!: (paths: string[]) => void;
      const promise = new Promise<string[]>(resolve => { deliver = resolve; watches.add(resolve); });
      return Object.assign(promise, { cancel() { watches.delete(deliver); deliver([]); } });
    },
    () => ({ active: at.active }), vm, { now: () => now },
  );
  const Changes = define(source), view = new Changes();
  const ticks: ((cx: unknown) => void)[] = [];
  const cx = {
    focus_handle: () => ({}),
    timer: { every(_interval: number, tick: (cx: unknown) => void) { ticks.push(tick); } },
    sleep(ms: number) { return new Promise<void>(wake => sleepers.push({ at: now + ms, wake })); },
    notify() {},
    spawn(task: (cx: unknown) => Promise<unknown>) {
      let done = false;
      task(cx).finally(() => { done = true; });
      return { is_done: () => done };
    },
  };
  const wait = async (ms: number) => {
    const until = now + ms;
    for (;;) {
      sleepers.sort((a, b) => a.at - b.at);
      const next = sleepers[0];
      if (!next || next.at > until) break;
      now = next.at;
      sleepers.shift()?.wake();
      await settled();
    }
    now = until;
    await settled();
  };
  view.init({}, cx);
  const files = both ? new (define(filesSource))() : null;
  files?.init({}, cx);
  await settled();
  await wait(startDelay);
  const show = async (cwd: string) => { at.cwd = cwd; view.refresh(cx); await settled(); await wait(50); };
  const edit = async (name = "a.txt") => {
    now++;
    directoryEntries.add(name);
    const listeners = [...watches];
    watches.clear();
    for (const deliver of listeners) deliver([`${at.cwd}/${name}`]);
    await settled();
  };
  return {
    view, files, cx, show, asked, edit, at,
    wait,
    poll: async () => { for (const tick of ticks) tick(cx); await settled(); await wait(50); },
    paths: () => view.files.map((f: { path: string }) => f.path),
    preview: () => view.rows.map((r: { text: string }) => r.text),
  };
}

test.each([
  ["Bengali spacing marks", "বাংলা ".repeat(8) + "END", 51],
  ["tab-indented CJK glyphs", "\t" + "界".repeat(20) + "END", 47],
])("Changes leaves horizontal room for %s", async (_name, text, columns) => {
  const h = await panel({}, { "/repo-a/a.txt": [{ text }] });
  expect(h.view.longest).toBeGreaterThanOrEqual(columns);
});

test("Changes follows a space switch while the last repo's preview never finishes", async () => {
  const h = await panel({}, { "/repo-a/a.txt": [{ text: "", wait: never }] });
  expect(h.paths()).toEqual(["a.txt"]);
  await h.show("/repo-b");
  expect(h.paths()).toEqual(["b.txt"]);
  await h.show("/plain");
  expect(h.paths()).toEqual([]);
  expect(h.view.error).toBe("Not a git repository");
});

test("Changes runs one load at a time in a repo, even when the one it left finishes late", async () => {
  const preview = held(), status = held();
  const h = await panel({ "/repo-b": [{ files: ["b.txt"], wait: status.wait }] }, { "/repo-a/a.txt": [{ text: "text\n", wait: preview.wait }] });
  await h.show("/repo-b");
  preview.release();
  await settled();
  h.view.refresh(h.cx);
  await settled();
  expect(h.asked).toEqual(["/repo-a", "/repo-b"]);
  status.release();
  await settled();
  await h.wait(50);
  expect(h.paths()).toEqual(["b.txt"]);
  expect(h.view.loading).toBe(false);
});

test("Changes keeps the newer list when a load from an earlier visit to the repo finishes last", async () => {
  const first = held();
  const h = await panel({ "/repo-a": [{ files: ["old.txt"], wait: first.wait }, { files: ["new.txt"] }] });
  await h.show("/repo-b");
  await h.show("/repo-a");
  expect(h.paths()).toEqual(["new.txt"]);
  first.release();
  await settled();
  await h.wait(50);
  expect(h.paths()).toEqual(["new.txt"]);
  expect(h.view.loading).toBe(false);
});

test("Changes keeps the newer preview when one from an earlier visit to the repo finishes last", async () => {
  const first = held();
  const h = await panel({}, { "/repo-a/a.txt": [{ text: "old\n", wait: first.wait }, { text: "new\n" }] });
  await h.show("/repo-b");
  await h.show("/repo-a");
  expect(h.preview()).toEqual(["@@ -0,0 +1,1 @@", "new"]);
  first.release();
  await settled();
  expect(h.preview()).toEqual(["@@ -0,0 +1,1 @@", "new"]);
});

test("Files and Changes use one status and both catch edits during it", async () => {
  const first = held();
  const h = await panel({ "/repo-a": [{ files: [], wait: first.wait }, { files: ["a.txt"] }] }, {}, true);
  expect(h.asked).toEqual(["/repo-a"]);
  await h.edit();
  await h.edit();
  expect(h.asked).toEqual(["/repo-a"]);
  first.release();
  await settled();
  await h.wait(50);
  await h.poll();
  expect(h.asked).toEqual(["/repo-a", "/repo-a"]);
  expect(h.paths()).toEqual(["a.txt"]);
  expect(h.files.marks).toEqual({ "/repo-a/a.txt": "A" });
});

test("Files lists a file created between its directory listing and the Git scan", async () => {
  const h = await panel({ "/repo-a": [{ files: ["created.txt"] }] }, {}, true, 0);
  await h.edit("created.txt");
  await h.wait(50);
  await h.poll();
  expect(h.files.entries.get("").map((entry: { name: string }) => entry.name)).toContain("created.txt");
  expect(h.files.marks).toEqual({ "/repo-a/created.txt": "A" });
  expect(h.asked).toEqual(["/repo-a"]);
});

test("Changes backs off a slow poll until five scan lengths after it finished", async () => {
  const first = held();
  const h = await panel({ "/repo-a": [{ files: [], wait: first.wait }] });
  await h.wait(5000);
  first.release();
  await settled();
  await h.wait(24900);
  await h.poll();
  expect(h.asked).toEqual(["/repo-a"]);
  await h.wait(200);
  await h.poll();
  expect(h.asked).toEqual(["/repo-a", "/repo-a"]);
});

test("Changes doesn't rescan in a loop when git status keeps failing after an edit", async () => {
  const failing = Array.from({ length: 50 }, () => ({ files: [], fail: true }));
  const h = await panel({ "/repo-a": failing });
  expect(h.view.error).toContain("bad object HEAD");
  const before = h.asked.length;
  await h.edit();
  for (let frame = 0; frame < 60; frame++) {
    h.view.refreshIfDue(h.cx);
    await h.wait(16);
  }
  expect(h.asked.length - before).toBeLessThanOrEqual(1);
});

test("Files and Changes space out watch scans, pause while unfocused and catch up once", async () => {
  const first = held();
  const h = await panel({ "/repo-a": [{ files: [], wait: first.wait }] }, {}, true);
  await h.wait(5000);
  first.release();
  await h.wait(50);
  await h.edit();
  await h.edit();
  await h.wait(5000);
  await h.poll();
  expect(h.asked).toEqual(["/repo-a"]);

  h.at.active = false;
  await h.edit();
  await h.wait(30000);
  await h.poll();
  expect(h.asked).toEqual(["/repo-a"]);

  h.at.active = true;
  await h.poll();
  expect(h.asked).toEqual(["/repo-a", "/repo-a"]);
  expect(h.paths()).toEqual(["a.txt"]);
  expect(h.files.marks).toEqual({ "/repo-a/a.txt": "A" });
  await h.poll();
  expect(h.asked).toEqual(["/repo-a", "/repo-a"]);
});
