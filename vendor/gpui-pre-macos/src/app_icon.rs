// HERDR: lets a window started beside another (its own app home) show its own
// Dock icon, the macOS twin of the `app_id` Linux docks use.
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSString;
use std::path::Path;

/// Shows the image at `path` as this process's Dock and app switcher icon
/// until it quits. False off the main thread or when macOS can't load it.
pub fn set_app_icon(path: &Path) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let Some(path) = path.to_str() else { return false };
    let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path)) else {
        return false;
    };
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    true
}
