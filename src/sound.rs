//! Attention sounds, as Herdr plays them: one when an agent needs you, one
//! when it finished while you weren't looking. Bundled sounds (Herdr's), or a
//! file from settings.json; played with whichever MP3-capable player exists
//! (Windows and macOS play them themselves).

use std::path::{Path, PathBuf};
#[cfg(not(any(windows, target_os = "macos")))]
use std::process::Stdio;
#[cfg(not(windows))]
use std::time::{Duration, Instant};

const DONE: &[u8] = include_bytes!("../assets/sounds/done.mp3");
const BLOCKED: &[u8] = include_bytes!("../assets/sounds/request.mp3");

/// Players that decode MP3, tried in order (never bare `aplay`: it plays MP3 bytes as noise).
#[cfg(not(any(windows, target_os = "macos")))]
const PLAYERS: &[(&str, &[&str])] = &[
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

/// macOS plays the file itself (AVAudioPlayer), so the app stays one Core
/// Audio client. An `afplay` per sound made a new client each time, and
/// coreaudiod kept a power assertion and IO state for every player that
/// exited: under bursts it climbed to several cores and gigabytes and all
/// audio on the Mac crackled until it was restarted (herdrdev/herdr#4595).
/// AVAudioPlayer rather than NSSound: NSSound's isPlaying never clears on a
/// thread without a run loop, like this one.
#[cfg(target_os = "macos")]
fn play_file(path: &Path) -> Result<(), String> {
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;
    use objc2_foundation::{NSString, NSURL};

    // AVAudioPlayer's framework. For the app's macOS 11 minimum the linker
    // records AVFoundation, which held it before 11.3 and re-exports it since.
    #[link(name = "AVFAudio", kind = "framework")]
    unsafe extern "C" {}

    let class = AnyClass::get(c"AVAudioPlayer").ok_or("AVAudioPlayer is missing")?;
    autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        // SAFETY: AVAudioPlayer's -initWithContentsOfURL:error: takes an
        // NSURL and an optional NSError out-pointer, and returns nil when
        // the file can't be read or decoded.
        let no_error = std::ptr::null_mut::<*mut AnyObject>();
        let player: Option<Retained<AnyObject>> =
            unsafe { msg_send![msg_send![class, alloc], initWithContentsOfURL: &*url, error: no_error] };
        let player = player.ok_or("cannot open it")?;
        // SAFETY: -play, -isPlaying and -stop take no arguments; the first
        // two return BOOL. AVAudioPlayer can be used from any thread.
        let started: bool = unsafe { msg_send![&*player, play] };
        if !started {
            return Err("cannot play it".into());
        }
        let start = Instant::now();
        while unsafe { msg_send![&*player, isPlaying] } {
            if start.elapsed() > PLAYER_LIMIT {
                let _: () = unsafe { msg_send![&*player, stop] };
                return Err("took too long".into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Ok(())
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
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

#[cfg(not(any(windows, target_os = "macos")))]
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// A mono 16-bit WAV of `ms` milliseconds of silence.
    fn silent_wav(ms: u32) -> Vec<u8> {
        let rate = 8000u32;
        let data = rate * ms / 1000 * 2;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        wav.resize(wav.len() + data as usize, 0);
        wav
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("herdr-sound-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    #[ignore = "needs an audio output device (CI's macOS runners have none); run with -- --ignored"]
    fn plays_a_file_in_process_and_returns_when_it_ends() {
        let dir = scratch("silence");
        let path = dir.join("silence.wav");
        std::fs::write(&path, silent_wav(200)).unwrap();
        let start = Instant::now();
        let played = play_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
        played.unwrap();
        // Off the main thread there's no run loop; playback must still end on its own.
        assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
    }

    #[test]
    fn a_file_that_is_not_audio_is_an_error() {
        let dir = scratch("not-audio");
        let path = dir.join("not-audio.mp3");
        std::fs::write(&path, b"not audio").unwrap();
        let garbage = play_file(&path);
        let missing = play_file(&dir.join("missing.mp3"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(garbage.is_err());
        assert!(missing.is_err());
    }
}
