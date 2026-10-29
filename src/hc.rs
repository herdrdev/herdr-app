//! One framed connection to herdr-core through `hc connect`: JSON-RPC on
//! channel 0, terminal byte streams on their own channels.

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const HEADER: usize = 9;
pub const DATA: u8 = 0;
pub const CREDIT: u8 = 1;
pub const CLOSE: u8 = 2;
pub const RESIZE: u8 = 3;
pub const FOCUS: u8 = 4;
pub const FACTS: u8 = 5;

pub enum Stream {
    Data(Vec<u8>),
    /// The core ended the stream; the payload says why (e.g. the program exited).
    Closed(Value),
    /// The connection to the core dropped. The program is most likely still
    /// running (cores restart and their programs survive), so reattach later.
    Lost,
}

/// Event streams are lines; DATA frame boundaries can fall anywhere in a line.
#[derive(Default)]
pub struct EventLines {
    pending: Vec<u8>,
    /// The last event applied: older ones were in the snapshot.
    seq: u64,
    /// Every terminal the stream describes, in its order (schema/README.md, Events).
    terminals: Vec<Value>,
}

/// What a push changed.
#[derive(Default)]
pub struct Pushed {
    /// A layout document changed, or events were missed: read them again.
    pub docs: bool,
    pub terminals: bool,
}

impl EventLines {
    /// From `events.subscribe`'s reply: its `seq` and `terminals`.
    pub fn new(snapshot: &Value) -> Self {
        let terminals = snapshot["terminals"].as_array().cloned().unwrap_or_default();
        EventLines { pending: Vec::new(), seq: snapshot["seq"].as_u64().unwrap_or(0), terminals }
    }

    /// The live terminals, as `terminal.list` puts them.
    pub fn live(&self) -> Value {
        let live: Vec<&Value> = self.terminals.iter().filter(|t| t["state"] == "live").collect();
        json!({ "terminals": live })
    }

    pub fn push(&mut self, bytes: &[u8]) -> Pushed {
        self.pending.extend_from_slice(bytes);
        let mut consumed = 0;
        let mut pushed = Pushed::default();
        let lines: Vec<Value> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| {
                let line = serde_json::from_slice::<Value>(&self.pending[consumed..i]).unwrap_or_default();
                consumed = i + 1;
                line
            })
            .collect();
        self.pending.drain(..consumed);
        for line in lines {
            let p = &line["params"];
            match line["method"].as_str() {
                Some("missed") => {
                    self.terminals = p["terminals"].as_array().cloned().unwrap_or_default();
                    self.seq = p["seq"].as_u64().unwrap_or(self.seq);
                    pushed.docs = true;
                    pushed.terminals = true;
                }
                Some("event") if p["seq"].as_u64().is_none_or(|s| s > self.seq) => {
                    self.seq = p["seq"].as_u64().unwrap_or(self.seq);
                    if p["type"] == "doc" {
                        pushed.docs = true;
                    } else {
                        pushed.terminals |= self.apply(p);
                    }
                }
                _ => {}
            }
        }
        pushed
    }

    /// One event's change to the terminal it names (schema/README.md, Events).
    fn apply(&mut self, p: &Value) -> bool {
        let (kind, address, data) = (p["type"].as_str().unwrap_or_default(), &p["terminal"], &p["data"]);
        let at = self.terminals.iter().position(|t| &t["address"] == address);
        let whole = match kind {
            "created" | "restarted" => Some(data),
            "labels" => data.get("record"),
            _ => None,
        };
        if let Some(record) = whole {
            match at {
                Some(i) => self.terminals[i] = record.clone(),
                None => self.terminals.push(record.clone()),
            }
            return true;
        }
        let Some(i) = at else { return false };
        if kind == "removed" {
            self.terminals.remove(i);
            return true;
        }
        let t = &mut self.terminals[i];
        let mut set = |keys: &[&str]| keys.iter().for_each(|k| t[*k] = data[*k].clone());
        match kind {
            "finished" | "lost" => {
                set(&["reason", "exit_code", "signal", "finished_ms"]);
                t["state"] = if kind == "lost" { "live" } else { "finished" }.into();
                if kind == "lost" {
                    t["pid"] = Value::Null;
                }
                t.as_object_mut().map(|r| r.remove("foreground"));
            }
            "status" => set(&["status", "records", "reported"]),
            "turn" => set(&["turn"]),
            "title" => set(&["title"]),
            "labels" => set(&["labels"]),
            "keep" => set(&["keep"]),
            "size" => set(&["cols", "rows"]),
            "foreground" => t["foreground"] = data.clone(),
            _ => return false,
        }
        true
    }
}

