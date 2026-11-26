import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// What a click in the Settings window writes to settings.json, without a window or a core.
const source = await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8");
// settings.json as the mod sees it through readTextFile / writeTextFile.
const disk: { raw: string | null; writes: number; fail?: string; readFail?: string; toasts: any[] } = { raw: null, writes: 0, toasts: [] };
const values: Record<string, any> = {
  View: class {},
  appHome: () => "/home",
  notify: (n: any) => { disk.toasts.push(n); },
  readTextFile: async () => {
    if (disk.readFail) throw new Error(disk.readFail);
    return disk.raw;
  },
  writeTextFile: async (_path: string, text: string, expected: string | null) => {
    disk.writes++;
    if (disk.fail) throw new Error(disk.fail);
    if (expected !== disk.raw) throw new Error("changed since read");
    disk.raw = text;
  },
};
const names = [...source.matchAll(/^import \{([^}]*)\}/gm)].flatMap(m => m[1].split(",").map(n => n.trim()));
const args = names.map(name => values[name] ?? (() => undefined));
const Settings = new Function(...names, source.replace(/^import .*;\n/gm, "").replace("export default class Settings", "return class Settings"))(...args);

/** Runs `edit` through the Settings window's change() on `raw`; the toasts it posts. */
async function change(raw: string | null, edit: (q: Record<string, any>) => void, fail?: string, readFail?: string) {
  Object.assign(disk, { raw, writes: 0, fail, readFail, toasts: [] });
  const view = new Settings();
  let done: () => void;
  const finished = new Promise<void>((resolve) => (done = resolve));
  view.change(edit, { spawn: (f: (cx: any) => Promise<void>) => f({ notify: () => done() }) });
  await finished;
  return { view, toasts: disk.toasts };
}

test("a change writes settings.json, keeping what else it holds", async () => {
  const { view, toasts } = await change('{"font_size": 15}', (q) => (q.dock_bounce = true));
  expect(JSON.parse(disk.raw!)).toEqual({ font_size: 15, dock_bounce: true });
  expect(view.person).toEqual({ font_size: 15, dock_bounce: true });
  expect(toasts).toEqual([]);
});

test("a broken settings.json is never written over, and says so", async () => {
  const broken = '{"font_size": 15, "theme": "x",}';
  const { toasts } = await change(broken, (q) => (q.dock_bounce = true));
  expect(disk.raw).toBe(broken);
  expect(disk.writes).toBe(0);
  expect(toasts).toEqual([{ title: "settings.json has errors: fix it first", level: "error" }]);
});

test("settings.json that isn't an object counts as broken", async () => {
  for (const raw of ["null", "[]", "3"]) {
    const { toasts } = await change(raw, (q) => (q.dock_bounce = true));
    expect(disk.raw).toBe(raw);
    expect(disk.writes).toBe(0);
    expect(toasts).toEqual([{ title: "settings.json has errors: fix it first", level: "error" }]);
  }
});

test("a write that keeps failing is reported, not dropped", async () => {
  const { toasts } = await change("{}", (q) => (q.dock_bounce = true), "read-only file system");
  expect(disk.writes).toBe(3);
  expect(disk.raw).toBe("{}");
  expect(toasts).toEqual([{ title: "Not saved to settings.json", body: "read-only file system", level: "error" }]);
});

test("a settings.json that can't be read, or an edit that throws, says why", async () => {
  const unread = await change("{}", (q) => (q.dock_bounce = true), undefined, "permission denied");
  expect(disk.writes).toBe(0);
  expect(unread.toasts).toEqual([{ title: "Cannot read settings.json", body: "permission denied", level: "error" }]);
  const thrown = await change("{}", () => { throw new Error("no such section"); });
  expect(disk.writes).toBe(0);
  expect(thrown.toasts).toEqual([{ title: "Cannot read settings.json", body: "no such section", level: "error" }]);
});

const key = (keystroke: string, mods: Partial<Record<"shift" | "control" | "alt" | "platform" | "function", boolean>> = {}) => {
  const k = keystroke.split(/-(?=.)/).pop()!;
  return { key: k, keystroke, modifiers: { shift: false, control: false, alt: false, platform: false, function: false, ...mods } };
};

