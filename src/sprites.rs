//! Box drawing and block characters drawn as shapes that fill their cell
//! exactly, as Ghostty does. Font glyphs for these rarely span the whole
//! cell, so tables get gaps between rows and block art gets seams.

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Window, fill, point, px, size};

/// Per direction (up, right, down, left): 0 none, 1 light, 2 heavy, 3 double.
fn lines(c: char) -> Option<[u8; 4]> {
    const LIGHT_HEAVY: &str = "\
        0101 0202 1010 2020 0101 0202 1010 2020 0101 0202 1010 2020 \
        0110 0210 0120 0220 0011 0012 0021 0022 1100 1200 2100 2200 \
        1001 1002 2001 2002 1110 1210 2110 1120 2120 2210 1220 2220 \
        1011 1012 2011 1021 2021 2012 1022 2022 0111 0112 0211 0212 \
        0121 0122 0221 0222 1101 1102 1201 1202 2101 2102 2201 2202 \
        1111 1112 1211 1212 2111 1121 2121 2112 2211 1122 1221 2212 \
        1222 2122 2221 2222 0101 0202 1010 2020";
    const DOUBLE: &str = "\
        0303 3030 0310 0130 0330 0013 0031 0033 1300 3100 3300 1003 \
        3001 3003 1310 3130 3330 1013 3031 3033 0313 0131 0333 1303 \
        3101 3303 1313 3131 3333 0110 0011 1001 1100";
    const SINGLES: &str = "0001 1000 0100 0010 0002 2000 0200 0020 0201 1020 0102 2010";
    let code = c as u32;
    let (table, base) = match code {
        0x2500..=0x254F => (LIGHT_HEAVY, 0x2500),
        0x2550..=0x2570 => (DOUBLE, 0x2550),
        0x2574..=0x257F => (SINGLES, 0x2574),
        _ => return None,
    };
    let word = table.split_whitespace().nth((code - base) as usize)?;
    let mut out = [0u8; 4];
    for (o, b) in out.iter_mut().zip(word.bytes()) {
        *o = b - b'0';
    }
    Some(out)
}

/// Rounded corners (╭╮╯╰): which way their two arms leave the center,
/// (right or left, down or up). Drawn as arcs, as Ghostty does.
fn arc(c: char) -> Option<(f32, f32)> {
    match c {
        '╭' => Some((1.0, 1.0)),
        '╮' => Some((-1.0, 1.0)),
        '╯' => Some((-1.0, -1.0)),
        '╰' => Some((1.0, -1.0)),
        _ => None,
    }
}

/// Dashed lines: (dashes per cell, heavy, horizontal), as Ghostty draws them.
fn dashes(c: char) -> Option<(u8, bool, bool)> {
    Some(match c {
        '┄' => (3, false, true),
        '┅' => (3, true, true),
        '┆' => (3, false, false),
        '┇' => (3, true, false),
        '┈' => (4, false, true),
        '┉' => (4, true, true),
        '┊' => (4, false, false),
        '┋' => (4, true, false),
        '╌' => (2, false, true),
        '╍' => (2, true, true),
        '╎' => (2, false, false),
        '╏' => (2, true, false),
        _ => return None,
    })
}

/// Rectangles in cell fractions (x0, y0, x1, y1) and an alpha, for block elements.
fn blocks(c: char) -> Option<(Vec<[f32; 4]>, f32)> {
    let e = 1.0 / 8.0;
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| [x0, y0, x1, y1];
    let quads = |ul: bool, ur: bool, ll: bool, lr: bool| {
        let mut v = Vec::new();
        if ul { v.push(r(0.0, 0.0, 0.5, 0.5)) }
        if ur { v.push(r(0.5, 0.0, 1.0, 0.5)) }
        if ll { v.push(r(0.0, 0.5, 0.5, 1.0)) }
        if lr { v.push(r(0.5, 0.5, 1.0, 1.0)) }
        v
    };
    Some(match c {
        '▀' => (vec![r(0.0, 0.0, 1.0, 0.5)], 1.0),
        '▁'..='█' => {
            let n = (c as u32 - '▁' as u32 + 1) as f32;
            (vec![r(0.0, 1.0 - n * e, 1.0, 1.0)], 1.0)
        }
        '▉'..='▏' => {
            let n = (8 - (c as u32 - '▉' as u32 + 1)) as f32;
            (vec![r(0.0, 0.0, n * e, 1.0)], 1.0)
        }
        '▐' => (vec![r(0.5, 0.0, 1.0, 1.0)], 1.0),
        '░' => (vec![r(0.0, 0.0, 1.0, 1.0)], 0.25),
        '▒' => (vec![r(0.0, 0.0, 1.0, 1.0)], 0.5),
        '▓' => (vec![r(0.0, 0.0, 1.0, 1.0)], 0.75),
        '▔' => (vec![r(0.0, 0.0, 1.0, e)], 1.0),
        '▕' => (vec![r(1.0 - e, 0.0, 1.0, 1.0)], 1.0),
        '▖' => (quads(false, false, true, false), 1.0),
        '▗' => (quads(false, false, false, true), 1.0),
        '▘' => (quads(true, false, false, false), 1.0),
        '▙' => (quads(true, false, true, true), 1.0),
        '▚' => (quads(true, false, false, true), 1.0),
        '▛' => (quads(true, true, true, false), 1.0),
        '▜' => (quads(true, true, false, true), 1.0),
        '▝' => (quads(false, true, false, false), 1.0),
        '▞' => (quads(false, true, true, false), 1.0),
        '▟' => (quads(false, true, true, true), 1.0),
        _ => return None,
    })
}

