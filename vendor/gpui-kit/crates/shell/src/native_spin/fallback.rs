//! Platforms without native spinners: GPUI draws them all.

use gpui::{App, EntityId, Window};

use super::Spinner;

pub(crate) fn enabled(_: &mut Window, _: &mut App) -> bool {
    false
}

pub(crate) fn covered(_: u64) -> bool {
    true
}

pub(crate) fn place(_: u64, _: EntityId, _: Spinner, _: &mut Window) {}
