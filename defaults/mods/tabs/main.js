import { View, div, svg, image } from "gpui-kit";
import { h_flex, ScrollHandle } from "gpui-base";
import { spaces, panes, focused, run, openOverlay, theme, window, shared, layout, settings } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// Slide: a tab moving to its new place. Glide: the dragged tab landing or flying
// back. Enter: a new tab fades in once its neighbours have mostly made room
// (fading in as they start to slide, they covered it). Exit: a closed tab fades out, shorter, and
// hides the neighbours sliding over its place.
// Slide is a spring: when the landing changes mid-slide it keeps its speed
// (a transition restarts from rest, a visible stutter); damping 1, no overshoot.
const SLIDE = { response: 200, damping: 1 }, GLIDE = { duration: 200, easing: "ease-out" };
const ENTER = { duration: 200, delay: 100, easing: "ease-out" }, EXIT = { duration: 120, easing: "ease-out" };
const GAP = 3, LIFT = 4;
// The held look, the same in every dragged list: nothing follows the pointer's
// hover while one is held or landing (a neighbour sliding under a still pointer
// flickered), nor after it lands until the pointer moves. The held one shows
// its hover look and a shadow over its own opaque look: the shadow fades in as
// it lifts, both fade out as it lands, so it ends exactly as it rests.
const RAISE = { duration: 120, easing: "ease-out" };
// Held within EDGE px of the row's ends, the row scrolls, up to STEP px a frame.
const EDGE = 40, STEP = 14;

