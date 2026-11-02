//! Presentation of application-owned scroll redraws. Input remains the
//! application's: only coherent row movement after wheel input is animated.

use std::collections::HashSet;
use std::ops::Range;
use std::time::Duration;

const ANIMATION: Duration = Duration::from_millis(120);
const RESPONSE_WINDOW: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollRegion {
    pub rows: Range<usize>,
    /// New row r contains the former row r + delta.
    pub delta: isize,
}

/// Match a single scrolling region without moving fixed headers or footers.
/// Hashes distinguish nonblank anchors; row equality verifies the motion.
fn detect<R: PartialEq>(
    old: &[R],
    new: &[R],
    direction: isize,
    mut anchor: impl FnMut(&R) -> Option<u64>,
) -> Option<ScrollRegion> {
    if old.len() != new.len() || new.len() < 5 || direction == 0 {
        return None;
    }
    let start = old.iter().zip(new).position(|(a, b)| a != b)?;
    let end = new.len()
        - old
            .iter()
            .zip(new)
            .rev()
            .take_while(|(a, b)| a == b)
            .count();
    let keys: Vec<_> = new.iter().map(&mut anchor).collect();
    let mut best: Option<(usize, ScrollRegion)> = None;
    let mut ambiguous = false;
    for distance in 1..=(end - start) / 2 {
        let delta = distance as isize * direction.signum();
        let mut run = start;
        while run < end {
            let matches = |r: usize| {
                let previous = r as isize + delta;
                previous >= start as isize
                    && previous < end as isize
                    && old[previous as usize] == new[r]
            };
            if !matches(run) {
                run += 1;
                continue;
            }
            let first = run;
            let mut anchors = HashSet::new();
            while run < end && matches(run) {
                if old[run] != new[run] {
                    if let Some(key) = keys[run] {
                        anchors.insert(key);
                    }
                }
                run += 1;
            }
            if anchors.len() < 3 {
                continue;
            }
            let region = ScrollRegion {
                rows: first.min((first as isize + delta) as usize)
                    ..run.max((run as isize + delta) as usize),
                delta,
            };
            let score = (run - first) + anchors.len() * 2;
            match &best {
                None => {
                    best = Some((score, region));
                    ambiguous = false;
                }
                Some((previous, _)) if score > *previous => {
                    best = Some((score, region));
                    ambiguous = false;
                }
                Some((previous, _)) if score == *previous => ambiguous = true,
                _ => {}
            }
        }
    }
    if ambiguous {
        None
    } else {
        best.map(|(_, region)| region)
    }
}

#[derive(Clone, Debug)]
pub struct Layer<R> {
    pub region: Range<usize>,
    /// Absolute screen row of rows[0], including outgoing guard rows.
    pub first_row: isize,
    pub rows: Vec<R>,
    pub offset_rows: f64,
}

struct Motion<R> {
    layer: Layer<R>,
    from: f64,
    started: Duration,
}

pub struct ApplicationScroll<R> {
    previous: Vec<R>,
    intent: Option<(isize, Duration)>,
    motion: Option<Motion<R>>,
}

impl<R> Default for ApplicationScroll<R> {
    fn default() -> Self {
        Self {
            previous: Vec::new(),
            intent: None,
            motion: None,
        }
    }
}

impl<R: Clone + PartialEq> ApplicationScroll<R> {
    /// Wheel direction, not a fabricated terminal scroll command. Programs
    /// still receive the exact same encoded events as before.
    pub fn wheel(&mut self, rows: i32, now: Duration) {
        if rows != 0 {
            self.intent = Some((-(rows.signum() as isize), now + RESPONSE_WINDOW));
        }
    }

    pub fn finish(&mut self) -> bool {
        self.intent = None;
        self.motion.take().is_some()
    }

    pub fn reset(&mut self) {
        self.finish();
        self.previous.clear();
    }

    fn advance(&mut self, now: Duration) {
        if let Some(motion) = &mut self.motion {
            let elapsed = now.saturating_sub(motion.started);
            if elapsed >= ANIMATION {
                self.motion = None;
                return;
            }
            let remaining = 1.0 - elapsed.as_secs_f64() / ANIMATION.as_secs_f64();
            motion.layer.offset_rows = motion.from * remaining.powi(3);
        }
    }

    #[cfg(test)]
    pub fn update(
        &mut self,
        rows: &[R],
        now: Duration,
        anchor: impl FnMut(&R) -> Option<u64>,
    ) -> Option<ScrollRegion> {
        self.update_with_overlays(rows, now, anchor, |_, _| false)
    }

