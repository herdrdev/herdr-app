//! HERDR: live-preview Markdown for script editors.
//!
//! A [`LinePresenter`] that reads each line as Markdown and tells the editor
//! what to hide, replace and restyle: markers disappear until the caret
//! touches their element, headings are larger, bullets and checkboxes are
//! drawn as glyphs, code and quotes get their own look. The text itself is
//! never changed; offsets stay in source bytes.
use std::collections::HashMap;
use std::ops::Range;

use gpui::{Hsla, SharedString};
use gpui_base::input::{
    LineImage, LinePresentation, LinePresenter, PresentationSpan, PresenterEdit, Rope, SpanStyle,
};

thread_local! {
    /// The presenter behind each live-preview editor, by the editor's entity:
    /// how a script reaches it later to hand over pictures.
    static PRESENTERS: std::cell::RefCell<
        HashMap<gpui::EntityId, std::rc::Weak<std::cell::RefCell<MarkdownPresenter>>>,
    > = std::cell::RefCell::new(HashMap::new());
}

pub(crate) fn register(
    editor: gpui::EntityId,
    presenter: &std::rc::Rc<std::cell::RefCell<MarkdownPresenter>>,
) {
    PRESENTERS.with(|all| {
        let mut all = all.borrow_mut();
        all.retain(|_, presenter| presenter.strong_count() > 0);
        all.insert(editor, std::rc::Rc::downgrade(presenter));
    });
}

pub(crate) fn presenter_of(
    editor: gpui::EntityId,
) -> Option<std::rc::Rc<std::cell::RefCell<MarkdownPresenter>>> {
    PRESENTERS.with(|all| all.borrow().get(&editor).and_then(std::rc::Weak::upgrade))
}

/// A picture a note embeds, read from disk once and kept until its file changes.
struct Picture {
    path: String,
    modified: Option<std::time::SystemTime>,
    image: std::sync::Arc<gpui::RenderImage>,
    size: gpui::Size<gpui::Pixels>,
}

fn read_picture(path: &str, cx: &gpui::App) -> Option<Picture> {
    let format = match std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => gpui::ImageFormat::Png,
        "jpg" | "jpeg" => gpui::ImageFormat::Jpeg,
        "webp" => gpui::ImageFormat::Webp,
        "gif" => gpui::ImageFormat::Gif,
        "svg" => gpui::ImageFormat::Svg,
        "bmp" => gpui::ImageFormat::Bmp,
        "tif" | "tiff" => gpui::ImageFormat::Tiff,
        _ => return None,
    };
    let modified = std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let bytes = std::fs::read(path).ok()?;
    // A picture that will not decode is left as its link.
    let image = gpui::Image::from_bytes(format, bytes)
        .to_image_data(cx.svg_renderer())
        .ok()?;
    let pixels = image.size(0);
    let size = gpui::size(
        gpui::px(i32::from(pixels.width) as f32),
        gpui::px(i32::from(pixels.height) as f32),
    );
    Some(Picture {
        path: path.to_owned(),
        modified,
        image,
        size,
    })
}

/// The picture a line is, when the line is nothing but an embed: its target
/// as written and the width it asks for (`![[shot.png|300]]`).
fn picture_line(line: &str) -> Option<(&str, Option<f32>)> {
    let trimmed = line.trim();
    if let Some(inner) = trimmed
        .strip_prefix("![[")
        .and_then(|rest| rest.strip_suffix("]]"))
    {
        if inner.contains("]]") || inner.is_empty() {
            return None;
        }
        let (target, size) = match inner.split_once('|') {
            Some((target, size)) => (target, size.trim().parse::<f32>().ok()),
            None => (inner, None),
        };
        let target = target.split('#').next().unwrap_or(target).trim();
        return Some((target, size.filter(|width| *width >= 16.)));
    }
    let rest = trimmed.strip_prefix("![")?;
    let close = rest.find("](")?;
    let src = rest[close + 2..].strip_suffix(')')?;
    if src.contains(')') || src.contains("](") {
        return None;
    }
    let src = match src.trim_start().strip_prefix('<') {
        Some(inner) => inner.split('>').next()?,
        None => src.split_whitespace().next()?,
    };
    Some((src, None))
}

/// The colors and fonts live preview draws with.
#[derive(Clone, Debug)]
pub(crate) struct MarkdownTheme {
    pub heading: Option<Hsla>,
    pub marker: Hsla,
    pub link: Hsla,
    pub code: Hsla,
    pub code_background: Option<Hsla>,
    pub quote: Hsla,
    pub quote_bar: Hsla,
    pub tag: Hsla,
    pub tag_background: Option<Hsla>,
    pub highlight: Hsla,
    pub rule: Hsla,
    pub muted: Hsla,
    pub checkbox: Hsla,
    pub mono: SharedString,
    /// An icon font with Font Awesome's square glyphs, for task checkboxes.
    pub symbols: Option<SharedString>,
    /// Font size multipliers for heading levels 1 to 6.
    pub heading_scale: [f32; 6],
}

impl MarkdownTheme {
    /// Colors by name from the editor's `syntax` option, over readable defaults.
    pub(crate) fn from_colors(colors: &HashMap<String, Hsla>, mono: SharedString) -> Self {
        let pick = |name: &str, fallback: u32| -> Hsla {
            colors
                .get(name)
                .copied()
                .unwrap_or_else(|| gpui::rgb(fallback).into())
        };
        let marker = pick("punctuation", 0x7f848e);
        let link = pick("link", 0x61afef);
        Self {
            heading: colors.get("heading").copied(),
            marker,
            link,
            code: pick("code", 0x98c379),
            code_background: colors.get("code.background").copied(),
            quote: pick("quote", 0x9aa0a6),
            quote_bar: pick("quote.bar", 0x5c6370),
            tag: pick("tag", 0x61afef),
            tag_background: colors.get("tag.background").copied(),
            highlight: pick("highlight", 0x6b5a1e),
            rule: pick("rule", 0x5c6370),
            muted: pick("muted", 0x7f848e),
            checkbox: pick("checkbox", 0x61afef),
            mono,
            symbols: None,
            heading_scale: [1.6, 1.4, 1.25, 1.12, 1.05, 1.0],
        }
    }
}

