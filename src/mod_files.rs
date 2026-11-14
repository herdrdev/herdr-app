//! Asynchronous text files for mods. Writes reject a stale editor snapshot.
use gpui_shell::{HostError, HostValue};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

pub const LIMIT: u64 = 1024 * 1024;
static WRITES: Mutex<()> = Mutex::new(());

pub fn read(path: &Path) -> Result<Option<String>, String> {
    let file = match std::fs::File::open(path) {
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
    read: impl FnOnce() -> Result<Option<String>, String> + Send + 'static,
) -> impl std::future::Future<Output = Result<HostValue, HostError>> + Send {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(read());
    });
    async move {
        let value = rx
            .recv()
            .await
            .map_err(|e| HostError::new(e.to_string()))?
            .map_err(HostError::new)?;
        Ok(value.map(HostValue::from).unwrap_or(HostValue::Null))
    }
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
}
