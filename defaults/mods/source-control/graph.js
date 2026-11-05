import { div, svg, PathBuilder } from "gpui-kit";
import { h_flex, v_flex, v_virtual_list, Scrollbar } from "gpui-base";

export function parseGraph(text) {
  const rows = [];
  for (const line of String(text ?? "").replace(/\r\n?/g, "\n").split("\n")) {
    const [graph, rawHash, refs = "", subject = "", author = "", date = ""] = line.split("\x1f");
    // Preserve topology-only rows, but ignore blank lines and Git headers.
    if (!/^[ *|\\/_.-]*$/.test(graph)) continue;
    if (rawHash === undefined) {
      if (!graph.includes("*") && /[|\\/_-]/.test(graph)) rows.push({ graph, refs, subject, author, date, height: 6 });
      continue;
    }
    const hash = rawHash.trim();
    if (!/^[0-9a-f]{4,64}$/i.test(hash)) continue;
    rows.push({ graph, hash, refs, subject, author, date, height: 22 });
  }
  return rows;
}

export function graphBadges(row, data = {}) {
  data = data || {};
  const tokens = String(row.refs || "").split(", ").filter(Boolean);
  const head = "head" in data ? !!data.head && row.hash === data.head : tokens.some(ref => ref === "HEAD" || ref.startsWith("HEAD -> "));
  let refs = (data.refs || []).filter(ref => ref.hash === row.hash && !ref.symbolicTarget && !/^refs\/remotes\/[^/]+\/HEAD$/.test(ref.ref));
  if (!data.refs) refs = tokens.filter(ref => ref !== "HEAD").map(ref => {
    const type = ref.startsWith("tag: ") ? "tag" : ref.startsWith("HEAD -> ") || !ref.includes("/") ? "branch" : "remote";
    const name = ref.replace(/^(?:HEAD -> |tag: )/, "");
    return { type, name, ref: `refs/${type === "branch" ? "heads" : type === "remote" ? "remotes" : "tags"}/${name}` };
  }).filter(ref => !/\/HEAD$/.test(ref.name));
  const priority = ref => ref.ref === `refs/heads/${data.branch?.name}` ? 0 : ref.ref === data.baseRef ? 1 : ref.ref === data.branch?.upstreamRef ? 2 : 3;
  refs = [...refs].sort((a, b) => priority(a) - priority(b) || a.name.localeCompare(b.name));
  const badges = ["branch", "remote", "tag"].flatMap(type => {
    const group = refs.filter(ref => ref.type === type);
    if (!group.length) return [];
    const current = type === "branch" && group.some(ref => ref.ref === `refs/heads/${data.branch?.name}`);
    return [{ type, current, icon: current ? "target" : type === "remote" ? "cloud" : type, names: group.map(ref => ref.name), count: group.length, label: "" }];
  });
  const important = refs.find(ref => priority(ref) < 3) || refs.find(ref => ref.type === "branch") || refs[0];
  if (important) badges.find(badge => badge.type === important.type).label = important.name;
  return { head, badges };
}

function fileItems(files, hash, view, directories) {
  if (view !== "tree") return files.map(file => ({ kind: "file", key: `file:${hash}:${file.path}`, hash, file, name: file.path, depth: 0, height: 22 }));
  const root = { children: new Map() };
  for (const file of files) {
    const parts = file.path.split("/"); let node = root, path = "";
    parts.forEach((name, index) => {
      path += `${path ? "/" : ""}${name}`;
      const key = `${index === parts.length - 1 ? "file" : "directory"}:${name}`;
      if (!node.children.has(key)) node.children.set(key, { name, path, children: new Map() });
      node = node.children.get(key);
      if (index === parts.length - 1) node.file = file;
    });
  }
  const result = [];
  const visit = (node, depth) => [...node.children.values()].sort((a, b) => Number(!!a.file) - Number(!!b.file) || a.name.localeCompare(b.name)).forEach(child => {
    const closed = directories?.[child.path] === true;
    result.push({ kind: child.file ? "file" : "directory", key: `${child.file ? "file" : "directory"}:${hash}:${child.path}`, hash, file: child.file, path: child.path, name: child.name, depth, closed, height: 22 });
    if (!child.file && !closed) visit(child, depth + 1);
  });
  visit(root, 0);
  return result;
}

