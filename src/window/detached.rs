//! The main window still owns the model and the terminal views. A detached
//! window draws the same tab, so the live sessions and their scrollback move
//! without another attachment or a change to the core document.

use super::*;

pub(super) struct DetachedTab {
    owner: WeakEntity<AppWindow>,
    main: WindowHandle<AppWindow>,
    tab: String,
    drag: Option<TitleDrag>,
    _owner_changed: Subscription,
}

struct TitleDrag {
    cursor: Point<Pixels>,
    origin: Point<Pixels>,
    moving: bool,
}

impl AppWindow {
    pub(super) fn main_tab(&self) -> Option<&Tab> {
        self.model.active_tab().filter(|tab| !self.detached_tabs.contains_key(&tab.id))
    }

    pub(super) fn sync_detached_tabs(&mut self, cx: &mut App) {
        self.detached_tabs.retain(|id, handle| {
            if self.model.tab(id).is_some() {
                true
            } else {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
                false
            }
        });
        if let Some(space) = self.model.active_index().map(|i| &mut self.model.spaces[i])
            && space.active_tab().is_some_and(|tab| self.detached_tabs.contains_key(&tab.id))
            && let Some(next) = space.tabs.iter().find(|tab| !self.detached_tabs.contains_key(&tab.id) && tab.settled.is_none())
                .or_else(|| space.tabs.iter().find(|tab| !self.detached_tabs.contains_key(&tab.id)))
        {
            space.active = Some(next.id.clone());
        }
    }

    pub(super) fn close_detached_tabs(&mut self, cx: &mut App) {
        for (_, handle) in self.detached_tabs.drain() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    pub(super) fn focus_detached_pane(&mut self, pane: &str, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.model.tab_of(pane) else { return false };
        if !self.detached_tabs.contains_key(&tab.id) { return false; }
        if tab.focused.as_deref() != Some(pane) {
            let tab_id = tab.id.clone();
            if let Some(tab) = self.model.tab_mut(&tab_id) {
                tab.focus(pane);
                cx.notify();
            }
        }
        true
    }

    pub(super) fn detach_tab(&mut self, args: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if !cfg!(target_os = "macos") {
            return self.fail("tab.detach requires macOS");
        }
        let Some(id) = Self::owned(args, "id").filter(|id| self.model.tab(id).is_some()) else {
            return self.fail("no such tab");
        };
        if let Some(handle) = self.detached_tabs.get(&id).copied() {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
            return;
        }
        let at = match (args.get("x"), args.get("y")) {
            (None, None) => None,
            (Some(x), Some(y)) => {
                let coord = |v: &Value| v.as_f64().map(|n| n as f32).filter(|n| n.is_finite()).map(px);
                let (Some(x), Some(y)) = (coord(x), coord(y)) else {
                    return self.fail("tab.detach needs finite x and y coordinates");
                };
                let at = point(x, y);
                if Bounds::new(Point::default(), window.viewport_size()).contains(&at) {
                    return;
                }
                native::to_screen(window, at)
            }
            _ => return self.fail("tab.detach needs both x and y"),
        };
        let main = window.window_handle().downcast::<AppWindow>().expect("app root window");
        let owner = cx.weak_entity();
        let display_id = window.display(cx).map(|d| d.id());
        let bounds = Bounds::centered(display_id, size(px(840.), px(540.)), cx);
        // GPUI draws a new window before returning. Open after the main view's
        // update, so its model and views are available to that first draw.
        cx.defer(move |cx| {
            let Some(owner) = owner.upgrade() else { return };
            let app = owner.read(cx);
            if app.model.tab(&id).is_none() || app.detached_tabs.contains_key(&id) { return; }
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                display_id,
                titlebar: Some(TitlebarOptions {
                    title: Some("Herdr".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(9.))),
                }),
                app_owns_titlebar_drag: true,
                ..Default::default()
            };
            let result = cx.open_window(options, |window, cx| {
                if let Some(at) = at { native::place(window, at); }
                if let Some(focus) = owner.read(cx).model.tab(&id).and_then(Tab::focused_pane)
                    .and_then(|pane| owner.read(cx).pane_focus(&pane, cx))
                {
                    focus.focus(window, cx);
                }
                cx.new(|cx| DetachedTab::new(main, &owner, id.clone(), window, cx))
            });
            match result {
                Ok(handle) => {
                    if main.update(cx, |this, window, cx| {
                        this.detached_tabs.insert(id, handle);
                        this.sync(window, cx);
                    }).is_err() {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                }
                Err(error) => {
                    let _ = main.update(cx, |this, window, cx| {
                        this.command_error("Open tab in new window", &error.to_string(), window, cx);
                    });
                }
            }
        });
    }

