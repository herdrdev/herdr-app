import { t as tr } from "herdr";
// The frame's command palette: everything you can do, by name, with its key.
// Places (spaces, tabs) stay in the picker. Fuzzy: your letters in
// order, word starts and runs rank higher. Empty: recent first, then every
// group. Choices are entries of their own ("Layout: Zen"); commands that need
// a name ask for it. Enter runs, ↑/↓ move, Esc closes.
// The app's own mod: rewritten from the binary on every start.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { commands, layouts, keys, run, theme, props, closeOverlay, openOverlay, shared, window as win } from "herdr";

// The list keeps one height while you move: rows and group titles have fixed
// heights, and the window shows as many as fit in LIST_H.
// 10 rows: with the search line it fits the popup (at most 85% of the
// window) down to ~580 px windows at zoom 1.3.
const SHOWN = 10;
const ROW_H = 32;
const HEAD_H = 26;
const LIST_H = SHOWN * ROW_H;
const GROUPS = ["Recent", "Tab", "Pane", "Space", "Layout", "Terminal", "Agent", "Window", "App"];
const groupLabel = (group) => GROUPS.includes(group) ? tr(`palette-group-${group.toLowerCase()}`) : group;

// The palette's names for commands: [command, args, group, title, keywords].
const NAMED = [
  ["app.feedback", {}, "App", tr("command-app-feedback"), "Send feedback feedback report issue"],
  ["app.check_updates", {}, "App", tr("command-app-check-updates"), "Check for updates check updates"],
  ["app.update_core", {}, "App", tr("command-app-update-core"), "Update herdr-core update core"],
  ["app.update", {}, "App", tr("command-app-update"), "Restart to update restart update"],
  ["terminal.search", {}, "Terminal", tr("command-terminal-search"), "Find in terminal find search terminal"],
  ["terminal.paste_image", {}, "Terminal", tr("command-terminal-paste-image"), "Paste image paste clipboard image"],
  ["terminal.scroll", {}, "Terminal", tr("command-terminal-scroll"), "Scroll terminal scroll terminal"],

  ["tab.new", {}, "Tab", tr("palette-new-tab"), "New tab tab agent new task"],
  ["mod.open", { name: "picker" }, "Tab", tr("palette-go-to-space-or-tab"), "Go to space or tab… switch jump find"],
  ["tab.rename", {}, "Tab", tr("palette-rename-tab"), "Rename tab tab"],
  ["tab.close", {}, "Tab", tr("palette-close-tab"), "Close tab tab kill"],
  ["tab.next", {}, "Tab", tr("palette-next-tab"), "Next tab tab"],
  ["tab.prev", {}, "Tab", tr("palette-previous-tab"), "Previous tab tab"],
  ["pane.split", { direction: "right" }, "Pane", tr("palette-split-right"), "Split right vertical"],
  ["pane.split", { direction: "down" }, "Pane", tr("palette-split-down"), "Split down horizontal"],
  ["pane.close", {}, "Pane", tr("palette-close-pane"), "Close pane kill"],
  ["pane.zoom", {}, "Pane", tr("palette-zoom-pane"), "Zoom pane maximize fullscreen"],
  ...["left", "right", "up", "down"].map((d) => ["pane.go", { direction: d }, "Pane", tr("palette-focus-pane", { d: tr(`common-direction-${d}`) }), `focus move go ${d}`]),
  ...["left", "right", "up", "down"].map((d) => ["pane.swap", { direction: d }, "Pane", tr("palette-swap-pane", { d: tr(`common-direction-${d}`) }), `swap move ${d}`]),
  ["space.new", {}, "Space", tr("palette-new-space"), "New space workspace"],
  ["space.rename", {}, "Space", tr("palette-rename-space"), "Rename space workspace"],
  ["space.close", {}, "Space", tr("palette-close-space"), "Close space workspace kill"],
  ["space.next", {}, "Space", tr("palette-next-space"), "Next space workspace"],
  ["space.prev", {}, "Space", tr("palette-previous-space"), "Previous space workspace"],
  ["layout.switch", {}, "Layout", tr("palette-next-layout"), "Next layout look switch"],
  ["layout.reset", {}, "Layout", tr("palette-reset-layout"), "Reset layout new version default restore herdr's"],
  ["layout.clone", {}, "Layout", tr("palette-duplicate-layout"), "Duplicate layout clone copy customize"],
  ["mod.clone", {}, "Layout", tr("palette-duplicate-mod"), "Duplicate mod clone copy customize"],
  ["terminal.open", {}, "Terminal", tr("palette-new-terminal"), "New terminal shell"],
  ["terminal.copy", {}, "Terminal", tr("palette-copy"), "Copy clipboard"],
  ["terminal.paste", {}, "Terminal", tr("palette-paste"), "Paste clipboard"],
  ["agent.next", {}, "Agent", tr("palette-next-agent"), "Next agent "],
  ["agent.prev", {}, "Agent", tr("palette-previous-agent"), "Previous agent "],
  ["window.minimize", {}, "Window", tr("palette-minimize-window"), "Minimize window "],
  ["window.maximize", {}, "Window", tr("palette-maximize-window"), "Maximize window restore"],
  ["window.close", {}, "Window", tr("palette-close-window"), "Close window quit"],
  ["app.settings", {}, "App", tr("palette-settings"), "Settings preferences options config look font theme sound"],
  ["app.help", {}, "App", tr("palette-keys"), "Keys keybindings shortcuts help"],
  ["app.welcome", {}, "App", tr("palette-welcome"), "Welcome intro start onboarding"],
  ["app.whats_new", {}, "App", tr("palette-what-s-new"), "What's new release changes news update"],
  ["app.reset", {}, "App", tr("palette-reset-to-the-default-screen"), "Reset to the default screen escape hatch broken restore"],
  ["app.zoom_in", {}, "App", tr("palette-zoom-in"), "Zoom in bigger font size"],
  ["app.zoom_out", {}, "App", tr("palette-zoom-out"), "Zoom out smaller font size"],
  ["app.zoom_reset", {}, "App", tr("palette-reset-zoom"), "Reset zoom font size"],
  ["app.quit", {}, "App", tr("palette-quit-herdr"), "Quit Herdr exit"],
  ...["graphite", "ink", "moss", "atom-one-dark"].map((n) => ["app.theme", { name: n }, "App", tr("palette-theme", { value1: n.replace(/-/g, " ") }), "colors look"]),
];