export function graphItems(rows, options = {}) {
  const items = [];
  rows.forEach((row, rowIndex) => {
    items.push({ kind: row.hash ? "commit" : "connector", key: row.hash ? `commit:${row.hash}` : `connector:${rowIndex}:${row.graph}`, hash: row.hash, rowIndex, height: row.height });
    const expanded = row.hash && options.expanded?.[row.hash];
    if (!expanded) return;
    let children;
    if (expanded.loading || expanded.error) children = [{ kind: "message", key: `message:${row.hash}`, hash: row.hash, text: expanded.error || "Loading changed files…", error: !!expanded.error, height: 22 }];
    else children = fileItems(expanded.files || [], row.hash, options.fileView, options.directories?.[row.hash]);
    if (!children.length) children.push({ kind: "message", key: `message:${row.hash}`, hash: row.hash, text: "No changed files", height: 22 });
    children.forEach(item => items.push({ ...item, rowIndex }));
  });
  return items;
}

export function graphIndex(rows, hash, options = {}) {
  return graphItems(rows, options).findIndex(item => item.kind === "commit" && item.hash === hash);
}

function graphCell(rows, index, width, t, head = false) {
  const row = rows[index], height = row.height, middle = height / 2;
  const previous = rows[index - 1]?.graph ?? "", next = rows[index + 1]?.graph ?? "";
  const colors = [t.ansi4, t.ansi5, t.ansi2, t.ansi3, t.ansi6];
  const cell = div().w(width).h(height).flex_none().relative();
  [...row.graph].forEach((char, column) => {
    if (char === " ") return;
    const x = 7 + column * 7, color = colors[Math.floor(column / 2) % colors.length];
    const path = PathBuilder.stroke(1.4);
    if (char === "|") path.move_to(x, 0).line_to(x, height);
    else if (char === "/") path.move_to(x + 7, 0).line_to(x - 7, height);
    else if (char === "\\") path.move_to(x - 7, 0).line_to(x + 7, height);
    else if (char === "_" || char === "-") path.move_to(x - 7, middle).line_to(x + 7, middle);
    else if (char === "*") {
      const above = "|*".includes(previous[column] ?? " ") || previous[column + 1] === "/" || previous[column - 1] === "\\";
      const below = "|*".includes(next[column] ?? " ") || next[column + 1] === "\\" || next[column - 1] === "/";
      path.move_to(x, above ? 0 : middle).line_to(x, below ? height : middle);
    } else return;
    cell.child(window.paint_path(path.build(), color).absolute().inset_0().size_full());
    if (char === "*") {
      cell.child(div().absolute().left(x - 3.5).top(middle - 3.5).size(7).rounded(4).bg(head ? t.ansi4 : color));
      if (head) cell.child(div().absolute().left(x - 5.5).top(middle - 5.5).size(11).rounded(6).border_1().border_color(t.ansi4));
    }
  });
  return cell;
}

