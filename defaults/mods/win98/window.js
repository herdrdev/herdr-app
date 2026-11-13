import { t as tr } from "herdr";
// A pane's title bar (the layout's pane header): the focused pane's is blue,
// the others gray. _ minimizes the tab to the taskbar (settle), [] maximizes
// the pane (zoom), x closes it. Double-click maximizes; right-click is the
// window menu.

import { div } from "gpui-kit";
import { panes, props, run, home } from "herdr";
import { C, unit, root, titleBar, programIcon, icon, dosPath, openMenu } from "./kit.js";

export function paneTitle(p) {
  if (!p) return "";
  if (p.mod) return p.title || p.mod;
  const prog = p.program;
  if (!prog || prog.shell) return tr("win98-dos-prompt-2", { value1: dosPath(p.cwd, home()) });
  const t = (p.title ?? "").trim();
  return t && t !== prog.name ? `${prog.name} - ${t}` : `${prog.name} - ${dosPath(p.cwd, home())}`;
}

export default class PaneWindow {
  init() {}

  render(_cx) {
    const u = unit();
    const id = props()?.pane;
    const p = panes().find((x) => x.pane === id);
    const active = Boolean(p?.focused);
    const zoomed = Boolean(p?.zoomed);
    const menu = () =>
      openMenu([
        { label: tr("win98-restore"), act: { run: ["pane.zoom", { pane: id }] }, disabled: !zoomed },
        { label: tr("win98-mi-nimize"), act: { run: ["tab.settle", { id: p?.tab }] } },
        { label: tr("win98-ma-ximize"), act: { run: ["pane.zoom", { pane: id }] }, disabled: zoomed },
        { sep: true },
        { label: tr("win98-split-right"), shortcut: "Ctrl+Shift+O", act: { run: ["pane.split", { direction: "right", pane: id }] } },
        { label: tr("win98-split-down"), shortcut: "Ctrl+Shift+E", act: { run: ["pane.split", { direction: "down", pane: id }] } },
        { sep: true },
        { label: tr("win98-close-2"), bold: true, shortcut: "Alt+F4", act: { run: ["pane.close", { pane: id }] } },
      ]);
    const bar = titleBar(u, {
      id: `pw-${id}`,
      fill: true,
      title: paneTitle(p),
      active,
      icon: p?.mod ? icon(u, "programs", 16) : programIcon(u, p?.program, 16),
      controls: [
        ["min", () => run("tab.settle", { id: p?.tab }), tr("win98-minimize")],
        [zoomed ? "restore" : "max", () => run("pane.zoom", { pane: id }), zoomed ? tr("win98-restore-2") : tr("win98-maximize")],
        ["close", () => run("pane.close", { pane: id }), tr("win98-close")],
      ],
      onClick: () => run("pane.focus", { pane: id }),
      onDouble: () => run("pane.zoom", { pane: id }),
      onRight: menu,
    });
    return root(u, div().size_full().flex().flex_col().bg(C.face).pb(u(1))).child(bar);
  }
}
