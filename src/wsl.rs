//! Running "This computer" inside a WSL distribution on Windows.
//!
//! When `wsl` is set in `settings.json` on Windows, `herdr-core` (`hc`), local
//! terminals, and local mod commands run inside that distribution via `wsl.exe`
//! while `herdr-app` itself and `~/.config/herdr-app` stay on the Windows host.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static DISTRO: OnceLock<Option<String>> = OnceLock::new();
static ENV: Mutex<EnvState> = Mutex::new(EnvState {
    value: None,
    failure: None,
});
const WSL_TIMEOUT: Duration = Duration::from_secs(30);
const ENV_BACKOFF: Duration = Duration::from_secs(5);
const TIMEOUT_ERROR: &str = "WSL did not respond; is the distro starting? Try again or check that the selected distribution starts.";

#[derive(Default)]
struct EnvState {
    value: Option<Env>,
    failure: Option<(Instant, String)>,
}

impl EnvState {
    fn get(
        &mut self,
        now: Instant,
        probe: impl FnOnce() -> Result<Env, String>,
    ) -> Result<Env, String> {
        if let Some(value) = &self.value {
            return Ok(value.clone());
        }
        if let Some((at, error)) = &self.failure
            && now.saturating_duration_since(*at) < ENV_BACKOFF
        {
            return Err(error.clone());
        }
        match probe() {
            Ok(value) => {
                self.failure = None;
                self.value = Some(value.clone());
                Ok(value)
            }
            Err(error) => {
                self.failure = Some((Instant::now(), error.clone()));
                Err(error)
            }
        }
    }
}
static EXTRA_PATH: Mutex<Option<String>> = Mutex::new(None);
const PATH_CACHE_CAP: usize = 512;
type PathKey = (String, String, String);
type PathCache = Mutex<VecDeque<(PathKey, String)>>;
static PATH_CACHE: PathCache = Mutex::new(VecDeque::new());

fn cached_path(
    cache: &PathCache,
    distro: &str,
    direction: &str,
    path: &str,
    convert: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    let key = (distro.to_owned(), direction.to_owned(), path.to_owned());
    if let Some((_, value)) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(k, _)| *k == key)
    {
        return Ok(value.clone());
    }
    // Do not hold the lock while WSL starts; unrelated conversions can proceed.
    let value = convert()?;
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if !cache.iter().any(|(k, _)| *k == key) {
        if cache.len() >= PATH_CACHE_CAP {
            cache.pop_front();
        }
        cache.push_back((key, value.clone()));
    }
    Ok(value)
}

#[derive(Clone)]
struct Env {
    home: PathBuf,
    shell: String,
    path: String,
}

/// Sets the WSL distribution this process uses for "This computer" (Windows only).
pub fn init(distro: Option<String>) {
    let active = if cfg!(windows) {
        distro
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty())
    } else {
        let _ = distro;
        None
    };
    let _ = DISTRO.set(active);
}

/// The active WSL distribution name, when running on Windows with `wsl` configured.
pub fn active() -> Option<&'static str> {
    DISTRO.get().and_then(Option::as_deref)
}

/// Prepends `dir` to the WSL `PATH` used by [`command`].
pub fn prepend_path(dir: &Path) {
    if active().is_none() {
        return;
    }
    let dir = dir.to_string_lossy().into_owned();
    if !dir.is_empty() {
        *EXTRA_PATH.lock().unwrap() = Some(dir);
    }
}

fn environment() -> Result<Env, String> {
    let distro = active().ok_or("WSL is not active")?;
    ENV.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(Instant::now(), || query_env(distro))
}

fn env() -> Option<Env> {
    environment().ok()
}