/// What kind of block a line belongs to; only blocks that span lines are tracked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    Normal,
    /// A ``` or ~~~ line opening or closing a code block.
    Fence,
    /// A line inside a fenced code block.
    Code,
    /// YAML front matter, including its `---` lines.
    Front,
    /// A row of a pipe table; `sig` changes when the table's column widths do.
    Table { sig: u64, header: bool },
    /// The `| --- | --- |` row under a table's header.
    TableRule { sig: u64 },
}

/// One pipe table: its rows and the width of each column, in characters of
/// what the cells show (markers hidden).
#[derive(Clone, Debug, PartialEq)]
struct Table {
    rows: Range<usize>,
    widths: Vec<usize>,
}

/// The cells of a table row: the byte range of each one's trimmed content.
fn table_cells(line: &str) -> Vec<Range<usize>> {
    let bytes = line.as_bytes();
    let mut cells = Vec::new();
    let mut i = line.len() - line.trim_start().len();
    if bytes.get(i) == Some(&b'|') {
        i += 1;
    }
    let mut start = i;
    let mut code = false;
    while i <= bytes.len() {
        let at_end = i == bytes.len();
        let b = if at_end { b'|' } else { bytes[i] };
        match b {
            b'\\' if !at_end => {
                i += 2;
                continue;
            }
            b'`' if !at_end => code = !code,
            b'|' if !code || at_end => {
                let raw = &line[start..i.min(line.len())];
                let lead = raw.len() - raw.trim_start().len();
                let content = start + lead..start + raw.trim_end().len().max(lead);
                // The text after the last pipe is not a cell when it is empty.
                if !(at_end && raw.trim().is_empty()) {
                    cells.push(content);
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    cells
}

fn is_table_rule(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('-')
        && trimmed.contains('|')
        && trimmed
            .bytes()
            .all(|b| matches!(b, b'|' | b'-' | b':' | b' '))
}

pub(crate) struct MarkdownPresenter {
    theme: MarkdownTheme,
    blocks: Vec<Block>,
    tables: Vec<Table>,
    /// Pictures by the target a note writes for them.
    pictures: HashMap<String, Picture>,
}

impl MarkdownPresenter {
    pub(crate) fn new(theme: MarkdownTheme) -> Self {
        Self {
            theme,
            blocks: Vec::new(),
            tables: Vec::new(),
            pictures: HashMap::new(),
        }
    }

    /// Replace the pictures the note's embeds draw: `(target as written, file)`.
    /// Files already read and unchanged are kept. Returns whether anything changed.
    pub(crate) fn set_pictures(&mut self, sources: Vec<(String, String)>, cx: &gpui::App) -> bool {
        let mut old = std::mem::take(&mut self.pictures);
        let mut changed = old.len() != sources.len();
        for (target, path) in sources {
            let modified = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok();
            let kept = old
                .remove(&target)
                .filter(|picture| picture.path == path && picture.modified == modified);
            let picture = match kept {
                Some(picture) => Some(picture),
                None => {
                    changed = true;
                    read_picture(&path, cx)
                }
            };
            if let Some(picture) = picture {
                self.pictures.insert(target, picture);
            }
        }
        changed
    }

    /// The picture drawn for a line that is only an embed.
    fn picture(&self, line: &str, below_text: bool) -> Option<LineImage> {
        let (target, width) = picture_line(line)?;
        let picture = self.pictures.get(target)?;
        Some(LineImage {
            image: picture.image.clone(),
            size: picture.size,
            max_width: width.map(gpui::px),
            below_text,
        })
    }

    fn table_at(&self, row: usize) -> Option<&Table> {
        let ix = self.tables.partition_point(|table| table.rows.end <= row);
        self.tables.get(ix).filter(|table| table.rows.contains(&row))
    }

    /// Find pipe tables among the normal lines and measure their columns.
    fn scan_tables(&mut self, text: &str) {
        self.tables.clear();
        let lines: Vec<&str> = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect();
        let is_row = |blocks: &[Block], row: usize| {
            blocks.get(row) == Some(&Block::Normal)
                && lines.get(row).is_some_and(|l| l.trim_start().starts_with('|'))
        };
        let mut row = 0;
        while row + 1 < lines.len() {
            if !(is_row(&self.blocks, row)
                && is_row(&self.blocks, row + 1)
                && is_table_rule(lines[row + 1]))
            {
                row += 1;
                continue;
            }
            let mut end = row + 2;
            while is_row(&self.blocks, end) {
                end += 1;
            }
            let plain = Parser {
                theme: &self.theme,
                reveal: None,
            };
            let mut widths: Vec<usize> = Vec::new();
            for (ix, line) in lines[row..end].iter().enumerate() {
                if ix == 1 {
                    continue;
                }
                for (col, cell) in table_cells(line).into_iter().enumerate() {
                    let mut spans = Vec::new();
                    plain.inline(line, cell.clone(), &SpanStyle::default(), &mut spans);
                    let shown = LinePresentation {
                        spans,
                        ..Default::default()
                    }
                    .visible_text(line, cell, &gpui::font(""))
                    .0
                    .chars()
                    .count();
                    if widths.len() <= col {
                        widths.resize(col + 1, 0);
                    }
                    widths[col] = widths[col].max(shown);
                }
            }
            let sig = widths
                .iter()
                .fold(0xcbf29ce484222325u64, |hash, width| {
                    (hash ^ *width as u64).wrapping_mul(0x100000001b3)
                });
            for (ix, block) in self.blocks[row..end].iter_mut().enumerate() {
                *block = if ix == 1 {
                    Block::TableRule { sig }
                } else {
                    Block::Table { sig, header: ix == 0 }
                };
            }
            self.tables.push(Table {
                rows: row..end,
                widths,
            });
            row = end;
        }
    }

    /// A table row drawn as aligned columns: pipes become gutters and each
    /// cell is padded to its column's width, in the code font so they line up.
    fn table_row(
        &self,
        table: &Table,
        line: &str,
        header: bool,
        reveal: Option<Range<usize>>,
    ) -> LinePresentation {
        let theme = &self.theme;
        let parser = Parser { theme, reveal };
        let mono = SpanStyle {
            font_family: Some(theme.mono.clone()),
            bold: header,
            ..Default::default()
        };
        let gap = SpanStyle {
            font_family: Some(theme.mono.clone()),
            ..Default::default()
        };
        let mut out = LinePresentation::default();
        let cells = table_cells(line);
        let mut at = 0;
        for (col, cell) in cells.iter().enumerate() {
            // Everything since the previous cell: its padding, the pipe, a space.
            if at < cell.start {
                let pad = if col == 0 {
                    String::new()
                } else {
                    " ".repeat(self.pad(table, line, &cells, col - 1, &parser) + 3)
                };
                out.spans
                    .push(PresentationSpan::replace(at..cell.start, pad, gap.clone()));
            }
            if cell.is_empty() {
                at = cell.end;
                continue;
            }
            parser.inline(line, cell.clone(), &mono, &mut out.spans);
            at = cell.end;
        }
        if at < line.len() {
            out.spans
                .push(PresentationSpan::replace(at..line.len(), "", gap));
        }
        out
    }

    /// How many spaces fill cell `col` of `line` out to its column's width.
    fn pad(
        &self,
        table: &Table,
        line: &str,
        cells: &[Range<usize>],
        col: usize,
        parser: &Parser<'_>,
    ) -> usize {
        let cell = cells[col].clone();
        let mut spans = Vec::new();
        parser.inline(line, cell.clone(), &SpanStyle::default(), &mut spans);
        let shown = LinePresentation {
            spans,
            ..Default::default()
        }
        .visible_text(line, cell, &gpui::font(""))
        .0
        .chars()
        .count();
        table.widths.get(col).copied().unwrap_or(0).saturating_sub(shown)
    }
}

/// The fence a line opens or closes: its character and length.
fn fence(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let len = trimmed.bytes().take_while(|&b| b == first).count();
    (len >= 3).then_some((first, len))
}

fn scan_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut open: Option<(u8, usize)> = None;
    let mut front = false;
    for (row, line) in text.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if row == 0 && line.trim_end() == "---" {
            front = true;
            blocks.push(Block::Front);
            continue;
        }
        if front {
            blocks.push(Block::Front);
            if matches!(line.trim_end(), "---" | "...") {
                front = false;
            }
            continue;
        }
        match (open, fence(line)) {
            (None, Some(found)) => {
                open = Some(found);
                blocks.push(Block::Fence);
            }
            (Some((ch, len)), Some((found, found_len)))
                if found == ch
                    && found_len >= len
                    && line.trim_start_matches(' ')[found_len..].trim().is_empty() =>
            {
                open = None;
                blocks.push(Block::Fence);
            }
            (Some(_), _) => blocks.push(Block::Code),
            (None, None) => blocks.push(Block::Normal),
        }
    }
    blocks
}

impl LinePresenter for MarkdownPresenter {
    fn sync(&mut self, text: &Rope, edit: Option<PresenterEdit>) -> Vec<usize> {
        let source = text.to_string();
        let old = std::mem::replace(&mut self.blocks, scan_blocks(&source));
        // Tables are an extra: a table this cannot measure stays plain source.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.scan_tables(&source)))
            .is_err()
        {
            self.tables.clear();
            self.blocks = scan_blocks(&source);
        }
        let Some(edit) = edit else {
            return Vec::new();
        };
        // Rows after the edit moved by its line delta; report the ones whose
        // block changed (a fence opened or closed above them).
        let delta = edit.new_end_row as isize - edit.old_end_row as isize;
        let above = (0..edit.start_row.min(self.blocks.len()))
            .filter(|&row| old.get(row) != Some(&self.blocks[row]));
        let below = (edit.new_end_row + 1..self.blocks.len()).filter(|&row| {
            let before = (row as isize - delta) as usize;
            old.get(before) != Some(&self.blocks[row])
        });
        above.chain(below).collect()
    }

    fn line(
        &self,
        row: usize,
        line: &str,
        reveal: Option<Range<usize>>,
    ) -> Option<LinePresentation> {
        // A line this parser cannot read is drawn as plain text, never a crash:
        // the editor asks about every line the person types.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.present(row, line, reveal)
        }))
        .unwrap_or(None)
    }
}

