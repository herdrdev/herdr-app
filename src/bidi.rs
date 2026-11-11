//! Right-to-left text in terminal rows, drawn in reading order.
//!
//! The grid stays as the program wrote it (logical order: what copy, search
//! and selection see); only where each cell is drawn changes. Each row is one
//! left-to-right paragraph for the Unicode Bidirectional Algorithm, and the
//! visual column of every cell follows from the resolved levels. This is the
//! "implicit" mode of the terminal-wg BiDi proposal, as iTerm2 and mlterm do
//! it. Rows without right-to-left characters are left alone, cheaply: a
//! class lookup per character and nothing else.

use crate::vt::Cell;
use unicode_bidi::{BidiClass, Level, ParagraphBidiInfo, bidi_class};

/// Where each cell of one row is drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowOrder {
    /// The visual column of each logical cell (a wide character's tail
    /// follows its head).
    pub visual: Vec<usize>,
    /// The logical cell at each visual column: the inverse of `visual`.
    pub logical: Vec<usize>,
    /// Whether each logical cell sits at a right-to-left level: its text
    /// runs right to left (so does "past the middle" of it for a click).
    pub rtl: Vec<bool>,
}

fn strong_rtl(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL | BidiClass::RLE | BidiClass::RLO | BidiClass::RLI)
}

/// Whether drawing `row` in logical order would show something backwards.
pub fn has_rtl(row: &[Cell]) -> bool {
    row.iter().any(|c| c.text.chars().any(strong_rtl))
}

