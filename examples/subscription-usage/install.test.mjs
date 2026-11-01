import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { install, appHome } from "./install.mjs";

async function scratch(t) {
  const root = await mkdtemp(join(tmpdir(), "herdr-quota-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  return root;
}

test("home resolution respects Herdr, XDG and default homes", () => {
  assert.equal(appHome({ HERDR_APP_HOME: "/custom" }, "/user"), resolve("/custom"));
  assert.equal(appHome({ XDG_CONFIG_HOME: "/xdg" }, "/user"), resolve("/xdg/herdr-app"));
  assert.equal(appHome({}, "/user"), resolve("/user/.config/herdr-app"));
});

test("install preserves existing settings, keys, layouts and mods byte for byte", async t => {
  const home = await scratch(t);
  await mkdir(join(home, "mods", "agents"), { recursive: true });
  await mkdir(join(home, "layouts"));
  const protectedFiles = ["keys.json", "settings.json", "shared.json", "layouts/personal.json", "mods/agents/main.js"];
  for (const file of protectedFiles) await writeFile(join(home, file), `existing ${file}\r\n`);
  const target = await install({ home });
  assert.deepEqual((await readdir(target)).sort(), ["main.js", "mod.json", "package.json", "query.mjs", "quota.js"]);
  for (const file of protectedFiles) assert.equal(await readFile(join(home, file), "utf8"), `existing ${file}\r\n`);
  const main = await readFile(join(target, "main.js"), "utf8");
  await assert.rejects(install({ home }), /Already exists/);
  assert.equal(await readFile(join(target, "main.js"), "utf8"), main);
});

test("rename avoids a collision and rewrites only the mod helper path", async t => {
  const home = await scratch(t);
  await install({ home });
  const target = await install({ home, name: "my-quota" });
  const main = await readFile(join(target, "main.js"), "utf8");
  assert.ok(main.includes("/mods/my-quota/query.mjs"));
  assert.ok(!main.includes("/mods/subscription-usage/query.mjs"));
  // Renamed copies still share rate limiting and normalized cached values.
  assert.ok(main.includes('"subscription-usage.attempt"'));
  for (const name of ["../outside", "a/b", "a\\b", "", "..", "A", "x\"", "x".repeat(65)]) {
    await assert.rejects(install({ home, name }), /mod name/);
  }
  assert.deepEqual((await readdir(join(home, "mods"))).sort(), ["my-quota", "subscription-usage"]);
});

test("CLI supports paths with spaces, rejects unknown options and leaves keys absent", async t => {
  const home = join(await scratch(t), "app home");
  const cli = fileURLToPath(new URL("./install.mjs", import.meta.url));
  const run = (...args) => spawnSync(process.execPath, [cli, ...args], { encoding: "utf8" });
  assert.equal(run("--home", home, "--name", "usage-test").status, 0);
  assert.equal(run("--home", home, "--name", "usage-test").status, 1);
  assert.equal(run("--force").status, 1);
  assert.deepEqual(await readdir(home), ["mods"]);
});
