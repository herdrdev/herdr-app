//! Optional native drawing. Terminal state and all interaction stay in Herdr.
use ghostty_sys::{GhosttyRenderState, GhosttyTerminal};
use gpui::{Bounds, Pixels, Size, Window};

#[cfg(all(target_os = "macos", feature = "ghostty-renderer"))]
mod metal;

#[derive(Default)]
pub struct Renderer {
    #[cfg(all(target_os = "macos", feature = "ghostty-renderer"))]
    native: metal::Renderer,
}

impl Renderer {
    pub fn active(&self) -> bool {
        #[cfg(all(target_os = "macos", feature = "ghostty-renderer"))]
        {
            return self.native.active();
        }
        #[cfg(not(all(target_os = "macos", feature = "ghostty-renderer")))]
        false
    }

    pub fn prepare(&mut self, window: &Window) -> Option<Size<Pixels>> {
        #[cfg(all(target_os = "macos", feature = "ghostty-renderer"))]
        {
            return self.native.prepare(window);
        }
        #[cfg(not(all(target_os = "macos", feature = "ghostty-renderer")))]
        {
            let _ = window;
            None
        }
    }

    pub fn draw(
        &mut self,
        terminal: GhosttyTerminal,
        snapshot: GhosttyRenderState,
        focused: bool,
        full: bool,
        changed: bool,
    ) -> bool {
        #[cfg(all(target_os = "macos", feature = "ghostty-renderer"))]
        {
            return self.native.draw(terminal, snapshot, focused, full, changed);
        }
        #[cfg(not(all(target_os = "macos", feature = "ghostty-renderer")))]
        {
            let _ = (terminal, snapshot, focused, full, changed);
            false
        }
    }

    pub fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        #[cfg(all(target_os = "macos", feature = "ghostty-renderer"))]
        self.native.paint(bounds, window);
        #[cfg(not(all(target_os = "macos", feature = "ghostty-renderer")))]
        let _ = (bounds, window);
    }
}

/// Register the same in-memory fonts with the native drawing backend.
pub fn register_fonts(fonts: &[std::borrow::Cow<'_, [u8]>]) {
    #[cfg(all(target_os = "macos", feature = "ghostty-renderer"))]
    metal::register_fonts(fonts);
    #[cfg(not(all(target_os = "macos", feature = "ghostty-renderer")))]
    let _ = fonts;
}

/// Match the first cell-size report sent to a new program to its renderer.
pub fn initial_cell_size(window: &Window) -> Option<Size<Pixels>> {
    #[cfg(all(target_os = "macos", feature = "ghostty-renderer"))]
    {
        return metal::initial_cell_size(window);
    }
    #[cfg(not(all(target_os = "macos", feature = "ghostty-renderer")))]
    {
        let _ = window;
        None
    }
}
