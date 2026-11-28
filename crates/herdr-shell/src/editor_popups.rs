// HERDR: gpui-base's editor keeps its hover, completion, code-action and
// diagnostic state but draws none of it; this view does, so a mod that answers
// LSP requests gets the popups without drawing anything itself.
use std::rc::{Rc, Weak};

use gpui::{
    Action, Anchor, AnyElement, Context, Entity, Hsla, InteractiveElement as _, IntoElement as _,
    MouseButton, ParentElement as _, Pixels, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle,
    WeakEntity, Window, anchored, deferred, div, px, uniform_list,
};
use gpui_base::input::lsp_types::{
    CompletionItem, Documentation, Hover, HoverContents, MarkedString,
};
use gpui_base::{
    SemanticThemeTokens, Theme,
    input::{
        CodeActionItem, CompletionInsert, DiagnosticSeverity, EditorState, Enter, Escape,
        IndentInline, InputOverlayKind, MoveDown, MoveUp,
    },
    text::TextView,
};

use crate::{
    editor_language::{ScriptHandler, ask},
    value::scaled_px,
};

const ROW_PADDING: f32 = 6.;
const MENU_WIDTH: f32 = 420.;
const MENU_ROWS: usize = 10;
const CARD_WIDTH: f32 = 520.;
const CARD_HEIGHT: f32 = 300.;
const WINDOW_MARGIN: f32 = 8.;
const PRIORITY: usize = 1;

pub(crate) struct EditorPopups {
    state: Entity<EditorState>,
    completions: Rc<Vec<CompletionItem>>,
    actions: Rc<Vec<CodeActionItem>>,
    /// What `completions` and `actions` were last filtered from: both menus' revisions and the word at the caret.
    menu_key: (u64, u64, String),
    caret: (usize, std::ops::Range<usize>),
    selected: usize,
    /// The completion picked with the arrows, which stays selected through new
    /// answers and filtering while it's still listed. None follows the best match.
    chosen: Option<ItemKey>,
    list: UniformListScrollHandle,
    completion_accepted: Option<Weak<ScriptHandler>>,
    drawn_at: Option<gpui::Point<Pixels>>,
    _state_changed: Subscription,
}

impl EditorPopups {
    pub(crate) fn new(state: Entity<EditorState>, cx: &mut Context<Self>) -> Self {
        let popups = cx.weak_entity();
        state.update(cx, |state, _| {
            state.set_overlay_action_handler(move |kind, action, window, cx| {
                popups
                    .update(cx, |popups, cx| popups.on_action(kind, action, window, cx))
                    .unwrap_or(false)
            });
        });
        let caret = {
            let state = state.read(cx);
            (state.cursor(), state.selected_range())
        };
        let state_changed = cx.observe(&state, |this, _, cx| {
            this.sync(cx);
            cx.notify();
        });
        Self {
            state,
            completions: Rc::default(),
            actions: Rc::default(),
            menu_key: (0, 0, String::new()),
            caret,
            selected: 0,
            chosen: None,
            list: UniformListScrollHandle::new(),
            completion_accepted: None,
            drawn_at: None,
            _state_changed: state_changed,
        }
    }

    pub(crate) fn set_completion_accepted(&mut self, handler: Option<Weak<ScriptHandler>>) {
        self.completion_accepted = handler;
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let completion = state.completion_menu_state();
        let code_action = state.code_action_menu_state();
        let word = state
            .text()
            .slice(state.completion_word_range().start..state.cursor())
            .to_string();
        if !completion.open {
            self.chosen = None;
        }
        let key = (completion.revision(), code_action.revision(), word);
        if key != self.menu_key {
            self.completions = Rc::new(visible_completions(&completion.items, &key.2));
            self.actions = Rc::new(code_action.items.clone());
            self.menu_key = key;
            let chosen = if completion.open {
                self.chosen.as_ref().and_then(|chosen| {
                    self.completions
                        .iter()
                        .position(|item| ItemKey::of(item) == *chosen)
                })
            } else {
                None
            };
            if chosen.is_none() {
                self.chosen = None;
            }
            self.selected = chosen.unwrap_or(0);
            self.list
                .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        }
        let shown = if completion.open {
            self.completions.len()
        } else {
            self.actions.len()
        };
        self.selected = self.selected.min(shown.saturating_sub(1));

        let caret = (state.cursor(), state.selected_range());
        let showing_info = state.hover_popover().is_some() || state.diagnostic_popover().is_some();
        if caret != self.caret {
            self.caret = caret;
            // These answer for where the pointer was on the old text.
            if showing_info {
                self.state.update(cx, |state, cx| {
                    state.clear_hover_state(cx);
                    state.clear_diagnostic_popover(cx);
                });
            }
        }
    }