#[derive(Default)]
struct Routes {
    closed: bool,
    replies: HashMap<u64, mpsc::Sender<Result<Value, Value>>>,
    streams: HashMap<u32, async_channel::Sender<Stream>>,
    /// Frames for a channel whose attach reply hasn't been handled yet.
    early: HashMap<u32, Vec<Stream>>,
}

/// The supported core API generation; a generation change requires a matching app update.
pub const GENERATION: u64 = 1;
/// What the app calls on the core: a core without one of them can't run it.
pub const USES: &[&str] = &["terminal.run", "terminal.list", "terminal.kill", "terminal.label", "terminal.attach", "terminal.keep", "doc.get", "doc.set", "events.subscribe"];

#[derive(Debug)]
pub struct RpcError(pub Value);

impl RpcError {
    pub fn kind(&self) -> Option<&str> { self.0.pointer("/data/kind").and_then(Value::as_str) }
}
impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.get("message").and_then(Value::as_str).unwrap_or("core call failed"))?;
        if let Some(hint) = self.0.pointer("/data/hint").and_then(Value::as_str) { write!(f, " ({hint})")?; }
        Ok(())
    }
}
impl std::error::Error for RpcError {}

pub fn error_kind(error: &anyhow::Error) -> Option<&str> {
    error.downcast_ref::<RpcError>().and_then(RpcError::kind)
}

/// Why a running core can't serve this app.
#[derive(Debug, Clone, PartialEq)]
pub enum Mismatch {
    /// Older than the oldest core this app works with: update the core.
    TooOld { have: String, need: String },
    /// A newer generation than this app speaks: update the app.
    Newer { generation: u64 },
    /// It lacks methods the app calls (an unusual build): update the core.
    Missing(Vec<String>),
}

/// The oldest core this app works with (distribution/core-version).
pub fn core_min() -> &'static str {
    include_str!("../distribution/core-version").trim()
}

fn semver(v: &str) -> (u64, u64, u64) {
    let mut p = v.trim().trim_start_matches("hc ").split(['.', '-']).map(|n| n.parse::<u64>().unwrap_or(0));
    (p.next().unwrap_or(0), p.next().unwrap_or(0), p.next().unwrap_or(0))
}

/// Whether a core's hello says it can serve this app.
pub fn check(hello: &Value) -> Result<(), Mismatch> {
    let generation = hello.get("generation").and_then(Value::as_u64).unwrap_or(0);
    if generation > GENERATION {
        return Err(Mismatch::Newer { generation });
    }
    let have = hello.get("version").and_then(Value::as_str).unwrap_or("0.0.0").to_owned();
    if generation < GENERATION || semver(&have) < semver(core_min()) {
        return Err(Mismatch::TooOld { have, need: core_min().to_owned() });
    }
    let methods: Vec<&str> = hello.get("methods").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let missing: Vec<String> = USES.iter().filter(|m| !methods.contains(m)).map(|m| m.to_string()).collect();
    if missing.is_empty() { Ok(()) } else { Err(Mismatch::Missing(missing)) }
}

pub struct Core {
    /// What the core said when we connected (generation, version, methods).
    pub hello: Value,
    /// Frames for the writer thread: a core that stops reading never blocks the caller (the UI).
    writer: mpsc::Sender<Vec<u8>>,
    routes: Arc<Mutex<Routes>>,
    next_id: AtomicU64,
    child: Mutex<Child>,
    /// Closes when the connection to the core drops.
    lost: async_channel::Receiver<()>,
}

