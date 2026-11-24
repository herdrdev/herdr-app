use std::{ffi::CStr, ptr::NonNull};

use anyhow::{Context as _, Result, ensure};
use libc::c_void;
use x11rb::xcb_ffi::XCBConnection;
use xkbcommon::xkb::{self, ffi};

type DeviceId = unsafe extern "C" fn(*mut c_void) -> i32;
type Keymap =
    unsafe extern "C" fn(*mut ffi::xkb_context, *mut c_void, i32, u32) -> *mut ffi::xkb_keymap;
type State = unsafe extern "C" fn(*mut ffi::xkb_keymap, *mut c_void, i32) -> *mut ffi::xkb_state;

// Wayland does not need this library. Keep it out of DT_NEEDED so a Wayland
// desktop without X11 keyboard support can still start the app.
pub(super) struct XkbX11 {
    library: NonNull<c_void>,
    device_id: DeviceId,
    keymap: Keymap,
    state: State,
}

impl XkbX11 {
    pub(super) fn load() -> Result<Self> {
        unsafe {
            let library = NonNull::new(libc::dlopen(c"libxkbcommon-x11.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL))
                .context("X11 needs libxkbcommon-x11.so.0; install libxkbcommon-x11-0 (Ubuntu/Debian) or libxkbcommon-x11 (Fedora/Arch)")?;
            let symbols = (|| {
                Ok(Self {
                    library,
                    device_id: std::mem::transmute::<*mut c_void, DeviceId>(symbol(
                        library,
                        c"xkb_x11_get_core_keyboard_device_id",
                    )?),
                    keymap: std::mem::transmute::<*mut c_void, Keymap>(symbol(
                        library,
                        c"xkb_x11_keymap_new_from_device",
                    )?),
                    state: std::mem::transmute::<*mut c_void, State>(symbol(
                        library,
                        c"xkb_x11_state_new_from_device",
                    )?),
                })
            })();
            if symbols.is_err() {
                libc::dlclose(library.as_ptr());
            }
            symbols
        }
    }

    pub(super) fn device_id(&self, connection: &XCBConnection) -> Result<i32> {
        let id = unsafe { (self.device_id)(connection.get_raw_xcb_connection()) };
        ensure!(id >= 0, "Failed to get X11 keyboard device");
        Ok(id)
    }

    pub(super) fn state(
        &self,
        context: &xkb::Context,
        connection: &XCBConnection,
        device: i32,
    ) -> Result<xkb::State> {
        unsafe {
            let connection = connection.get_raw_xcb_connection();
            let keymap = (self.keymap)(
                context.get_raw_ptr(),
                connection,
                device,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            );
            ensure!(!keymap.is_null(), "Failed to create X11 keyboard keymap");
            let keymap = xkb::Keymap::from_raw_ptr(keymap);
            let state = (self.state)(keymap.get_raw_ptr(), connection, device);
            ensure!(!state.is_null(), "Failed to create X11 keyboard state");
            Ok(xkb::State::from_raw_ptr(state))
        }
    }
}

impl Drop for XkbX11 {
    fn drop(&mut self) {
        unsafe {
            libc::dlclose(self.library.as_ptr());
        }
    }
}

unsafe fn symbol(library: NonNull<c_void>, name: &CStr) -> Result<*mut c_void> {
    let pointer = unsafe { libc::dlsym(library.as_ptr(), name.as_ptr()) };
    ensure!(
        !pointer.is_null(),
        "libxkbcommon-x11.so.0 is missing {}",
        name.to_string_lossy()
    );
    Ok(pointer)
}