// Commands that act on something you point at (ids, terminals): not for a list.
const HIDDEN = new Set([
  "space.move", "tab.move", "space.select", "tab.select", "pane.focus", "terminal.keep",
  "terminal.park", "terminal.kill", "terminal.show", "agent.focus", "layout.set", "layout.toggle",
  "layout.show", "layout.hide", "layout.resize", "layout.select", "mod.open", "overlay.close",
  "tab.settle", "tab.unsettle",
  "app.palette", "app.hide", "app.hide_others", "app.show_all", "app.theme", "app.settings",
]);

function titleOf(name) {
  const [noun, verb] = name.split(".");
  return { group: noun.charAt(0).toUpperCase() + noun.slice(1), title: `${verb.replace(/_/g, " ")} ${noun}`.replace(/^./, (c) => c.toUpperCase()) };
}

// Every entry for the state we're in now.
function entries() {
  const known = new Set(commands().map((c) => c.name));
  const mac = win()?.platform === "macos";
  const list = NAMED.filter(([n]) => known.has(n))
    .filter(([n]) => !(mac && n.startsWith("window.")))
    .map(([name, args, group, title, words]) => ({ name, args, group, title, words }));
  for (const l of layouts()) {
    const t = { main: "Herdr", terminals: tr("palette-terminals"), win98: "Herdr 98" }[l] ?? l.charAt(0).toUpperCase() + l.slice(1);
    list.push({ name: "layout.switch", args: { name: l }, group: "Layout", title: tr("palette-layout", { t: t }), words: `look ${l}` });
  }
  // Anything new in the app shows up too, until it gets a better name here.
  const named = new Set(NAMED.map(([n]) => n));
  for (const c of commands()) {
    if (named.has(c.name) || HIDDEN.has(c.name) || c.args.some((a) => a.required && a.name !== "name")) continue;
    list.push({ name: c.name, args: {}, ...titleOf(c.name), words: c.about });
  }
  return list.map((e) => ({ ...e, id: `${e.name}:${JSON.stringify(e.args)}` }));
}

// The key bound to an entry: `name` or `name:value` in keys.json.
function keyOf(e, bound) {
  const v = Object.values(e.args)[0];
  const want = v === undefined ? e.name : `${e.name}:${v}`;
  const k = bound.find((b) => b.section === "global" && b.action === want);
  return k ? k.keys : null;
}

// "ctrl-shift-p" -> ["ctrl", "shift", "p"]; "ctrl--" -> ["ctrl", "-"].
function keyParts(k) {
  return k.endsWith("--") ? [...k.slice(0, -2).split("-"), "-"] : k.split("-");
}