    /// A small background-backed overlay may repaint inside the moving area.
    /// Its caller paints those changed cells at their fixed screen positions.
    pub fn update_with_overlays(
        &mut self,
        rows: &[R],
        now: Duration,
        anchor: impl FnMut(&R) -> Option<u64>,
        mut overlay: impl FnMut(&R, &R) -> bool,
    ) -> Option<ScrollRegion> {
        self.advance(now);
        if self.previous == rows {
            return None;
        }
        let detected = self
            .intent
            .filter(|(_, deadline)| now <= *deadline)
            .and_then(|(direction, _)| detect(&self.previous, rows, direction, anchor));
        let observed = detected.clone();
        if let Some(scroll) = detected {
            let prior = self.motion.take().filter(|m| m.layer.region == scroll.rows);
            let (first, old, offset) = match prior {
                Some(m) => (m.layer.first_row, m.layer.rows, m.layer.offset_rows),
                None => (
                    scroll.rows.start as isize,
                    self.previous[scroll.rows.clone()].to_vec(),
                    0.0,
                ),
            };
            let offset = offset + scroll.delta as f64;
            if offset.abs() > 1e-9 && offset.abs() < scroll.rows.len() as f64 {
                let top = scroll.rows.start as isize - offset.max(0.0).ceil() as isize;
                let bottom = scroll.rows.end as isize + (-offset).max(0.0).ceil() as isize;
                let mut buffer = Vec::new();
                for at in top..bottom {
                    let value = if scroll.rows.contains(&(at as usize)) && at >= 0 {
                        rows.get(at as usize)
                    } else {
                        let old_at = at + scroll.delta - first;
                        (old_at >= 0).then(|| old.get(old_at as usize)).flatten()
                    };
                    let Some(value) = value else {
                        buffer.clear();
                        break;
                    };
                    buffer.push(value.clone());
                }
                if buffer.len() == (bottom - top) as usize {
                    self.motion = Some(Motion {
                        layer: Layer {
                            region: scroll.rows,
                            first_row: top,
                            rows: buffer,
                            offset_rows: offset,
                        },
                        from: offset,
                        started: now,
                    });
                }
            }
        } else if self.motion.as_ref().is_some_and(|motion| {
            if rows.len() != self.previous.len() {
                return true;
            }
            let mut changed = 0;
            for at in motion.layer.region.clone() {
                let base = &motion.layer.rows[(at as isize - motion.layer.first_row) as usize];
                if base != &rows[at] {
                    changed += 1;
                    if changed > 2 || !overlay(base, &rows[at]) {
                        return true;
                    }
                }
            }
            false
        }) {
            // An unrelated redraw is not evidence of scrolling.
            self.motion = None;
        }
        self.previous = rows.to_vec();
        observed
    }

