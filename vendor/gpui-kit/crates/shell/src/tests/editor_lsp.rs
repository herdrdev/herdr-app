//! HERDR: a mod drives a code editor's language features through `EditorState`.
use std::{cell::RefCell, rc::Rc};

use gpui::{AppContext as _, Entity, Task, TestAppContext, VisualTestContext};
use gpui_base::input::{EditorState, RopeExt as _, lsp_types};

use crate::{ScriptView, ShellRuntime};

struct Harness {
    context: VisualTestContext,
    view: Entity<ScriptView>,
    editor: Entity<EditorState>,
}

/// `body` is a view whose `this.code` is the editor and whose `this.log` shows up in `log()`.
fn open(cx: &mut TestAppContext, body: &str) -> Harness {
    cx.update(crate::init);
    let runtime = ShellRuntime::new_isolated().expect("runtime");
    cx.update(|cx| runtime.set_global(cx));
    let source = format!(
        r#"
import {{ View, div }} from "gpui-kit";
import {{ Editor, EditorState }} from "gpui-base";
export default class Code extends View {{
  {body}
  render() {{
    return div().size_full().child(Editor.new(this.code).h(240)).child("log=" + this.log.join(","));
  }}
}}
"#
    );
    let view_type = runtime.load_source("code", &source).expect("load");
    let script_view = Rc::new(RefCell::new(None));
    let saved_view = script_view.clone();
    let runtime_for_view = runtime.clone();
    // The real root rather than `draw(element)`: popups are deferred, and only a
    // whole window's draw gets back to them.
    let (_root, context) = cx.add_window_view(move |window, cx| {
        let view = runtime_for_view
            .instantiate_view(&view_type, window, cx)
            .expect("instantiate");
        *saved_view.borrow_mut() = Some(view.clone());
        crate::root::ShellRoot::new(view.into(), window, cx)
    });
    let context = context.clone();
    let view = script_view.borrow_mut().take().expect("the view");
    let editor = runtime.entities().first_editor().expect("editor");
    let mut harness = Harness {
        context,
        view,
        editor,
    };
    harness.draw();
    harness
}

impl Harness {
    fn draw(&mut self) {
        self.context.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn settle(&mut self) {
        self.context.run_until_parked();
        self.draw();
    }

    fn focus(&mut self) {
        let editor = self.editor.clone();
        self.context
            .update(|window, cx| editor.update(cx, |state, cx| state.focus(window, cx)));
        self.settle();
    }

    fn log(&mut self) -> String {
        self.settle();
        let view = self.view.clone();
        let tree = self.context.update(|_, cx| {
            view.read(cx)
                .snapshot()
                .map(crate::snapshot::RenderSnapshot::debug_tree)
                .unwrap_or_default()
        });
        tree.split("log=")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_default()
            .to_owned()
    }

    fn text(&mut self) -> String {
        let editor = self.editor.clone();
        self.context
            .update(|_, cx| editor.read(cx).value().to_string())
    }

    fn cursor(&mut self) -> usize {
        let editor = self.editor.clone();
        self.context.update(|_, cx| editor.read(cx).cursor())
    }

    /// Runs `ask` from inside an editor update, which is where base asks from.
    fn ask<R: 'static>(
        &mut self,
        ask: impl FnOnce(
            &mut EditorState,
            &mut gpui::Window,
            &mut gpui::Context<EditorState>,
        ) -> Task<anyhow::Result<R>>,
    ) -> anyhow::Result<R> {
        let answer = Rc::new(RefCell::new(None));
        let editor = self.editor.clone();
        let slot = answer.clone();
        self.context.update(|window, cx| {
            let task = editor.update(cx, |state, cx| ask(state, window, cx));
            cx.spawn(async move |_| {
                *slot.borrow_mut() = Some(task.await);
            })
            .detach();
        });
        self.settle();
        answer
            .borrow_mut()
            .take()
            .expect("the handler never answered")
    }
}

