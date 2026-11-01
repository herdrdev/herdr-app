import { View, div, image, svg } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { theme, run, props, window, shared, setShared, updates, layout } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's rail goes by its width, not a flag: dragging its edge narrows it too.
const RAIL_BELOW = 160;
const width = () => find(layout(), "sidebar")?.size?.px ?? Infinity;
const narrow = () => width() < RAIL_BELOW;
// The rail's width: on macOS wide enough for the traffic lights.
const railWidth = () => (window()?.platform === "macos" ? 76 : 44);
// macOS draws the traffic lights over the brand row unless in full screen.
const lights = () => window()?.platform === "macos" && !window()?.fullscreen;

// To the rail, or back to the width it had.
function toggleSidebar() {
  if (!narrow()) setShared("sidebar.width", width());
  // Mods edited before the rail went by width still read the flag.
  setShared("sidebar.collapsed", !narrow());
  run("layout.resize", { id: "sidebar", px: narrow() ? (shared("sidebar.width") ?? 268) : railWidth() });
}

// A drag let go between the rail and the full sidebar draws neither well (and
// on macOS, narrower than the rail, the traffic lights spill onto the tabs):
// it snaps to whichever is nearer. Divider drags publish the layout once, on
// release, so this never fights the pointer. Returns the width to snap to.
export function snapWidth(px, rail) {
  if (!Number.isFinite(px) || px === rail || px >= RAIL_BELOW) return null;
  return px < (rail + RAIL_BELOW) / 2 ? rail : RAIL_BELOW;
}

// What fits in the brand row beside its two buttons, most important first:
// the buttons, then the logo, then the name. Nothing is drawn half-hidden.
export function brandFits(px, inset) {
  const room = px - inset - 10 - 26 - 4 - 26 - 4; // right padding, two buttons and gaps
  return { logo: room >= 20, name: room >= 20 + 8 + 40 };
}

export default class WorkspaceChrome extends View {
  button(id, label, tip, action) {
    const t = theme();
    return div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.muted).hover(s => s.bg(t.hover).text_color(t.foreground))
      .tooltip(tip).on_mouse_down("left", (_e, cx) => cx.stop_propagation()).on_click(action).child(label);
  }
  // One copy (the brand row's) snaps the sidebar; once per width, so a failed resize isn't retried every frame.
  snap() {
    const px = width();
    const to = snapWidth(px, railWidth());
    if (to === null || this.snapped === px) return;
    this.snapped = px;
    run("layout.resize", { id: "sidebar", px: to });
  }
  render() {
    const t = theme();
    const collapsed = narrow();
    if (props()?.footer) {
      // On the rail: the icons alone, centered, their names in tooltips.
      const row = (id, icon, label, dim, action, dot) => h_flex().id(id).h(28).px(collapsed ? 0 : 8).gap(9).items_center().rounded(8)
        .when(collapsed, el => el.justify_center().tooltip(label))
        .text_color(dim ? t.dim : t.muted).hover(s => s.bg(t.hover).text_color(t.foreground)).cursor_pointer()
        .on_click(action)
        .child(div().relative().flex_none().child(svg(`mods/workspace-chrome/${icon}.svg`).size(14))
          .when(dot && collapsed, el => el.child(div().absolute().top(-2).right(-3).size(6).rounded_full().bg(t.accent))))
        .when(!collapsed, el => el.child(label).when(dot, el => el.child(div().size(6).rounded_full().bg(t.accent))));
      return v_flex().size_full().bg(t.chrome).border_t(1).border_color(t.border).px(collapsed ? 4 : 8).pt(4).pb(8).text_size(13)
        .child(row("settings", "settings", "Settings", false, () => run("app.settings"), Boolean(updates().app.ready)))
        .child(row("machine", "plus", "Add machine", true, () => run("mod.open", { name: "machine-add" }), false));
    }
    this.snap();
    const toggle = this.button("sidebar-toggle", svg("mods/workspace-chrome/sidebar.svg").size(15), collapsed ? "Expand sidebar" : "Collapse sidebar", toggleSidebar);
    if (collapsed) {
      // The rail's row: its toggle, centered, where the traffic lights leave room
      // (the agents list draws it at the bottom of the rail where they don't).
      return h_flex().size_full().bg(t.chrome).items_center().justify_center()
        .when(!lights(), el => el.child(toggle));
    }
    const inset = lights() ? 76 : 14;
    const fits = brandFits(width(), inset);
    return h_flex().size_full().bg(t.chrome).pl(inset).pr(10).gap(4).items_center().text_size(13.5)
      .child(h_flex().flex_1().min_w(0).gap(8).items_center().overflow_hidden()
        .when(fits.logo, el => el.child(image("frame/welcome/herdr.png").size(20).flex_none().rounded(5)))
        .when(fits.name, el => el.child(div().min_w(0).truncate().font_semibold().text_color(t.foreground).child("Herdr"))))
      .child(this.button("search", svg("mods/workspace-chrome/search.svg").size(15), "Search", () => run("mod.open", { name: "picker" })))
      .child(toggle);
  }
}
