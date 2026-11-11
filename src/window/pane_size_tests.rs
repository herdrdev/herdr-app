#[test]
fn a_pane_hidden_before_first_paint_still_attaches() {
    in_own_process("unpainted_hidden_pane");
}

#[gpui::test]
fn unpainted_hidden_pane(cx: &mut gpui::TestAppContext) {
    if !running_alone("unpainted_hidden_pane") { return; }
    let plain = serde_json::json!({"id": "context", "type": "block", "content": {"tab": "active"}});
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", plain);
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.model = crate::model::Model::default();
        let mut tab = super::Tab::new("tab", "", "a", "probe-a");
        tab.root.split("a", super::Axis::Row, crate::model::pane("b", "probe-b"), "split".into());
        this.model.add_space("space", "test", "/", tab);
        this.sync(window, cx);
        assert!(!this.terms["b"].view.read(cx).lost);
        this.model.tab_mut("tab").unwrap().zoomed = Some("a".into());
        this.sync(window, cx);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    assert!(view.update(cx, |this, cx| this.terms["b"].view.read(cx).lost),
        "the hidden pane must attempt to attach without ever being painted");
}

#[test]
fn a_covered_pane_attaches_without_painting() {
    in_own_process("covered_pane_deadline");
}

#[gpui::test]
fn covered_pane_deadline(cx: &mut gpui::TestAppContext) {
    if !running_alone("covered_pane_deadline") { return; }
    let plain = serde_json::json!({"id": "context", "type": "block", "content": {"tab": "active"}});
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", plain);
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.model = crate::model::Model::default();
        this.model.add_space("space", "test", "/", super::Tab::new("tab", "", "a", "probe-a"));
        this.sync(window, cx);
        this.live.insert("replacement".into(), serde_json::json!({"state": "live"}));
        this.swapped.insert("a".into(), "replacement".into());
        this.sync(window, cx);
    });
    cx.run_until_parked();
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    assert!(view.update(cx, |this, cx| this.terms["a"].view.read(cx).lost),
        "a covered original pane must still attempt to attach");
}

#[test]
fn an_agent_only_layout_fits_its_pane_before_attaching() {
    in_own_process("agent_only_first_fit");
}

#[gpui::test]
fn agent_only_first_fit(cx: &mut gpui::TestAppContext) {
    if !running_alone("agent_only_first_fit") { return; }
    let layout = serde_json::json!({"id": "agent", "type": "block", "content": {"agent": "focused"}});
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", layout);
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.model = crate::model::Model::default();
        this.model.add_space("space", "test", "/", super::Tab::new("tab", "", "a", "probe-a"));
        this.live.insert("probe-a".into(), serde_json::json!({"state": "live"}));
        this.agent = Some("probe-a".into());
        this.sync(window, cx);
        assert!(!this.terms["a"].view.read(cx).lost);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    view.update(cx, |this, cx| {
        let terminal = this.terms["a"].view.read(cx);
        assert!(terminal.lost, "the drawn agent pane must attach before the deadline");
        assert_ne!(terminal.dimensions(), (120, 40), "replay must use agent-block layout");
    });
}

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

#[test]
fn other_spaces_keep_their_size() {
    in_own_process("other_spaces_keep_size");
}

