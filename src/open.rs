//! Opening a local folder from the shell, using the core's configured shell.
use std::path::Path;
use std::time::{Duration, Instant};

pub const WAIT: Duration = Duration::from_secs(30);

/// Serialize folder launches until the first process has installed its listener.
pub fn launch_lock(home: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(home).map_err(|e| format!("cannot create Herdr home: {e}"))?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(home.join("open.lock")).map_err(|e| format!("cannot lock Herdr startup: {e}"))?;
    let deadline = Instant::now() + WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => return Err(format!("cannot lock Herdr startup: {error}")),
        }
    }
}

pub struct Pending {
    pub folder: String,
    pub deadline: Instant,
    pub reply: std::sync::mpsc::Sender<serde_json::Value>,
}

pub fn wait_for_reply(answer: std::sync::mpsc::Receiver<serde_json::Value>, timeout: Duration) -> Result<serde_json::Value, String> {
    answer.recv_timeout(timeout).map_err(|error| match error {
        std::sync::mpsc::RecvTimeoutError::Timeout => "Herdr did not confirm opening the folder before the startup timeout".into(),
        std::sync::mpsc::RecvTimeoutError::Disconnected => "Herdr closed before confirming that the folder was opened".into(),
    })
}

pub fn request(folder: &str) -> serde_json::Value {
    let until = (std::time::SystemTime::now() + WAIT).duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    serde_json::json!({ "open": folder, "open_until": until })
}

pub fn deadline(request: &serde_json::Value) -> Instant {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    Instant::now() + Duration::from_millis(request["open_until"].as_u64().unwrap_or(now).saturating_sub(now)).min(WAIT)
}

pub fn folder(path: &str) -> Result<String, String> {
    if path.is_empty() { return Err("a folder path is required".into()); }
    let path = std::path::absolute(Path::new(path)).map_err(|e| format!("cannot resolve folder: {e}"))?;
    if !path.is_dir() { return Err(format!("not a folder: {}", path.display())); }
    path.into_os_string().into_string().map_err(|_| "folder path is not Unicode".into())
}

pub fn ready(available: bool, error: Option<&str>, deadline: Instant) -> Result<bool, String> {
    if Instant::now() >= deadline { return Err("timed out waiting for this machine's layout".into()); }
    if available { return Ok(true); }
    if let Some(error) = error { return Err(error.to_owned()); }
    Ok(false)
}

pub fn report(error: &str, explorer: bool) {
    eprintln!("herdr-app: {error}");
    #[cfg(windows)]
    if explorer {
        let text: Vec<u16> = error.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "Open in Herdr".encode_utf16().chain(Some(0)).collect();
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR); }
    }
    #[cfg(not(windows))]
    let _ = explorer;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_absolute_and_files_and_missing_paths_are_rejected() {
        assert!(Path::new(&folder(".").unwrap()).is_absolute());
        assert!(folder("").is_err());
        assert!(folder("Cargo.toml").is_err());
        assert!(folder("missing-herdr-open-test-folder").is_err());
    }

    #[test]
    fn waits_only_while_loading_and_preserves_errors() {
        assert_eq!(ready(false, None, Instant::now() + WAIT), Ok(false));
        assert_eq!(ready(true, None, Instant::now() + WAIT), Ok(true));
        assert_eq!(ready(false, Some("bad document"), Instant::now() + WAIT), Err("bad document".into()));
        assert!(ready(false, None, Instant::now()).is_err());
        assert!(ready(true, None, deadline(&serde_json::json!({ "open_until": 0 }))).is_err());
        assert_eq!(ready(false, None, deadline(&request("."))), Ok(false));
    }

    #[test]
    fn launch_lock_is_released_with_the_guard() {
        let dir = std::env::temp_dir().join(format!("herdr-open-lock-{}", std::process::id()));
        let guard = launch_lock(&dir).unwrap();
        let other = std::fs::OpenOptions::new().read(true).write(true).open(dir.join("open.lock")).unwrap();
        assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(guard);
        other.try_lock().unwrap();
        drop(other);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cold_replies_preserve_results_and_report_timeouts_and_disconnects() {
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(wait_for_reply(rx, Duration::ZERO).unwrap_err().contains("timeout"));
        drop(tx);
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        assert!(wait_for_reply(rx, Duration::ZERO).unwrap_err().contains("closed"));
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(serde_json::json!({ "error": "bad document" })).unwrap();
        assert_eq!(wait_for_reply(rx, Duration::ZERO).unwrap()["error"], "bad document");
    }

    #[test]
    fn folder_validation_preserves_literal_shell_characters() {
        let dir = std::env::temp_dir().join(format!("herdr-open-{}-тест & 'quoted' $(literal)", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = dir.to_str().unwrap();
        assert_eq!(folder(name).unwrap(), std::path::absolute(&dir).unwrap().to_str().unwrap());
        assert_eq!(request(name)["open"], name);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
