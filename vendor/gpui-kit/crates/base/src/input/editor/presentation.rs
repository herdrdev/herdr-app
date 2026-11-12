//! HERDR: line presentation — a source line drawn differently from its bytes.
//!
//! A [`LinePresenter`] tells the editor, per buffer line, which byte ranges are
//! hidden, replaced by another glyph or drawn in another style, and how large
//! the line's text is. Offsets stay in source bytes everywhere: the editor only
//! changes what it shapes, measures and paints. This is what a live-preview
//! Markdown editor needs (markers that disappear, larger headings, bullets).
use std::ops::Range;

use gpui::{
    Font, FontStyle, FontWeight, Hsla, SharedString, StrikethroughStyle, TextRun, UnderlineStyle,
    px,
};
use ropey::Rope;

/// The style of one presented span. Unset fields keep the editor's own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpanStyle {
    pub color: Option<Hsla>,
    pub background: Option<Hsla>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub font_family: Option<SharedString>,
}

impl SpanStyle {
    /// The font this style draws with, starting from the editor's.
    pub fn font(&self, base: &Font) -> Font {
        let mut font = base.clone();
        if let Some(family) = &self.font_family {
            font.family = family.clone();
        }
        if self.bold {
            font.weight = FontWeight::BOLD;
        }
        if self.italic {
            font.style = FontStyle::Italic;
        }
        font
    }

    /// Apply this style over an existing run.
    pub fn apply(&self, run: &mut TextRun) {
        run.font = self.font(&run.font);
        if let Some(color) = self.color {
            run.color = color;
        }
        if let Some(background) = self.background {
            run.background_color = Some(background);
        }
        if self.underline {
            run.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            });
        }
        if self.strikethrough {
            run.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            });
        }
    }

    fn is_plain(&self) -> bool {
        *self == Self::default()
    }
}

/// How a span's bytes are drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum SpanKind {
    /// The source text, in the span's style.
    Text,
    /// Nothing: the bytes take no room.
    Hidden,
    /// Another string in place of the bytes; the caret steps over it as one unit.
    Replace(SharedString),
}

/// One styled, hidden or replaced byte range of a line (line-local offsets).
#[derive(Clone, Debug, PartialEq)]
pub struct PresentationSpan {
    pub range: Range<usize>,
    pub kind: SpanKind,
    pub style: SpanStyle,
}

impl PresentationSpan {
    pub fn text(range: Range<usize>, style: SpanStyle) -> Self {
        Self {
            range,
            kind: SpanKind::Text,
            style,
        }
    }
    pub fn hidden(range: Range<usize>) -> Self {
        Self {
            range,
            kind: SpanKind::Hidden,
            style: SpanStyle::default(),
        }
    }
    pub fn replace(range: Range<usize>, with: impl Into<SharedString>, style: SpanStyle) -> Self {
        Self {
            range,
            kind: SpanKind::Replace(with.into()),
            style,
        }
    }
}

/// A picture drawn in a line's rows, from the line's left edge. The line takes
/// as many rows as the picture needs at the width it is shown.
#[derive(Clone)]
pub struct LineImage {
    pub image: std::sync::Arc<gpui::RenderImage>,
    /// The picture's own size.
    pub size: gpui::Size<gpui::Pixels>,
    /// The widest it should be drawn, when the source says so (`|300`).
    pub max_width: Option<gpui::Pixels>,
    /// Drawn under the line's own text (its source is showing) rather than in its place.
    pub below_text: bool,
}

impl LineImage {
    /// The size it is drawn at: never wider than `available` or taller than
    /// a screenful, keeping its proportions.
    pub fn fit(&self, available: gpui::Pixels) -> gpui::Size<gpui::Pixels> {
        let (w, h) = (f32::from(self.size.width), f32::from(self.size.height));
        if w <= 0. || h <= 0. {
            return gpui::size(px(0.), px(0.));
        }
        let limit = self
            .max_width
            .map_or(f32::from(available), |max| f32::from(max).min(f32::from(available)))
            .max(16.);
        let scale = (limit / w).min(520. / h).min(1.);
        gpui::size(px((w * scale).round()), px((h * scale).round()))
    }
}

impl LineImage {
    /// How many rows a picture drawn at `shown` covers, with a little air around it.
    pub fn rows(shown: gpui::Size<gpui::Pixels>, line_height: gpui::Pixels) -> usize {
        ((f32::from(shown.height) + 8.) / f32::from(line_height)).ceil() as usize
    }
}

