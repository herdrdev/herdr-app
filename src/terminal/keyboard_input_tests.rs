#[gpui::test]
fn keyboard_activity_observer_is_optional(cx: &mut gpui::TestAppContext) {
    use gpui::*;
    use gpui_shell::action::ShellAction;
    use std::{cell::Cell, rc::Rc};

    const CHANNEL: u32 = 7;

    struct Root(Entity<super::TerminalView>);
    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }

    let (core, drain_input) = crate::hc::record_terminal_input();
    let typed = Rc::new(Cell::new(0));
    let observed = typed.clone();

    cx.update(gpui_base::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let terminal = cx.new(|cx| {
            super::TerminalView::attach(core, "t1".into(), (40, 6), cx)
        });
        cx.subscribe(&terminal, move |_, _, _: &super::KeyboardInputQueued, _| {
            observed.set(observed.get() + 1);
        }).detach();
        let focus = terminal.read(cx).focus_handle().clone();
        focus.focus(window, cx);
        Root(terminal)
    });
    cx.run_until_parked();

    let terminal = root.read_with(cx, |root, _| root.0.clone());
    let expect = |case, events, sent: &[&[u8]]| {
        assert_eq!(typed.replace(0), events, "{case}: typing events");
        let expected: Vec<_> = sent.iter().map(|bytes| (CHANNEL, bytes.to_vec())).collect();
        assert_eq!(drain_input(), expected, "{case}: bytes written to the core");
    };

    cx.simulate_keystrokes("x");
    expect("disconnected terminal", 0, &[]);

    terminal.update(cx, |terminal, _| terminal.grid.borrow_mut().channel = Some(CHANNEL));
    cx.simulate_keystrokes("a");
    expect("ordinary key without an observer enabled", 0, &[b"a"]);

    cx.update(|window, app| terminal.update(app, |terminal, cx| {
        terminal.replace_text_in_range(None, "日本", window, cx);
    }));
    expect("IME commit without an observer enabled", 0, &["日本".as_bytes()]);

    cx.dispatch_action(ShellAction::new("send:ready"));
    expect("send key binding without an observer enabled", 0, &[b"ready"]);

    let observer = terminal.update(cx, |terminal, _| terminal.enable_keyboard_input_observer());
    cx.simulate_keystrokes("a");
    expect("ordinary key with an observer enabled", 1, &[b"a"]);

    cx.update(|window, app| terminal.update(app, |terminal, cx| {
        terminal.replace_and_mark_text_in_range(None, "nihon", None, window, cx);
    }));
    expect("IME composition preview", 0, &[]);

    cx.update(|window, app| terminal.update(app, |terminal, cx| {
        terminal.replace_text_in_range(None, "日本", window, cx);
        terminal.replace_text_in_range(None, "", window, cx);
    }));
    expect("IME commit and empty update", 1, &["日本".as_bytes()]);

    cx.dispatch_action(ShellAction::new("send:ready"));
    expect("send key binding", 1, &[b"ready"]);
    cx.dispatch_action(ShellAction::new("send:"));
    expect("empty send key binding", 0, &[]);

    terminal.update(cx, |terminal, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("pasted".into()));
        terminal.paste(cx);
    });
    expect("clipboard paste", 0, &[b"pasted"]);

    terminal.update(cx, |terminal, cx| {
        assert!(terminal.ingest([crate::hc::Stream::Data(b"program output\r\n".to_vec())], cx));
    });
    expect("program output", 0, &[]);

    drop(observer);
    cx.simulate_keystrokes("b");
    expect("ordinary key after dropping the observer", 0, &[b"b"]);

    let _observer = terminal.update(cx, |terminal, _| terminal.enable_keyboard_input_observer());
    cx.update(|window, app| terminal.update(app, |terminal, cx| terminal.open_search(window, cx)));
    cx.run_until_parked();
    cx.simulate_keystrokes("x");
    cx.dispatch_action(ShellAction::new("send:ignored"));
    cx.update(|window, app| terminal.update(app, |terminal, cx| {
        terminal.replace_text_in_range(None, "ignored", window, cx);
    }));
    expect("Find owns the keyboard", 0, &[]);
    assert_eq!(terminal.read_with(cx, |terminal, _| terminal.search_query.clone()), "x");

    cx.update(|window, app| terminal.update(app, |terminal, cx| terminal.focus.focus(window, cx)));
    cx.simulate_keystrokes("a");
    expect("terminal has the keyboard while Find is open", 1, &[b"a"]);

    drop(drain_input);
    cx.simulate_keystrokes("b");
    assert_eq!(typed.get(), 0, "a closed writer queue must not be reported as input");
}
