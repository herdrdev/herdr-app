use crate::layout::{TypingPulse, TypingPulseStep};
use crate::settings;
use gpui::{App, BorderStyle, Corners, Div, Edges, EntityId, Hsla, ParentElement, Pixels, Styled, Task, Window, canvas, quad, rgb};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) const DEFAULT_DURATION: Duration = Duration::from_millis(880);
const COLORS: [&str; 4] = ["accent", "ansi4", "ansi5", "ansi6"];

pub(super) struct PaneTypingPulse {
    pub(super) pane: String,
    pub(super) terminal: EntityId,
    pub(super) started: Instant,
    pub(super) timing: TypingPulse,
    burst: Burst,
    wake: Option<Wake>,
}

struct Wake {
    transition: Duration,
    deadline: Duration,
    _task: Task<()>,
}

impl PaneTypingPulse {
    pub(super) fn start(
        pane: &str,
        terminal: EntityId,
        at: Instant,
        now: Instant,
        previous: Option<&Self>,
        timing: &TypingPulse,
    ) -> Option<Self> {
        if previous.is_some_and(|pulse| {
            pulse.terminal == terminal && &pulse.timing == timing && pulse.is_active(at)
        }) {
            return None;
        }

        let seed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
        let burst = Burst::new(seed, previous.map(|pulse| pulse.burst.color), timing);
        if now.duration_since(at) >= burst.duration {
            return None;
        }

        Some(Self {
            pane: pane.to_owned(),
            terminal,
            started: at,
            timing: timing.clone(),
            burst,
            wake: None,
        })
    }

    fn is_active(&self, now: Instant) -> bool {
        now.duration_since(self.started) < self.burst.duration
    }

    pub(super) fn schedule_frame(&mut self, window: &Window, cx: &mut App) {
        if cx.reduce_motion() {
            self.wake = None;
            return;
        }

        let elapsed = cx.background_executor().now().duration_since(self.started);
        let (_, redraw) = self.burst.sample(elapsed);
        match redraw {
            Redraw::Idle => self.wake = None,
            Redraw::NextFrame => {
                self.wake = None;
                window.request_animation_frame();
            }
            Redraw::At(transition) => {
                if self.wake.as_ref().is_some_and(|wake| {
                    wake.transition == transition && elapsed < wake.deadline
                }) {
                    return;
                }

                // Very distant transitions use bounded timer waits; the
                // designer's actual timing still determines when the pulse changes.
                let delay = (transition - elapsed).min(Duration::from_secs(24 * 60 * 60));
                let view = window.current_view();
                let task = window.spawn(cx, async move |cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = cx.update(|_, cx| cx.notify(view));
                });
                self.wake = Some(Wake { transition, deadline: elapsed + delay, _task: task });
            }
        }
    }

    pub(super) fn paint(&self, frame: Div, color: Hsla, radius: Pixels, now: Instant) -> Div {
        let elapsed = now.duration_since(self.started);
        let (strength, _) = self.burst.sample(elapsed);
        if strength == 0.0 {
            return frame;
        }

        let tint = settings::color_or_literal(COLORS[self.burst.color]);
        let light = tint.blend(rgb(0xf6f8ff).alpha(0.82).into());
        let layers = [(14.0, 0.08), (9.0, 0.16), (5.0, 0.28), (2.0, 0.47)]
            .map(|(width, opacity)| (settings::z(width), opacity));

        // Paint the inset glow over the frame without taking space or input.
        frame.border_color(color.blend(light.alpha(strength))).child(
            canvas(|_, _, _| (), move |bounds, _, window, _| {
                for (width, opacity) in layers {
                    let width = width.min(bounds.size.width.min(bounds.size.height) / 2.);
                    window.paint_quad(quad(
                        bounds,
                        Corners::all(radius),
                        Hsla::transparent_black(),
                        Edges::all(width),
                        tint.alpha(opacity * strength),
                        BorderStyle::Solid,
                    ));
                }
            }).absolute().inset_0(),
        )
    }
}