impl MarkdownPresenter {
    fn present(
        &self,
        row: usize,
        line: &str,
        reveal: Option<Range<usize>>,
    ) -> Option<LinePresentation> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let theme = &self.theme;
        let mono = |color: Hsla| SpanStyle {
            color: Some(color),
            font_family: Some(theme.mono.clone()),
            ..Default::default()
        };
        match self.blocks.get(row).copied().unwrap_or(Block::Normal) {
            Block::Front => Some(LinePresentation {
                spans: whole(line, mono(theme.muted)),
                ..Default::default()
            }),
            Block::Fence => Some(LinePresentation {
                spans: whole(line, mono(theme.marker)),
                background: theme.code_background,
                ..Default::default()
            }),
            Block::Code => Some(LinePresentation {
                spans: whole(line, mono(theme.code)),
                background: theme.code_background,
                ..Default::default()
            }),
            Block::Table { header, .. } => {
                let table = self.table_at(row)?;
                Some(self.table_row(table, line, header, reveal))
            }
            Block::TableRule { .. } => Some(if reveal.is_some() {
                LinePresentation {
                    spans: whole(line, mono(theme.marker)),
                    ..Default::default()
                }
            } else {
                LinePresentation {
                    spans: whole(line, SpanStyle::default())
                        .into_iter()
                        .map(|span| PresentationSpan::hidden(span.range))
                        .collect(),
                    rule: Some(theme.rule),
                    ..Default::default()
                }
            }),
            Block::Normal => {
                // A line that is only a picture draws it: in place of its source,
                // or under it while the caret is on the line.
                if let Some(image) = self.picture(line, reveal.is_some()) {
                    let mut presentation = if reveal.is_some() {
                        Parser { theme, reveal }.line(line)
                    } else {
                        LinePresentation {
                            spans: vec![PresentationSpan::hidden(0..line.len())],
                            ..Default::default()
                        }
                    };
                    presentation.image = Some(image);
                    return Some(presentation);
                }
                let presentation = Parser { theme, reveal }.line(line);
                (presentation.changes_layout() || presentation.has_decor()).then_some(presentation)
            }
        }
    }
}

