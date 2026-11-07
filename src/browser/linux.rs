//! GTK embeds into X11, not a foreign Wayland surface.
use gpui::{App, Global, Window};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle};
use std::time::Duration;

struct GtkLoop;
impl Global for GtkLoop {}

pub fn build(url: &str, window: &Window, cx: &mut App) -> anyhow::Result<wry::WebView> {
    let raw = match HasWindowHandle::window_handle(window)?.as_raw() {
        RawWindowHandle::Xcb(handle) => {
            let mut parent = XlibWindowHandle::new(handle.window.get().into());
            parent.visual_id = handle.visual_id.map_or(0, |id| id.get().into());
            parent
        }
        RawWindowHandle::Xlib(handle) => handle,
        _ => anyhow::bail!("The browser needs X11/XWayland. Start Herdr with WAYLAND_DISPLAY unset."),
    };
    if !cx.has_global::<GtkLoop>() {
        if !gtk::is_initialized_main_thread() {
            gtk::gdk::set_allowed_backends("x11");
            gtk::init().map_err(|e| anyhow::anyhow!("GTK initialization failed: {e}"))?;
        }
        cx.set_global(GtkLoop);
        cx.spawn(async move |cx| {
            loop {
                while gtk::events_pending() {
                    gtk::main_iteration_do(false);
                }
                cx.background_executor().timer(Duration::from_millis(16)).await;
            }
        }).detach();
    }
    // SAFETY: the native browser is owned by retained state inside this window.
    struct Parent(XlibWindowHandle);
    impl HasWindowHandle for Parent {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Xlib(self.0)) })
        }
    }
    thread_local! {
        static CONTEXT: std::cell::RefCell<Option<wry::WebContext>> = const { std::cell::RefCell::new(None) };
    }
    CONTEXT.with_borrow_mut(|context| {
        let context = context.get_or_insert_with(|| wry::WebContext::new(Some(crate::mods::app_dir().join("browser"))));
        Ok(super::Browser::builder(url, context).build_as_child(&Parent(raw))?)
    })
}
