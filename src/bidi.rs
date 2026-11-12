//! Right-to-left text in terminal rows (Persian, Arabic, Hebrew).
//!
//! Programs write text in logical order and the grid keeps it that way, so
//! copying, search and the program's own cursor stay logical. Drawing runs the
//! Unicode Bidirectional Algorithm (UAX #9) on each row with a left-to-right
//! base, as WezTerm (`AutoLeftToRight`) and herdr's TUI do: left-to-right text
//! never moves, and each right-to-left run is mirrored inside the cells it
//! already takes. Box-drawing and block cells (frames, tables) and wide
//! characters end a run's context, so a table's columns stay apart.

use std::ops::Range;

use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};

use crate::vt::Cell;

/// A row's drawing order. Rows without right-to-left text have none.
#[derive(Clone, Debug, PartialEq)]
pub struct Order {
    /// The logical column drawn at each column.
    pub visual: Vec<u16>,
    /// Where each logical column is drawn (the inverse of `visual`).
    pub drawn: Vec<u16>,
    /// Each column's embedding level: odd is right to left.
    pub levels: Vec<u8>,
}

impl Order {
    /// Where logical column `x` is drawn.
    pub fn drawn_at(&self, x: usize) -> usize {
        self.drawn.get(x).map_or(x, |v| *v as usize)
    }

    /// The logical column drawn at column `v`.
    pub fn logical_at(&self, v: usize) -> usize {
        self.visual.get(v).map_or(v, |x| *x as usize)
    }

    /// Columns `from..to` (logical) as the column ranges they are drawn in, left to right.
    pub fn drawn_spans(&self, from: usize, to: usize) -> Vec<Range<usize>> {
        let mut cols: Vec<usize> = (from..to).map(|x| self.drawn_at(x)).collect();
        cols.sort_unstable();
        let mut spans: Vec<Range<usize>> = Vec::new();
        for v in cols {
            match spans.last_mut() {
                Some(span) if span.end == v => span.end += 1,
                _ => spans.push(v..v + 1),
            }
        }
        spans
    }
}

fn starts_rtl(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI)
}

/// Whether a cell's letter belongs to a right-to-left script: such neighbors
/// at one level form a word that is shaped and placed as a whole.
pub fn is_rtl_letter(cell: &Cell) -> bool {
    cell.text.chars().next().is_some_and(|c| matches!(bidi_class(c), BidiClass::R | BidiClass::AL))
}

/// Cells that end a run's context and never move: frames, blocks and wide characters.
fn is_boundary(row: &[Cell], x: usize) -> bool {
    row[x].wide || (x > 0 && row[x - 1].wide) || crate::sprites::is_sprite(&row[x].text)
}

/// The drawing order of a row, if it has right-to-left text.
pub fn order(row: &[Cell]) -> Option<Order> {
    if !row.iter().any(|c| !c.text.is_ascii() && c.text.chars().any(starts_rtl)) {
        return None;
    }
    let mut levels = vec![0u8; row.len()];
    let mut start = 0;
    while start < row.len() {
        let mut end = start;
        while end < row.len() && !is_boundary(row, end) {
            end += 1;
        }
        resolve(&row[start..end], &mut levels[start..end]);
        start = end + 1;
    }
    if levels.iter().all(|l| *l == 0) {
        return None;
    }
    let visual = reorder(&levels);
    let mut drawn = vec![0u16; visual.len()];
    for (v, x) in visual.iter().enumerate() {
        drawn[*x as usize] = v as u16;
    }
    Some(Order { visual, drawn, levels })
}

/// UAX #9 levels for one stretch of cells, left-to-right paragraph, one line.
fn resolve(cells: &[Cell], out: &mut [u8]) {
    if !cells.iter().any(|c| c.text.chars().any(starts_rtl)) {
        return;
    }
    let mut text = String::new();
    let mut starts = Vec::with_capacity(cells.len());
    for c in cells {
        starts.push(text.len());
        // Blank cells read as spaces; a lone line break would end the paragraph.
        text.push_str(if c.text.is_empty() || c.text.contains(['\n', '\r', '\u{2029}', '\u{85}']) { " " } else { &c.text });
    }
    let info = BidiInfo::new(&text, Some(Level::ltr()));
    let Some(para) = info.paragraphs.first() else { return };
    let levels = info.reordered_levels(para, 0..text.len());
    for (x, start) in starts.iter().enumerate() {
        out[x] = levels[*start].number();
    }
}

