#[test]
fn focus_follows_the_pointer_that_stays_on_a_pane() {
    in_own_process("focus_follows_mouse");
}

/// The whole path, from a pointer move over a painted pane to `pane.focus`.
#[gpui::test]
fn focus_follows_mouse(cx: &mut gpui::TestAppContext) {
    use crate::model::pane;
    use gpui::{Modifiers, VisualTestContext, point, px, size};
    use serde_json::json;
    if !running_alone("focus_follows_mouse") { return; }
    let (view, cx) = open_window(cx, "probe", "export default () => null;", json!({
        "id": "context", "type": "block", "content": { "tab": "active" }
    }));
    cx.simulate_resize(size(px(1000.0), px(700.0)));
    let mut tab = super::Tab::new("tab", "", "a", "term-a");
    tab.root.split("a", super::Axis::Row, pane("b", "term-b"), "row".into());
    tab.focus("a");
    view.update_in(cx, |this, window, cx| {
        this.person.focus_follows_mouse = true;
        this.apply_look(window, cx);
        this.model.add_space("space", "test", "/", tab);
        this.sync(window, cx);
        window.activate_window();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let center = |id: &str, cx: &mut VisualTestContext| view.update(cx, |this, _| this.bounds.borrow()[id].center());
    let wait = |cx: &mut VisualTestContext| {
        cx.executor().advance_clock(std::time::Duration::from_millis(200));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    };
    let focused = |cx: &mut VisualTestContext| view.update(cx, |this, _| this.model.active_tab().unwrap().focused.clone().unwrap());
    let (a, b) = (center("a", cx), center("b", cx));

    cx.simulate_mouse_move(b, None, Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "b", "staying on b focuses it");
    view.update_in(cx, |this, window, _| assert!(this.terms["b"].focus.is_focused(window), "with the keyboard"));

    cx.simulate_mouse_move(a, None, Modifiers::none());
    cx.simulate_mouse_move(b, None, Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "b", "passing over a doesn't focus it");

    cx.simulate_mouse_move(a, Some(gpui::MouseButton::Left), Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "b", "never while a button is held");

    // A command moves the focus under a pointer that stays put.
    cx.simulate_mouse_move(b, None, Modifiers::none());
    wait(cx);
    view.update_in(cx, |this, window, cx| this.run("pane.focus", json!({ "pane": "a" }), window, cx));
    cx.simulate_mouse_move(b + point(px(20.0), px(0.0)), None, Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "a", "moving within b doesn't take it back");
    cx.simulate_mouse_move(a, None, Modifiers::none());
    cx.simulate_mouse_move(b, None, Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "b", "entering b again does");

    // The pointer enters b while another app is in front, and stays there.
    cx.simulate_mouse_move(a, None, Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "a");
    cx.deactivate_window();
    cx.simulate_mouse_move(b, None, Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "a", "not while the window is inactive");
    view.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    cx.simulate_mouse_move(b + point(px(5.0), px(0.0)), None, Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "b", "the first move once it's active counts as entering");

    // A window opens over b while the pointer waits there.
    cx.simulate_mouse_move(a, None, Modifiers::none());
    wait(cx);
    assert_eq!(focused(cx), "a");
    cx.simulate_mouse_move(b, None, Modifiers::none());
    view.update_in(cx, |this, window, cx| {
        this.open_overlay("cover".into(), json!({ "mod": "probe", "kind": "window", "width": 950, "height": 650 }), window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    wait(cx);
    assert_eq!(focused(cx), "a", "a pane covered when the wait ends keeps its place");
}
