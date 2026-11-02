import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/agents/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Agents", "class Agents");

function harness(pins: string[], snapshots = {}) {
  const data = new Map([["pins", JSON.stringify(pins)]]);
  const writes: unknown[] = [];
  const storage = { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => data.set(k, v), removeItem: (k: string) => data.delete(k) };
  const element = new Function(`
    return function element(asset) {
      const node = { items: [], handlers: {}, asset };
      const proxy = new Proxy(node, { get(target, key) {
        if (key in target) return target[key];
        return (...args) => {
          if (key === "when") { if (args[0]) args[1](proxy); }
          else if (key === "child") node.items.push(args[0]);
          else if (key === "children") node.items.push(...args[0]);
          else if (key === "id") node.name = args[0];
          else if (key === "on_click" || key === "on_hover") node.handlers[key] = args[0];
          else if (key === "tooltip") node.tip = args[0];
          else if (key === "border") node.borderWidth = args[0];
          return proxy;
        };
      } });
      return proxy;
    };
  `)();
  const order: string[][] = [];
  const api = {
    agents: () => [], terminals: () => [], panes: () => [], spaces: () => [],
    focused: () => null, theme: () => ({ dim: "#888" }), shared: () => null,
    layout: () => ({}), setAgentOrder: (addresses: string[]) => order.push(addresses),
    div: element, h_flex: element, v_flex: element, svg: element, image: element,
    ...snapshots,
  };
  const Agents = new Function("View", "localStorage", "props", "run", ...Object.keys(api), `${source}; return Agents;`)(
    class {}, storage, () => ({}), (...args: unknown[]) => writes.push(args),
    ...Object.values(api),
  );
  const agents = new Agents();
  agents.init({}, { timer: { every() {} } });
  return { agents, data, writes, order };
}

test("legacy pins migrate once listed, retaining offline pins until their labels arrive", () => {
  const { agents, data, writes } = harness(["t1", "offline/t2"]);
  agents.migratePins([]);
  expect(JSON.parse(data.get("pins")!)).toEqual(["t1", "offline/t2"]);
  const local = { address: "t1", state: "live", labels: {} };
  agents.migratePins([local]);
  agents.migratePins([local]);
  expect(writes).toEqual([["terminal.pin", { terminal: "t1", pinned: true }]]);
  expect(JSON.parse(data.get("pins")!)).toEqual(["t1", "offline/t2"]);
  agents.migratePins([{ ...local, labels: { "herdr.pinned": "1" } }]);
  expect(JSON.parse(data.get("pins")!)).toEqual(["offline/t2"]);
  const remote = { address: "offline/t2", state: "live", labels: {} };
  agents.migratePins([remote]);
  expect(writes).toHaveLength(2);
  agents.migratePins([{ ...remote, labels: { "herdr.pinned": "1" } }]);
  expect(data.has("pins")).toBe(false);
});

const omoRoot = "/Users/test/.nvm/versions/node/v24/lib/node_modules/omo-ai";
const omoForeground = ["/Users/test/.bun/bin/bun", `${omoRoot}/node_modules/@code-yeongyu/senpi/dist/bundle/cli.js`, "--extension", `${omoRoot}/plugin`];
const live = (address: string, argv: readonly string[]) => ({
  address, state: "live", foreground: { argv }, argv: ["omo"], status: null,
  reported: false, labels: {}, program: { key: "bun", name: "Bun" },
});
const pane = (terminal: string, tab = "tab") => ({ terminal, pane: `pane-${terminal}`, tab, title: terminal, focused: false });
const space = (id = "space", tab = "tab") => ({ id, name: id, tabs: [{ id: tab, index: 1, name: "1", named: false }] });
const descendants = (node: { readonly items?: readonly unknown[] }): unknown[] =>
  [node, ...(node.items ?? []).flatMap((item) => typeof item === "object" && item !== null ? descendants(item) : [item])];

test("live omo and gjc panes render unreported rows with focus and pin commands", () => {
  const { agents, writes, order } = harness([], {
    terminals: () => [live("omo", omoForeground), { ...live("remote/gjc", ["/home/test/bin/gjc"]), labels: { "herdr.pinned": "1" } }],
    panes: () => [pane("omo"), { ...pane("omo"), pane: "duplicate-pane" }, pane("remote/gjc", "remote/tab")],
    spaces: () => [space(), space("remote/space", "remote/tab")],
  });
  agents.hovered = "omo";
  const nodes = descendants(agents.render({}));
  const row = nodes.find((node) => typeof node === "object" && node !== null && "name" in node && node.name === "ag-omo");
  const pin = nodes.find((node) => typeof node === "object" && node !== null && "name" in node && node.name === "pin-omo");
  expect(order).toEqual([["remote/gjc", "omo"]]);
  expect(nodes).toContain("Unreported");
  expect(nodes.filter((node) => node === "?")).toHaveLength(2);
  // The real render's callbacks, not a separate detector test seam.
  if (!row || !pin) throw new TypeError("Missing live sidebar controls");
  Reflect.get(row, "handlers").on_click();
  Reflect.get(pin, "handlers").on_click({}, { stop_propagation() {}, notify() {} });
  expect(writes).toEqual([
    ["pane.focus", { pane: "pane-omo" }],
    ["terminal.pin", { terminal: "omo", pinned: true }],
  ]);
});