    fn reattach_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.detached_tabs.remove(id).is_some() {
            self.tab_drop_target = false;
            self.model.select_tab(id);
            self.sync(window, cx);
            window.activate_window();
        }
    }

    fn tab_bar_bounds(&self) -> Option<Bounds<Pixels>> {
        self.chrome.visible_blocks().into_iter().find(|node| node.id == "tabs")?;
        self.bounds.borrow().get("tabs").copied()
    }

    pub(super) fn tab_drop_outline(&self) -> Option<AnyElement> {
        let bounds = self.tab_drop_target.then(|| self.tab_bar_bounds()).flatten()?;
        Some(div().absolute().left(bounds.origin.x).top(bounds.origin.y)
            .w(bounds.size.width).h(bounds.size.height)
            .border_2().border_color(c("accent")).into_any_element())
    }
}

impl DetachedTab {
    fn new(main: WindowHandle<AppWindow>, owner: &Entity<AppWindow>, tab: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let close_tab = tab.clone();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = main.update(cx, |this, window, cx| this.reattach_tab(&close_tab, window, cx));
            true
        });
        Self { owner: owner.downgrade(), main, tab, drag: None, _owner_changed: cx.observe(owner, |_, _, cx| cx.notify()) }
    }

    fn drag_title(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(drag) = &mut self.drag else { return false };
        let cursor = native::cursor();
        let delta = cursor - drag.cursor;
        if !drag.moving && delta.x.abs().max(delta.y.abs()) < px(4.) { return false; }
        drag.moving = true;
        native::move_frame(window, drag.origin + delta);
        let _ = self.main.update(cx, |this, main, cx| {
            let over = main.is_visible() && native::from_screen(main, cursor)
                .is_some_and(|at| this.tab_bar_bounds().is_some_and(|bounds| bounds.contains(&at)));
            if std::mem::replace(&mut this.tab_drop_target, over) != over { cx.notify(); }
        });
        true
    }

    fn release_title(&mut self, drop: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.drag.take() else { return false };
        if !drag.moving { return false; }
        let cursor = native::cursor();
        let mut joined = false;
        let _ = self.main.update(cx, |this, main, cx| {
            if drop && main.is_visible() && native::from_screen(main, cursor)
                .is_some_and(|at| this.tab_bar_bounds().is_some_and(|bounds| bounds.contains(&at)))
            {
                this.reattach_tab(&self.tab, main, cx);
                joined = true;
            } else if std::mem::take(&mut this.tab_drop_target) {
                cx.notify();
            }
        });
        if joined { window.remove_window(); }
        true
    }
}

