//! Anonymous usage data: one small report per install per UTC day, so we
//! know how Herdr is used. Never what's in a terminal, never from the core,
//! never free text: every field is a number, a bucket, a yes/no or a name
//! from a list the app ships (anything else is `other`).
//!
//! The window tells this module what it sees now and then (`observe`,
//! `command`, `count`); a thread of its own keeps the day's counts in
//! `telemetry/today.json`, freezes a finished day into `telemetry/queue/`
//! and sends it. Nothing here may affect the app: it runs off the UI thread,
//! never waits on anything, and every error just loses data.
//!
//! On unless settings.json says `"telemetry": false` (that also deletes the
//! id and anything not sent yet). `DO_NOT_TRACK`, `HERDR_TELEMETRY=0` and CI
//! turn it off for one run. Only release builds (`HERDR_APP_RELEASE`) send,
//! or a build pointed at a test server with `HERDR_TELEMETRY_URL`: a build
//! from source and the sandbox collect and show, but never send.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

pub const VERSION: u32 = 1;
const ENDPOINT: &str = "https://telemetry.herdr.dev/v1/report";
/// Frozen reports older than this are dropped unsent.
const KEEP_DAYS: i64 = 7;
const TICK: Duration = Duration::from_secs(30);

/// What the window sees, every few seconds while telemetry is on.
#[derive(Default)]
pub struct Observation {
    pub foreground: bool,
    /// Every terminal reporting an agent: its 7501 `app` (made safe in `agent_name`).
    pub agents: Vec<Option<String>>,
    pub agents_working: usize,
    pub spaces: usize,
    pub tabs: usize,
    pub terminals: usize,
    pub layout: String,
    /// Mods on screen (frame mods left out).
    pub mods: Vec<String>,
    pub theme: String,
    /// The welcome's outcome (shared `frame.welcomed`).
    pub welcome: Option<String>,
}

enum Event {
    Observe(Observation),
    Command(&'static str),
    Count(&'static str),
    Startup(u64),
    /// Consent changed: look at `ON` now (a lost one is seen on the next tick).
    Wake,
    /// A clean quit: save the day, clear the unclean-exit marker. Best
    /// effort; quitting never waits for it.
    Quit,
}

/// Events carry the consent epoch they were made in (`EPOCH`).
static TX: OnceLock<SyncSender<(u64, Event)>> = OnceLock::new();
/// Bumped whenever collecting stops: everything from before is wiped and
/// dropped, even if it's back on by the time the worker looks.
static EPOCH: AtomicU64 = AtomicU64::new(0);
/// settings.json's `telemetry`.
static SETTING: AtomicBool = AtomicBool::new(false);
/// Collecting now: the setting is on and no env var turned it off. The
/// worker obeys this, not its events, so an opt-out can't get lost.
static ON: AtomicBool = AtomicBool::new(false);
/// For the settings screen (`status`), refreshed by the thread.
static STATUS: Mutex<Option<Value>> = Mutex::new(None);
static STARTED: OnceLock<Instant> = OnceLock::new();

fn dir() -> PathBuf {
    crate::mods::app_dir().join("telemetry")
}

/// Why it's off for this run, whatever the setting says.
fn env_off() -> Option<&'static str> {
    static OFF: OnceLock<Option<&'static str>> = OnceLock::new();
    *OFF.get_or_init(read_env_off)
}

fn read_env_off() -> Option<&'static str> {
    let set = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let truthy = |v: String| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes");
    if set("DO_NOT_TRACK").is_some_and(truthy) {
        return Some("DO_NOT_TRACK");
    }
    if set("HERDR_TELEMETRY").is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no")) {
        return Some("HERDR_TELEMETRY");
    }
    const CI: &[&str] = &["CI", "GITHUB_ACTIONS", "GITLAB_CI", "CIRCLECI", "BUILDKITE", "JENKINS_URL", "TEAMCITY_VERSION"];
    CI.iter().any(|k| set(k).is_some()).then_some("CI")
}

