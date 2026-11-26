use anyhow::Result;
use gpui::{App, Context, Hsla, SharedString, Task, Window};
use ropey::Rope;
use std::rc::Rc;

use crate::input::{EditorMode, InputBaseState, RopeExt};

mod code_actions;
mod completions;
mod definitions;
mod document_colors;
mod hover;
mod overlay;
mod semantic_tokens;

pub use code_actions::*;
pub use completions::*;
pub use definitions::*;
pub use document_colors::*;
pub use hover::*;
pub use overlay::*;
pub use semantic_tokens::*;

/// Host hook to show a document when following an LSP location
/// (Go to Definition), modeled after the `window/showDocument` request.
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#window_showDocument
///
/// Called before the built-in behavior. Return `true` if the host has shown
/// the document (e.g. opened a docs window for a virtual/external URI);
/// return `false` to fall through to the default handling (`external` URIs
/// open in the browser, anything else jumps within the current document).
pub type ShowDocumentHandler =
    Rc<dyn Fn(&lsp_types::ShowDocumentParams, &mut Window, &mut App) -> bool>;

/// LSP ServerCapabilities
///
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#serverCapabilities
pub struct Lsp {
    /// The completion provider.
    pub completion_provider: Option<Rc<dyn CompletionProvider>>,
    /// The code action providers.
    pub code_action_providers: Vec<Rc<dyn CodeActionProvider>>,
    /// The hover provider.
    pub hover_provider: Option<Rc<dyn HoverProvider>>,
    /// The definition provider.
    pub definition_provider: Option<Rc<dyn DefinitionProvider>>,
    /// The document color provider.
    pub document_color_provider: Option<Rc<dyn DocumentColorProvider>>,
    /// The range semantic tokens provider.
    pub semantic_tokens_provider: Option<Rc<dyn DocumentRangeSemanticTokensProvider>>,
    /// Optional host hook to show documents for Go to Definition locations,
    /// following the `window/showDocument` request (see [`ShowDocumentHandler`]).
    ///
    /// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#window_showDocument
    pub show_document: Option<ShowDocumentHandler>,

    /// Display options for the completion popover.
    pub completion_menu: CompletionMenuOptions,

    pub(crate) document_colors: Vec<(lsp_types::Range, Hsla)>,
    /// Cached semantic tokens as absolute position ranges + theme token-type
    /// names. Color is resolved from the name at paint time so theme switches
    /// take effect without a refetch.
    pub(crate) semantic_tokens: Vec<(lsp_types::Range, SharedString)>,
    pub(crate) _hover_task: Task<Result<()>>,
    pub(crate) _definition_task: Task<Result<()>>,
    pub(crate) _document_color_task: Task<()>,
    pub(crate) _semantic_tokens_task: Task<()>,
}

impl Default for Lsp {
    fn default() -> Self {
        Self {
            completion_provider: None,
            code_action_providers: vec![],
            hover_provider: None,
            definition_provider: None,
            document_color_provider: None,
            completion_menu: CompletionMenuOptions::default(),
            semantic_tokens_provider: None,
            show_document: None,
            document_colors: vec![],
            semantic_tokens: vec![],
            _hover_task: Task::ready(Ok(())),
            _definition_task: Task::ready(Ok(())),
            _document_color_task: Task::ready(()),
            _semantic_tokens_task: Task::ready(()),
        }
    }
}

impl Lsp {
    /// Update the LSP when the text changes.
    pub(crate) fn update(
        &mut self,
        text: &Rope,
        window: &mut Window,
        cx: &mut Context<InputBaseState<EditorMode>>,
    ) {
        self.update_document_colors(text, window, cx);
        self.update_semantic_tokens(text, window, cx);
    }

    /// Replace the cached semantic tokens, for a host that pushes them rather
    /// than installing a provider.
    pub fn set_semantic_tokens(&mut self, mut tokens: Vec<(lsp_types::Range, SharedString)>) {
        // HERDR: painting binary-searches this list by start position.
        tokens.sort_by_key(|(range, _)| range.start);
        self.semantic_tokens = tokens;
    }

    /// Reset all LSP states.
    pub(crate) fn reset(&mut self) {
        self.document_colors.clear();
        self.semantic_tokens.clear();
        self._hover_task = Task::ready(Ok(()));
        self._definition_task = Task::ready(Ok(()));
        self._document_color_task = Task::ready(());
        self._semantic_tokens_task = Task::ready(());
    }
}

/// LSP forbids a batch of text edits whose ranges overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlappingEdits;

impl std::fmt::Display for OverlappingEdits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the edits overlap")
    }
}

impl std::error::Error for OverlappingEdits {}

impl InputBaseState<EditorMode> {
    /// Apply [`lsp_types::TextEdit`]s as one undo step.
    ///
    /// Every range is read against the document before any of the edits, as
    /// LSP defines them, and the caret stays on the text it was on. Changes
    /// nothing if the ranges overlap, which LSP forbids.
    pub fn apply_lsp_edits(
        &mut self,
        text_edits: &[lsp_types::TextEdit],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), OverlappingEdits> {
        if text_edits.is_empty() || !self.is_editable() {
            return Ok(());
        }
        let mut edits: Vec<(std::ops::Range<usize>, String)> = text_edits
            .iter()
            .map(|edit| {
                let start = self.text.position_to_offset(&edit.range.start);
                let end = self.text.position_to_offset(&edit.range.end);
                (start.min(end)..start.max(end), edit.new_text.clone())
            })
            .collect();
        // Stable, so edits at one position keep the order LSP gives them in.
        edits.sort_by_key(|(range, _)| range.start);
        if edits.windows(2).any(|pair| pair[0].0.end > pair[1].0.start) {
            return Err(OverlappingEdits);
        }
        // HERDR: edits at one position become one, their texts in LSP's order,
        // so no edit relies on how the batch orders equal starts.
        let mut merged: Vec<(std::ops::Range<usize>, String)> = Vec::with_capacity(edits.len());
        for (range, text) in edits {
            match merged.last_mut() {
                Some((last, last_text)) if last.start == range.start => {
                    last.end = last.end.max(range.end);
                    last_text.push_str(&text);
                }
                _ => merged.push((range, text)),
            }
        }

        let cursor = self.cursor();
        let caret = merged.iter().fold(cursor as isize, |caret, (range, text)| {
            if range.end <= cursor {
                caret + text.len() as isize - range.len() as isize
            } else if range.start < cursor {
                caret + (range.start + text.len()) as isize - cursor as isize
            } else {
                caret
            }
        }) as usize;
        self.replace_text_in_ranges(&merged, window, cx);
        self.set_selected_range(caret..caret, cx);
        Ok(())
    }
}
