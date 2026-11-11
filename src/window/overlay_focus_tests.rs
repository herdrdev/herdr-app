use crate::terminal::test_support::record_input;

#[test]
fn popup_terminals_receive_keys_and_return_focus() {
    in_own_process("popup_terminal_keys");
}

#[gpui::test]
fn popup_terminal_keys(cx: &mut gpui::TestAppContext) {
    use serde_json::json;
    if !running_alone("popup_terminal_keys") {
        return;
    }
    cx.update(gpui_base::init);
    let (view, cx) = open_window(cx, "autofocus", r#"
import { View } from "gpui-kit";
import { InputState, Input } from "gpui-base";
export default class FocusPopup extends View {
  init() {
    this.input = InputState.new({ value: "" });
    this.input.focus();
  }
  render() { return Input.new(this.input); }
}
"#, json!({
        "id": "root", "type": "block", "content": {"tab": "active"}
    }));
    let background = view.update_in(cx, |this, window, cx| {
        this.model.add_space("s1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
        this.sync(window, cx);
        this.pane_term("p1").unwrap().view.update(cx, |t, _| record_input(t))
    });
    let bytes = |frames: &std::sync::mpsc::Receiver<Vec<u8>>| -> Vec<u8> {
        frames.try_iter().filter(|f| f[4] == crate::hc::DATA).flat_map(|f| f[9..].to_vec()).collect()
    };
    cx.run_until_parked();
    cx.simulate_keystrokes("a");
    assert_eq!(bytes(&background), b"a");

    for kind in ["popup", "dialog"] {
        let (popup, second) = view.update_in(cx, |this, window, cx| {
            // A launcher opens a terminal, then closes itself in finally.
            this.open_overlay("launcher".into(), json!({"kind": "popup"}), window, cx);
            this.open_overlay("terminal".into(), json!({
                "kind": kind, "width": 600, "height": 400,
                "layout": {"type": "row", "children": [
                    {"type": "block", "hidden": true, "content": {"terminal": "hidden"}},
                    {"id": "first", "type": "block", "content": {"terminal": "unowned"}},
                    {"id": "second", "type": "block", "content": {"terminal": "second"}}
                ]}
            }), window, cx);
            let frames = this.pane_term("ovterminal/first").unwrap().view.update(cx, |t, _| record_input(t));
            let second = this.pane_term("ovterminal/second").unwrap().view.update(cx, |t, _| record_input(t));
            this.close_overlay("launcher", window, cx);
            (frames, second)
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("x escape");
        assert_eq!(bytes(&popup), b"x\x1b", "{kind}: typing and Esc must reach the popup program without a click");
        assert!(bytes(&background).is_empty());
        assert!(bytes(&second).is_empty());
        view.update_in(cx, |this, window, cx| this.run("pane.focus", json!({"terminal": "second"}), window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("s");
        assert_eq!(bytes(&second), b"s", "pane.focus must target a terminal in the top modal overlay");
        for spec in [
            json!({"kind": "popup", "mod": "autofocus"}),
            json!({"kind": "popup", "layout": {"type": "block", "content": {"mod": "autofocus"}}}),
        ] {
            view.update_in(cx, |this, window, cx| {
                let slot = if spec.get("layout").is_some() { "ovquick/1" } else { "overlay:quick" };
                this.open_overlay("quick".into(), spec, window, cx);
                assert!(this.copies[slot].error.is_none());
                this.close_overlay("quick", window, cx);
            });
            cx.run_until_parked();
            cx.simulate_keystrokes("r");
            assert_eq!(bytes(&second), b"r", "a self-focusing popup closed before rendering must restore the second terminal");
            assert!(bytes(&popup).is_empty());
        }
        for menu_kind in ["popup", "dialog"] {
            view.update_in(cx, |this, window, cx| {
                assert!(this.overlays.iter().any(|o| o.id == "terminal"), "Esc must not dismiss a terminal layout");
                this.open_overlay("menu".into(), json!({"kind": menu_kind}), window, cx);
            });
            cx.run_until_parked();
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            view.read_with(cx, |this, _| assert!(!this.overlays.iter().any(|o| o.id == "menu")));
            cx.simulate_keystrokes("y");
            assert_eq!(bytes(&second), b"y", "closing a menu must return to the previously focused overlay terminal, not the first");
            assert!(bytes(&popup).is_empty());
        }
        view.update_in(cx, |this, window, cx| this.close_overlay("terminal", window, cx));
        cx.run_until_parked();
        cx.simulate_keystrokes("z");
        assert_eq!(bytes(&background), b"z", "closing the terminal popup must restore the pane");
    }

    let floating = view.update_in(cx, |this, window, cx| {
        this.open_overlay("floating".into(), json!({
            "kind": "window", "width": 600, "height": 400,
            "layout": {"type": "block", "content": {"terminal": "unowned"}}
        }), window, cx);
        this.pane_term("ovfloating/1").unwrap().view.update(cx, |t, _| record_input(t))
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("b");
    assert_eq!(bytes(&background), b"b", "windows must not steal focus on opening");
    assert!(bytes(&floating).is_empty());
    let next_background = view.update_in(cx, |this, window, cx| {
        this.model.split("p1", crate::layout::Axis::Row, "p2", "t2");
        this.sync(window, cx);
        let frames = this.pane_term("p2").unwrap().view.update(cx, |t, _| record_input(t));
        this.run("pane.focus", json!({"pane": "p2"}), window, cx);
        frames
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("n");
    assert_eq!(bytes(&next_background), b"n");
    view.update_in(cx, |this, window, cx| this.run("pane.focus", json!({"terminal": "unowned"}), window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("c escape");
    assert_eq!(bytes(&floating), b"c\x1b", "pane.focus must reach an unowned overlay terminal");
    assert!(bytes(&background).is_empty());
    view.update_in(cx, |this, window, cx| {
        this.open_overlay("menu".into(), json!({"kind": "popup"}), window, cx);
        this.run("pane.focus", json!({"terminal": "unowned"}), window, cx);
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("q");
    assert!(bytes(&floating).is_empty(), "pane.focus must not bypass a modal popup");
    assert!(bytes(&background).is_empty());
    view.update_in(cx, |this, window, cx| this.close_overlay("menu", window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("d");
    assert_eq!(bytes(&floating), b"d", "a menu must return focus to the floating terminal");
    view.update_in(cx, |this, window, cx| this.open_overlay("toast".into(), json!({"kind": "toast"}), window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("f");
    assert_eq!(bytes(&floating), b"f", "a toast must not steal the keyboard");
    view.update_in(cx, |this, window, cx| this.close_overlay("toast", window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("g");
    assert_eq!(bytes(&floating), b"g", "closing a toast must leave the keyboard alone");
    view.update_in(cx, |this, window, cx| this.close_overlay("floating", window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("e");
    assert_eq!(bytes(&next_background), b"e", "closing a window must return to the current pane, not its opening-time pane");
    assert!(bytes(&background).is_empty());
    let other_tab = view.update_in(cx, |this, window, cx| {
        this.open_overlay("switching".into(), json!({
            "kind": "popup", "layout": {"type": "block", "content": {"terminal": "unowned"}}
        }), window, cx);
        this.model.add_tab("s1", crate::model::Tab::new("tb2", "", "p3", "t3"));
        this.model.select_tab("tb2");
        this.sync(window, cx);
        this.pane_term("p3").unwrap().view.update(cx, |t, _| record_input(t))
    });
    cx.run_until_parked();
    view.update_in(cx, |this, window, cx| this.close_overlay("switching", window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("h");
    assert_eq!(bytes(&other_tab), b"h", "closing a popup must not reactivate its old tab");
    assert!(bytes(&next_background).is_empty());
}
