use gpui::{FontId, GlyphId, ShapedGlyph, ShapedLine, ShapedRun, point, px};
use std::sync::Arc;

#[test]
fn niqqud_before_base_in_glyph_order_keeps_its_shaped_offset() {
    let glyph = |index, x| ShapedGlyph {
        id: GlyphId(1), position: point(px(x), px(0.)), index, is_emoji: false,
    };
    let mut line = ShapedLine::default();
    // CoreText returns qamats and shin-dot before shin. The base is the
    // earliest text index, not the first painted glyph. A fallback font may
    // also put a mark in a separate run.
    Arc::make_mut(&mut *line).runs = vec![
        ShapedRun { font_id: FontId(0), glyphs: vec![glyph(2, 28.962), glyph(4, 35.190)] },
        ShapedRun { font_id: FontId(1), glyphs: vec![glyph(0, 25.398)] },
    ];
    let line = line.with_cell_positions(&[(0, px(10.))], px(20.));
    let mark = line.runs[0].glyphs[0].position.x;
    let dot = line.runs[0].glyphs[1].position.x;
    let base = line.runs[1].glyphs[0].position.x;
    assert!((f32::from(base) - 10.).abs() < 0.001);
    assert!((f32::from(mark - base) - 3.564).abs() < 0.001);
    assert!((f32::from(dot - base) - 9.792).abs() < 0.001);
}
