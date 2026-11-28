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
    pub version: u64,
    pub available: bool,
    pub read_attempted: bool,
    pub connected: bool,
    pub legacy_child_lifetime: bool,
    connected_since: Option<u64>,
    pub pending: Option<mpsc::Receiver<Outcome>>,
    pub dirty: bool,
    pub blocked: bool,
    pub error: Option<String>,
    pub notified_error: Option<String>,
}

impl Slot {
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
