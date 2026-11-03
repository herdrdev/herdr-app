//! HERDR: spinners the system compositor turns.
//!
//! GPUI repaints and presents the whole window for any change, so a spinner
//! GPUI draws costs a frame per step; that's why they step at ~10 fps (see
//! `spin_layer`). Where the platform can, a spinner is instead a native layer
//! over the window with a repeating rotation: the compositor turns it every
//! display frame and the app does no work until the spinner moves, changes or
//! goes.
//!
//! GPUI still decides where it is. A spinner paints a native anchor where it
//! would be drawn, so a cached view replays it and the frame can tell whether
//! anything was painted over it. A covered spinner is drawn by GPUI again
//! ([`covered`]) until it's clear, and a spinner whose anchor isn't painted
//! any more takes its layer with it.

use gpui::{Bounds, EntityId, Hsla, Pixels, SharedString};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{covered, enabled, place};

#[cfg(not(target_os = "macos"))]
mod fallback;
#[cfg(not(target_os = "macos"))]
pub(crate) use fallback::{covered, enabled, place};

/// A spinner as GPUI would draw it this frame.
#[derive(Clone)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct Spinner {
    /// Where the icon is drawn, unclipped, in window pixels.
    pub bounds: Bounds<Pixels>,
    pub path: SharedString,
    /// With every opacity it is painted under.
    pub color: Hsla,
    /// How long one turn takes.
    pub ms: f64,
}

/// The name of the `slot`th spinner `view` draws.
pub(crate) fn key(view: EntityId, slot: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (view, slot).hash(&mut hasher);
    hasher.finish()
}