    fn on_action(
        &mut self,
        kind: InputOverlayKind,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let len = match kind {
            InputOverlayKind::Completion => self.completions.len(),
            InputOverlayKind::CodeAction => self.actions.len(),
        };
        if len == 0 {
            return false;
        }
        if action.partial_eq(&MoveUp) {
            self.select(kind, (self.selected + len - 1) % len, cx);
        } else if action.partial_eq(&MoveDown) {
            self.select(kind, (self.selected + 1) % len, cx);
        } else if Enter::is_primary(&*action) {
            self.accept(kind, self.selected, CompletionInsert::Insert, window, cx);
        } else if action.partial_eq(&IndentInline) {
            self.accept(kind, self.selected, CompletionInsert::Replace, window, cx);
        } else if !action.partial_eq(&Escape) {
            return false;
        }
        true
    }

    fn select(&mut self, kind: InputOverlayKind, index: usize, cx: &mut Context<Self>) {
        if kind == InputOverlayKind::Completion {
            self.chosen = self.completions.get(index).map(ItemKey::of);
        }
        self.selected = index;
        self.list.scroll_to_item(index, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn accept(
        &mut self,
        kind: InputOverlayKind,
        index: usize,
        how: CompletionInsert,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        match kind {
            InputOverlayKind::Completion => {
                let Some(item) = self.completions.get(index).cloned() else {
                    return;
                };
                let accepted = self.completion_accepted.clone();
                // Accepting edits the editor, and a key got us here from inside
                // its update.
                window.defer(cx, move |window, cx| {
                    state.update(cx, |state, cx| {
                        state.dismiss_completion_overlay(cx);
                        insert(state, &item, how, window, cx);
                    });
                    if let Some(handler) = accepted {
                        ask::<serde_json::Value>(&handler, &item, window, cx).detach();
                    }
                });
            }
            InputOverlayKind::CodeAction => {
                let Some(item) = self.actions.get(index).cloned() else {
                    return;
                };
                window.defer(cx, move |window, cx| {
                    state.update(cx, |state, cx| {
                        state.dismiss_code_action_overlay(cx);
                        state.perform_code_action(&item, window, cx);
                    });
                });
            }
        }
    }

    fn menu(
        &self,
        kind: InputOverlayKind,
        rows: Rc<Vec<(SharedString, Option<SharedString>)>>,
        tokens: &SemanticThemeTokens,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = tokens.colors;
        let row_height = window.line_height() + scaled_px(ROW_PADDING);
        let selected = self.selected;
        let popups = cx.weak_entity();
        let count = rows.len();
        let list = uniform_list("editor-popup-menu", count, move |range, _, _| {
            range
                .map(|index| {
                    let (label, detail) = rows[index].clone();
                    let popups = popups.clone();
                    let active = index == selected;
                    div()
                        .id(index)
                        .h(row_height)
                        .w_full()
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .cursor_pointer()
                        .when_active(active, colors.accent, colors.accent_foreground)
                        .hover(move |style| style.bg(colors.accent))
                        .on_click(move |_, window, cx| {
                            popups
                                .update(cx, |popups, cx| {
                                    popups.accept(kind, index, CompletionInsert::Insert, window, cx)
                                })
                                .ok();
                        })
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .children(detail.map(|detail| {
                            div()
                                .flex_shrink_0()
                                .max_w(scaled_px(MENU_WIDTH / 2.))
                                .truncate()
                                .text_color(colors.muted_foreground)
                                .child(detail)
                        }))
                })
                .collect()
        })
        .track_scroll(&self.list)
        .h(row_height * count.min(MENU_ROWS) as f32)
        .w_full();
        let state = self.state.clone();
        popup_frame(&state, tokens)
            .on_mouse_down_out(move |_, _, cx| {
                state.update(cx, |state, cx| state.dismiss_lsp_overlays(cx));
            })
            .w(scaled_px(MENU_WIDTH))
            .child(list)
            .into_any_element()
    }

    fn info_card(
        &self,
        hover: Option<&Hover>,
        diagnostic: Option<(SharedString, Hsla)>,
        tokens: &SemanticThemeTokens,
    ) -> AnyElement {
        let markdown = hover.map(hover_markdown).filter(|text| !text.is_empty());
        popup_frame(&self.state, tokens)
            .id("editor-popup-info")
            .max_w(scaled_px(CARD_WIDTH))
            .max_h(scaled_px(CARD_HEIGHT))
            .overflow_y_scroll()
            .p(px(8.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(diagnostic.map(|(message, color)| {
                div()
                    .border_l_2()
                    .border_color(color)
                    .pl(px(6.))
                    .child(message)
            }))
            .children(markdown.map(|markdown| {
                TextView::markdown("editor-popup-hover", markdown).selectable(false)
            }))
            .into_any_element()
    }

    /// Popups draw from the editor's last layout, and the frame that moved it
    /// lays out after this view renders. Look once more when that frame is done.
    fn watch_anchor(
        &mut self,
        anchor: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drawn_at == Some(anchor) {
            return;
        }
        self.drawn_at = Some(anchor);
        let popups: WeakEntity<Self> = cx.weak_entity();
        window.on_next_frame(move |_, cx| {
            popups.update(cx, |_, cx| cx.notify()).ok();
        });
    }
}

trait WhenActive: Sized {
    fn when_active(self, active: bool, background: Hsla, foreground: Hsla) -> Self;
}

impl<T: gpui::Styled> WhenActive for T {
    fn when_active(self, active: bool, background: Hsla, foreground: Hsla) -> Self {
        if active {
            self.bg(background).text_color(foreground)
        } else {
            self
        }
    }
}

fn popup_frame(state: &Entity<EditorState>, tokens: &SemanticThemeTokens) -> gpui::Div {
    let state = state.clone();
    let mut frame = div()
        .occlude()
        .bg(tokens.colors.surface)
        .text_color(tokens.colors.surface_foreground)
        .border_1()
        .border_color(tokens.colors.border)
        .rounded(tokens.radius.md)
        .overflow_hidden()
        // A press on something unfocusable counts as a click away, which would
        // take the keyboard from the editor and close this popup under the pointer.
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            window.prevent_default();
            state.update(cx, |state, cx| state.focus(window, cx));
        });
    frame.style().box_shadow = Some(tokens.shadow.md.clone());
    frame
}

/// Which completion an entry is, across answers that rebuild the list.
#[derive(Clone, PartialEq, Eq)]
struct ItemKey {
    label: String,
    detail: Option<String>,
    sort_text: Option<String>,
}

impl ItemKey {
    fn of(item: &CompletionItem) -> Self {
        Self {
            label: item.label.clone(),
            detail: item.detail.clone(),
            sort_text: item.sort_text.clone(),
        }
    }
}

fn insert(
    state: &mut EditorState,
    item: &CompletionItem,
    how: CompletionInsert,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) {
    state.insert_completion(item, how, window, cx);
    // Edits outside the completion's own, an auto-import most often. They read
    // against the document from before the insert, which only moved what's
    // after it, and servers put them before it.
    if let Some(edits) = &item.additional_text_edits {
        state.apply_lsp_edits(edits, window, cx).ok();
    }
}

/// How well an item's filter text matches the typed word, best first.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Match {
    Exact,
    Prefix,
    PrefixIgnoringCase,
    /// Each typed run starts a word of the name, as `gBN` or `get_by` match
    /// `getByName`.
    WordStarts,
    Scattered,
}

// The server's sortText ranks items it considers equally good, so it orders
// items within a match quality rather than across them, as IntelliJ's lookup
// does; otherwise `fo` could list `afxoo` above `focus`.
fn visible_completions(items: &[CompletionItem], word: &str) -> Vec<CompletionItem> {
    let mut visible: Vec<(Match, &CompletionItem)> = items
        .iter()
        .filter_map(|item| {
            let text = item.filter_text.as_deref().unwrap_or(&item.label);
            Some((match_quality(text, word)?, item))
        })
        .collect();
    let key = |item: &CompletionItem| item.sort_text.clone().unwrap_or_else(|| item.label.clone());
    visible.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| key(a.1).cmp(&key(b.1))));
    visible.into_iter().map(|(_, item)| item.clone()).collect()
}

