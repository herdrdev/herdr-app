//! A native `WKWebView` overlaid on the GPUI content view (macOS).
//!
//! GPUI renders everything into one Metal-backed `NSView` (`GPUIView`) inside
//! the window's `contentView`. A real web page can't be drawn by GPUI or a mod,
//! so we add a `WKWebView` as a sibling subview *above* that Metal view and move
//! it to the pane's rect every frame. The pane's rect is already captured each
//! paint into `AppWindow::bounds` by the `canvas(...)` element in `window.rs`;
//! this module only owns the native view and keeps its frame in sync.
//!
//! Phase 1 scope: create one web view, load a URL, track a rect, hide/show,
//! tear down. One `WebView` per web pane comes in Phase 2.
//!
//! Written against objc2 0.6 / objc2-app-kit 0.3 / objc2-web-kit 0.3.
//! Everything here must run on the main thread.

use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// A native web view pinned over the GPUI content view. Dropping it removes the
/// view from the window.
pub struct WebView {
    webview: Retained<WKWebView>,
    /// The window's content view, for flipping GPUI's top-left rect into
    /// AppKit's bottom-left coordinates.
    content: Retained<NSView>,
    /// Last rect we applied, to skip redundant `setFrame:` churn each paint.
    last: Option<Bounds<Pixels>>,
}

impl WebView {
    /// Create a web view as a subview of `window`'s content view and load `url`.
    /// Returns `None` if the window's native view, window, or content view is
    /// unavailable, or we're off the main thread.
    pub fn new(window: &Window, url: &str) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;

        // GPUI's native handle points at the GPUIView NSView, not the NSWindow.
        // Disambiguate to the `HasWindowHandle` trait method: GPUI's inherent
        // `window_handle()` returns an unrelated `AnyWindowHandle`.
        let raw = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = raw.as_raw() else {
            return None;
        };
        // SAFETY: GPUI keeps the GPUIView alive for the window's lifetime; we
        // retain it only to reach its window and content view, then drop it.
        let gpui_view: Retained<NSView> =
            unsafe { Retained::retain(handle.ns_view.as_ptr().cast())? };
        let ns_window: Retained<NSWindow> = gpui_view.window()?;
        let content: Retained<NSView> = ns_window.contentView()?;

        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
        // SAFETY: main thread; standard WKWebView construction.
        let webview = unsafe {
            let config = WKWebViewConfiguration::new(mtm);
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config)
        };
        // Start hidden; the first `sync` reveals it at the real rect.
        unsafe {
            webview.setHidden(true);
            content.addSubview(&webview);
        }

        let view = WebView { webview, content, last: None };
        view.load(url);
        Some(view)
    }

    /// Navigate to `url`.
    pub fn load(&self, url: &str) {
        let s = NSString::from_str(url);
        // SAFETY: main thread; URL may be rejected (returns None) for bad input.
        unsafe {
            if let Some(nsurl) = NSURL::URLWithString(&s) {
                let req = NSURLRequest::requestWithURL(&nsurl);
                let _ = self.webview.loadRequest(&req);
            }
        }
    }

    /// Place the view at `bounds` (GPUI logical pixels, top-left origin) and show
    /// it. Call every paint with the pane's captured rect.
    pub fn sync(&mut self, bounds: Bounds<Pixels>) {
        if self.last == Some(bounds) {
            unsafe { self.webview.setHidden(false) };
            return;
        }
        let x = f64::from(bounds.origin.x);
        let y = f64::from(bounds.origin.y);
        let w = f64::from(bounds.size.width);
        let h = f64::from(bounds.size.height);
        // AppKit's content view is not flipped: y grows upward from the bottom.
        let content_h = unsafe { self.content.frame() }.size.height;
        let frame = NSRect::new(NSPoint::new(x, content_h - (y + h)), NSSize::new(w, h));
        unsafe {
            self.webview.setFrame(frame);
            self.webview.setHidden(false);
        }
        self.last = Some(bounds);
    }

    /// Hide the view without destroying it (pane zoomed away, tab not shown).
    pub fn hide(&mut self) {
        self.webview.setHidden(true);
        self.last = None;
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        // Main thread; removing a subview is safe even if already gone.
        self.webview.removeFromSuperview();
    }
}
