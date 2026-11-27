//! Counts for `scripts/bench.py`: how often the window and terminals are drawn.
//! With HERDR_APP_BENCH=<file>, one JSON line per second goes there.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub static WINDOW: AtomicU64 = AtomicU64::new(0);
pub static TERMINAL: AtomicU64 = AtomicU64::new(0);
pub static GRID: AtomicU64 = AtomicU64::new(0);
/// Terminal rows whose drawing was built again (the rest were kept).
pub static ROWS: AtomicU64 = AtomicU64::new(0);
/// Microseconds spent in the window's own render.
pub static WINDOW_US: AtomicU64 = AtomicU64::new(0);

/// Microseconds from the window's render to the end of its paint: the
/// main thread's work for a frame, before the GPU.
pub static FRAME_US: AtomicU64 = AtomicU64::new(0);
static FRAME_START: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

pub fn frame_start() {
    *FRAME_START.lock().unwrap() = Some(std::time::Instant::now());
}

/// Put last in the window: painted last, it ends the frame's clock.
pub fn frame_end() -> impl gpui::IntoElement {
    gpui::canvas(|_, _, _| {}, |_, _, _, _| {
        if let Some(t) = FRAME_START.lock().unwrap().take() {
            FRAME_US.fetch_add(t.elapsed().as_micros() as u64, Relaxed);
        }
    })
}

/// The mods runtime's totals so far (scripts run, native rebuilds), copied
/// in by the window.
pub static SCRIPT_RENDERS: AtomicU64 = AtomicU64::new(0);
pub static SCRIPT_US: AtomicU64 = AtomicU64::new(0);
pub static MATERIALIZE: AtomicU64 = AtomicU64::new(0);
pub static MATERIALIZE_US: AtomicU64 = AtomicU64::new(0);

pub fn note_mods(m: &herdr_shell::metrics::RuntimeMetrics) {
    SCRIPT_RENDERS.store(m.script_renders(), Relaxed);
    SCRIPT_US.store(m.script_render_time().as_micros() as u64, Relaxed);
    MATERIALIZE.store(m.materializations(), Relaxed);
    MATERIALIZE_US.store(m.materialize_time().as_micros() as u64, Relaxed);
}

/// Adds the time until it's dropped to a counter.
pub struct Timed(&'static AtomicU64, std::time::Instant);

pub fn timed(c: &'static AtomicU64) -> Timed {
    Timed(c, std::time::Instant::now())
}

impl Drop for Timed {
    fn drop(&mut self) {
        self.0.fetch_add(self.1.elapsed().as_micros() as u64, Relaxed);
    }
}

pub fn count(c: &AtomicU64) {
    c.fetch_add(1, Relaxed);
}

pub fn start() {
    let Some(path) = std::env::var_os("HERDR_APP_BENCH") else { return };
    std::thread::spawn(move || {
        use std::io::Write;
        let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
        let mut last = [0u64; 4];
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let now = [SCRIPT_RENDERS.load(Relaxed), SCRIPT_US.load(Relaxed), MATERIALIZE.load(Relaxed), MATERIALIZE_US.load(Relaxed)];
            let d: Vec<u64> = now.iter().zip(last).map(|(n, l)| n.saturating_sub(l)).collect();
            last = now;
            let _ = writeln!(
                f,
                r#"{{"t":{t},"scripts":{},"script_ms":{},"materialize":{},"materialize_ms":{},"window":{},"terminal":{},"grid":{},"rows":{},"window_ms":{},"frame_ms":{}}}"#,
                d[0],
                d[1] as f64 / 1000.0,
                d[2],
                d[3] as f64 / 1000.0,
                WINDOW.swap(0, Relaxed),
                TERMINAL.swap(0, Relaxed),
                GRID.swap(0, Relaxed),
                ROWS.swap(0, Relaxed),
                WINDOW_US.swap(0, Relaxed) as f64 / 1000.0,
                FRAME_US.swap(0, Relaxed) as f64 / 1000.0
            );
        }
    });
}
