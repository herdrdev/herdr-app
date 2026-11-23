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
fn a_divider_moves_only_its_two_panes() {
    use serde_json::json;
    use std::collections::HashMap;
    let row: super::Node = serde_json::from_value(json!({
        "id": "row", "type": "row", "children": [
            {"id": "w1", "type": "block", "content": {"mod": "probe"}},
            {"id": "a", "type": "block", "content": {"mod": "probe"}, "size": {"px": 700}},
            {"id": "b", "type": "block", "content": {"mod": "probe"}},
            {"id": "w2", "type": "block", "content": {"mod": "probe"}}
        ]
    })).unwrap();
    let current = HashMap::from([
        ("w1".to_owned(), 300.0), ("a".to_owned(), 700.0),
        ("b".to_owned(), 400.0), ("w2".to_owned(), 300.0),
    ]);
    let size = |children: &[super::Node], id: &str| children.iter().find(|n| n.id == id).unwrap().size;
    let px = |px| Some(super::Span::Px { px });
    let weight = |weight| Some(super::Span::Weight { weight });

    // Fixed a on the left of the divider: b and the rest of the row keep their sizes.
    let mut children: Vec<super::Node> = row.children().to_vec();
    super::move_divider(&mut children, "a", "b", 750.0, 1100.0, &current);
    assert_eq!(size(&children, "a"), px(750.0));
    assert_eq!(size(&children, "b"), weight(350.0));
    assert_eq!(size(&children, "w1"), weight(300.0));
    assert_eq!(size(&children, "w2"), weight(300.0));

    // Fixed b on the right: a takes the change, the rest stays put.
    let mut children: Vec<super::Node> = row.children().to_vec();
    children[1].size = weight(700.0);
    children[2].size = px(400.0);
    super::move_divider(&mut children, "a", "b", 450.0, 1100.0, &current);
    assert_eq!(size(&children, "a"), weight(450.0));
    assert_eq!(size(&children, "b"), px(650.0));
    assert_eq!(size(&children, "w1"), weight(300.0));
    assert_eq!(size(&children, "w2"), weight(300.0));

    // Two fixed panes: the pair keeps its total, so nothing else moves either.
    let mut children: Vec<super::Node> = row.children().to_vec();
    children[2].size = px(400.0);
    super::move_divider(&mut children, "a", "b", 750.0, 1100.0, &current);
    assert_eq!(size(&children, "a"), px(750.0));
    assert_eq!(size(&children, "b"), px(350.0));
    assert_eq!(size(&children, "w1"), weight(300.0));
    assert_eq!(size(&children, "w2"), weight(300.0));
}
