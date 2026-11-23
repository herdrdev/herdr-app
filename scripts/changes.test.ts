import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../defaults/mods/changes/main.js", import.meta.url), "utf8");
const rowsSource = source.slice(source.indexOf("const untab ="), source.indexOf("\nexport default class"));
const diffRows = new Function(`${rowsSource}; return diffRows;`)();

test("Changes keeps tab indentation, to the next 4-column stop", () => {
  const rows = diffRows(["@@ -1,2 +1,2 @@", "-\tif x {", "+\t\tif x {", " a\tb", "+    spaces"]);
  expect(rows.map((r: { text: string }) => r.text)).toEqual(["@@ -1,2 +1,2 @@", "    if x {", "        if x {", "a   b", "    spaces"]);
});

type Watch = Promise<string[]> & { root: string; cancel(): void; change(): void };
// A timer turn runs every promise continuation that is ready.
const settled = () => new Promise(resolve => setTimeout(resolve));
const held = () => { let release!: () => void; const wait = new Promise<void>(resolve => { release = resolve; }); return { wait, release }; };
const never = new Promise<void>(() => {});
type Answer<T> = T & { wait?: Promise<void> };

// The panel on a machine where every /repo-<name> is a git repo; any other folder is not a repo.
// Each git status in a repo, and each read of a file, takes the next answer queued for it, after its
// wait; without one, a repo has one untracked file (a.txt, b.txt) and a file reads "text".
// Folder watches stay pending like the app's: until they are cancelled or their folder changes.
async function panel(statuses: Record<string, Answer<{ files: string[] }>[]> = {}, reads: Record<string, Answer<{ text: string }>[]> = {}) {
  const at = { cwd: "/repo-a", machine: null as string | null, refused: 0 };
  const asked: string[] = [];
  const watching = new Set<Watch>();
  const Changes = new Function("View", "InputState", "spaces", "panes", "focused", "exec", "readTextFile", "watchFolder",
    `${source.replace(/^import .*;\n/gm, "").replace("export default class Changes", "class Changes")}; return Changes;`)(
    class {},
    { new: () => ({ on() {}, value: () => "", set_value() {} }) },
    () => [{ active: true, folder: at.cwd, machine: at.machine }], () => [], () => null,
    async (_git: string, args: string[], { cwd, on }: { cwd: string; on: string | null }) => {
      if (on === "away") throw new Error("herdr.exec: away is not connected");
      const repo = /^\/repo-\w+/.exec(cwd)?.[0];
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
    (root: string) => {
      let settle!: (paths: string[]) => void;
      const refused = at.refused-- > 0;
      const watch: Watch = Object.assign(refused ? Promise.reject(new Error("already watching 64 folders")) : new Promise<string[]>(resolve => { settle = resolve; }), {
        root,
        cancel() { if (watching.delete(watch)) settle([]); },
        change() { if (watching.delete(watch)) settle([root]); },
      });
      if (!refused) watching.add(watch);
      return watch;
    },
  );
  const view = new Changes();
  const cx = { focus_handle: () => ({}), timer: { every() {} }, notify() {}, spawn(task: (cx: unknown) => Promise<unknown>) { task(cx); } };
  view.init({}, cx);
  await settled();
  const show = async (cwd: string, machine: string | null = null) => { Object.assign(at, { cwd, machine }); view.refresh(cx); await settled(); };
  return {
    view, cx, at, show, asked, watching,
    roots: () => [...watching].map(watch => watch.root),
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

test("Changes holds one folder watch while the focused pane moves through 75 quiet repos", async () => {
  const h = await panel();
  for (let i = 0; i < 75; i++) {
    await h.show(`/repo-${i}`);
    expect(h.roots()).toEqual([`/repo-${i}`]);
  }
});

test("Changes keeps a repo's watch while the pane moves inside it", async () => {
  const h = await panel();
  const [watch] = h.watching;
  await h.show("/repo-a/src");
  expect(h.watching.size).toBe(1);
  expect(h.watching.has(watch)).toBe(true);
  const checked = h.asked.length;
  watch.change();
  await settled();
  expect(h.asked.length).toBe(checked + 1);
  expect(h.roots()).toEqual(["/repo-a"]);
});

test("Changes releases its watch when the pane leaves for a folder it does not watch", async () => {
  const h = await panel();
  await h.show("/plain");
  expect(h.roots()).toEqual([]);
  await h.show("/repo-b");
  expect(h.roots()).toEqual(["/repo-b"]);
  // A remote space's repo is polled, not watched.
  await h.show("/repo-b", "vm");
  expect(h.roots()).toEqual([]);
});

test("Changes releases its watch when the pane leaves for a remote space it cannot reach", async () => {
  const h = await panel();
  await h.show("/repo-a", "away");
  expect(h.roots()).toEqual([]);
});

test("Changes asks again on its next refresh for a watch the app refused", async () => {
  const h = await panel();
  h.at.refused = 1;
  await h.show("/repo-b");
  expect(h.roots()).toEqual([]);
  await h.show("/repo-b");
  expect(h.roots()).toEqual(["/repo-b"]);
});

test("Changes keeps a single watch on a repo it left and came back to before the cancelled wait settled", async () => {
  const h = await panel();
  h.view.watch("/repo-b", h.cx);
  h.view.watch("/repo-a", h.cx);
  await settled();
  expect(h.roots()).toEqual(["/repo-a"]);
  const checked = h.asked.length;
  [...h.watching][0].change();
  await settled();
  expect(h.asked.length).toBe(checked + 1);
  expect(h.roots()).toEqual(["/repo-a"]);
});
