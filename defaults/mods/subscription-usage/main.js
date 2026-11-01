import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { agents, appHome, exec, theme, shared, setShared } from "herdr";
import { providerOf, resetLabel } from "./quota.js";

const CACHE = "subscription-usage.result";
const BUSY = "subscription-usage.started";
const ATTEMPT = "subscription-usage.attempt";
const unavailable = () => ({ status: "unavailable", message: "Requires Node.js 18+ on PATH; check CLI installation", windows: [] });

export default class SubscriptionUsage extends View {
  init(_props, cx) {
    this.refresh(cx);
    cx.timer.every(120000, cx => this.refresh(cx));
  }

  refresh(cx) {
    const now = Date.now();
    // Shared across copies; closing an in-flight popup must not lock out retries.
    if (now - (shared(BUSY) ?? 0) < 35000 || now - (shared(ATTEMPT) ?? 0) < 30000) return;
    setShared(BUSY, now);
    setShared(ATTEMPT, now);
    cx.spawn(async cx => {
      let result;
      try {
        const output = await exec("node", [`${appHome()}/mods/subscription-usage/query.mjs`]);
        if (output.code !== 0) throw new Error("helper unavailable");
        result = JSON.parse(output.stdout);
        if (!result.claude || !result.codex) throw new Error("invalid result");
      } catch { result = { claude: unavailable(), codex: unavailable() }; }
      // A late result cannot replace a newer query after a view was closed.
      if (shared(BUSY) === now) {
        setShared(CACHE, result);
        setShared(BUSY, 0);
      }
      cx.notify();
    });
    cx.notify();
  }

  render() {
    const t = theme();
    const data = shared(CACHE) ?? {};
    const now = Date.now();
    const busy = now - (shared(BUSY) ?? 0) < 35000;
    const list = agents();
    const card = (id, name) => {
      const result = data[id];
      const count = list.filter(a => providerOf(a) === id).length;
      const stale = result?.status === "ok" && now - result.checkedAt > 300000;
      return v_flex().gap(9).p(14).rounded(10).border(1).border_color(t.border).bg(t.raised)
        .child(h_flex().justify_between().items_center()
          .child(div().font_semibold().child(name))
          .child(div().text_size(11).text_color(t.dim).child(`${count} local agents`)))
        .when(!result || result.status !== "ok", el => el.child(div().text_size(12).text_color(t.muted).child(result?.message ?? (busy ? "Checking subscription…" : "Not checked"))))
        .when(result?.status === "ok", el => el.children((result?.windows ?? []).map(w => {
          const expired = w.resetsAt !== null && w.resetsAt <= now;
          const color = stale || expired ? t.dim : w.remaining <= 10 ? t.blocked : t.accent;
          return v_flex().gap(5)
            .child(h_flex().justify_between().gap(12)
              .child(div().text_size(12).text_color(t.muted).child(w.label))
              .child(div().font_semibold().text_color(color).child(`${Math.floor(w.remaining)}% left${stale || expired ? " · stale" : ""}`)))
            .child(h_flex().h(5).rounded(3).bg(t.input).overflow_hidden()
              .when(w.remaining > 0, bar => bar.child(div().h_full().flex_grow(w.remaining).flex_basis(0).bg(color)))
              .when(w.remaining < 100, bar => bar.child(div().h_full().flex_grow(100 - w.remaining).flex_basis(0))))
            .child(div().text_size(10.5).text_color(t.dim).child(resetLabel(w.resetsAt, now)));
        })))
        .when(Boolean(result?.checkedAt), el => el.child(div().text_size(10.5).text_color(t.dim).child(`Checked ${new Date(result.checkedAt).toLocaleTimeString()}`)));
    };
    return v_flex().size_full().p(18).gap(12).bg(t.popover).text_color(t.foreground).text_size(13)
      .child(h_flex().items_center().justify_between()
        .child(div().font_semibold().text_size(16).child("Subscription usage"))
        .child(div().id("usage-refresh").text_size(12).text_color(t.accent).cursor_pointer()
          .on_click((_event, cx) => this.refresh(cx)).child(busy ? "Checking…" : "Refresh")))
      .child(div().text_size(11.5).text_color(t.muted).child("Local default accounts · shared across agents"))
      .child(v_flex().id("usage-cards").flex_1().min_h(0).overflow_y_scrollbar().gap(12).child(card("claude", "Claude Code")).child(card("codex", "Codex")))
      .child(div().text_size(10.5).text_color(t.dim).child("Updates every 2 minutes while open. Remote agents and per-terminal account overrides are not measured. Other providers are not supported yet."));
  }
}