fn query_env(distro: &str) -> Result<Env, String> {
    let script = r#"s=${SHELL:-/bin/sh}; printf '%s\n%s\n' "$HOME" "$s"; "$s" -lic 'printf "\nherdr-path:%s\n" "$PATH"' 2>/dev/null || "$s" -lc 'printf "\nherdr-path:%s\n" "$PATH"' 2>/dev/null || printf '\nherdr-path:%s\n' "$PATH""#;
    let mut command = host_command();
    command.args(["-d", distro, "--cd", "~", "-e", "sh", "-c", script]);
    let out = run_wsl_timed(command, WSL_TIMEOUT)?;
    require_success(&out, &format!("Cannot reach WSL ({distro})"))?;
    let text = decode_output(&out.stdout);
    let mut lines = text.lines();
    let home = lines.next().filter(|s| s.starts_with('/')).ok_or_else(|| {
        format!(
            "Cannot read the environment in WSL ({distro}); check that the distribution starts."
        )
    })?;
    let shell = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("/bin/sh")
        .to_owned();
    let login_path = lines
        .filter_map(|line| line.strip_prefix("herdr-path:"))
        .last()
        .unwrap_or("/usr/local/bin:/usr/bin:/bin");
    let path = merge_linux_path(home, login_path);
    Ok(Env {
        home: PathBuf::from(home),
        shell,
        path,
    })
}

