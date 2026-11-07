//! Latest-state CAS flushing, with one request per machine and no operation queue.
use crate::{
    hc::{self, Core},
    mirror::Document,
};
use serde_json::json;
use std::{sync::mpsc, time::Duration};

#[derive(Default)]
pub struct Slot {
    pub document: Option<Document>,
    /// Ephemeral projection at dismissal, retaining edits awaiting acknowledgement.
    /// Never persisted or replayed across an authoritative replacement.
    pub staged: Option<Document>,
    pub version: u64,
    pub available: bool,
    pub read_attempted: bool,
    pub connected: bool,
    connected_since: Option<u64>,
    pub pending: Option<mpsc::Receiver<Outcome>>,
    pub dirty: bool,
    pub blocked: bool,
    pub error: Option<String>,
    pub notified_error: Option<String>,
}

impl Slot {
    pub fn source(&self) -> Option<&Document> { self.staged.as_ref().or(self.document.as_ref()) }

    pub fn capture(&self, model: &crate::model::Model, machine: Option<&str>) -> Document {
        Document::capture_with_base(model, machine, self.source())
    }

    /// Called before removing visible work, while its full current content exists.
    pub fn stage_dismissal(&mut self, model: &crate::model::Model, machine: &str) {
        let current = self.capture(model, Some(machine));
        self.staged = (self.document.as_ref() != Some(&current)).then_some(current);
    }

    pub fn acknowledge(&mut self, document: Document, model: &crate::model::Model, machine: Option<&str>) {
        self.document = Some(document);
        if self.staged.is_some()
            && self.capture(model, machine) == Document::capture_with_base(model, machine, self.document.as_ref()) {
            self.staged = None;
        }
    }

    pub fn layout(&self) -> &'static str {
        if self.available { "ready" }
        else if self.connected && !self.read_attempted { "loading" }
        else { "unavailable" }
    }

    pub fn ready(&self) -> bool {
        self.connected && self.pending.is_none() && (!self.blocked || self.dirty)
    }

    pub fn connection(&mut self, connected: bool, since: Option<u64>, reason: Option<String>) {
        if connected && (!self.connected || self.connected_since != since) {
            self.blocked = false;
            self.dirty = true;
            self.available = false;
            self.read_attempted = false;
            self.error = None;
        } else if !connected {
            self.available = false;
            self.error = reason;
        }
        self.connected = connected;
        self.connected_since = since;
    }
}

pub struct Work {
    pub machine: String,
    pub version: u64,
    pub write: Option<Document>,
}

pub enum Outcome {
    Read(Document, u64),
    Written(Document, u64),
    Conflict(Document, u64),
    Undecodable(String),
    Blocked(String),
    Unavailable(String),
}

fn rpc_error(error: anyhow::Error) -> Outcome {
    let reason = format!("{error:#}");
    match hc::error_kind(&error) {
        Some("needs_update" | "needs_login") => Outcome::Blocked(reason),
        _ => Outcome::Unavailable(reason),
    }
}

fn read(core: &Core, machine: &str, timeout: Duration) -> Result<(Document, u64), Outcome> {
    let on = (!machine.is_empty()).then_some(machine);
    let result = core.call_timeout("doc.get", json!({"name":"herdr-app", "on":on}), timeout).map_err(rpc_error)?;
    let document = if result["value"].is_null() {
        Document::default()
    } else {
        serde_json::from_value(result["value"].clone()).map_err(|e| Outcome::Undecodable(format!("Cannot read this machine's layout: {e}")))?
    };
    let version = result["version"].as_u64().ok_or_else(|| Outcome::Undecodable("Layout document has no version".into()))?;
    Ok((document, version))
}

pub fn round(core: &Core, work: Work) -> (String, Outcome) {
    round_timeout(core, work, Duration::from_secs(10))
}

