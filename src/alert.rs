//! System notifications mods post (`systemNotify`). Each carries the posting
//! mod's name in its tag, so a click goes back to that mod, also a click on one
//! from an earlier run (it can start Herdr): that one is held until a mod with
//! that name loads.

use std::cell::RefCell;

use gpui::SystemNotification;

const PREFIX: &str = "herdr-mod/";

/// The tag the system sees: a mod's own tag, scoped to the mod, so two mods
/// never replace each other's notifications. Mod names are folder names, so
/// they hold no `/`.
pub fn system_tag(mod_name: &str, tag: &str) -> String {
    format!("{PREFIX}{mod_name}/{tag}")
}

fn parse(system_tag: &str) -> Option<(&str, &str)> {
    let (name, tag) = system_tag.strip_prefix(PREFIX)?.split_once('/')?;
    (!name.is_empty()).then_some((name, tag))
}

pub fn notification(mod_name: &str, tag: &str, title: String, body: String) -> SystemNotification {
    SystemNotification { tag: system_tag(mod_name, tag).into(), title: title.into(), body: body.into(), actions: Vec::new() }
}

/// What Settings > Alerts sends on request: the person is there when macOS
/// asks for permission, so no mod's notification is the one lost to the
/// question.
pub fn test_notification() -> SystemNotification {
    SystemNotification {
        tag: "herdr-test".into(),
        title: "Herdr can reach you here".into(),
        body: "Mods that post system notifications show like this.".into(),
        actions: Vec::new(),
    }
}

thread_local! {
    /// Where each loaded copy's clicks go: (mod, sender).
    static LISTENERS: RefCell<Vec<(String, async_channel::Sender<String>)>> = const { RefCell::new(Vec::new()) };
    /// Clicks for a mod not loaded yet: (mod, tag).
    static HELD: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
}

/// Clicks on the notifications `mod_name` posted, for one loaded copy. Clicks
/// held for that mod go to it now.
pub fn listen(mod_name: &str) -> async_channel::Receiver<String> {
    let (tx, rx) = async_channel::unbounded();
    HELD.with_borrow_mut(|held| {
        held.retain(|(name, tag)| name != mod_name || tx.try_send(tag.clone()).is_err());
    });
    LISTENERS.with_borrow_mut(|listeners| {
        listeners.retain(|(_, tx)| !tx.is_closed());
        listeners.push((mod_name.to_owned(), tx));
    });
    rx
}

/// Delivers a click to every loaded copy of the mod that posted it, or holds
/// it until one loads. Returns whether a mod posted it.
pub fn clicked(system_tag: &str) -> bool {
    let Some((name, tag)) = parse(system_tag) else { return false };
    let delivered = LISTENERS.with_borrow_mut(|listeners| {
        listeners.retain(|(_, tx)| !tx.is_closed());
        let mut any = false;
        for (n, tx) in listeners.iter() {
            if n == name {
                any |= tx.try_send(tag.to_owned()).is_ok();
            }
        }
        any
    });
    if !delivered {
        HELD.with_borrow_mut(|held| held.push((name.to_owned(), tag.to_owned())));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_names_the_posting_mod() {
        assert_eq!(system_tag("toast", "agent:t1"), "herdr-mod/toast/agent:t1");
        assert_eq!(parse("herdr-mod/toast/agent:t1"), Some(("toast", "agent:t1")));
        assert_eq!(parse("herdr-mod/toast/a/b"), Some(("toast", "a/b")));
        assert_eq!(parse("herdr-mod/toast/"), Some(("toast", "")));
        assert_eq!(parse("herdr-mod//x"), None);
        assert_eq!(parse("herdr-test"), None);
    }

    #[test]
    fn a_click_goes_to_the_mod_that_posted_it() {
        std::thread::spawn(|| {
            let toast = listen("toast");
            let other = listen("tower");
            assert!(clicked(&system_tag("toast", "t1")));
            assert_eq!(toast.try_recv().unwrap(), "t1");
            assert!(other.is_empty());
            assert!(!clicked("herdr-test"), "not a mod's");
            assert!(toast.is_empty() && other.is_empty());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn a_click_from_an_earlier_run_waits_for_its_mod() {
        std::thread::spawn(|| {
            // Clicked in Notification Center, which started Herdr.
            assert!(clicked(&system_tag("toast", "t2")));
            let other = listen("tower");
            assert!(other.is_empty());
            let toast = listen("toast");
            assert_eq!(toast.try_recv().unwrap(), "t2");
            // Delivered once: a later copy doesn't get it again.
            assert!(listen("toast").is_empty());
            // A copy that unloaded doesn't take a click from the next one.
            drop(toast);
            let next = listen("toast");
            assert!(clicked(&system_tag("toast", "t3")));
            assert_eq!(next.try_recv().unwrap(), "t3");
        })
        .join()
        .unwrap();
    }
}
