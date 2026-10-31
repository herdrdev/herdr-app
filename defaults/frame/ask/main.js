// Change anything (app.ask): one line to an agent, which changes Herdr from
// its home folder. Sent, the box says so, then flies into the sidebar's
// Settings row, where the conversation shows working; asking again opens it.
// The app's own mod: rewritten from the binary on every start.

import { View, div, svg, image } from "gpui-kit";
import { h_flex, v_flex, InputState, Input } from "gpui-base";
import { theme, run, props, closeOverlay, knownAgents, hc, shared, setShared, layout, settings, notify } from "herdr";

const W = 560;
const H = 46;
// How long "modding" shows before the box flies, and the flight, ms.
const SHOWN = 1200;
const FLY = 450;
// The spinner it lands on, px.
const SPINNER = 12;
// Agents that take their first message another way than as the last argument.
const MESSAGE = { opencode: (m) => ["--prompt", m], gemini: (m) => ["-i", m], copilot: (m) => ["-i", m] };
// What every agent is told before the request: AGENTS.md, in the folder it
// starts in, is the map to the rest.
const brief = (request) => `You're in Herdr's app folder. The person wants to change Herdr: read AGENTS.md
here first (it covers mods, layouts, themes, settings and commands), make the
change, and check it works in the running app.

Their request: ${request}`;

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

export default class Ask extends View {
  init(_props, cx) {
    this.phase = "ask";
    // The last one asked shows at once; checking what's installed takes a moment.
    const last = shared("ask.agent");
    this.agent = last?.run ? last : undefined;
    this.agents = this.agent ? [this.agent] : [];
    this.input = InputState.new({ placeholder: "Change anything…" });
    this.input.on("submit", (_e, cx) => this.send(cx));
    this.input.focus();
    cx.timer.after(600, () => this.input.focus());
    this.handle = cx.focus_handle();
    cx.spawn(async (cx) => {
      try {
        this.agents = (await knownAgents()).filter((a) => a.installed);
      } catch {}
      // Picked from the list meanwhile: keep it.
      this.agent = this.agents.find((a) => a.key === this.agent?.key) ?? this.agents[0];
      this.loaded = true;
      cx.notify();
    });
  }

  close() {
    closeOverlay(props().overlay);
  }

  send(cx) {
    const text = this.input.value().trim();
    const agent = this.agent;
    if (!text || !agent || this.phase !== "ask") return;
    this.phase = "sent";
    this.picking = false;
    setShared("ask.agent", { key: agent.key, name: agent.name, icon: agent.icon, tint: agent.tint, run: agent.run });
    run("app.ask", { argv: [...agent.run, ...(MESSAGE[agent.key] ?? ((m) => [m]))(brief(text))] });
    cx.notify();
    cx.timer.after(SHOWN, (cx) => cx.spawn(async (cx) => {
      // Not started (the app said why), or the agent quit at once: say why, and
      // keep the message to send again.
      const address = shared("ask");
      const got = address && await hc(["get", address, "--json"]).catch(() => null);
      let state = null;
      try { state = JSON.parse(got.stdout).state; } catch {}
      if (!address || (state && state !== "live")) {
        if (address) {
          const screen = await hc(["read", address]).catch(() => null);
          const lines = (screen?.stdout ?? "").split("\n").map((l) => l.trim()).filter(Boolean);
          notify({ title: `${agent.name} stopped`, body: lines.slice(-3).join("\n") || undefined, level: "error" });
        }
        this.phase = "ask";
        this.input.focus();
        cx.notify();
        return;
      }
      this.phase = "fly";
      cx.notify();
      cx.timer.after(FLY, () => this.close());
    }));
  }

  // Where the box is: the middle, then the spinner the conversation shows in
  // the sidebar's footer (workspace-chrome draws it there: the rail's row
  // starts with it, the wide row ends with it), or it fades where it is when
  // the layout has no such footer on the left.
  rect() {
    const { width, height } = this.size;
    const w = this.width();
    const mid = { x: (width - w) / 2, y: Math.round(height * 0.4 - H / 2), w, h: H };
    if (this.phase !== "fly") return mid;
    const root = layout();
    const sidebar = find(root, "sidebar");
    const footer = find(root, "sidebar-footer");
    if (!footer || sidebar?.hidden || root?.children?.[0] !== sidebar) return { ...mid, x: mid.x + 40, w: w - 80 };
    const wide = sidebar.size?.px ?? 268;
    // Below the footer's top border: the rail's rows are 21 px from 2 px down
    // (Change anything is its second row), the wide row 28 px from 4 px down.
    const top = height - (footer.size?.px ?? 69) + 1;
    const [x, middle] = wide < 160 ? [16, top + 2 + 21 + 10.5] : [wide - 16 - SPINNER, top + 4 + 14];
    return { x, y: middle - SPINNER / 2, w: SPINNER, h: SPINNER };
  }

