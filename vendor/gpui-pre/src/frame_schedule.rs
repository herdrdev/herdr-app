//! HERDR: timed frame demand.
//!
//! `Window::request_animation_frame` asks for the next display frame, so an
//! element that only changes every 100 ms (an animated image, a spinner step)
//! redraws the whole window on every vsync in between: 120 draws a second on a
//! 120 Hz display for 10 visible changes. Views instead ask to be drawn again
//! at a time, with a tolerance, and the window serves them with one timer
//! per deadline. Requests whose tolerances overlap are served by the same draw, the
//! way OS timer coalescing (timer tolerance, timerslack) batches wakeups, so
//! many indicators stepping at their own phase still cost one draw per step.

use crate::EntityId;
use collections::FxHashMap;
use scheduler::Instant;
use std::time::Duration;

/// One view's pending redraw: not before `due`, at the latest by `latest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Deadline {
    due: Instant,
    latest: Instant,
}

#[derive(Default)]
pub(crate) struct FrameSchedule {
    pending: FxHashMap<EntityId, Deadline>,
    /// When the window's timer fires, if one is armed.
    armed: Option<Instant>,
}

impl FrameSchedule {
    /// Records that `view` wants to be drawn again at `due`, or up to
    /// `tolerance` later. A view keeps its earliest request. Returns the time
    /// the window's timer must fire at, when it has to be (re)armed earlier.
    ///
    /// While armed, the timer is at or before every pending `latest` (requests
    /// only ever move it earlier), so a request compares against it alone.
    pub(crate) fn request(&mut self, view: EntityId, due: Instant, tolerance: Duration) -> Option<Instant> {
        let latest = due + tolerance;
        let deadline = self
            .pending
            .entry(view)
            .and_modify(|d| {
                d.due = d.due.min(due);
                d.latest = d.latest.min(latest);
            })
            .or_insert(Deadline { due, latest });
        let fire = deadline.latest;
        match self.armed {
            Some(armed) if armed <= fire => None,
            _ => {
                self.armed = Some(fire);
                Some(fire)
            }
        }
    }

    /// The latest time that still meets every pending request: the earliest
    /// `latest`. Firing then serves as many requests as possible in one draw.
    /// Only computed once a firing has taken the served requests out.
    fn fire_at(&self) -> Option<Instant> {
        self.pending.values().map(|d| d.latest).min()
    }

    /// The timer armed for `fired` went off at `now`: takes the views that are
    /// due, and returns when to arm the timer next, if anything is left. A
    /// timer that was replaced by an earlier one returns nothing.
    pub(crate) fn fire(&mut self, fired: Instant, now: Instant) -> Option<(Vec<EntityId>, Option<Instant>)> {
        if self.armed != Some(fired) {
            return None;
        }
        self.armed = None;
        let due: Vec<EntityId> = self
            .pending
            .iter()
            .filter(|(_, d)| d.due <= now)
            .map(|(view, _)| *view)
            .collect();
        for view in &due {
            self.pending.remove(view);
        }
        let next = self.fire_at();
        self.armed = next;
        Some((due, next))
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn view(n: u64) -> EntityId {
        EntityId::from(n)
    }

    #[test]
    fn requests_within_each_others_tolerance_share_one_firing() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut s = FrameSchedule::default();
        // Three spinners stepping every 100 ms at different phases, 50 ms tolerance.
        assert_eq!(s.request(view(1), t0 + ms(100), ms(50)), Some(t0 + ms(150)));
        assert_eq!(s.request(view(2), t0 + ms(120), ms(50)), None, "fits the armed time");
        assert_eq!(s.request(view(3), t0 + ms(140), ms(50)), None);
        let (due, next) = s.fire(t0 + ms(150), t0 + ms(150)).unwrap();
        assert_eq!(due.len(), 3, "one draw serves all three");
        assert_eq!(next, None);
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn a_request_that_cannot_wait_rearms_earlier_and_the_old_timer_is_ignored() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut s = FrameSchedule::default();
        assert_eq!(s.request(view(1), t0 + ms(100), ms(50)), Some(t0 + ms(150)));
        assert_eq!(s.request(view(2), t0 + ms(20), ms(5)), Some(t0 + ms(25)));
        let (due, next) = s.fire(t0 + ms(25), t0 + ms(25)).unwrap();
        assert_eq!(due, vec![view(2)], "view 1 is not due yet");
        assert_eq!(next, Some(t0 + ms(150)));
        assert_eq!(s.fire(t0 + ms(150), t0 + ms(150)).unwrap().0, vec![view(1)]);
        assert!(s.fire(t0 + ms(25), t0 + ms(200)).is_none(), "a stale timer does nothing");
    }

    #[test]
    fn a_view_keeps_its_earliest_request() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut s = FrameSchedule::default();
        s.request(view(1), t0 + ms(100), ms(10));
        s.request(view(1), t0 + ms(300), ms(10));
        assert_eq!(s.pending(), 1);
        assert_eq!(s.fire(t0 + ms(110), t0 + ms(110)).unwrap().0, vec![view(1)]);
    }
}
