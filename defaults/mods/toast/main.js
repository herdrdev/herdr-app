// How notifications look. Anything can post one with notify({ title, body,
// level, icon, action }); replace this mod to restyle them all. Click to
// run its action (e.g. go to the agent) and dismiss it.

import { View, div, image } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { agents, panes, spaces, terminals, props, closeOverlay, run, theme } from "herdr";

const DOT = { error: "blocked", warning: "working", success: "done", info: "accent" };

// Same as the agents, tabs and picker mods: drops a status glyph or Codex mark, keeps brackets and emoji.
const clean = (s) => (s ?? "").replace(/^(?:\[ [!.] \] |[✋⏳]\uFE0F?(?=\s)|(?!(?:\p{Emoji}\uFE0F|[#*]\u20E3))[^\p{L}\p{N}\p{Ps}\p{Pi}\p{Emoji_Presentation}])+/u, "").trim();

function rootAddress(addr, all) {
  let cur = addr;
  const seen = new Set();
  while (cur && !seen.has(cur)) {
    seen.add(cur);
    const term = all.find((t) => t.address === cur);
    if (!term?.parent) break;
    cur = term.parent;
  }
  return cur;
}

function title(tab, lead) {
  const prog = lead?.program?.name;
  if (!lead?.program?.agent) return tab.name;
  const live = clean(lead.title);
  // clean() would strip a path's leading ~ or /: test the title with only its status glyph gone.
  const path = /^[~/]|^[A-Za-z]:\\/.test((lead.title ?? "").replace(/^[^\p{L}\p{N}~/]+/u, ""));
  const telling = live && live !== (lead.terminal ?? lead.address) && live !== prog && !path && !live.includes("@") && live.toLowerCase() !== lead.program.key;
  return (tab.name === prog || !tab.named) && telling ? live : tab.name;
}

// Agent alerts arrive with "<space> / <tab.name>"; resolve an unnamed or
// program-named tab to its agent's live session title, as tabs and agents do.
function agentBody(p) {
  if (p.action?.name !== "agent.focus") return null;
  const addr = p.action.args?.terminal;
  if (typeof addr !== "string" || !addr) return null;
  const root = rootAddress(addr, terminals());
  const allPanes = panes();
  const allAgents = agents();
  const pane = allPanes.find((x) => x.terminal === addr) ?? allPanes.find((x) => x.terminal === root);
  const agent = allAgents.find((x) => x.address === addr) ?? allAgents.find((x) => x.address === root);
  const tabId = pane?.tab ?? agent?.tab;
  if (!tabId) return null;
  for (const space of spaces()) {
    const tab = space.tabs.find((tb) => tb.id === tabId);
    if (!tab) continue;
    const lead = pane ?? agent ?? allPanes.find((x) => x.tab === tab.id && x.program?.agent) ?? allPanes.find((x) => x.tab === tab.id);
    const raw = `${space.name} / ${tab.name}`;
    const nice = `${space.name} / ${title(tab, lead)}`;
    if (!p.body || p.body === raw) return nice;
    if (typeof p.body === "string" && p.body.includes(raw)) return p.body.replace(raw, nice);
    return null;
  }
  return null;
}

export default class Toast extends View {
  render(cx) {
    const t = theme();
    const p = props() ?? {};
    const login = /^([a-z0-9][a-z0-9_.-]{0,31}) needs login; run hc machine login \1$/.exec(p.body ?? "");
    const command = login ? `hc machine login ${login[1]}` : null;
    const resolved = !login ? agentBody(p) : null;
    if (resolved !== null) this.body = resolved;
    const body = resolved ?? this.body ?? p.body;
    const text = (value) => p.fitHeight
      ? div().child(value)
      : div().truncate().child(typeof value === "string" ? value.split("\n", 1)[0] : value);
    return h_flex()
      .id("toast")
      .when(p.fitHeight, (el) => el.w_full().min_h(70).flex_none().py(12))
      .when(!p.fitHeight, (el) => el.size_full())
      .px(18)
      .gap(12)
      .items_center()
      .bg(t.popover)
      .cursor_pointer()
      .hover((s) => s.bg(t.hover))
      .on_click(() => {
        if (p.action?.name) run(p.action.name, p.action.args ?? {});
        closeOverlay(p.overlay);
      })
      .child(p.icon ? image(p.icon).w(18).h(18).flex_none() : div().size(8).rounded(4).flex_none().bg(t[DOT[p.level] ?? "accent"]))
      .child(
        v_flex()
          .flex_1()
          .overflow_hidden()
          .child(text(login ? `${login[1]} needs a login: run` : p.title ?? "").font_semibold().text_size(13).text_color(t.foreground))
          .when(!login && Boolean(body), (el) => el.child(text(body).text_size(12).text_color(t.muted)))
          .when(Boolean(login), (el) => el.child(h_flex().gap(8).items_center()
            .child(div().flex_1().min_w(0).truncate().tooltip(command).text_size(12).text_color(t.muted).child(command))
            .child(div().id("toast-login-copy").flex_none().px(6).py(3).rounded(4).bg(t.raised)
              .text_size(12).text_color(t.foreground).cursor_pointer().hover(s => s.bg(t.hover))
              .on_click((_e, cx) => { cx.stop_propagation(); cx.write_to_clipboard(command); this.copied = true; cx.notify(); })
              .child(this.copied ? "Copied" : "Copy")))),
      );
  }
}