// Fuzzy score of `q` in `text`, with the matched positions; null: no match.
function fuzzy(q, text) {
  const t = text.toLowerCase();
  let score = 0;
  let at = 0;
  let run = 0;
  const hits = [];
  for (const ch of q) {
    const i = t.indexOf(ch, at);
    if (i < 0) return null;
    const start = i === 0 || /[\s:.\-_/]/.test(t[i - 1]);
    run = i === at && hits.length ? run + 1 : 0;
    score += 1 + (start ? 6 : 0) + run * 3 - Math.min(i - at, 6) * 0.3;
    hits.push(i);
    at = i + 1;
  }
  return { score: score - text.length * 0.02, hits };
}

export default class Palette extends View {
  init(_props, cx) {
    this.query = "";
    this.input = InputState.new({ placeholder: tr("palette-type-a-command") });
    this.input.on("change", (_e, cx) => { this.query = this.input.value(); this.at = 0; cx.notify(); });
    this.input.on("submit", () => this.go(this.rows()[this.at]?.e));
    this.input.focus();
    cx.timer.after(600, () => this.input.focus());
    this.at = 0;
    // How far the list is scrolled, px.
    this.scroll = 0;
    this.recent = JSON.parse(localStorage.getItem("recent") ?? "[]");
    this.handle = cx.focus_handle();
  }

  close() {
    closeOverlay(props().overlay);
  }

  go(e) {
    if (!e) return;
    this.recent = [e.id, ...this.recent.filter((r) => r !== e.id)].slice(0, 20);
    localStorage.setItem("recent", JSON.stringify(this.recent));
    this.close();
    if (e.name === "mod.clone") {
      openOverlay({ mod: "prompt", kind: "dialog", width: 380, props: {
        title: tr("palette-duplicate-mod"), hint: tr("palette-mod-to-copy-creates-name-copy"),
        command: { name: "mod.clone", args: {} },
      } });
    } else {
      run(e.name, e.name === "layout.clone" ? { name: shared("layout") ?? "main" } : e.args);
    }
  }

  // The rows to show: grouped when empty, ranked while typing.
  rows() {
    const all = entries();
    const q = this.query.trim().toLowerCase().replace(/\s+/g, "");
    if (!q) {
      const recent = this.recent.map((id) => all.find((e) => e.id === id)).filter(Boolean).slice(0, 5);
      const rest = GROUPS.slice(1).flatMap((g) => all.filter((e) => e.group === g && !recent.includes(e)));
      const others = all.filter((e) => !GROUPS.includes(e.group) && !recent.includes(e));
      return [...recent.map((e) => ({ e, head: "Recent" })), ...[...rest, ...others].map((e) => ({ e, head: e.group }))];
    }
    const rank = (e) => this.recent.indexOf(e.id);
    return all
      .map((e) => {
        const title = fuzzy(q, e.title);
        const more = fuzzy(q, `${groupLabel(e.group)} ${e.group} ${e.title} ${e.name} ${e.words}`);
        const best = title ?? (more && { score: more.score - 4, hits: [] });
        return best && { e, score: best.score, hits: best.hits };
      })
      .filter(Boolean)
      .sort((a, b) => b.score - a.score || (rank(a.e) < 0 ? 99 : rank(a.e)) - (rank(b.e) < 0 ? 99 : rank(b.e)));
  }

  title(t, text, hits, on) {
    if (!hits?.length) return div().text_color(on ? t.foreground : t.muted).child(text);
    const parts = [];
    let cur = "";
    let lit = false;
    [...text].forEach((ch, i) => {
      const h = hits.includes(i);
      if (h !== lit && cur) {
        parts.push([cur, lit]);
        cur = "";
      }
      lit = h;
      cur += ch;
    });
    if (cur) parts.push([cur, lit]);
    return h_flex().children(parts.map(([s, h]) => div().text_color(h ? t.accent : on ? t.foreground : t.muted).when(h, (d) => d.font_semibold()).child(s.replace(/ /g, "\u00a0"))));
  }