#[gpui::test]
fn cursor_and_selection_cross_as_lsp_positions(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    this.code = EditorState.new({ value: "one\ntwo\nthree" });
    const place = (p) => p.line + ":" + p.character;
    this.log.push(this.code.selection() === null ? "none" : "some");
    this.code.set_cursor({ line: 1, character: 2 });
    this.log.push(place(this.code.cursor()));
    this.code.set_selection({ start: { line: 2, character: 4 }, end: { line: 0, character: 1 } });
    const selection = this.code.selection();
    this.log.push(place(selection.start) + "-" + place(selection.end));
    this.code.set_cursor({ line: 99, character: 99 });
    this.log.push(place(this.code.cursor()));
  }
"#,
    );
    assert_eq!(h.log(), "none,1:2,0:1-2:4,2:5");
}

#[gpui::test]
fn the_cursor_event_fires_only_when_the_caret_or_selection_moves(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    this.code = EditorState.new({ value: "one\ntwo" });
    this.code.on("cursor", (event, cx) => {
      this.log.push(event.position.line + ":" + event.position.character + (event.selection ? "s" : ""));
      cx.notify();
    });
  }
"#,
    );
    assert_eq!(h.log(), "");
    let editor = h.editor.clone();
    h.context
        .update(|_, cx| editor.update(cx, |_, cx| cx.notify()));
    assert_eq!(h.log(), "");
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(4..4, cx)));
    assert_eq!(h.log(), "1:0");
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(4..7, cx)));
    assert_eq!(h.log(), "1:0,1:3s");
}

#[gpui::test]
fn diagnostics_survive_being_stale_unsorted_or_backwards(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    this.code = EditorState.new({ value: "one\ntwo" });
    const at = (line, a, b) => ({ start: { line, character: a }, end: { line, character: b } });
    this.code.set_diagnostics([
      { range: at(1, 0, 3), severity: 2, message: "second" },
      { range: at(0, 3, 0), severity: 1, message: "first" },
      { range: at(50, 0, 9), message: "past the end" },
    ]);
  }
"#,
    );
    let editor = h.editor.clone();
    h.context.update(|_, cx| {
        let state = editor.read(cx);
        let set = state.diagnostics().expect("a code editor has them");
        assert_eq!(set.len(), 3);
        assert_eq!(set.for_offset(1).expect("first").message.as_ref(), "first");
        assert_eq!(
            set.for_offset(5).expect("second").message.as_ref(),
            "second"
        );
    });
    h.draw();
}

#[gpui::test]
fn a_handler_answers_while_base_is_mid_update_and_may_read_the_editor(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    this.code = EditorState.new({
      value: "alpha beta",
      lsp: {
        hover: async ({ position }, cx) => {
          this.log.push(this.code.value().length + "@" + position.character);
          await Promise.resolve();
          cx.notify();
          if (position.character === 0) return null;
          if (position.character === 1) throw new Error("server down");
          return { contents: { kind: "markdown", value: "**doc**" } };
        },
      },
    });
  }
"#,
    );
    let provider = h.context.update(|_, cx| {
        h.editor
            .read(cx)
            .lsp()
            .hover_provider
            .clone()
            .expect("hover")
    });
    let ask = |offset: usize| {
        let provider = provider.clone();
        move |state: &mut EditorState,
              window: &mut gpui::Window,
              cx: &mut gpui::Context<EditorState>| {
            provider.hover(state.text(), offset, window, cx)
        }
    };
    let hover = h.ask(ask(6)).expect("answer").expect("a hover");
    assert!(matches!(
        hover.contents,
        lsp_types::HoverContents::Markup(_)
    ));
    assert!(h.ask(ask(0)).expect("answer").is_none());
    assert!(
        h.ask(ask(1)).is_err(),
        "a rejection reaches base as an error"
    );
    assert_eq!(h.log(), "10@6,10@0,10@1");
}

#[gpui::test]
fn a_released_editor_answers_nothing_and_does_not_call_its_handler(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    this.code = EditorState.new({ value: "x", lsp: { hover: () => { throw new Error("called"); } } });
    this.code.on("cursor", () => this.code.release());
  }
"#,
    );
    let editor = h.editor.clone();
    let provider = h
        .context
        .update(|_, cx| editor.read(cx).lsp().hover_provider.clone().expect("hover"));
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(1..1, cx)));
    h.context.run_until_parked();
    let answer = h.ask(move |state, window, cx| provider.hover(state.text(), 0, window, cx));
    let message = answer.expect_err("nothing left to answer").to_string();
    assert!(message.contains("released"), "{message}");
}

