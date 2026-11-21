const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const mod = path.join(__dirname, "../defaults/mods/workspace-status");
const source = fs.readFileSync(path.join(mod, "usage.js"), "utf8")
  .replace(/^import .*;\n/gm, "").replace(/^export /gm, "");
const explicit = "49F35021-99A9-454A-A029-8EEDDDE44C6B";
const bound = "f5655cd7-6208-47a3-a710-c6a003e86538";

function element() {
  const children = [];
  const node = new Proxy({ children }, {
    get(target, method) {
      if (method in target) return target[method];
      return value => {
        if (method === "child") children.push(value);
        if (method === "on_click") target.click = value;
        return node;
      };
    },
  });
  return node;
}

function renderedText(node) {
  return typeof node === "string" ? [node] : node.children.flatMap(renderedText);
}

function statusBar(terminal, providers, response = { windows: [] }) {
  let dialog;
  const requests = [];
  const api = {
    div: element, v_flex: element, h_flex: element,
    focused: () => ({ terminal: terminal.address }),
    terminals: () => [terminal],
    appHome: () => "/app",
    shared: () => providers,
    exec: async (_command, [, action, provider, ...selection]) => {
      requests.push([action, provider, ...selection]);
      return { code: 0, stdout: JSON.stringify(response) };
    },
    openOverlay: ({ props }) => { dialog = props; },
    theme: () => ({}),
  };
  const { Usage, details } = new Function(...Object.keys(api), source + "\nreturn { Usage, details };")(...Object.values(api));
  const usage = new Usage();
  return {
    requests,
    async refresh() {
      const tasks = [];
      usage.refresh({ spawn: run => tasks.push(run({ notify() {} })) });
      await Promise.all(tasks);
    },
    button: compact => renderedText(usage.button(compact)),
    openDetails: () => usage.button(false).click(),
    details: () => renderedText(details(dialog)),
  };
}

const selectionCases = [
  ["Codex resume", "codex", ["codex", "resume", explicit], explicit],
  ["Cursor resume", "cursor", ["cursor-agent", "--resume", explicit], explicit],
  ["Agy conversation", "antigravity", ["agy", "--conversation", explicit], explicit],
  ["Claude resume", "claude", ["claude", "--resume", explicit], explicit],
  ["Claude session ID", "claude", ["claude", "--session-id", explicit], explicit],
  ["inline resume ID", "cursor", ["cursor-agent", "--resume=" + explicit], explicit],
  ["inline resume with extra equals", "cursor", ["cursor-agent", "--resume=" + explicit + "=extra"], bound],
  ["bare resume", "cursor", ["cursor-agent", "--resume"], bound],
  ["resume followed by a flag", "claude", ["claude", "--resume", "--verbose"], bound],
  ["non-UUID conversation", "antigravity", ["agy", "--conversation", "conversation-name"], bound],
  ["non-UUID Codex thread without a binding", "codex", ["codex", "resume", "thread-name"], "", { labels: {} }],
  ["empty foreground arguments", "cursor", [], explicit, { argv: ["cursor-agent", "--resume", explicit] }],
];

for (const [name, program, argv, expected, overrides] of selectionCases) {
  test(`selects the session for ${name}`, async () => {
    const provider = program === "antigravity" ? "agy" : program;
    const bar = statusBar({
      address: "t1",
      program: { key: program },
      foreground: { argv },
      labels: { "workspace.usage": provider + "/" + bound },
      ...overrides,
    }, [provider]);
    await bar.refresh();
    assert.deepEqual(bar.requests, [["read", provider, "t1", expected]]);
  });
}

test("compact usage keeps the focused session visible when quotas are unavailable", async () => {
  const bar = statusBar({ address: "t1", program: { key: "claude" } }, ["claude"], {
    windows: [], session: { id: bound, model: "Fixture model", turns: 2 },
  });
  await bar.refresh();
  assert.deepEqual(bar.button(true), ["Fixture model · 2 turns"]);
});

test("disabling a provider hides its cached session in the button and open details", async () => {
  const providers = ["claude"];
  const bar = statusBar({ address: "t1", program: { key: "claude" } }, providers, {
    windows: [], session: { id: bound, model: "Fixture model", inputTokens: 12 },
  });
  await bar.refresh();
  bar.openDetails();
  assert.deepEqual(bar.button(false), ["Fixture model"]);
  assert.ok(bar.details().includes("Input · 12"));
  providers.pop();
  assert.deepEqual(bar.button(false), ["Usage"]);
  assert.ok(bar.details().every(text => !text.includes("Fixture model") && !text.includes("Input · 12")));
});

test("bind labels only UUID-shaped session IDs from argv or hook input", {
  skip: process.platform === "win32" && "the fixture uses a Unix executable script",
}, t => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "status-bar-usage-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const labels = path.join(dir, "labels");
  fs.writeFileSync(path.join(dir, "hc"), '#!/bin/sh\nprintf "%s\\n" "$@" >> "$USAGE_TEST_LABELS"\n', { mode: 0o700 });
  const env = { ...process.env, PATH: dir, HC_TERMINAL_ID: "t1", USAGE_TEST_LABELS: labels };
  const bind = (args, input) => spawnSync(process.execPath, [path.join(mod, "usage.cjs"), "bind", "codex", ...args], { env, input });
  assert.equal(bind(["thread-name"]).status, 1);
  assert.equal(bind([], JSON.stringify({ session_id: "--verbose" })).status, 1);
  assert.equal(bind([], JSON.stringify({ session_id: [bound] })).status, 1);
  assert.equal(fs.existsSync(labels), false);
  assert.equal(bind([explicit]).status, 0);
  assert.equal(bind([], JSON.stringify({ session_id: bound })).status, 0);
  assert.equal(fs.readFileSync(labels, "utf8"),
    `label\nt1\nworkspace.usage=codex/${explicit}\nlabel\nt1\nworkspace.usage=codex/${bound}\n`);
});