test.each([
  ["direct omo", ["omo"], "live", true],
  ["native gjc", ["/home/test/bin/gjc"], "live", true],
  ["omo launcher", ["/usr/bin/node", `${omoRoot}/bin/omo.js`], "live", true],
  ["observed omo", omoForeground, "live", true],
  ["unrelated bun", ["bun", "/tmp/cli.js", "--extension", `${omoRoot}/plugin`], "live", false],
  ["wrong plugin", ["bun", omoForeground[1], "--extension", "/tmp/plugin"], "live", false],
  ["unrelated script", ["node", "/tmp/omo.js"], "live", false],
  ["path argument", ["node", "/tmp/task.js", ...omoForeground.slice(1)], "live", false],
  ["shell after exit", ["/bin/zsh"], "live", false],
  ["shell command argument", ["sh", "-c", "omo"], "live", false],
  ["empty foreground", [], "live", false],
  ["exited terminal", ["gjc"], "exited", false],
  ["unrelated native", ["/tmp/gjc-helper"], "live", false],
  ["Senpi without extension", omoForeground.slice(0, 2), "live", false],
  ["different package", ["bun", "/tmp/senpi/dist/bundle/cli.js", "--extension", `${omoRoot}/plugin`], "live", false],
])("%s is admitted only for a live identified foreground", (_name, argv, state, admitted) => {
  const terminal = { ...live("t", argv), state };
  const { agents, order } = harness([], { terminals: () => [terminal], panes: () => [pane("t")], spaces: () => [space()] });
  agents.render({});
  expect(order).toEqual([admitted ? ["t"] : []]);
});

test("a missing foreground never reuses the original launch command", () => {
  const { agents, order } = harness([], {
    terminals: () => [{ ...live("omo", ["omo"]), foreground: null }],
    panes: () => [pane("omo")], spaces: () => [space()],
  });
  agents.render({});
  expect(order).toEqual([[]]);
});

test("the narrow rail keeps an unreported marker and pane focus", () => {
  const { agents, writes } = harness([], {
    terminals: () => [live("gjc", ["gjc"])], panes: () => [pane("gjc")], spaces: () => [space()],
    layout: () => ({ id: "sidebar", size: { px: 76 } }),
  });
  const nodes = descendants(agents.render({}));
  const row = nodes.find((node) => typeof node === "object" && node !== null && "name" in node && node.name === "ag-gjc");
  expect(nodes).toContain("?");
  if (!row) throw new TypeError("Missing compact sidebar row");
  Reflect.get(row, "handlers").on_click();
  expect(writes).toEqual([["pane.focus", { pane: "pane-gjc" }]]);
});

test("omo and gjc registry defaults supply launch commands and shipped generic icons", async () => {
  const registry = JSON.parse(await readFile(new URL("../defaults/programs.json", import.meta.url), "utf8"));
  for (const key of ["omo", "gjc"]) {
    expect(registry.programs[key].run).toEqual([key]);
    expect(registry.programs[key].icon).toBe("icons/agent.svg");
    expect(await readFile(new URL(`../defaults/${registry.programs[key].icon}`, import.meta.url), "utf8")).toContain("<svg");
  }
});

test("reported Claude Codex and omo status wins while grouped live rows stay in their space", () => {
  const reported = [
    { ...pane("claude"), address: "claude", status: "working", space: "space", space_name: "space", seq: 3, program: { key: "claude" } },
    { ...pane("omo"), address: "omo", status: "blocked", space: "space", space_name: "space", seq: 4, program: { key: "omo" } },
    { ...pane("codex", "other-tab"), address: "codex", status: "idle", space: "other", space_name: "other", seq: 2, program: { key: "codex" } },
  ];
  const { agents, order } = harness([], {
    agents: () => reported, terminals: () => [live("omo", omoForeground), live("gjc", ["gjc"]), live("unplaced", ["omo"])],
    panes: () => [...reported, pane("gjc")], spaces: () => [space(), space("other", "other-tab")],
    shared: () => "grouped",
  });
  const nodes = descendants(agents.render({}));
  expect(order).toEqual([["claude", "omo", "gjc", "codex"]]);
  expect(nodes.filter((node) => node === "?")).toHaveLength(1);
  expect(nodes).toContain("Needs you");
  expect(nodes.filter((node) => typeof node === "object" && node !== null && "asset" in node && node.asset === "mods/workspace-chrome/spinner.svg")).toHaveLength(1);
  expect(nodes.filter((node) => typeof node === "object" && node !== null && "borderWidth" in node && node.borderWidth === 1.5)).toHaveLength(1);
  expect(nodes.filter((node) => typeof node === "object" && node !== null && "name" in node && node.name === "sp-space")).toHaveLength(1);
});

test("already shared pins need no write and pending writes survive restart", () => {
  expect(harness([]).data.has("pins")).toBe(false);
  const { agents, data, writes } = harness(["t1"]);
  agents.migratePins([{ address: "t1", state: "live", labels: { "herdr.pinned": "1" } }]);
  expect(writes).toHaveLength(0);
  expect(data.has("pins")).toBe(false);
  const retry = harness(["t2"]);
  retry.agents.migratePins([{ address: "t2", state: "live", labels: {} }]);
  expect(JSON.parse(retry.data.get("pins")!)).toEqual(["t2"]);
  const restarted = harness(JSON.parse(retry.data.get("pins")!));
  restarted.agents.migratePins([{ address: "t2", state: "live", labels: {} }]);
  expect(restarted.writes).toHaveLength(1);
});

test("an unnamed tab's row reads its terminal's title, not the tab number", () => {
  const title = new Function("View", "localStorage", "props", "run", `${source}; return title;`)(class {}, {}, () => ({}), () => {});
  const claude = { program: { name: "claude", key: "claude" }, title: "Herdr session names" };
  expect(title({ name: "3", named: false }, claude)).toBe("Herdr session names");
  expect(title({ name: "claude", named: true }, claude)).toBe("Herdr session names");
  expect(title({ name: "fix login", named: true }, claude)).toBe("fix login");
  expect(title({ name: "3", named: false }, { ...claude, title: "claude" })).toBe("3");
});