/// Where reports go, if this build sends at all.
fn endpoint() -> Option<String> {
    match std::env::var("HERDR_TELEMETRY_URL").ok().filter(|u| !u.is_empty()) {
        Some(url) => Some(url),
        None => option_env!("HERDR_APP_RELEASE").map(|_| ENDPOINT.to_owned()),
    }
}

pub fn collecting() -> bool {
    ON.load(Ordering::Relaxed)
}

fn send(e: Event) {
    if let Some(tx) = TX.get() {
        let _ = tx.try_send((EPOCH.load(Ordering::SeqCst), e));
    }
}

pub fn observe(o: Observation) {
    if collecting() {
        send(Event::Observe(o));
    }
}

/// A command the person ran (names come from `commands::COMMANDS`).
pub fn command(name: &'static str) {
    if collecting() {
        send(Event::Command(name));
    }
}

/// Something that happened, by a fixed name (`core_lost`, `app_update_failed`...).
pub fn count(name: &'static str) {
    if collecting() {
        send(Event::Count(name));
    }
}

/// The window is up: how long the start took.
pub fn window_open() {
    if collecting()
        && let Some(t) = STARTED.get()
    {
        send(Event::Startup(t.elapsed().as_millis() as u64));
    }
}

/// settings.json's `telemetry` (read again whenever the file changes).
pub fn set_setting(on: bool) {
    SETTING.store(on, Ordering::Relaxed);
    let now = on && env_off().is_none();
    if ON.swap(now, Ordering::SeqCst) && !now {
        EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    send(Event::Wake);
}

/// What the settings screen shows.
pub fn status() -> Value {
    STATUS.lock().ok().and_then(|s| s.clone()).unwrap_or(Value::Null)
}

/// A clean quit, handed to the worker (never waited for).
pub fn quit() {
    if collecting() {
        send(Event::Quit);
    }
}

/// When the app started, for how long the window took.
pub fn clock() {
    let _ = STARTED.set(Instant::now());
}

/// Starts the thread. `setting`: settings.json's `telemetry`.
pub fn start(setting: bool) {
    let (tx, rx) = std::sync::mpsc::sync_channel(512);
    if TX.set(tx).is_err() {
        return;
    }
    SETTING.store(setting, Ordering::Relaxed);
    ON.store(setting && env_off().is_none(), Ordering::Relaxed);
    let _ = std::thread::Builder::new().name("telemetry".into()).spawn(move || Worker::new().run(rx));
}

// ---- the day ----------------------------------------------------------------

/// One UTC day's counts, kept locally until the day is over.
#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
struct Day {
    day: String,
    open_secs: u64,
    foreground_secs: u64,
    startups: u32,
    startup_ms: Vec<u64>,
    unclean_exits: u32,
    agents: BTreeSet<String>,
    agents_open: usize,
    agents_working: usize,
    spaces: usize,
    tabs: usize,
    terminals: usize,
    layouts: BTreeSet<String>,
    mods: BTreeSet<String>,
    commands: BTreeSet<String>,
    theme_custom: bool,
    core: Option<String>,
    welcome: Option<String>,
    events: BTreeMap<String, u32>,
}

impl Day {
    fn new(day: String) -> Day {
        Day { day, ..Day::default() }
    }

    fn observe(&mut self, o: Observation, layouts: &[&str]) {
        for a in &o.agents {
            self.agents.insert(crate::programs::agent_name(a.as_deref()));
        }
        self.agents_open = self.agents_open.max(o.agents.len());
        self.agents_working = self.agents_working.max(o.agents_working);
        self.spaces = self.spaces.max(o.spaces);
        self.tabs = self.tabs.max(o.tabs);
        self.terminals = self.terminals.max(o.terminals);
        self.layouts.insert(if layouts.contains(&o.layout.as_str()) { o.layout } else { "custom".into() });
        self.mods.extend(o.mods.into_iter().filter(|m| !m.starts_with("frame-")));
        if let Some(w) = o.welcome.filter(|w| w == "finished" || w == "skipped") {
            self.welcome = Some(w);
        }
        self.theme_custom |= !crate::settings::PRESETS.contains(&o.theme.as_str()) && !crate::settings::SHIPPED_THEMES.contains(&o.theme.as_str());
    }

    /// The report for this day: exactly what gets sent.
    fn report(&self, install: &str) -> Value {
        let (builtin, edited, custom) = crate::mods::kinds(&self.mods);
        let mut ms = self.startup_ms.clone();
        ms.sort_unstable();
        json!({
            "v": VERSION,
            "install": install,
            "day": self.day,
            "app": {
                "version": version(env!("CARGO_PKG_VERSION")),
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "channel": "stable",
            },
            "core": { "version": self.core.as_deref().map(version) },
            "usage": {
                "open": time_bucket(self.open_secs),
                "foreground": time_bucket(self.foreground_secs),
                "startups": self.startups.min(99),
                "startup": ms.get(ms.len() / 2).map(|m| startup_bucket(*m)),
                "unclean_exits": self.unclean_exits.min(99),
            },
            "agents": self.agents,
            "agents_open": bucket(self.agents_open),
            "agents_working": bucket(self.agents_working),
            "spaces": bucket(self.spaces),
            "tabs": bucket(self.tabs),
            "terminals": bucket(self.terminals),
            "layouts": self.layouts,
            "mods": { "builtin": builtin, "builtin_edited": edited, "custom": custom },
            "commands": self.commands,
            "custom": { "keys": keys_custom(), "theme": self.theme_custom },
            "welcome": self.welcome.as_deref().unwrap_or("none"),
            "events": self.events,
        })
    }
}

fn bucket(n: usize) -> &'static str {
    match n {
        0 => "0",
        1 => "1",
        2..=5 => "2-5",
        6..=20 => "6-20",
        _ => "21+",
    }
}