/// ⏺ (Claude Code marks every step with it): few terminal fonts have it, and
/// the fallback may draw nothing.
const RECORD: char = '\u{23FA}';

pub fn is_sprite(text: &str) -> bool {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => lines(c).is_some() || blocks(c).is_some() || ('\u{E0B0}'..='\u{E0BF}').contains(&c) || c == RECORD,
        _ => false,
    }
}

/// Paints `text` (one sprite character) into `cell`, snapped to device pixels.
pub fn paint(text: &str, cell: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    let Some(c) = text.chars().next() else { return };
    let scale = window.scale_factor();
    let snap = |v: f32| (v * scale).round() / scale;
    let (x, y, w, h) = (f32::from(cell.origin.x), f32::from(cell.origin.y), f32::from(cell.size.width), f32::from(cell.size.height));
    let mut rect = |x0: f32, y0: f32, x1: f32, y1: f32, color: Hsla| {
        let (x0, y0, x1, y1) = (snap(x0), snap(y0), snap(x1), snap(y1));
        if x1 > x0 && y1 > y0 {
            window.paint_quad(fill(Bounds::new(point(px(x0), px(y0)), size(px(x1 - x0), px(y1 - y0))), color));
        }
    };

    if c == RECORD {
        let d = snap((w * 0.62).min(h * 0.5));
        let (x0, y0) = (snap(x + (w - d) / 2.0), snap(y + (h - d) / 2.0));
        window.paint_quad(fill(Bounds::new(point(px(x0), px(y0)), size(px(d), px(d))), color).corner_radii(px(d / 2.0)));
        return;
    }
    if ('\u{E0B0}'..='\u{E0BF}').contains(&c) {
        let line = (w / 8.0).round().max(1.0 / scale);
        powerline(c, (x, y, w, h), line, color, window);
        return;
    }
    if let Some((rects, alpha)) = blocks(c) {
        let mut color = color;
        color.a *= alpha;
        for [x0, y0, x1, y1] in rects {
            rect(x + x0 * w, y + y0 * h, x + x1 * w, y + y1 * h, color);
        }
        return;
    }
    let light = (w / 8.0).round().max(1.0 / scale);
    let (cx, cy) = (snap(x + w / 2.0), snap(y + h / 2.0));
    if let Some((sx, sy)) = arc(c) {
        // Like Ghostty, center the stroke between its pixel-aligned edges.
        // The solid arms snap both edges, not the path center.
        let (x0, x1) = (snap(cx - light / 2.0), snap(cx + light / 2.0));
        let (y0, y1) = (snap(cy - light / 2.0), snap(cy + light / 2.0));
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let ex = snap(if sx > 0.0 { x + w } else { x });
        let ey = snap(if sy > 0.0 { y + h } else { y });
        // Keep the turn inside the snapped cell, without doubling back.
        let (r, k) = ((ex - cx).abs().min((ey - cy).abs()), 0.25);
        let mut path = PathBuilder::stroke(px(x1 - x0));
        path.move_to(point(px(cx), px(ey)));
        path.line_to(point(px(cx), px(cy + sy * r)));
        path.cubic_bezier_to(point(px(cx + sx * r), px(cy)), point(px(cx), px(cy + sy * k * r)), point(px(cx + sx * k * r), px(cy)));
        path.line_to(point(px(ex), px(cy)));
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
        return;
    }
    if let Some((n, heavy, horizontal)) = dashes(c) {
        // Gaps are at most half the cell. A horizontal line keeps half a gap
        // at each end and a vertical one its gap below, so neighbors tile evenly.
        let t = if heavy { light * 2.0 } else { light };
        let (len, from) = if horizontal { (w, x) } else { (h, y) };
        let gap = (len / (2.0 * n as f32)).min(light.max(4.0 / scale));
        let dash = (len - gap * n as f32) / n as f32;
        let mut at = from + if horizontal { gap / 2.0 } else { 0.0 };
        for _ in 0..n {
            if horizontal {
                rect(at, cy - t / 2.0, at + dash, cy + t / 2.0, color);
            } else {
                rect(cx - t / 2.0, at, cx + t / 2.0, at + dash, color);
            }
            at += dash + gap;
        }
        return;
    }
    let Some(dirs) = lines(c) else { return };
    let thick = |k: u8| if k == 2 { light * 2.0 } else { light };
    // Where a line along an arm starts, measured from the center toward the
    // arm: `before` and `after` are the crossing arms on this line's side and
    // the other side. Negative reaches past the center so joins close.
    let cover = |k: u8| if k == 3 { light * 1.5 } else { thick(k) / 2.0 };
    let start = |before: u8, after: u8, double: bool| -> f32 {
        match (before, after) {
            // A double line stops at a double crossing on its side (inner corner).
            (3, _) if double => light / 2.0,
            (0, 0) => 0.0,
            (0, k) | (k, _) => -cover(k),
        }
    };
    // Arms: (direction, sign along the axis, horizontal?).
    for (i, sign, horizontal) in [(0usize, -1.0f32, false), (1, 1.0, true), (2, 1.0, false), (3, -1.0, true)] {
        let k = dirs[i];
        if k == 0 {
            continue;
        }
        // The crossing arms on each side of this arm (up/down for a horizontal arm).
        let (side_a, side_b) = if horizontal { (dirs[0], dirs[2]) } else { (dirs[3], dirs[1]) };
        let lines: Vec<(f32, f32, u8, u8)> = if k == 3 {
            vec![(-light, light, side_a, side_b), (light, light, side_b, side_a)]
        } else {
            let (a, b) = if side_a >= side_b { (side_a, side_b) } else { (side_b, side_a) };
            vec![(0.0, thick(k), a, b)]
        };
        for (off, t, before, after) in lines {
            let near = start(before, after, k == 3);
            let (c, edge) = if horizontal { (cx, if sign > 0.0 { x + w } else { x }) } else { (cy, if sign > 0.0 { y + h } else { y }) };
            let from = c + sign * near;
            let (a0, a1) = if sign > 0.0 { (from, edge) } else { (edge, from) };
            if horizontal {
                rect(a0, cy + off - t / 2.0, a1, cy + off + t / 2.0, color);
            } else {
                rect(cx + off - t / 2.0, a0, cx + off + t / 2.0, a1, color);
            }
        }
    }
}

