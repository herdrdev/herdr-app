import { div, PathBuilder } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";

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

function graphCell(rows, index, width, t) {
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
      cell.child(div().absolute().left(x - 3.5).top(middle - 3.5).size(7).rounded(4).bg(color));
      if (row.refs.includes("HEAD")) cell.child(div().absolute().left(x - 5.5).top(middle - 5.5).size(11).rounded(6).border_1().border_color(color));
    }
  });
  return cell;
}

export function renderGraph(rows, t, options = {}) {
  const width = Math.max(20, ...rows.map((row) => row.graph.trimEnd().length * 7 + 9));
  return v_flex().id("source-control-graph-list").flex_1().min_h(0).min_w(0).overflow_y_scrollbar()
    .children(rows.map((row, index) => {
      const line = h_flex().id(`graph-row-${index}`).w_full().h(row.height).flex_none().min_w(0).px(6).gap(4).items_center()
        .child(graphCell(rows, index, width, t));
      if (!row.hash) return line;
      const refs = row.refs.split(", ").filter(Boolean), current = refs.find((ref) => ref.startsWith("HEAD -> "));
      const badge = current ? current.slice(8) : refs[0];
      const selected = (options.selected?.hash ?? options.selected) === row.hash;
      return line.when(selected, (e) => e.bg(t.selected)).hover((s) => s.bg(selected ? t.selected : t.hover))
        .when(typeof options.onSelect === "function", (e) => e.cursor_pointer().on_click((_event, cx) => { cx.stop_propagation(); options.onSelect(row, cx); }))
        .when(typeof options.onContext === "function", (e) => e.on_mouse_down("right", (_event, cx) => { cx.stop_propagation(); options.onContext(row, cx); }))
        .tooltip(`${row.subject}\n${row.hash} · ${row.author} · ${row.date}${row.refs ? `\n${row.refs}` : ""}`)
        .child(div().flex_1().min_w(0).truncate().text_size(12).text_color(t.foreground).child(row.subject))
        .when(Boolean(badge), (e) => e.child(div().max_w(90).flex_none().truncate().px(5).py(1).rounded(4)
          .bg(current ? t.selected : t.raised).text_color(current ? t.ansi4 : t.ansi5).text_size(10).child(badge)));
    }));
}
