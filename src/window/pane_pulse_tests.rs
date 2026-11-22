#[test]
fn pane_typing_pulse_routes_events_and_restarts_at_expiry() {
    in_own_process("typing_pulse_lifecycle");
}

#[gpui::test]
fn typing_pulse_lifecycle(cx: &mut TestAppContext) {
    use super::pane_pulse::DURATION;
    use crate::terminal::{KeyboardInputQueued, TerminalView};
    use gpui::Entity;
    use serde_json::json;
    use std::time::Duration;

    if !running_alone("typing_pulse_lifecycle") {
        return;
    }
    let (view, cx) = open_window(cx, "unused", "", json!({
        "id": "tab", "type": "block",
        "content": { "tab": "active", "pane": {
            "lone": true, "focus": { "width": 2, "color": "accent" }
        }}
    }));
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();

    let first = cx.update(|window, cx| view.update(cx, |app, cx| {
        app.model.add_space("sp1", "", "/tmp", crate::model::Tab::new("tb1", "", "p1", "t1"));
        app.sync(window, cx);
        app.pane_term("p1").unwrap().view.clone()
    }));
    cx.run_until_parked();

    let pulse = |cx: &mut gpui::VisualTestContext| view.read_with(cx, |app, _| {
        app.typing_pulse.as_ref().map(|pulse| (pulse.terminal, pulse.started))
    });
    let emit_input = |terminal: &Entity<TerminalView>, cx: &mut gpui::VisualTestContext| {
        terminal.update(cx, |_, cx| {
            cx.emit(KeyboardInputQueued { at: cx.background_executor().now() });
        });
    };
    let edit_frame = |cx: &mut gpui::VisualTestContext, edit: fn(&mut super::PaneFrame)| {
        cx.update(|window, cx| view.update(cx, |app, cx| {
            let super::Kind::Block { content: super::Content::Context { pane: Some(frame), .. } } =
                &mut app.chrome.kind else { panic!("the test layout must frame panes") };
            edit(frame);
            app.sync(window, cx);
        }));
    };

    emit_input(&first, cx);
    assert!(pulse(cx).is_none(), "the layout must opt in");

    edit_frame(cx, |frame| {
        frame.typing_pulse = true;
        frame.lone = false;
    });
    emit_input(&first, cx);
    assert!(pulse(cx).is_none(), "an unframed single pane cannot pulse");

    edit_frame(cx, |frame| {
        frame.lone = true;
        frame.focus.as_mut().unwrap().width = 0.0;
    });
    emit_input(&first, cx);
    assert!(pulse(cx).is_none(), "a zero-width border cannot pulse");

    edit_frame(cx, |frame| frame.focus.as_mut().unwrap().width = 2.0);
    assert!(view.read_with(cx, |app, _| app.pane_term("p1").unwrap().typing_observer.is_some()));
    let second = cx.update(|window, cx| view.update(cx, |app, cx| {
        assert!(app.model.split("p1", super::Axis::Row, "p2", "t2"));
        app.model.focus_pane("p1");
        app.sync(window, cx);
        let now = cx.background_executor().now();
        let stale_terminal = cx.new(|_| ()).entity_id();
        app.pulse_typed_pane("p1", stale_terminal, now, window, cx);
        assert!(app.typing_pulse.is_none(), "a replaced terminal cannot pulse a reused pane");
        app.pulse_typed_pane("offscreen", first.entity_id(), now, window, cx);
        assert!(app.typing_pulse.is_none(), "an offscreen pane cannot pulse");
        app.pane_term("p2").unwrap().view.clone()
    }));
    cx.run_until_parked();

    emit_input(&second, cx);
    assert!(pulse(cx).is_none(), "background input cannot start a pulse");

    emit_input(&first, cx);
    let initial = pulse(cx).unwrap();
    assert_eq!(initial.0, first.entity_id());

    let tick = Duration::from_millis(1);
    cx.executor().advance_clock(DURATION - tick);
    emit_input(&first, cx);
    assert_eq!(pulse(cx), Some(initial), "repeat input must keep the active burst");

    cx.executor().advance_clock(tick);
    emit_input(&first, cx);
    assert_eq!(pulse(cx).unwrap().1.duration_since(initial.1), DURATION);

    edit_frame(cx, |frame| frame.typing_pulse = false);
    emit_input(&first, cx);
    assert!(pulse(cx).is_none(), "disabling the layout clears the pulse and removes the handler");
    assert!(view.read_with(cx, |app, _| app.pane_term("p1").unwrap().typing_observer.is_none()));
    edit_frame(cx, |frame| frame.typing_pulse = true);
    emit_input(&first, cx);
    assert_eq!(pulse(cx).unwrap().0, first.entity_id());

    cx.update(|window, cx| view.update(cx, |app, cx| {
        app.model.focus_pane("p2");
        app.sync(window, cx);
    }));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(pulse(cx).is_none(), "switching panes clears the old burst");
    emit_input(&first, cx);
    assert!(pulse(cx).is_none(), "late input from the old pane is ignored");

    emit_input(&second, cx);
    assert_eq!(pulse(cx).unwrap().0, second.entity_id());
    cx.update(|window, cx| second.update(cx, |terminal, cx| terminal.open_search(window, cx)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(pulse(cx).is_none(), "opening Find clears the burst");
    emit_input(&second, cx);
    assert!(pulse(cx).is_none(), "a late event cannot pulse while Find owns the keyboard");

    cx.update(|window, cx| second.read(cx).focus_handle().clone().focus(window, cx));
    emit_input(&second, cx);
    assert_eq!(pulse(cx).unwrap().0, second.entity_id());

    cx.deactivate_window();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(pulse(cx).is_none(), "the glow clears when the window loses focus");
    emit_input(&second, cx);
    assert!(pulse(cx).is_none(), "an inactive window ignores incoming typing events");
}