/// UAX #9 rule L2: from the highest level down to the lowest odd one, reverse
/// every run at that level or above. Returns the logical column at each place.
fn reorder(levels: &[u8]) -> Vec<u16> {
    let mut visual: Vec<u16> = (0..levels.len() as u16).collect();
    let Some(highest) = levels.iter().copied().max() else { return visual };
    let Some(lowest_odd) = levels.iter().copied().filter(|l| l % 2 == 1).min().or((highest > 0).then_some(1)) else {
        return visual;
    };
    for level in (lowest_odd..=highest).rev() {
        let mut i = 0;
        while i < visual.len() {
            if levels[visual[i] as usize] < level {
                i += 1;
                continue;
            }
            let start = i;
            while i < visual.len() && levels[visual[i] as usize] >= level {
                i += 1;
            }
            visual[start..i].reverse();
        }
    }
    visual
}

/// Pieces of a moved run that are placed on their own, as logical ranges: a
/// right-to-left word (letters side by side at one level, shaped joined) or
/// any other single cell (a space, a digit, a mark of punctuation).
pub fn pieces(row: &[Cell], order: &Order, run: Range<usize>) -> Vec<Range<usize>> {
    let mut pieces = Vec::new();
    let mut x = run.start;
    while x < run.end {
        let start = x;
        x += 1;
        if is_rtl_letter(&row[start]) {
            while x < run.end && is_rtl_letter(&row[x]) && order.levels[x] == order.levels[start] {
                x += 1;
            }
        }
        pieces.push(start..x);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> Vec<Cell> {
        text.chars().map(|c| Cell { text: if c == ' ' { String::new() } else { c.to_string() }, ..Default::default() }).collect()
    }

    fn drawn(text: &str) -> String {
        let cells = row(text);
        match order(&cells) {
            None => text.to_owned(),
            Some(o) => o.visual.iter().map(|x| text.chars().nth(*x as usize).unwrap()).collect(),
        }
    }

    #[test]
    fn rows_without_right_to_left_text_keep_their_order() {
        assert!(order(&row("plain ascii, café, 日本")).is_none());
        // Arabic-Indic digits alone don't reorder a left-to-right row.
        assert!(order(&row("v ۱۲۳ ok")).is_none());
    }

    #[test]
    fn persian_words_read_right_to_left_and_ltr_text_stays() {
        assert_eq!(drawn("Is نوع ورزش required?"), "Is شزرو عون required?");
        assert_eq!(drawn("سلام دنیا TESTING 123"), "ایند مالس TESTING 123");
    }

    #[test]
    fn numbers_and_latin_inside_persian_keep_left_to_right() {
        // The number inside the Persian run still reads 42.
        assert_eq!(drawn("تعداد 42 مورد"), "دروم 42 دادعت");
        // With a left-to-right base, Latin splits the line into two Persian runs.
        assert_eq!(drawn("فایل main.rs را باز کن"), "لیاف main.rs نک زاب ار");
    }

    #[test]
    fn frames_and_wide_cells_end_a_runs_context() {
        // Table cells stay in their own columns.
        assert_eq!(drawn("│ سلام │ دنیا │"), "│ مالس │ ایند │");
        let mut cells = row("ab سل");
        cells.insert(2, Cell { text: "日".into(), wide: true, ..Default::default() });
        cells.insert(3, Cell::default());
        let o = order(&cells).unwrap();
        assert_eq!(&o.visual[..4], &[0, 1, 2, 3]);
    }

    #[test]
    fn drawn_and_logical_columns_round_trip() {
        let cells = row("ab سلام cd");
        let o = order(&cells).unwrap();
        for x in 0..cells.len() {
            assert_eq!(o.logical_at(o.drawn_at(x)), x);
        }
        assert_eq!(o.levels, [0, 0, 0, 1, 1, 1, 1, 0, 0, 0]);
        assert_eq!(o.drawn_spans(3, 5), vec![5..7]);
    }

    #[test]
    fn words_are_pieces_and_spaces_stand_alone() {
        let cells = row("ab نوع ورزش cd");
        let o = order(&cells).unwrap();
        assert_eq!(o.levels.iter().filter(|l| **l > 0).count(), 8);
        assert_eq!(pieces(&cells, &o, 3..11), vec![3..6, 6..7, 7..11]);
    }
}
