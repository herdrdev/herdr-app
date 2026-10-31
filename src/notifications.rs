//! Optional native agent alerts. The platform callback queues data; only the
//! window's normal event loop may navigate or touch GPUI.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub terminal: String,
    pub created_ms: u64,
    pub home: String,
}

pub enum Event {
    Clicked { target: Target, test: bool },
    Error(String),
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{Event, Target};
    use std::ffi::{CStr, CString, c_char};
    use std::sync::{Mutex, OnceLock};
    use std::sync::mpsc::{self, Receiver, SyncSender};

    unsafe extern "C" {
        fn herdr_notifications_configure(enabled: bool, callback: extern "C" fn(u32, *const c_char));
        fn herdr_notifications_post(identifier: *const c_char, target: *const c_char, title: *const c_char, body: *const c_char, test: bool);
    }

    fn events() -> &'static (SyncSender<Event>, Mutex<Receiver<Event>>) {
        static EVENTS: OnceLock<(SyncSender<Event>, Mutex<Receiver<Event>>)> = OnceLock::new();
        EVENTS.get_or_init(|| { let (tx, rx) = mpsc::sync_channel(32); (tx, Mutex::new(rx)) })
    }

    extern "C" fn callback(kind: u32, text: *const c_char) {
        if text.is_null() { return; }
        // The native bridge owns this string until the callback returns.
        let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
        let event = match kind {
            0 | 2 => match serde_json::from_str::<Target>(&text) {
                Ok(target) => Event::Clicked { target, test: kind == 2 },
                Err(_) => return,
            },
            _ => Event::Error(text.into_owned()),
        };
        let _ = events().0.try_send(event);
    }

    pub fn configure(enabled: bool) {
        // Called on GPUI's main thread. The bridge retains the delegate and
        // performs authorization and delivery asynchronously.
        unsafe { herdr_notifications_configure(enabled, callback) };
    }

    pub fn post(target: &Target, title: &str, body: &str, test: bool) {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        target.home.hash(&mut hash);
        let identifier = format!("herdr.{:x}.{}", hash.finish(), if test { "test" } else { &target.terminal });
        // Strip terminal controls and bound text before giving it to the OS.
        let clean = |s: &str, limit| CString::new(s.chars().filter(|c| !c.is_control()).take(limit).collect::<String>()).unwrap();
        let (identifier, payload, title, body) = (
            CString::new(identifier).unwrap(),
            CString::new(serde_json::to_string(target).unwrap()).unwrap(),
            clean(title, 120), clean(body, 300),
        );
        unsafe { herdr_notifications_post(identifier.as_ptr(), payload.as_ptr(), title.as_ptr(), body.as_ptr(), test) };
    }

    pub fn take_events() -> Vec<Event> {
        events().1.lock().unwrap().try_iter().collect()
    }
}

#[cfg(target_os = "macos")]
pub use macos::{configure, post, take_events};

#[cfg(not(target_os = "macos"))]
pub fn configure(_: bool) {}
#[cfg(not(target_os = "macos"))]
pub fn post(_: &Target, _: &str, _: &str, _: bool) {}
#[cfg(not(target_os = "macos"))]
pub fn take_events() -> Vec<Event> { vec![] }

pub fn target_matches(target: &Target, home: &str, created_ms: Option<u64>) -> bool {
    target.home == home && created_ms == Some(target.created_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_cannot_focus_a_reused_terminal_or_another_app_home() {
        let target = Target { terminal: "vps/t1".into(), created_ms: 42, home: "/tmp/herdr-a".into() };
        assert!(target_matches(&target, "/tmp/herdr-a", Some(42)));
        assert!(!target_matches(&target, "/tmp/herdr-a", None));
        assert!(!target_matches(&target, "/tmp/herdr-a", Some(43)));
        assert!(!target_matches(&target, "/tmp/herdr-b", Some(42)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bare_binary_reports_an_error_instead_of_calling_notification_center() {
        // The native service is process-global. Exercise it in a child so
        // parallel GPUI tests cannot share its delegate or authorization state.
        const CHILD: &str = "HERDR_NOTIFICATION_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "notifications::tests::a_bare_binary_reports_an_error_instead_of_calling_notification_center", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }
        configure(false);
        let target = Target { terminal: "t1".into(), created_ms: 42, home: "/tmp/herdr-notifications-test".into() };
        post(&target, "A test", "Must not ask for permission from a test runner", true);
        let events = take_events();
        assert!(events.iter().any(|e| matches!(e, Event::Error(message) if message.contains("Herdr.app"))));
    }
}
