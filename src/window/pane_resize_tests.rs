#[test]
fn pane_resize_commands_preserve_focus_and_validate_before_mutation() {
    in_own_process("pane_resize_dispatch");
}

#[gpui::test]
fn pane_resize_dispatch(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    use super::{Axis, Span};
    if !running_alone("pane_resize_dispatch") { return; }
    let layout = json!({"id":"context", "type":"block", "content":{"tab":"active"}});
    let (view, cx) = open_window(cx, "resize-probe", "export default () => null;", layout);
    view.update_in(cx, |this, window, cx| {
        this.model = crate::model::Model::default();
        let mut tab = super::Tab::new("tab", "", "a", "probe-a");
        tab.root.split("a", Axis::Row, crate::model::pane("b", "probe-b"), "split".into());
        tab.zoomed = Some("a".into());
        this.model.add_space("space", "test", "/", tab);
        this.documents.get_mut("").unwrap().available = true;
        this.bounds.borrow_mut().insert("context".into(), gpui::Bounds::new(gpui::point(gpui::px(0.0), gpui::px(0.0)), gpui::size(gpui::px(800.0), gpui::px(600.0))));
        let before = this.model.active_tab().unwrap().root.clone();
        for args in [
            json!({}), json!({"direction":"diagonal"}),
            json!({"direction":"right", "amount":0}), json!({"direction":"right", "amount":-1}),
            json!({"direction":"right", "amount":"10"}), json!({"direction":"right", "amount":null}),
            json!({"direction":"right", "amount":1e100}),
            json!({"direction":"right", "pane":123}), json!({"direction":"right", "pane":"unknown"}),
        ] {
            let reply = this.run_as("pane.resize", args.clone(), false, window, cx);
            assert!(reply.get("error").is_some(), "{args}: {reply}");
            let tab = this.model.active_tab().unwrap();
            assert_eq!(tab.root, before);
            assert_eq!(tab.zoomed.as_deref(), Some("a"));
        }
        this.bounds.borrow_mut().remove("context");
        assert!(this.run_as("pane.resize:right", json!({}), false, window, cx).get("error").is_some());
        assert_eq!(this.model.active_tab().unwrap().zoomed.as_deref(), Some("a"));
        this.bounds.borrow_mut().insert("context".into(), gpui::Bounds::new(gpui::point(gpui::px(0.0), gpui::px(0.0)), gpui::size(gpui::px(800.0), gpui::px(600.0))));
        let focused = this.model.focused_pane();
        let addresses: Vec<_> = this.model.active_tab().unwrap().root.terminals().iter().map(|n| n.terminal().unwrap().to_owned()).collect();
        for _ in 0..2 {
            assert_eq!(this.run_as("pane.resize:right", json!({}), false, window, cx), json!({"ok":true}));
        }
        let tab = this.model.active_tab().unwrap();
        let parent = tab.root.parent_of("a").unwrap();
        let available = this.split_available(tab, parent).unwrap();
        let children: Vec<_> = parent.children().iter().collect();
        let measured = super::pane_size::shares(&children, available, 1.0);
        assert!((measured[0] - available / 2.0 - 20.0).abs() < 0.01, "{measured:?}");
        assert_eq!(tab.zoomed, None);
        assert_eq!(this.model.focused_pane(), focused);
        assert_eq!(tab.root.terminals().iter().map(|n| n.terminal().unwrap().to_owned()).collect::<Vec<_>>(), addresses);
        assert!(matches!(tab.root.find("a").unwrap().size, Some(Span::Weight { .. })));
        let resized = tab.root.clone();
        this.documents.get_mut("").unwrap().available = false;
        assert!(this.run_as("pane.resize:left", json!({}), false, window, cx).get("error").is_some());
        assert_eq!(this.model.active_tab().unwrap().root, resized);
    });
}