#[gpui::test]
fn the_completion_menu_filters_navigates_and_inserts(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    const at = (line, a, b) => ({ start: { line, character: a }, end: { line, character: b } });
    this.code = EditorState.new({
      value: "first\nfo",
      lsp: {
        completion_triggers: ["."],
        completion: ({ context }, cx) => {
          this.log.push("kind" + context.triggerKind);
          cx.notify();
          return {
            isIncomplete: false,
            items: [
              { label: "zzz", sortText: "3" },
              { label: "fourth", sortText: "2", insertText: "fourth()" },
              {
                label: "focus", sortText: "1",
                textEdit: { range: at(1, 0, 2), newText: "focus" },
                additionalTextEdits: [{ range: at(0, 0, 0), newText: "import x\n" }],
              },
            ],
          };
        },
        completion_accepted: (item, cx) => { this.log.push("accepted-" + item.label); cx.notify(); },
      },
    });
  }
"#,
    );
    let editor = h.editor.clone();
    h.focus();
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(8..8, cx)));
    let invoke = |h: &mut Harness| {
        let editor = h.editor.clone();
        h.context
            .update(|window, cx| editor.update(cx, |state, cx| state.show_completions(window, cx)));
        h.settle();
    };

    invoke(&mut h);
    let open = h
        .context
        .update(|_, cx| editor.read(cx).completion_menu_state().open);
    assert!(open, "the menu opened");

    // `zzz` is filtered out by the typed `fo`, and `focus` sorts first.
    h.context.simulate_keystrokes("down enter");
    h.settle();
    assert_eq!(h.text(), "first\nfourth()");
    assert_eq!(h.cursor(), "first\nfourth()".len());

    let editor = h.editor.clone();
    h.context.update(|window, cx| {
        editor.update(cx, |state, cx| {
            state.set_value("first\nfo", window, cx);
            state.set_selected_range(8..8, cx);
        })
    });
    h.settle();
    invoke(&mut h);
    h.context.simulate_keystrokes("tab");
    h.settle();
    assert_eq!(h.text(), "import x\nfirst\nfocus");
    assert_eq!(
        h.cursor(),
        "import x\nfirst\nfocus".len(),
        "the caret stays on the completion, not on the import"
    );
    assert_eq!(h.log(), "kind1,accepted-fourth,kind1,accepted-focus");
}

#[gpui::test]
fn the_menu_stays_up_while_typing_on_through_its_word(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    const at = (line, a, b) => ({ start: { line, character: a }, end: { line, character: b } });
    this.answered = 0;
    this.code = EditorState.new({
      value: "",
      lsp: {
        completion: async (_, cx) => {
          // Only the first answer comes: the rest stay on their way. Holding the
          // resolver keeps them alive, or the abandoned promise would be freed
          // and read as the mod giving up.
          if (this.answered++ > 0) await new Promise((resolve) => { this.pending = resolve; });
          return [
            { label: "focus", textEdit: { range: at(0, 0, 1), newText: "focus" } },
            { label: "other", textEdit: { range: at(0, 0, 1), newText: "other" } },
          ];
        },
      },
    });
  }
"#,
    );
    let editor = h.editor.clone();
    h.focus();
    h.context.simulate_input("f");
    h.settle();
    let open = h
        .context
        .update(|_, cx| editor.read(cx).completion_menu_state().open);
    assert!(open, "the first key's answer opened the menu");
    h.context.simulate_input("o");
    h.settle();
    let open = h
        .context
        .update(|_, cx| editor.read(cx).completion_menu_state().open);
    assert!(
        open,
        "the second key's answer hasn't come, and the menu is still up"
    );
    h.context.simulate_keystrokes("enter");
    h.settle();
    assert_eq!(
        h.text(),
        "focus",
        "the `o` typed after the answer is inside the replaced range"
    );
}

#[gpui::test]
fn moving_away_closes_the_menu_for_good(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    this.code = EditorState.new({
      value: "x y",
      lsp: {
        completion: async () => {
          if (this.waiting) await new Promise((resolve) => { this.pending = resolve; });
          this.waiting = true;
          return [{ label: "xylophone" }];
        },
      },
    });
  }
