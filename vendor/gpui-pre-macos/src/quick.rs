// HERDR: Herdr's quick terminal. A system-wide key (Carbon hot keys need no
// Accessibility permission) and the window as a drop-down panel. Call these
// from a foreground task, never inside a window update: a frame change or
// makeKey re-enters GPUI.

use cocoa::{
    base::{BOOL, NO, YES, id, nil},
    foundation::{NSPoint, NSRect, NSSize},
};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle as rwh;
use std::{cell::Cell, ffi::c_void, ptr::null_mut};

/// How long the panel slides, in seconds.
const SLIDE: f64 = 0.15;
/// NSWindowStyleMaskNonactivatingPanel alone: no border, and showing it leaves
/// the frontmost app frontmost, so hiding it gives that app the keys back.
const NONACTIVATING_BORDERLESS: u64 = 1 << 7;

type OSStatus = i32;
type EventRef = *mut c_void;
type EventHandlerUPP = extern "C" fn(*mut c_void, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;
const HOT_KEY_EXISTS: OSStatus = -9878;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandlerUPP,
        num_types: u32,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> OSStatus;
}

thread_local! {
    static HOT_KEY: Cell<*mut c_void> = const { Cell::new(null_mut()) };
    static ON_PRESS: Cell<Option<fn()>> = const { Cell::new(None) };
}

extern "C" fn pressed(_: *mut c_void, _: EventRef, _: *mut c_void) -> OSStatus {
    if let Some(on_press) = ON_PRESS.get() {
        on_press();
    }
    0
}

/// Registers the one system-wide key (a virtual key code and Carbon modifier
/// bits) in place of the last; `None` removes it.
pub fn set_hotkey(key: Option<(u32, u32)>, on_press: fn()) -> Result<(), String> {
    unsafe {
        let old = HOT_KEY.replace(null_mut());
        if !old.is_null() {
            UnregisterEventHotKey(old);
        }
        let Some((code, modifiers)) = key else {
            return Ok(());
        };
        if ON_PRESS.replace(Some(on_press)).is_none() {
            let spec = EventTypeSpec { event_class: EVENT_CLASS_KEYBOARD, event_kind: EVENT_HOT_KEY_PRESSED };
            let status = InstallEventHandler(GetApplicationEventTarget(), pressed, 1, &spec, null_mut(), null_mut());
            if status != 0 {
                ON_PRESS.set(None);
                return Err(format!("cannot listen for the key (error {status})"));
            }
        }
        let id = EventHotKeyID { signature: u32::from_be_bytes(*b"hrdr"), id: 1 };
        let mut hot_key = null_mut();
        match RegisterEventHotKey(code, modifiers, id, GetApplicationEventTarget(), 0, &mut hot_key) {
            0 => {
                HOT_KEY.set(hot_key);
                Ok(())
            }
            HOT_KEY_EXISTS => Err("another app already uses this key".into()),
            status => Err(format!("cannot use this key (error {status})")),
        }
    }
}

/// The window as a drop-down: opened as a `WindowKind::PopUp`, a nonactivating
/// NSPanel on every Space, also over full screen apps.
#[derive(Clone, Copy)]
pub struct QuickPanel {
    window: id,
    view: id,
}

impl QuickPanel {
    pub fn new(window: &gpui::Window) -> Option<Self> {
        let handle = rwh::HasWindowHandle::window_handle(window).ok()?;
        let rwh::RawWindowHandle::AppKit(h) = handle.as_raw() else {
            return None;
        };
        let view = h.ns_view.as_ptr() as id;
        let window: id = unsafe { msg_send![view, window] };
        (!window.is_null()).then_some(QuickPanel { window, view })
    }

    /// No border, can't be dragged, and stays up when Herdr isn't the active app.
    pub fn style(self) {
        unsafe {
            let () = msg_send![self.window, setStyleMask: NONACTIVATING_BORDERLESS];
            let () = msg_send![self.window, setHidesOnDeactivate: NO];
            let () = msg_send![self.window, setMovable: NO];
            let _: BOOL = msg_send![self.window, makeFirstResponder: self.view];
        }
    }

    /// On screen and taking the keys.
    pub fn shown_and_key(self) -> bool {
        unsafe {
            let visible: BOOL = msg_send![self.window, isVisible];
            let key: BOOL = msg_send![self.window, isKeyWindow];
            visible == YES && key == YES
        }
    }

