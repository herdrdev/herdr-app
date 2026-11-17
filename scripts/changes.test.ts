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

// The panel on a machine where every /repo-N is a clean git repo. Folder watches
// stay pending like the app's: until they are cancelled or their folder changes.
async function panel() {
  const at = { cwd: "/repo-0", machine: null as string | null, refused: 0 };
  const watching = new Set<Watch>();
  const count = { status: 0 };
  const Changes = new Function("View", "InputState", "spaces", "panes", "focused", "exec", "watchFolder",
    `${source.replace(/^import .*;\n/gm, "").replace("export default class Changes", "class Changes")}; return Changes;`)(
    class {},
    { new: () => ({ on() {}, value: () => "", set_value() {} }) },
    () => [{ active: true, folder: at.cwd, machine: at.machine }], () => [], () => null,
    async (_git: string, args: string[], { cwd }: { cwd: string }) => {
      if (args.includes("--show-toplevel")) {
        const repo = /^\/repo-\d+/.exec(cwd)?.[0];
        return repo ? { code: 0, stdout: `${repo}\n`, stderr: "" } : { code: 128, stdout: "", stderr: "not a git repository" };
      }
      if (args.includes("status")) count.status++;
      return { code: 0, stdout: "", stderr: "" };
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
  const show = async (cwd: string, machine: string | null = null) => {
    Object.assign(at, { cwd, machine });
    view.refresh(cx);
    await settled();
  };
  view.init({}, cx);
  await settled();
  return { view, cx, at, watching, count, show, roots: () => [...watching].map(watch => watch.root) };
}

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
  await h.show("/repo-0/src");
  expect(h.watching.size).toBe(1);
  expect(h.watching.has(watch)).toBe(true);
  const checked = h.count.status;
  watch.change();
  await settled();
  expect(h.count.status).toBe(checked + 1);
  expect(h.roots()).toEqual(["/repo-0"]);
});

test("Changes releases its watch when the pane leaves for a folder it does not watch", async () => {
  const h = await panel();
  await h.show("/not-a-repo");
  expect(h.roots()).toEqual([]);
  await h.show("/repo-1");
  expect(h.roots()).toEqual(["/repo-1"]);
  // A remote space's repo is polled, not watched.
  await h.show("/repo-1", "vm");
  expect(h.roots()).toEqual([]);
});

test("Changes asks again on its next refresh for a watch the app refused", async () => {
  const h = await panel();
  h.at.refused = 1;
  await h.show("/repo-1");
  expect(h.roots()).toEqual([]);
  await h.show("/repo-1");
  expect(h.roots()).toEqual(["/repo-1"]);
});

test("Changes keeps a single watch on a repo it left and came back to before the cancelled wait settled", async () => {
  const h = await panel();
  h.view.watch("/repo-1", h.cx);
  h.view.watch("/repo-0", h.cx);
  await settled();
  expect(h.roots()).toEqual(["/repo-0"]);
  const checked = h.count.status;
  [...h.watching][0].change();
  await settled();
  expect(h.count.status).toBe(checked + 1);
  expect(h.roots()).toEqual(["/repo-0"]);
});