pub fn round_timeout(core: &Core, work: Work, timeout: Duration) -> (String, Outcome) {
    let on = (!work.machine.is_empty()).then_some(work.machine.as_str());
    let deadline = std::time::Instant::now() + timeout;
    let result = match work.write {
        Some(document) => match core.call_timeout("doc.set", json!({"name":"herdr-app", "on":on, "version":work.version, "value":document}), timeout) {
            Ok(v) => match v["version"].as_u64() {
                Some(version) => Outcome::Written(document, version),
                None => Outcome::Undecodable("Layout write returned no version".into()),
            },
            Err(e) if hc::error_kind(&e) == Some("conflict") => match read(core, &work.machine, deadline.saturating_duration_since(std::time::Instant::now())) {
                Ok((d, v)) => Outcome::Conflict(d, v),
                Err(e) => e,
            },
            Err(e) => rpc_error(e),
        },
        None => match read(core, &work.machine, timeout) {
            Ok((d, v)) => Outcome::Read(d, v),
            Err(e) => e,
        },
    };
    (work.machine, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote() -> (crate::model::Model, Document) {
        use crate::model::{Model, Tab};
        let mut local = Model::default();
        local.add_space("s", "space", "/tmp", Tab::new("a", "A", "pa", "ta"));
        local.space_mut("s").unwrap().tabs.push(Tab::new("b", "B", "pb", "tb"));
        let doc = Document::capture(&local, None);
        let mut model = Model::default();
        doc.replace(&mut model, Some("remote"));
        (model, doc)
    }

    #[test]
    fn staged_dismissal_preserves_pending_rename_new_tab_and_split() {
        use crate::model::Tab;
        let (mut model, doc) = remote();
        let mut slot = Slot { document: Some(doc.clone()), version: 7, ..Default::default() };
        let (_tx, rx) = mpsc::channel();
        slot.pending = Some(rx);
        model.tab_mut("remote/b").unwrap().name = "newer rename".into();
        model.split("remote/pb", crate::layout::Axis::Row, "remote/new-pane", "remote/new-term");
        model.space_mut("remote/s").unwrap().tabs.push(Tab::new("remote/new-tab", "new", "remote/pn", "remote/tn"));
        let expected = Document::capture(&model, Some("remote"));
        slot.stage_dismissal(&model, "remote");
        assert_eq!(slot.document.as_ref(), Some(&doc));
        assert_eq!(slot.version, 7);
        model.dismiss_tab("remote/b");
        model.dismiss_tab("remote/new-tab");
        assert_ne!(Document::capture_with_base(&model, Some("remote"), slot.document.as_ref()), expected,
            "the old authoritative-only projection loses the pending rename, split and newborn tab");
        assert_eq!(slot.capture(&model, Some("remote")), expected);
        slot.acknowledge(doc.clone(), &model, Some("remote"));
        assert!(slot.staged.is_some(), "older Written must not discard newer hidden edits");
        assert_eq!(slot.capture(&model, Some("remote")), expected);
        // Unrelated visible edits must not keep staging alive after hidden edits land.
        model.tab_mut("remote/a").unwrap().name = "visible edit after write".into();
        slot.acknowledge(expected, &model, Some("remote"));
        assert!(slot.staged.is_none());
        assert!(slot.capture(&model, Some("remote")).project(Some("remote"))[0].tabs.iter().any(|t| t.name == "visible edit after write"));
    }

    #[test]
    fn staged_identity_replacement_survives_older_written_and_restore() {
        let (mut model, doc) = remote();
        let mut slot = Slot { document: Some(doc.clone()), ..Default::default() };
        model.tab_mut("remote/b").unwrap().root = crate::model::Tab::new("remote/b", "", "remote/new-pane", "remote/new-terminal").root;
        let expected = Document::capture(&model, Some("remote"));
        slot.stage_dismissal(&model, "remote");
        model.dismiss_tab("remote/b");
        slot.acknowledge(doc, &model, Some("remote"));
        assert_eq!(slot.capture(&model, Some("remote")), expected);
        model.tab_mut("remote/a").unwrap().name = "visible".into();
        assert!(slot.source().unwrap().restore(&mut model, "remote", "remote/b", true));
        assert_eq!(model.tab("remote/a").unwrap().name, "visible");
        assert_eq!(model.pane_of("remote/new-terminal").as_deref(), Some("remote/new-pane"));
    }

    #[test]
    fn layout_loading_ends_on_read_failure_and_restarts_on_reconnect() {
        let mut slot = Slot::default();
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(1), None);
        assert_eq!(slot.layout(), "loading");
        slot.read_attempted = true;
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(1), None);
        assert_eq!(slot.layout(), "unavailable");
        slot.available = true;
        assert_eq!(slot.layout(), "ready");
        slot.connection(false, Some(2), Some("offline".into()));
        assert_eq!(slot.layout(), "unavailable");
        slot.connection(true, Some(3), None);
        assert_eq!(slot.layout(), "loading");
    }

    #[test]
    fn blocked_reads_wait_for_an_event_or_reconnect() {
        let mut slot = Slot { blocked: true, connected: true, ..Default::default() };
        assert!(!slot.ready());
        slot.dirty = true;
        assert!(slot.ready());
        let (_tx, rx) = mpsc::channel();
        slot.pending = Some(rx);
        assert!(!slot.ready());
        slot.pending = None;
        slot.dirty = false;
        assert!(!slot.ready());
        slot.blocked = false;
        assert!(slot.ready());
    }

    #[test]
    fn offline_slots_do_not_read_even_when_dirty() {
        let mut slot = Slot::default();
        slot.connection(false, Some(1), Some("machine is connecting".into()));
        slot.dirty = true;
        assert!(!slot.ready());
        assert_eq!(slot.error.as_deref(), Some("machine is connecting"));
        slot.connection(true, Some(2), None);
        assert!(slot.ready());
        assert!(slot.error.is_none());
    }

    #[test]
    fn reconnect_rearms_blocked_slots_but_a_connected_poll_does_not() {
        let mut slot = Slot::default();
        slot.connection(true, Some(1), None);
        slot.blocked = true;
        slot.dirty = false;
        slot.connection(true, Some(1), None);
        assert!(!slot.ready());
        slot.connection(true, Some(2), None);
        assert!(slot.ready());
        assert!(!slot.blocked);
        slot.blocked = true;
        slot.dirty = false;
        slot.connection(false, Some(3), Some("needs_update".into()));
        assert!(!slot.ready());
        slot.connection(true, Some(4), None);
        assert!(slot.ready());
    }

    #[test]
    fn login_and_update_errors_keep_the_real_reason_and_hint() {
        for kind in ["needs_login", "needs_update"] {
            let error = anyhow::Error::new(hc::RpcError(json!({"message":"cannot use machine", "data":{"kind":kind,"hint":"run hc machine login"}})));
            let Outcome::Blocked(reason) = rpc_error(error) else { panic!("must not retry this error on a timer") };
            assert!(reason.contains("cannot use machine"));
            assert!(reason.contains("run hc machine login"));
        }
    }
}
