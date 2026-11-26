#[test]
fn the_shipped_tab_follows_the_pointer_before_reordering() {
    in_own_process("tab_strip_drag");
}

#[gpui::test]
fn tab_strip_drag(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px, size};
    use serde_json::json;
    if !running_alone("tab_strip_drag") { return; }

    let (view, cx) = open_window(cx, "tabs", include_str!("../../defaults/mods/tabs/main.js"), json!({
        "id": "root", "type": "column", "titlebar": "none", "children": [
            { "id": "tabs", "type": "block", "size": { "px": 42 }, "content": { "mod": "tabs" } },
            { "id": "body", "type": "block", "content": { "tab": "active" } }
        ]
    }));
    view.update_in(cx, |this, window, cx| {
        this.model.add_space("space", "test", "/", super::Tab::new("t1", "", "first", "term-a"));
        this.model.add_tab_unselected("space", super::Tab::new("t2", "", "second", "term-b"));
        this.model.add_tab_unselected("space", super::Tab::new("t3", "", "third", "term-c"));
        this.documents.entry(String::new()).or_default().available = true;
        this.write_person(|p| p.zoom = 1.25, window, cx);
        this.sync(window, cx);
    });
    cx.simulate_resize(size(px(800.), px(240.)));
    let draw = |cx: &mut gpui::VisualTestContext| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    };
    let order = |cx: &mut gpui::VisualTestContext| {
        view.read_with(cx, |this, _| this.model.active_space().unwrap().tabs.iter().map(|t| t.id.clone()).collect::<Vec<_>>())
    };
    draw(cx);
    let start = cx.update(|window, _| {
        let raised = super::c("raised").into();
        let quads = window.painted_quads();
        let tab = quads.iter().find(|q| q.background == raised && q.corner_radii.top_left.0 > 0.)
            .expect("the active tab should be painted");
        let scale = window.scale_factor();
        point(px((tab.bounds.origin.x.0 + tab.bounds.size.width.0 / 4.) / scale),
            px((tab.bounds.origin.y.0 + tab.bounds.size.height.0 / 2.) / scale))
    });
    let end = start + point(px(370.), px(2.));
    let none = Modifiers::default();
    let drag = |cx: &mut gpui::VisualTestContext| {
        cx.simulate_mouse_down(start, MouseButton::Left, none);
        draw(cx);
        cx.simulate_mouse_move(end, MouseButton::Left, none);
        draw(cx);
    };
    drag(cx);
    assert_eq!(order(cx), ["t1", "t2", "t3"]);
    cx.update(|window, _| {
        let quads = window.painted_quads();
        assert!(quads.iter().any(|q| q.border_color == super::c("accent") && q.border_widths.top.0 > 0.),
            "drop slot is painted");
        let pointer = end.scale(window.scale_factor());
        let hover = super::c("hover").into();
        assert!(quads.iter().any(|q| q.background == hover && q.bounds.contains(&pointer)), "held tab is painted under the pointer");
    });
    let outside = end + point(px(0.), px(80.));
    cx.simulate_mouse_move(outside, MouseButton::Left, none);
    draw(cx);
    cx.simulate_mouse_up(outside, MouseButton::Left, none);
    cx.run_until_parked();
    assert_eq!(order(cx), ["t1", "t2", "t3"]);

    drag(cx);
    cx.simulate_mouse_up(end, MouseButton::Left, none);
    cx.run_until_parked();
    assert_eq!(order(cx), ["t2", "t3", "t1"]);
}

#[test]
fn dividers_follow_the_current_layout() {
    in_own_process("divider_interaction");
}

