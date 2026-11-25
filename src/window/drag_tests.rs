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

#[test]
fn double_clicking_a_divider_evens_out_its_row() {
    in_own_process("divider_double_click");
}

#[gpui::test]
fn divider_double_click(cx: &mut gpui::TestAppContext) {
    use gpui::{point, px, size, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};
    use serde_json::json;
    if !running_alone("divider_double_click") { return; }

    let block = |id: &str, size: serde_json::Value| json!({ "id": id, "type": "block", "size": size, "content": { "mod": "even-probe" } });
    let layout = json!({
        "id": "row", "type": "row", "titlebar": "none",
        "children": [
            block("fixed", json!({ "px": 40 })),
            block("a", json!({ "weight": 2 })),
            block("b", json!({ "weight": 1 })),
            block("c", json!({ "weight": 1 }))
        ]
    });
    let (view, cx) = open_window(cx, "even-probe", "export default () => null;", layout);
    cx.simulate_resize(size(px(440.0), px(200.0)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();

    let sizes = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |this, _| {
        this.chrome.children().iter().map(|n| n.size).collect::<Vec<_>>()
    });
    let before = sizes(cx);

    // The divider between a (200px) and b (100px) sits at x = 40 + 200.
    let at = point(px(240.0), px(100.0));
    let click = |click_count: usize, cx: &mut gpui::VisualTestContext| {
        cx.simulate_event(MouseDownEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count, first_mouse: false });
        cx.simulate_event(MouseUpEvent { button: MouseButton::Left, position: at, modifiers: Modifiers::default(), click_count });
        cx.run_until_parked();
    };
    click(1, cx);
    assert_eq!(sizes(cx), before, "a single click without moving must not resize");

    // The second press of a double-click is what evens out the row.
    click(2, cx);
    assert!(view.read_with(cx, |this, _| this.drag.is_none()), "a double-click does not leave a drag behind");
    let sizes = sizes(cx);
    assert_eq!(sizes[0], Some(super::Span::Px { px: 40.0 }), "fixed blocks keep their size");
    let w = 4.0 / 3.0;
    for s in &sizes[1..] {
        assert!(matches!(s, Some(super::Span::Weight { weight }) if (weight - w).abs() < 1e-5), "{s:?}");
    }
}
