import { View, div, image, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { theme, run, props, window, shared, setShared, updates, layout, terminals, knownAgents } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's rail goes by its width, not a flag: dragging its edge narrows it too.
const narrow = () => (find(layout(), "sidebar")?.size?.px ?? Infinity) < 160;

// To the rail (on macOS wide enough for the traffic lights), or back to the width it had.
function toggleSidebar() {
  const width = find(layout(), "sidebar")?.size?.px;
  if (!narrow()) setShared("sidebar.width", width);
  // Mods edited before the rail went by width still read the flag.
  setShared("sidebar.collapsed", !narrow());
  run("layout.resize", { id: "sidebar", px: narrow() ? (shared("sidebar.width") ?? 268) : window()?.platform === "macos" ? 76 : 44 });
}

// Change anything's agent that says nothing this long after it starts is
// waiting on the person before its own start (a folder trust question), ms.
const SILENT = 6000;
// Sidebar width with room for "Change anything" beside Settings and a spinner or dot, px.
const ASK_TEXT = 266;

export default class WorkspaceChrome extends View {
  init(_props, cx) {
    this.silentSince = {};
    if (!props()?.footer) return;
    // Only to notice the silence running out; it draws nothing by itself.
    cx.timer.every(1000, cx => { if (this.askStatus() !== this.askShown) cx.notify(); });
    // The full-color agent icons, held below (the app lets go of an image
    // unused for a few seconds, and one loaded afresh shows ~0.3 s late).
    cx.spawn(async cx => {
      try { this.icons = (await knownAgents()).filter(a => !a.tint).map(a => a.icon); } catch {}
      cx.notify();
    });
  }
  // This session's Change anything conversation: working, waiting on you, or done.
  askStatus() {
    const ask = shared("ask");
    const term = ask ? terminals().find(term => term.address === ask) : null;
    if (term?.state !== "live") return null;
    const seen = shared("ask.seen");
    if (term.status === "working") return "working";
    if (term.status === "blocked" || term.status === "error") return "blocked";
    // A finish is news until it's opened (app.ask notes the turn it was opened at).
    if (term.turn > (seen ?? 0)) return "done";
    if (seen != null) return "idle";
    // Before its first message is done, it's on it (some say idle while they
    // start), unless it says nothing for long: then it asks before it starts.
    if (term.status) return "working";
    const since = this.silentSince[ask] ??= Date.now();
    return Date.now() - since > SILENT ? "blocked" : "working";
  }
  button(id, label, action) {
    const t = theme();
    return div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.muted).hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).on_click(action).child(label);
  }
  render() {
    const t = theme();
    const collapsed = narrow();
    if (props()?.footer) {
      // The rail stacks Change anything as a third row in the same height.
      const rowH = collapsed ? 21 : 28;
      const row = (id, icon, label, dim, action) => h_flex().id(id).h(rowH).px(8).gap(9).items_center().rounded(8)
        .text_color(dim ? t.dim : t.muted).hover(s => s.bg(t.hover).text_color(t.foreground)).cursor_pointer()
        .on_click(action).child(svg(`mods/workspace-chrome/${icon}.svg`).size(14).flex_none())
        .when(!collapsed, el => el.child(label));
      const settingsRow = row("settings", "settings", "Settings", false, () => run("app.settings"))
        .when(Boolean(updates().app.ready), el => el.child(div().size(6).rounded_full().bg(t.accent)));
      const status = this.askShown = this.askStatus();
      const spinner = svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.working).spin(900);
      // As the agents list marks one that needs you.
      const needsYou = div().size(14).flex_none().rounded(4).bg(t.blocked + "26").text_size(10.5).font_bold().text_color(t.blocked)
        .flex().items_center().justify_center().child("!");
      const chat = svg("mods/workspace-chrome/message-circle.svg").size(14).flex_none();
      const askRow = h_flex().id("ask").h(rowH).px(8).gap(6).items_center().rounded(8)
        .text_color(t.dim).hover(s => s.bg(t.hover).text_color(t.foreground)).cursor_pointer()
        .on_click(() => run("app.ask"));
      const footer = v_flex().size_full().bg(t.chrome).border_t(1).border_color(t.border).px(8).text_size(13)
        .children((this.icons ?? []).map(icon => image(icon).absolute().size(1).opacity(0)));
      if (collapsed) {
        return footer.pt(2).pb(3)
          .child(settingsRow)
          .child(askRow.child(status === "working" ? spinner : status === "blocked" ? needsYou : chat.when(status === "done", el => el.text_color(t.done))))
          .child(row("machine", "plus", "Add machine", true, () => run("mod.open", { name: "machine-add" })));
      }
      return footer.pt(4).pb(8)
        .child(h_flex().gap(2).child(settingsRow.flex_1().min_w(0))
          .child(askRow
            .when((find(layout(), "sidebar")?.size?.px ?? Infinity) >= ASK_TEXT, el => el.child("Change anything"))
            .child(chat)
            .when(status === "working", el => el.child(spinner))
            .when(status === "blocked", el => el.child(needsYou))
            .when(status === "done", el => el.child(div().size(6).rounded_full().bg(t.done)))))
        .child(row("machine", "plus", "Add machine", true, () => run("mod.open", { name: "machine-add" })));
    }
    return h_flex().size_full().bg(t.chrome).pl(collapsed ? 6 : 14).pr(10).gap(4).items_center().text_size(13.5)
      .when(window()?.platform === "macos" && !collapsed, el => el.pl(76))
      .when(!collapsed, el => el.child(h_flex().flex_1().gap(8).items_center().child(image("frame/welcome/herdr.png").size(20).rounded(5)).child(div().font_semibold().text_color(t.foreground).child("Herdr"))))
      .when(!collapsed, el => el.child(this.button("search", svg("mods/workspace-chrome/search.svg").size(15), () => run("mod.open", { name: "picker" }))))
      // On the macOS rail the traffic lights fill the row; the agents list's » unfolds it.
      .when(!(collapsed && window()?.platform === "macos"), el => el.child(this.button("sidebar-toggle", svg("mods/workspace-chrome/sidebar.svg").size(15), toggleSidebar)));
  }
}
