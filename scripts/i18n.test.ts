import { expect, test } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { translatorFor } from "./i18n-fixture";

function source(relative: string) {
  return readFileSync(new URL(relative, import.meta.url), "utf8").replace(/^import .*;\r?\n/gm, "");
}

class Element {
  nodes: any[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, { get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
      if (key === "child") target.nodes.push(args[0]);
      else if (key === "children") target.nodes.push(...args[0]);
      else if (key === "when") { if (args[0]) args[1](proxy); }
      else target.calls[key] = args;
      return proxy;
    }});
    return proxy;
  }
}
function text(node: any): string {
  return typeof node === "string" ? node : (node?.nodes ?? []).map(text).join("\n");
}
function find(node: any, id: string): Element | undefined {
  if (node?.calls?.id?.[0] === id) return node;
  for (const child of node?.nodes ?? []) { const found = find(child, id); if (found) return found; }
}
const languages = () => [{ code: "en", nativeName: "English" }, { code: "zh-Hans", nativeName: "简体中文" }, { code: "zh-Hant", nativeName: "繁體中文" }];

test("language picker uses the registry and compares with the launch preference", () => {
  for (const code of ["en", "zh-Hans", "zh-Hant"]) {
    const tr = translatorFor(code);
    const values = { View: class {}, div: () => new Element(), h_flex: () => new Element(), v_flex: () => new Element(),
      tr, locale: () => code, languages, settings: () => ({ language_preference: "system" }), window: () => ({ platform: "windows" }) };
    const Settings = new Function(...Object.keys(values), source("../defaults/frame/settings/main.js").replace("export default class Settings", "return class Settings"))(...Object.values(values));
    const view = new Settings();
    view.person = { zoom: 1.5 };
    view.change = (edit: (p: any) => void) => edit(view.person);
    const first = view.languagePicker({}, {});
    expect(text(first)).toContain(languages().find(l => l.code === code)!.nativeName);
    expect(text(first)).not.toContain(tr("settings-language-restart"));
    find(first, "st-language-zh-Hant")!.calls.on_click[0]({}, {});
    expect(view.person).toEqual({ zoom: 1.5, language: "zh-Hant" });
    expect(text(view.languagePicker({}, {}))).toContain(tr("settings-language-restart"));
    expect(values.locale()).toBe(code); // changing a saved preference does not change this launch
    view.person.language = "system";
    expect(text(view.languagePicker({}, {}))).not.toContain(tr("settings-language-restart"));
  }
});

test("legacy zh-CN selects Simplified Chinese without rewriting the saved value", () => {
  const values = { View: class {}, div: () => new Element(), h_flex: () => new Element(), v_flex: () => new Element(),
    tr: translatorFor("zh-Hans"), locale: () => "zh-Hans", languages, settings: () => ({ language_preference: "zh-Hans" }), window: () => ({}) };
  const Settings = new Function(...Object.keys(values), source("../defaults/frame/settings/main.js").replace("export default class Settings", "return class Settings"))(...Object.values(values));
  const view = new Settings(); view.person = { language: "zh-CN" };
  const root = view.languagePicker({ raised: "selected", background: "normal" }, {});
  expect(find(root, "st-language-zh-Hans")!.calls.bg).toEqual(["selected"]);
  expect(text(root)).not.toContain(values.tr("settings-language-restart"));
  expect(view.person.language).toBe("zh-CN");
});

test("Chinese palette search and English keywords execute stable command IDs", () => {
  for (const code of ["zh-Hans", "zh-Hant"]) {
    const calls: any[] = [];
    const values = { View: class {}, tr: translatorFor(code), win: () => ({ platform: "windows" }), layouts: () => [],
      commands: () => [{ name: "app.settings", args: [] }, { name: "pane.go", args: [] }, { name: "tab.new", args: [] }, { name: "tab.close", args: [] }],
      run: (...args: any[]) => calls.push(args), closeOverlay() {}, props: () => ({ overlay: "test" }), localStorage: { setItem() {} } };
    const Palette = new Function(...Object.keys(values), source("../defaults/frame/palette/main.js").replace("export default class Palette", "return class Palette"))(...Object.values(values));
    const view = new Palette(); view.recent = [];
    for (const query of [values.tr("palette-settings"), "preferences", "app.settings"]) {
      view.query = query;
      const result = view.rows().find((r: any) => r.e.name === "app.settings");
      expect(result).toBeDefined(); view.go(result.e);
      expect(calls.at(-1)).toEqual(["app.settings", {}]);
    }
    view.query = values.tr("common-direction-left");
    expect(view.rows().some((r: any) => r.e.name === "pane.go" && r.e.args.direction === "left")).toBe(true);
    for (const [query, command] of [["New tab", "tab.new"], ["Close tab", "tab.close"]]) {
      view.query = query;
      expect(view.rows().some((r: any) => r.e.name === command)).toBe(true);
    }
  }
});

test("catalogs expose the same IDs and variables", () => {
  function shape(code: string) {
    const directory = new URL(`../defaults/locales/${code}/`, import.meta.url);
    const result: Record<string, string[]> = {};
    for (const file of readdirSync(directory)) {
      const contents = readFileSync(new URL(file, directory), "utf8");
      for (const match of contents.matchAll(/^([\w-]+) = (.*(?:\n[ \t]+.*)*)/gm)) {
        expect(result[match[1]]).toBeUndefined();
        result[match[1]] = [...new Set([...match[2].matchAll(/\$([\w-]+)/g)].map(m => m[1]))].sort();
      }
    }
    return result;
  }
  const en = shape("en");
  expect(Object.keys(en).length).toBeGreaterThan(600);
  expect(shape("zh-Hans")).toEqual(en);
  expect(shape("zh-Hant")).toEqual(en);
});

test("translated Mines difficulty labels retain the original score and action keys", () => {
  for (const code of ["en", "zh-Hans", "zh-Hant"]) {
    const dialogs: any[] = [];
    const values = { tr: translatorFor(code), C: {}, openDialog: (...args: any[]) => dialogs.push(args) };
    const Mines = new Function(...Object.keys(values), source("../defaults/mods/win98/mines.js").replace("export default class Mines", "return class Mines"))(...Object.values(values));
    const view = new Mines();
    view.options = { level: "beginner", best: { beginner: 12, intermediate: 34, expert: 56 } };
    view.request("best");
    for (const [level, time] of Object.entries(view.options.best)) {
      expect(dialogs[0][1].text).toContain(values.tr("win98-seconds", { value1: values.tr(`win98-${level}`), value2: Number(time) }));
    }
    const beginner = view.menuItems(false).find((item: any) => item.label === values.tr("win98-beginner"));
    expect(beginner.act.shared[1].cmd).toBe("beginner");
    expect(beginner.radio).toBe(true);
  }
});
