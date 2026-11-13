import { t as tr } from "herdr";
// The Office Assistant, floating over the desktop. When an agent needs you
// (blocked) or finished while you looked away (done), it pops a balloon that
// takes you there. Click it for a tip. Display Properties (or its balloon)
// hides it; Start > Programs > Accessories brings it back.

import { div, image } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { agents, run, shared, setShared, placement } from "herdr";
import { C, ASSETS, unit, root, button, label, sfx, own } from "./kit.js";

const TIPS = [
  tr("win98-it-looks-like-you-re-herding-agents-did-you-know-ctrl-esc-opens-the-start-menu"),
  tr("win98-click-a-tab-s-taskbar-button-while-it-s-shown-to-minimize-it-herdr-settles-it-click-a"),
  tr("win98-ctrl-shift-esc-opens-close-program-every-terminal-herdr-runs-and-end-task"),
  tr("win98-right-click-a-folder-on-the-desktop-to-rename-or-close-that-space"),
  tr("win98-waiting-on-an-agent-mines-is-on-the-desktop-i-won-t-tell-anyone"),
  tr("win98-double-click-a-window-s-title-bar-to-maximize-it-double-click-again-to-restore"),
  tr("win98-start-run-opens-any-command-line-in-a-new-window-like-it-s-1998"),
  tr("win98-shut-down-restart-in-dos-mode-takes-you-to-the-terminals-layout"),
  tr("win98-log-off-takes-you-back-to-herdr-s-normal-layout-i-ll-miss-you"),
];

export default class Clippy {
  init() {
    this.dismissed = new Set();
    this.tip = null;
  }

  news() {
    const list = agents()
      .filter((a) => (a.status === "blocked" || a.status === "done") && !a.focused)
      .filter((a) => !this.dismissed.has(`${a.address}:${a.status}`))
      .sort((a, b) => (a.status === "blocked") - (b.status === "blocked") || a.seq - b.seq);
    return list.pop() ?? null;
  }

  render(_cx) {
    if (shared("win98.clippy") === "off") return div();
    const u = unit();
    const a = this.news();
    const right = placement()?.edge !== "left";
    let balloon = null;
    if (a) {
      const name = a.program?.name ?? a.app ?? tr("win98-your-agent");
      const where = `'${a.space_name} \\ ${a.tab_name}'`;
      const text =
        a.status === "blocked"
          ? tr("win98-it-looks-like-in-is-waiting-for-you-would-you-like-help", { value1: name, value2: where })
          : tr("win98-it-looks-like-finished-in-want-to-see-what-it-did", { value1: name, value2: where });
      balloon = this.balloon(u, text, [
        [tr("win98-take-me-there"), () => {
          run("agent.focus", { terminal: a.address });
          this.dismissed.add(`${a.address}:${a.status}`);
        }],
        [tr("win98-not-now"), () => this.dismissed.add(`${a.address}:${a.status}`)],
      ]);
    } else if (this.tip !== null) {
      balloon = this.balloon(u, TIPS[this.tip % TIPS.length], [
        [tr("win98-next-tip"), () => (this.tip = (this.tip + 1) % TIPS.length)],
        [tr("win98-close"), () => (this.tip = null)],
        [tr("win98-hide-me"), () => setShared("win98.clippy", "off")],
      ]);
    }
    // Clean the dismissed list once their agents moved on.
    for (const k of [...this.dismissed]) {
      const [addr, st] = k.split(":");
      if (!agents().some((x) => x.address === addr && x.status === st)) this.dismissed.delete(k);
    }
    const clip = own(
      div()
        .id("clippy")
        .relative()
        .flex_none()
        .cursor_pointer()
        .when(Boolean(a), (el) => el.loop("top", { from: 0, to: u(-5), period: 800 }))
        .on_click((_e, cx) => {
          this.tip = this.tip === null ? Math.floor(Math.random() * TIPS.length) : null;
          sfx("clippy");
          cx.notify();
        })
        .child(image(`${ASSETS}/img/clippy.svg`).w(u(64)).h(u(80))),
    );
    return root(
      u,
      v_flex()
        .p(u(4))
        .gap(u(2))
        .map((el) => (right ? el.items_end() : el.items_start())),
    )
      .when(Boolean(balloon), (el) => el.child(balloon))
      .child(clip);
  }

  balloon(u, text, actions) {
    return v_flex()
      .w(u(200))
      .p(u(8))
      .gap(u(8))
      .bg(C.tip)
      .border(u(1))
      .border_color(C.text)
      .rounded(u(8))
      .child(
        v_flex().children(
          String(text)
            .split("\n")
            .map((l) => div().child(l || " ")),
        ),
      )
      .child(
        v_flex()
          .gap(u(3))
          .children(
            actions.map(([t, fn], i) =>
              own(
                h_flex()
                  .id(`clip-${i}`)
                  .gap(u(5))
                  .items_center()
                  .cursor_pointer()
                  .on_click((_e, cx) => {
                    fn();
                    cx.notify();
                  })
                  .child(div().w(u(7)).h(u(7)).rounded(u(4)).bg(C.navy))
                  .child(div().text_color(C.navy).underline().child(t)),
              ),
            ),
          ),
      );
  }
}