impl Render for DetachedTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(settings::ui_font_size()));
        let (title, body) = self.owner.update(cx, |owner, cx| {
            let space = owner.model.space_of(&self.tab)?;
            let tab = space.tab(&self.tab)?;
            let title = Model::tab_name(space, tab);
            let focused = tab.focused_pane().and_then(|pane| tab.shown().path_to(&pane))
                .unwrap_or_default().into_iter().collect();
            let body = owner.render_node(tab.shown(), &focused, None, &mut HashSet::new(), cx);
            Some((title, body))
        }).ok().flatten().unwrap_or_else(|| ("Herdr".into(), div().into_any_element()));
        window.set_window_title(&title);
        let subject = cx.weak_entity();
        let tracker = canvas(|_, _, _| {}, move |_, _, window, _| {
            let subject = subject.clone();
            let movement = subject.clone();
            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture {
                    let _ = movement.update(cx, |this, cx| {
                        if ev.pressed_button == Some(MouseButton::Left) {
                            this.drag_title(window, cx);
                        } else {
                            this.release_title(false, window, cx);
                        }
                    });
                }
            });
            window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && ev.button == MouseButton::Left {
                    let _ = subject.update(cx, |this, cx| this.release_title(true, window, cx));
                }
            });
        }).absolute().size_full();
        div().relative().size_full().flex().flex_col().bg(c("background"))
            .child(div().h(px(34.)).flex_none().w_full().flex().items_center().pl(px(80.))
                .bg(c("chrome")).border_b_1().border_color(c("border")).text_color(c("foreground"))
                .text_size(settings::z(13.)).child(title)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, _| {
                    if event.click_count == 2 {
                        this.drag = None;
                        window.titlebar_double_click();
                    } else {
                        this.drag = native::frame_origin(window).map(|origin| TitleDrag {
                            cursor: native::cursor(), origin, moving: false,
                        });
                    }
                })))
            .child(div().flex_1().min_h_0().child(body))
            .child(tracker)
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::{NSPoint, NSRect};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    fn objects(window: &Window) -> Option<(*mut AnyObject, *mut AnyObject)> {
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw() else { return None };
        let view = handle.ns_view.as_ptr().cast::<AnyObject>();
        // The view comes from the live GPUI window on the main thread.
        let panel: *mut AnyObject = unsafe { msg_send![view, window] };
        (!panel.is_null()).then_some((view, panel))
    }

    fn point_from(p: NSPoint) -> Point<Pixels> {
        point(px(p.x as f32), px(p.y as f32))
    }

    fn ns(p: Point<Pixels>) -> NSPoint {
        NSPoint::new(f32::from(p.x) as f64, f32::from(p.y) as f64)
    }

    pub(super) fn cursor() -> Point<Pixels> {
        point_from(unsafe { msg_send![class!(NSEvent), mouseLocation] })
    }

    pub(super) fn to_screen(window: &Window, at: Point<Pixels>) -> Option<Point<Pixels>> {
        let (_, panel) = objects(window)?;
        let p = ns(point(at.x, window.viewport_size().height - at.y));
        Some(point_from(unsafe { msg_send![panel, convertPointToScreen: p] }))
    }

    pub(super) fn from_screen(window: &Window, at: Point<Pixels>) -> Option<Point<Pixels>> {
        let (_, panel) = objects(window)?;
        let p = point_from(unsafe { msg_send![panel, convertPointFromScreen: ns(at)] });
        Some(point(p.x, window.viewport_size().height - p.y))
    }

    pub(super) fn frame_origin(window: &Window) -> Option<Point<Pixels>> {
        let (_, panel) = objects(window)?;
        let frame: NSRect = unsafe { msg_send![panel, frame] };
        Some(point_from(frame.origin))
    }

    pub(super) fn place(window: &Window, at: Point<Pixels>) {
        if let Some((_, panel)) = objects(window) {
            let top = ns(at + point(px(-50.), px(16.)));
            unsafe { let _: () = msg_send![panel, setFrameTopLeftPoint: top]; }
        }
    }

    pub(super) fn move_frame(window: &Window, at: Point<Pixels>) {
        if let Some((_, panel)) = objects(window) {
            unsafe { let _: () = msg_send![panel, setFrameOrigin: ns(at)]; }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native {
    use super::*;

    pub(super) fn cursor() -> Point<Pixels> { Point::default() }
    pub(super) fn to_screen(_: &Window, _: Point<Pixels>) -> Option<Point<Pixels>> { None }
    pub(super) fn from_screen(_: &Window, _: Point<Pixels>) -> Option<Point<Pixels>> { None }
    pub(super) fn frame_origin(_: &Window) -> Option<Point<Pixels>> { None }
    pub(super) fn place(_: &Window, _: Point<Pixels>) {}
    pub(super) fn move_frame(_: &Window, _: Point<Pixels>) {}
}