fn whole(line: &str, style: SpanStyle) -> Vec<PresentationSpan> {
    if line.is_empty() {
        Vec::new()
    } else {
        vec![PresentationSpan::text(0..line.len(), style)]
    }
}

struct Parser<'a> {
    theme: &'a MarkdownTheme,
    /// The selection inside this line, when the caret is on it.
    reveal: Option<Range<usize>>,
}

impl Parser<'_> {
    /// Whether the selection touches `element`, so its markers show as source.
    fn revealed(&self, element: &Range<usize>) -> bool {
        self.reveal
            .as_ref()
            .is_some_and(|sel| sel.start <= element.end && sel.end >= element.start)
    }

    /// A marker: hidden, or dimmed source while its element is revealed.
    fn marker(&self, out: &mut Vec<PresentationSpan>, range: Range<usize>, revealed: bool) {
        if range.is_empty() {
            return;
        }
        out.push(if revealed {
            PresentationSpan::text(
                range,
                SpanStyle {
                    color: Some(self.theme.marker),
                    ..Default::default()
                },
            )
        } else {
            PresentationSpan::hidden(range)
        });
    }

    fn line(&self, line: &str) -> LinePresentation {
        let theme = self.theme;
        let mut out = LinePresentation::default();
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let rest = &line[indent..];
        let on_line = self.reveal.is_some();

        // Heading: `#` to `######`, then a space or the end of the line.
        let hashes = rest.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes)
            && indent <= 3
            && rest[hashes..].chars().next().is_none_or(|c| c == ' ')
        {
            let spaces = rest[hashes..].bytes().take_while(|&b| b == b' ').count();
            let text_start = indent + hashes + spaces;
            out.scale = theme.heading_scale[hashes - 1];
            self.marker(&mut out.spans, 0..text_start, on_line);
            let style = SpanStyle {
                bold: true,
                color: theme.heading,
                ..Default::default()
            };
            self.inline(line, text_start..line.len(), &style, &mut out.spans);
            return out;
        }

        // Thematic break: three or more of the same `-`, `*` or `_`.
        let compact: Vec<u8> = rest.bytes().filter(|&b| b != b' ').collect();
        if compact.len() >= 3
            && indent <= 3
            && matches!(compact[0], b'-' | b'*' | b'_')
            && compact.iter().all(|&b| b == compact[0])
        {
            if on_line {
                self.marker(&mut out.spans, 0..line.len(), true);
            } else {
                out.spans.push(PresentationSpan::hidden(0..line.len()));
                out.rule = Some(theme.rule);
            }
            return out;
        }

        // Block quote (and callout): `>` markers become a bar down the side.
        if rest.starts_with('>') && indent <= 3 {
            let mut end = indent;
            let bytes = line.as_bytes();
            while end < bytes.len() && bytes[end] == b'>' {
                end += 1;
                if bytes.get(end) == Some(&b' ') {
                    end += 1;
                }
            }
            if on_line {
                self.marker(&mut out.spans, 0..end, true);
            } else {
                out.spans.push(PresentationSpan::replace(
                    0..end,
                    "   ",
                    SpanStyle::default(),
                ));
            }
            out.left_bar = Some(theme.quote_bar);
            let mut style = SpanStyle {
                color: Some(theme.quote),
                ..Default::default()
            };
            let mut text_start = end;
            // `[!note]` opens a callout: the type reads as a label.
            if line[end..].starts_with("[!")
                && let Some(close) = line[end..].find(']')
            {
                let label = end..end + close + 1;
                out.spans.push(PresentationSpan::text(
                    label.clone(),
                    SpanStyle {
                        color: Some(theme.tag),
                        bold: true,
                        ..Default::default()
                    },
                ));
                text_start = label.end;
                style.bold = true;
            }
            self.inline(line, text_start..line.len(), &style, &mut out.spans);
            return out;
        }

        // List item: a bullet, a number, or a task with its checkbox.
        if let Some(marker_len) = list_marker(rest) {
            let marker = indent..indent + marker_len;
            let after = marker.end + line[marker.end..].bytes().take_while(|&b| b == b' ').count();
            let bullet = matches!(rest.as_bytes()[0], b'-' | b'*' | b'+');
            let task = bullet
                .then(|| line[after..].get(..3))
                .flatten()
                .filter(|boxed| matches!(*boxed, "[ ]" | "[x]" | "[X]"))
                .filter(|_| line[after + 3..].chars().next().is_none_or(|c| c == ' '));
            let dim = SpanStyle {
                color: Some(theme.marker),
                ..Default::default()
            };
            if let Some(boxed) = task {
                let done = boxed != "[ ]";
                let element = marker.start..after + 3;
                if self.revealed(&element) {
                    out.spans
                        .push(PresentationSpan::text(element.clone(), dim.clone()));
                } else {
                    out.spans.push(PresentationSpan::hidden(marker.start..after));
                    // Font Awesome's square-o and check-square from the icon
                    // font when there is one; fallback fonts draw the plain
                    // box characters at uneven sizes.
                    let glyph = match (&theme.symbols, done) {
                        (Some(_), false) => "\u{f096}",
                        (Some(_), true) => "\u{f14a}",
                        (None, false) => "☐",
                        (None, true) => "☑",
                    };
                    out.spans.push(PresentationSpan::replace(
                        after..after + 3,
                        glyph,
                        SpanStyle {
                            color: Some(if done { theme.checkbox } else { theme.marker }),
                            font_family: theme.symbols.clone(),
                            ..Default::default()
                        },
                    ));
                }
                let style = if done {
                    SpanStyle {
                        color: Some(theme.muted),
                        strikethrough: true,
                        ..Default::default()
                    }
                } else {
                    SpanStyle::default()
                };
                self.inline(line, after + 3..line.len(), &style, &mut out.spans);
            } else {
                if bullet {
                    out.spans
                        .push(PresentationSpan::replace(marker.clone(), "•", dim));
                } else {
                    out.spans.push(PresentationSpan::text(marker.clone(), dim));
                }
                self.inline(
                    line,
                    marker.end..line.len(),
                    &SpanStyle::default(),
                    &mut out.spans,
                );
            }
            return out;
        }

        self.inline(line, 0..line.len(), &SpanStyle::default(), &mut out.spans);
        out
    }

    /// Emit a plain stretch in the surrounding style (nothing when that is the default).
    fn plain(&self, out: &mut Vec<PresentationSpan>, range: Range<usize>, style: &SpanStyle) {
        if !range.is_empty() && *style != SpanStyle::default() {
            out.push(PresentationSpan::text(range, style.clone()));
        }
    }

    /// Inline Markdown inside `range` of `line`, drawn over `style`.
    fn inline(
        &self,
        line: &str,
        range: Range<usize>,
        style: &SpanStyle,
        out: &mut Vec<PresentationSpan>,
    ) {
        let theme = self.theme;
        let bytes = line.as_bytes();
        let end = range.end;
        let mut i = range.start;
        let mut plain_start = i;
        let prev_char = |at: usize| line[..at].chars().next_back();
        while i < end {
            let b = bytes[i];
            // Every construct starts with an ASCII byte; anything else is text
            // (and may be the middle of a character, which must not be sliced).
            if !b.is_ascii() || !line.is_char_boundary(i) {
                i += 1;
                continue;
            }
            let rest = &line[i..end];
            let mut consumed: Option<usize> = None;
            match b {
                b'\\' if rest.len() >= 2 && bytes[i + 1].is_ascii_punctuation() => {
                    self.plain(out, plain_start..i, style);
                    self.marker(out, i..i + 1, self.revealed(&(i..i + 2)));
                    plain_start = i + 1;
                    i += 2;
                    continue;
                }
                b'`' => {
                    let n = rest.bytes().take_while(|&c| c == b'`').count();
                    if let Some(close) = find_run(rest, n, b'`', n) {
                        let element = i..i + close + n;
                        let shown = self.revealed(&element);
                        self.plain(out, plain_start..i, style);
                        self.marker(out, i..i + n, shown);
                        out.push(PresentationSpan::text(
                            i + n..i + close,
                            SpanStyle {
                                color: Some(theme.code),
                                background: theme.code_background,
                                font_family: Some(theme.mono.clone()),
                                ..style.clone()
                            },
                        ));
                        self.marker(out, i + close..i + close + n, shown);
                        consumed = Some(close + n);
                    }
                }
                b'!' | b'[' if rest.starts_with("[[") || rest.starts_with("![[") => {
                    let bang = usize::from(b == b'!');
                    if let Some(close) = rest.find("]]")
                        && close > bang + 2
                    {
                        let inner = &rest[bang + 2..close];
                        let element = i..i + close + 2;
                        let shown = self.revealed(&element);
                        let label = match inner.rfind('|') {
                            Some(pipe) if !shown => bang + 2 + pipe + 1,
                            _ => bang + 2,
                        };
                        self.plain(out, plain_start..i, style);
                        self.marker(out, i..i + label, shown);
                        out.push(PresentationSpan::text(
                            i + label..i + close,
                            SpanStyle {
                                color: Some(theme.link),
                                underline: !shown,
                                ..style.clone()
                            },
                        ));
                        self.marker(out, i + close..i + close + 2, shown);
                        consumed = Some(close + 2);
                    }
                }
                b'!' | b'[' if b == b'[' || rest.starts_with("![") => {
                    let bang = usize::from(b == b'!');
                    if let Some((text_end, url_end)) = link_parts(&rest[bang..]) {
                        let (text_end, url_end) = (text_end + bang, url_end + bang);
                        let element = i..i + url_end;
                        let shown = self.revealed(&element);
                        self.plain(out, plain_start..i, style);
                        self.marker(out, i..i + bang + 1, shown);
                        let link = SpanStyle {
                            color: Some(theme.link),
                            underline: !shown,
                            ..style.clone()
                        };
                        if text_end > bang + 1 {
                            self.inline(line, i + bang + 1..i + text_end, &link, out);
                        }
                        self.marker(out, i + text_end..i + url_end, shown);
                        consumed = Some(url_end);
                    }
                }
                b'<' => {
                    if let Some(close) = rest.find('>')
                        && (rest.starts_with("<http://")
                            || rest.starts_with("<https://")
                            || rest.starts_with("<mailto:"))
                        && !rest[..close].contains(' ')
                    {
                        let shown = self.revealed(&(i..i + close + 1));
                        self.plain(out, plain_start..i, style);
                        self.marker(out, i..i + 1, shown);
                        out.push(PresentationSpan::text(
                            i + 1..i + close,
                            SpanStyle {
                                color: Some(theme.link),
                                underline: true,
                                ..style.clone()
                            },
                        ));
                        self.marker(out, i + close..i + close + 1, shown);
                        consumed = Some(close + 1);
                    }
                }
                b'h' if (rest.starts_with("http://") || rest.starts_with("https://"))
                    && prev_char(i).is_none_or(|c| !c.is_alphanumeric()) =>
                {
                    let stop = rest
                        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '|'))
                        .unwrap_or(rest.len());
                    let url = rest[..stop].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '*']);
                    if url.len() > 8 {
                        self.plain(out, plain_start..i, style);
                        out.push(PresentationSpan::text(
                            i..i + url.len(),
                            SpanStyle {
                                color: Some(theme.link),
                                underline: true,
                                ..style.clone()
                            },
                        ));
                        consumed = Some(url.len());
                    }
                }
                b'*' | b'_' => {
                    let n = rest.bytes().take_while(|&c| c == b).count();
                    let opens = n <= 3
                        && rest[n..].chars().next().is_some_and(|c| !c.is_whitespace())
                        && (b == b'*' || prev_char(i).is_none_or(|c| !c.is_alphanumeric()));
                    if opens
                        && let Some(close) = find_closer(rest, n, b)
                    {
                        let element = i..i + close + n;
                        let shown = self.revealed(&element);
                        self.plain(out, plain_start..i, style);
                        self.marker(out, i..i + n, shown);
                        let inner = SpanStyle {
                            bold: style.bold || n >= 2,
                            italic: style.italic || n != 2,
                            ..style.clone()
                        };
                        self.inline(line, i + n..i + close, &inner, out);
                        self.marker(out, i + close..i + close + n, shown);
                        consumed = Some(close + n);
                    } else {
                        // Not emphasis: step over the whole run so its tail
                        // is not read as a shorter opener.
                        i += n;
                        continue;
                    }
                }
                b'~' | b'=' if rest.len() > 4 && bytes[i + 1] == b => {
                    let n = rest.bytes().take_while(|&c| c == b).count();
                    if n == 2
                        && rest[2..].chars().next().is_some_and(|c| !c.is_whitespace())
                        && let Some(close) = find_closer(rest, 2, b)
                    {
                        let shown = self.revealed(&(i..i + close + 2));
                        self.plain(out, plain_start..i, style);
                        self.marker(out, i..i + 2, shown);
                        let inner = if b == b'~' {
                            SpanStyle {
                                strikethrough: true,
                                ..style.clone()
                            }
                        } else {
                            SpanStyle {
                                background: Some(theme.highlight),
                                ..style.clone()
                            }
                        };
                        self.inline(line, i + 2..i + close, &inner, out);
                        self.marker(out, i + close..i + close + 2, shown);
                        consumed = Some(close + 2);
                    } else {
                        i += n;
                        continue;
                    }
                }
                b'#' if prev_char(i).is_none_or(char::is_whitespace) => {
                    let len = rest[1..]
                        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '/')))
                        .unwrap_or(rest.len() - 1);
                    let name = &rest[1..1 + len];
                    if !name.is_empty() && name.chars().any(|c| !c.is_ascii_digit()) {
                        self.plain(out, plain_start..i, style);
                        out.push(PresentationSpan::text(
                            i..i + 1 + len,
                            SpanStyle {
                                color: Some(theme.tag),
                                background: theme.tag_background,
                                ..style.clone()
                            },
                        ));
                        consumed = Some(1 + len);
                    }
                }
                _ => {}
            }
            match consumed {
                Some(len) => {
                    i += len;
                    plain_start = i;
                }
                None => i += 1,
            }
        }
        self.plain(out, plain_start..end, style);
    }
}