// A regular file avoids filling a pipe before the shell exits, without leaving
// a reader thread blocked when a shell descendant inherits stdout.
pub fn run_wsl_timed(mut command: Command, timeout: Duration) -> Result<Output, String> {
    use std::io::{Read, Seek, SeekFrom};
    const LIMIT: u64 = 1024 * 1024;
    struct OutputFile {
        file: Option<std::fs::File>,
        path: PathBuf,
    }
    impl Drop for OutputFile {
        fn drop(&mut self) {
            drop(self.file.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }
    let io_error = |e: std::io::Error| format!("Cannot run WSL: {e}");
    let capture = |stream| -> Result<OutputFile, String> {
        let path = std::env::temp_dir().join(format!(
            "herdr-wsl-env-{}-{}-{stream}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(7);
        } // Allow cleanup even if a descendant inherits an output handle.
        let file = options.open(&path).map_err(io_error)?;
        Ok(OutputFile {
            file: Some(file),
            path,
        })
    };
    let mut stdout = capture("out")?;
    let mut stderr = capture("err")?;
    command
        .stdin(Stdio::null())
        .stdout(
            stdout
                .file
                .as_ref()
                .unwrap()
                .try_clone()
                .map_err(io_error)?,
        )
        .stderr(
            stderr
                .file
                .as_ref()
                .unwrap()
                .try_clone()
                .map_err(io_error)?,
        );
    let mut child = command.spawn().map_err(io_error)?;
    drop(command);
    let deadline = Instant::now() + timeout;
    let status = loop {
        let mut check = || -> Result<Option<std::process::ExitStatus>, String> {
            for output in [&stdout, &stderr] {
                if output
                    .file
                    .as_ref()
                    .unwrap()
                    .metadata()
                    .map_err(io_error)?
                    .len()
                    > LIMIT
                {
                    return Err("WSL output exceeds 1 MiB".into());
                }
            }
            if Instant::now() >= deadline {
                return Err(TIMEOUT_ERROR.into());
            }
            child.try_wait().map_err(io_error)
        };
        match check() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    };
    let read = |output: &mut OutputFile| -> Result<Vec<u8>, String> {
        let file = output.file.as_mut().unwrap();
        file.seek(SeekFrom::Start(0)).map_err(io_error)?;
        let mut bytes = Vec::new();
        file.take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() as u64 > LIMIT {
            return Err("WSL output exceeds 1 MiB".into());
        }
        Ok(bytes)
    };
    Ok(Output {
        status,
        stdout: read(&mut stdout)?,
        stderr: read(&mut stderr)?,
    })
}

fn require_success(out: &Output, context: &str) -> Result<(), String> {
    if out.status.success() {
        return Ok(());
    }
    let detail = decode_output(&out.stderr);
    Err(format!(
        "{context}: {}",
        if detail.trim().is_empty() {
            "WSL command failed"
        } else {
            detail.trim()
        }
    ))
}

fn merge_linux_path(home: &str, login_path: &str) -> String {
    let home = home.trim_end_matches('/');
    let usual = [
        format!("{home}/.local/bin"),
        "/usr/local/bin".into(),
        "/home/linuxbrew/.linuxbrew/bin".into(),
        format!("{home}/.local/share/mise/shims"),
        format!("{home}/.nix-profile/bin"),
        "/run/current-system/sw/bin".into(),
        format!("{home}/.cargo/bin"),
        "/usr/bin".into(),
        "/bin".into(),
    ];
    let mut dirs: Vec<String> = login_path
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    for u in usual {
        if !dirs.contains(&u) {
            dirs.push(u);
        }
    }
    dirs.join(":")
}

/// The person's `$HOME` inside the active WSL distribution.
pub fn home() -> Option<PathBuf> {
    env().map(|e| e.home)
}

/// The person's login `$SHELL` inside the active WSL distribution.
pub fn shell() -> Option<String> {
    env().map(|e| e.shell)
}

/// A `wsl.exe` command on the Windows host with UTF-8 output enabled.
pub fn host_command() -> Command {
    let mut c = crate::spawn::command("wsl.exe");
    c.env("WSL_UTF8", "1");
    c
}

/// Builds a command that runs `program` inside the active WSL distribution.
pub fn command(program: impl AsRef<OsStr>, cwd: Option<&str>) -> Command {
    let distro = active().unwrap_or_default();
    let dir = match cwd.filter(|d| !d.is_empty()) {
        None | Some("~") => {
            env().map_or_else(|| "~".into(), |e| e.home.to_string_lossy().into_owned())
        }
        Some(d) if d.starts_with("~/") => match env() {
            Some(e) => format!(
                "{}/{}",
                e.home.to_string_lossy().trim_end_matches('/'),
                &d[2..]
            ),
            None => d.to_owned(),
        },
        // --cd accepts Windows paths itself, using this distro's actual mounts.
        Some(d) => guest_path(Path::new(d), distro).unwrap_or_else(|| d.to_owned()),
    };
    let mut c = host_command();
    if !distro.is_empty() {
        c.args(["-d", distro]);
    }
    c.args(["--cd", &dir, "-e"]);
    if let Some(e) = env() {
        let path = match EXTRA_PATH.lock().unwrap().as_deref() {
            Some(extra) => path_with_core_first(&e.path, extra),
            None => e.path.clone(),
        };
        c.args(["env", &format!("PATH={path}")]);
    }
    c.arg(program);
    c
}

fn path_with_core_first(path: &str, dir: &str) -> String {
    std::iter::once(dir)
        .chain(path.split(':').filter(|p| *p != dir))
        .collect::<Vec<_>>()
        .join(":")
}

const FIND_CORE_SCRIPT: &str = include_str!("../defaults/frame/settings/find-core.sh");

/// Finds `herdr-core` (`hc`) inside the active WSL distribution.
pub fn find_core(configured: Option<&str>) -> Result<Option<PathBuf>, String> {
    environment()?;
    let mut cmd = command("sh", None);
    cmd.args(["-c", FIND_CORE_SCRIPT, "sh", configured.unwrap_or("")]);
    let out = run_wsl_timed(cmd, WSL_TIMEOUT)?;
    core_probe_result(&out)
}

fn core_probe_result(out: &Output) -> Result<Option<PathBuf>, String> {
    if out.status.code() == Some(3) {
        return Ok(None);
    }
    require_success(out, "Cannot reach the selected WSL distribution")?;
    let text = decode_output(&out.stdout);
    let found = text
        .lines()
        .next()
        .filter(|s| !s.is_empty())
        .ok_or("WSL returned an empty herdr-core probe; check the selected distribution.")?;
    Ok(Some(PathBuf::from(found)))
}

/// Checks a guest binary without allowing an unbounded WSL version probe.
pub fn version(bin: &Path) -> Result<Option<String>, String> {
    if bin
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Ok(None);
    }
    environment()?;
    let mut cmd = command(bin.as_os_str(), None);
    cmd.arg("--version");
    let out = run_wsl_timed(cmd, WSL_TIMEOUT)?;
    require_success(&out, "Cannot check herdr-core in WSL")?;
    let text = decode_output(&out.stdout).trim().to_owned();
    Ok(text.starts_with("hc ").then_some(text))
}

/// Installs the Linux `herdr-core` release inside the active WSL distribution.
pub fn install_core(releases: &str) -> Result<PathBuf, String> {
    environment()?;
    let mut cmd = command("sh", None);
    cmd.args([
        "-c",
        "script=$(curl -fsSL -- \"$1/core/install.sh\") && printf '%s\\n' \"$script\" | sh",
        "sh",
        releases,
    ]);
    let out = run_wsl_timed(cmd, WSL_TIMEOUT)?;
    if !out.status.success() {
        let err = decode_output(&out.stderr).trim().to_owned();
        return Err(if err.is_empty() {
            "the installer failed in WSL".into()
        } else {
            err
        });
    }
    find_core(None)?.ok_or_else(|| "installed in WSL, but herdr-core doesn't run".into())
}

/// Decodes output from `wsl.exe`, handling both UTF-8 and UTF-16LE.
pub fn decode_output(bytes: &[u8]) -> String {
    let utf16_bom = bytes.starts_with(&[0xFF, 0xFE]);
    let bytes = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes);
    let text = if utf16_bom
        || (bytes.len() >= 2
            && bytes.len() % 2 == 0
            && bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count() * 2 >= bytes.len() / 2)
    {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).replace('\0', "")
    };
    text.trim_start_matches('\u{feff}').replace("\r\n", "\n")
}

