use crate::settings;
use gpui::{FontWeight, TextRun, Window, font};

// The shipped workspace-chrome, spaces and agents mods switch presentation here.
const COMPACT_BELOW: f32 = 160.;

const PULL: f32 = 40.;

pub(super) struct SidebarDrag {
    wide: f32,
    // The farthest the pointer traveled into this mode during this drag.
    anchor: Option<f32>,
}

impl SidebarDrag {
    pub(super) fn for_header(window: &Window) -> Self {
        // Match workspace-chrome's wide brand row: left/right padding, logo,
        // wordmark, Search and toggle, with one inner and two outer gaps.
        let mac = cfg!(target_os = "macos");
        let left = if mac && !window.is_fullscreen() { 76. } else { 14. };
        let mut font = font(settings::get().ui_font_family.clone());
        font.weight = FontWeight::SEMIBOLD;
        let run = TextRun { len: "Herdr".len(), font, ..Default::default() };
        let word = window.text_system().shape_line("Herdr".into(), settings::z(13.5), &[run], None);
        let width = left + 10. + 20. + 8. + f32::from(word.width) / settings::zoom() + 2. * (4. + 26.);
        Self { wide: width.ceil().max(COMPACT_BELOW), anchor: None }
    }

    pub(super) fn snap(&mut self, requested: f32, previous: f32, min: f32, available: f32) -> (f32, bool) {
        // Use the layout's ordinary drag minimum (44 in main), not the wider
        // macOS toggle preset. `requested` already respects the divider limits.
        let compact = previous < COMPACT_BELOW;
        let anchor = self.anchor.get_or_insert(previous.min(self.wide));
        let width = if min >= COMPACT_BELOW || min > available {
            requested
        } else if available < self.wide {
            min
        } else if compact {
            *anchor = anchor.min(requested);
            if requested - *anchor >= PULL {
                requested.max(self.wide)
            } else if requested >= previous {
                // A button-folded macOS rail can start at 76. Don't shrink it
                // under a rightward drag before there's enough pull to open.
                previous.clamp(min, available)
            } else {
                min
            }
        } else {
            *anchor = anchor.max(requested).min(self.wide);
            if *anchor - requested >= PULL { min } else { requested.max(self.wide) }
        };
        let switched = compact != (width.round() < COMPACT_BELOW);
        // The pointer did not jump with the divider. Count the next pull from
        // here, not from the new divider position, until the button is released.
        if switched { self.anchor = Some(requested); }
        (width, switched)
    }
}

#[cfg(test)]
mod tests {
    use super::SidebarDrag;

    #[test]
    fn threshold_is_directional_and_never_overrides_the_dividers_limits() {
        for wide in [210., 160.] {
            let mut s = SidebarDrag { wide, anchor: None };
            let fold = wide - 40.;
            assert_eq!(s.snap(fold + 1., wide, 44., 480.), (wide, false));
            assert_eq!(s.snap(fold, wide, 44., 480.), (44., true));
            assert_eq!(s.snap(fold, 44., 44., 480.), (44., false));
            assert_eq!(s.snap(fold + 1., 44., 44., 480.), (44., false));
            assert_eq!(s.snap(44., 44., 44., 480.), (44., false));
            assert_eq!(s.snap(83., 44., 44., 480.), (44., false));
            assert_eq!(s.snap(84., 44., 44., 480.), (wide, true));
            assert_eq!(s.snap(84., wide, 44., 480.), (wide, false));
            assert_eq!(s.snap(44., wide, 44., 480.), (44., true));
        }
        let mut s = SidebarDrag { wide: 210., anchor: None };
        assert_eq!(s.snap(172., 171., 44., 480.), (210., false), "restored width below header fit");
        let mut s = SidebarDrag { wide: 210., anchor: None };
        assert_eq!(s.snap(77., 76., 44., 480.), (76., false), "button-folded macOS rail");
        assert_eq!(s.snap(116., 76., 44., 480.), (210., true));
        assert_eq!(s.snap(150., 210., 44., 150.), (44., true));
        assert_eq!(s.snap(30., 44., 44., 30.), (30., false));
        assert_eq!(s.snap(180., 210., 180., 480.), (180., false));
    }
}
