import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

class Element {
  nodes: any[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child") target.nodes.push(args[0]);
        else if (key === "children") target.nodes.push(...args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}

function textOf(node: any): string {
  if (typeof node === "string") return node;
  return (node?.nodes ?? []).map(textOf).join(" ");
}

const source = await readFile(new URL("../defaults/mods/toast/main.js", import.meta.url), "utf8");
const element = () => new Element();

let testProps: any = {};
let testSpaces: any[] = [];
let testPanes: any[] = [];
let testAgents: any[] = [];
let testTerminals: any[] = [];

const values: Record<string, any> = {
  View: class {}, div: element, image: element, v_flex: element, h_flex: element,
  agents: () => testAgents, panes: () => testPanes, spaces: () => testSpaces, terminals: () => testTerminals,
  props: () => testProps, closeOverlay() {}, run() {}, theme: () => ({}),
};
const Toast = new Function(
  ...Object.keys(values),
  source.replace(/^import .*;\n/gm, "").replace("export default class Toast", "return class Toast"),
)(...Object.values(values));

const opencode = { key: "opencode", name: "OpenCode", agent: true };
const codex = { key: "codex", name: "Codex", agent: true };

function reset() {
  testProps = {};
  testSpaces = [];
  testPanes = [];
  testAgents = [];
  testTerminals = [];
}

test("agent alert toast replaces unnamed tab number with the agent's live session title", () => {
  reset();
  testSpaces = [{ id: "s1", name: "infra", tabs: [{ id: "tab13", name: "13", named: false }] }];
  testPanes = [{ pane: "p13", terminal: "t13", tab: "tab13", title: "OC | ssv2 oc51", program: opencode }];
  testAgents = [{ address: "t13", pane: "p13", tab: "tab13", title: "OC | ssv2 oc51", program: opencode }];
  testTerminals = [{ address: "t13", state: "live" }];
  testProps = {
    title: "OpenCode finished",
    body: "infra / 13",
    level: "info",
    action: { name: "agent.focus", args: { terminal: "t13" } },
  };
  const view = new Toast();
  const out = textOf(view.render());
  expect(out).toContain("OpenCode finished");
  expect(out).toContain("infra / OC | ssv2 oc51");
  expect(out).not.toContain("infra / 13");
});

test("split tab alert resolves the notified pane's own session title and caches it if the tab closes", () => {
  reset();
  testSpaces = [{ id: "s1", name: "infra", tabs: [{ id: "tab6", name: "6", named: false }] }];
  testPanes = [
    { pane: "p6a", terminal: "t6a", tab: "tab6", title: "OC | first session", program: opencode },
    { pane: "p6b", terminal: "t6b", tab: "tab6", title: "[ ! ] Action Required | deploy fix", program: codex },
  ];
  testTerminals = [{ address: "t6a", state: "live" }, { address: "t6b", state: "live" }];
  testProps = {
    title: "Codex needs you",
    body: "infra / 6",
    level: "error",
    action: { name: "agent.focus", args: { terminal: "t6b" } },
  };
  const view = new Toast();
  expect(textOf(view.render())).toContain("infra / Action Required | deploy fix");

  // If the tab closes while the toast is still visible, keep the resolved label.
  testSpaces = [];
  testPanes = [];
  expect(textOf(view.render())).toContain("infra / Action Required | deploy fix");
});

test("sub-agent alert resolves through its parent terminal", () => {
  reset();
  testSpaces = [{ id: "s1", name: "infra", tabs: [{ id: "tab9", name: "9", named: false }] }];
  testPanes = [{ pane: "p9", terminal: "t9", tab: "tab9", title: "OC | parent task", program: opencode }];
  testTerminals = [{ address: "t9", state: "live" }, { address: "t99", state: "live", parent: "t9" }];
  testProps = {
    title: "OpenCode finished",
    body: "infra / 9",
    level: "info",
    action: { name: "agent.focus", args: { terminal: "t99" } },
  };
  const view = new Toast();
  expect(textOf(view.render())).toContain("infra / OC | parent task");
});

test("manually named tabs, untitled agents, and non-agent toasts keep their original body", () => {
  reset();
  testSpaces = [
    {
      id: "s1",
      name: "infra",
      tabs: [
        { id: "tab1", name: "fix login", named: true },
        { id: "tab2", name: "2", named: false },
      ],
    },
  ];
  testPanes = [
    { pane: "p1", terminal: "t1", tab: "tab1", title: "OC | other title", program: opencode },
    { pane: "p2", terminal: "t2", tab: "tab2", title: "~/infra", program: opencode },
  ];
  testTerminals = [{ address: "t1", state: "live" }, { address: "t2", state: "live" }];

  testProps = { title: "OpenCode finished", body: "infra / fix login", action: { name: "agent.focus", args: { terminal: "t1" } } };
  expect(textOf(new Toast().render())).toContain("infra / fix login");

  testProps = { title: "OpenCode finished", body: "infra / 2", action: { name: "agent.focus", args: { terminal: "t2" } } };
  expect(textOf(new Toast().render())).toContain("infra / 2");

  testProps = { title: "Done", body: "Build complete", action: { name: "app.help" } };
  expect(textOf(new Toast().render())).toContain("Build complete");
});