/// Parses `wsl.exe --list --quiet` output into distribution names, excluding Docker's internal stores.
#[cfg(test)]
pub fn parse_distros(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for line in decode_output(bytes).lines() {
        let name = line.trim();
        if !name.is_empty()
            && !name.eq_ignore_ascii_case("docker-desktop")
            && !name.eq_ignore_ascii_case("docker-desktop-data")
            && !out.iter().any(|d: &String| d.eq_ignore_ascii_case(name))
        {
            out.push(name.to_owned());
        }
    }
    out
}

/// A path already in this distro's namespace; no process or mount guesses needed.
fn guest_path(path: &Path, distro: &str) -> Option<String> {
    let s = path.to_string_lossy();
    if s.starts_with('/') && !s.starts_with("//") {
        return Some(s.into_owned());
    }
    let normalized = s.replace('\\', "/");
    // File dialogs and canonicalize may return Windows verbatim paths.
    let normalized = if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else {
        normalized
            .strip_prefix("//?/")
            .unwrap_or(&normalized)
            .to_owned()
    };
    for prefix in ["//wsl.localhost/", "//wsl$/"] {
        if normalized
            .get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
        {
            let rest = &normalized[prefix.len()..];
            let (d, tail) = rest.split_once('/').unwrap_or((rest, ""));
            if d.eq_ignore_ascii_case(distro) {
                let tail = tail.trim_start_matches('/');
                return Some(if tail.is_empty() {
                    "/".into()
                } else {
                    format!("/{tail}")
                });
            }
        }
    }
    None
}

fn linux_path_with(
    path: &Path,
    distro: &str,
    convert: impl FnOnce(&str) -> Result<String, String>,
) -> Result<String, String> {
    match guest_path(path, distro) {
        Some(path) => Ok(path),
        None => convert(&path.to_string_lossy()),
    }
}

/// Converts a host path using the selected distro's actual automount configuration.
pub fn linux_path(path: &Path, distro: &str) -> Result<String, String> {
    linux_path_with(path, distro, |path| convert_path(path, distro, "-u"))
}

fn convert_path(path: &str, distro: &str, direction: &str) -> Result<String, String> {
    cached_path(&PATH_CACHE, distro, direction, path, || {
        convert_path_uncached(path, distro, direction)
    })
}

fn convert_path_uncached(path: &str, distro: &str, direction: &str) -> Result<String, String> {
    let mut cmd = host_command();
    cmd.args(["-d", distro, "-e", "wslpath", "-a", direction, path]);
    let out = run_wsl_timed(cmd, WSL_TIMEOUT)?;
    if !out.status.success() {
        return Err(format!(
            "Cannot convert {path} for WSL: {}",
            decode_output(&out.stderr).trim()
        ));
    }
    converted_path(&out.stdout, path, direction)
}