/// The length of a list marker at the start of `rest`, without the spaces after it.
fn list_marker(rest: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let first = *bytes.first()?;
    let len = if matches!(first, b'-' | b'*' | b'+') {
        1
    } else {
        let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits == 0 || digits > 9 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
            return None;
        }
        digits + 1
    };
    (bytes.get(len) == Some(&b' ')).then_some(len)
}

/// The offset in `rest` of a closing run of exactly `n` `ch` bytes, after `from`.
fn find_run(rest: &str, from: usize, ch: u8, n: usize) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut j = from;
    while j < bytes.len() {
        if bytes[j] == ch {
            let run = bytes[j..].iter().take_while(|&&c| c == ch).count();
            if run == n {
                return Some(j);
            }
            j += run;
        } else {
            j += 1;
        }
    }
    None
}

/// The closing delimiter for emphasis opened by `n` `ch` at the start of
/// `rest`: a run of the same length that follows a non-space character (and,
/// for `_`, is not inside a word).
fn find_closer(rest: &str, n: usize, ch: u8) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut j = n;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'`' => {
                // Code spans are opaque to emphasis.
                let run = bytes[j..].iter().take_while(|&&c| c == b'`').count();
                j = match find_run(rest, j + run, b'`', run) {
                    Some(close) => close + run,
                    None => j + run,
                };
            }
            c if c == ch => {
                let run = bytes[j..].iter().take_while(|&&c| c == ch).count();
                let after_text = rest[..j]
                    .chars()
                    .next_back()
                    .is_some_and(|c| !c.is_whitespace());
                let word_end = ch != b'_'
                    || rest[j + run..]
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_alphanumeric());
                if run == n && after_text && word_end && j > n {
                    return Some(j);
                }
                j += run;
            }
            _ => j += 1,
        }
    }
    None
}