fn match_quality(text: &str, word: &str) -> Option<Match> {
    if text == word {
        return Some(Match::Exact);
    }
    if text.starts_with(word) {
        return Some(Match::Prefix);
    }
    let text: Vec<char> = text.chars().collect();
    let word: Vec<char> = word.chars().collect();
    let same = |a: char, b: char| a.to_lowercase().eq(b.to_lowercase());
    if word.len() <= text.len() && text.iter().zip(&word).all(|(&a, &b)| same(a, b)) {
        return Some(Match::PrefixIgnoringCase);
    }
    let starts: Vec<usize> = (0..text.len()).filter(|&i| starts_word(&text, i)).collect();
    if matches_word_starts(&text, &starts, &word, &same) {
        return Some(Match::WordStarts);
    }
    let mut wanted = word.iter().peekable();
    for &ch in &text {
        if wanted.peek().is_some_and(|&&w| same(ch, w)) {
            wanted.next();
        }
    }
    wanted.peek().is_none().then_some(Match::Scattered)
}

fn starts_word(text: &[char], i: usize) -> bool {
    let ch = text[i];
    if !ch.is_alphanumeric() {
        return false;
    }
    let Some(&before) = i.checked_sub(1).and_then(|j| text.get(j)) else {
        return true;
    };
    !before.is_alphanumeric()
        || (ch.is_uppercase() && !before.is_uppercase())
        || ch.is_ascii_digit() != before.is_ascii_digit()
}

