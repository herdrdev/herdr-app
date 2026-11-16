//! Right-to-left text in terminal rows, drawn in reading order.
//!
//! The grid stays as the program wrote it (logical order: what copy, search
//! and selection see); only where each cell is drawn changes. Each row is one
//! left-to-right paragraph for the Unicode Bidirectional Algorithm, and the
//! visual column of every cell follows from the resolved levels. This is the
//! "implicit" mode of the terminal-wg BiDi proposal, as iTerm2 and mlterm do
//! it. Rows without right-to-left characters are left alone, cheaply: a
//! class lookup per character and nothing else.
//!
//! Terminal rows are laid out in columns, so a row is cut into segments that
//! are reordered on their own and never trade places: table and frame
//! borders (box-drawing and block characters) and runs of two or more blank
//! cells (aligned columns) part them and keep their columns. In file paths
//! and URLs (Ghostty's link pattern, links.rs) `/` and `\` count as left to
//! right, so a path's folders stay in order while a Hebrew name in one still
//! reads right to left.

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

/// A table or frame border: it parts segments and keeps its column.
fn border(cell: &Cell) -> bool {
    crate::sprites::is_sprite(&cell.text) || cell.text.chars().next().is_some_and(|c| ('\u{2500}'..='\u{259f}').contains(&c))
}

fn blank(cell: &Cell) -> bool {
    cell.text.is_empty() || cell.text == " "
}

/// The row's drawing order, or `None` when it reads left to right as it is.
pub fn order(row: &[Cell]) -> Option<RowOrder> {
    if !has_rtl(row) {
        return None;
    }
    // Each logical cell is one unit (its whole grapheme; a wide character's
    // tail cell is part of its head): (logical cell, columns). `line` is the
    // row's text, a blank cell a space, with each unit's byte offset in `at`.
    let mut units: Vec<(usize, usize)> = Vec::with_capacity(row.len());
    let mut line = String::with_capacity(row.len());
    let mut at = Vec::with_capacity(row.len());
    let mut x = 0;
    while x < row.len() {
        let width = if row[x].wide { 2 } else { 1 };
        units.push((x, width));
        at.push(line.len());
        line.push_str(if row[x].text.is_empty() { " " } else { &row[x].text });
        x += width;
    }
    let forced = path_separators(row, &units, &line, &at);
    let mut visual: Vec<usize> = (0..row.len()).collect();
    let mut rtl = vec![false; row.len()];
    let unit_blank = |u: usize| units.get(u).is_some_and(|&(cell, _)| blank(&row[cell]));
    let ends_segment = |u: usize| border(&row[units[u].0]) || (unit_blank(u) && (u > 0 && unit_blank(u - 1) || unit_blank(u + 1)));
    let mut start = 0;
    for u in 0..=units.len() {
        if u < units.len() && !ends_segment(u) {
            continue;
        }
        let segment = &units[start..u];
        // A segment without right-to-left text reads as it is.
        if segment.iter().any(|&(cell, _)| row[cell].text.chars().any(strong_rtl)) {
            place(row, segment, &forced[start..u], &mut visual, &mut rtl);
        }
        start = u + 1;
    }
    let mut logical = vec![0; row.len()];
    for (cell, &v) in visual.iter().enumerate() {
        logical[v] = cell;
    }
    Some(RowOrder { visual, logical, rtl })
}

/// Which units are a `/` or `\` inside a path or URL.
fn path_separators(row: &[Cell], units: &[(usize, usize)], line: &str, at: &[usize]) -> Vec<bool> {
    let mut forced = vec![false; units.len()];
    if !line.contains(['/', '\\']) {
        return forced;
    }
    for span in crate::links::spans(line) {
        for (u, &(cell, _)) in units.iter().enumerate() {
            if span.contains(&at[u]) && matches!(row[cell].text.as_str(), "/" | "\\") {
                forced[u] = true;
            }
        }
    }
    forced
}