/// For `[text](url)` at the start of `rest`: the offsets of `]` and of the end
/// of the closing `)`.
fn link_parts(rest: &str) -> Option<(usize, usize)> {
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    let mut text_end = None;
    let mut j = 0;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    text_end = Some(j);
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    let text_end = text_end?;
    if bytes.get(text_end + 1) != Some(&b'(') {
        return None;
    }
    let mut parens = 0usize;
    for (k, &c) in bytes.iter().enumerate().skip(text_end + 1) {
        match c {
            b'(' => parens += 1,
            b')' => {
                parens -= 1;
                if parens == 0 {
                    return Some((text_end, k + 1));
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
fn presenter_has_no_tables(text: &str) -> bool {
    let mut presenter =
        MarkdownPresenter::new(MarkdownTheme::from_colors(&HashMap::new(), "Mono".into()));
    presenter.sync(&Rope::from(text), None);
    presenter.tables.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_base::input::SpanKind;

    fn presenter(text: &str) -> MarkdownPresenter {
        let mut presenter =
            MarkdownPresenter::new(MarkdownTheme::from_colors(&HashMap::new(), "Mono".into()));
        presenter.sync(&Rope::from(text), None);
        presenter
    }

    /// What a line shows: visible text with replacements, hidden bytes dropped.
    fn shown(text: &str, row: usize, reveal: Option<Range<usize>>) -> String {
        let line = text.split('\n').nth(row).unwrap();
        match presenter(text).line(row, line, reveal) {
            Some(presentation) => {
                presentation
                    .visible_text(line, 0..line.len(), &gpui::font("Test"))
                    .0
            }
            None => line.to_string(),
        }
    }

    fn spans(text: &str) -> Vec<PresentationSpan> {
        presenter(text)
            .line(0, text, None)
            .map(|p| p.spans)
            .unwrap_or_default()
    }

    #[test]
    fn markers_hide_until_the_caret_touches_their_element() {
        let text = "some **bold** and *it* text";
        assert_eq!(shown(text, 0, None), "some bold and it text");
        // Caret inside the bold word: its markers show, the italic's stay hidden.
        assert_eq!(shown(text, 0, Some(8..8)), "some **bold** and it text");
        // Caret right after the closing marker still counts as touching.
        assert_eq!(shown(text, 0, Some(13..13)), "some **bold** and it text");
        assert_eq!(shown(text, 0, Some(20..20)), "some bold and *it* text");
    }

    #[test]
    fn headings_scale_and_hide_their_hashes() {
        let presentation = presenter("## Title").line(0, "## Title", None).unwrap();
        assert_eq!(presentation.scale, 1.4);
        assert_eq!(shown("## Title", 0, None), "Title");
        assert_eq!(shown("## Title", 0, Some(3..3)), "## Title");
        assert!(presenter("#tag only").line(0, "#tag only", None).unwrap().scale == 1.);
    }

    #[test]
    fn links_wikilinks_and_code_show_only_their_text() {
        assert_eq!(shown("a [text](https://x.dev) b", 0, None), "a text b");
        assert_eq!(shown("see [[Note]] and [[Note|alias]]", 0, None), "see Note and alias");
        assert_eq!(shown("run `cargo test` now", 0, None), "run cargo test now");
        assert_eq!(shown("~~gone~~ ==marked== \\*not\\*", 0, None), "gone marked *not*");
        // Emphasis markers inside code stay literal.
        assert_eq!(shown("`a*b*c`", 0, None), "a*b*c");
    }

    #[test]
    fn lists_and_tasks_draw_glyphs() {
        assert_eq!(shown("- item", 0, None), "• item");
        assert_eq!(shown("  * nested", 0, None), "  • nested");
        assert_eq!(shown("1. first", 0, None), "1. first");
        assert_eq!(shown("- [ ] todo", 0, None), "☐ todo");
        assert_eq!(shown("- [x] done", 0, None), "☑ done");
        // Caret on the checkbox shows its source.
        assert_eq!(shown("- [x] done", 0, Some(2..2)), "- [x] done");
        assert_eq!(shown("- [x] done", 0, Some(9..9)), "☑ done");
    }

    #[test]
    fn code_fences_and_front_matter_span_lines() {
        let text = "---\ntitle: x\n---\ntext\n```rs\nlet a = **1**;\n```\nafter **b**";
        let presenter = presenter(text);
        assert_eq!(
            presenter.blocks,
            vec![
                Block::Front,
                Block::Front,
                Block::Front,
                Block::Normal,
                Block::Fence,
                Block::Code,
                Block::Fence,
                Block::Normal
            ]
        );
        // Code is not read as Markdown.
        assert_eq!(shown(text, 5, None), "let a = **1**;");
        assert_eq!(shown(text, 7, None), "after b");
    }

    #[test]
    fn opening_a_fence_reports_the_rows_it_restyles() {
        let mut presenter = presenter("a\nb\nc");
        let changed = presenter.sync(
            &Rope::from("```\nb\nc"),
            Some(PresenterEdit {
                start_row: 0,
                old_end_row: 0,
                new_end_row: 0,
            }),
        );
        assert_eq!(changed, vec![1, 2]);
        // Inserting a line shifts rows without restyling them.
        let changed = presenter.sync(
            &Rope::from("```\nx\nb\nc"),
            Some(PresenterEdit {
                start_row: 1,
                old_end_row: 1,
                new_end_row: 2,
            }),
        );
        assert!(changed.is_empty(), "{changed:?}");
    }

    #[test]
    fn spans_are_sorted_and_never_overlap() {
        for text in [
            "# **bold** [l](u) `c` #tag",
            "- [x] *a* **b** ~~c~~ ==d== [[w|x]] https://x.dev/a.",
            "> [!note] Title with **bold**",
            "***both*** _under_ snake_case_word * loose * ** x",
            "| a | **b** |\n| - | - |",
            "![alt](img.png) ![[embed.png]] <https://x.dev>",
        ] {
            let spans = spans(text);
            for pair in spans.windows(2) {
                assert!(pair[0].range.end <= pair[1].range.start, "{text}: {spans:?}");
            }
            for span in &spans {
                assert!(span.range.start < span.range.end, "{text}: {span:?}");
                assert!(text.is_char_boundary(span.range.start), "{text}");
                assert!(text.is_char_boundary(span.range.end), "{text}");
            }
        }
    }

    #[test]
    fn tables_draw_as_aligned_columns() {
        let text = "| Name | Qty |\n| --- | ---: |\n| **Apples** | 3 |\n| Kiwi |  |\nafter";
        let presenter = presenter(text);
        assert_eq!(presenter.tables.len(), 1);
        assert_eq!(presenter.tables[0].rows, 0..4);
        // Widths count what a cell shows: "Apples", not "**Apples**".
        assert_eq!(presenter.tables[0].widths, vec![6, 3]);
        assert_eq!(shown(text, 0, None), "Name     Qty");
        assert_eq!(shown(text, 1, None), "");
        assert_eq!(shown(text, 2, None), "Apples   3");
        assert_eq!(shown(text, 3, None), "Kiwi     ");
        assert_eq!(shown(text, 4, None), "after");
        // A lone pipe line is not a table.
        assert!(super::presenter_has_no_tables("| not a table |\nnext"));
    }

    #[test]
    fn widening_a_cell_restyles_the_other_rows() {
        let mut presenter = presenter("| a | b |\n| - | - |\n| c | d |");
        let changed = presenter.sync(
            &Rope::from("| a | b |\n| - | - |\n| cccc | d |"),
            Some(PresenterEdit {
                start_row: 2,
                old_end_row: 2,
                new_end_row: 2,
            }),
        );
        // The header and rule rows above the edit are padded to the new width.
        assert_eq!(changed, vec![0, 1]);
        assert_eq!(presenter.tables[0].widths, vec![4, 1]);
    }

    #[test]
    fn text_in_any_script_is_read_without_slicing_characters() {
        for text in [
            "Texto después de la imagen.",
            "ñandú **así** y *café* con [[Año nuevo|año]] #título",
            "日本語の**太字**と`コード`と[リンク](https://例.jp)",
            "- [ ] tarea ñ",
            "> cita – con — rayas",
            "| País | Ñ |\n| --- | --- |\n| México | sí |",
            "emoji 👩‍💻 **x** \\é ==ü==",
            "# Título — con ~~tachón~~",
        ] {
            let presenter = presenter(text);
            for (row, line) in text.split('\n').enumerate() {
                for reveal in [None, Some(0..0), Some(line.len()..line.len())] {
                    if let Some(presentation) = presenter.present(row, line, reveal) {
                        for span in &presentation.spans {
                            assert!(line.is_char_boundary(span.range.start), "{line}: {span:?}");
                            assert!(line.is_char_boundary(span.range.end), "{line}: {span:?}");
                        }
                        presentation.visible_text(line, 0..line.len(), &gpui::font("Test"));
                    }
                }
            }
        }
        assert_eq!(shown("ñandú **así** y *café*", 0, None), "ñandú así y café");
    }

    #[test]
    fn a_line_that_is_only_an_embed_names_its_picture() {
        assert_eq!(picture_line("![[shot.png]]"), Some(("shot.png", None)));
        assert_eq!(picture_line("  ![[img/a b.png|300]] "), Some(("img/a b.png", Some(300.))));
        assert_eq!(picture_line("![[shot.png|caption]]"), Some(("shot.png", None)));
        assert_eq!(picture_line("![alt text](pics/x.jpg \"title\")"), Some(("pics/x.jpg", None)));
        assert_eq!(picture_line("![a](<with space.png>)"), Some(("with space.png", None)));
        assert_eq!(picture_line("text ![[shot.png]]"), None);
        assert_eq!(picture_line("![[a.png]] and ![[b.png]]"), None);
        assert_eq!(picture_line("![a](x.png) more"), None);
        assert_eq!(picture_line("[[note]]"), None);
    }

    #[test]
    fn plain_words_are_left_alone() {
        assert_eq!(shown("snake_case_word and 2 * 3 * 4", 0, None), "snake_case_word and 2 * 3 * 4");
        assert!(presenter("just text").line(0, "just text", None).is_none());
        assert!(matches!(
            spans("- item")[0].kind,
            SpanKind::Replace(ref with) if with.as_ref() == "•"
        ));
    }
}
