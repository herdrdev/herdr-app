//! WebView2 creation pumps Win32 messages, including GPUI's foreground work.
//! Run it between App updates, without an App or Browser RefCell borrow.
use super::{Browser, State};
use gpui::{App, Window};
use std::rc::Rc;
use wry::raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};

pub(super) fn create(state: &State, window: &Window, cx: &App) {
    {
        let mut browser = state.borrow_mut();
        if browser.native.is_some() || browser.error.is_some() || browser.creating {
            return;
        }
        browser.creating = true;
    }
    let weak = Rc::downgrade(state);
    window.spawn(cx, async move |cx| {
        // Acquire the HWND from a still-open window in this poll. Release the
        // App borrow BEFORE calling Wry: even its synchronous build pumps work.
        let Ok(parent) = cx.update(|window, _| parent(window)) else {
            if let Some(state) = weak.upgrade() { state.borrow_mut().creating = false; }
            return;
        };
        let Some(url) = weak.upgrade().map(|state| state.borrow().url.clone()) else { return; };
        let mut context = wry::WebContext::new(Some(crate::mods::app_dir().join("browser")));
        let mut container = None;
        let built = parent.and_then(|parent| {
            let host = parent.0.hwnd.get() as windows_sys::Win32::Foundation::HWND;
            let before = super::win32::children_of_class(host, "WRY_WEBVIEW");
            let view = Browser::builder(&url, &mut context).build_as_child(&parent)?;
            // Wry's own container window: overlays are cut out of it.
            container = super::win32::children_of_class(host, "WRY_WEBVIEW").into_iter().find(|h| !before.contains(h)).map(|h| h as isize);
            Ok(view)
        });
        // The pane may have gone away or navigated while the build pumped
        // messages. Never keep its state borrowed across native calls either.
        let Some((latest_url, bounds)) = weak.upgrade().map(|state| {
            let browser = state.borrow();
            (browser.url.clone(), browser.bounds)
        }) else { return; };
        let mut built = Some(built.and_then(|view| {
            if let Some(bounds) = bounds { view.set_bounds(bounds)?; }
            view.set_visible(false)?;
            if latest_url != url { view.load_url(&latest_url)?; }
            Ok(view)
        }));
        let _ = cx.update(|window, _| {
            if let Some(state) = weak.upgrade() {
                let mut browser = state.borrow_mut();
                debug_assert!(browser.native.is_none());
                match built.take().unwrap() {
                    Ok(view) => { browser.context = Some(context); browser.native = Some(view); browser.hwnd = container; },
                    Err(error) => browser.error = Some(error.to_string()),
                }
                window.refresh();
            }
        });
        if let Some(state) = weak.upgrade() { state.borrow_mut().creating = false; }
        // If the window closed, `built` still owns the result. Drop it here,
        // outside the App update, since destroying a native child can reenter.
    }).detach();
}

fn parent(window: &Window) -> anyhow::Result<Parent> {
    match HasWindowHandle::window_handle(window)?.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(Parent(handle)),
        _ => Err(wry::Error::UnsupportedWindowHandle.into()),
    }
}

struct Parent(Win32WindowHandle);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: read from the open GPUI window in the same foreground poll
        // in which Wry creates its child, before any message-pumping wait.
        // Win32 validates HWNDs on use: closing the parent during a wait
        // destroys its child and makes later creation calls fail.
        Ok(unsafe { WindowHandle::borrow_raw(self.0.into()) })
    }
}
