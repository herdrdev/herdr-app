import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../defaults/mods/changes/main.js", import.meta.url), "utf8");
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
type Answer<T> = T & { wait?: Promise<void> };

// The panel on a machine with git repos /repo-a and /repo-b; any other folder is not a repo.
// Each git status in a repo, and each read of a file, takes the next answer queued for it, after its
// wait; without one, a repo has one untracked file (a.txt, b.txt) and a file reads "text".
async function panel(statuses: Record<string, Answer<{ files: string[] }>[]> = {}, reads: Record<string, Answer<{ text: string }>[]> = {}) {
  const at = { cwd: "/repo-a" };
  const asked: string[] = [];
  const Changes = new Function("View", "InputState", "spaces", "panes", "focused", "exec", "readTextFile", "watchFolder",
    `${source.replace(/^import .*;\n/gm, "").replace("export default class Changes", "class Changes")}; return Changes;`)(
    class {},
    { new: () => ({ on() {}, value: () => "", set_value() {} }) },
    () => [{ active: true, folder: at.cwd, machine: null }], () => [], () => null,
    async (_git: string, args: string[], { cwd }: { cwd: string }) => {
      const repo = /^\/repo-[ab]/.exec(cwd)?.[0];
      if (!repo) return { code: 128, stdout: "", stderr: "not a git repository" };
      if (args.includes("--show-toplevel")) return { code: 0, stdout: `${repo}\n`, stderr: "" };
      if (args.includes("status")) {
        asked.push(repo);
        const answer = statuses[repo]?.shift() ?? { files: [`${repo.slice(-1)}.txt`] };
        await answer.wait;
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
    () => new Promise(() => {}),
  );
  const view = new Changes();
  const cx = { focus_handle: () => ({}), timer: { every() {} }, notify() {}, spawn(task: (cx: unknown) => Promise<unknown>) { task(cx); } };
  view.init({}, cx);
  await settled();
  const show = async (cwd: string) => { at.cwd = cwd; view.refresh(cx); await settled(); };
  return {
    view, cx, show, asked,
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