impl PartialEq for LineImage {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.image, &other.image)
            && self.size == other.size
            && self.max_width == other.max_width
            && self.below_text == other.below_text
    }
}

impl std::fmt::Debug for LineImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LineImage")
            .field("size", &self.size)
            .field("max_width", &self.max_width)
            .finish()
    }
}

/// How one buffer line is presented.
#[derive(Clone, Debug, PartialEq)]
pub struct LinePresentation {
    /// Font size multiplier for the whole line.
    pub scale: f32,
    /// Sorted, non-overlapping spans; bytes outside them are drawn as they are.
    pub spans: Vec<PresentationSpan>,
    /// Fills the line's rows edge to edge, behind the text.
    pub background: Option<Hsla>,
    /// A bar down the left edge of the line's rows.
    pub left_bar: Option<Hsla>,
    /// A horizontal rule across the middle of the first row.
    pub rule: Option<Hsla>,
    /// A picture drawn in the line's rows; the line grows to hold it.
    pub image: Option<LineImage>,
}

impl Default for LinePresentation {
    fn default() -> Self {
        Self {
            scale: 1.,
            spans: Vec::new(),
            background: None,
            left_bar: None,
            rule: None,
            image: None,
        }
    }
}

impl LinePresentation {
    /// Whether shaping this line differs from shaping its plain bytes.
    pub fn changes_layout(&self) -> bool {
        self.scale != 1. || !self.spans.is_empty() || self.image.is_some()
    }

    pub fn has_decor(&self) -> bool {
        self.background.is_some()
            || self.left_bar.is_some()
            || self.rule.is_some()
            || self.image.is_some()
    }

    /// The rows the line takes beyond its text, to hold its picture.
    pub fn extra_rows(&self, available: gpui::Pixels, line_height: gpui::Pixels) -> usize {
        let Some(image) = &self.image else {
            return 0;
        };
        if line_height <= px(0.) {
            return 0;
        }
        let rows = LineImage::rows(image.fit(available), line_height);
        rows.saturating_sub(usize::from(!image.below_text))
    }

    /// The line as consecutive pieces covering `range`: the spans, clipped,
    /// with plain text filling the gaps between them.
    pub fn pieces(&self, range: Range<usize>) -> Vec<PresentationSpan> {
        let mut out = Vec::new();
        let mut at = range.start;
        let first = self.spans.partition_point(|s| s.range.end <= range.start);
        for span in &self.spans[first..] {
            if span.range.start >= range.end {
                break;
            }
            let start = span.range.start.max(range.start);
            let end = span.range.end.min(range.end);
            if start >= end {
                continue;
            }
            if at < start {
                out.push(PresentationSpan::text(at..start, SpanStyle::default()));
            }
            // A replacement cut by a row boundary draws once, where it starts.
            let kind = match &span.kind {
                SpanKind::Replace(_) if start != span.range.start => SpanKind::Hidden,
                kind => kind.clone(),
            };
            out.push(PresentationSpan {
                range: start..end,
                kind,
                style: span.style.clone(),
            });
            at = end;
        }
        if at < range.end {
            out.push(PresentationSpan::text(at..range.end, SpanStyle::default()));
        }
        out
    }

    /// What `range` of `line` shapes as: its visible text and the fonts it uses.
    /// Used to measure wrap widths with the same glyphs painting will use.
    pub fn visible_text(&self, line: &str, range: Range<usize>, font: &Font) -> (String, Vec<TextRun>) {
        let mut text = String::with_capacity(range.len());
        let mut runs: Vec<TextRun> = Vec::new();
        for piece in self.pieces(range) {
            let part: &str = match &piece.kind {
                SpanKind::Hidden => continue,
                SpanKind::Replace(with) => with.as_ref(),
                SpanKind::Text => match line.get(piece.range.clone()) {
                    Some(part) => part,
                    None => continue,
                },
            };
            if part.is_empty() {
                continue;
            }
            text.push_str(part);
            let font = if piece.style.is_plain() {
                font.clone()
            } else {
                piece.style.font(font)
            };
            match runs.last_mut() {
                Some(last) if last.font == font => last.len += part.len(),
                _ => runs.push(TextRun {
                    len: part.len(),
                    font,
                    color: gpui::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }),
            }
        }
        (text, runs)
    }
}