struct Burst {
    pulses: Vec<TypingPulseStep>,
    duration: Duration,
    color: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Redraw {
    Idle,
    NextFrame,
    At(Duration),
}

impl Burst {
    fn new(seed: u32, last_color: Option<usize>, timing: &TypingPulse) -> Self {
        let color = match last_color {
            Some(last) => (last + 1 + seed.rotate_left(22) as usize % (COLORS.len() - 1)) % COLORS.len(),
            None => seed as usize % COLORS.len(),
        };
        let (pulses, duration) = match timing {
            TypingPulse::Enabled(true) => {
                let first = u64::from(150 + seed % 126);
                let second = u64::from(150 + seed.rotate_left(11) % 126);
                (vec![
                    TypingPulseStep { at_ms: 0, hold_ms: 85, fade_ms: 65, strength: 1.0 },
                    TypingPulseStep { at_ms: first, hold_ms: 85, fade_ms: 65, strength: 0.72 },
                    TypingPulseStep { at_ms: first + second, hold_ms: 85, fade_ms: 245, strength: 0.44 },
                ], DEFAULT_DURATION)
            }
            TypingPulse::Enabled(false) => (Vec::new(), Duration::ZERO),
            TypingPulse::Custom { pulses } => {
                let mut pulses = pulses.clone();
                pulses.sort_by_key(|pulse| pulse.at_ms);
                let duration = pulses.last().map_or(Duration::ZERO, |pulse| {
                    Duration::from_millis(pulse.at_ms) + Duration::from_millis(pulse.hold_ms)
                        + Duration::from_millis(pulse.fade_ms)
                });
                (pulses, duration)
            }
        };
        Self { pulses, duration, color }
    }

    fn sample(&self, elapsed: Duration) -> (f32, Redraw) {
        if elapsed >= self.duration {
            return (0.0, Redraw::Idle);
        }

        let next = self.pulses.partition_point(|pulse| Duration::from_millis(pulse.at_ms) <= elapsed);
        let next_start = self.pulses.get(next).map(|pulse| Duration::from_millis(pulse.at_ms));
        let rest = || (0.0, next_start.map_or(Redraw::Idle, Redraw::At));
        let Some(pulse) = next.checked_sub(1).and_then(|index| self.pulses.get(index)) else {
            return rest();
        };
        let strength = pulse.strength.clamp(0.0, 1.0);
        if strength == 0.0 {
            return rest();
        }

        let hold_until = Duration::from_millis(pulse.at_ms) + Duration::from_millis(pulse.hold_ms);
        let fade = Duration::from_millis(pulse.fade_ms);
        if elapsed < hold_until {
            let transition = next_start.map_or(hold_until, |next| next.min(hold_until));
            (strength, Redraw::At(transition))
        } else if elapsed < hold_until + fade {
            let remaining = 1.0 - (elapsed - hold_until).div_duration_f32(fade);
            (strength * remaining * remaining, Redraw::NextFrame)
        } else {
            rest()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_have_separate_peaks_and_finish_at_the_resting_border() {
        for seed in [0, 2345, u32::MAX] {
            let burst = Burst::new(seed, None, &TypingPulse::Enabled(true));
            let first = burst.pulses[1].at_ms;
            let second = burst.pulses[2].at_ms;
            let strength = |ms| burst.sample(Duration::from_millis(ms)).0;
            assert!((150..=275).contains(&first));
            assert!((150..=275).contains(&(second - first)));
            assert_eq!(strength(0), 1.0);
            assert_eq!(strength(80), 1.0);
            assert!(strength(first - 1) < 0.001);
            assert_eq!(strength(first), 0.72);
            assert!(strength(second - 1) < 0.001);
            assert_eq!(strength(second), 0.44);
            assert_eq!(burst.sample(DEFAULT_DURATION), (0.0, Redraw::Idle));
        }
    }

    #[test]
    fn consecutive_bursts_use_different_colors() {
        for last in 0..COLORS.len() {
            for seed in [0, 2345, u32::MAX] {
                assert_ne!(Burst::new(seed, Some(last), &TypingPulse::Enabled(true)).color, last);
            }
        }
    }

    #[test]
    fn custom_timing_controls_holds_fades_and_delayed_pulses() {
        let timing = TypingPulse::Custom { pulses: vec![
            TypingPulseStep { at_ms: 3500, hold_ms: 40, fade_ms: 0, strength: 0.3 },
            TypingPulseStep { at_ms: 100, hold_ms: 600, fade_ms: 400, strength: 0.8 },
        ] };
        let burst = Burst::new(0, None, &timing);
        let ms = Duration::from_millis;
        assert_eq!(burst.sample(ms(0)), (0.0, Redraw::At(ms(100))));
        assert_eq!(burst.sample(ms(100)), (0.8, Redraw::At(ms(700))));
        assert_eq!(burst.sample(ms(900)), (0.2, Redraw::NextFrame));
        assert_eq!(burst.sample(ms(1100)), (0.0, Redraw::At(ms(3500))));
        assert_eq!(burst.sample(ms(3500)), (0.3, Redraw::At(ms(3540))));
        assert_eq!(burst.sample(ms(3540)), (0.0, Redraw::Idle));
    }
}