export default class Tabs extends View {
  init(_props, cx) {
    // null, or the one pointer gesture going on: "pressed" (not moved yet),
    // "drag" (held), "drop" or "back" (landing).
    this.gesture = null;
    this.quiet = false;
    this.widths = {};
    // What the row showed last, so a tab that goes fades where it was.
    this.last = {};
    this.ghosts = {};
    this.first = new Set();
    this.placed = new Set();
    this.handle = cx.focus_handle();
    this.scroll = ScrollHandle.new();
    this.view = 0;
    this.panels = JSON.parse(localStorage.getItem("panels") ?? "{}");
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

  // Where each tab sits in the row, from the widths the tabs report.
  geometry(ids) {
    const x = []; let at = 0;
    for (const id of ids) { x.push(at); at += (this.widths[id] ?? 0) + GAP; }
    return { x, end: at - GAP };
  }

  held() {
    const g = this.gesture;
    return g && g.kind !== "pressed" ? g : null;
  }

  follow(pointer, shown, cx) {
    if (this.quiet) { this.quiet = false; cx.notify(); }
    const g = this.gesture;
    if (g?.kind === "drag") {
      g.pointer = pointer;
      this.place(g);
      this.edge(g, cx);
      cx.notify();
    } else if (g?.kind === "pressed" && Math.abs(pointer - g.x) >= LIFT && g.left !== undefined) {
      const order = shown.map(c => c.id);
      if (order.length < 2 || order.some(id => !(id in this.widths))) return;
      const from = order.indexOf(g.id), scroll = this.scroll.offset().x;
      // The row's visible left edge in the window, from where the pressed tab was.
      const left = g.left - this.geometry(order).x[from] + scroll;
      const d = this.gesture = { kind: "drag", id: g.id, order, from, down: g.x, pointer, scroll, left, dx: 0, pane: focused()?.pane };
      this.place(d);
      this.handle.focus();
      // A transition starts from what was drawn: the shadow is drawn clear once, then raised.
      cx.timer.after(0, cx => { d.raised = true; cx.notify(); });
      cx.notify();
    }
  }

  // Under the pointer, in the row's own coordinates (what the row scrolled
  // counts), and kept inside the row and its visible part.
  place(d) {
    const { x, end } = this.geometry(d.order), w = this.widths[d.id], at = this.scroll.offset().x;
    const left = x[d.from] + d.pointer - d.down + at - d.scroll;
    d.dx = Math.min(Math.max(left, 0, at), end - w, at + this.view - w) - x[d.from];
  }

  // How fast the row scrolls with the pointer here: faster nearer an end.
  speed(d) {
    const into = Math.min(d.pointer - d.left, this.view - (d.pointer - d.left));
    if (into >= EDGE) return 0;
    const v = STEP * Math.min(1, (EDGE - into) / EDGE);
    return d.pointer - d.left < this.view / 2 ? -v : v;
  }

  // A held pointer at an end keeps scrolling without moving, so this one ticks,
  // only while held there.
  edge(d, cx) {
    if (this.speed(d) === 0) { d.tick?.cancel(); d.tick = null; return; }
    d.tick ??= cx.timer.every(16, cx => {
      const v = this.gesture === d ? this.speed(d) : 0;
      if (v === 0) { d.tick?.cancel(); d.tick = null; return; }
      const at = this.scroll.offset().x;
      this.scroll.set_offset({ x: at + v });
      if (this.scroll.offset().x === at) { d.tick?.cancel(); d.tick = null; return; }
      this.place(d);
      cx.notify();
    });
  }

  // Which slot letting go lands in: past half of a neighbour, it makes room.
  landing() {
    const d = this.gesture, { x } = this.geometry(d.order), w = this.widths[d.id];
    const left = x[d.from] + d.dx, right = left + w;
    let to = d.from;
    d.order.forEach((id, j) => {
      const mid = x[j] + this.widths[id] / 2;
      if (j > d.from && right > mid) to++;
      if (j < d.from && left < mid) to--;
    });
    return { to, x };
  }

  settle(drop, cx) {
    const d = this.gesture, { to } = this.landing();
    d.tick?.cancel();
    const kind = drop && to !== d.from ? "drop" : "back";
    const g = this.gesture = { ...d, kind, to: kind === "drop" ? to : d.from, tick: null };
    if (kind === "drop") run("tab.move", { id: d.id, index: g.to + 1 });
    if (d.pane) run("pane.focus", { pane: d.pane });
    cx.timer.after(GLIDE.duration, cx => { if (this.gesture === g) { this.gesture = null; this.quiet = true; cx.notify(); } });
    cx.notify();
  }

  // Every tab sits at a place worked out from the widths, and slides when that
  // place changes, whatever changed it: a drag, a command, a tab opening or
  // closing elsewhere. A new tab fades in once it has a width; a closed one
  // fades out where it was. What a space shows first (on start, on a switch)
  // just appears: a tab's first place, and its first showing, are cuts.
  // While a tab is held it is drawn last, over the others, at the pointer; a
  // dashed box of its size is where letting go lands.
  row(space, shown, item, t, cx) {
    let d = this.held();
    if (d && (shown.length !== d.order.length || shown.some(c => !d.order.includes(c.id)))) d = this.gesture = null;
    if (space?.id !== this.rowSpace) {
      this.rowSpace = space?.id; this.last = {}; this.ghosts = {}; this.placed = new Set();
      this.first = new Set(shown.map(c => c.id));
    }
    // The first tabs this space shows (none yet on start) were already there, not new.
    if (!this.first.size) shown.forEach(c => this.first.add(c.id));
    for (const [id, g] of Object.entries(this.last)) {
      if (shown.some(c => c.id === id) || this.ghosts[id]) continue;
      this.ghosts[id] = g;
      cx.timer.after(EXIT.duration, cx => { if (this.ghosts[id] === g) { delete this.ghosts[id]; cx.notify(); } });
    }
    let order = shown.map(c => c.id);
    if (d) {
      order = d.order.filter(id => id !== d.id);
      order.splice(d.kind === "drag" ? this.landing().to : d.to, 0, d.id);
    }
    const { x, end } = this.geometry(order), at = Object.fromEntries(order.map((id, j) => [id, x[j]]));
    const byId = Object.fromEntries(shown.map(c => [c.id, c]));
    this.last = {};
    const cell = (c, left, motion, opacity, fade) => div().id(`cell-${c.id}`).absolute().top(0).bottom(0).flex().items_center()
      .left(left).map(el => motion === SLIDE ? el.spring("left", SLIDE) : el.transition("left", motion))
      .opacity(opacity).transition("opacity", fade);
    const placed = this.placed; this.placed = new Set();
    const cells = [];
    // The dashed box comes and goes with the backing (it shows round the held tab's edge).
    const up = d?.kind === "drag" && d.raised;
    if (d) cells.push(div().id(`slot-${d.id}`).absolute().top(0).bottom(0).flex().items_center().left(at[d.id]).spring("left", SLIDE)
      .opacity(up ? 1 : 0).transition("opacity", up ? RAISE : GLIDE)
      .child(div().w(this.widths[d.id]).h(29).rounded(7).border(1).border_dashed().border_color(t.dim)));
    for (const [id, g] of Object.entries(this.ghosts)) cells.push(cell(g.c, g.x, SLIDE, 0, EXIT).child(item(g.c)));
    for (const id of order) {
      const c = byId[id], known = id in this.widths;
      if (known) this.placed.add(id);
      this.last[id] = { c, x: at[id] };
      if (d && id === d.id) continue;
      // Hidden until it has a width (its place isn't known yet).
      cells.push(cell(c, at[id], placed.has(id) ? SLIDE : 0, known ? 1 : 0, this.first.has(id) ? 0 : ENTER).child(item(c)));
    }
    if (d) {
      const c = byId[d.id], dragging = d.kind === "drag";
      const lifted = dragging ? this.geometry(d.order).x[d.from] + d.dx : at[d.id];
      const layer = (name, bg) => div().id(`${name}-${d.id}`).absolute().inset(0).rounded(7).bg(bg);
      cells.push(cell(c, lifted, dragging ? 0 : GLIDE, 1, 0).child(div().relative()
        .child(layer("backing", t.chrome).shadow_md().opacity(up ? 1 : 0).transition("opacity", up ? RAISE : GLIDE))
        .child(layer("rest", c.active ? t.raised : t.chrome))
        // A tab's hover replaces its fill: the hover look is its own opaque layer.
        .child(layer("hovered", t.chrome).child(div().size_full().rounded(7).bg(t.hover))
          .opacity(dragging ? 1 : 0).transition("opacity", dragging ? 0 : GLIDE))
        .child(item(c, true))));
    }
    return [div().id("tab-row").relative().h_full().w(Math.max(end, 0)).flex_none().children(cells)];
  }

  render(cx) {
    const t = theme();
    const space = spaces().find(p => p.active);
    const shown = space?.tabs ?? [];
    const stop = (_e, cx) => cx.stop_propagation();
    const button = (id, label, action) => div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim).hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", stop).on_click(action).child(label);
    const hovers = !this.quiet && !this.held();
    const item = (c, lifted) => {
      const program = panes().find(p => p.tab === c.id && p.program?.agent)?.program ?? panes().find(p => p.tab === c.id)?.program;
      const name = /^tb\d+$/.test(c.name) ? program?.name ?? "Terminal" : c.name;
      return h_flex().id(`tab-${c.id}`).h(29).px(10).gap(7).rounded(7).items_center().flex_none().text_size(12.8)
        .border(1).border_color(c.active ? t.border : t.chrome).bg(lifted ? "#00000000" : c.active ? t.raised : t.chrome).text_color(c.active ? t.foreground : t.muted)
        .when(hovers, el => el.hover(s => s.bg(t.hover)))
        .on_size_change((size, cx) => { if (this.widths[c.id] !== size.width) { this.widths[c.id] = size.width; cx.notify(); } })
        .on_mouse_down("left", (e, cx) => {
          cx.stop_propagation();
          if (!this.held()) this.gesture = { kind: "pressed", id: c.id, x: e.position.x, left: e.bounds?.x };
          if (e.click_count === 2) run("tab.rename", { id: c.id });
        })
        .on_mouse_down("right", (_e, cx) => { cx.stop_propagation(); openOverlay({ mod: "menu", kind: "popup", width: 200, props: { tab: "tab", target: { id: c.id, space: space.id } } }); })
        .when(Boolean(program?.icon), el => el.child(program.tint ? svg(program.icon).size(14).text_color(t.muted) : image(program.icon).size(14)))
        .child(div().max_w(170).truncate().child(c.zoomed ? `${name} Z` : name))
        .when(c.status === "working", el => el.child(svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.working).spin(900)))
        .when(c.status === "blocked" || c.status === "done", el => el.child(div().size(6).rounded(3).bg(t[c.status])))
        .when(c.active, el => el.child(div().id(`close-${c.id}`).text_color(t.dim).on_mouse_down("left", stop).on_click((_e, cx) => { cx.stop_propagation(); run("tab.close", { id: c.id }); }).child(svg("mods/pane/close.svg").size(12))));
    };
    const root = layout(), sidebar = find(root, "sidebar"), w = window();
    // Old saved rails and a dragged-narrow sidebar can be smaller than the window buttons.
    const lightsPadding = w?.platform === "macos" && w.titlebar === "none" && !w.fullscreen && root?.traffic_lights
      ? Math.max(10, (root.traffic_lights[0] + 64) / settings().zoom - (sidebar?.hidden ? 0 : sidebar?.size?.px ?? 0))
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
    return h_flex().id("tabs-bar").size_full().min_w(0).px(10).pl(lightsPadding).gap(3).items_center().bg(t.chrome).border_b(1).border_color(t.border)
      .track_focus(this.handle).capture_pointer()
      .on_key_down((e, cx) => { if (e.key === "escape" && this.gesture?.kind === "drag") { cx.stop_propagation(); this.settle(false, cx); } })
      .on_mouse_move((e, cx) => this.follow(e.position.x, shown, cx))
      .on_mouse_up("left", (_e, cx) => {
        const g = this.gesture;
        if (g?.kind === "drag") this.settle(true, cx);
        else if (g?.kind === "pressed") { this.gesture = null; run("tab.select", { id: g.id }); }
      })
      .on_scroll_wheel(e => { if (Math.abs(e.delta.y) >= 1) run(e.delta.y > 0 ? "tab.prev" : "tab.next"); })
      .child(h_flex().id("tab-scroll").flex_1().min_w(0).h_full().items_center().gap(GAP).overflow_x_scrollbar()
        .track_scroll(this.scroll).on_size_change(size => { this.view = size.width; }).children(this.row(space, shown, item, t, cx)))
      .child(button("tab-new", "+", () => openOverlay({ mod: "new-menu", kind: "popup", width: 250, height: 512 })))
      .when(enabled.includes("files"), el => el.child(panelButton("files", "Files", "mods/files/file.svg")))
      .when(enabled.includes("changes"), el => el.child(panelButton("changes", "Changes", "mods/changes/changes.svg")))
      .when(w?.buttons, el => el.child(button("minimize", w.platform === "windows" ? svg("mods/tabs/win-min.svg").size(16) : "−", () => run("window.minimize")))
        .child(button("maximize", w.platform === "windows" ? svg(`mods/tabs/${w.maximized ? "win-restore" : "win-max"}.svg`).size(16) : "□", () => run("window.maximize")))
        .child(button("close-window", w.platform === "windows" ? svg("mods/tabs/win-close.svg").size(16) : svg("mods/pane/close.svg").size(13), () => run("window.close"))));
  }
}
