import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { props, theme, checking, appHome, exec, run, openUrlSystem, closeOverlay } from "herdr";
import { isRemoteLoopbackUrl } from "./policy.js";

export default class RemoteLinks extends View {
  init(_props, cx) {
    const p = props() || {};
    // Capture the explicit source. Never retarget a click to the focused terminal.
    this.origin = { url: p.url, terminal: p.terminal, machine: p.machine };
    this.runner = p.runner || appHome() + "/mods/remote-links/runner.ts";
    this.state = "ready";
    this.message = "";
    this.session = null;
    this.overlay = p.overlay;
    this.activated = !!p.activated;
    if (this.activated && !checking()) {
      if (!isRemoteLoopbackUrl(this.origin)) {
        openUrlSystem(this.origin.url);
        closeOverlay(this.overlay);
      } else {
        this.open(cx);
      }
    }
  }

  open(cx) {
    if (checking() || this.state === "opening") return;
    const { url, terminal, machine } = this.origin;
    if (![url, terminal, machine].every(v => typeof v === "string" && v.length)) {
      this.state = "error";
      this.message = "This link needs an explicit URL, source terminal and remote machine.";
      cx.notify();
      return;
    }
    this.state = "opening";
    this.message = "";
    cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec("bun", [this.runner, "open", machine, terminal, url]);
        if (result.code !== 0) throw new Error(result.stderr || result.stdout);
        this.session = JSON.parse(result.stdout.trim());
        this.state = "open";
        if (this.activated && this.overlay) closeOverlay(this.overlay);
      } catch (error) {
        this.state = "error";
        this.message = String(error);
      }
      cx.notify();
    });
  }

  render(cx) {
    const t = theme();
    const machine = this.origin.machine || "remote machine";
    const opening = this.state === "opening";
    const failed = this.state === "error";
    const connected = !!this.session;
    const title = opening ? "Connecting to " + machine : failed ? "Couldn’t connect to " + machine
      : connected ? "Your browser is ready" : "Open remote localhost";
    const subtitle = failed ? "The link has not been opened on this computer."
      : "A local Chrome window, connected to your remote service.";
    const button = (id, label, action, primary = false) => div().id(id).px(12).py(7).rounded(6)
      .text_size(12).font_semibold().bg(primary ? t.solid : t.raised)
      .text_color(primary ? t.solid_foreground : t.foreground)
      .hover(s => s.bg(primary ? t.accent : t.hover).text_color(primary ? t.accent_foreground : t.foreground))
      .on_click(action).child(label);
    return v_flex().size_full().p(22).gap(16).text_size(13).text_color(t.foreground)
      .child(h_flex().gap(12).items_center()
        .child(h_flex().w(42).h(42).flex_shrink_0().items_center().justify_center().rounded(10).bg(t.raised)
          .child(svg("mods/remote-links/browser.svg").w(23).h(23).text_color(failed ? t.blocked : t.accent)))
        .child(v_flex().flex_1().min_w(0).gap(4)
          .child(div().text_size(18).font_semibold().truncate().child(title))
          .child(div().text_size(12).text_color(t.muted).child(subtitle))))
      .child(v_flex().p(12).gap(6).rounded(8).bg(t.raised).border(1).border_color(t.border)
        .child(h_flex().gap(8).items_center()
          .child(div().text_size(10).font_semibold().text_color(t.accent).child("REMOTE LOCALHOST"))
          .child(div().flex_1().min_w(0).text_size(11).text_color(t.dim).truncate().child(this.origin.terminal || "No source terminal")))
        .child(div().text_size(14).truncate().child(this.origin.url || "No URL supplied")))
      .child(h_flex().gap(8).items_center().text_size(12).text_color(t.muted)
        .child(div().child("Chrome here"))
        .child(div().text_color(t.dim).child("→"))
        .child(div().flex_1().min_w(0).px(7).py(3).rounded(4).bg(t.raised).text_color(t.foreground).truncate().child(machine))
        .child(div().child("over private SSH SOCKS")))
      .when(!!this.message, el => el.child(div().id("connection-error").h(64).overflow_y_scrollbar()
        .text_size(12).text_color(t.blocked).child(this.message)))
      .child(div().flex_1())
      .child(h_flex().gap(10).items_center().justify_between()
        .when(opening, el => el
          .child(h_flex().gap(8).items_center().text_size(12).text_color(t.muted)
            .child(svg("mods/remote-links/spinner.svg").w(15).h(15).text_color(t.accent).spin(900))
            .child(div().child("Starting browser and secure connection…")))
          .child(button("hide", "Hide", () => { if (this.overlay) closeOverlay(this.overlay); })))
        .when(!opening && !connected, el => el
          .child(div().text_size(11).text_color(t.dim).child(failed ? "No local fallback." : "Your normal browser stays untouched."))
          .child(button("open", failed ? "Try again" : "Connect & open", (_event, cx) => this.open(cx), true)))
        .when(connected, el => el
          .child(button("peek", "Inspect connection", () => {
            if (this.session.terminal) run("terminal.peek", { terminal: this.session.terminal });
          }))
          .child(button("stop", "Stop browser & connection", (_event, cx) => {
            if (this.session.terminal) run("terminal.kill", { terminal: this.session.terminal });
            this.session = null;
            this.state = "ready";
            cx.notify();
          }))));
  }
}