"#,
    );
    let editor = h.editor.clone();
    h.focus();
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(1..1, cx)));
    h.context.simulate_input("y");
    h.settle();
    let open = |h: &mut Harness| {
        let editor = h.editor.clone();
        h.context
            .update(|_, cx| editor.read(cx).completion_menu_state().open)
    };
    assert!(open(&mut h));
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(4..4, cx)));
    h.settle();
    assert!(!open(&mut h));
    h.context.simulate_input("z");
    h.settle();
    assert!(
        !open(&mut h),
        "elsewhere in the text, the old menu doesn't come back with the next key"
    );
}

#[gpui::test]
fn escape_closes_the_menu_and_leaves_the_text(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    this.code = EditorState.new({
      value: "x",
      lsp: { completion: () => [{ label: "xylophone" }] },
    });
  }
"#,
    );
    let editor = h.editor.clone();
    h.focus();
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(1..1, cx)));
    h.context
        .update(|window, cx| editor.update(cx, |state, cx| state.show_completions(window, cx)));
    h.settle();
    h.context.simulate_keystrokes("escape");
    h.settle();
    assert_eq!(h.text(), "x");
    let open = h
        .context
        .update(|_, cx| editor.read(cx).completion_menu_state().open);
    assert!(!open);
}

#[gpui::test]
fn code_actions_get_their_diagnostics_and_run_through_the_mod(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    const at = (line, a, b) => ({ start: { line, character: a }, end: { line, character: b } });
    this.code = EditorState.new({
      value: "let x = 1",
      lsp: {
        code_actions: ({ range, context }, cx) => {
          this.log.push("for-" + context.diagnostics.map((d) => d.data.id).join("+"));
          cx.notify();
          return [
            { title: "Run", command: "do.it", arguments: [] },
            { title: "Nope", disabled: { reason: "no" } },
            { title: "Rename", edit: { changes: { "file:///a": [{ range: at(0, 4, 5), newText: "y" }] } } },
          ];
        },
        perform_code_action: (action, cx) => {
          this.log.push("run-" + action.title);
          cx.notify();
          if (action.edit) this.code.apply_edits(action.edit.changes["file:///a"]);
        },
      },
    });
    this.code.set_diagnostics([
      { range: at(0, 4, 5), severity: 1, message: "unused", data: { id: 7 } },
      { range: at(0, 8, 9), severity: 2, message: "far", data: { id: 8 } },
    ]);
  }
"#,
    );
    let editor = h.editor.clone();
    h.focus();
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(5..5, cx)));
    h.context.update(|window, cx| {
        editor.update(cx, |state, cx| state.handle_code_action_trigger(window, cx))
    });
    h.settle();
    let titles: Vec<String> = h.context.update(|_, cx| {
        editor
            .read(cx)
            .code_action_menu_state()
            .items
            .iter()
            .map(|item| item.action.title.clone())
            .collect()
    });
    assert_eq!(titles, ["Run", "Rename"], "a disabled action isn't offered");
    h.context.simulate_keystrokes("down enter");
    h.settle();
    assert_eq!(h.text(), "let y = 1");
    assert_eq!(h.log(), "for-7,run-Rename");
}

fn definition_source(show_document: &str) -> String {
    format!(
        r#"
  init() {{
    this.log = [];
    const at = (line, a, b) => ({{ start: {{ line, character: a }}, end: {{ line, character: b }} }});
    this.code = EditorState.new({{
      value: "one\ntwo\nthree",
      lsp: {{
        definition: () => ({{ uri: "file:///other.kt", range: at(2, 0, 5) }}),
        {show_document}
      }},
    }});
  }}
"#
    )
}

fn go_to_definition(h: &mut Harness) {
    let editor = h.editor.clone();
    h.focus();
    h.context
        .update(|_, cx| editor.update(cx, |state, cx| state.set_selected_range(1..1, cx)));
    h.context.update(|window, cx| {
        editor.update(cx, |state, cx| state.go_to_definition_at_cursor(window, cx))
    });
    h.settle();
}

