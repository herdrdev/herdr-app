const PROBE: &str = r#"import { View, div } from "gpui-kit";
export default class extends View {
  render() {
    return div().id("root").size_full();
  }
}
"#;

fn open_probe_window<'a>(cx: &'a mut gpui::TestAppContext, spec: serde_json::Value) -> (gpui::Entity<super::AppWindow>, &'a mut gpui::VisualTestContext) {
    let (view, cx) = open_window(cx, "probe", PROBE, serde_json::json!({
        "id": "root", "type": "block", "content": { "tab": "active" }
    }));
    cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(600.0)));
    view.update_in(cx, |this, window, cx| this.open_overlay("w".into(), spec, window, cx));
    redraw(cx);
    (view, cx)
}

/// Twice: a window is measured on its first frame and shown on the next.
fn redraw(cx: &mut gpui::VisualTestContext) {
    for _ in 0..2 {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
    }
}

/// The window as drawn, border included, rounded to whole px.
fn drawn(view: &gpui::Entity<super::AppWindow>, cx: &mut gpui::VisualTestContext) -> [f32; 4] {
    view.update(cx, |this, _| {
        let o = this.overlays.iter().find(|o| o.id == "w").unwrap();
        let b = this.bounds.borrow()[&o.slot];
        let b = if o.bare { b } else { b.dilate(gpui::px(1.0)) };
        [b.origin.x, b.origin.y, b.size.width, b.size.height].map(|v| f32::from(v).round())
    })
}

fn drag(cx: &mut gpui::VisualTestContext, from: (f32, f32), to: (f32, f32)) {
    use gpui::{Modifiers, MouseButton, point, px};
    cx.simulate_mouse_move(point(px(from.0), px(from.1)), None, Modifiers::default());
    cx.simulate_mouse_down(point(px(from.0), px(from.1)), MouseButton::Left, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_move(point(px(to.0), px(to.1)), MouseButton::Left, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_up(point(px(to.0), px(to.1)), MouseButton::Left, Modifiers::default());
    redraw(cx);
}

fn stored() -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(crate::mods::app_dir().join("windows.json")).unwrap_or_else(|_| "{}".into())).unwrap()
}

#[test]
fn a_moved_window_lands_exactly_where_it_was_dropped() {
    in_own_process("window_move_exact");
}

#[gpui::test]
fn window_move_exact(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("window_move_exact") {
        return;
    }
    let spec = json!({ "kind": "window", "mod": "probe", "key": "moved", "width": 300, "height": 200, "at": { "x": 100, "y": 100 } });
    let (view, cx) = open_probe_window(cx, spec);
    assert_eq!(drawn(&view, cx), [100.0, 100.0, 300.0, 200.0]);
    drag(cx, (200.0, 150.0), (250.0, 200.0));
    assert_eq!(drawn(&view, cx), [150.0, 150.0, 300.0, 200.0], "moved by the drag exactly, border included");
    assert_eq!(stored()["places"]["moved"], json!([150.0, 150.0]));
}

#[test]
fn a_window_released_outside_the_app_stays_where_it_was_dropped() {
    in_own_process("window_move_released_outside");
}

#[gpui::test]
fn window_move_released_outside(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, MouseButton, point, px};
    use serde_json::json;
    if !running_alone("window_move_released_outside") {
        return;
    }
    let spec = json!({ "kind": "window", "mod": "probe", "key": "moved", "width": 300, "height": 200, "at": { "x": 100, "y": 100 } });
    let (view, cx) = open_probe_window(cx, spec);
    // No mouse up comes: the next move, back over the app, has no button.
    let (from, to) = (point(px(200.0), px(150.0)), point(px(250.0), px(200.0)));
    cx.simulate_mouse_move(from, None, Modifiers::default());
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_move(to, None, Modifiers::default());
    redraw(cx);
    assert!(view.update(cx, |this, _| this.win_drag.is_none()));
    assert_eq!(drawn(&view, cx), [150.0, 150.0, 300.0, 200.0]);
    assert_eq!(stored()["places"]["moved"], json!([150.0, 150.0]));
}
