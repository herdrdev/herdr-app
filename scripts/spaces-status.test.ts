import { describe, expect, mock, test } from "bun:test";

// The spaces mod runs inside the app (gpui-kit/gpui-base/herdr are the app's
// modules). Stub them so the mod's pure status helpers can run here: div()
// records the style calls statusDot makes instead of drawing.
function recorder() {
  const calls: Record<string, unknown[][]> = {};
  const el: Record<string, unknown> = new Proxy(
    { calls },
    {
      get(target, prop: string) {
        if (prop === "calls") return target.calls;
        return (...args: unknown[]) => {
          target.calls[prop] ??= [];
          target.calls[prop].push(args);
          return el;
        };
      },
    }
  );
  return el;
}

mock.module("gpui-kit", () => ({
  View: class {},
  div: () => recorder(),
  svg: () => recorder(),
}));
mock.module("gpui-base", () => ({ v_flex: () => recorder(), h_flex: () => recorder() }));
mock.module("herdr", () =>
  Object.fromEntries(
    ["spaces", "machines", "panes", "run", "openOverlay", "exec", "hc", "theme", "shared", "setShared", "watchFolder", "window", "layout"].map(
      (name) => [name, () => []]
    )
  )
);

const { glyph, tone, default: Spaces } = await import("../defaults/mods/spaces/main.js");

const t = { blocked: "#b", done: "#d", working: "#w", idle: "#i", unknown: "#u", dim: "#dim" };

describe("spaces status dot", () => {
  const spaces = new Spaces();

  test("blocked/done/working draw a filled dot in the status color", () => {
    for (const status of ["blocked", "done", "working"] as const) {
      const el = spaces.statusDot(status, t);
      expect(el.calls.bg).toEqual([[t[status]]]);
      expect(el.calls.border_color).toBeUndefined();
    }
  });

  test("idle draws a ring, not a filled dot", () => {
    const el = spaces.statusDot("idle", t);
    expect(el.calls.bg).toBeUndefined();
    expect(el.calls.border_color).toEqual([[t.idle]]);
  });

  test("no agent draws an empty slot of the same size", () => {
    const el = spaces.statusDot(null, t);
    expect(el.calls.bg).toBeUndefined();
    expect(el.calls.border_color).toBeUndefined();
    expect(el.calls.size).toEqual([[7]]);
  });

  test("compact glyph tells every status apart", () => {
    expect(glyph("blocked")).toBe("●");
    expect(glyph("done")).toBe("●");
    expect(glyph("working")).toBe("●");
    expect(glyph("idle")).toBe("○");
    expect(glyph(null)).toBe("·");
    // Same shape, different color: tone resolves each status' own theme slot.
    expect(["blocked", "done", "working"].map((s) => tone(t, s))).toEqual(["#b", "#d", "#w"]);
    expect(tone(t, null)).toBe("#u");
  });
});
