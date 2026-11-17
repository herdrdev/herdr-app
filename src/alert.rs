//! System notifications: what Settings > Alerts sends to ask for permission.

use gpui::SystemNotification;

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
