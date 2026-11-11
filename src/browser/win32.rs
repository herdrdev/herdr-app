//! Win32 helpers shared by native browser and app views.
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_DIFF},
    UI::WindowsAndMessaging::{EnumChildWindows, GetClassNameW, GetParent},
};

/// Cut `holes` (`[x, y, width, height]`, relative to the window) out of a
/// child window of `width` x `height` device pixels, or show all of it again.
/// Children of the window are clipped by the same region.
pub fn set_holes(hwnd: HWND, width: i32, height: i32, holes: &[[i32; 4]]) {
    unsafe {
        if holes.is_empty() {
            SetWindowRgn(hwnd, std::ptr::null_mut(), 1);
            return;
        }
        let region = CreateRectRgn(0, 0, width, height);
        for &[x, y, w, h] in holes {
            let hole = CreateRectRgn(x, y, x + w, y + h);
            CombineRgn(region, region, hole, RGN_DIFF);
            DeleteObject(hole);
        }
        // The window owns the region from here on.
        if SetWindowRgn(hwnd, region, 1) == 0 {
            DeleteObject(region);
        }
    }
}

/// Direct children of `parent` whose window class is `class`.
pub fn children_of_class(parent: HWND, class: &str) -> Vec<HWND> {
    struct Search<'a> {
        parent: HWND,
        class: &'a str,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> windows_sys::core::BOOL {
        let search = unsafe { &mut *(data as *mut Search) };
        if unsafe { GetParent(hwnd) } != search.parent {
            return 1;
        }
        let mut name = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
        if String::from_utf16_lossy(&name[..len.max(0) as usize]) == search.class {
            search.found.push(hwnd);
        }
        1
    }
    let mut search = Search {
        parent,
        class,
        found: Vec::new(),
    };
    unsafe { EnumChildWindows(parent, Some(visit), (&raw mut search) as LPARAM) };
    search.found
}