pub struct Attached {
    pub channel: u32,
    pub stream: async_channel::Receiver<Stream>,
    pub terminal: Value,
}

fn encode(channel: u32, kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&channel.to_le_bytes());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn read_frame(r: &mut impl Read) -> Option<(u32, u8, Vec<u8>)> {
    let mut h = [0u8; HEADER];
    r.read_exact(&mut h).ok()?;
    let channel = u32::from_le_bytes(h[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(h[5..9].try_into().unwrap()) as usize;
    let mut payload = vec![0; len];
    r.read_exact(&mut payload).ok()?;
    Some((channel, h[4], payload))
}

impl Core {
    /// Connects to the user's core (starting it if needed) and switches to framed mode.
    pub fn connect() -> Result<Arc<Core>> {
        let mut child = crate::spawn::command(crate::core_bin::path())
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("cannot run `hc connect`: is herdr-core installed?")?;
        let mut stdin = child.stdin.take().unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"hello\",\"params\":{\"framed\":true}}\n")?;
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let hello: Value = serde_json::from_str(&line).context("bad hello reply from core")?;
        if let Some(err) = hello.get("error") {
            bail!("core refused hello: {err}");
        }

        let (writer, frames) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new().name("hc-writer".into()).spawn(move || {
            while let Ok(frame) = frames.recv() {
                if stdin.write_all(&frame).is_err() {
                    break;
                }
            }
        })?;

        let routes = Arc::new(Mutex::new(Routes::default()));
        let (lost_tx, lost) = async_channel::bounded::<()>(1);
        let core = Arc::new(Core {
            hello: hello.get("result").cloned().unwrap_or(Value::Null),
            writer,
            routes: routes.clone(),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
            lost,
        });
        std::thread::Builder::new().name("hc-reader".into()).spawn(move || {
            let _lost_tx = lost_tx;
            while let Some((channel, kind, payload)) = read_frame(&mut reader) {
                let mut r = routes.lock().unwrap();
                if channel == 0 {
                    let Ok(msg) = serde_json::from_slice::<Value>(&payload) else { continue };
                    let Some(id) = msg.get("id").and_then(Value::as_u64) else { continue };
                    if let Some(tx) = r.replies.remove(&id) {
                        let _ = tx.send(match msg.get("error") {
                            Some(e) => Err(e.clone()),
                            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                        });
                    }
                    continue;
                }
                let item = match kind {
                    DATA => Stream::Data(payload),
                    CLOSE => Stream::Closed(serde_json::from_slice(&payload).unwrap_or(Value::Null)),
                    _ => continue,
                };
                let closing = matches!(item, Stream::Closed(_));
                match r.streams.get(&channel) {
                    Some(tx) => {
                        let _ = tx.try_send(item);
                    }
                    None => r.early.entry(channel).or_default().push(item),
                }
                if closing {
                    r.streams.remove(&channel);
                }
            }
            // The core went away: tell every pane, and fail calls still waiting.
            let mut r = routes.lock().unwrap();
            r.closed = true;
            for (_, tx) in r.streams.drain() {
                let _ = tx.try_send(Stream::Lost);
            }
            for (_, tx) in r.replies.drain() {
                let _ = tx.send(Err(json!({"message": "lost the core"})));
            }
        })?;
        Ok(core)
    }

    /// Resolves once the connection to the core is gone.
    pub async fn closed(&self) {
        let _ = self.lost.recv().await;
    }

    pub fn send(&self, channel: u32, kind: u8, payload: &[u8]) {
        let _ = self.writer.send(encode(channel, kind, payload));
    }

    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(method, params, Duration::from_secs(10))
    }

    pub fn call_timeout(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut routes = self.routes.lock().unwrap();
            if routes.closed { bail!("{method}: lost the core"); }
            routes.replies.insert(id, tx);
        }
        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send(0, DATA, req.to_string().as_bytes());
        match rx.recv_timeout(timeout) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(anyhow::Error::new(RpcError(e)).context(method.to_owned())),
            Err(_) => {
                self.routes.lock().unwrap().replies.remove(&id);
                Err(anyhow!("{method}: no reply from core"))
            },
        }
    }

    /// Starts a program in a new core terminal and returns its address (`tN`).
    /// Starts a terminal, here or on another machine (`on`: its name in `hc machine ls`).
    pub fn run(&self, argv: Vec<String>, cwd: Option<String>, labels: Value, on: Option<&str>) -> Result<String> {
        let env = json!({"TERM": "xterm-256color", "COLORTERM": "truecolor", "TERM_PROGRAM": "herdr"});
        let r = self.call("terminal.run", json!({"argv": argv, "cwd": cwd, "env": env, "labels": labels, "on": on, "keep": true}))?;
        r.get("address")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("terminal.run returned no address: {r}"))
    }

    /// One subscription includes local and forwarded remote events.
    /// The event stream, and its snapshot (`seq`, `terminals`) to apply it to.
    pub fn subscribe(&self) -> Result<(u32, async_channel::Receiver<Stream>, Value)> {
        let result = self.call("events.subscribe", json!({}))?;
        let channel = result["channel"].as_u64().ok_or_else(|| anyhow!("subscription returned no channel"))? as u32;
        let (tx, rx) = async_channel::unbounded();
        let mut r = self.routes.lock().unwrap();
        for item in r.early.remove(&channel).unwrap_or_default() { let _ = tx.try_send(item); }
        r.streams.insert(channel, tx);
        drop(r);
        self.credit(channel, 1024 * 1024);
        Ok((channel, rx, result))
    }

    pub fn attach(&self, terminal: &str, cols: u16, rows: u16, display: Value) -> Result<Attached> {
        let result = self.call(
            "terminal.attach",
            json!({"terminal": terminal, "cols": cols, "rows": rows, "history": 2000, "display": display}),
        )?;
        let channel = result["channel"].as_u64().ok_or_else(|| anyhow!("attach returned no channel"))? as u32;
        let (tx, rx) = async_channel::unbounded();
        let mut r = self.routes.lock().unwrap();
        for item in r.early.remove(&channel).unwrap_or_default() {
            let _ = tx.try_send(item);
        }
        r.streams.insert(channel, tx);
        Ok(Attached { channel, stream: rx, terminal: result["terminal"].clone() })
    }

    pub fn input(&self, channel: u32, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.send(channel, DATA, bytes);
        }
    }

    pub fn credit(&self, channel: u32, n: usize) {
        self.send(channel, CREDIT, &(n as u32).to_le_bytes());
    }

    pub fn resize(&self, channel: u32, cols: u16, rows: u16) {
        let mut p = cols.to_le_bytes().to_vec();
        p.extend(rows.to_le_bytes());
        self.send(channel, RESIZE, &p);
    }

    /// This viewer's display facts changed (colors, cell size): the core
    /// answers the program's queries with them.
    pub fn facts(&self, channel: u32, display: &Value) {
        self.send(channel, FACTS, display.to_string().as_bytes());
    }

    pub fn focus(&self, channel: u32, focused: bool) {
        self.send(channel, FOCUS, &[focused as u8]);
    }

    pub fn detach(&self, channel: u32) {
        self.routes.lock().unwrap().streams.remove(&channel);
        self.send(channel, CLOSE, &[]);
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_stream_keeps_the_terminals_as_the_core_has_them() {
        let event = |seq: u64, kind: &str, t: &str, data: Value| {
            format!("{}\n", json!({"method": "event", "params": {"seq": seq, "type": kind, "terminal": t, "data": data}}))
        };
        let rec = |a: &str| json!({"address": a, "state": "live", "title": "", "labels": {}, "status": null});
        let mut lines = EventLines::new(&json!({"seq": 5, "terminals": [rec("t1")]}));
        let addresses = |l: &EventLines| l.live()["terminals"].as_array().unwrap().iter().map(|t| t["address"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(addresses(&lines), ["t1"]);
        assert!(!lines.push(event(5, "title", "t1", json!({"title": "old"})).as_bytes()).terminals, "not newer than the snapshot");
        let got = lines.push(format!("{}{}", event(6, "created", "t2", rec("t2")), event(7, "title", "t1", json!({"title": "hi"}))).as_bytes());
        assert!(got.terminals && !got.docs);
        assert_eq!(addresses(&lines), ["t1", "t2"]);
        assert_eq!(lines.live()["terminals"][0]["title"], "hi");
        lines.push(event(8, "status", "t2", json!({"status": {"state": "working"}, "records": {}, "reported": true})).as_bytes());
        lines.push(event(9, "foreground", "t2", json!({"pid": 7, "name": "vim", "argv": ["vim"], "cwd": "/src"})).as_bytes());
        assert_eq!(lines.live()["terminals"][1]["status"]["state"], "working");
        assert_eq!(lines.live()["terminals"][1]["foreground"]["cwd"], "/src");
        lines.push(event(10, "finished", "t1", json!({"reason": "exited", "exit_code": 0, "signal": null, "finished_ms": 1})).as_bytes());
        assert_eq!(addresses(&lines), ["t2"], "finished: no longer live");
        lines.push(event(11, "removed", "t2", json!({})).as_bytes());
        assert!(addresses(&lines).is_empty());
        let missed = format!("{}\n", json!({"method": "missed", "params": {"seq": 20, "terminals": [rec("t9")]}}));
        let got = lines.push(missed.as_bytes());
        assert!(got.terminals && got.docs, "missed: everything again");
        assert_eq!(addresses(&lines), ["t9"]);
        assert!(lines.push(format!("{}\n", json!({"method": "event", "params": {"seq": 21, "type": "doc"}})).as_bytes()).docs);
    }

    #[test]
    fn document_and_missed_events_survive_arbitrary_frame_boundaries() {
        for line in [b"{\"method\":\"event\",\"params\":{\"type\":\"doc\"}}\n".as_slice(), b"{\"method\":\"missed\",\"params\":{\"terminals\":[]}}\n".as_slice()] {
            for at in 1..line.len() {
                let mut lines = EventLines::default();
                assert!(!lines.push(&line[..at]).docs);
                assert!(lines.push(&line[at..]).docs);
                assert!(lines.pending.is_empty());
            }
            let mut lines = EventLines::default();
            let mut refresh = false;
            for chunk in line.chunks(3) { refresh |= lines.push(chunk).docs; }
            assert!(refresh);
        }
        let mut old = EventLines::default();
        old.push(b"{\"method\":\"mis");
        let mut replacement = EventLines::default();
        assert!(replacement.push(b"{\"method\":\"missed\"}\n").docs);
    }

    #[test]
    fn calls_after_disconnect_fail_without_waiting_for_a_reply() {
        let (writer, _frames) = mpsc::channel();
        let (_lost_tx, lost) = async_channel::bounded(1);
        let child = std::process::Command::new(std::env::current_exe().unwrap()).arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let core = Core {
            hello: Value::Null,
            writer,
            routes: Arc::new(Mutex::new(Routes { closed: true, ..Default::default() })),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
            lost,
        };
        let start = std::time::Instant::now();
        assert!(core.call("doc.get", json!({"name":"herdr-app"})).unwrap_err().to_string().contains("lost the core"));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(core.routes.lock().unwrap().replies.is_empty());
    }

    fn hello(generation: u64, version: &str, methods: &[&str]) -> Value {
        json!({ "generation": generation, "version": version, "methods": methods })
    }

    #[test]
    fn a_core_serves_the_app_by_generation_version_and_methods() {
        assert_eq!(check(&hello(1, "9.9.9", USES)), Ok(()));
        assert_eq!(check(&hello(1, "0.0.1", USES)), Err(Mismatch::TooOld { have: "0.0.1".into(), need: core_min().into() }));
        assert_eq!(check(&hello(2, "9.9.9", USES)), Err(Mismatch::Newer { generation: 2 }));
        assert_eq!(check(&hello(1, "9.9.9", &["terminal.run"])), Err(Mismatch::Missing(USES[1..].iter().map(|m| m.to_string()).collect())));
        // 0.10 is newer than 0.9 (numbers, not text).
        assert!(semver("0.10.0") > semver("0.9.3"));
    }
}