/// The buffer rows an edit replaced, in the old and the new text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresenterEdit {
    pub start_row: usize,
    /// Last replaced row of the old text (inclusive).
    pub old_end_row: usize,
    /// Last row of the replacement in the new text (inclusive).
    pub new_end_row: usize,
}

/// Decides how each line of an editor is presented.
pub trait LinePresenter {
    /// The text changed (or is seen for the first time). Returns the rows
    /// outside the edited ones whose presentation changed with it, in the new
    /// text's numbering (an opened code fence restyles everything below it).
    fn sync(&mut self, text: &Rope, edit: Option<PresenterEdit>) -> Vec<usize>;

    /// How `line` (buffer row `row`, without its newline) is presented.
    /// `reveal` is the selection inside this line, when the caret is on it:
    /// markers the selection touches are shown as source. `None` for a plain line.
    fn line(&self, row: usize, line: &str, reveal: Option<Range<usize>>) -> Option<LinePresentation>;
}

/// Lay `spans` over `runs`, which cover `part` (line-local bytes) of a line.
pub(crate) fn apply_spans_to_runs(
    runs: Vec<TextRun>,
    part: &Range<usize>,
    pieces: &[PresentationSpan],
) -> Vec<TextRun> {
    if pieces.iter().all(|piece| piece.style.is_plain()) {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + pieces.len());
    let mut at = part.start;
    for run in runs {
        let run_end = at + run.len;
        let mut cursor = at;
        while cursor < run_end {
            let piece = pieces
                .iter()
                .find(|piece| piece.range.start <= cursor && cursor < piece.range.end);
            let end = match piece {
                Some(piece) => piece.range.end.min(run_end),
                None => pieces
                    .iter()
                    .map(|piece| piece.range.start)
                    .filter(|&start| start > cursor)
                    .min()
                    .unwrap_or(run_end)
                    .min(run_end),
            };
            let mut next = TextRun {
                len: end - cursor,
                ..run.clone()
            };
            if let Some(piece) = piece {
                piece.style.apply(&mut next);
            }
            out.push(next);
            cursor = end;
        }
        at = run_end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Font {
        gpui::font("Test")
    }

    #[test]
    fn pieces_fill_gaps_and_clip_to_the_row() {
        let presentation = LinePresentation {
            spans: vec![
                PresentationSpan::hidden(0..2),
                PresentationSpan::text(
                    4..8,
                    SpanStyle {
                        bold: true,
                        ..Default::default()
                    },
                ),
            ],
            ..Default::default()
        };
        let pieces = presentation.pieces(1..6);
        let ranges: Vec<_> = pieces.iter().map(|p| p.range.clone()).collect();
        assert_eq!(ranges, vec![1..2, 2..4, 4..6]);
        assert_eq!(pieces[0].kind, SpanKind::Hidden);
        assert_eq!(pieces[1].kind, SpanKind::Text);
        assert!(pieces[2].style.bold);
    }

    #[test]
    fn visible_text_drops_hidden_bytes_and_swaps_replacements() {
        let presentation = LinePresentation {
            spans: vec![
                PresentationSpan::replace(0..1, "•", SpanStyle::default()),
                PresentationSpan::hidden(2..4),
                PresentationSpan::text(
                    4..8,
                    SpanStyle {
                        bold: true,
                        ..Default::default()
                    },
                ),
            ],
            ..Default::default()
        };
        let (text, runs) = presentation.visible_text("- **bold** x", 0..12, &font());
        assert_eq!(text, "• bold** x");
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), text.len());
        assert!(runs.iter().any(|r| r.font.weight == FontWeight::BOLD));
    }

    #[test]
    fn spans_restyle_only_their_part_of_a_run() {
        let run = TextRun {
            len: 10,
            font: font(),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let pieces = vec![PresentationSpan::text(
            3..6,
            SpanStyle {
                italic: true,
                ..Default::default()
            },
        )];
        let runs = apply_spans_to_runs(vec![run], &(0..10), &pieces);
        assert_eq!(runs.iter().map(|r| r.len).collect::<Vec<_>>(), vec![3, 3, 4]);
        assert_eq!(runs[1].font.style, FontStyle::Italic);
        assert_eq!(runs[0].font.style, FontStyle::Normal);
    }
}
