//! Asynchronous text files for mods. Writes reject a stale editor snapshot.
use herdr_shell::{HostError, HostValue};
use std::collections::{HashMap, hash_map::Entry};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{LazyLock, Mutex};

pub const LIMIT: u64 = 1024 * 1024;
static WRITES: Mutex<()> = Mutex::new(());
type ReadResult = Result<Option<String>, String>;
type PendingReads = HashMap<(Option<String>, String), Vec<async_channel::Sender<ReadResult>>>;
static READS: LazyLock<Mutex<PendingReads>> = LazyLock::new(Mutex::default);

pub fn read(path: &Path) -> ReadResult {
    let open = || {
        if !std::fs::metadata(path)?.is_file() {
            return Err(std::io::Error::other("Not a regular file"));
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // The path can be replaced with a pipe between metadata and open.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("Not a regular file"));
        }
        Ok(file)
    };
    let file = match open() {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut text = String::new();
    file.take(LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > LIMIT {
        return Err("Text file exceeds 1 MiB".into());
    }
    Ok(Some(text))
}

fn write(path: &Path, text: &str, expected: Option<&str>) -> Result<(), String> {
    let _guard = WRITES.lock().map_err(|e| e.to_string())?;
    if text.len() as u64 > LIMIT {
        return Err("Text file exceeds 1 MiB".into());
    }
    if read(path)?.as_deref() != expected {
        return Err(
            "The file changed outside Herdr. Reload before saving; your draft is still here."
                .into(),
        );
    }
    let parent = path.parent().ok_or("File needs a parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    if expected.is_none() {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        return f.write_all(text.as_bytes()).map_err(|e| e.to_string());
    }
    // Resolve symlinks so replacing the file does not replace the link itself.
    let target = crate::persist::resolve_target(path).map_err(|e| e.to_string())?;
    let temp = target.with_file_name(format!(
        ".herdr-app-write-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        f.set_permissions(
            std::fs::metadata(&target)
                .map_err(|e| e.to_string())?
                .permissions(),
        )
        .map_err(|e| e.to_string())?;
        f.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        if read(&target)?.as_deref() != expected {
            return Err("The file changed while saving. Reload before saving.".into());
        }
        std::fs::rename(&temp, &target).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(temp);
    result
}

/// Reads off the UI thread: the text, or null when absent.
pub fn read_with(
    on: Option<String>,
    path: String,
    read: impl FnOnce(Option<&str>, &str) -> ReadResult + Send + 'static,
) -> async_channel::Receiver<ReadResult> {
    let (tx, rx) = async_channel::bounded(1);
    let key = (on, path);
    match READS.lock().unwrap().entry(key.clone()) {
        Entry::Occupied(mut entry) => entry.get_mut().push(tx),
        Entry::Vacant(entry) => {
            entry.insert(vec![tx]);
            std::thread::spawn(move || {
                let result = read(key.0.as_deref(), &key.1);
                let waiting = READS.lock().unwrap().remove(&key).unwrap_or_default();
                for tx in waiting {
                    let _ = tx.try_send(result.clone());
                }
            });
        }
    }
    rx
}

pub fn write_text(
    path: String,
    text: String,
    expected: Option<String>,
) -> impl std::future::Future<Output = Result<HostValue, HostError>> + Send {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(write(Path::new(&path), &text, expected.as_deref()));
    });
    async move {
        rx.recv()
            .await
            .map_err(|e| HostError::new(e.to_string()))?
            .map_err(HostError::new)?;
        Ok(HostValue::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creates_updates_and_rejects_stale_text() {
        let root = crate::test_tmp().join(format!(
            "herdr-app-files-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join(".brain/todo.md");
        assert_eq!(read(&path).unwrap(), None);
        write(&path, "- [ ] Task\n", None).unwrap();
        assert!(write(&path, "overwrite", None).is_err());
        write(&path, "- [x] Task\n", Some("- [ ] Task\n")).unwrap();
        assert!(write(&path, "stale", Some("- [ ] Task\n")).is_err());
        assert_eq!(read(&path).unwrap().unwrap(), "- [x] Task\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_fifo_is_rejected_without_waiting_for_a_writer() {
        use std::{ffi::CString, os::unix::{ffi::OsStrExt, fs::symlink}, time::Duration};
        let root = crate::test_tmp().join(format!("herdr-app-preview-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let fifo = root.join("pipe");
        let path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let link = root.join("preview");
        symlink(fifo, &link).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || { let _ = tx.send(read(&link)); });
        let result = rx.recv_timeout(Duration::from_secs(2));
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(result.unwrap().unwrap_err(), "Not a regular file");
    }

    #[test]
    fn same_file_reads_share_outstanding_work_only_on_the_same_machine() {
        let (release, blocked) = std::sync::mpsc::channel();
        let path = "duplicate-preview-test".to_string();
        let first = read_with(None, path.clone(), move |_, _| {
            blocked.recv().unwrap();
            Ok(Some("first".into()))
        });
        let second = read_with(None, path.clone(), |_, _| Ok(Some("duplicate".into())));
        let remote = read_with(Some("remote".into()), path.clone(), |_, _| Ok(Some("remote".into())));
        release.send(()).unwrap();
        assert_eq!(first.recv_blocking().unwrap().unwrap().as_deref(), Some("first"));
        assert_eq!(second.recv_blocking().unwrap().unwrap().as_deref(), Some("first"));
        assert_eq!(remote.recv_blocking().unwrap().unwrap().as_deref(), Some("remote"));
        let again = read_with(None, path, |_, _| Ok(Some("new".into())));
        assert_eq!(again.recv_blocking().unwrap().unwrap().as_deref(), Some("new"));
    }
}
