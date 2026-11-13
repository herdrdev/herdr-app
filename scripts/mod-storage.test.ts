import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

async function harness(mod: string, values: Record<string, string>) {
  const source = (await readFile(new URL(`../defaults/mods/${mod}/main.js`, import.meta.url), "utf8"))
    .replace(/^import .*;\n/gm, "").replace(/export default class (\w+)/, "class $1").replace(/^export /gm, "");
  const name = { agents: "Agents", spaces: "Spaces", tabs: "Tabs" }[mod]!;
  const data = new Map(Object.entries(values));
  const globals = {
    View: class {}, props: () => ({}), spaces: () => [], ScrollHandle: { new: () => ({}) },
    localStorage: { getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => data.set(key, value), removeItem: (key: string) => data.delete(key) },
  };
  const Mod = new Function(...Object.keys(globals), `${source}; return ${name};`)(...Object.values(globals));
  const view = new Mod();
  view.refreshMachines = () => {};
  view.init({}, { timer: { every() {} } });
  return { view, data };
}

for (const raw of ["{{{not json", "null", "42", '"old shape"', "[]"]) {
  test(`agents recover invalid changed state: ${raw}`, async () => {
    const { view, data } = await harness("agents", { changed: raw });
    view.track([{ address: "t1", status: "working" }], 123);
    expect(JSON.parse(data.get("changed")!)).toEqual({ t1: { s: "working", at: 123 } });
  });
  test(`tabs recover invalid panel map: ${raw}`, async () => {
    const { view, data } = await harness("tabs", { panels: raw });
    view.syncPanel({ id: "s1" }, { hidden: true }, {}, []);
    expect(JSON.parse(data.get("panels")!)).toEqual({ s1: null });
  });
}

for (const raw of ["{{{not json", "null", "42", '"old shape"', "{}"]) {
  test(`agents recover invalid pins: ${raw}`, async () => {
    const { view, data } = await harness("agents", { pins: raw });
    view.migratePins([]);
    expect(view.pendingPins).toEqual([]);
    expect(data.has("pins")).toBe(false);
  });
  test(`spaces recover invalid folded machines: ${raw}`, async () => {
    const { view } = await harness("spaces", { "machines.folded": raw });
    expect([...view.folded]).toEqual(raw === '"old shape"' ? [...new Set("old shape")] : []);
  });
}

for (const raw of ["{{{not json", "null", "42", '"old shape"', "[]", '{"cloud":null}', '{"cloud":42}']) {
  test(`spaces recover invalid reconnect state: ${raw}`, async () => {
    const { view } = await harness("spaces", { "machines.reconnecting": raw });
    expect(view.reconnecting).toEqual({});
  });
}

test("valid saved state is retained and reconnects are ready to retry", async () => {
  const { view: agents } = await harness("agents", { changed: '{"t1":{"s":"idle","at":123}}', pins: '["cloud/t1"]' });
  expect(agents.changed).toEqual({ t1: { s: "idle", at: 123 } });
  expect(agents.pendingPins).toEqual(["cloud/t1"]);
  const { view: spaces } = await harness("spaces", { "machines.reconnecting": '{"cloud":{"busy":true,"removed":true}}', "machines.folded": '["cloud"]' });
  expect(spaces.reconnecting).toEqual({ cloud: { busy: false, removed: true } });
  expect([...spaces.folded]).toEqual(["cloud"]);
  const { view: tabs } = await harness("tabs", { panels: '{"s1":"files"}' });
  expect(tabs.panels).toEqual({ s1: "files" });
});
