//! The main model owns all spaces, including remote machines; mod windows do
//! not own another copy. Count terminals, not panes or spaces.
use crate::model::Model;
use std::collections::HashSet;

pub fn count(model: &Model, mut connected: impl FnMut(Option<&str>) -> bool, mut live: impl FnMut(&str) -> bool) -> usize {
    let mut counted = HashSet::new();
    for space in &model.spaces {
        if !connected(space.machine.as_deref()) { continue; }
        for tab in &space.tabs {
            for pane in tab.root.terminals() {
                let Some(address) = pane.terminal() else { continue };
                if live(address) && matches!(model.status(address), Some("blocked" | "done")) {
                    counted.insert(address);
                }
            }
        }
    }
    counted.len()
}

#[cfg(all(target_os = "macos", not(test)))]
mod tile;

/// Called from the UI thread when the same model published to mods changes.
#[cfg(all(target_os = "macos", not(test)))]
pub fn show(count: usize) {
    use std::cell::Cell;
    thread_local! { static PUBLISHED: Cell<Option<usize>> = const { Cell::new(None) }; }
    let Some(main_thread) = objc2::MainThreadMarker::new() else { return };
    PUBLISHED.with(|previous| {
        if previous.get() != Some(count) {
            tile::show(count, main_thread);
            previous.set(Some(count));
        }
    });
}

// Headless tests must never change the test runner's Dock tile.
#[cfg(all(target_os = "macos", test))]
pub fn show(_: usize) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Tab;

    fn fixture() -> Model {
        let mut model = Model::default();
        model.add_space("s1", "", "", Tab::new("a", "", "p1", "t1"));
        model.add_space("s2", "", "", Tab::new("b", "", "p2", "remote/t1"));
        model.spaces[1].machine = Some("remote".into());
        model
    }

    #[test]
    fn counts_attention_across_machines_once_per_terminal() {
        let mut model = fixture();
        model.set_word("t1", Some("blocked".into()), None, true);
        model.set_word("remote/t1", Some("working".into()), None, false);
        model.set_word("remote/t1", Some("idle".into()), None, false);
        model.add_tab("s1", Tab::new("c", "", "p3", "t1"));
        assert_eq!(count(&model, |_| true, |_| true), 2);
        assert_eq!(count(&model, |machine| machine.is_none(), |_| true), 1);
        assert_eq!(count(&model, |_| false, |_| true), 0);
        assert_eq!(count(&model, |_| true, |address| address != "t1"), 1);
        model.mark_seen(&["remote/t1".into()]);
        assert_eq!(count(&model, |_| true, |_| true), 1);
        model.set_word("t1", Some("working".into()), None, true);
        assert_eq!(count(&model, |_| true, |_| true), 0);
    }

    #[test]
    fn shared_acknowledgement_and_errors_match_sidebar() {
        let mut model = fixture();
        model.set_word("remote/t1", Some("idle".into()), None, false);
        model.set_turn("remote/t1", 7, Some(6));
        assert_eq!(count(&model, |_| true, |_| true), 1);
        model.set_turn("remote/t1", 7, Some(7));
        assert_eq!(count(&model, |_| true, |_| true), 0);
        model.set_word("t1", Some("error".into()), None, true);
        assert_eq!(count(&model, |_| true, |_| true), 1);
        model.spaces.clear();
        assert_eq!(count(&model, |_| true, |_| true), 0);
    }
}