  // Narrow windows (or a big zoom) get a narrower box.
  width() {
    return Math.min(W, this.size.width - 32);
  }

  agentIcon(a, size) {
    const t = theme();
    if (!a) return div().size(size);
    return a.tint ? svg(a.icon).size(size).text_color(t.foreground) : image(a.icon).size(size);
  }

  render() {
    const t = theme();
    const fly = this.phase === "fly";
    // Measured at the person's zoom (which can change while it's open); the
    // box is placed before it.
    const z = settings().zoom ?? 1;
    this.size = this.measured && { width: this.measured.width / z, height: this.measured.height / z };
    const r = this.size && this.rect();
    const motion = { duration: FLY, easing: "ease-in-out" };
    const box = r && div()
      .id("ask-box")
      .absolute()
      .left(r.x).top(r.y).w(r.w).h(r.h)
      .opacity(fly ? 0 : 1)
      .transition("left", motion).transition("top", motion).transition("width", motion).transition("height", motion)
      .transition("opacity", { duration: FLY, easing: "ease-in" })
      .flex().items_center()
      // In flight its edge takes the spinner's color: it becomes the spinner.
      .rounded(10).border_1().border_color(fly ? t.working : t.border).bg(t.popover).shadow_lg().overflow_hidden()
      .font_family(settings().font_family).text_size(13).text_color(t.foreground)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      // Full width while the box shrinks around it, gone before it lands.
      .child(h_flex().id("ask-line").flex_none().w(this.width() - 2).h_full().px(14).gap(10).items_center()
        .opacity(fly ? 0 : 1).transition("opacity", 150)
        .child(h_flex().id("ask-agent").flex_none().gap(3).items_center().rounded(5).cursor_pointer()
          .on_click((_e, cx) => {
            if (this.phase !== "ask" || this.agents.length < 2) return;
            this.picking = !this.picking;
            this.input.focus();
            cx.notify();
          })
          .child(this.agentIcon(this.agent, 16))
          .when(this.agents.length > 1 && this.phase === "ask", (el) => el.child(svg("frame/ask/chevron-down.svg").size(11).text_color(t.dim))))
        .child(div().flex_none().text_color(t.dim).child("❯"))
        .when(this.phase === "ask", (el) => el.child(this.loaded && !this.agent
          ? div().flex_1().text_color(t.dim).child("No agents installed")
          : Input.new(this.input).flex_1().h_full().text_size(13).text_color(t.foreground)))
        .when(this.phase !== "ask", (el) => el
          .child(svg("frame/ask/spinner.svg").size(13).flex_none().text_color(t.working).spin(900))
          .child(div().flex_1().min_w(0).truncate().text_color(t.working).child("Your agent is modding Herdr…"))));
    // Many agents in a short window (or at a big zoom): the rest scroll.
    const top = r && r.y + H + 6;
    const list = r && this.picking && v_flex()
      .id("ask-agents")
      .absolute().left(r.x).top(top).w(240).max_h(Math.max(38, this.size.height - top - 16)).overflow_y_scroll().p(4)
      .rounded(8).border_1().border_color(t.border).bg(t.popover).shadow_lg().text_size(12.8)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .children(this.agents.map((a) => h_flex().id(`ask-${a.key}`).flex_none().h(30).px(8).gap(10).items_center().rounded(6).cursor_pointer()
        .when(a === this.agent, (el) => el.bg(t.selected)).hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => { this.agent = a; this.picking = false; this.input.focus(); cx.notify(); })
        .child(this.agentIcon(a, 14)).child(div().text_color(t.foreground).child(a.name))));
    return div()
      .id("ask")
      .track_focus(this.handle)
      .size_full()
      .relative()
      .on_size_change((size, cx) => { this.measured = size; cx.notify(); })
      // A press beside the box closes it (and never drags the window).
      .on_mouse_down("left", (_e, cx) => {
        cx.stop_propagation();
        if (this.picking) { this.picking = false; cx.notify(); }
        else if (this.phase === "ask") this.close();
      })
      .on_key_down((e, cx) => {
        // Tab picks the next agent (shift: the one before).
        if (e.key === "tab" && this.phase === "ask" && this.agents.length > 1) {
          cx.stop_propagation();
          const i = this.agents.findIndex((a) => a.key === this.agent?.key);
          this.agent = this.agents[(i + (e.modifiers.shift ? -1 : 1) + this.agents.length) % this.agents.length];
          cx.notify();
          return;
        }
        if (e.key !== "escape") return;
        cx.stop_propagation();
        if (this.picking) { this.picking = false; cx.notify(); }
        // Once sent, it closes by itself after landing.
        else if (this.phase === "ask") this.close();
      })
      .child(div().absolute().inset_0().bg("#000000").opacity(this.phase === "ask" ? 0.35 : 0).transition("opacity", 200))
      .when(Boolean(box), (el) => el.child(box))
      .when(Boolean(list), (el) => el.child(list));
  }
}
