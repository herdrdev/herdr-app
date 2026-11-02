//! Fractional scroll positions and bounded wheel motion for native history.
//! Positions are rows above the live bottom. Rendering needs an older anchor
//! plus one guard row below it whenever the position is fractional.

use std::time::Duration;

const TRANSITION: Duration = Duration::from_millis(120);
const EPSILON: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollOwner {
    NativeHistory,
    ApplicationMouse,
    AlternateScreen,
}

pub fn scroll_owner(mouse_tracking: bool, alternate_screen: bool) -> ScrollOwner {
    if mouse_tracking {
        ScrollOwner::ApplicationMouse
    } else if alternate_screen {
        ScrollOwner::AlternateScreen
    } else {
        ScrollOwner::NativeHistory
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presentation {
    pub anchor_rows_above_bottom: u32,
    pub translate_y_px: f64,
    pub needs_bottom_guard_row: bool,
}

impl Presentation {
    /// A displayed row index, including the guard row; the integration must
    /// convert this to a scrollback/screen coordinate before selecting text.
    pub fn displayed_row_at(self, y_px: f64, row_height_px: f64) -> Option<u32> {
        if !y_px.is_finite() || y_px < 0.0 || !row_height_px.is_finite() || row_height_px <= 0.0 {
            return None;
        }
        let y = (y_px - self.translate_y_px) / row_height_px;
        if y < 0.0 || y > u32::MAX as f64 {
            return None;
        }
        Some(y.floor() as u32)
    }
}

#[derive(Clone, Copy, Debug)]
struct Transition {
    from: f64,
    started: Duration,
}

#[derive(Debug)]
pub struct SmoothScroll {
    position: f64,
    target: f64,
    extent_rows: u32,
    transition: Option<Transition>,
    animate_wheel: bool,
}

impl SmoothScroll {
    pub fn new(extent_rows: u32, animate_wheel: bool) -> Self {
        Self {
            position: 0.0,
            target: 0.0,
            extent_rows,
            transition: None,
            animate_wheel,
        }
    }

    pub fn set_animate_wheel(&mut self, enabled: bool) {
        self.animate_wheel = enabled;
        if !enabled {
            self.position = self.target;
            self.transition = None;
        }
    }

    fn bounded(&self, value: f64) -> f64 {
        value.clamp(0.0, self.extent_rows as f64)
    }

    pub fn advance(&mut self, now: Duration) -> bool {
        let Some(transition) = self.transition else {
            return false;
        };
        let elapsed = now.saturating_sub(transition.started);
        if elapsed >= TRANSITION {
            self.position = self.target;
            self.transition = None;
            return false;
        }
        let t = elapsed.as_secs_f64() / TRANSITION.as_secs_f64();
        let eased = 1.0 - (1.0 - t).powi(3);
        self.position = self.bounded(transition.from + (self.target - transition.from) * eased);
        true
    }

    /// High-resolution gestures already carry OS momentum. Follow their pixel
    /// positions directly rather than adding another inertia/animation layer.
    pub fn touchpad_pixels(&mut self, delta_px: f64, row_height_px: f64) {
        if !delta_px.is_finite() || !row_height_px.is_finite() || row_height_px <= 0.0 {
            return;
        }
        self.position = self.bounded(self.position + delta_px / row_height_px);
        self.target = self.position;
        self.transition = None;
    }

    /// Wheel distance keeps the existing upstream speed; only presentation
    /// changes. Opposite input cancels old queued motion immediately.
    pub fn wheel_rows(&mut self, delta_rows: f64, now: Duration) {
        if !delta_rows.is_finite() || delta_rows == 0.0 {
            return;
        }
        self.advance(now);
        let pending = self.target - self.position;
        let anchor = if pending * delta_rows < 0.0 {
            self.position
        } else {
            self.target
        };
        self.target = self.bounded(anchor + delta_rows);
        if !self.animate_wheel || (self.target - self.position).abs() <= EPSILON {
            self.position = self.target;
            self.transition = None;
        } else {
            self.transition = Some(Transition {
                from: self.position,
                started: now,
            });
        }
    }

    pub fn set_extent(&mut self, extent_rows: u32) {
        self.extent_rows = extent_rows;
        self.position = self.bounded(self.position);
        self.target = self.bounded(self.target);
        // A truncated history invalidates an old animation's starting point.
        self.transition = None;
        self.target = self.position;
    }

    /// Output moves the live bottom. Keep the same content under a reader who
    /// is in history; a reader already at the bottom continues following it.
    pub fn output_appended(&mut self, extent_rows: u32, appended_rows: u32) {
        let anchored_in_history = self.position > EPSILON;
        self.extent_rows = extent_rows;
        if anchored_in_history {
            let added = appended_rows as f64;
            self.position += added;
            self.target += added;
            if let Some(transition) = &mut self.transition {
                transition.from += added;
            }
        }
        self.position = self.bounded(self.position);
        self.target = self.bounded(self.target);
        if (self.position - self.target).abs() <= EPSILON {
            self.transition = None;
        }
    }

    pub fn jump_to(&mut self, rows_above_bottom: f64) {
        if !rows_above_bottom.is_finite() {
            return;
        }
        self.position = self.bounded(rows_above_bottom);
        self.target = self.position;
        self.transition = None;
    }

    pub fn presentation(&self, row_height_px: f64) -> Option<Presentation> {
        if !row_height_px.is_finite() || row_height_px <= 0.0 {
            return None;
        }
        let rounded = self.position.round();
        let position = if (self.position - rounded).abs() <= EPSILON {
            rounded
        } else {
            self.position
        };
        let anchor = position.ceil() as u32;
        let remainder = anchor as f64 - position;
        Some(Presentation {
            anchor_rows_above_bottom: anchor,
            translate_y_px: -remainder * row_height_px,
            needs_bottom_guard_row: remainder > 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }
    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-7, "{a} != {b}");
    }

    #[test]
    fn a_quarter_row_is_visible_before_a_whole_row_accumulates() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.touchpad_pixels(5.0, 20.0);
        let p = scroll.presentation(20.0).unwrap();
        assert_eq!(p.anchor_rows_above_bottom, 1);
        close(p.translate_y_px, -15.0);
        assert!(p.needs_bottom_guard_row);
    }

    #[test]
    fn whole_rows_have_no_translation_or_guard() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.touchpad_pixels(40.0, 20.0);
        let p = scroll.presentation(20.0).unwrap();
        assert_eq!(p.anchor_rows_above_bottom, 2);
        close(p.translate_y_px, 0.0);
        assert!(!p.needs_bottom_guard_row);
    }

    #[test]
    fn wheel_input_has_a_bounded_transition_and_settles() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.wheel_rows(3.0, ms(0));
        assert!(scroll.advance(ms(60)));
        assert!(scroll.position > 0.0 && scroll.position < 3.0);
        assert!(!scroll.advance(ms(120)));
        close(scroll.position, 3.0);
        assert!(!scroll.advance(ms(1000)));
    }

    #[test]
    fn equal_elapsed_time_has_equal_position_at_different_refresh_rates() {
        let mut slow = SmoothScroll::new(100, true);
        let mut fast = SmoothScroll::new(100, true);
        slow.wheel_rows(3.0, ms(0));
        fast.wheel_rows(3.0, ms(0));
        for t in [16, 33, 50, 67, 83] {
            slow.advance(ms(t));
        }
        for t in (0..90).step_by(7) {
            fast.advance(ms(t));
        }
        slow.advance(ms(90));
        fast.advance(ms(90));
        close(slow.position, fast.position);
    }

    #[test]
    fn opposite_input_immediately_reverses_pending_motion() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.wheel_rows(12.0, ms(0));
        scroll.advance(ms(30));
        let reversed_at = scroll.position;
        scroll.wheel_rows(-3.0, ms(30));
        scroll.advance(ms(31));
        assert!(scroll.position < reversed_at);
        scroll.advance(ms(150));
        close(scroll.position, reversed_at - 3.0);
    }

    #[test]
    fn repeated_wheel_input_preserves_requested_distance_without_jumping() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.wheel_rows(3.0, ms(0));
        scroll.advance(ms(30));
        let before = scroll.position;
        scroll.wheel_rows(3.0, ms(30));
        close(scroll.position, before);
        scroll.advance(ms(150));
        close(scroll.position, 6.0);
    }

    #[test]
    fn direct_gestures_do_not_get_a_second_momentum_layer() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.touchpad_pixels(7.0, 20.0);
        let before = scroll.position;
        assert!(!scroll.advance(ms(10_000)));
        close(scroll.position, before);
    }

    #[test]
    fn direct_input_cancels_a_pending_wheel_animation() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.wheel_rows(10.0, ms(0));
        scroll.advance(ms(30));
        scroll.touchpad_pixels(-1.0, 20.0);
        let before = scroll.position;
        assert!(!scroll.advance(ms(1000)));
        close(scroll.position, before);
    }

    #[test]
    fn reduced_motion_wheel_has_no_synthetic_animation() {
        let mut scroll = SmoothScroll::new(100, false);
        scroll.wheel_rows(3.0, ms(0));
        close(scroll.position, 3.0);
        assert!(!scroll.advance(ms(1)));
    }

    #[test]
    fn range_edges_never_overscroll() {
        let mut scroll = SmoothScroll::new(5, true);
        scroll.touchpad_pixels(-100.0, 20.0);
        close(scroll.position, 0.0);
        scroll.wheel_rows(1000.0, ms(0));
        scroll.advance(ms(120));
        close(scroll.position, 5.0);
        scroll.touchpad_pixels(10000.0, 20.0);
        close(scroll.position, 5.0);
    }

    #[test]
    fn empty_history_never_schedules_animation() {
        let mut scroll = SmoothScroll::new(0, true);
        scroll.wheel_rows(3.0, ms(0));
        assert!(!scroll.advance(ms(1)));
        close(scroll.position, 0.0);
    }

    #[test]
    fn truncated_history_clamps_and_cancels_stale_motion() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.jump_to(80.0);
        scroll.wheel_rows(10.0, ms(0));
        scroll.set_extent(5);
        close(scroll.position, 5.0);
        assert!(!scroll.advance(ms(1000)));
    }

    #[test]
    fn output_moves_the_bottom_without_moving_the_readers_content() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.jump_to(7.25);
        scroll.output_appended(110, 10);
        close(scroll.position, 17.25);
    }

    #[test]
    fn output_still_follows_a_reader_at_the_live_bottom() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.output_appended(110, 10);
        close(scroll.position, 0.0);
        assert!(!scroll.advance(ms(1000)));
    }

    #[test]
    fn an_output_append_preserves_in_flight_animation_progress() {
        let mut baseline = SmoothScroll::new(100, true);
        let mut with_output = SmoothScroll::new(100, true);
        for scroll in [&mut baseline, &mut with_output] {
            scroll.jump_to(10.0);
            scroll.wheel_rows(3.0, ms(0));
            scroll.advance(ms(30));
        }
        with_output.output_appended(110, 10);
        baseline.advance(ms(60));
        with_output.advance(ms(60));
        close(with_output.position - baseline.position, 10.0);
    }

    #[test]
    fn recycled_history_clamps_an_anchor_that_has_disappeared() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.jump_to(99.0);
        scroll.output_appended(100, 10);
        close(scroll.position, 100.0);
        assert!(!scroll.advance(ms(1000)));
    }

    #[test]
    fn returning_to_live_bottom_cancels_pending_motion() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.wheel_rows(20.0, ms(0));
        scroll.jump_to(0.0);
        assert!(!scroll.advance(ms(60)));
        close(scroll.position, 0.0);
    }

    #[test]
    fn changing_font_height_preserves_the_logical_row_anchor() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.jump_to(1.25);
        let a = scroll.presentation(20.0).unwrap();
        let b = scroll.presentation(40.0).unwrap();
        assert_eq!(a.anchor_rows_above_bottom, b.anchor_rows_above_bottom);
        close(b.translate_y_px, a.translate_y_px * 2.0);
    }

    #[test]
    fn pointer_mapping_includes_the_partially_displayed_guard_row() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.jump_to(0.25);
        let p = scroll.presentation(20.0).unwrap();
        assert_eq!(p.displayed_row_at(2.0, 20.0), Some(0));
        assert_eq!(p.displayed_row_at(6.0, 20.0), Some(1));
        assert_eq!(p.displayed_row_at(799.0, 20.0), Some(40));
    }

    #[test]
    fn invalid_input_never_contaminates_the_position() {
        let mut scroll = SmoothScroll::new(100, true);
        scroll.touchpad_pixels(f64::NAN, 20.0);
        scroll.touchpad_pixels(1.0, 0.0);
        scroll.wheel_rows(f64::INFINITY, ms(0));
        scroll.jump_to(f64::NEG_INFINITY);
        close(scroll.position, 0.0);
        assert_eq!(scroll.presentation(0.0), None);
    }

    #[test]
    fn app_owned_modes_are_explicitly_outside_native_scrollback() {
        assert_eq!(scroll_owner(true, false), ScrollOwner::ApplicationMouse);
        assert_eq!(scroll_owner(true, true), ScrollOwner::ApplicationMouse);
        assert_eq!(scroll_owner(false, true), ScrollOwner::AlternateScreen);
        assert_eq!(scroll_owner(false, false), ScrollOwner::NativeHistory);
    }

    #[test]
    fn long_small_gestures_keep_translation_within_a_single_row() {
        let mut scroll = SmoothScroll::new(10_000, true);
        for _ in 0..100_000 {
            scroll.touchpad_pixels(0.17, 17.0);
            let p = scroll.presentation(17.0).unwrap();
            assert!(p.translate_y_px > -17.0 && p.translate_y_px <= 0.0);
        }
        close(scroll.position, 1000.0);
    }

    #[test]
    fn wheel_input_already_at_a_boundary_does_not_keep_frames_alive() {
        let mut scroll = SmoothScroll::new(5, true);
        scroll.jump_to(5.0);
        scroll.wheel_rows(3.0, ms(0));
        assert!(!scroll.advance(ms(10)));
        close(scroll.position, 5.0);
    }
}