  render() {
    const t = theme();
    const rows = this.rows();
    const bound = keys();
    this.at = Math.min(this.at, Math.max(0, rows.length - 1));
    // One continuous strip: each row (and the group title before it) at a
    // fixed y. Scrolling moves the strip in pixels, only as far as it takes to
    // keep the selected row (and its group's title, if it opens one) in view.
    const tops = [];
    let y = 0;
    rows.forEach((r, i) => {
      const head = !this.query && (i === 0 || r.head !== rows[i - 1].head);
      tops.push({ head, title: y, row: y + (head ? HEAD_H : 0) });
      y += (head ? HEAD_H : 0) + ROW_H;
    });
    const total = y;
    const fixed = total > LIST_H;
    this.maxScroll = Math.max(0, total - LIST_H);
    const sel = tops[this.at];
    if (sel) {
      const from = sel.head ? sel.title : sel.row;
      if (from < this.scroll) this.scroll = from;
      if (sel.row + ROW_H > this.scroll + LIST_H) this.scroll = sel.row + ROW_H - LIST_H;
    }
    this.scroll = Math.max(0, Math.min(this.scroll, this.maxScroll));
    const key = (k) =>
      h_flex().gap(3).children(keyParts(k).map((p) => div().px(5).rounded(4).border_1().border_color(t.border).bg(t.raised).text_size(11).text_color(t.dim).child(p)));
    // In a scrolling list each item is placed at its edges from the model
    // above, not stacked: GPUI rounds every height to whole pixels, and a
    // stack of rounded heights drifts away from the model (19 px at zoom 1.3
    // by row 40, cutting the selection off). Edges from the model can't add up.
    const place = (el, start, end) =>
      fixed
        ? el.absolute().left(0).right(0).top(start - this.scroll).bottom(LIST_H - (end - this.scroll))
        : el.flex_none().h(end - start);
    const visible = (start, end) => !fixed || (end > this.scroll && start < this.scroll + LIST_H);
    const list = [];
    rows.forEach((r, i) => {
      const { head, title, row } = tops[i];
      if (head && visible(title, row)) list.push(place(h_flex(), title, row).px(14).pb(4).items_end().text_size(11).font_semibold().text_color(t.dim).child(groupLabel(r.head)));
      if (!visible(row, row + ROW_H)) return;
      const on = i === this.at;
      const k = keyOf(r.e, bound);
      list.push(
        place(h_flex(), row, row + ROW_H)
          .id(`pl-${i}`)
          .mx(6)
          .px(10)
          .gap(10)
          .items_center()
          .rounded(7)
          .cursor_pointer()
          .text_size(12.8)
          .when(on, (el) => el.bg(t.selected))
          .hover((s) => s.bg(t.hover))
          .on_click(() => this.go(r.e))
          .child(div().flex_1().min_w(0).child(this.title(t, r.e.title, r.hits, on)))
          .when(Boolean(this.query), (el) => el.child(div().text_size(11.5).text_color(t.dim).child(groupLabel(r.e.group))))
          .when(Boolean(k), (el) => el.child(key(k))),
      );
    });
    return v_flex()
      .id("palette")
      .track_focus(this.handle)
      .w_full()
      .min_h(0)
      .p(18)
      .gap(10)
      .bg(t.popover)
      .on_key_down((e, cx) => {
        const k = e.key;
        if (k === "escape") return this.close();
        // Stops at the ends: wrapping jumps out of sight.
        if (k === "down") this.at = Math.min(this.at + 1, rows.length - 1);
        else if (k === "up") this.at = Math.max(this.at - 1, 0);
        else if (k === "pagedown") this.at = Math.min(this.at + SHOWN, rows.length - 1);
        else if (k === "pageup") this.at = Math.max(this.at - SHOWN, 0);
        else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(
        h_flex()
          .h(36)
          .flex_none()
          .px(10)
          .gap(10)
          .items_center()
          .rounded(9)
          .border_1()
          .border_color(t.accent)
          .bg(t.background)
          .child(svg(win()?.platform === "macos" ? "frame/palette/command.svg" : "mods/workspace-chrome/search.svg").size(16).text_color(t.dim))
          .child(Input.new(this.input).flex_1().h_full().text_size(12.8).text_color(t.foreground))
          .child(div().text_size(11).text_color(t.dim).child(tr("palette-esc"))),
      )
      .child(
        rows.length
          ? div()
              .id("palette-rows")
              .relative()
              .mb(6)
              .when(fixed, (el) => el.h(LIST_H).overflow_hidden())
              .on_scroll_wheel((e, cx) => {
                // The event bridge converts both wheel and touchpad deltas to pixels.
                this.scroll = Math.max(0, Math.min(this.scroll - e.delta.y, this.maxScroll));
                // The selection stays on screen, so the next frame doesn't scroll back to it.
                // A row that opens a group is shown with its title, so the title counts.
                while (this.at < rows.length - 1 && (tops[this.at].head ? tops[this.at].title : tops[this.at].row) < this.scroll) this.at++;
                while (this.at > 0 && tops[this.at].row + ROW_H > this.scroll + LIST_H) this.at--;
                cx.notify();
              })
              .children(fixed ? list : [v_flex().children(list)])
          : div().px(16).py(14).text_size(13).text_color(t.dim).child(tr("palette-nothing-matches-places-are-in-the-picker", { query: this.query })),
      );
  }
}