/// The typed word, split into runs, each the start of a later word in `text`.
fn matches_word_starts(
    text: &[char],
    starts: &[usize],
    word: &[char],
    same: &impl Fn(char, char) -> bool,
) -> bool {
    let Some((&first, rest)) = starts.split_first() else {
        return word.is_empty();
    };
    if word.is_empty() {
        return true;
    }
    let run = text[first..]
        .iter()
        .zip(word)
        .take_while(|&(&a, &b)| same(a, b))
        .count();
    (1..=run)
        .rev()
        .any(|taken| matches_word_starts(text, rest, &word[taken..], same))
        || matches_word_starts(text, rest, word, same)
}

fn hover_markdown(hover: &Hover) -> String {
    let marked = |text: &MarkedString| match text {
        MarkedString::String(text) => text.clone(),
        MarkedString::LanguageString(code) => {
            format!("```{}\n{}\n```", code.language, code.value)
        }
    };
    match &hover.contents {
        HoverContents::Scalar(text) => marked(text),
        HoverContents::Array(parts) => parts.iter().map(marked).collect::<Vec<_>>().join("\n\n"),
        HoverContents::Markup(markup) => markup.value.clone(),
    }
}

fn completion_detail(item: &CompletionItem) -> Option<SharedString> {
    item.detail
        .clone()
        .or_else(|| {
            item.label_details
                .as_ref()
                .and_then(|details| details.description.clone())
        })
        .or_else(|| match &item.documentation {
            Some(Documentation::String(text)) => text.lines().next().map(str::to_owned),
            _ => None,
        })
        .map(Into::into)
}