/// Nerd Font powerline separators (U+E0B0..E0BF): arrows, half circles and
/// slants, drawn to fill the cell exactly as Ghostty does. Fonts that have
/// them rarely match the cell, and fonts that don't fall back to whatever
/// font has those codes (on Linux, sometimes digits).
fn powerline(c: char, (x, y, w, h): (f32, f32, f32, f32), line: f32, color: Hsla, window: &mut Window) {
    let solid = (c as u32).is_multiple_of(2);
    // A solid shape reaches a device pixel past its cell's sides, so its flat
    // edge meets the neighbor's color without an anti-aliased seam.
    let ext = if solid { 1.0 / window.scale_factor() } else { 0.0 };
    let p = |px_: f32, py_: f32| point(px(x + px_ * w + (px_ - 0.5) * 2.0 * ext), px(y + py_ * h));
    let mut path = if solid { PathBuilder::fill() } else { PathBuilder::stroke(px(line)) };
    // Half an ellipse filling the cell: four quarter arcs as cubics.
    const K: f32 = 0.552_284_8;
    match c {
        '\u{E0B0}' | '\u{E0B1}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 0.5), p(0.0, 1.0)], solid),
        '\u{E0B2}' | '\u{E0B3}' => path.add_polygon(&[p(1.0, 0.0), p(0.0, 0.5), p(1.0, 1.0)], solid),
        '\u{E0B4}' | '\u{E0B5}' => {
            path.move_to(p(0.0, 0.0));
            path.cubic_bezier_to(p(1.0, 0.5), p(K, 0.0), p(1.0, 0.5 - K / 2.0));
            path.cubic_bezier_to(p(0.0, 1.0), p(1.0, 0.5 + K / 2.0), p(K, 1.0));
            if solid {
                path.close();
            }
        }
        '\u{E0B6}' | '\u{E0B7}' => {
            path.move_to(p(1.0, 0.0));
            path.cubic_bezier_to(p(0.0, 0.5), p(1.0 - K, 0.0), p(0.0, 0.5 - K / 2.0));
            path.cubic_bezier_to(p(1.0, 1.0), p(0.0, 0.5 + K / 2.0), p(1.0 - K, 1.0));
            if solid {
                path.close();
            }
        }
        '\u{E0B8}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)], true),
        '\u{E0BA}' => path.add_polygon(&[p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)], true),
        '\u{E0BC}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0)], true),
        '\u{E0BE}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0)], true),
        '\u{E0B9}' | '\u{E0BF}' => path.add_polygon(&[p(0.0, 0.0), p(1.0, 1.0)], false),
        _ => path.add_polygon(&[p(0.0, 1.0), p(1.0, 0.0)], false), // E0BB, E0BD
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_cover_their_ranges() {
        assert_eq!(lines('─'), Some([0, 1, 0, 1]));
        assert_eq!(lines('┼'), Some([1, 1, 1, 1]));
        assert_eq!(lines('╋'), Some([2, 2, 2, 2]));
        assert_eq!(lines('┌'), Some([0, 1, 1, 0]));
        assert_eq!(lines('┘'), Some([1, 0, 0, 1]));
        assert_eq!(lines('╏'), Some([2, 0, 2, 0]));
        assert_eq!(lines('╔'), Some([0, 3, 3, 0]));
        assert_eq!(lines('╬'), Some([3, 3, 3, 3]));
        assert_eq!(lines('╭'), Some([0, 1, 1, 0]));
        assert_eq!(lines('╰'), Some([1, 1, 0, 0]));
        assert_eq!(lines('╱'), None);
        assert_eq!(lines('╿'), Some([2, 0, 1, 0]));
        for c in ('\u{2500}'..='\u{2570}').chain('\u{2574}'..='\u{257F}') {
            assert!(lines(c).is_some(), "{c} has no entry");
        }
        for c in '\u{2580}'..='\u{259F}' {
            assert!(blocks(c).is_some(), "{c} has no entry");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rendered_rounded_corners_join_straight_lines() {
        use gpui::{prelude::*, *};

        struct BoxScene { w: f32, h: f32 }
        impl Render for BoxScene {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let (w, h) = (self.w, self.h);
                div().size_full().bg(rgb(0)).child(canvas(|_, _, _| {}, move |_, _, window, _| {
                    for (row, text) in ["╭─╮", "│ │", "╰─╯"].iter().enumerate() {
                        for (col, c) in text.chars().enumerate() {
                            paint(&c.to_string(), Bounds::new(
                                point(px(7.3 + col as f32 * w), px(5.7 + row as f32 * h)),
                                size(px(w), px(h)),
                            ), rgb(0xffffff).into(), window);
                        }
                    }
                }).size_full())
            }
        }

        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(()),
            gpui_platform::current_headless_renderer,
        );
        // The headless platform renders at 2x. Include half-sized cells to
        // exercise the one-device-pixel strokes used at 14 and 18.2px.
        for (w, h) in [(4.2, 11.), (5.46, 14.3), (8.4, 22.), (10.92, 28.6), (16., 32.)] {
            let handle = cx.open_window(size(px(80.), px(120.)), |_, cx| cx.new(|_| BoxScene { w, h })).unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                let scale = window.scale_factor();
                let _ = window.draw(cx);
                let image = window.render_to_image().unwrap();
                let snap = |v: f32| (v * scale).round() as u32;
                // Compare each arc endpoint with the neighboring solid arm,
                // including the background on either side of the stroke.
                for row in [0, 2] {
                    let cy = snap(5.7 + row as f32 * h + h / 2.);
                    for boundary in [1, 2] {
                        let edge = snap(7.3 + boundary as f32 * w);
                        for y in cy - 3..=cy + 3 {
                            let a = image.get_pixel(edge - 1, y).0[0];
                            let b = image.get_pixel(edge, y).0[0];
                            assert!(a.abs_diff(b) <= 24, "horizontal join: {w}x{h} scale {scale}, row {row}, edge {boundary}, y {y}: {a} != {b}");
                        }
                    }
                }
                for col in [0, 2] {
                    let cx = snap(7.3 + col as f32 * w + w / 2.);
                    for boundary in [1, 2] {
                        let edge = snap(5.7 + boundary as f32 * h);
                        for x in cx - 3..=cx + 3 {
                            let a = image.get_pixel(x, edge - 1).0[0];
                            let b = image.get_pixel(x, edge).0[0];
                            assert!(a.abs_diff(b) <= 24, "vertical join: {w}x{h} scale {scale}, col {col}, edge {boundary}, x {x}: {a} != {b}");
                        }
                    }
                }
            }).unwrap();
        }
    }

    #[test]
    fn rounded_corners_and_dashes_have_their_own_shapes() {
        for c in ['╭', '╮', '╯', '╰'] {
            assert!(arc(c).is_some() && is_sprite(&c.to_string()), "{c}");
        }
        for c in ['┄', '┅', '┆', '┇', '┈', '┉', '┊', '┋', '╌', '╍', '╎', '╏'] {
            assert!(dashes(c).is_some() && is_sprite(&c.to_string()), "{c}");
        }
        assert_eq!(arc('┌'), None);
        assert_eq!(dashes('─'), None);
    }
}
