//! Built-in macOS VM capability. Layouts remain ordinary mod panes.
use serde_json::{Value, json};

pub fn status() -> Value {
    let supported = cfg!(all(
        target_os = "macos",
        target_arch = "aarch64",
        feature = "native-vm"
    ));
    let directory = std::env::var_os("HERDR_VM_HOME");
    let state = directory
        .as_ref()
        .and_then(|p| std::fs::read(std::path::Path::new(p).join("status.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    json!({"supported": supported, "configured": directory.is_some(), "session": state})
}

pub fn command(command: i32) -> Result<(), String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "native-vm"))]
    if std::env::var_os("HERDR_VM_HOME").is_some() {
        // HostModule callbacks execute on the AppKit thread.
        unsafe { native::herdr_vm_command(command) };
        return Ok(());
    }
    let _ = command;
    Err("Local VM support requires an Apple Silicon native-vm build and HERDR_VM_HOME".into())
}

pub fn install_lifecycle() {
    #[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "native-vm"))]
    if std::env::var_os("HERDR_VM_HOME").is_some() {
        unsafe { native::herdr_vm_install_lifecycle() }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "native-vm"))]
pub use native::{begin_frame, pane};

#[cfg(all(target_os = "macos", target_arch = "aarch64", feature = "native-vm"))]
mod native {
    use gpui::{AnyElement, IntoElement, Styled, canvas};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    unsafe extern "C" {
        pub(super) fn herdr_vm_install_lifecycle();
        pub(super) fn herdr_vm_command(command: i32);
        fn herdr_vm_begin_frame();
        fn herdr_vm_mark_visible();
        fn herdr_vm_show(
            host: *mut std::ffi::c_void,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            slot: *const std::ffi::c_char,
            focus: extern "C" fn(*const std::ffi::c_char),
        );
    }

    pub fn begin_frame() {
        // GPUI's headless test renderer has no AppKit event loop. The bridge is
        // activated only by the explicit experimental VM launch configuration.
        if std::env::var_os("HERDR_VM_HOME").is_none() {
            return;
        }
        // GPUI renders on the AppKit main thread; Swift requires the same thread.
        unsafe { herdr_vm_begin_frame() }
    }

    extern "C" fn focus_pane(slot: *const std::ffi::c_char) {
        // Swift calls synchronously with a live UTF-8 C string on the AppKit thread.
        let slot = unsafe { std::ffi::CStr::from_ptr(slot) }.to_string_lossy();
        crate::mods::request(crate::mods::Request::Run {
            name: "pane.focus".into(),
            args: serde_json::json!({"pane": slot}),
        });
    }

    pub fn pane(slot: &str) -> AnyElement {
        unsafe { herdr_vm_mark_visible() };
        let slot = slot.to_owned();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let Ok(slot) = std::ffi::CString::new(slot.as_str()) else {
                    return;
                };
                let Ok(handle) = window.window_handle() else {
                    return;
                };
                let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                    return;
                };
                let bounds = bounds.intersect(&window.content_mask().bounds);
                unsafe {
                    herdr_vm_show(
                        handle.ns_view.as_ptr(),
                        f32::from(bounds.origin.x) as f64,
                        f32::from(bounds.origin.y) as f64,
                        f32::from(bounds.size.width) as f64,
                        f32::from(bounds.size.height) as f64,
                        slot.as_ptr(),
                        focus_pane,
                    );
                }
            },
        )
        .size_full()
        .into_any_element()
    }
}