/// The row's drawing order, or `None` when it reads left to right as it is.
pub fn order(row: &[Cell]) -> Option<RowOrder> {
    if !has_rtl(row) {
        return None;
    }
    // Each logical cell becomes one "character" of the paragraph (its whole
    // grapheme; a blank cell a space); a wide character's tail cell is part of
    // its head. `units`: (logical cell, byte offset in `text`, columns).
    let mut text = String::with_capacity(row.len());
    let mut units: Vec<(usize, usize, usize)> = Vec::with_capacity(row.len());
    let mut x = 0;
    while x < row.len() {
        let c = &row[x];
        let width = if c.wide { 2 } else { 1 };
        units.push((x, text.len(), width));
        if c.text.is_empty() {
            text.push(' ');
        } else {
            // A paragraph separator would split the row; a row is one line.
            text.extend(c.text.chars().map(|ch| if bidi_class(ch) == BidiClass::B { ' ' } else { ch }));
        }
        x += width;
    }
    let info = ParagraphBidiInfo::new(&text, Some(Level::ltr()));
    // Resolved levels per byte, with rule L1 applied (trailing whitespace at
    // the paragraph level: blank cells after the text stay on the right).
    let levels = info.reordered_levels(0..text.len());
    let unit_levels: Vec<Level> = units.iter().map(|&(_, at, _)| levels[at]).collect();
    // Visual position -> unit index (rule L2).
    let by_visual = ParagraphBidiInfo::reorder_visual(&unit_levels);

    let mut visual = vec![0; row.len()];
    let mut rtl = vec![false; row.len()];
    let mut col = 0;
    for &u in &by_visual {
        let (cell, _, width) = units[u];
        let is_rtl = unit_levels[u].is_rtl();
        // A wide character in the row's last column has no tail cell: it
        // takes the one column it has, so every visual column stays in the row.
        let take = width.min(row.len() - cell);
        for i in 0..take {
            visual[cell + i] = col + i;
            rtl[cell + i] = is_rtl;
        }
        col += take;
    }
    debug_assert_eq!(col, row.len());
    let mut logical = vec![0; row.len()];
    for (cell, &v) in visual.iter().enumerate() {
        logical[v] = cell;
    }
    Some(RowOrder { visual, logical, rtl })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row from text: one cell per character, `wide` cells marked with the
    /// character and an empty tail, trailing blank cells up to `cols`.
    fn row(text: &str, cols: usize) -> Vec<Cell> {
        let mut cells = Vec::new();
        for ch in text.chars() {
            let wide = matches!(ch, '\u{3000}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}');
            cells.push(Cell { text: ch.to_string(), wide, ..Cell::default() });
            if wide {
                cells.push(Cell::default());
            }
        }
        while cells.len() < cols {
            cells.push(Cell::default());
        }
        cells
    }

    /// What the row looks like on screen: the cells' text in visual order.
    fn shown(cells: &[Cell], order: &RowOrder) -> String {
        order.logical.iter().map(|&i| if cells[i].text.is_empty() { if i > 0 && cells[i - 1].wide { "" } else { " " } } else { cells[i].text.as_str() }).collect::<String>().trim_end().to_owned()
    }

    #[test]
    fn plain_rows_cost_only_a_scan() {
        assert!(order(&row("ls -la ~/projects 123", 30)).is_none());
        assert!(order(&row("", 10)).is_none());
        // Numbers and neutrals alone never turn a row around.
        assert!(order(&row("1 + (2 * 3) = 7", 20)).is_none());
    }

    #[test]
    fn pure_hebrew_reads_right_to_left_from_the_left_edge() {
        let cells = row("שלום עולם", 20);
        let o = order(&cells).unwrap();
        // The first letter typed is drawn rightmost of the text; blanks after
        // the text stay on the right (rule L1).
        assert_eq!(shown(&cells, &o), "םלוע םולש");
        assert_eq!(o.visual[0], 8);
        assert_eq!(o.visual[8], 0);
        assert_eq!(o.visual[9], 9, "the blank after the text keeps its column");
        assert!(o.rtl[..9].iter().all(|&r| r));
        assert!(!o.rtl[9]);
        for (cell, &v) in o.visual.iter().enumerate() {
            assert_eq!(o.logical[v], cell);
        }
    }

    #[test]
    fn mixed_hebrew_english_and_numbers_keep_each_run_in_reading_order() {
        let cells = row("Mixed: הקובץ auth.ts עבר", 30);
        let o = order(&cells).unwrap();
        // Each Hebrew word reads right to left in its place; the words keep
        // the left-to-right order of the paragraph (the LTR paragraph
        // direction of implicit mode: a Hebrew sentence with Latin words in
        // it reads as an English one with Hebrew words in it).
        assert_eq!(shown(&cells, &o), "Mixed: ץבוקה auth.ts רבע");
        let cells = row("שלום עולם, בדיקה 123", 30);
        let o = order(&cells).unwrap();
        // Numbers inside Hebrew text keep their digits in order (rule W2/L2).
        assert_eq!(shown(&cells, &o), "123 הקידב ,םלוע םולש");
    }

    #[test]
    fn claude_code_prompt_lines_keep_their_latin_frame() {
        let cells = row("> תקן את הבאג ב-auth.ts", 40);
        let o = order(&cells).unwrap();
        // The hyphen between Hebrew and Latin is neutral, so it goes with the
        // paragraph direction and the file name follows the Hebrew.
        assert_eq!(shown(&cells, &o), "> ב גאבה תא ןקת-auth.ts");
        assert_eq!(o.visual[0], 0, "the prompt marker stays first");
        let cells = row("│ שלום │", 20);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "│ םולש │");
    }

    #[test]
    fn brackets_and_punctuation_follow_their_run() {
        // Brackets inside Hebrew text take its direction (rule N0); the text
        // shaper mirrors them in the right-to-left run it shapes (rule L4),
        // so the glyphs read "(עולם)".
        let cells = row("שלום (עולם) טוב", 20);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "בוט )םלוע( םולש");
        assert!(o.rtl[5] && o.rtl[10]);
        // Brackets that open at the line's start follow the paragraph (N0
        // looks before the opening bracket and finds the left-to-right start).
        let cells = row("(שלום) עולם.", 20);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "(םולש) םלוע.");
        assert!(!o.rtl[0] && !o.rtl[5]);
        // A trailing period after Hebrew in an LTR paragraph goes to the
        // paragraph side (it is neutral between RTL text and the line end).
        let cells = row("שלום.", 10);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "םולש.");
    }

    #[test]
    fn wide_characters_keep_both_columns_together() {
        let cells = row("שלום 日本", 12);
        assert!(cells[5].wide && cells[6].text.is_empty());
        let o = order(&cells).unwrap();
        // The CJK word is LTR, so it stays to the right of the Hebrew.
        assert_eq!(o.visual[5], 5);
        assert_eq!(o.visual[6], 6, "a wide tail follows its head");
        assert_eq!(o.visual[7], 7);
        assert_eq!(o.visual[8], 8);
        assert_eq!(o.logical[5], 5);
        assert_eq!(o.logical[6], 6);
        let cells = row("日本 שלום", 12);
        let o = order(&cells).unwrap();
        assert_eq!((o.visual[0], o.visual[1], o.visual[2], o.visual[3]), (0, 1, 2, 3));
        assert_eq!(&o.visual[5..9], &[8, 7, 6, 5]);
        let cells = row("ש日", 3);
        let o = order(&cells).unwrap();
        // RTL letter, then a wide LTR glyph: the wide glyph's two columns stay adjacent.
        assert_eq!(o.visual, vec![0, 1, 2]);
    }

    #[test]
    fn a_wide_character_without_room_for_its_tail_stays_in_the_row() {
        let mut cells = row("ש", 2);
        cells[1] = Cell { text: "日".into(), wide: true, ..Cell::default() };
        let o = order(&cells).unwrap();
        assert!(o.visual.iter().all(|&v| v < 3));
        assert_eq!(o.logical.len(), 2);
    }

    #[test]
    fn the_cursor_follows_its_cell() {
        // Typing "שלום": the cursor is logically after the last letter, at
        // column 4, which is drawn to the left of the word.
        let cells = row("שלום", 10);
        let o = order(&cells).unwrap();
        assert_eq!(o.visual[3], 0, "the last letter is drawn leftmost");
        // The blank cell the cursor sits in keeps its column: the caret waits
        // at the right of the word, where the next letter will not go, but
        // where the program thinks it is.
        assert_eq!(o.visual[4], 4);
        // In mixed text the cursor on a Hebrew letter moves with that letter.
        let cells = row("ab שלום cd", 12);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "ab םולש cd");
        assert_eq!(o.visual[3], 6);
        assert_eq!(o.visual[6], 3);
        assert_eq!(o.visual[8], 8);
    }

    #[test]
    fn selections_map_back_through_logical() {
        let cells = row("ab שלום cd", 12);
        let o = order(&cells).unwrap();
        // A click at visual column 3 lands on the Hebrew word's last letter.
        assert_eq!(cells[o.logical[3]].text, "ם");
        assert_eq!(cells[o.logical[6]].text, "ש");
        assert_eq!(cells[o.logical[0]].text, "a");
    }
}