fn time_bucket(secs: u64) -> &'static str {
    match secs / 60 {
        0 => "0",
        1..=14 => "<15m",
        15..=59 => "15m-1h",
        60..=239 => "1-4h",
        240..=479 => "4-8h",
        _ => "8h+",
    }
}

fn startup_bucket(ms: u64) -> &'static str {
    match ms {
        0..=499 => "<0.5s",
        500..=999 => "0.5-1s",
        1000..=1999 => "1-2s",
        2000..=4999 => "2-5s",
        _ => "5s+",
    }
}

/// A release version (`0.2.0`, `0.2.0-rc.1`), else `other`: never a free string.
pub(crate) fn version(v: &str) -> String {
    let v = v.trim().trim_start_matches("hc ").trim_start_matches('v');
    let number = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
    let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
    let parts: Vec<&str> = core.split('.').collect();
    let pre_ok = pre.is_none_or(|p| {
        let (label, n) = p.split_once('.').map_or((p, None), |(l, n)| (l, Some(n)));
        matches!(label, "alpha" | "beta" | "rc" | "preview") && n.is_none_or(number)
    });
    if parts.len() == 3 && parts.iter().all(|p| number(p)) && pre_ok { v.to_owned() } else { "other".into() }
}

/// Whether keys.json differs from the shipped one. keys.json is installed
/// once, so one shipped before the find keys and left alone isn't custom.
fn keys_custom() -> bool {
    let parse = |s: &str| serde_json::from_str::<Value>(s).ok();
    match std::fs::read_to_string(crate::keys::path()) {
        Ok(text) => {
            let (mine, shipped) = (parse(&text), parse(crate::keys::DEFAULTS));
            mine != shipped && mine != shipped.map(without_find_keys)
        }
        Err(_) => false,
    }
}

/// The shipped keys as they were before terminal find.
fn without_find_keys(mut keys: Value) -> Value {
    if let Some(terminal) = keys.get_mut("terminal").and_then(Value::as_object_mut) {
        terminal.retain(|_, action| !action.as_str().is_some_and(|a| a.starts_with("terminal.find")));
    }
    keys
}

// ---- dates (UTC, no calendar crate) ------------------------------------------

fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `YYYY-MM-DD` for days since 1970-01-01 (Howard Hinnant's civil_from_days).
fn date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn today_index() -> i64 {
    (unix_now() / 86_400) as i64
}

// ---- the thread -----------------------------------------------------------------

struct Worker {
    /// Whether it was collecting when last looked (`follow_consent`).
    was_on: bool,
    epoch: u64,
    /// The saved setting the settings screen last got.
    setting_shown: bool,
    day: Day,
    install: Option<String>,
    foreground: bool,
    last_tick: Instant,
    dirty: bool,
    /// Not before then: a little after start, longer after failures.
    next_send: Instant,
    failures: u32,
    layouts: Vec<&'static str>,
}

impl Worker {
    fn new() -> Worker {
        let jitter = Duration::from_secs(20 + random_u64() % 100);
        Worker {
            was_on: false,
            epoch: EPOCH.load(Ordering::SeqCst),
            setting_shown: SETTING.load(Ordering::Relaxed),
            day: Day::new(date(today_index())),
            install: None,
            foreground: true,
            last_tick: Instant::now(),
            dirty: false,
            next_send: Instant::now() + jitter,
            failures: 0,
            layouts: crate::window::shipped_layouts(),
        }
    }

    fn on(&self) -> bool {
        collecting()
    }

    fn run(mut self, rx: Receiver<(u64, Event)>) {
        if self.on() {
            self.begin();
        } else {
            self.wipe();
        }
        self.was_on = self.on();
        self.publish();
        loop {
            let next = rx.recv_timeout(TICK);
            self.follow_consent();
            match next {
                Ok((epoch, e)) if epoch == self.epoch => self.event(e),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if self.last_tick.elapsed() >= TICK {
                self.tick();
            }
        }
    }

    /// Starts or wipes when consent changed since last looked.
    fn follow_consent(&mut self) {
        let epoch = EPOCH.load(Ordering::SeqCst);
        let mut changed = false;
        if epoch != self.epoch {
            // It was off at least once since: whatever it had goes.
            self.epoch = epoch;
            self.wipe();
            self.was_on = false;
            changed = true;
        }
        let on = self.on();
        if on != self.was_on {
            self.was_on = on;
            if on { self.begin() } else { self.wipe() }
            changed = true;
        }
        let setting = SETTING.load(Ordering::Relaxed);
        if changed || setting != self.setting_shown {
            self.setting_shown = setting;
            self.publish();
        }
    }

    /// Off (by the setting or for this run): nothing kept, nothing sent. On
    /// again starts fresh, with a new id.
    fn wipe(&mut self) {
        let _ = std::fs::remove_dir_all(dir());
        self.day = Day::new(date(today_index()));
        self.install = None;
        self.dirty = false;
    }

    /// Picks up the day so far, counts this start, notes an unclean exit.
    fn begin(&mut self) {
        let d = dir();
        let _ = std::fs::create_dir_all(d.join("queue"));
        self.install = Some(install_id());
        if let Some(saved) = std::fs::read_to_string(d.join("today.json")).ok().and_then(|s| serde_json::from_str::<Day>(&s).ok()) {
            if saved.day == self.day.day {
                self.day = saved;
            } else {
                self.freeze(&saved);
            }
        }
        if d.join("running").exists() {
            self.day.unclean_exits += 1;
        }
        let _ = std::fs::write(d.join("running"), "");
        self.day.startups += 1;
        self.dirty = true;
        self.save();
    }

    fn event(&mut self, e: Event) {
        if !self.on() || !self.was_on {
            return;
        }
        // An event after midnight belongs to the new day.
        self.roll();
        match e {
            Event::Observe(o) => {
                self.foreground = o.foreground;
                self.day.observe(o, &self.layouts);
                let core = crate::update::core().version;
                if core.is_some() {
                    self.day.core = core;
                }
            }
            Event::Command(name) => {
                self.day.commands.insert(name.to_owned());
            }
            Event::Count(name) => *self.day.events.entry(name.to_owned()).or_default() += 1,
            Event::Startup(ms) => {
                if self.day.startup_ms.len() < 50 {
                    self.day.startup_ms.push(ms);
                }
            }
            Event::Wake => return,
            Event::Quit => {
                self.dirty = true;
                self.save();
                let _ = std::fs::remove_file(dir().join("running"));
                return;
            }
        }
        self.dirty = true;
    }

    /// A new UTC day: the finished one is frozen and the new one starts.
    fn roll(&mut self) {
        let today = date(today_index());
        if self.day.day != today {
            let done = std::mem::replace(&mut self.day, Day::new(today));
            // The new day is on disk before the old one can be sent, so a
            // restart never finds the old one again.
            self.dirty = true;
            self.save();
            self.freeze(&done);
        }
    }

    fn tick(&mut self) {
        let gap = self.last_tick.elapsed();
        self.last_tick = Instant::now();
        if !self.on() {
            return;
        }
        // A long gap is a suspended machine, not use.
        if gap < TICK * 4 {
            self.day.open_secs += gap.as_secs();
            if self.foreground {
                self.day.foreground_secs += gap.as_secs();
            }
            self.dirty = true;
        }
        self.roll();
        self.save();
        self.send_queued();
        self.publish();
    }

    fn save(&mut self) {
        if self.dirty
            && let Ok(text) = serde_json::to_string(&self.day)
        {
            let _ = write_atomic(&dir().join("today.json"), &text);
            self.dirty = false;
        }
    }

    /// A finished day becomes a report that never changes (retries send the
    /// same bytes). Each day is frozen once: `frozen` holds the latest.
    fn freeze(&self, day: &Day) {
        let Some(install) = &self.install else { return };
        let marker = dir().join("frozen");
        let latest = std::fs::read_to_string(&marker).unwrap_or_default();
        if day.day.is_empty() || day.day.as_str() <= latest.trim() {
            return;
        }
        if write_atomic(&marker, &day.day).is_ok() {
            let _ = write_atomic(&dir().join("queue").join(format!("{}.json", day.day)), &day.report(install).to_string());
        }
    }

    /// Reports waiting, oldest first; ones too old to matter are dropped.
    fn queued(&self) -> Vec<(PathBuf, String)> {
        let oldest = date(today_index() - KEEP_DAYS);
        let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(dir().join("queue"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let day = path.file_stem()?.to_str()?.to_owned();
                if day < oldest {
                    let _ = std::fs::remove_file(&path);
                    return None;
                }
                Some((path, day))
            })
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }

    fn send_queued(&mut self) {
        let Some(url) = endpoint() else { return };
        if Instant::now() < self.next_send {
            return;
        }
        for (path, _) in self.queued() {
            if !self.on() {
                return;
            }
            let Ok(body) = std::fs::read_to_string(&path) else { continue };
            match post(&url, &body) {
                Some(code) if (200..300).contains(&code) => {
                    let sent = json!({ "at": unix_now(), "report": serde_json::from_str::<Value>(&body).unwrap_or(Value::Null) });
                    let _ = write_atomic(&dir().join("last-sent.json"), &sent.to_string());
                    let _ = std::fs::remove_file(&path);
                    self.failures = 0;
                }
                // The server will never take this one (a bad or too old report).
                Some(400 | 404 | 410 | 413 | 422) => {
                    let _ = std::fs::remove_file(&path);
                }
                _ => {
                    self.failures += 1;
                    let hours = 1u64 << self.failures.min(5);
                    self.next_send = Instant::now() + Duration::from_secs(hours.min(24) * 3600 + random_u64() % 600);
                    return;
                }
            }
        }
    }

    /// Refreshes what the settings screen shows; mods redraw only if it changed.
    fn publish(&self) {
        let on = self.on();
        let queued: Vec<Value> = if on {
            self.queued().into_iter().filter_map(|(p, _)| serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()).collect()
        } else {
            Vec::new()
        };
        let last = on.then(|| std::fs::read_to_string(dir().join("last-sent.json")).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok())).flatten();
        let status = json!({
            "enabled": on,
            "setting": SETTING.load(Ordering::Relaxed),
            "off_because": env_off(),
            "sends": endpoint().is_some(),
            "endpoint": endpoint().unwrap_or_else(|| ENDPOINT.to_owned()),
            "draft": self.install.as_ref().filter(|_| on).map(|i| self.day.report(i)),
            "queued": queued,
            "last_sent": last,
        });
        let changed = match STATUS.lock() {
            Ok(mut s) if s.as_ref() != Some(&status) => {
                *s = Some(status);
                true
            }
            _ => false,
        };
        if changed {
            crate::mods::request(crate::mods::Request::Refresh);
        }
    }
}

/// The random id, made on first use and kept until telemetry is turned off.
fn install_id() -> String {
    let path = dir().join("id");
    if let Ok(id) = std::fs::read_to_string(&path)
        && id.len() == 32
        && id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return id;
    }
    let id = format!("{:016x}{:016x}", random_u64(), random_u64());
    let _ = std::fs::write(&path, &id);
    id
}