    pub fn layer(&mut self, now: Duration) -> Option<Layer<R>> {
        self.advance(now);
        self.motion.as_ref().map(|motion| motion.layer.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    fn frame(first: usize) -> Vec<String> {
        ["header".to_owned()]
            .into_iter()
            .chain((first..first + 12).map(|n| format!("chat row {n}")))
            .chain(["separator".into(), "composer".into(), "status".into()])
            .collect()
    }
    fn key(row: &String) -> Option<u64> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        row.hash(&mut h);
        (!row.trim().is_empty()).then(|| h.finish())
    }
    fn started() -> ApplicationScroll<String> {
        let mut s = ApplicationScroll::default();
        s.update(&frame(100), ms(0), key);
        s
    }

    #[test]
    fn fixed_header_and_composer_do_not_join_the_scroll_region() {
        let found = detect(&frame(100), &frame(97), -1, key).unwrap();
        assert_eq!(
            found,
            ScrollRegion {
                rows: 1..13,
                delta: -3
            }
        );
        assert!(detect(&frame(100), &frame(97), 1, key).is_none());
    }

    #[test]
    fn ordinary_output_without_wheel_input_is_never_animated() {
        let mut s = started();
        s.update(&frame(103), ms(1), key);
        assert!(s.layer(ms(1)).is_none());
    }

    #[test]
    fn stale_wheel_input_does_not_animate_later_output() {
        let mut s = started();
        s.wheel(-3, ms(0));
        s.update(&frame(103), ms(501), key);
        assert!(s.layer(ms(501)).is_none());
    }

    #[test]
    fn upward_scroll_keeps_outgoing_bottom_rows_until_settled() {
        let mut s = started();
        s.wheel(3, ms(1));
        s.update(&frame(97), ms(2), key);
        let start = s.layer(ms(2)).unwrap();
        assert_eq!(start.offset_rows, -3.0);
        assert_eq!(start.first_row, 1);
        assert_eq!(start.rows.last().unwrap(), "chat row 111");
        assert_eq!(start.rows[3], "chat row 100");
        assert!(s.layer(ms(62)).unwrap().offset_rows > -3.0);
        assert!(s.layer(ms(122)).is_none());
        assert!(s.layer(ms(1000)).is_none());
    }

    #[test]
    fn downward_scroll_keeps_outgoing_top_rows() {
        let mut s = started();
        s.wheel(-3, ms(1));
        s.update(&frame(103), ms(2), key);
        let start = s.layer(ms(2)).unwrap();
        assert_eq!(start.offset_rows, 3.0);
        assert_eq!(start.first_row, -2);
        assert_eq!(start.rows[0], "chat row 100");
        assert_eq!(start.rows[3], "chat row 103");
    }

    #[test]
    fn repeated_scroll_preserves_the_current_visual_position() {
        let mut s = started();
        s.wheel(3, ms(0));
        s.update(&frame(97), ms(1), key);
        let old = s.layer(ms(31)).unwrap();
        s.wheel(3, ms(31));
        s.update(&frame(94), ms(31), key);
        let new = s.layer(ms(31)).unwrap();
        assert!((new.offset_rows - (old.offset_rows - 3.0)).abs() < 1e-9);
        for (i, row) in old.rows.iter().enumerate() {
            let y = old.first_row as f64 + i as f64 + old.offset_rows;
            if y + 1.0 > old.region.start as f64 && y < old.region.end as f64 {
                let at = new.rows.iter().position(|r| r == row).unwrap();
                let new_y = new.first_row as f64 + at as f64 + new.offset_rows;
                assert!((new_y - y).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn reversing_scroll_retains_rows_needed_at_both_edges() {
        let mut s = started();
        s.wheel(3, ms(0));
        s.update(&frame(97), ms(1), key);
        let old = s.layer(ms(31)).unwrap();
        s.wheel(-3, ms(31));
        s.update(&frame(100), ms(31), key);
        let new = s.layer(ms(31)).unwrap();
        assert!((new.offset_rows - (old.offset_rows + 3.0)).abs() < 1e-9);
        assert_eq!(new.region, 1..13);
    }

    #[test]
    fn immediately_reversing_to_the_presented_frame_needs_no_animation() {
        let mut s = started();
        s.wheel(3, ms(1));
        s.update(&frame(97), ms(1), key);
        s.wheel(-3, ms(1));
        s.update(&frame(100), ms(1), key);
        assert!(s.layer(ms(1)).is_none());
    }

    #[test]
    fn footer_updates_do_not_move_or_interrupt_the_chat() {
        let mut s = started();
        s.wheel(3, ms(0));
        s.update(&frame(97), ms(1), key);
        let mut update = frame(97);
        *update.last_mut().unwrap() = "busy".into();
        s.update(&update, ms(10), key);
        assert_eq!(s.layer(ms(10)).unwrap().region, 1..13);
    }

    #[test]
    fn a_small_overlay_keeps_motion_without_overwriting_the_text_under_it() {
        let mut s = started();
        s.wheel(3, ms(0));
        s.update(&frame(97), ms(1), key);
        let mut with_overlay = frame(97);
        with_overlay[11] = "jump-to-bottom overlay".into();
        s.update_with_overlays(&with_overlay, ms(20), key, |_, row| {
            row == "jump-to-bottom overlay"
        });
        let layer = s.layer(ms(20)).unwrap();
        assert_eq!(
            layer.rows[(11isize - layer.first_row) as usize],
            "chat row 107"
        );
        // Removing the overlay reveals the retained underlying text.
        s.update_with_overlays(&frame(97), ms(30), key, |_, _| false);
        assert!(s.layer(ms(30)).is_some());
    }

    #[test]
    fn editing_the_chat_or_resizing_cancels_old_motion() {
        let mut s = started();
        s.wheel(3, ms(0));
        s.update(&frame(97), ms(1), key);
        let mut update = frame(97);
        update[5] = "different content".into();
        s.update(&update, ms(10), key);
        assert!(s.layer(ms(10)).is_none());
        s.update(&[], ms(11), key);
        assert!(s.layer(ms(11)).is_none());
    }

    #[test]
    fn finish_cancels_presentation_and_pending_wheel_intent() {
        let mut s = started();
        s.wheel(3, ms(0));
        s.update(&frame(97), ms(1), key);
        assert!(s.finish());
        assert!(s.layer(ms(2)).is_none());
        s.update(&frame(94), ms(3), key);
        assert!(s.layer(ms(3)).is_none());
    }

    #[test]
    fn static_periodic_content_is_not_motion_when_only_chrome_changes() {
        let mut old = vec!["header".to_owned()];
        old.extend(["a", "b", "c"].repeat(5).into_iter().map(String::from));
        old.push("footer".into());
        let mut new = old.clone();
        new[0] = "updated header".into();
        *new.last_mut().unwrap() = "updated footer".into();
        assert!(detect(&old, &new, -1, key).is_none());
    }

    #[test]
    fn repeated_or_blank_rows_are_not_sufficient_evidence() {
        let mut old = vec!["same".to_owned(); 20];
        let mut new = old.clone();
        new[0] = "new title".into();
        assert!(detect(&old, &new, -1, key).is_none());
        old.fill(String::new());
        new = old.clone();
        new[0] = "only one anchor".into();
        assert!(detect(&old, &new, -1, key).is_none());
    }
}
