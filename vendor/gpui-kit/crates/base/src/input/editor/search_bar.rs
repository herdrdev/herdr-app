//! The code editor's find bar: the built-in panel the search session opens.
//! It drives the same engine a custom search UI would (`set_search_query`,
//! `next_search_match`, the replace methods), so the two never disagree.

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement as _, IntoElement as _,
    MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use super::{EditorState, Escape, Input, InputEvent, InputState, Replace, Search, state::ScrollPadding};

/// The bar's two fields, kept while the editor lives so a reopened bar
/// resumes its query and replacement.
pub(crate) struct SearchBar {
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    _subscriptions: [Subscription; 2],
}

impl EditorState {
    /// Answers the search shortcut: builds the bar the first time, then
    /// focuses its query with the text selected, as a second Cmd+F does in
    /// VS Code.
    pub(crate) fn show_search_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_session.open {
            return;
        }
        if self.extras.search_bar.is_none() {
            let query = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
            let replacement = cx.new(|cx| InputState::new(window, cx).placeholder("Replace"));
            let on_query = cx.subscribe_in(&query, window, |this, query, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    let text = query.read(cx).value().to_string();
                    this.search_typed(text, cx);
                }
                InputEvent::PressEnter { shift, .. } => this.step_search(*shift, cx),
                _ => {}
            });
            let on_replacement =
                cx.subscribe_in(&replacement, window, |this, input, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { secondary, .. } = event {
                        let text = input.read(cx).value().to_string();
                        if *secondary {
                            this.replace_all_search_matches(&text, window, cx);
                        } else {
                            this.replace_current_search_match(&text, window, cx);
                        }
                    }
                });
            self.extras.search_bar = Some(SearchBar {
                query,
                replacement,
                _subscriptions: [on_query, on_replacement],
            });
        }
        let Some(bar) = &self.extras.search_bar else { return };
        let query = bar.query.clone();
        let text = self.search_session.query.clone();
        query.update(cx, |field, cx| {
            if field.value().as_ref() != text {
                field.set_value(text, window, cx);
            }
            field.select_all(window, cx);
            field.focus(window, cx);
        });
        cx.notify();
    }

    /// A new query from the bar: the current match becomes the first one at
    /// or after the cursor, and it is scrolled into view.
    fn search_typed(&mut self, query: String, cx: &mut Context<Self>) {
        // The field reports a change for keys that leave its text as it was (Enter).
        if query == self.search_session.query {
            return;
        }
        let case_insensitive = self.search_session.case_insensitive;
        self.set_search_query(query, case_insensitive, cx);
        self.reveal_search_from_cursor(cx);
    }

    fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        let query = self.search_session.query.clone();
        let case_insensitive = !self.search_session.case_insensitive;
        self.set_search_query(query, case_insensitive, cx);
        self.reveal_search_from_cursor(cx);
    }

    fn reveal_search_from_cursor(&mut self, cx: &mut Context<Self>) {
        let cursor = self.selected_range().start;
        let matcher = &mut self.search_session.matcher;
        matcher.update_cursor_by_offset(cursor);
        let current = matcher.current().and_then(|ix| matcher.matched_ranges().get(ix).cloned());
        if let Some(range) = current {
            self.scroll_to_with_padding(range.end, None, ScrollPadding::SurroundingLines, cx);
        }
        cx.notify();
    }

    fn step_search(&mut self, back: bool, cx: &mut Context<Self>) {
        if back {
            self.previous_search_match(cx);
        } else {
            self.next_search_match(cx);
        }
        cx.notify();
    }

    /// Closes the bar and hands the keyboard back to the text, with the
    /// current match selected so the caret lands where the search was.
    fn dismiss_search_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let matcher = &self.search_session.matcher;
        let current = matcher.current().and_then(|ix| matcher.matched_ranges().get(ix).cloned());
        if let Some(range) = current {
            self.set_selected_range(range, cx);
        }
        self.close_search(cx);
        self.focus(window, cx);
    }
}