/// Random enough for an id: std seeds each RandomState from the OS.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

/// POSTs one report; the HTTP status, or None if it never got an answer.
/// Turning telemetry off stops a request still in flight.
fn post(url: &str, body: &str) -> Option<u16> {
    use std::io::Write;
    use std::process::Stdio;
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut child = crate::spawn::command(crate::update::system("curl"))
        .args(["-sS", "--max-time", "20", "-X", "POST", "-H", "content-type: application/json", "--data-binary", "@-", "-o", null, "-w", "%{http_code}", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(body.as_bytes()).ok()?;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if collecting() => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok().filter(|c| *c > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_keys_before_terminal_find_lack_only_its_keys() {
        let shipped: Value = serde_json::from_str(crate::keys::DEFAULTS).unwrap();
        let before = without_find_keys(shipped.clone());
        assert!(!before.to_string().contains("terminal.find"));
        let count = |v: &Value| v["terminal"].as_object().unwrap().len();
        let find_keys = shipped["terminal"].as_object().unwrap().values().filter(|a| a.as_str().unwrap().starts_with("terminal.find")).count();
        assert!(find_keys > 0);
        assert_eq!(count(&before) + find_keys, count(&shipped));
        assert_eq!(before["global"], shipped["global"]);
    }

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(19_723), "2024-01-01");
        assert_eq!(date(19_782), "2024-02-29");
        assert_eq!(date(20_745), "2026-10-19");
    }

    #[test]
    fn versions_are_releases_or_other() {
        assert_eq!(version("0.2.0"), "0.2.0");
        assert_eq!(version("hc 0.1.5"), "0.1.5");
        assert_eq!(version("0.2.0-rc.1"), "0.2.0-rc.1");
        assert_eq!(version("0.2.0-my/secret path"), "other");
        assert_eq!(version("hc 0.1.5-acmeclient"), "other");
        assert_eq!(version("0.1.5-rc.x"), "other");
        assert_eq!(version("dev"), "other");
    }

    #[test]
    fn a_report_holds_no_free_text() {
        let mut day = Day::new("2026-10-19".into());
        let o = Observation {
            agents: vec![Some("claude-code".into()), Some("/home/me/secret-tool".into()), None],
            layout: "my-private-layout".into(),
            mods: vec!["agents".into(), "my-secret-mod".into(), "frame-settings".into()],
            theme: "my-theme".into(),
            ..Observation::default()
        };
        day.observe(o, &["main", "terminals"]);
        day.core = Some("hc /home/me/build".into());
        let text = day.report("0123456789abcdef0123456789abcdef").to_string();
        for leak in ["secret", "private", "/home", "my-theme", "frame-"] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
        assert!(text.contains("\"claude\""), "known agents are named: {text}");
        assert!(text.contains("\"other\""), "{text}");
        assert!(text.contains("\"custom\""), "{text}");
    }
}
