//! The window's own title bar: a layout can hide the system's
//! (`"titlebar": "none"` on its root) and draw one from mods. Nodes with
//! `window_drag` move the window when dragged and maximize it on a
//! double-click; a mod's own control opts out by stopping the mouse-down.
//!
//! macOS: the title bar is always see-through and the app owns dragging.
//! With `system`, the app draws an empty strip for the window buttons; with
//! `none`, they sit over the layout where it says (`traffic_lights`).
//! Linux: decorations are asked for live; when the compositor leaves them to
//! us (GNOME always does), the app resizes from the window's edges and mods
//! draw the buttons (`window()`).
//! Windows: the system's title bar is always hidden. With `system`, the app
//! draws a strip in the theme with Windows' own buttons (snap layouts work);
//! with `none`, mods draw the buttons, as on Linux.

use super::*;
use crate::layout::Titlebar;

/// How far the pointer moves before a press on a drag node moves the window.
const DRAG_START: f32 = 4.0;
/// The strip a `system` title bar leaves on macOS, px at zoom 1.
#[cfg(target_os = "macos")]
const MAC_STRIP: f32 = 28.0;
/// Where the window buttons sit by default (macOS), px.
#[cfg(target_os = "macos")]
const MAC_LIGHTS: [f32; 2] = [12.0, 7.0];
/// The grab band along the edges when the app resizes the window (Linux).
#[cfg(not(any(target_os = "macos", windows)))]
const EDGE: f32 = 6.0;
/// Windows' caption height and button width, px at zoom 1.
#[cfg(windows)]
const WIN_STRIP: f32 = 32.0;
#[cfg(windows)]
const WIN_BUTTON: f32 = 46.0;

impl AppWindow {
    pub(super) fn titlebar(&self) -> Titlebar {
        self.chrome.titlebar.unwrap_or_default()
    }

