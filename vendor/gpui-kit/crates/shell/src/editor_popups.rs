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
    CompletionItem, CompletionTextEdit, Documentation, Hover, HoverContents, MarkedString,
};
use gpui_base::{
    SemanticThemeTokens, Theme,
    input::{
        CodeActionItem, DiagnosticSeverity, EditorState, Enter, Escape, IndentInline,
        InputOverlayKind, MoveDown, MoveUp,
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
            .slice(word_start(state)..state.cursor())
            .to_string();
        let key = (completion.revision(), code_action.revision(), word);
        if key != self.menu_key {
            let revised = key.0 != self.menu_key.0 || key.1 != self.menu_key.1;
            self.completions = Rc::new(visible_completions(&completion.items, &key.2));
            self.actions = Rc::new(code_action.items.clone());
            self.menu_key = key;
            if revised {
                self.selected = 0;
                self.list.scroll_to_item(0, ScrollStrategy::Top);
            }
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
            self.select((self.selected + len - 1) % len, cx);
        } else if action.partial_eq(&MoveDown) {
            self.select((self.selected + 1) % len, cx);
        } else if Enter::is_primary(&*action) || action.partial_eq(&IndentInline) {
            self.accept(kind, self.selected, window, cx);
        } else if !action.partial_eq(&Escape) {
            return false;
        }
        true
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index;
        self.list.scroll_to_item(index, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn accept(
        &mut self,
        kind: InputOverlayKind,
        index: usize,
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
                        insert(state, &item, window, cx);
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
                                .update(cx, |popups, cx| popups.accept(kind, index, window, cx))
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

fn word_start(state: &EditorState) -> usize {
    let cursor = state.cursor();
    let word: usize = state
        .text()
        .chars_at(cursor)
        .reversed()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .map(char::len_utf8)
        .sum();
    cursor - word
}

fn insert(
    state: &mut EditorState,
    item: &CompletionItem,
    window: &mut Window,
    cx: &mut Context<EditorState>,
) {
    // Base inserts a label over the range it's given but an `insertText` after
    // it, which would leave the typed prefix in front of the completion.
    let mut item = item.clone();
    if item.text_edit.is_none()
        && let Some(text) = item.insert_text.take()
    {
        item.label = text;
    }
    // Typing on while the menu waits for its next answer leaves the item's range
    // short of the caret, and the characters past it after the completion.
    let caret = state.cursor_position();
    let end = match &mut item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => Some(&mut edit.range.end),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => Some(&mut edit.replace.end),
        None => None,
    };
    if let Some(end) = end
        && end.line == caret.line
        && end.character < caret.character
    {
        *end = caret;
    }
    let range = word_start(state)..state.cursor();
    state.insert_completion(&item, range, window, cx);
    // Edits outside the completion's own, an auto-import most often. They read
    // against the document from before the insert, which only moved what's
    // after it, and servers put them before it.
    if let Some(edits) = &item.additional_text_edits {
        state.apply_lsp_edits(edits, window, cx);
    }
}

fn visible_completions(items: &[CompletionItem], word: &str) -> Vec<CompletionItem> {
    let word: Vec<char> = word.to_lowercase().chars().collect();
    let mut visible: Vec<CompletionItem> = items
        .iter()
        .filter(|item| {
            let text = item.filter_text.as_deref().unwrap_or(&item.label);
            let mut wanted = word.iter().peekable();
            for ch in text.to_lowercase().chars() {
                if wanted.peek() == Some(&&ch) {
                    wanted.next();
                }
            }
            wanted.peek().is_none()
        })
        .cloned()
        .collect();
    visible.sort_by(|a, b| {
        let key =
            |item: &CompletionItem| item.sort_text.clone().unwrap_or_else(|| item.label.clone());
        key(a).cmp(&key(b))
    });
    visible
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
            let start = word_start(state);
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
