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
    syntax: &'static SyntaxReference,
    /// Non-overlapping highlights, ordered by start offset.
    highlights: Vec<(Range<usize>, &'static str)>,
    fold_ranges: Vec<FoldRange>,
    /// Scope ids are cheap to compare but expensive to stringify, so remember
    /// the semantic name each one maps to.
    semantic_names: HashMap<Scope, Option<&'static str>>,
}

impl SyntectHighlighter {
    pub(crate) fn new(language: &str) -> Option<Self> {
        let syntax = find_syntax(language)?;

        Some(Self {
            language: language.to_owned().into(),
            syntax,
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
                .or_insert_with(|| semantic_name(*scope, self.syntax.name == "Markdown"))
        });
        if let Some(name) = name {
            self.highlights.push((range, name));
        }
    }

    fn reparse(&mut self, text: &str) {
        let mut parser = ParseState::new(self.syntax);
        let mut scopes = ScopeStack::new();
        let mut offset = 0;
        self.highlights.clear();

        for line in LinesWithEndings::from(text) {
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
        self.reparse(&text);
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

/// A language with no grammar still has diagnostics and semantic tokens, and the
/// editor composes those only on top of a highlighter.
pub(crate) struct PlainHighlighter(pub(crate) SharedString);

impl InputHighlighter for PlainHighlighter {
    fn language(&self) -> SharedString {
        self.0.clone()
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        _text: &Rope,
        _folding: bool,
        _window: &mut Window,
        _cx: &mut Context<EditorState>,
    ) {
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        vec![(range.clone(), HighlightStyle::default())]
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
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

fn semantic_name(scope: Scope, markdown: bool) -> Option<&'static str> {
    let scope = scope.build_string();
    // Other grammars, including reStructuredText, use the same markup prefixes.
    if markdown && let Some(name) = markup_name(&scope) {
        return Some(name);
    }
    let name = if scope.starts_with("comment") {
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

/// HERDR: Markdown's scopes, so a Markdown editor shows its structure. A marker
/// (`#`, `**`, a list bullet) takes its construct's name rather than
/// `punctuation`, the way VS Code colors it. Lists and links name only their
/// marker and URL: the list item's own text stays plain.
fn markup_name(scope: &str) -> Option<&'static str> {
    const NAMES: &[(&[&str], &str)] = &[
        (
            &["markup.heading", "punctuation.definition.heading"],
            "heading",
        ),
        (&["markup.bold", "punctuation.definition.bold"], "strong"),
        (
            &["markup.italic", "punctuation.definition.italic"],
            "emphasis",
        ),
        (&["markup.raw", "punctuation.definition.raw"], "code"),
        (
            &["markup.quote", "punctuation.definition.blockquote"],
            "quote",
        ),
        (
            &[
                "markup.list.numbered.bullet",
                "markup.list.unnumbered.bullet",
                "punctuation.definition.list_item",
            ],
            "list",
        ),
        (&["markup.underline.link"], "link"),
    ];
    NAMES
        .iter()
        .find(|(prefixes, _)| prefixes.iter().any(|prefix| scope.starts_with(prefix)))
        .map(|(_, name)| *name)
}

/// HERDR: a Markdown name a mod gave no color takes the color of a code name,
/// so a mod that themes `keyword`, `string` and `comment` themes Markdown too.
fn markup_fallback(root: &str) -> Option<&'static str> {
    Some(match root {
        "heading" | "strong" | "list" => "keyword",
        "code" => "string",
        "quote" => "comment",
        "link" => "function",
        _ => return None,
    })
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
#[derive(Default)]
pub(crate) struct SyntaxStyles(pub std::collections::HashMap<String, gpui::Hsla>);

impl HighlightStyleResolver for SyntaxStyles {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        let root = name.split('.').next()?;
        let fallback = markup_fallback(root);
        let color = self
            .0
            .get(name)
            .or_else(|| self.0.get(root))
            .or_else(|| fallback.and_then(|fallback| self.0.get(fallback)))
            .copied()
            .or_else(|| {
                let rgb = match fallback.unwrap_or(root) {
                    "comment" => 0x7f848e,
                    "string" => 0x98c379,
                    "number" => 0xd19a66,
                    "keyword" => 0xc678dd,
                    "boolean" | "constant" => 0xe5c07b,
                    "function" => 0x61afef,
                    "type" => 0xe5c07b,
                    "operator" => 0x56b6c2,
                    // Unclassified identifiers and punctuation follow the theme foreground.
                    _ => return None,
                };
                Some(gpui::rgb(rgb).into())
            });
        let font_weight = matches!(root, "heading" | "strong").then_some(gpui::FontWeight::BOLD);
        let font_style = (root == "emphasis").then_some(gpui::FontStyle::Italic);
        (color.is_some() || font_weight.is_some() || font_style.is_some()).then(|| HighlightStyle {
            color,
            font_weight,
            font_style,
            ..Default::default()
        })
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
    fn a_language_without_a_grammar_still_gets_a_highlighter() {
        assert!(SyntectHighlighter::new("kotlin").is_none());
        let plain = PlainHighlighter("kotlin".into());
        assert_eq!(
            plain.styles(&(3..9), &SyntaxStyles::default()),
            vec![(3..9, HighlightStyle::default())],
            "the editor needs runs that cover the range to compose diagnostics over"
        );
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

    fn highlight_names<'a>(language: &str, text: &'a str) -> Vec<(&'a str, &'static str)> {
        let mut highlighter = SyntectHighlighter::new(language).unwrap();
        highlighter.reparse(text);
        highlighter
            .highlights
            .into_iter()
            .map(|(range, name)| (text[range].trim(), name))
            .filter(|(text, _)| !text.is_empty())
            .collect()
    }

    #[test]
    fn markdown_shows_its_structure() {
        let text =
            "# Title\n\n- item with `code` and **bold** and *it*\n\n> quote\n\n[x](http://x)\n";
        let names = highlight_names("markdown", text);
        assert_eq!(highlight_names("md", text), names);
        for expected in [
            ("#", "heading"),
            ("Title", "heading"),
            ("-", "list"),
            ("`", "code"),
            ("code", "code"),
            ("**", "strong"),
            ("bold", "strong"),
            ("it", "emphasis"),
            ("quote", "quote"),
            ("http://x", "link"),
        ] {
            assert!(names.contains(&expected), "{expected:?} not in {names:?}");
        }
        assert!(!names.iter().any(|(text, _)| text.contains("item with")));
    }

    #[test]
    fn restructuredtext_keeps_its_existing_syntax() {
        let names = highlight_names("rst", "**strong** and *emphasis* and ``literal``.\n");
        assert!(names.contains(&("**", "punctuation")), "{names:?}");
        assert!(
            !names
                .iter()
                .any(|(text, _)| matches!(*text, "strong" | "emphasis" | "literal")),
            "{names:?}"
        );
    }

    #[test]
    fn markdown_names_follow_code_colors_and_set_weight() {
        let keyword = gpui::rgb(0x112233).into();
        let heading = gpui::rgb(0x445566).into();
        let styles = SyntaxStyles(HashMap::from([("keyword".into(), keyword)]));
        let style = styles.style("heading").unwrap();
        assert_eq!(style.color, Some(keyword));
        assert_eq!(style.font_weight, Some(gpui::FontWeight::BOLD));
        assert_eq!(
            styles.style("emphasis").unwrap().font_style,
            Some(gpui::FontStyle::Italic)
        );
        assert!(styles.style("code").unwrap().color.is_some());

        let styles = SyntaxStyles(HashMap::from([
            ("keyword".into(), keyword),
            ("heading".into(), heading),
        ]));
        assert_eq!(styles.style("heading").unwrap().color, Some(heading));
    }
}
