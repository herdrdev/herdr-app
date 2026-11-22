use crate::settings;
use gpui::{BorderStyle, Corners, Div, Edges, EntityId, Hsla, ParentElement, Pixels, Styled, canvas, quad, rgb};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(super) const DURATION: Duration = Duration::from_millis(880);
const COLORS: [&str; 4] = ["accent", "ansi4", "ansi5", "ansi6"];

pub(super) struct PaneTypingPulse {
    pub(super) pane: String,
    pub(super) terminal: EntityId,
    pub(super) started: Instant,
    burst: Burst,
}

impl PaneTypingPulse {
    pub(super) fn start(
        pane: &str,
        terminal: EntityId,
        at: Instant,
        now: Instant,
        previous: Option<&Self>,
    ) -> Option<Self> {
        if now.duration_since(at) >= DURATION
            || previous.is_some_and(|pulse| pulse.terminal == terminal && pulse.is_active(at))
        {
            return None;
        }

        let seed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos();
        Some(Self {
            pane: pane.to_owned(),
            terminal,
            started: at,
            burst: Burst::new(seed, previous.map(|pulse| pulse.burst.color)),
        })
    }

    pub(super) fn is_active(&self, now: Instant) -> bool {
        now.duration_since(self.started) < DURATION
    }

    pub(super) fn paint(&self, frame: Div, color: Hsla, radius: Pixels, now: Instant) -> Div {
        let elapsed = now.duration_since(self.started);
        if elapsed >= DURATION {
            return frame;
        }
        let strength = self.burst.intensity(elapsed.as_secs_f32() * 1000.0);
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
    echoes: [f32; 2],
    color: usize,
}

impl Burst {
    fn new(seed: u32, last_color: Option<usize>) -> Self {
        let first = 150 + seed % 126;
        let second = 150 + seed.rotate_left(11) % 126;
        let color = match last_color {
            Some(last) => (last + 1 + seed.rotate_left(22) as usize % (COLORS.len() - 1)) % COLORS.len(),
            None => seed as usize % COLORS.len(),
        };
        Self { echoes: [first as f32, (first + second) as f32], color }
    }

    fn intensity(&self, elapsed_ms: f32) -> f32 {
        [(0.0, 1.0, 65.0), (self.echoes[0], 0.72, 65.0), (self.echoes[1], 0.44, 245.0)]
            .into_iter()
            .rev()
            .find(|(start, _, _)| elapsed_ms >= *start)
            .map_or(0.0, |(start, peak, fade)| {
                let remaining = (1.0 - (elapsed_ms - start - 85.0) / fade).clamp(0.0, 1.0);
                peak * remaining * remaining
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echoes_have_separate_peaks_and_finish_at_the_resting_border() {
        for seed in [0, 2345, u32::MAX] {
            let burst = Burst::new(seed, None);
            assert!((150.0..=275.0).contains(&burst.echoes[0]));
            assert!((150.0..=275.0).contains(&(burst.echoes[1] - burst.echoes[0])));
            assert_eq!(burst.intensity(0.0), 1.0);
            assert_eq!(burst.intensity(80.0), 1.0);
            assert!(burst.intensity(burst.echoes[0] - 1.0) < 0.001);
            assert_eq!(burst.intensity(burst.echoes[0]), 0.72);
            assert!(burst.intensity(burst.echoes[1] - 1.0) < 0.001);
            assert_eq!(burst.intensity(burst.echoes[1]), 0.44);
            assert_eq!(burst.intensity(DURATION.as_millis() as f32), 0.0);
        }
    }

    #[test]
    fn consecutive_bursts_use_different_colors() {
        for last in 0..COLORS.len() {
            for seed in [0, 2345, u32::MAX] {
                assert_ne!(Burst::new(seed, Some(last)).color, last);
            }
        }
    }
}
