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
fn main_sidebar_snaps_while_the_button_is_still_down() {
    in_own_process("sidebar_snap_gesture");
}

#[gpui::test]
fn sidebar_snap_gesture(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton, VisualTestContext, point, px, size};
    use serde_json::json;
    if !running_alone("sidebar_snap_gesture") { return; }
    let probe = r#"import { View, div } from 'gpui-kit'; import { layout, setShared } from 'herdr';
    export default class Probe extends View { render() {
        const width = layout()?.children?.[0]?.size?.px;
        if (width != null && width !== this.last) { this.last = width; setShared('probe.sidebar', width); }
        return div();
    } }"#;
    let layout = json!({"id":"root", "type":"row", "divider":{"width":1,"color":"border"}, "children":[
        {"id":"sidebar","type":"column","size":{"px":268},"min":44,"max":480,"children":[
            {"id":"brand","type":"block","content":{"mod":"workspace-chrome"}}]},
        {"id":"work","type":"column","children":[]}]});
    let (view, cx) = open_window(cx, "workspace-chrome", probe, layout);
    cx.simulate_resize(size(px(900.), px(480.)));
    let draw = |cx: &mut VisualTestContext| { cx.update(|w,cx| w.draw(cx).clear(cx)); cx.run_until_parked(); };
    draw(cx);
    let pos = |x| point(px(x), px(120.));
    let drag = |x, cx: &mut VisualTestContext| { cx.simulate_mouse_move(pos(x), MouseButton::Left, Modifiers::default()); draw(cx); };
    let width = |cx: &mut VisualTestContext| view.read_with(cx, |v,_| match v.chrome.children()[0].size {
        Some(super::Span::Px { px }) => px, _ => panic!("expected a pixel sidebar"),
    });
    let seen = || crate::mods::shared("probe.sidebar").and_then(|n| n.as_f64()).unwrap() as f32;
    let rail = 44.;
    // Same IDs in a personal layout: ordinary continuous resizing, no snapping.
    cx.simulate_mouse_down(pos(268.), MouseButton::Left, Modifiers::default());
    drag(150., cx);
    assert_eq!(width(cx), 150.);
    drag(268., cx);
    cx.simulate_mouse_up(pos(268.), MouseButton::Left, Modifiers::default());
    view.update(cx, |v,_| v.layout_name = "main".into());
    draw(cx);
    cx.simulate_mouse_down(pos(268.), MouseButton::Left, Modifiers::default());
    drag(250., cx);
    assert_eq!(width(cx), 250.);
    let wide = (151..250).rev().step_by(6).find_map(|x| {
        drag(x as f32, cx);
        let w = width(cx);
        (w > x as f32).then_some(w)
    }).expect("the sidebar stops shrinking when its header fits");
    drag(wide - 39., cx);
    assert_eq!(width(cx), wide);
    cx.simulate_mouse_up(pos(wide - 39.), MouseButton::Left, Modifiers::default());
    draw(cx);
    assert_eq!(seen(), wide, "release on the plateau keeps wide mode");
    cx.simulate_mouse_down(pos(wide), MouseButton::Left, Modifiers::default());
    drag(wide - 40., cx);
    assert_eq!((width(cx), seen()), (rail, rail), "collapse before release");
    drag(wide - 39., cx);
    assert_eq!(width(cx), rail, "a one-pixel reversal must not bounce back");
    cx.simulate_mouse_up(pos(wide - 39.), MouseButton::Left, Modifiers::default());
    draw(cx);
    cx.simulate_mouse_down(pos(rail), MouseButton::Left, Modifiers::default());
    drag(rail + 39., cx);
    assert_eq!((width(cx), seen()), (rail, rail), "same pull distance in both directions");
    drag(rail + 40., cx);
    assert_eq!((width(cx), seen()), (wide, wide), "reopen before release");
    drag(rail + 41., cx);
    assert_eq!(width(cx), wide, "the pointer doesn't jump when the divider does");
    drag(245., cx);
    assert_eq!((width(cx), seen()), (245., wide), "follow the pointer, but don't publish every pixel");
    cx.simulate_mouse_up(pos(245.), MouseButton::Left, Modifiers::default());
    draw(cx);
    assert_eq!(seen(), 245.);
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
                        this.drag = Some(super::Drag { container: container.into(), index, axis: super::Axis::Column, sidebar: None });
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