#[gpui::test]
fn other_spaces_keep_size(cx: &mut gpui::TestAppContext) {
    use gpui::{px, size};
    if !running_alone("other_spaces_keep_size") { return; }
    let main: serde_json::Value = serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap();
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", main);
    // Space "other" may be on screen in another window; "space" is the one shown here.
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.model = crate::model::Model::default();
        this.live.insert("probe-a".into(), serde_json::json!({"state": "live", "cols": 77, "rows": 33}));
        this.model.add_space("other", "other", "/", super::Tab::new("ta", "", "a", "probe-a"));
        this.model.add_space("space", "test", "/", super::Tab::new("tb", "", "b", "probe-b"));
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let dims = |cx: &mut gpui::VisualTestContext| view.update(cx, |this, cx| this.terms["a"].view.read(cx).dimensions());
    let before = dims(cx);
    cx.simulate_resize(size(px(1400.0), px(900.0)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    view.update_in(cx, |this, window, cx| this.fit_hidden_tabs(window, cx));
    view.update_in(cx, |this, window, cx| this.fit_hidden_tabs(window, cx));
    assert_eq!(dims(cx), before, "a settled window must leave other spaces' terminals alone");
}

#[test]
fn other_spaces_attach_at_their_own_size() {
    in_own_process("other_spaces_attach_size");
}

#[gpui::test]
fn other_spaces_attach_size(cx: &mut gpui::TestAppContext) {
    if !running_alone("other_spaces_attach_size") { return; }
    let main: serde_json::Value = serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap();
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", main);
    // Before the first listing, attaching "probe-a" would set it to a guessed size.
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.model = crate::model::Model::default();
        this.live.clear();
        this.model.add_space("other", "other", "/", super::Tab::new("ta", "", "a", "probe-a"));
        this.model.add_space("space", "test", "/", super::Tab::new("tb", "", "b", "probe-b"));
        this.sync(window, cx);
    });
    view.update(cx, |this, _| {
        assert!(!this.terms.contains_key("a"), "another space's terminal must wait for its size");
        assert!(this.terms.contains_key("b"), "the shown space's terminal attaches right away");
    });
    view.update_in(cx, |this, window, cx| {
        let listed = serde_json::json!({"terminals": [
            {"address": "probe-a", "state": "live", "cols": 77, "rows": 33},
            {"address": "probe-b", "state": "live", "cols": 90, "rows": 30},
        ]});
        this.terminals_listed(listed, window, cx);
    });
    let dims = view.update(cx, |this, cx| this.terms["a"].view.read(cx).dimensions());
    assert_eq!(dims, (77, 33), "once listed, it attaches at the size the core reports");
}

#[test]
fn a_space_shown_again_tells_the_core_its_size() {
    in_own_process("shown_again_resizes");
}