    /// Drops down from the top of the screen under the pointer, `height` of
    /// it tall, and takes the keys. `full`: the whole screen, over the menu
    /// bar and the Dock (a pop-up panel sits above both). `animate`: slides
    /// down, else just appears.
    pub fn show(self, height: f64, full: bool, animate: bool) {
        unsafe {
            let app: id = msg_send![class!(NSApplication), sharedApplication];
            let () = msg_send![class!(NSObject), cancelPreviousPerformRequestsWithTarget: self.window selector: sel!(orderOut:) object: nil];
            let () = msg_send![class!(NSObject), cancelPreviousPerformRequestsWithTarget: app selector: sel!(hide:) object: nil];
            let () = msg_send![app, unhideWithoutActivation];
            let area = screen_under_pointer(full);
            let h = if full { area.size.height } else { (area.size.height * height).round() };
            let to = NSRect::new(
                NSPoint::new(area.origin.x, area.origin.y + area.size.height - h),
                NSSize::new(area.size.width, h),
            );
            let visible: BOOL = msg_send![self.window, isVisible];
            if !animate {
                let () = msg_send![self.window, setFrame: to display: YES];
            } else if visible == NO {
                // A point still on the screen: off every screen, AppKit doesn't animate it.
                let above = NSRect::new(NSPoint::new(to.origin.x, to.origin.y + h - 1.0), to.size);
                let () = msg_send![self.window, setFrame: above display: NO];
            }
            let () = msg_send![self.window, makeKeyAndOrderFront: nil];
            let _: BOOL = msg_send![self.window, makeFirstResponder: self.view];
            if animate {
                slide(self.window, to);
            }
        }
    }

    /// Slides up and off the screen (or just goes, without `animate`); the
    /// app that was in front has the keys again.
    pub fn hide(self, animate: bool) {
        unsafe {
            let visible: BOOL = msg_send![self.window, isVisible];
            if visible == NO {
                return;
            }
            let delay = if animate {
                let frame: NSRect = msg_send![self.window, frame];
                slide(self.window, NSRect::new(NSPoint::new(frame.origin.x, frame.origin.y + frame.size.height), frame.size));
                SLIDE
            } else {
                0.0
            };
            let () = msg_send![self.window, performSelector: sel!(orderOut:) withObject: nil afterDelay: delay];
            // Herdr came to the front (the Dock, Cmd-Tab): hiding the app hands
            // the keys to the app before it, not to a Herdr with no window.
            if frontmost() {
                let app: id = msg_send![class!(NSApplication), sharedApplication];
                let () = msg_send![app, performSelector: sel!(hide:) withObject: nil afterDelay: delay];
            }
        }
    }
}

/// Whether Herdr is the app in front (`NSApp.isActive` also says yes while
/// only the nonactivating panel has the keys).
unsafe fn frontmost() -> bool {
    unsafe {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let front: id = msg_send![workspace, frontmostApplication];
        if front.is_null() {
            return false;
        }
        let pid: i32 = msg_send![front, processIdentifier];
        pid as u32 == std::process::id()
    }
}

unsafe fn slide(window: id, to: NSRect) {
    unsafe {
        let () = msg_send![class!(NSAnimationContext), beginGrouping];
        let context: id = msg_send![class!(NSAnimationContext), currentContext];
        let () = msg_send![context, setDuration: SLIDE];
        let animator: id = msg_send![window, animator];
        let () = msg_send![animator, setFrame: to display: YES];
        let () = msg_send![class!(NSAnimationContext), endGrouping];
    }
}

/// The screen the pointer is on: all of it, or the part the menu bar and the
/// Dock leave.
unsafe fn screen_under_pointer(full: bool) -> NSRect {
    unsafe {
        let at: NSPoint = msg_send![class!(NSEvent), mouseLocation];
        let screens: id = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        for i in 0..count {
            let screen: id = msg_send![screens, objectAtIndex: i];
            let f: NSRect = msg_send![screen, frame];
            let inside = at.x >= f.origin.x
                && at.x <= f.origin.x + f.size.width
                && at.y >= f.origin.y
                && at.y <= f.origin.y + f.size.height;
            if inside {
                return if full { f } else { msg_send![screen, visibleFrame] };
            }
        }
        let main: id = msg_send![class!(NSScreen), mainScreen];
        if full { msg_send![main, frame] } else { msg_send![main, visibleFrame] }
    }
}
