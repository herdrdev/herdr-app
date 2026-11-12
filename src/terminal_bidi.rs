//! Display-only Unicode BiDi mapping. Terminal storage and input stay logical.

use crate::vt::Cell;
use unicode_bidi::{BidiInfo, Level};

#[derive(Clone, Debug, Default)]
pub(crate) struct RowMap {
    logical_to_visual: Vec<usize>,
    visual_to_logical: Vec<usize>,
    rtl: Vec<bool>,
    clusters: Vec<(usize, usize)>,
}

impl RowMap {
    pub(crate) fn new(row: &[Cell], enabled: bool) -> Self {
        let mut map = Self {
            logical_to_visual: (0..row.len()).collect(),
            visual_to_logical: (0..row.len()).collect(),
            rtl: vec![false; row.len()],
            clusters: (0..row.len()).map(|x| (x, 1)).collect(),
        };
        if !enabled || row.is_empty() { return map; }
        // A terminal cell is already a grapheme cluster. Never split marks or
        // reverse the tail half of a double-width cluster independently.
        let mut text = String::new();
        let mut clusters = Vec::new();
        let mut x = 0;
        while x < row.len() {
            let width = if row[x].wide && x + 1 < row.len() { 2 } else { 1 };
            clusters.push((x, width, text.len()));
            text.push_str(if row[x].text.is_empty() { " " } else { &row[x].text });
            x += width;
        }
        // Fixed LTR base preserves shell prompts, indentation and TUI layout.
        // Resolve the whole row before splitting it into font/style runs.
        let bidi = BidiInfo::new(&text, Some(Level::ltr()));
        let mut levels = bidi.levels.clone();
        for para in &bidi.paragraphs {
            let resolved = bidi.reordered_levels(para, para.range.clone());
            levels[para.range.clone()].copy_from_slice(&resolved[para.range.clone()]);
        }
        let cell_levels: Vec<_> = clusters.iter().map(|(_, _, byte)| levels[*byte]).collect();
        let order = BidiInfo::reorder_visual(&cell_levels);
        let mut visual = 0;
        for index in order {
            let (logical, width, _) = clusters[index];
            for offset in 0..width {
                map.logical_to_visual[logical + offset] = visual + offset;
                map.visual_to_logical[visual + offset] = logical + offset;
                map.rtl[logical + offset] = cell_levels[index].is_rtl();
                map.clusters[logical + offset] = (logical, width);
            }
            visual += width;
        }
        map
    }

    pub(crate) fn visual(&self, logical: usize) -> usize {
        self.logical_to_visual.get(logical).copied().unwrap_or(logical)
    }

    pub(crate) fn logical(&self, visual: usize) -> usize {
        self.visual_to_logical.get(visual).copied().unwrap_or(visual)
    }

    pub(crate) fn is_rtl(&self, logical: usize) -> bool {
        self.rtl.get(logical).copied().unwrap_or(false)
    }

    pub(crate) fn logical_x(&self, visual: f32) -> f32 {
        let column = visual.max(0.0).floor() as usize;
        let logical = self.logical(column);
        if self.is_rtl(logical) {
            let (start, width) = self.clusters[logical];
            let within = visual - self.visual(start) as f32;
            return start as f32 + (width as f32 - within).clamp(0.0, width as f32 - 0.000_001);
        }
        let fraction = visual - column as f32;
        // Keep the inverse-mapped point inside its cell at exact boundaries.
        logical as f32 + fraction.clamp(0.0, 0.999_999)
    }

    /// Insertion edge before a logical cell, distinct from its painted left edge.
    pub(crate) fn leading_edge(&self, logical: usize) -> usize {
        if self.is_rtl(logical) {
            let (start, width) = self.clusters[logical];
            self.visual(start) + width
        } else {
            self.visual(logical)
        }
    }

    /// A logical interval may occupy several disjoint visual intervals.
    pub(crate) fn segments(&self, from: usize, to: usize) -> Vec<(usize, usize)> {
        let mut cells: Vec<_> = (from..to).map(|x| self.visual(x)).collect();
        cells.sort_unstable();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for x in cells {
            if let Some(last) = out.last_mut().filter(|last| last.1 == x) { last.1 += 1; }
            else { out.push((x, x + 1)); }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> Vec<Cell> {
        text.chars().map(|c| Cell { text: c.to_string(), ..Cell::default() }).collect()
    }

    fn visual_text(row: &[Cell], map: &RowMap) -> String {
        (0..row.len()).map(|x| row[map.logical(x)].text.as_str()).collect()
    }

    #[test]
    fn mixed_hebrew_numbers_and_paths_keep_logical_data() {
        let cells = row("abc \u{5e9}\u{5dc}\u{5d5}\u{5dd} 123 /tmp/a.txt");
        let map = RowMap::new(&cells, true);
        assert_eq!(visual_text(&cells, &map), "abc 123 \u{5dd}\u{5d5}\u{5dc}\u{5e9} /tmp/a.txt");
        for x in 0..cells.len() { assert_eq!(map.logical(map.visual(x)), x); }
        assert_eq!(cells[4].text, "\u{5e9}");
        assert_eq!(visual_text(&cells, &RowMap::new(&cells, false)), "abc \u{5e9}\u{5dc}\u{5d5}\u{5dd} 123 /tmp/a.txt");
    }

    #[test]
    fn wide_cells_and_combining_marks_stay_atomic() {
        let cells = vec![Cell { text: "\u{5d0}\u{5b7}".into(), ..Cell::default() }, Cell { text: "\u{1f600}".into(), wide: true, ..Cell::default() }, Cell::default(), Cell { text: "\u{5d1}".into(), ..Cell::default() }];
        let map = RowMap::new(&cells, true);
        assert_eq!(map.visual(2), map.visual(1) + 1);
        assert_eq!(visual_text(&cells, &map), "\u{5d1}\u{1f600}\u{5d0}\u{5b7}");
        assert_eq!(map.logical_x(map.visual(0) as f32 + 0.25), 0.75);
    }

    #[test]
    fn rtl_wide_cluster_hit_testing_reflects_the_whole_character() {
        let cells = vec![Cell { text: "\u{5d0}".into(), ..Cell::default() }, Cell { text: "\u{1f600}".into(), wide: true, ..Cell::default() }, Cell::default(), Cell { text: "\u{5d1}".into(), ..Cell::default() }];
        let map = RowMap::new(&cells, true);
        let left = map.visual(1) as f32;
        assert_eq!(map.logical_x(left + 0.25), 2.75);
        assert_eq!(map.logical_x(left + 1.75), 1.25);
        assert_eq!(map.leading_edge(1), map.visual(1) + 2);
        assert_eq!(map.leading_edge(0), map.visual(0) + 1);
    }

    #[test]
    fn selection_segments_and_inverse_points_match_display() {
        let cells = row("a \u{5d0}\u{5d1}\u{5d2} 12 z");
        let map = RowMap::new(&cells, true);
        for x in 0..cells.len() {
            let visual = map.visual(x);
            assert_eq!(map.logical_x(visual as f32 + 0.5).floor() as usize, x);
        }
        let segments = map.segments(1, 4);
        let count: usize = segments.iter().map(|(a, b)| b - a).sum();
        assert_eq!(count, 3);
        assert!(segments.len() > 1);
    }
}
