// HERDR: built-in pure-Rust syntax highlighting for script editors, adapted from Base's showcase.
use std::{collections::HashMap, ops::Range, sync::LazyLock};

use gpui::{Context, HighlightStyle, SharedString, Window};
use gpui_base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, Rope,
};
use syntect::{
    parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

/// Loading the default syntax definitions deserializes a few megabytes, so share
/// one set across every highlighter instance.
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// A small, WASM-compatible example adapter for Base's parser-independent
/// highlighting API. Syntect identifies scopes; the application-owned resolver
/// supplies all colors and font styles.
pub(crate) struct SyntectHighlighter {
    language: SharedString,
    /// Non-overlapping highlights, ordered by start offset.
    highlights: Vec<(Range<usize>, &'static str)>,
    fold_ranges: Vec<FoldRange>,
    /// Scope ids are cheap to compare but expensive to stringify, so remember
    /// the semantic name each one maps to.
    semantic_names: HashMap<Scope, Option<&'static str>>,
}

impl SyntectHighlighter {
    pub(crate) fn new(language: &str) -> Option<Self> {
        find_syntax(language)?;

        Some(Self {
            language: language.to_owned().into(),
            highlights: Vec::new(),
            fold_ranges: Vec::new(),
            semantic_names: HashMap::new(),
        })
    }

    fn push_highlight(&mut self, range: Range<usize>, scopes: &ScopeStack) {
        if range.is_empty() {
            return;
        }

        let name = scopes.scopes.iter().rev().find_map(|scope| {
            *self
                .semantic_names
                .entry(*scope)
                .or_insert_with(|| semantic_name(*scope))
        });
        if let Some(name) = name {
            self.highlights.push((range, name));
        }
    }
}

fn find_syntax(language: &str) -> Option<&'static SyntaxReference> {
    SYNTAX_SET
        .find_syntax_by_token(language)
        .or_else(|| SYNTAX_SET.find_syntax_by_extension(language))
}

impl InputHighlighter for SyntectHighlighter {
    fn language(&self) -> SharedString {
        self.language.clone()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
        // `syntect` has no incremental mode, so the whole document is reparsed.
        // Read the rope once and reuse that string for folding too.
        let text = text.to_string();
        let syntax = find_syntax(self.language.as_ref())
            .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
        let mut parser = ParseState::new(syntax);
        let mut scopes = ScopeStack::new();
        let mut offset = 0;
        self.highlights.clear();

        for line in LinesWithEndings::from(&text) {
            if let Ok(operations) = parser.parse_line(line, &SYNTAX_SET) {
                let mut cursor = 0;
                for (index, operation) in operations {
                    self.push_highlight(offset + cursor..offset + index, &scopes);
                    let _ = scopes.apply(&operation);
                    cursor = index;
                }
                self.push_highlight(offset + cursor..offset + line.len(), &scopes);
            }
            offset += line.len();
        }

        self.fold_ranges = if folding {
            brace_fold_ranges(&text)
        } else {
            Vec::new()
        };
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        resolve_styles(&self.highlights, range, resolver)
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        self.fold_ranges.clone()
    }

    fn fold_ranges_for_edit(&self, _: Range<usize>, _: &Rope) -> Vec<FoldRange> {
        self.fold_ranges.clone()
    }
}

/// Turn the highlights overlapping `range` into gap-free style runs.
///
/// `highlights` is ordered and non-overlapping, so the first candidate is found
/// by binary search instead of scanning the whole document on every frame.
fn resolve_styles(
    highlights: &[(Range<usize>, &'static str)],
    range: &Range<usize>,
    resolver: &dyn HighlightStyleResolver,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let first = highlights.partition_point(|(highlight, _)| highlight.end <= range.start);
    let mut runs = Vec::new();
    let mut cursor = range.start;

    for (highlight_range, name) in &highlights[first..] {
        if highlight_range.start >= range.end {
            break;
        }

        let start = highlight_range.start.max(range.start);
        let end = highlight_range.end.min(range.end);
        if start >= end || end <= cursor {
            continue;
        }
        if cursor < start {
            runs.push((cursor..start, HighlightStyle::default()));
        }
        runs.push((start..end, resolver.style(name).unwrap_or_default()));
        cursor = end;
    }

    if cursor < range.end {
        runs.push((cursor..range.end, HighlightStyle::default()));
    }
    runs
}

fn semantic_name(scope: Scope) -> Option<&'static str> {
    let scope = scope.build_string();
    // HERDR: Markdown (and other markup) scopes, so prose editors can style
    // headings, emphasis, links and code. Their delimiters keep `punctuation`.
    let name = if scope.starts_with("markup.heading") {
        "heading"
    } else if scope.starts_with("markup.bold") {
        "bold"
    } else if scope.starts_with("markup.italic") {
        "italic"
    } else if scope.starts_with("markup.strikethrough") {
        "strikethrough"
    } else if scope.starts_with("string.other.link")
        || scope.starts_with("constant.other.reference.link")
        || (scope.starts_with("meta.link") && scope.contains(".description"))
    {
        "link"
    } else if scope.starts_with("markup.underline.link") {
        "link.url"
    } else if scope.starts_with("markup.raw") {
        "code"
    } else if scope.starts_with("markup.quote") {
        "quote"
    } else if scope.starts_with("comment") {
        "comment"
    } else if scope.starts_with("constant.character.escape") {
        "string.escape"
    } else if scope.starts_with("string") {
        "string"
    } else if scope.starts_with("constant.numeric") {
        "number"
    } else if scope.starts_with("constant.language.boolean") {
        "boolean"
    } else if scope.starts_with("keyword.operator") {
        "operator"
    } else if scope.starts_with("keyword") || scope.starts_with("storage") {
        "keyword"
    } else if scope.starts_with("entity.name.function") || scope.starts_with("support.function") {
        "function"
    } else if scope.starts_with("entity.name.type")
        || scope.starts_with("entity.name.class")
        || scope.starts_with("support.type")
    {
        "type"
    } else if scope.starts_with("variable") {
        "variable"
    } else if scope.starts_with("constant") {
        "constant"
    } else if scope.starts_with("punctuation") {
        "punctuation"
    } else {
        return None;
    };
    Some(name)
}

fn brace_fold_ranges(text: &str) -> Vec<FoldRange> {
    let mut starts = Vec::new();
    let mut ranges = Vec::new();

    for (line_number, line) in text.lines().enumerate() {
        let mut chars = line.chars().peekable();
        let mut quoted = false;
        let mut escaped = false;
        while let Some(character) = chars.next() {
            if !quoted && character == '/' && chars.peek() == Some(&'/') {
                break;
            }
            if character == '"' && !escaped {
                quoted = !quoted;
            } else if !quoted && character == '{' {
                starts.push(line_number);
            } else if !quoted && character == '}' {
                if let Some(start_line) = starts.pop() {
                    if start_line < line_number {
                        ranges.push(FoldRange::new(start_line, line_number));
                    }
                }
            }
            escaped = quoted && character == '\\' && !escaped;
            if character != '\\' {
                escaped = false;
            }
        }
    }
    ranges
}

/// HERDR: semantic overrides fall back to readable, dark-editor defaults.
/// Markup names also carry a font treatment (bold headings, italic emphasis,
/// underlined links) that applies whatever their color.
#[derive(Default)]
pub(crate) struct SyntaxStyles(pub std::collections::HashMap<String, gpui::Hsla>);

impl HighlightStyleResolver for SyntaxStyles {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        let root = name.split('.').next()?;
        let mut style = markup_style(name, root);
        let color = self
            .0
            .get(name)
            .or_else(|| self.0.get(root))
            .copied()
            .or_else(|| {
                let rgb = match root {
                    "comment" => 0x7f848e,
                    "string" => 0x98c379,
                    "number" => 0xd19a66,
                    "keyword" => 0xc678dd,
                    "boolean" | "constant" => 0xe5c07b,
                    "function" => 0x61afef,
                    "type" => 0xe5c07b,
                    "operator" => 0x56b6c2,
                    "link" => 0x61afef,
                    "code" => 0x98c379,
                    "quote" => 0x7f848e,
                    // Unclassified identifiers and punctuation follow the theme foreground.
                    _ => return None,
                };
                Some(gpui::rgb(rgb).into())
            });
        style.color = color;
        (style != HighlightStyle::default()).then_some(style)
    }
}

/// HERDR: the font part of a markup name's style; colors come from the resolver.
fn markup_style(name: &str, root: &str) -> HighlightStyle {
    let line = || gpui::px(1.);
    match root {
        "heading" | "bold" => HighlightStyle {
            font_weight: Some(gpui::FontWeight::BOLD),
            ..Default::default()
        },
        "italic" | "quote" => HighlightStyle {
            font_style: Some(gpui::FontStyle::Italic),
            ..Default::default()
        },
        "strikethrough" => HighlightStyle {
            strikethrough: Some(gpui::StrikethroughStyle {
                thickness: line(),
                color: None,
            }),
            ..Default::default()
        },
        "link" if name == "link.url" => HighlightStyle {
            underline: Some(gpui::UnderlineStyle {
                thickness: line(),
                color: None,
                wavy: false,
            }),
            ..Default::default()
        },
        _ => HighlightStyle::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_styles_cover_the_requested_range_without_gaps() {
        let runs = resolve_styles(
            &[(2..4, "keyword"), (6..8, "string")],
            &(0..10),
            &SyntaxStyles::default(),
        );
        let ranges: Vec<_> = runs.into_iter().map(|(range, _)| range).collect();
        assert_eq!(ranges, vec![0..2, 2..4, 4..6, 6..8, 8..10]);
    }

    #[test]
    fn semantic_overrides_and_nested_names_resolve() {
        let color = gpui::rgb(0xc678dd).into();
        let styles = SyntaxStyles(HashMap::from([("string".into(), color)]));
        assert_eq!(styles.style("string.escape").unwrap().color, Some(color));
        assert!(styles.style("keyword").is_some());
        assert!(find_syntax("rust").is_some());
        assert!(find_syntax("rs").is_some());
        assert!(find_syntax("unknown-language").is_none());
    }

    /// The semantic name of each highlighted span of `text`, in order.
    fn markdown_names(text: &str) -> Vec<(String, &'static str)> {
        let mut highlighter = SyntectHighlighter::new("markdown").expect("markdown syntax");
        let syntax = find_syntax("markdown").unwrap();
        let mut parser = ParseState::new(syntax);
        let mut scopes = ScopeStack::new();
        let mut offset = 0;
        for line in LinesWithEndings::from(text) {
            let operations = parser.parse_line(line, &SYNTAX_SET).unwrap();
            let mut cursor = 0;
            for (index, operation) in operations {
                highlighter.push_highlight(offset + cursor..offset + index, &scopes);
                scopes.apply(&operation).unwrap();
                cursor = index;
            }
            highlighter.push_highlight(offset + cursor..offset + line.len(), &scopes);
            offset += line.len();
        }
        highlighter
            .highlights
            .iter()
            .map(|(range, name)| (text[range.clone()].to_string(), *name))
            .collect()
    }

    #[test]
    fn markdown_prose_gets_markup_names() {
        let names = markdown_names(
            "# Title\n\nSome **bold**, *italic*, ~~gone~~ and `code`, [a link](https://x.dev).\n\n> quoted\n",
        );
        let name_of = |text: &str| {
            names
                .iter()
                .find(|(span, _)| span == text)
                .map(|(_, name)| *name)
        };
        assert_eq!(name_of("Title"), Some("heading"));
        assert_eq!(name_of("#"), Some("punctuation"));
        assert_eq!(name_of("bold"), Some("bold"));
        assert_eq!(name_of("**"), Some("punctuation"));
        assert_eq!(name_of("italic"), Some("italic"));
        assert_eq!(name_of("code"), Some("code"));
        assert_eq!(name_of("a link"), Some("link"), "{names:?}");
        assert_eq!(name_of("https://x.dev"), Some("link.url"));
        assert!(
            names
                .iter()
                .any(|(span, name)| span.contains("quoted") && *name == "quote")
        );
    }

    #[test]
    fn markup_names_carry_font_styles_and_take_colors() {
        let color = gpui::rgb(0xe06c75).into();
        let styles = SyntaxStyles(HashMap::from([("heading".into(), color)]));
        let heading = styles.style("heading").unwrap();
        assert_eq!(heading.color, Some(color));
        assert_eq!(heading.font_weight, Some(gpui::FontWeight::BOLD));
        let bold = styles.style("bold").unwrap();
        assert_eq!(bold.color, None);
        assert_eq!(bold.font_weight, Some(gpui::FontWeight::BOLD));
        assert_eq!(
            styles.style("italic").unwrap().font_style,
            Some(gpui::FontStyle::Italic)
        );
        assert!(
            styles
                .style("strikethrough")
                .unwrap()
                .strikethrough
                .is_some()
        );
        assert!(styles.style("link.url").unwrap().underline.is_some());
        assert!(styles.style("link").unwrap().underline.is_none());
    }
}