/// The bar over the editor's top-right corner while the panel is open.
pub(crate) fn render(state: &Entity<EditorState>, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let editor = state.read(cx);
    let session = editor.search_session();
    if !session.open {
        return None;
    }
    let bar = editor.extras.search_bar.as_ref()?;
    let colors = crate::Theme::global(cx).tokens.colors;
    let replaceable = editor.is_replaceable();
    let replace_mode = session.replace_mode;
    let label: SharedString = if session.query.is_empty() {
        "".into()
    } else if session.matcher.is_empty() {
        "No results".into()
    } else {
        session.matcher.label().into()
    };

    let button = |id: &'static str, text: &'static str, active: bool, on_click: fn(&mut EditorState, &mut Window, &mut Context<EditorState>)| {
        div()
            .id(id)
            .h(px(22.))
            .min_w(px(22.))
            .px_1()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .when(active, |el| el.bg(colors.accent).text_color(colors.foreground))
            .hover(|el| el.bg(colors.accent))
            .on_click(window.listener_for(state, move |this, _: &ClickEvent, window, cx| on_click(this, window, cx)))
            .child(text)
    };
    let field = |input: &Entity<InputState>| {
        div()
            .flex_1()
            .min_w(px(48.))
            .h(px(24.))
            .px_1()
            .flex()
            .items_center()
            .rounded_sm()
            .border_1()
            .border_color(colors.border)
            .bg(colors.input)
            .child(Input::new(input))
    };

    let find_row = div()
        .flex()
        .items_center()
        .gap_1()
        .when(replaceable, |row| {
            row.child(button("search-bar-toggle-replace", if replace_mode { "⌄" } else { "›" }, false, |this, _, cx| {
                let replace_mode = !this.search_session.replace_mode;
                this.set_search_replace_mode(replace_mode, cx);
            }))
        })
        .child(field(&bar.query))
        .child(div().w(px(64.)).flex_none().text_color(colors.muted_foreground).child(label))
        .child(button("search-bar-case", "Aa", !session.case_insensitive, |this, _, cx| this.toggle_search_case(cx)))
        .child(button("search-bar-previous", "↑", false, |this, _, cx| this.step_search(true, cx)))
        .child(button("search-bar-next", "↓", false, |this, _, cx| this.step_search(false, cx)))
        .child(button("search-bar-close", "×", false, |this, window, cx| this.dismiss_search_bar(window, cx)));
    let replacement = bar.replacement.clone();
    let replace_row = (replaceable && replace_mode).then(|| {
        let one = replacement.clone();
        let all = replacement.clone();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(div().w(px(22.)).flex_none())
            .child(field(&replacement))
            .child(
                div()
                    .id("search-bar-replace")
                    .px_1()
                    .h(px(22.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|el| el.bg(colors.accent))
                    .on_click(window.listener_for(state, move |this, _: &ClickEvent, window, cx| {
                        let text = one.read(cx).value().to_string();
                        this.replace_current_search_match(&text, window, cx);
                    }))
                    .child("Replace"),
            )
            .child(
                div()
                    .id("search-bar-replace-all")
                    .px_1()
                    .h(px(22.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|el| el.bg(colors.accent))
                    .on_click(window.listener_for(state, move |this, _: &ClickEvent, window, cx| {
                        let text = all.read(cx).value().to_string();
                        this.replace_all_search_matches(&text, window, cx);
                    }))
                    .child("All"),
            )
    });

    Some(
        div()
            .id("search-bar")
            .absolute()
            .top_1()
            .right(px(16.))
            .w(px(420.))
            .max_w_full()
            .p_1()
            .flex()
            .flex_col()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .text_color(colors.foreground)
            .text_size(px(12.))
            .shadow_md()
            .cursor_default()
            .key_context("EditorSearchBar")
            // The editor's frame focuses the text on a click; the bar keeps it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(window.listener_for(state, |this, _: &Escape, window, cx| this.dismiss_search_bar(window, cx)))
            .on_action(window.listener_for(state, |this, _: &Search, window, cx| {
                this.open_search(false, cx);
                this.show_search_bar(window, cx);
            }))
            .on_action(window.listener_for(state, |this, _: &Replace, window, cx| {
                this.open_search(true, cx);
                this.show_search_bar(window, cx);
            }))
            .child(find_row)
            .children(replace_row)
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, prelude::*};

    use crate::Theme;
    use crate::input::{Editor, EditorState};

    /// Draws the editor through its element, so the find bar is on screen.
    struct Host(Entity<EditorState>);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(Editor::new(&self.0))
        }
    }

    fn open(cx: &mut TestAppContext, text: &'static str) -> (Entity<EditorState>, VisualTestContext) {
        let mut editor = None;
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                cx.set_global(Theme::default());
                super::super::init(cx);
                let state = cx.new(|cx| EditorState::new(window, cx));
                editor = Some(state.clone());
                cx.new(|_| Host(state))
            })
            .unwrap()
        });
        let editor = editor.unwrap();
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value(text, window, cx);
                state.focus(window, cx);
            });
        });
        cx.run_until_parked();
        (editor, cx)
    }

    fn press(cx: &mut VisualTestContext, mac: &str, other: &str) {
        cx.simulate_keystrokes(if cfg!(target_os = "macos") { mac } else { other });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_shortcut_opens_a_find_bar_that_types_steps_and_closes(cx: &mut TestAppContext) {
        let (editor, mut cx) = open(cx, "foo bar\nfoo baz foo");
        press(&mut cx, "cmd-f", "ctrl-f");
        cx.update(|window, cx| {
            let state = editor.read(cx);
            assert!(state.search_session().open);
            let query = &state.extras.search_bar.as_ref().expect("the shortcut builds the bar").query;
            assert!(query.read(cx).focus_handle.is_focused(window), "typing goes to the query");
        });

        cx.simulate_input("foo");
        cx.run_until_parked();
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.search_session().query, "foo");
            assert_eq!(state.search_session().matcher.len(), 3);
            assert_eq!(state.search_session().matcher.current(), Some(0));
        });

        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        editor.read_with(&cx, |state, _| {
            assert_eq!(state.value().as_ref(), "foo bar\nfoo baz foo", "Enter in the bar never edits the text");
            assert_eq!(state.search_session().matcher.current(), Some(1));
        });
        cx.simulate_keystrokes("shift-enter");
        cx.run_until_parked();
        editor.read_with(&cx, |state, _| assert_eq!(state.search_session().matcher.current(), Some(0)));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let state = editor.read(cx);
            assert!(!state.search_session().open);
            assert!(!state.search_session().is_active());
            assert!(state.focus_handle.is_focused(window), "Escape hands the keyboard back to the text");
            assert_eq!(state.selected_range(), 8..11, "the caret lands on the match the search was on");
        });

        // Reopening resumes the query, selected so typing replaces it.
        press(&mut cx, "cmd-f", "ctrl-f");
        cx.update(|_, cx| {
            let state = editor.read(cx);
            let query = state.extras.search_bar.as_ref().unwrap().query.read(cx);
            assert_eq!(query.value().as_ref(), "foo");
            assert_eq!(query.selected_range(), 0..3);
        });
    }

    #[gpui::test]
    fn the_replace_field_replaces_one_match_or_all(cx: &mut TestAppContext) {
        let (editor, mut cx) = open(cx, "a1 a2 a3");
        press(&mut cx, "cmd-shift-f", "ctrl-h");
        cx.simulate_input("a");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let replacement = editor.read(cx).extras.search_bar.as_ref().unwrap().replacement.clone();
            assert!(editor.read(cx).search_session().replace_mode);
            replacement.update(cx, |field, cx| {
                field.set_value("b", window, cx);
                field.focus(window, cx);
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        editor.read_with(&cx, |state, _| assert_eq!(state.value().as_ref(), "b1 a2 a3"));

        press(&mut cx, "cmd-enter", "ctrl-enter");
        editor.read_with(&cx, |state, _| assert_eq!(state.value().as_ref(), "b1 b2 b3"));
    }

    #[gpui::test]
    fn a_read_only_editor_finds_but_offers_no_replace(cx: &mut TestAppContext) {
        let (editor, mut cx) = open(cx, "x y x");
        cx.update(|_, cx| editor.update(cx, |state, cx| state.set_readonly(true, cx)));
        press(&mut cx, "cmd-shift-f", "ctrl-h");
        cx.simulate_input("x");
        cx.run_until_parked();
        editor.read_with(&cx, |state, _| {
            assert!(state.search_session().open);
            assert!(!state.search_session().replace_mode);
            assert_eq!(state.search_session().matcher.len(), 2);
        });
    }
}
