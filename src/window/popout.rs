//! A terminal popped out into its own OS window.
//!
//! Popping out detaches the terminal: its pane is parked (removed; labels and
//! keep cleared) and the terminal keeps running in herdr-core, with this
//! window as its only view. `Return` gives it a pane again (`terminal.keep`,
//! as a tab) and closes only once the session has it back; closing the window
//! leaves it detached but running, recoverable with `terminal.keep`.

use super::*;

/// Open pop-outs by terminal address, so a second request activates the
/// window that already shows the terminal instead of attaching another view
/// to it (two views would fight over its size).
#[derive(Default)]
pub(super) struct Popouts(pub(super) HashMap<String, WindowHandle<Popout>>);

impl Global for Popouts {}

pub(super) struct Popout {
    address: String,
    view: Entity<TerminalView>,
    term_focus: FocusHandle,
    tx: async_channel::Sender<Request>,
    /// Why Return didn't close the window, shown until the next try.
    notice: Option<String>,
    _sub: Option<Subscription>,
    _act: Option<Subscription>,
}

impl Popout {
    /// Re-attaches the view when the core connection is replaced.
    pub(super) fn reattach(&self, core: &Arc<Core>, cx: &mut Context<Self>) {
        self.view.update(cx, |v, cx| v.reattach(core.clone(), cx));
    }

    /// Releases the core attachment and the registry entry. Every close path
    /// runs this, and it is safe more than once.
    fn release(&self, cx: &mut Context<Self>) {
        self.view.read(cx).detach();
        if cx.has_global::<Popouts>() {
            cx.global_mut::<Popouts>().0.remove(&self.address);
        }
    }
}

/// Opens `address` in its own OS window, centered, with the system's
/// (resizable) title bar.
pub(super) fn open(core: &Arc<Core>, address: &str, title: &str, tx: async_channel::Sender<Request>, cx: &mut App) -> Result<(), String> {
    if let Some(open) = cx
        .try_global::<Popouts>()
        .and_then(|p| p.0.get(address).cloned())
        && open.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return Ok(());
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(920.0), px(560.0)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some(
                if title.is_empty() {
                    format!("Herdr · {address}")
                } else {
                    format!("{title} · {address}")
                }
                .into(),
            ),
            ..Default::default()
        }),
        // One app wherever the platform groups windows by app id.
        app_id: Some(
            std::env::var("HERDR_APP_ID")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "herdr-app".into()),
        ),
        ..Default::default()
    };
    let addr = address.to_owned();
    let handle = cx
        .open_window(options, |window, cx| {
            // A terminal that turns out to be gone announces its end: this
            // window has no listing to notice it never came up.
            let view = cx.new(|cx| TerminalView::attach_with(core.clone(), addr.clone(), (120, 40), true, cx));
            let focus = view.read(cx).focus_handle().clone();
            focus.focus(window, cx);
            let term_focus = focus;
            cx.new(|_cx| Popout { address: addr.clone(), view, term_focus, tx, notice: None, _sub: None, _act: None })
        })
        .map_err(|e| format!("cannot open a pop-out window: {e}"))?;
    if !cx.has_global::<Popouts>() {
        cx.set_global(Popouts::default());
    }
    cx.global_mut::<Popouts>().0.insert(addr, handle.clone());
    // The subscription and close handling need the window, which only a
    // built window has.
    let _ = handle.update(cx, |this, window, cx| {
        let view = this.view.clone();
        let tx = this.tx.clone();
        this._sub = Some(cx.subscribe_in(&view, window, move |this, _, ev: &TerminalEvent, window, cx| match ev {
            // A ctrl-clicked address peeks where the peek overlay lives.
            TerminalEvent::Peek(at) => {
                let _ = tx.try_send(Request::Run {
                    name: "terminal.peek".into(),
                    args: json!({ "terminal": at.to_string() }),
                });
            }
            // The program ended: release the core attachment and the window.
            TerminalEvent::Ended => {
                this.release(cx);
                window.remove_window();
            }
            _ => {}
        }));
        // An activated window hands the keyboard straight to the terminal,
        // the way clicking a pane does.
        this._act = Some(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.term_focus.focus(window, cx);
            }
        }));
        // The core keeps an attachment per view; the main window releases
        // pane views when they leave it, this releases when it closes.
        let closing = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = closing.update(cx, |this, cx| this.release(cx));
            true
        });
        window.activate_window();
    });
    Ok(())
}

/// Detaches and closes the pop-out showing `address`, if one is open: a
/// terminal that has a pane again is not alone anymore.
pub(super) fn close(address: &str, cx: &mut App) {
    let Some(open) = cx.try_global::<Popouts>().and_then(|p| p.0.get(address).cloned()) else { return; };
    let _ = open.update(cx, |this, window, cx| {
        this.release(cx);
        window.remove_window();
    });
}

impl Render for Popout {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The shared output clock only redraws views drawn in the latest global
        // frame (terminal.rs `FRAME`), which a second window's views never are
        // while the main one keeps drawing. Keep this window's frames coming so
        // its terminal's output and cursor reach the screen.
        _window.request_animation_frame();
        let address = self.address.clone();
        let tx = self.tx.clone();
        let notice = self.notice.clone();
        let back = div()
            .id("popout-return")
            .flex()
            .items_center()
            .h(settings::z(22.0))
            .px(settings::z(10.0))
            .rounded(settings::z(4.0))
            .cursor_pointer()
            .text_size(settings::z(12.0))
            .text_color(c("dim"))
            .hover(|s| s.bg(c("hover")).text_color(c("foreground")))
            .child("Return")
            .on_click(cx.listener(move |this, _, _, cx| {
                // Ask for a pane back, then close only when the session has
                // it: a keep that cannot run leaves this window as the
                // terminal's only view.
                let (reply_tx, reply_rx) = std::sync::mpsc::channel::<Value>();
                let _ = tx.try_send(Request::Control {
                    cmd: json!({ "do": "terminal.keep", "args": { "terminal": address } }),
                    reply: reply_tx,
                });
                this.notice = None;
                cx.spawn(async move |this, cx| {
                    // The reply says whether the keep was even accepted; a
                    // timeout or a gone window counts as done.
                    let ok = cx
                        .background_spawn(async move {
                            match reply_rx.recv_timeout(Duration::from_secs(2)) {
                                Ok(v) => v.get("error").is_none(),
                                // A late reply or a gone window must not
                                // strand this one.
                                Err(_) => true,
                            }
                        })
                        .await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if ok {
                            this.release(cx);
                            window.remove_window();
                        } else {
                            this.notice =
                                Some("Could not return it (it stays detached, still running): open a space on its machine or see the main window's toast.".into());
                            cx.notify();
                        }
                    });
                })
                .detach();
            }));
        div()
            .id("popout")
            .size_full()
            .flex()
            .flex_col()
            .bg(c("background"))
            .child(
                div()
                    .h(settings::z(32.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(settings::z(8.0))
                    .gap(settings::z(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(settings::z(12.0))
                            .text_color(c("accent"))
                            .children(notice),
                    )
                    .child(back),
            )
            .child(div().flex_1().min_h(px(0.)).child(shown_terminal(&self.view, cx)))
    }
}