#[gpui::test]
fn divider_interaction(cx: &mut gpui::TestAppContext) {
    use gpui::{point, px, size, Background, Modifiers, MouseButton, VisualTestContext};
    use serde_json::json;
    if !running_alone("divider_interaction") { return; }

    let layout = |left: f32| json!({
        "id": "split", "type": "row", "titlebar": "none",
        "divider": { "color": "#00000000", "hover": "#2266aa", "active": "#cc3399" },
        "children": [
            { "id": "left", "type": "block", "size": { "px": left }, "content": { "mod": "divider-probe" } },
            { "id": "right", "type": "column", "divider": { "color": "#00000000", "hover": "#22aa66" }, "children": [
                { "id": "top", "type": "block", "size": { "px": 100 }, "content": { "mod": "divider-probe" } },
                { "id": "bottom", "type": "block", "content": { "mod": "divider-probe" } }
            ] }
        ]
    });
    let (view, cx) = open_window(cx, "divider-probe", "export default () => null;", layout(120.0));
    std::fs::write(crate::mods::app_dir().join("layouts/other.json"), layout(160.0).to_string()).unwrap();
    cx.simulate_resize(size(px(400.0), px(240.0)));

    let draw = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    };
    let active_line = |cx: &mut VisualTestContext| {
        draw(cx);
        cx.update(|window, _| {
            let color: Background = gpui::rgb(0xcc3399).into();
            window.painted_quads().iter().any(|quad| quad.background == color)
        })
    };
    let hovered = |cx: &mut VisualTestContext| {
        draw(cx);
        view.read_with(cx, |this, _| this.divider_hover.iter().any(|(id, index)| id == "split" && *index == 0))
    };
    let switch = |name, cx: &mut VisualTestContext| {
        view.update_in(cx, |this, window, cx| this.switch_layout(Some(name), window, cx));
    };

    draw(cx);
    let edge = point(px(119.0), px(90.0));
    cx.simulate_mouse_move(edge, None, Modifiers::default());
    assert!(hovered(cx));
    cx.simulate_mouse_move(point(px(122.0), px(100.0)), None, Modifiers::default());
    draw(cx);
    assert_eq!(view.read_with(cx, |this, _| this.divider_hover.len()), 2, "both handles cover the junction");
    cx.simulate_mouse_move(edge, None, Modifiers::default());
    switch("test", cx);
    assert!(hovered(cx), "reselecting the layout must keep hover under the stationary pointer");

    view.update_in(cx, |this, window, cx| this.run("layout.hide", json!({ "id": "right" }), window, cx));
    draw(cx);
    let elsewhere = point(px(200.0), px(90.0));
    cx.simulate_mouse_move(elsewhere, None, Modifiers::default());
    view.update_in(cx, |this, window, cx| this.run("layout.show", json!({ "id": "right" }), window, cx));
    assert!(!hovered(cx), "showing the divider again away from the pointer must clear its highlight");
    cx.simulate_mouse_move(edge, None, Modifiers::default());
    assert!(hovered(cx), "the shown divider still responds to hover");

    cx.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
    assert!(active_line(cx), "the active color appears before the first drag motion");
    switch("other", cx);
    assert!(!active_line(cx), "the new layout's divider cannot inherit the old drag");
    cx.simulate_mouse_move(elsewhere, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(elsewhere, MouseButton::Left, Modifiers::default());
    assert_eq!(
        view.read_with(cx, |this, _| this.chrome.children()[0].size),
        Some(super::Span::Px { px: 160.0 }),
        "the remainder of that gesture cannot resize the new layout",
    );
}

#[test]
fn dragging_squeezed_layout_blocks_never_crashes() {
    in_own_process("squeezed_layout_drag");
}

#[gpui::test]
fn squeezed_layout_drag(cx: &mut gpui::TestAppContext) {
    use gpui::{point, px, size};
    use serde_json::json;
    if !running_alone("squeezed_layout_drag") { return; }
    let block = |id: &str, height: f32| json!({
        "id": id, "type": "block", "size": {"px": height}, "content": {"mod": "drag-probe"}
    });
    let layout = json!({"id": "root", "type": "row", "children": [
        {"id": "sidebar", "type": "column", "size": {"px": 268}, "resizable": false, "children": [
            block("brand", 46.0), block("button", 52.0),
            {"id": "lists", "type": "column", "divider": {"width": 1, "color": "border"}, "children": [
                block("heading", 34.0),
                {"id": "stack", "type": "stack", "children": [
                    {"id": "list", "type": "block", "content": {"mod": "drag-probe"}}
                ]},
                block("zero1", 0.0), block("zero2", 0.0), block("zero3", 0.0), block("zero4", 0.0), block("zero5", 0.0)
            ]},
            block("pendientes", 200.0), block("vistas", 46.0)
        ]}
    ]});
    let (view, cx) = open_window(cx, "drag-probe", "export default () => null;", layout.clone());
    for height in [800.0, 380.0, 344.0, 300.0] {
        cx.simulate_resize(size(px(900.0), px(height)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        view.update(cx, |this, _| {
            for (container, count) in [("sidebar", 4), ("lists", 6)] {
                for index in 0..count {
                    for y in [-2000.0, 0.0, 2000.0] {
                        this.chrome = serde_json::from_value(layout.clone()).unwrap();
                        let parent = this.chrome.find(container).unwrap();
                        let a = &parent.children()[index];
                        let b = &parent.children()[index + 1];
                        let bounds = this.bounds.borrow();
                        let total = f32::from(bounds[&a.id].size.height + bounds[&b.id].size.height) / super::settings::zoom();
                        drop(bounds);
                        let before = serde_json::to_value(parent).unwrap();
                        this.drag = Some(super::Drag { container: container.into(), index, axis: super::Axis::Column });
                        this.drag_to(point(px(100.0), px(y)));
                        let after = this.chrome.find(container).unwrap();
                        if container == "sidebar" || total <= 48.0 {
                            assert_eq!(serde_json::to_value(after).unwrap(), before, "{container}[{index}], height {height}");
                        } else {
                            for child in &after.children()[index..=index + 1] {
                                if let Some(super::Span::Px { px }) = child.size {
                                    assert!(px >= 0.0 && px <= total, "{}: {px} out of {total}", child.id);
                                }
                            }
                        }
                    }
                }
            }
            this.drag = None;
        });
    }
}