/// Draws one segment (neighboring units) as a left-to-right paragraph in the
/// columns it covers.
fn place(row: &[Cell], units: &[(usize, usize)], forced: &[bool], visual: &mut [usize], rtl: &mut [bool]) {
    // The text the algorithm sees: each unit's grapheme, a blank cell a
    // space, a path separator a left-to-right letter in its place.
    let mut text = String::with_capacity(units.len());
    let mut starts = Vec::with_capacity(units.len());
    for (&(cell, _), &forced) in units.iter().zip(forced) {
        starts.push(text.len());
        let c = &row[cell];
        if forced {
            text.push('a');
        } else if c.text.is_empty() {
            text.push(' ');
        } else {
            // A paragraph separator would split the row; a row is one line.
            text.extend(c.text.chars().map(|ch| if bidi_class(ch) == BidiClass::B { ' ' } else { ch }));
        }
    }
    let info = ParagraphBidiInfo::new(&text, Some(Level::ltr()));
    // Resolved levels per byte, with rule L1 applied (trailing whitespace at
    // the paragraph level: blank cells after the text stay on the right).
    let levels = info.reordered_levels(0..text.len());
    let unit_levels: Vec<Level> = starts.iter().map(|&at| levels[at]).collect();
    // Visual position -> unit index (rule L2).
    let by_visual = ParagraphBidiInfo::reorder_visual(&unit_levels);
    let mut col = units[0].0;
    for &u in &by_visual {
        let (cell, width) = units[u];
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
    fn table_borders_keep_columns_apart() {
        // Under the plain algorithm a border between two Hebrew words is a
        // neutral inside one right-to-left run, and the columns trade places.
        let cells = row("│ שם │ גודל │", 20);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "│ םש │ לדוג │");
        for x in [0, 4, 11] {
            assert_eq!(o.visual[x], x, "a border keeps its column");
        }
        let cells = row("╭─ שלום ─╮", 12);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "╭─ םולש ─╮");
    }

    #[test]
    fn aligned_columns_keep_their_order() {
        // Two or more blanks part columns (ls, git status, CLI tables): each
        // column's words read right to left in place.
        let cells = row("שם      גודל", 20);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "םש      לדוג");
        let cells = row("קובץ.txt  12K  היום", 30);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "ץבוק.txt  12K  םויה");
        // One space is part of the sentence: the words keep reading order.
        let cells = row("שלום עולם", 20);
        assert_eq!(shown(&cells, &order(&cells).unwrap()), "םלוע םולש");
    }

    #[test]
    fn paths_and_urls_keep_their_parts_in_order() {
        // A slash between two Hebrew folder names would otherwise turn
        // right to left and swap the folders.
        let cells = row("ls /Users/dan/פרויקטים/תיקייה/src", 40);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "ls /Users/dan/םיטקיורפ/הייקית/src");
        let cells = row("ראה https://example.com/עמוד/שני?x=1", 50);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "האר https://example.com/דומע/ינש?x=1");
        let cells = row("ערוך src/רכיבים/כפתור.tsx", 40);
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "ךורע src/םיביכר/רותפכ.tsx");
        // Outside a path a slash is prose ("yes/no") and reads right to left.
        let cells = row("כן/לא", 10);
        assert_eq!(shown(&cells, &order(&cells).unwrap()), "אל/ןכ");
    }

    /// What agents and tools print: each row as drawn (UAX #9, implicit mode,
    /// left-to-right rows). Read the Hebrew in each from right to left.
    #[test]
    fn numbers_commands_and_prompts_read_as_written() {
        for (text, drawn) in [
            // Numbers keep their digits in order inside Hebrew (W4, W5, L2).
            ("מחיר: ₪1,234.50", "₪1,234.50 :ריחמ"),
            ("תאריך 15/11/2026", "15/11/2026 ךיראת"),
            ("גרסה 0.1.15", "0.1.15 הסרג"),
            ("טלפון +972-50-1234567", "972-50-1234567+ ןופלט"),
            ("שגיאה בשורה 42: unexpected token", "42 הרושב האיגש: unexpected token"),
            // A row is a left-to-right paragraph: one that starts with a
            // number keeps it at the left.
            ("50% הושלמו", "50% ומלשוה"),
            // Commands, hashes and code stay as typed around Hebrew.
            ("commit a1b2c3d נדחף", "commit a1b2c3d ףחדנ"),
            ("`npm run build` נכשל", "`npm run build` לשכנ"),
            ("$ echo שלום", "$ echo םולש"),
            ("❯ תקן את הבאג", "❯ גאבה תא ןקת"),
            // A branch name is no path (no dot): it reads as Hebrew text.
            ("git checkout -b תיקון/באג-12", "git checkout -b 12-גאב/ןוקית"),
        ] {
            let cells = row(text, 40);
            assert_eq!(shown(&cells, &order(&cells).unwrap()), drawn, "{text}");
        }
    }

    #[test]
    fn niqqud_and_emoji_stay_with_their_cells() {
        // Libghostty keeps a letter and its marks in one cell.
        let mut cells: Vec<Cell> = ["שָׁ", "ל", "וֹ", "ם"].iter().map(|t| Cell { text: (*t).into(), ..Cell::default() }).collect();
        cells.resize(8, Cell::default());
        let o = order(&cells).unwrap();
        assert_eq!(shown(&cells, &o), "םוֹלשָׁ");
        assert_eq!(&o.visual[..4], &[3, 2, 1, 0]);
        // An emoji is a neutral, two cells wide: it stays first, whole.
        let mut cells = vec![Cell { text: "🙂".into(), wide: true, ..Cell::default() }, Cell::default()];
        cells.extend(row(" שלום", 10));
        let o = order(&cells).unwrap();
        assert_eq!((o.visual[0], o.visual[1]), (0, 1));
        assert_eq!(&o.visual[3..7], &[6, 5, 4, 3]);
    }

    /// Escape sequences are read by libghostty before a row exists, so they
    /// can't be reordered or split; colors and links travel with their letters.
    #[test]
    fn escape_sequences_never_reach_the_reordering() {
        let mut vt = crate::vt::Vt::new(20, 2);
        vt.write("\x1b[31mשל\x1b[0mום \x1b]8;;https://example.com\x1b\\קישור\x1b]8;;\x1b\\".as_bytes());
        let row = vt.frame().rows[0].clone();
        assert!(row.iter().all(|c| !c.text.contains(['\x1b', '\x07'])));
        let text: String = row.iter().map(|c| if c.text.is_empty() { " " } else { c.text.as_str() }).collect();
        assert_eq!(text.trim_end(), "שלום קישור", "the grid stays in logical order");
        let o = order(&row).unwrap();
        assert_eq!(shown(&row, &o), "רושיק םולש");
        // The two red letters typed first are drawn rightmost, still red.
        assert!(row[0].fg == row[1].fg && row[1].fg != row[2].fg);
        assert_eq!((o.visual[0], o.visual[1]), (9, 8));
    }

    /// Rules that hold for any row, checked on random mixes of Hebrew,
    /// Latin, digits, punctuation, borders, blanks and wide characters.
    #[test]
    fn every_row_keeps_the_invariants() {
        let alphabet = ["א", "ב", "ש", "ל", "ם", "a", "b", "x", "1", "2", " ", " ", ".", ",", ":", "-", "/", "(", ")", "│", "─", "", "日"];
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let hebrew = |c: &Cell| c.text.chars().any(strong_rtl);
        let latin = |c: &Cell| c.text.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
        let text = |cells: &[Cell]| cells.iter().map(|c| if c.text.is_empty() { "·" } else { c.text.as_str() }).collect::<String>();
        for _ in 0..5000 {
            let len = 1 + (next() % 30) as usize;
            let mut cells = Vec::with_capacity(len);
            while cells.len() < len {
                let text = alphabet[(next() % alphabet.len() as u64) as usize];
                let wide = text == "日" && cells.len() + 1 < len;
                cells.push(Cell { text: if text == "日" && !wide { "a".into() } else { text.into() }, wide, ..Cell::default() });
                if wide {
                    cells.push(Cell::default());
                }
            }
            let Some(o) = order(&cells) else {
                assert!(!has_rtl(&cells));
                continue;
            };
            let mut seen = vec![false; len];
            for (x, &v) in o.visual.iter().enumerate() {
                assert!(v < len && !seen[v], "a permutation of the row's columns: {}", text(&cells));
                seen[v] = true;
                assert_eq!(o.logical[v], x);
            }
            for x in 0..len {
                if cells[x].wide && x + 1 < len {
                    assert_eq!(o.visual[x + 1], o.visual[x] + 1, "a wide tail follows its head");
                }
                if border(&cells[x]) {
                    assert_eq!(o.visual[x], x, "a border keeps its column");
                }
                if x + 1 < len && latin(&cells[x]) && latin(&cells[x + 1]) {
                    assert_eq!(o.visual[x + 1], o.visual[x] + 1, "Latin reads left to right: {}", text(&cells));
                }
                if x + 1 < len && hebrew(&cells[x]) && hebrew(&cells[x + 1]) {
                    assert_eq!(o.visual[x + 1] + 1, o.visual[x], "Hebrew reads right to left: {}", text(&cells));
                }
            }
        }
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
