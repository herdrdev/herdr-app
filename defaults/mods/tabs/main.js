import { View, div, svg, image } from "gpui-kit";
import { h_flex, Popup, ScrollHandle } from "gpui-base";
import { spaces, panes, run, openOverlay, theme, window, shared, layout, settings } from "herdr";

const TAB_GAP = 3;

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// Same as the agents mod: drops a status glyph, keeps a leading bracket or quote.
const clean = (s) => (s ?? "").replace(/^[^\p{L}\p{N}\p{Ps}\p{Pi}]+/u, "").replace(/^\[ [!.] \] /, "").trim();

// A tab Herdr named (unnamed, or after its program) follows its agent's live
// title, as the agents list does; a name the person gave stays.
function label(tab, lead) {
  const prog = lead?.program?.name;
  const base = tab.named ? tab.name : prog ?? "Terminal";
  if (!lead?.program?.agent) return base;
  const live = clean(lead.title);
  // clean() would strip a path's leading ~ or /: test the title with only its status glyph gone.
  const path = /^[~/]|^[A-Za-z]:\\/.test((lead.title ?? "").replace(/^[^\p{L}\p{N}~/]+/u, ""));
  const telling = live && live !== lead.terminal && live !== prog && !path && !live.includes("@") && live.toLowerCase() !== lead.program.key;
  return (tab.name === prog || !tab.named) && telling ? live : base;
}

export default class Tabs extends View {
  init() {
    this.drag = null;
    this.widths = new Map();
    this.scroll = ScrollHandle.new();
    try { this.panels = JSON.parse(localStorage.getItem("panels") ?? "{}"); } catch { this.panels = {}; }
    if (!this.panels || typeof this.panels !== "object" || Array.isArray(this.panels)) this.panels = {};
    this.swipe = { acc: 0, steps: 0, at: 0, last: 0 };
  }

  dragTab(e, cx, shown, zoom, released = false) {
    const drag = this.drag;
    if (!drag) return;
    if (released || e.pressed_button !== "left") {
      this.drag = null;
      cx.notify();
      if (!released) return;
    }
    const { local_position: p, bounds } = e;
    if (!p || !bounds) return;
    const inside = p.x >= 0 && p.x < bounds.width && p.y >= 0 && p.y < bounds.height;
    const { x, y } = e.position;
    const down = drag.down;
    if (!drag.preview && Math.max(Math.abs(x - down.x), Math.abs(y - down.y)) < 4 * zoom) {
      const b = drag.bounds;
      if (released && inside && x >= b.x && x < b.x + b.width && y >= b.y && y < b.y + b.height) {
        run("tab.select", { id: drag.id });
      }
      return;
    }

    const from = shown.findIndex(tab => tab.id === drag.id);
    if (from < 0) return;
    let to = from;
    if (inside) {
      const pointer = p.x / zoom + this.scroll.offset().x;
      let left = 0;
      to = 0;
      // Compare with the resting tab centers so the preview cannot move its own thresholds.
      for (const tab of shown) {
        const width = this.widths.get(tab.id) / zoom;
        if (!Number.isFinite(width)) { to = from; break; }
        if (tab.id !== drag.id && pointer >= left + width / 2) to++;
        left += width + TAB_GAP;
      }
    }
    if (released) {
      if (to !== from) run("tab.move", { id: drag.id, index: shown[to].index ?? to + 1 });
      else if (!inside && window()?.platform === "macos") run("tab.detach", { id: drag.id, x, y });
    } else {
      drag.preview = { inside, to, x: (p.x - drag.grab.x) / zoom, y: (p.y - drag.grab.y) / zoom };
      cx.notify();
    }
  }

  // One tab per wheel notch. A touchpad or smooth-scrolling mouse sends many
  // small events per gesture: a short swipe or one smoothed notch (about
  // 25 px) switches one tab, a longer swipe keeps going a tab per 60 px, at
  // most one every 100 ms so each shows.
  wheel(e) {
    if (e.delta_lines) {
      if (e.delta_lines.y) run(e.delta_lines.y > 0 ? "tab.prev" : "tab.next");
      return;
    }
    const s = this.swipe, now = Date.now();
    // Phases often arrive only as moves (a macOS touchpad included): a pause
    // or a turn starts a new gesture.
    if (e.touch_phase === "started" || now - s.at > 250 || e.delta.y * s.acc < 0) Object.assign(s, { acc: 0, steps: 0 });
    s.at = now;
    s.acc += e.delta.y;
    const need = s.steps ? 60 : 20;
    if (Math.abs(s.acc) < need || now - s.last < 100) return;
    s.acc -= Math.sign(s.acc) * need;
    s.steps++;
    s.last = now;
    run(e.delta.y > 0 ? "tab.prev" : "tab.next");
  }

