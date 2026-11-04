#[test]
fn initial_pane_size_matches_the_rendered_terminal() {
    in_own_process("initial_pane_size");
}

#[gpui::test]
fn initial_pane_size(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    use super::Axis::{Column, Row};
    if !running_alone("initial_pane_size") { return; }
    let plain = json!({"id": "context", "type": "block", "content": {"tab": "active"}});
    let framed = json!({"id": "context", "type": "block", "padding": [8, 10],
        "border": {"width": 1, "color": "border"},
        "content": {"tab": "active", "pane": {
            "gap": 6, "border": {"width": 2, "color": "border"},
            "header": {"height": 23, "content": {"mod": "size-probe"}}
        }}
    });
    let main: serde_json::Value = serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap();
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", plain.clone());
    for (label, layout, steps) in [
        ("lone", plain.clone(), vec![]),
        ("right", plain.clone(), vec![Row]),
        ("down", plain.clone(), vec![Column]),
        ("nested", plain.clone(), vec![Row, Column, Row]),
        ("constrained-max", plain.clone(), vec![Row]),
        ("constrained-min", plain.clone(), vec![Row]),
        ("constrained-mixed", plain.clone(), vec![Row, Row]),
        ("thick-border", json!({"id": "context", "type": "block", "padding": [8, 10],
            "border": {"width": 20, "color": "border"}, "content": {"tab": "active"}}), vec![Row]),
        ("headers-padding", framed, vec![Row, Column]),
        ("main-right", main.clone(), vec![Row]),
        ("main-down", main.clone(), vec![Column]),
        ("main-nested", main, vec![Row, Column, Row]),
    ] {
        let mut root = crate::model::pane("a", "probe-a");
        let mut target = "a".to_owned();
        for (i, &axis) in steps.iter().enumerate().take(steps.len().saturating_sub(1)) {
            let next = format!("p{i}");
            root.split(&target, axis, crate::model::pane(&next, &format!("probe-{next}")), format!("s{i}"));
            target = next;
        }
        view.update_in(cx, |this, window, cx| {
            this.folders.retain(|id, _| id == "size-probe");
            this.chrome = serde_json::from_value(layout).unwrap();
            this.bounds.borrow_mut().clear();
            this.model = crate::model::Model::default();
            let mut tab = super::Tab::new("tab", "", "a", "probe-a");
            tab.root = root.clone();
            tab.focus(&target);
            this.model.add_space("space", "test", "/", tab);
            this.sync(window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        if let Some(&axis) = steps.last() {
            root.split(&target, axis, crate::model::pane("new", "probe-new"), "last".into());
            target = "new".into();
        }
        match label {
            "constrained-max" => root.find_mut("a").unwrap().max = Some(100.0),
            "constrained-min" => root.find_mut("a").unwrap().min = Some(1000.0),
            "constrained-mixed" => {
                root.find_mut("a").unwrap().min = Some(1100.0);
                root.find_mut("p0").unwrap().max = Some(100.0);
            }
            _ => {}
        }
        let predicted = view.update_in(cx, |this, window, _| this.initial_size(&root, &target, window)).expect(label);
        view.update_in(cx, |this, window, cx| {
            let tab = &mut this.model.spaces[0].tabs[0];
            tab.root = root.clone();
            tab.focus(&target);
            this.sync(window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let actual = view.update(cx, |this, cx| this.terms[&target].view.read(cx).dimensions());
        let delta = (predicted.0.abs_diff(actual.0), predicted.1.abs_diff(actual.1));
        eprintln!("{label}: initial {predicted:?}, rendered {actual:?}, delta {delta:?} cells");
        assert_eq!(predicted, actual, "{label}: initial size must match the rendered grid exactly");
    }
}

#[test]
fn hidden_tabs_follow_the_window() {
    in_own_process("hidden_tabs_follow");
}

#[gpui::test]
fn hidden_tabs_follow(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};
    if !running_alone("hidden_tabs_follow") { return; }
    let main: serde_json::Value = serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap();
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", main);
    // Tab "a" has two panes side by side; tab "b" is the one shown.
    let mut root = crate::model::pane("a", "probe-a");
    root.split("a", super::Axis::Row, crate::model::pane("a2", "probe-a2"), "s".into());
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.model = crate::model::Model::default();
        let mut tab = super::Tab::new("ta", "", "a", "probe-a");
        tab.root = root;
        tab.focus("a2");
        this.model.add_space("space", "test", "/", tab);
        this.model.add_tab("space", super::Tab::new("tb", "", "b", "probe-b"));
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let dims = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, cx| ["a", "a2"].map(|p| this.terms[p].view.read(cx).dimensions()));
    let before = dims(cx);
    cx.simulate_resize(size(px(1400.0), px(900.0)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // The first tick after a change waits for the window to settle; the second sizes them.
    view.update_in(cx, |this, window, cx| this.fit_hidden_tabs(window, cx));
    assert_eq!(dims(cx), before, "a tick right after a change must leave hidden tabs alone");
    view.update_in(cx, |this, window, cx| this.fit_hidden_tabs(window, cx));
    let fitted = dims(cx);
    eprintln!("hidden tab: {before:?} before the window grew, {fitted:?} once it settled");
    assert_ne!(fitted, before, "a settled window must size the hidden tab's terminals");
    // Shown, the panes draw with the size they were given: nothing changes.
    view.update_in(cx, |this, window, cx| {
        this.model.select_tab("ta");
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(dims(cx), fitted, "hidden tabs must have been sized as their panes draw them");
}