impl Render for EditorPopups {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let root = div().absolute();
        let tokens = Theme::global(cx).tokens.clone();
        let state = self.state.read(cx);
        let completion = state.completion_menu_state();

        let mut anchor = None;
        let menu = if completion.open && !self.completions.is_empty() {
            let start = state.completion_word_range().start;
            state.range_to_bounds(&(start..start)).map(|bounds| {
                let rows = self
                    .completions
                    .iter()
                    .map(|item| (item.label.clone().into(), completion_detail(item)))
                    .collect();
                (bounds, InputOverlayKind::Completion, Rc::new(rows))
            })
        } else if state.code_action_menu_state().open && !self.actions.is_empty() {
            let cursor = state.cursor();
            state.range_to_bounds(&(cursor..cursor)).map(|bounds| {
                let rows = self
                    .actions
                    .iter()
                    .map(|item| (item.action.title.clone().into(), None))
                    .collect();
                (bounds, InputOverlayKind::CodeAction, Rc::new(rows))
            })
        } else {
            None
        };

        let hover = state.hover_popover().cloned();
        let diagnostic = state.diagnostic_popover();
        let info = hover
            .as_ref()
            .map(|hover| hover.symbol_range.start)
            .or(diagnostic.as_ref().map(|entry| entry.range.start))
            .and_then(|start| state.range_to_bounds(&(start..start)));
        let diagnostic = diagnostic.map(|entry| {
            let colors = state.editor_style().diagnostics;
            let color = match entry.severity {
                DiagnosticSeverity::Error => colors.error,
                DiagnosticSeverity::Warning => colors.warning,
                DiagnosticSeverity::Info => colors.info,
                DiagnosticSeverity::Hint => colors.hint,
            };
            (entry.message.clone(), color)
        });
        let margin = scaled_px(WINDOW_MARGIN);

        let menu = menu.map(|(bounds, kind, rows)| {
            let element = self.menu(kind, rows, &tokens, window, cx);
            anchor = Some(bounds.origin);
            deferred(
                anchored()
                    .position(bounds.bottom_left())
                    .snap_to_window_with_margin(margin)
                    .child(element),
            )
            .with_priority(PRIORITY)
        });
        let info = info.map(|bounds| {
            let element = self.info_card(
                hover.as_ref().map(|hover| &hover.hover),
                diagnostic,
                &tokens,
            );
            anchor.get_or_insert(bounds.origin);
            // Above the line when the card fits, so it doesn't cover the code
            // being read under the pointer.
            let above = bounds.top() > scaled_px(CARD_HEIGHT + WINDOW_MARGIN);
            let (position, corner) = if above {
                (bounds.origin, Anchor::BottomLeft)
            } else {
                (bounds.bottom_left(), Anchor::TopLeft)
            };
            deferred(
                anchored()
                    .position(position)
                    .anchor(corner)
                    .snap_to_window_with_margin(margin)
                    .child(element),
            )
            .with_priority(PRIORITY)
        });

        match anchor {
            Some(anchor) => self.watch_anchor(anchor, window, cx),
            None => self.drawn_at = None,
        }
        root.children(menu).children(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranked(labels: &[&str], word: &str) -> Vec<String> {
        let items: Vec<CompletionItem> = labels
            .iter()
            .map(|label| CompletionItem {
                label: label.to_string(),
                ..Default::default()
            })
            .collect();
        visible_completions(&items, word)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn a_better_match_ranks_above_a_better_sort_text() {
        assert_eq!(
            ranked(&["afxoo", "Focus", "focus", "fo", "xfo"], "fo"),
            ["fo", "focus", "Focus", "afxoo", "xfo"]
        );
    }

    #[test]
    fn word_starts_rank_above_scattered_letters() {
        assert_eq!(
            ranked(&["gabbyNote", "getByName", "get_by_name", "zzz"], "gBN"),
            ["getByName", "get_by_name", "gabbyNote"]
        );
        assert_eq!(
            ranked(&["get_by_name", "target_by"], "getby"),
            ["get_by_name", "target_by"]
        );
    }
}