#[gpui::test]
fn a_mod_can_show_a_followed_definition_itself(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        &definition_source(
            r#"show_document: (params, cx) => { this.log.push(params.uri + "@" + params.selection.start.line + this.code.value().length); cx.notify(); return true; },"#,
        ),
    );
    go_to_definition(&mut h);
    assert_eq!(h.cursor(), 1, "the mod showed it, so the editor stays put");
    assert_eq!(h.log(), "file:///other.kt@213");
}

#[gpui::test]
fn the_editor_jumps_when_the_mod_declines_or_has_no_hook(cx: &mut TestAppContext) {
    let mut declined = open(cx, &definition_source("show_document: async () => false,"));
    go_to_definition(&mut declined);
    assert_eq!(declined.cursor(), "one\ntwo\nthree".len());
}

#[gpui::test]
fn edits_read_against_the_old_text_undo_as_one_and_keep_the_caret(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    const at = (line, a, b) => ({ start: { line, character: a }, end: { line, character: b } });
    this.code = EditorState.new({ value: "abc\ndef" });
    this.code.on("change", (_, cx) => { this.log.push("change"); cx.notify(); });
    this.code.set_cursor({ line: 1, character: 1 });
    this.code.apply_edits([
      { range: at(1, 2, 3), newText: "F" },
      { range: at(0, 0, 0), newText: "1" },
      { range: at(0, 0, 0), newText: "2" },
    ]);
    const caret = this.code.cursor();
    this.log.push(caret.line + ":" + caret.character);
    try {
      this.code.apply_edits([{ range: at(0, 0, 2), newText: "" }, { range: at(0, 1, 3), newText: "" }]);
    } catch (error) {
      this.log.push("overlap");
    }
  }
"#,
    );
    assert_eq!(h.text(), "12abc\ndeF");
    assert_eq!(h.log(), "1:1,overlap,change");
    let editor = h.editor.clone();
    h.focus();
    assert_eq!(
        h.context.update(|_, cx| editor
            .read(cx)
            .text()
            .offset_to_position(editor.read(cx).cursor())
            .line),
        1
    );
    h.context.simulate_keystrokes("ctrl-z");
    h.settle();
    assert_eq!(h.text(), "abc\ndef", "undo reverts the whole batch");
}

#[gpui::test]
fn a_misnamed_handler_is_refused(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let runtime = ShellRuntime::new_isolated().expect("runtime");
    cx.update(|cx| runtime.set_global(cx));
    for (lsp, wanted) in [
        ("{ hoover: () => null }", "unknown lsp handler `hoover`"),
        ("{ code_actions: () => [] }", "perform_code_action"),
    ] {
        let source = format!(
            r#"
import {{ View }} from "gpui-kit";
import {{ EditorState }} from "gpui-base";
export default class Code extends View {{
  init() {{ this.code = EditorState.new({{ lsp: {lsp} }}); }}
  render() {{ return "x"; }}
}}
"#
        );
        let view_type = runtime.load_source("code", &source).expect("load");
        let window = cx.add_empty_window();
        let failure = window
            .update(|window, cx| runtime.instantiate(&view_type, window, cx))
            .err()
            .expect("init threw");
        assert!(failure.to_string().contains(wanted), "{failure}");
    }
}

#[gpui::test]
fn popups_draw_over_the_editor_without_disturbing_it(cx: &mut TestAppContext) {
    let mut h = open(
        cx,
        r#"
  init() {
    this.log = [];
    this.code = EditorState.new({ value: "one\ntwo" });
    this.code.set_diagnostics([
      { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 3 } }, severity: 1, message: "bad" },
    ]);
  }
"#,
    );
    let editor = h.editor.clone();
    h.context.update(|_, cx| {
        editor.update(cx, |state, cx| {
            let entry = state
                .diagnostics()
                .and_then(|set| set.for_offset(0))
                .cloned()
                .expect("diagnostic");
            state.present_diagnostic(entry, cx);
            state.present_hover(
                0..3,
                lsp_types::Hover {
                    contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                        kind: lsp_types::MarkupKind::Markdown,
                        value: "**doc**".into(),
                    }),
                    range: None,
                },
                cx,
            );
        })
    });
    h.settle();
    h.draw();
    assert_eq!(h.text(), "one\ntwo");
}