    /// Makes the window match the shown layout's title bar. Cheap; runs on every render.
    pub(super) fn apply_titlebar(&mut self, window: &mut Window) {
        let want = (self.titlebar(), self.chrome.traffic_lights);
        if self.titlebar_applied == Some(want) {
            return;
        }
        self.titlebar_applied = Some(want);
        #[cfg(windows)]
        let _ = window;
        #[cfg(target_os = "macos")]
        {
            let [x, y] = match want {
                (Titlebar::Layout, Some(at)) => at,
                _ => MAC_LIGHTS,
            };
            window.set_traffic_light_position(point(px(x), px(y)));
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        window.request_decorations(match want.0 {
            Titlebar::System => WindowDecorations::Server,
            Titlebar::Layout => WindowDecorations::Client,
        });
    }

    /// What mods know about the window: `window()`.
    pub(super) fn window_json(&self, window: &Window) -> Value {
        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(windows) {
            "windows"
        } else {
            "linux"
        };
        // Window buttons are the layout's to draw only where the system draws none.
        let buttons = if cfg!(windows) {
            self.titlebar() == Titlebar::Layout
        } else {
            !cfg!(target_os = "macos") && matches!(window.window_decorations(), Decorations::Client { .. })
        };
        json!({
            "platform": platform,
            "titlebar": if self.titlebar() == Titlebar::Layout { "none" } else { "system" },
            "buttons": buttons,
            "maximized": window.is_maximized(),
            "fullscreen": window.is_fullscreen(),
            "active": self.window_active,
        })
    }

    /// Makes a node move the window when dragged and maximize it on a double-click.
    pub(super) fn window_drag(&self, el: Div, cx: &mut Context<Self>) -> Div {
        el.on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, _| {
            if ev.click_count == 2 {
                this.drag_from = None;
                #[cfg(target_os = "macos")]
                window.titlebar_double_click();
                #[cfg(windows)]
                win::toggle_maximize(window);
                #[cfg(not(any(target_os = "macos", windows)))]
                window.zoom_window();
            } else {
                this.drag_from = Some(ev.position);
            }
        }))
        .on_mouse_down(MouseButton::Right, cx.listener(|_, _ev: &MouseDownEvent, _window, _| {
            #[cfg(not(any(target_os = "macos", windows)))]
            _window.show_window_menu(_ev.position);
        }))
    }

    /// The root's pointer moves: a press on a drag node that moved far enough moves the window.
    pub(super) fn drag_window(&mut self, ev: &MouseMoveEvent, window: &mut Window) {
        let Some(from) = self.drag_from else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag_from = None;
            return;
        }
        let d = ev.position - from;
        if d.x.abs() > px(DRAG_START) || d.y.abs() > px(DRAG_START) {
            self.drag_from = None;
            #[cfg(windows)]
            win::start_move(window);
            #[cfg(not(windows))]
            window.start_window_move();
        }
    }

    /// Wraps the layout in what the window itself needs: on macOS a strip for the
    /// window buttons (system title bar), on Linux resize edges (client decorations).
    pub(super) fn window_frame(&self, tree: AnyElement, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        #[cfg(target_os = "macos")]
        {
            let _ = window;
            if self.titlebar() == Titlebar::System {
                let strip = self.window_drag(div().flex_none().h(px(MAC_STRIP)).w_full(), cx);
                return div().size_full().flex().flex_col().child(strip).child(div().flex_1().min_h_0().flex().child(tree)).into_any_element();
            }
            tree
        }
        #[cfg(windows)]
        {
            if self.titlebar() == Titlebar::Layout || window.is_fullscreen() {
                return tree;
            }
            let strip = win::strip(window.is_maximized(), cx);
            div().size_full().flex().flex_col().child(strip).child(div().flex_1().min_h_0().flex().child(tree)).into_any_element()
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = cx;
            let Decorations::Client { tiling } = window.window_decorations() else { return tree };
            if window.is_maximized() || window.is_fullscreen() {
                return tree;
            }
            let e = px(EDGE);
            let grip = |id: &'static str, edge: ResizeEdge, cursor: CursorStyle| {
                div()
                    .id(id)
                    .absolute()
                    .occlude()
                    .cursor(cursor)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        window.start_window_resize(edge);
                    })
            };
            let mut grips: Vec<AnyElement> = Vec::new();
            let (top, bottom, left, right) = (!tiling.top, !tiling.bottom, !tiling.left, !tiling.right);
            if top {
                grips.push(grip("rz-t", ResizeEdge::Top, CursorStyle::ResizeUpDown).top_0().left(e).right(e).h(e).into_any_element());
            }
            if bottom {
                grips.push(grip("rz-b", ResizeEdge::Bottom, CursorStyle::ResizeUpDown).bottom_0().left(e).right(e).h(e).into_any_element());
            }
            if left {
                grips.push(grip("rz-l", ResizeEdge::Left, CursorStyle::ResizeLeftRight).left_0().top(e).bottom(e).w(e).into_any_element());
            }
            if right {
                grips.push(grip("rz-r", ResizeEdge::Right, CursorStyle::ResizeLeftRight).right_0().top(e).bottom(e).w(e).into_any_element());
            }
            let corner = |id, edge, cursor| grip(id, edge, cursor).w(e * 2.).h(e * 2.);
            if top && left {
                grips.push(corner("rz-tl", ResizeEdge::TopLeft, CursorStyle::ResizeUpLeftDownRight).top_0().left_0().into_any_element());
            }
            if top && right {
                grips.push(corner("rz-tr", ResizeEdge::TopRight, CursorStyle::ResizeUpRightDownLeft).top_0().right_0().into_any_element());
            }
            if bottom && left {
                grips.push(corner("rz-bl", ResizeEdge::BottomLeft, CursorStyle::ResizeUpRightDownLeft).bottom_0().left_0().into_any_element());
            }
            if bottom && right {
                grips.push(corner("rz-br", ResizeEdge::BottomRight, CursorStyle::ResizeUpLeftDownRight).bottom_0().right_0().into_any_element());
            }
            div().size_full().relative().flex().child(tree).children(grips).into_any_element()
        }
    }

    pub(super) fn window_command(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match name {
            "window.close" => {
                self.drain_documents(window, cx);
                self.close_mods_now(std::time::Instant::now() + Duration::from_millis(250), window, cx);
                crate::place::save(window, cx);
                window.remove_window();
            }
            "window.minimize" => window.minimize_window(),
            #[cfg(windows)]
            "window.maximize" => win::toggle_maximize(window),
            #[cfg(not(windows))]
            "window.maximize" => window.zoom_window(),
            _ => {}
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsZoomed, PostMessageW, SC_MOVE, SW_MAXIMIZE, SW_RESTORE, ShowWindowAsync, WM_SYSCOMMAND};

    fn hwnd(window: &Window) -> Option<HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
            _ => None,
        }
    }

    /// Hands the press to Windows' own move loop (snapping included). Posted,
    /// not sent: the loop runs after this event, outside GPUI's handler.
    pub(super) fn start_move(window: &Window) {
        let Some(h) = hwnd(window) else { return };
        // SC_MOVE + HTCAPTION: move with the mouse, not the keyboard.
        unsafe {
            ReleaseCapture();
            PostMessageW(h, WM_SYSCOMMAND, (SC_MOVE | 2) as usize, 0);
        }
    }

    /// GPUI's zoom only maximizes.
    pub(super) fn toggle_maximize(window: &Window) {
        let Some(h) = hwnd(window) else { return };
        unsafe {
            ShowWindowAsync(h, if IsZoomed(h) != 0 { SW_RESTORE } else { SW_MAXIMIZE });
        }
    }

    /// The `system` title bar: a strip in the theme, Windows' buttons on the
    /// right. Windows itself handles the areas (drag, snap layouts, double-click).
    pub(super) fn strip(maximized: bool, _cx: &mut Context<AppWindow>) -> AnyElement {
        let button = |id: &'static str, icon: &'static [u8], area: WindowControlArea, hover: Hsla, hover_text: Hsla| {
            div()
                .id(id)
                .group(id)
                .window_control_area(area)
                .w(settings::z(WIN_BUTTON))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .hover(move |s| s.bg(hover))
                .child(svg().data(icon).size(settings::z(16.0)).text_color(c("dim"))
                    .group_hover(id, move |s| s.text_color(hover_text)))
        };
        let red: Hsla = rgb(0xc42b1c).into();
        div()
            .flex_none()
            .h(settings::z(WIN_STRIP))
            .w_full()
            .flex()
            .child(div().flex_1().h_full().window_control_area(WindowControlArea::Drag))
            .child(button("win-min", include_bytes!("../../defaults/mods/tabs/win-min.svg"), WindowControlArea::Min, c("raised"), c("foreground")))
            .child(button("win-max", if maximized { include_bytes!("../../defaults/mods/tabs/win-restore.svg") } else { include_bytes!("../../defaults/mods/tabs/win-max.svg") }, WindowControlArea::Max, c("raised"), c("foreground")))
            .child(button("win-close", include_bytes!("../../defaults/mods/tabs/win-close.svg"), WindowControlArea::Close, red, gpui::white()))
            .into_any_element()
    }
}