fn converted_path(bytes: &[u8], path: &str, direction: &str) -> Result<String, String> {
    let text = decode_output(bytes);
    let text = text.strip_suffix('\n').unwrap_or(&text);
    if text.is_empty() || (direction == "-u" && !text.starts_with('/')) {
        return Err(format!("WSL did not return an absolute path for {path}"));
    }
    Ok(text.to_owned())
}

fn host_path_with(
    path: &Path,
    convert: impl FnOnce(&str) -> Result<String, String>,
) -> Result<PathBuf, String> {
    let s = path.to_string_lossy();
    if !s.starts_with('/') || s.starts_with("//") {
        return Ok(path.to_path_buf());
    }
    // Windows normalizes these before the WSL server sees them. In particular,
    // `link/..` must not lose Linux's symlink and mount-boundary semantics, and
    // `note ` or `note.` must not alias the separate guest file `note`.
    if s.contains('\\')
        || s.split('/').any(|part| {
            part == ".." || (part != "." && (part.ends_with(' ') || part.ends_with('.')))
        })
    {
        return Err("Cannot access a WSL path containing a literal backslash, '..', or a name ending in a space or period through Windows. Use a guest command instead.".into());
    }
    convert(&s).map(PathBuf::from)
}

/// Uses actual mounts: UNC for guest files, host drives for DrvFs files (which
/// cannot be opened through WSL's UNC share on Windows).
pub fn host_path(path: &Path, distro: &str) -> Result<PathBuf, String> {
    host_path_with(path, |path| convert_path(path, distro, "-w"))
}

pub fn host_path_if_active(path: &Path) -> Result<PathBuf, String> {
    active().map_or_else(|| Ok(path.to_path_buf()), |d| host_path(path, d))
}

