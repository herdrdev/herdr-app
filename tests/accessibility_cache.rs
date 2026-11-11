use gpui::{AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Render, Role, StatefulInteractiveElement, StyleRefinement,
    Styled, TestAppContext, Window, div};

struct TerminalChild(FocusHandle);

impl Render for TerminalChild {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("terminal").role(Role::Terminal).aria_label("QA terminal")
            .track_focus(&self.0).size_full()
    }
}

struct Parent(Entity<TerminalChild>);

impl Render for Parent {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.0.clone().cached(StyleRefinement::default()))
    }
}

#[gpui::test]
fn cached_terminal_remains_accessible_across_redraws(cx: &mut TestAppContext) {
    let handle = cx.add_window(|window, cx| {
        let child = cx.new(|cx| TerminalChild(cx.focus_handle()));
        let focus = child.read(cx).0.clone();
        focus.focus(window, cx);
        Parent(child)
    });
    // Populate the visual cache before an assistive client connects.
    cx.update_window(handle.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    }).unwrap();
    cx.update_window(handle.into(), |_, window, _| {
        window.activate_accessibility_for_test();
    }).unwrap();

    // The child is never notified. Reusing its visual cache must not remove
    // its accessibility node or focus from the next frame's complete tree.
    for frame in 0..3 {
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            let dump = window.debug_a11y_tree_json().expect("active accessibility tree");
            let tree: serde_json::Value = serde_json::from_str(&dump).unwrap();
            assert!(dump.contains("QA terminal"), "terminal disappeared on frame {frame}: {dump}");
            assert_ne!(tree["gpui_focus"], tree["root"], "focus fell back to the window on frame {frame}");
            assert!(!tree["gpui_focus"].is_null(), "terminal focus is missing");
        }).unwrap();
    }
}
