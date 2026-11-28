//! Double modifier taps (`"shift shift"` in keys.json): one modifier pressed
//! and released twice within 300 ms, with nothing else between.
//!
//! GPUI reports a lone modifier (pressed and released with no other key) as a
//! keystroke named after it, and its keymap would take `shift shift` as a
//! chord of two of those, with no time limit and counting Shift held through
//! a click. So double taps stay out of GPUI's keymap, in one of their own,
//! and the window counts GPUI's lone-modifier keystrokes itself.

use gpui::{Action, App, KeyBinding, KeyContext, Keymap, Keystroke, Modifiers};
use std::time::{Duration, Instant};

/// How soon the second tap must end after the first.
const WITHIN: Duration = Duration::from_millis(300);

/// GPUI's names for a lone modifier as a key (`ctrl` parses to `control`,
/// `cmd`, `super` and `win` to `platform`).
const MODIFIERS: [&str; 5] = ["shift", "control", "alt", "platform", "function"];

/// `Some(Ok(modifier))` for a double tap, `Some(Err(why))` for other keys
/// made only of modifiers (`shift ctrl`, `shift shift shift`), `None` for
/// every other key. One modifier alone (`shift`) stays GPUI's.
pub fn double_tap(binding: &KeyBinding) -> Option<Result<&'static str, String>> {
    let strokes: Vec<&Keystroke> = binding.keystrokes().iter().map(|k| k.inner()).collect();
    if strokes.len() < 2 || !strokes.iter().all(|k| MODIFIERS.contains(&k.key.as_str())) {
        return None;
    }
    let modifier = MODIFIERS.into_iter().find(|m| *m == strokes[0].key)?;
    if strokes.len() == 2 && strokes.iter().all(|k| k.key == modifier && k.modifiers == Modifiers::none()) {
        Some(Ok(modifier))
    } else {
        Some(Err("a double tap is one modifier twice, like \"shift shift\" or \"ctrl ctrl\"".into()))
    }
}

/// keys.json's double taps, by section.
struct Bound(Keymap);
impl gpui::Global for Bound {}

pub fn install(bindings: Vec<KeyBinding>, cx: &mut App) {
    cx.set_global(Bound(Keymap::new(bindings)));
}

/// What tapping `modifier` twice runs where the keyboard is: chosen as GPUI
/// chooses for other keys (the more specific section wins, `terminal` turns
/// it off).
pub fn action(modifier: &str, context: &[KeyContext], cx: &App) -> Option<Box<dyn Action>> {
    let tap = Keystroke { key: modifier.into(), ..Default::default() };
    let (found, _) = cx.try_global::<Bound>()?.0.bindings_for_input(&[tap.clone(), tap], context);
    found.first().map(|b| b.action().boxed_clone())
}

/// Spots double taps in the keystrokes and clicks a window gets.
#[derive(Default)]
pub struct Taps {
    /// The last tap: its modifier and when it ended.
    last: Option<(&'static str, Instant)>,
    /// A press came while a modifier was held: that hold is no tap.
    clicked: bool,
}

impl Taps {
    /// A keystroke reached the window: the modifier, when this ends its second
    /// tap. Any other keystroke starts over.
    pub fn key(&mut self, keystroke: &Keystroke, at: Instant) -> Option<&'static str> {
        let (last, clicked) = (self.last.take(), std::mem::take(&mut self.clicked));
        let tap = MODIFIERS.into_iter().find(|m| *m == keystroke.key && keystroke.modifiers == Modifiers::none()).filter(|_| !clicked)?;
        match last {
            Some((last, ended)) if last == tap && at.saturating_duration_since(ended) <= WITHIN => Some(tap),
            _ => {
                self.last = Some((tap, at));
                None
            }
        }
    }

    /// A mouse button went down, with a modifier held or not: no tap counts
    /// across it, and the one held now is none.
    pub fn click(&mut self, modifier_held: bool) {
        self.last = None;
        self.clicked = modifier_held;
    }
}

#[cfg(test)]
mod tests {
    use super::{Taps, WITHIN};
    use gpui::Keystroke;
    use std::time::{Duration, Instant};

    /// Feeds `keys` (a keystroke, or a click when `click`/`shift-click`) at
    /// `ms` after a fixed start: what each one fired.
    fn run(steps: &[(u64, &str)]) -> Vec<Option<&'static str>> {
        let start = Instant::now();
        let mut taps = Taps::default();
        steps.iter().map(|(ms, step)| match *step {
            "click" => { taps.click(false); None }
            "shift-click" => { taps.click(true); None }
            key => taps.key(&Keystroke::parse(key).unwrap(), start + Duration::from_millis(*ms)),
        }).collect()
    }

    #[test]
    fn two_taps_within_300_ms_fire_once_and_a_third_starts_over() {
        assert_eq!(run(&[(0, "shift"), (150, "shift"), (300, "shift"), (420, "shift")]),
            [None, Some("shift"), None, Some("shift")]);
        let edge = WITHIN.as_millis() as u64;
        assert_eq!(run(&[(0, "ctrl"), (edge, "ctrl")]), [None, Some("control")]);
        // A single tap expires: a late second one is a new first one.
        assert_eq!(run(&[(0, "shift"), (edge + 1, "shift"), (edge + 100, "shift")]), [None, None, Some("shift")]);
        for (name, key) in [("alt", "alt"), ("platform", "cmd"), ("function", "fn")] {
            assert_eq!(run(&[(0, key), (50, key)]), [None, Some(name)]);
        }
    }

    #[test]
    fn any_other_keystroke_starts_over() {
        // Shift held to type a capital makes no lone `shift` keystroke; the capital resets.
        for other in ["shift-a", "a", "f6", "ctrl-shift", "ctrl"] {
            assert_eq!(run(&[(0, "shift"), (50, other), (100, "shift")]), [None, None, None], "{other}");
        }
        assert_eq!(run(&[(0, "shift"), (50, "ctrl"), (100, "shift"), (150, "shift")]), [None, None, None, Some("shift")]);
    }

    #[test]
    fn a_click_during_or_between_taps_cancels() {
        // Shift held through a click (a selection): that tap never counts.
        assert_eq!(run(&[(0, "shift-click"), (40, "shift"), (80, "shift")]), [None, None, None]);
        assert_eq!(run(&[(0, "shift"), (40, "shift-click"), (80, "shift"), (120, "shift"), (160, "shift")]),
            [None, None, None, None, Some("shift")]);
        // A plain click between taps starts over too.
        assert_eq!(run(&[(0, "shift"), (40, "click"), (80, "shift"), (120, "shift")]), [None, None, None, Some("shift")]);
        // A press with a modifier held drops only that hold's tap: a plain
        // click or any keystroke after it clears it.
        assert_eq!(run(&[(0, "shift-click"), (40, "click"), (80, "shift"), (120, "shift")]), [None, None, None, Some("shift")]);
        assert_eq!(run(&[(0, "shift-click"), (40, "a"), (80, "shift"), (120, "shift")]), [None, None, None, Some("shift")]);
    }
}
