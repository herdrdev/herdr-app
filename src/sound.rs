//! Attention sounds, as Herdr plays them: one when an agent needs you, one
//! when it finished while you weren't looking. Bundled sounds (Herdr's), or a
//! file from settings.json; played with whichever MP3-capable player exists
//! (Windows plays them itself).

use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Stdio;
#[cfg(not(windows))]
use std::time::{Duration, Instant};

const DONE: &[u8] = include_bytes!("../assets/sounds/done.mp3");
const BLOCKED: &[u8] = include_bytes!("../assets/sounds/request.mp3");

/// Players that decode MP3, tried in order (never bare `aplay`: it plays MP3 bytes as noise).
#[cfg(not(windows))]
const PLAYERS: &[(&str, &[&str])] = &[
    #[cfg(target_os = "macos")]
    ("afplay", &[]),
    ("paplay", &[]),
    ("pw-play", &[]),
    ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
    ("mpg123", &["-q"]),
    ("mpv", &["--no-video", "--really-quiet"]),
];
#[cfg(not(windows))]
const PLAYER_LIMIT: Duration = Duration::from_secs(15);

/// `done`, `blocked`, or a file (absolute, or relative to ~/.config/herdr-app). Off the UI thread.
pub fn play(sound: &str) {
    if std::env::var_os("HERDR_APP_NO_SOUND").is_some() {
        return;
    }
    let sound = sound.to_owned();
    let attention = matches!(sound.as_str(), "done" | "blocked");
    // Settings are local to the UI thread; resolve them before handing playback off.
    let chosen = if attention { custom(&sound) } else { Some(resolve(&sound)) };
    std::thread::spawn(move || {
        if let Some(path) = chosen {
            match play_file(&path) {
                Ok(()) => return,
                Err(e) => tracing::warn!("sound {}: {e}", path.display()),
            }
        }
        // A custom attention sound that can't play (moved, other OS) falls back to the bundled one.
        if attention && let Err(e) = bundled(&sound).and_then(|p| play_file(&p)) {
            tracing::warn!("sound {sound}: {e}");
        }
    });
}

fn resolve(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() { p } else { crate::mods::app_dir().join(p) }
}

/// The file settings.json names for `done` / `blocked`, if any.
fn custom(sound: &str) -> Option<PathBuf> {
    let s = crate::settings::get();
    let path = if sound == "done" { &s.sounds.done } else { &s.sounds.blocked };
    path.as_deref().filter(|p| !p.is_empty()).map(resolve)
}

/// The bundled sound, written once to a temp file players can open.
fn bundled(sound: &str) -> Result<PathBuf, String> {
    let (name, bytes) = if sound == "done" { ("done", DONE) } else { ("blocked", BLOCKED) };
    let path = std::env::temp_dir().join(format!("herdr-app-{}-{name}.mp3", std::process::id()));
    if !path.exists() {
        std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(path)
}

/// Windows plays MP3 itself (MCI): no player to find.
#[cfg(windows)]
fn play_file(path: &Path) -> Result<(), String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::Win32::Media::Multimedia::mciSendStringW;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    // Its own alias, so two sounds can overlap.
    let alias = format!("herdr{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let mci = |cmd: String| {
        let wide: Vec<u16> = cmd.encode_utf16().chain([0]).collect();
        match unsafe { mciSendStringW(wide.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut()) } {
            0 => Ok(()),
            code => Err(format!("MCI error {code}")),
        }
    };
    mci(format!("open \"{}\" type mpegvideo alias {alias}", path.display()))?;
    let played = mci(format!("play {alias} wait"));
    let _ = mci(format!("close {alias}"));
    played
}

#[cfg(not(windows))]
fn play_file(path: &Path) -> Result<(), String> {
    let mut errors = Vec::new();
    for (program, args) in PLAYERS {
        match run(program, args, path) {
            Ok(()) => return Ok(()),
            Err(e) => errors.push(format!("{program}: {e}")),
        }
    }
    Err(format!("no MP3 player worked ({})", errors.join("; ")))
}

#[cfg(not(windows))]
fn run(program: &str, args: &[&str], path: &Path) -> Result<(), String> {
    let mut child = crate::spawn::command(program)
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("exited with {status}")),
            Ok(None) if start.elapsed() > PLAYER_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("took too long".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(e.to_string()),
        }
    }
}