pub fn linux_path_if_active(path: &Path) -> Result<String, String> {
    active().map_or_else(
        || Ok(path.to_string_lossy().into_owned()),
        |d| linux_path(path, d),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converted_paths_decode_utf16le_and_require_absolute_guest_paths() {
        let bytes: Vec<u8> = "\u{feff}/home/日本語/file\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            converted_path(&bytes, "fixture", "-u").unwrap(),
            "/home/日本語/file"
        );
        assert!(converted_path(b"relative\n", "fixture", "-u").is_err());
        assert!(converted_path(b"\n", "fixture", "-w").is_err());
    }

    #[test]
    fn path_cache_hits_are_scoped_to_distro_direction_and_literal_path() {
        let cache = Mutex::new(VecDeque::new());
        assert_eq!(
            cached_path(&cache, "Ubuntu", "-w", "/home/a b", || Ok("UNC".into())).unwrap(),
            "UNC"
        );
        assert_eq!(
            cached_path(&cache, "Ubuntu", "-w", "/home/a b", || panic!(
                "cache hit must not spawn"
            ))
            .unwrap(),
            "UNC"
        );
        for (distro, direction, path) in [
            ("Debian", "-w", "/home/a b"),
            ("Ubuntu", "-u", "/home/a b"),
            ("Ubuntu", "-w", "/home/A b"),
        ] {
            assert_eq!(
                cached_path(&cache, distro, direction, path, || Ok("other".into())).unwrap(),
                "other"
            );
        }
        assert_eq!(cache.lock().unwrap().len(), 4);
    }

    #[test]
    fn path_cache_evicts_oldest_and_retries_failed_conversions() {
        let cache = Mutex::new(VecDeque::new());
        assert!(cached_path(&cache, "Ubuntu", "-u", "bad", || Err("starting".into())).is_err());
        assert!(cache.lock().unwrap().is_empty());
        assert_eq!(
            cached_path(&cache, "Ubuntu", "-u", "bad", || Ok("recovered".into())).unwrap(),
            "recovered"
        );
        for n in 0..PATH_CACHE_CAP {
            cached_path(&cache, "Ubuntu", "-u", &n.to_string(), || {
                Ok(format!("/{n}"))
            })
            .unwrap();
        }
        assert_eq!(cache.lock().unwrap().len(), PATH_CACHE_CAP);
        assert_eq!(
            cached_path(&cache, "Ubuntu", "-u", "bad", || Ok("new probe".into())).unwrap(),
            "new probe"
        );
        assert_eq!(cache.lock().unwrap().len(), PATH_CACHE_CAP);
        let recent = (PATH_CACHE_CAP - 1).to_string();
        assert_eq!(
            cached_path(&cache, "Ubuntu", "-u", &recent, || panic!(
                "recent entry must remain"
            ))
            .unwrap(),
            format!("/{recent}")
        );
    }

    #[test]
    fn environment_retries_after_backoff_and_caches_only_success() {
        let mut state = EnvState::default();
        assert!(
            state
                .get(Instant::now(), || Err("cold boot".into()))
                .is_err()
        );
        let at = state.failure.as_ref().unwrap().0;
        assert!(
            state
                .get(at + ENV_BACKOFF / 2, || panic!(
                    "backoff must suppress probes"
                ))
                .is_err()
        );
        assert!(state.value.is_none());
        let value = state
            .get(at + ENV_BACKOFF, || {
                Ok(Env {
                    home: "/home/test".into(),
                    shell: "/bin/bash".into(),
                    path: "/usr/bin".into(),
                })
            })
            .unwrap();
        assert_eq!(value.home, Path::new("/home/test"));
        assert!(state.failure.is_none());
        assert_eq!(
            state
                .get(at + ENV_BACKOFF, || panic!("success must stay cached"))
                .unwrap()
                .shell,
            "/bin/bash"
        );
    }

    #[cfg(unix)]
    #[test]
    fn core_probe_distinguishes_missing_from_unreachable() {
        use std::os::unix::process::ExitStatusExt;
        let output = |code, stdout: &[u8]| Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: b"distribution unavailable".to_vec(),
        };
        assert_eq!(core_probe_result(&output(3, b"")), Ok(None));
        assert!(
            core_probe_result(&output(1, b""))
                .unwrap_err()
                .contains("distribution unavailable")
        );
        assert_eq!(
            core_probe_result(&output(0, b"/home/test/hc\n")),
            Ok(Some("/home/test/hc".into()))
        );
        assert!(core_probe_result(&output(0, b"")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn environment_capture_handles_chatty_shell_timeout_and_output_limit() {
        let mut chatty = Command::new("sh");
        chatty.args(["-c", "head -c 262144 /dev/zero; printf guest-env"]);
        let before: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("herdr-wsl-env-{}-", std::process::id()))
            })
            .map(|e| e.path())
            .collect();
        let bytes = run_wsl_timed(chatty, Duration::from_secs(2))
            .unwrap()
            .stdout;
        assert_eq!(bytes.len(), 262144 + 9);
        assert!(bytes.ends_with(b"guest-env"));
        let mut oversized = Command::new("sh");
        oversized.args(["-c", "head -c 2097152 /dev/zero"]);
        assert!(
            run_wsl_timed(oversized, Duration::from_secs(2))
                .unwrap_err()
                .contains("output exceeds")
        );
        let mut slow = Command::new("sh");
        slow.args(["-c", "exec sleep 2"]);
        let start = std::time::Instant::now();
        assert_eq!(
            run_wsl_timed(slow, Duration::from_millis(50)).unwrap_err(),
            TIMEOUT_ERROR
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        let after: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&format!("herdr-wsl-env-{}-", std::process::id()))
            })
            .map(|e| e.path())
            .collect();
        assert_eq!(before, after, "capture files must be removed on all paths");
    }

    #[test]
    fn parses_utf8_and_utf16le_distro_lists_and_skips_docker() {
        let utf8 = b"Ubuntu\r\ndocker-desktop\r\nDebian\r\ndocker-desktop-data\r\n\r\n";
        assert_eq!(parse_distros(utf8), vec!["Ubuntu", "Debian"]);

        let mut utf16 = vec![0xFF, 0xFE];
        for u in "Ubuntu-24.04\r\ndocker-desktop\r\nArch\r\n".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(parse_distros(&utf16), vec!["Ubuntu-24.04", "Arch"]);
    }

    #[test]
    fn preserves_literal_linux_paths_and_unicode_output() {
        assert_eq!(
            linux_path(Path::new("/home/alice/O'Brien \\src "), "Ubuntu").unwrap(),
            "/home/alice/O'Brien \\src "
        );
        assert_eq!(
            linux_path_with(
                Path::new(r"C:\Users\Alice Smith\project "),
                "Ubuntu",
                |path| {
                    assert_eq!(path, r"C:\Users\Alice Smith\project ");
                    Ok("/drives/c/Users/Alice Smith/project ".into())
                }
            )
            .unwrap(),
            "/drives/c/Users/Alice Smith/project "
        );
        let bytes: Vec<_> = [0xfeff]
            .into_iter()
            .chain("中文".encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_output(&bytes), "中文");
        assert_eq!(
            parse_distros(b"Ubuntu\nubuntu\nDOCKER-DESKTOP\nDebian\n"),
            vec!["Ubuntu", "Debian"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn core_probe_preserves_spaces_quotes_and_globs() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("herdr-wsl-probe-{}", std::process::id()));
        let home = root.join("home O'Brien [literal]");
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let core = bin.join("hc");
        std::fs::write(&core, "#!/bin/sh\nprintf 'hc 0.1.0\\n'\n").unwrap();
        std::fs::set_permissions(&core, std::fs::Permissions::from_mode(0o755)).unwrap();
        let probe = |configured: &str, shell: &Path| {
            Command::new("/bin/sh")
                .args(["-c", FIND_CORE_SCRIPT, "sh", configured])
                .env("HOME", &home)
                .env("SHELL", shell)
                .env("PATH", "/nonexistent")
                .env("PROBE_CORE", &core)
                .output()
                .unwrap()
        };
        for configured in [core.to_str().unwrap(), "~/.local/bin/hc", ""] {
            let out = probe(configured, Path::new("/bin/false"));
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                String::from_utf8(out.stdout)
                    .unwrap()
                    .trim_end_matches('\n'),
                core.to_str().unwrap()
            );
        }
        // A login shell may itself live at a path containing spaces.
        let login = root.join("login shell");
        std::fs::write(&login, "#!/bin/sh\nprintf '%s\\n' \"$PROBE_CORE\"\n").unwrap();
        std::fs::set_permissions(&login, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = probe("", &login);
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout)
                .unwrap()
                .trim_end_matches('\n'),
            core.to_str().unwrap()
        );
        // A configured executable that isn't hc must not prevent usual-dir discovery.
        let fake = root.join("not hc");
        std::fs::write(&fake, "#!/bin/sh\necho other\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            probe(fake.to_str().unwrap(), Path::new("/bin/false"))
                .status
                .success()
        );
        // A Windows core left in settings must be skipped, without even running it.
        let windows = root.join("hc.ExE");
        let marker = root.join("host-was-run");
        std::fs::write(
            &windows,
            format!(
                "#!/bin/sh\nprintf ran > '{}'\necho 'hc 0.1.0'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&windows, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = probe(windows.to_str().unwrap(), Path::new("/bin/false"));
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout)
                .unwrap()
                .trim_end_matches('\n'),
            core.to_str().unwrap()
        );
        assert!(!marker.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_core_is_first_even_when_already_in_path() {
        assert_eq!(
            path_with_core_first(
                "/usr/local/bin:/opt/chosen/bin:/usr/bin:/opt/chosen/bin",
                "/opt/chosen/bin"
            ),
            "/opt/chosen/bin:/usr/local/bin:/usr/bin"
        );
    }

    #[cfg(unix)]
    #[test]
    fn selected_core_beats_an_earlier_core_in_shell() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("herdr-wsl-path-{}", std::process::id()));
        let first = root.join("first");
        let chosen = root.join("chosen");
        for (dir, label) in [(&first, "wrong"), (&chosen, "chosen")] {
            std::fs::create_dir_all(dir).unwrap();
            let hc = dir.join("hc");
            std::fs::write(&hc, format!("#!/bin/sh\nprintf '{label}\\n'\n")).unwrap();
            std::fs::set_permissions(hc, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!("{}:{}", first.display(), chosen.display());
        let out = Command::new("/bin/sh")
            .args(["-c", "hc"])
            .env(
                "PATH",
                path_with_core_first(&path, chosen.to_str().unwrap()),
            )
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"chosen\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn host_conversion_uses_distro_result_and_propagates_failure() {
        let host = Path::new(r"C:\Users\alice");
        assert_eq!(
            linux_path_with(host, "CustomMount", |path| {
                assert_eq!(path, host.to_string_lossy());
                Ok("/drives/c/Users/alice".into())
            })
            .unwrap(),
            "/drives/c/Users/alice"
        );
        assert!(linux_path_with(host, "NoMount", |_| Err("drive not mounted".into())).is_err());
        assert_eq!(
            linux_path_with(Path::new("/mnt/c/note"), "NoMount", |_| panic!(
                "guest path must not be converted"
            ))
            .unwrap(),
            "/mnt/c/note"
        );
    }

    #[test]
    fn rejects_guest_names_that_windows_would_alias_before_conversion() {
        for path in [
            r"/home/alice/back\slash.txt",
            "/mnt/c/../c/note.txt",
            "/home/alice/note ",
            "/home/alice/note.",
            "/home/alice/dir./note",
        ] {
            assert!(
                host_path_with(Path::new(path), |_| panic!(
                    "unsafe path must not reach Windows"
                ))
                .is_err(),
                "{path}"
            );
        }
        assert_eq!(
            host_path_with(Path::new("/home/alice/./.brain/note.txt"), |path| {
                assert_eq!(path, "/home/alice/./.brain/note.txt");
                Ok(r"\\wsl.localhost\Ubuntu\home\alice\.brain\note.txt".into())
            })
            .unwrap(),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\alice\.brain\note.txt")
        );
    }

    #[test]
    fn converts_between_windows_and_wsl_paths() {
        assert_eq!(
            linux_path(Path::new("/home/alice/src"), "Ubuntu").unwrap(),
            "/home/alice/src"
        );
        assert_eq!(
            linux_path(
                Path::new(r"\\wsl.localhost\Ubuntu\home\alice\repo"),
                "Ubuntu"
            )
            .unwrap(),
            "/home/alice/repo"
        );
        assert_eq!(
            linux_path(Path::new(r"\\wsl$\ubuntu\home\alice"), "Ubuntu").unwrap(),
            "/home/alice"
        );
        assert_eq!(
            linux_path(Path::new(r"\\wsl.localhost\Ubuntu"), "Ubuntu").unwrap(),
            "/"
        );
        assert_eq!(
            linux_path(
                Path::new(r"\\?\UNC\WSL.LOCALHOST\Ubuntu\home\alice"),
                "Ubuntu"
            )
            .unwrap(),
            "/home/alice"
        );

        for (guest, converted) in [
            (
                "/home/alice/repo",
                r"\\wsl.localhost\Ubuntu\home\alice\repo",
            ),
            ("/", r"\\wsl.localhost\Ubuntu\"),
            // With automount disabled, /mnt/c is an ordinary guest directory.
            (
                "/mnt/c/Users/alice",
                r"\\wsl.localhost\Ubuntu\mnt\c\Users\alice",
            ),
            // A custom automount root must use the converter's drive result.
            ("/drives/c/Users/alice", r"C:\Users\alice"),
        ] {
            let result = host_path_with(Path::new(guest), |path| {
                assert_eq!(path, guest);
                Ok(converted.into())
            })
            .unwrap();
            assert_eq!(result, PathBuf::from(converted));
        }
        assert_eq!(
            host_path(Path::new(r"C:\Users\alice"), "Ubuntu").unwrap(),
            PathBuf::from(r"C:\Users\alice")
        );
        assert!(host_path(Path::new("/mnt/c/../c/Users/alice"), "Ubuntu").is_err());
        assert!(host_path(Path::new("/home/alice/link/../note"), "Ubuntu").is_err());
    }
}
