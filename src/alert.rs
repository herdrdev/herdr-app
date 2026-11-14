//! Alerts outside the window: a system notification. The toast and the sounds
//! stay in window.rs.

use gpui::SystemNotification;
use serde_json::Value;

use crate::model::Alert;
use crate::settings::SystemNotifications;

/// One alert, as the system notification shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub address: String,
    pub alert: Alert,
    pub name: String,
    /// "space / tab", as the toast shows it.
    pub place: Option<String>,
    /// The agent's own summary from its 7501 report (`msg`), e.g. "approve
    /// Bash: make"; empty when it said nothing.
    pub message: String,
}

impl Note {
    pub fn title(&self) -> String {
        match self.alert {
            Alert::Blocked => format!("{} needs you", self.name),
            Alert::Error => format!("{} hit an error", self.name),
            Alert::Done => format!("{} finished", self.name),
        }
    }

    /// The agent's own message when it gave one, over "space / tab".
    fn body(&self) -> String {
        let lines = [self.message.trim(), self.place.as_deref().unwrap_or_default()];
        lines.into_iter().filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
    }
}

/// The `msg` of a terminal's 7501 record in the core's listing.
pub fn message_of(report: Option<&Value>) -> String {
    report.and_then(|r| r.get("msg")).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// Whether an alert also goes to the notification center. `app_active`:
/// Herdr's window is key in the app in front. The agent the person is looking
/// at never alerts (window.rs).
pub fn should_notify(mode: SystemNotifications, app_active: bool) -> bool {
    match mode {
        SystemNotifications::Off => false,
        SystemNotifications::Background => !app_active,
        SystemNotifications::Always => true,
    }
}

const TAG_PREFIX: &str = "herdr-agent:";

/// One notification per agent: a newer alert replaces the older one.
pub fn tag(address: &str) -> String {
    format!("{TAG_PREFIX}{address}")
}

pub fn address_of(tag: &str) -> Option<&str> {
    tag.strip_prefix(TAG_PREFIX).filter(|a| !a.is_empty())
}

/// What Settings > Alerts sends on request, whatever `system_notifications`
/// says: the person is there when macOS asks for permission, so no agent's
/// alert is the one lost to the question. Its own tag: a click goes nowhere.
pub fn test_notification() -> SystemNotification {
    SystemNotification {
        tag: "herdr-test".into(),
        title: "Herdr can reach you here".into(),
        body: "An agent that needs you, hits an error or finishes shows like this.".into(),
        actions: Vec::new(),
    }
}

pub fn system_notification(note: &Note) -> SystemNotification {
    SystemNotification {
        tag: tag(&note.address).into(),
        title: note.title().into(),
        body: note.body().into(),
        actions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(alert: Alert) -> Note {
        Note { address: "local:t7".into(), alert, name: "Claude".into(), place: Some("herdr / main".into()), message: String::new() }
    }

    #[test]
    fn notifies_only_when_away_unless_always() {
        use SystemNotifications::*;
        let cases = [
            // mode, app_active, notify
            (Background, false, true),
            (Background, true, false),
            (Always, true, true),
            (Always, false, true),
            (Off, false, false),
            (Off, true, false),
        ];
        for (mode, active, want) in cases {
            assert_eq!(should_notify(mode, active), want, "{mode:?} active={active}");
        }
    }

    #[test]
    fn the_notification_says_what_the_toast_says_tagged_by_agent() {
        let n = system_notification(&note(Alert::Blocked));
        assert_eq!((n.tag.as_ref(), n.title.as_ref(), n.body.as_ref()), ("herdr-agent:local:t7", "Claude needs you", "herdr / main"));
        let done = system_notification(&Note { place: None, ..note(Alert::Done) });
        assert_eq!((done.title.as_ref(), done.body.as_ref()), ("Claude finished", ""));
        assert_eq!(address_of(&n.tag), Some("local:t7"));
        assert_eq!(address_of("herdr-agent:"), None);
        assert_eq!(address_of("other:local:t7"), None);
    }

    #[test]
    fn the_agents_own_message_goes_over_the_place() {
        let n = Note { message: "approve Bash: make".into(), ..note(Alert::Blocked) };
        assert_eq!(system_notification(&n).body.as_ref(), "approve Bash: make\nherdr / main");
        let alone = Note { place: None, message: "  done: 3 files  ".into(), ..note(Alert::Done) };
        assert_eq!(system_notification(&alone).body.as_ref(), "done: 3 files");
        let report = serde_json::json!({ "state": "blocked", "kind": "question", "title": "t", "msg": "which one?" });
        assert_eq!(message_of(Some(&report)), "which one?");
        assert_eq!(message_of(Some(&serde_json::json!("blocked"))), "", "a bare word says nothing more");
        assert_eq!(message_of(None), "");
    }

    #[test]
    fn an_error_is_told_apart_from_blocked() {
        assert_eq!(note(Alert::Error).title(), "Claude hit an error");
        assert!(Alert::Error.needs_you() && Alert::Blocked.needs_you() && !Alert::Done.needs_you());
    }
}
