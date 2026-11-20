use std::time::Duration;

pub(super) const DURATION: Duration = Duration::from_millis(880);
pub(super) const COLORS: [&str; 4] = ["accent", "ansi4", "ansi5", "ansi6"];

#[derive(Clone, Copy)]
pub(super) struct Burst {
    echoes: [f32; 2],
    pub(super) color: usize,
}

impl Burst {
    pub(super) fn new(seed: u32, last_color: Option<usize>) -> Self {
        let first = 150 + seed % 126;
        let second = 150 + seed.rotate_left(11) % 126;
        let color = match last_color {
            Some(last) => (last + 1 + seed.rotate_left(22) as usize % (COLORS.len() - 1)) % COLORS.len(),
            None => seed as usize % COLORS.len(),
        };
        Self { echoes: [first as f32, (first + second) as f32], color }
    }

    pub(super) fn intensity(self, elapsed_ms: f32) -> f32 {
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