  // The side panel is remembered per space: switching shows that space's choice
  // (closed if it never had one), and opening or closing it records it.
  syncPanel(space, panel, body, enabled) {
    if (!space || !panel) return;
    const now = panel.hidden ? null : body?.active ?? null;
    if (space.id !== this.space) {
      const first = this.space === undefined;
      this.space = space.id; this.pending = null;
      if (!(first && !(space.id in this.panels))) {
        const stored = this.panels[space.id] ?? null, want = enabled.includes(stored) ? stored : null;
        if (want !== now) {
          this.pending = { want, until: Date.now() + 1000 };
          if (want) { run("layout.select", { stack: "panel-body", child: want }); run("layout.show", { id: "panel" }); }
          else run("layout.hide", { id: "panel" });
          return;
        }
      }
    }
    // Until the restore lands (or something else won), the old state isn't a choice.
    if (this.pending) {
      if (this.pending.want !== now && Date.now() < this.pending.until) return;
      this.pending = null;
    }
    if (this.panels[space.id] === now) return;
    this.panels[space.id] = now;
    localStorage.setItem("panels", JSON.stringify(this.panels));
  }

  render() {
    const t = theme();
    const space = spaces().find(p => p.active);
    const shown = space?.tabs.filter(tab => !tab.detached) ?? [];
    const zoom = settings().zoom;
    const ids = new Set(shown.map(tab => tab.id));
    if (this.drag && (this.drag.space !== space?.id || !ids.has(this.drag.id))) this.drag = null;
    const drag = this.drag, preview = drag?.preview;
    const held = preview ? shown.find(tab => tab.id === drag.id) : null;
    const order = held ? shown.filter(tab => tab !== held) : shown;
    if (held) order.splice(preview.to, 0, held);
    for (const id of this.widths.keys()) if (!ids.has(id)) this.widths.delete(id);
    const active = shown.findIndex(tab => tab.active);
    const selected = shown[active]?.id;
    if (selected !== this.selected || active !== this.selectedIndex) {
      this.selected = selected;
      this.selectedIndex = active;
      if (active >= 0) this.scroll.scroll_to_item(active);
    }
    const stop = (_e, cx) => cx.stop_propagation();
    const button = (id, label, action) => div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim).hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", stop).on_click(action).child(label);
    const item = (c, i, floating = false) => {
      const lead = panes().find(p => p.tab === c.id && p.program?.agent) ?? panes().find(p => p.tab === c.id);
      const program = lead?.program;
      const name = label(c, lead);
      return h_flex().id(floating ? "tab-drag" : `tab-${c.id}`).h(29).px(10).gap(7).rounded(7).items_center().flex_none().text_size(12.8)
        .border(1).border_color(c.active || floating ? t.border : t.chrome).bg(floating ? t.hover : c.active ? t.raised : t.chrome).text_color(c.active ? t.foreground : t.muted)
        .when(!floating, el => el
          .on_size_change((size, cx) => {
            this.widths.set(c.id, size.width);
            // A rename or status dot on this or an earlier tab can push the active one out.
            if (!this.drag && i <= active) { this.scroll.scroll_to_item(active); cx.notify(); }
          })
          .when(!held, tab => tab.hover(s => s.bg(t.hover)))
          .on_mouse_down("left", (e, cx) => {
            cx.stop_propagation();
            if (!e.bounds || !e.local_position) return;
            this.drag = { id: c.id, space: space.id, down: e.position, grab: e.local_position, bounds: e.bounds, preview: null };
            this.widths.set(c.id, e.bounds.width);
            if (e.click_count === 2) run("tab.rename", { id: c.id });
            cx.notify();
          })
          .on_mouse_down("right", (_e, cx) => { cx.stop_propagation(); openOverlay({ mod: "menu", kind: "popup", width: 200, props: { tab: "tab", target: { id: c.id, space: space.id } } }); }))
        .when(Boolean(program?.icon), el => el.child(program.tint ? svg(program.icon).size(14).text_color(t.muted) : image(program.icon).size(14)))
        .child(div().max_w(170).truncate().child(c.zoomed ? `${name} Z` : name))
        .when(c.status === "working", el => el.child(svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.working).spin(900)))
        .when(c.status === "blocked" || c.status === "done", el => el.child(div().size(6).rounded(3).bg(t[c.status])))
        .when(c.active, el => el.child(div().id(`close-${c.id}`).text_color(t.dim)
          .when(!floating, close => close.on_mouse_down("left", stop).on_click((_e, cx) => { cx.stop_propagation(); run("tab.close", { id: c.id }); }))
          .child(svg("mods/pane/close.svg").size(12))));
    };
    const root = layout(), sidebar = find(root, "sidebar"), w = window();
    // Old saved rails and a dragged-narrow sidebar can be smaller than the window buttons.
    const lightsPadding = w?.platform === "macos" && w.titlebar === "none" && !w.fullscreen && root?.traffic_lights
      ? Math.max(10, (root.traffic_lights[0] + 64) / zoom - (sidebar?.hidden ? 0 : sidebar?.size?.px ?? 0))
      : 10;
    const panel = find(root, "panel"), body = find(root, "panel-body");
    const enabled = shared("workspace.panels") ?? ["files", "changes"];
    this.syncPanel(space, panel, body, enabled);
    const panelButton = (id, label, icon) => {
      const on = panel && !panel.hidden && body?.active === id;
      return h_flex().id(`panel-${id}`).h(26).px(8).gap(5).flex_none().items_center().rounded(6)
        .text_size(12).bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.muted)
        .hover(s => s.bg(t.hover)).on_mouse_down("left", stop)
        .on_click(() => {
          if (on) run("layout.hide", { id: "panel" });
          else { run("layout.select", { stack: "panel-body", child: id }); run("layout.show", { id: "panel" }); }
        }).child(svg(icon).size(14)).child(label);
    };
    return h_flex().size_full().min_w(0).px(10).pl(lightsPadding).gap(3).items_center().bg(t.chrome).border_b(1).border_color(t.border)
      .on_scroll_wheel(e => this.wheel(e))
      .child(div().id("tab-strip").relative().flex_1().min_w(0).h_full().overflow_hidden()
        .when(drag, el => el.on_window_mouse_move((e, cx) => this.dragTab(e, cx, shown, zoom)))
        .on_window_mouse_up("left", (e, cx) => this.dragTab(e, cx, shown, zoom, true))
        .child(h_flex().id("tab-scroll").size_full().items_center().gap(TAB_GAP).overflow_x_scrollbar().track_scroll(this.scroll)
          .on_size_change((_size, cx) => { if (!this.drag && active >= 0) { this.scroll.scroll_to_item(active); cx.notify(); } })
          .children(order.map((tab, i) => tab === held
            ? div().id("tab-drop").h(29).w(drag.bounds.width / zoom).flex_none().rounded(7)
              .when(preview.inside, el => el.border_1().border_color(t.accent))
            : item(tab, i))))
        // Popup paints above the layout's clip when the tab crosses into a terminal.
        .when(drag, strip => strip.child(Popup.new("tab-drag-layer", div().size(0)).absolute().size(0)
          .left(preview?.x ?? 0).top(preview?.y ?? 0)
          .when(held, el => el.content(item(held, preview.to, true).w(drag.bounds.width / zoom))))))
      .child(button("tab-new", "+", () => openOverlay({ mod: "new-menu", kind: "popup", width: 250, height: 512 })))
      .when(enabled.includes("files"), el => el.child(panelButton("files", "Files", "mods/files/file.svg")))
      .when(enabled.includes("changes"), el => el.child(panelButton("changes", "Changes", "mods/changes/changes.svg")))
      .when(w?.buttons, el => el.child(button("minimize", w.platform === "windows" ? svg("mods/tabs/win-min.svg").size(16) : "−", () => run("window.minimize")))
        .child(button("maximize", w.platform === "windows" ? svg(`mods/tabs/${w.maximized ? "win-restore" : "win-max"}.svg`).size(16) : "□", () => run("window.maximize")))
        .child(button("close-window", w.platform === "windows" ? svg("mods/tabs/win-close.svg").size(16) : svg("mods/pane/close.svg").size(13), () => run("window.close"))));
  }
}
