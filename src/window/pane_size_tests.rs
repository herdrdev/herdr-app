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
        ("nested", plain, vec![Row, Column, Row]),
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
        assert!(delta.0 <= 1 && delta.1 <= 1, "{label}: initial {predicted:?}, rendered {actual:?}");
    }
}
