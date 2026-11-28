import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";
const source = (await readFile(new URL("../defaults/mods/menu/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Menu", "class Menu").replace(/^export /gm, "");

type Reply = { code: number; stdout: string; stderr: string };
// A space menu opened with `menuProps`; `exec` answers git's reads.
function harness(menuProps: Record<string, unknown>, exec: (args: string[]) => Promise<Reply>) {
  const calls: string[][] = [];
  const tasks: Promise<unknown>[] = [];
  const space = { id: "s1", name: "api", folder: "/work/api", machine: "remote", tabs: [] };
  const { Menu, items } = new Function("View", "div", "svg", "v_flex", "h_flex", "run", "props", "closeOverlay", "openOverlay", "theme", "keys", "terminals", "spaces", "panes", "exec", `${source}; return { Menu, items };`)(
    class {}, ...Array(4).fill(() => null), () => {}, () => menuProps, () => {}, () => {}, () => ({}), () => [], () => [], () => [space], () => [],
    (_command: string, args: string[]) => { calls.push(args); return exec(args); },
  );
  const view = new Menu();
  const cx = { focus_handle: () => ({ focus() {} }), notify() {}, spawn(fn: (cx: unknown) => Promise<unknown>) { tasks.push(fn(cx)); } };
  view.init(menuProps, cx);
  const labels = () => items(menuProps, view.git).map((it: unknown[]) => it[0]);
  return { view, calls, tasks, labels };
}

const never = () => new Promise<Reply>(() => {});

test("a space menu given its checkout shows the worktree items at once, without running git", () => {
  const h = harness({ tab: "space", target: { id: "s1" }, git: "linked" }, never);
  expect(h.calls).toEqual([]);
  expect(h.labels()).toEqual(["Rename", "New worktree", "Open worktree", "Remove worktree", "Close"]);
});

test("a space menu told its folder is outside git runs no git either", () => {
  const h = harness({ tab: "space", target: { id: "s1" }, git: null }, never);
  expect(h.calls).toEqual([]);
  expect(h.labels()).toEqual(["Rename", "Close"]);
});

test("a space menu without its checkout asks git, both reads at once", async () => {
  const pending: ((r: Reply) => void)[] = [];
  const h = harness({ tab: "space", target: { id: "s1" } }, () => new Promise<Reply>((resolve) => pending.push(resolve)));
  expect(h.labels()).toEqual(["Rename", "Close"]);
  // Both reads started before either answered: one round trip, not two.
  expect(h.calls.map((a) => a.at(-1))).toEqual(["--show-toplevel", "--git-common-dir"]);
  // A main checkout: each answer must be read as the one it is.
  pending[0]({ code: 0, stdout: "/work/api\n", stderr: "" });
  pending[1]({ code: 0, stdout: "/work/api/.git\n", stderr: "" });
  await Promise.all(h.tasks);
  expect(h.view.git).toBe("main");
});