export function renderGraph(rows, t, options = {}) {
  const width = rows.reduce((width, row) => Math.max(width, row.graph.trimEnd().length * 7 + 9), 20);
  const items = graphItems(rows, options), icon = (name, color = t.muted) => svg(`mods/source-control/icons/${name}.svg`).size(12).flex_none().text_color(color);
  const byKey = new Map(items.map(item => [item.key, item]));
  const continuation = index => {
    const row = rows[index], next = rows[index + 1]?.graph || "", lanes = [];
    [...row.graph].forEach((char, column) => {
      if (char === "|" || char === "*" && ("|*".includes(next[column] || " ") || next[column + 1] === "\\" || next[column - 1] === "/")) lanes[column] = "|";
      else if (char === "/" && column > 0) lanes[column - 1] = "|";
      else if (char === "\\") lanes[column + 1] = "|";
    });
    return Array.from({ length: lanes.length }, (_, index) => lanes[index] || " ").join("");
  };
  const render = item => {
    const row = rows[item.rowIndex], info = item.kind === "commit" ? graphBadges(row, options.data) : null;
    const cell = item.kind === "commit" || item.kind === "connector" ? graphCell(rows, item.rowIndex, width, t, info?.head)
      : graphCell([{ graph: continuation(item.rowIndex), height: item.height }], 0, width, t);
    const line = h_flex().id(item.key).w_full().h(item.height).min_w(0).px(6).gap(4).items_center().child(cell);
    if (item.kind === "connector") return line;
    if (item.kind !== "commit") {
      line.child(div().w(item.depth ? item.depth * 14 : 0).flex_none());
      if (item.kind === "message") return line.text_color(item.error ? t.ansi1 : t.muted).text_size(11).child(div().truncate().tooltip(item.text).child(item.text));
      if (item.kind === "directory") return line.hover(s => s.bg(t.hover)).tooltip(item.path)
        .when(typeof options.onToggleDirectory === "function", e => e.cursor_pointer())
        .child(icon(item.closed ? "chevron-right" : "chevron-down")).child(icon("folder")).child(div().min_w(0).truncate().child(item.name));
      const status = item.file.status || "M", color = status === "A" ? t.ansi2 : status === "D" ? t.ansi1 : t.ansi3;
      return line.hover(s => s.bg(t.hover)).tooltip(`${item.file.oldPath ? `${item.file.oldPath} → ` : ""}${item.file.path}\n${status}`)
        .when(typeof options.onFile === "function", e => e.cursor_pointer())
        .child(icon("file", color)).child(div().flex_1().min_w(0).truncate().text_size(11).child(item.name)).child(div().w(16).text_size(10).text_color(color).child(status));
    }
    const selected = (options.selected?.hash ?? options.selected) === row.hash;
    return line.when(selected, e => e.bg(t.selected)).hover(s => s.bg(selected ? t.selected : t.hover))
      .when(typeof options.onSelect === "function", e => e.cursor_pointer())
      .tooltip(`${row.subject}\n${row.hash} · ${row.author} · ${row.date}${row.refs ? `\n${row.refs}` : ""}`)
      .child(icon(options.expanded?.[row.hash] ? "chevron-down" : "chevron-right"))
      .child(div().flex_1().min_w(0).truncate().text_size(12).text_color(t.foreground).when(info.head, e => e.font_semibold()).child(row.subject))
      .children(info.badges.map(badge => h_flex().gap(3).flex_none().max_w(105).px(4).py(1).rounded(3).bg(badge.current ? t.selected : t.raised)
        .tooltip(badge.names.join("\n")).child(icon(badge.icon, badge.type === "remote" ? t.ansi5 : badge.type === "tag" ? t.ansi3 : t.ansi4))
        .when(!!badge.label, e => e.child(div().min_w(0).truncate().text_size(10).child(badge.label)))
        .when(badge.count > 1, e => e.child(div().text_size(10).child(String(badge.count))))));
  };
  return v_flex().id("source-control-graph-list").flex_1().min_h(0).min_w(0).relative()
    .child(v_virtual_list("source-control-graph-rows", items.length, items.map(item => item.height), index => items[index].key,
      range => items.slice(range.start, range.end).map(render)).size_full().when(!!options.scroll, e => e.track_scroll(options.scroll))
      // Virtual rows are rebuilt during layout; only the list may register handlers.
      .on_item_click((key, cx) => {
        const item = byKey.get(key);
        if (!item) return;
        cx.stop_propagation();
        if (item.kind === "commit") options.onSelect?.(rows[item.rowIndex], cx);
        else if (item.kind === "file") options.onFile?.(item.file, item.hash, cx);
        else if (item.kind === "directory") options.onToggleDirectory?.(item.hash, item.path, cx);
      })
      .on_item_secondary_click((key, _event, cx) => {
        const item = byKey.get(key);
        if (item?.kind === "commit" && options.onContext) { cx.stop_propagation(); options.onContext(rows[item.rowIndex], cx); }
      }))
    .child(Scrollbar.vertical("source-control-graph-rows").viewport_from_layout().absolute().inset_0());
}