/** Presses `e` while the quick terminal's key is being recorded; what settings.json holds after. */
async function press(raw: string, e: ReturnType<typeof key>) {
  Object.assign(disk, { raw, writes: 0, fail: undefined, readFail: undefined, toasts: [] });
  const view = new Settings();
  Object.assign(view, { recording: true, keyError: null });
  let pending: Promise<void> | undefined;
  view.record(e, { spawn: (f: (cx: any) => Promise<void>) => { pending = f({ notify: () => {} }); } });
  await pending;
  return { view, saved: JSON.parse(disk.raw!) };
}

test("quick terminal keys read in words, modifiers in macOS's order", () => {
  const view = new Settings();
  expect(view.keyLabel("shift-escape")).toBe("Shift + Esc");
  expect(view.keyLabel("cmd-shift-.")).toBe("Shift + Command + .");
  expect(view.keyLabel("cmd-ctrl-alt-space")).toBe("Control + Option + Command + Space");
  expect(view.keyLabel("cmd--")).toBe("Command + -");
  expect(view.keyLabel("f13")).toBe("F13");
});

test("quick terminal steppers move in whole steps and stop at their ends", () => {
  const view = new Settings();
  expect(view.stepped(0.37, 100, 10, 100, 5)).toBe(0.42);
  expect(view.stepped(0.1, 100, 10, 100, 5)).toBe(0.15);
  expect(view.stepped(0.12, 100, 10, 100, -5)).toBe(0.1);
  expect(view.stepped(1, 100, 10, 100, 5)).toBe(1);
  expect(view.stepped(1, 100, 10, 100, -10)).toBe(0.9);
  expect(view.stepped(50, 1, 0, 100, -10)).toBe(40);
  expect(view.stepped(4, 1, 0, 100, -10)).toBe(0);
  expect(view.stepped(0.05, 100, 10, 100, -10)).toBe(0.05);
  expect(view.stepped(0.05, 100, 10, 100, 10)).toBe(0.15);
});

test("a recorded quick terminal key is written in keys.json's spelling, keeping the rest", async () => {
  const { view, saved } = await press('{"quick_terminal": {"height": 0.4}, "font_size": 15}', key("shift-escape", { shift: true }));
  expect(saved).toEqual({ quick_terminal: { height: 0.4, key: "shift-escape" }, font_size: 15 });
  expect(view.recording).toBe(false);
  expect(view.keyError).toBe(null);
});

test("recording takes no plain key, waits through a lone modifier, and Escape stops it", async () => {
  const before = '{"quick_terminal": {"key": "cmd-."}}';
  const plain = await press(before, key("a"));
  expect(plain.saved).toEqual({ quick_terminal: { key: "cmd-." } });
  expect(plain.view.recording).toBe(true);
  expect(plain.view.keyError).toBe("Hold Command, Control or Option with it, or use Esc or an F key from F1 to F20.");
  const shiftA = await press(before, key("shift-a", { shift: true }));
  expect(shiftA.view.keyError).toBe("Hold Command, Control or Option with it, or use Esc or an F key from F1 to F20.");
  const f21 = await press(before, key("f21"));
  expect(f21.view.keyError).toBe("macOS takes F keys up to F20.");
  const reset = await press(before, key("ctrl-alt-shift-escape", { control: true, alt: true, shift: true }));
  expect(reset.view.keyError).toBe("That's Herdr's reset key.");
  expect(reset.view.recording).toBe(true);
  expect(reset.saved).toEqual({ quick_terminal: { key: "cmd-." } });
  const modifier = await press(before, key("shift", { shift: true }));
  expect(modifier.view.recording).toBe(true);
  expect(modifier.view.keyError).toBe(null);
  const stop = await press(before, key("escape"));
  expect(stop.view.recording).toBe(false);
  expect(disk.writes).toBe(0);
  const fkey = await press(before, key("f13"));
  expect(fkey.saved).toEqual({ quick_terminal: { key: "f13" } });
});

test("shift with a symbol key is recorded as macOS reports it, and a typed symbol alone is refused", async () => {
  const cmdShiftDot = await press("{}", key("cmd->", { platform: true }));
  expect(cmdShiftDot.saved).toEqual({ quick_terminal: { key: "cmd->" } });
  const shiftSlash = await press("{}", key("?"));
  expect(shiftSlash.saved).toEqual({});
  expect(shiftSlash.view.recording).toBe(true);
  const shiftEsc = await press("{}", key("shift-escape", { shift: true }));
  expect(shiftEsc.saved).toEqual({ quick_terminal: { key: "shift-escape" } });
});