#[gpui::test]
fn shown_again_resizes(cx: &mut gpui::TestAppContext) {
    use crate::hc::Sent;
    if !running_alone("shown_again_resizes") { return; }
    let main: serde_json::Value = serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap();
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", main);
    let (core, sent) = crate::hc::recording();
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.core = core;
        this.model = crate::model::Model::default();
        this.model.add_space("two", "two", "/", super::Tab::new("tb", "", "b", "probe-b"));
        this.model.add_space("one", "one", "/", super::Tab::new("ta", "", "a", "probe-a"));
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    let select = |space: &str, cx: &mut gpui::VisualTestContext| {
        view.update_in(cx, |this, window, cx| {
            this.model.active = Some(space.into());
            this.sync(window, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    };
    // Another window may size "a" while this one shows space "two".
    select("two", cx);
    let attaches = sent.try_iter().filter(|s| matches!(s, Sent::Attach(p) if p["terminal"] == "probe-a")).count();
    assert_eq!(attaches, 1, "a attached once");
    select("one", cx);
    let dims = view.update(cx, |this, cx| this.terms["a"].view.read(cx).dimensions());
    let resize = std::iter::from_fn(|| sent.recv_timeout(std::time::Duration::from_secs(2)).ok()).find_map(|s| match s { Sent::Resize(c, r) => Some((c, r)), _ => None });
    assert_eq!(resize, Some(dims), "a terminal back on screen must tell the core its size");
}

#[test]
fn reattaching_leaves_other_spaces_their_size() {
    in_own_process("reattach_resumed");
}

#[gpui::test]
fn reattach_resumed(cx: &mut gpui::TestAppContext) {
    use crate::hc::Sent;
    if !running_alone("reattach_resumed") { return; }
    let main: serde_json::Value = serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap();
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", main);
    let (core, _) = crate::hc::recording();
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.core = core;
        this.model = crate::model::Model::default();
        this.live.insert("probe-b".into(), serde_json::json!({"state": "live", "cols": 77, "rows": 33}));
        this.model.add_space("two", "two", "/", super::Tab::new("tb", "", "b", "probe-b"));
        this.model.add_space("one", "one", "/", super::Tab::new("ta", "", "a", "probe-a"));
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    // A restarted core: "b" may be on screen in another window.
    let (core, sent) = crate::hc::recording();
    view.update(cx, |this, cx| this.reconnected(core, cx));
    cx.run_until_parked();
    let resumed: std::collections::BTreeMap<String, serde_json::Value> = sent.try_iter()
        .filter_map(|s| match s { Sent::Attach(p) => Some((p["terminal"].as_str().unwrap().to_owned(), p["resumed"].clone())), _ => None })
        .collect();
    assert_eq!(resumed["probe-b"], true, "a terminal off screen here must not take the size back");
    assert_eq!(resumed["probe-a"], false, "a terminal on screen here sets its size");
}

#[test]
fn other_spaces_attach_without_taking_the_size() {
    in_own_process("other_spaces_attach_resumed");
}

#[gpui::test]
fn other_spaces_attach_resumed(cx: &mut gpui::TestAppContext) {
    use crate::hc::Sent;
    if !running_alone("other_spaces_attach_resumed") { return; }
    let main: serde_json::Value = serde_json::from_str(include_str!("../../defaults/layouts/main.json")).unwrap();
    let (view, cx) = open_window(cx, "size-probe", "export default () => null;", main);
    let (core, sent) = crate::hc::recording();
    // "b" may be on screen in another window that started first.
    view.update_in(cx, |this, window, cx| {
        this.folders.retain(|id, _| id == "size-probe");
        this.core = core;
        this.model = crate::model::Model::default();
        this.live.insert("probe-b".into(), serde_json::json!({"state": "live", "cols": 77, "rows": 33}));
        this.model.add_space("two", "two", "/", super::Tab::new("tb", "", "b", "probe-b"));
        this.model.add_space("one", "one", "/", super::Tab::new("ta", "", "a", "probe-a"));
        this.sync(window, cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.executor().advance_clock(std::time::Duration::from_millis(100));
    cx.run_until_parked();
    let resumed: std::collections::BTreeMap<String, serde_json::Value> = sent.try_iter()
        .filter_map(|s| match s { Sent::Attach(p) => Some((p["terminal"].as_str().unwrap().to_owned(), p["resumed"].clone())), _ => None })
        .collect();
    assert_eq!(resumed["probe-b"], true, "another space's terminal must not take the size from the window showing it");
    assert_eq!(resumed["probe-a"], false, "the shown space's terminal sets its size");
}

#[test]
fn shown_while_attaching_tells_the_core_once_attached() {
    in_own_process("shown_before_attached");
}

#[gpui::test]
fn shown_before_attached(cx: &mut gpui::TestAppContext) {
    use crate::hc::Sent;
    use gpui::{px, size};
    if !running_alone("shown_before_attached") { return; }
    let (core, sent) = crate::hc::recording();
    let view = cx.new(|cx| crate::terminal::TerminalView::attach(core, "probe".into(), (80, 24), cx));
    view.update(cx, |v, _| v.resumed = true);
    // Its space is shown while its resumed attach is on the way.
    crate::terminal::next_frame();
    crate::terminal::next_frame();
    view.read_with(cx, |v, _| {
        v.mark_shown();
        v.fit(80, 24, size(px(8.), px(18.)), 1.);
    });
    cx.run_until_parked();
    assert!(matches!(sent.try_recv(), Ok(Sent::Attach(p)) if p["resumed"] == true));
    view.read_with(cx, |v, _| v.fit(80, 24, size(px(8.), px(18.)), 1.));
    let resize = std::iter::from_fn(|| sent.recv_timeout(std::time::Duration::from_secs(2)).ok()).find_map(|s| match s { Sent::Resize(c, r) => Some((c, r)), _ => None });
    assert_eq!(resize, Some((80, 24)), "once attached, a terminal shown meanwhile must tell the core its size");
}
