//! The badge on the app's icon (the Dock on macOS), which mods set with
//! `setBadge`: only the app's own process can draw it. Other platforms keep
//! no badge for now; the call is accepted and does nothing there.

use std::cell::RefCell;

thread_local! {
    static SHOWN: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Shows `text` on the app icon; `None` or blank clears it. Runs on the UI thread.
pub fn set(text: Option<&str>) {
    let text = text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned);
    let changed = SHOWN.with_borrow_mut(|shown| {
        if *shown == text {
            return false;
        }
        shown.clone_from(&text);
        true
    });
    if changed {
        platform::set(text.as_deref());
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    pub fn set(text: Option<&str>) {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("setBadge off the main thread");
            return;
        };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        tile.setBadgeLabel(text.map(NSString::from_str).as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub fn set(_text: Option<&str>) {}
}
