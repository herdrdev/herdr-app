import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../defaults/mods/changes/main.js", import.meta.url), "utf8");
const menuSource = await readFile(new URL("../defaults/mods/new-menu/main.js", import.meta.url), "utf8");
const locationSource = source.slice(source.indexOf("function location()"), source.indexOf("\nconst key"));
const rowsSource = source.slice(source.indexOf("const untab ="), source.indexOf("\nexport default class"));
const diffRows = new Function(`${rowsSource}; return diffRows;`)();

test("Changes tabs keep their repository and machine when focus moves", () => {
  for (const machine of [null, "server"]) {
    const space = { id: machine ? "server/space" : "space", active: true, folder: "/original", machine };
    let spaces = [space], focus: any = { space: space.id, pane: "shell", cwd: "/current/project" };
    let panes = [{ pane: "shell", cwd: "/current/project" }];
    let props: any = {}, command: any;
    const NewMenu = new Function("View", "spaces", "panes", "focused", "run",
      menuSource.replace(/^import .*;\n/gm, "").replace("export default class NewMenu", "return class NewMenu"))(
      class {}, () => spaces, () => panes, () => focus, (name: string, args: any) => { command = { name, args }; });
    const location = new Function("spaces", "panes", "focused", "props", `${locationSource}; return location;`)(
      () => spaces, () => panes, () => focus, () => props);

    new NewMenu().openChanges();
    expect(command).toEqual({ name: "tab.new", args: { name: "Changes", space: space.id, mod: "changes",
      props: { folder: "/current/project", on: machine, space: space.id } } });
    props = command.args.props;
    focus = { space: space.id, pane: "changes", cwd: null };
    panes = [];
    expect(location()).toEqual({ cwd: "/current/project", on: machine });
    spaces = [{ id: "other", active: true, folder: "/other", machine: null }];
    focus = { space: "other", pane: "other-shell", cwd: "/other" };
    expect(location()).toEqual({ cwd: "/current/project", on: machine });
  }
});

test("Changes tabs keep a focused terminal block's machine in a local space", () => {
  let command: any;
  const NewMenu = new Function("View", "spaces", "panes", "focused", "run",
    menuSource.replace(/^import .*;\n/gm, "").replace("export default class NewMenu", "return class NewMenu"))(
    class {}, () => [{ id: "local", active: true, folder: "/original", machine: null }], () => [],
    () => ({ space: "local", pane: null, terminal: "server/t1", cwd: "/remote/project" }),
    (name: string, args: any) => { command = { name, args }; });
  new NewMenu().openChanges();
  expect(command).toEqual({ name: "tab.new", args: { name: "Changes", space: "local", mod: "changes",
    props: { folder: "/remote/project", on: "server", space: "local" } } });
});

test("Changes without folder props follows focus and the space folder fallback", () => {
  let focus: any = { pane: "shell", cwd: "/current" };
  const location = new Function("spaces", "panes", "focused", "props", `${locationSource}; return location;`)(
    () => [{ active: true, folder: "/original", machine: "server" }], () => [], () => focus, () => ({}));
  expect(location()).toEqual({ cwd: "/current", on: "server" });
  focus = { pane: "changes", cwd: null };
  expect(location()).toEqual({ cwd: "/original", on: "server" });
  focus = { pane: null, terminal: "other/t1", cwd: "/remote" };
  expect(location()).toEqual({ cwd: "/remote", on: "other" });
});

test("Changes keeps tab indentation, to the next 4-column stop", () => {
  const rows = diffRows(["@@ -1,2 +1,2 @@", "-\tif x {", "+\t\tif x {", " a\tb", "+    spaces"]);
  expect(rows.map((r: { text: string }) => r.text)).toEqual(["@@ -1,2 +1,2 @@", "    if x {", "        if x {", "a   b", "    spaces"]);
});
