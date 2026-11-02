//! Nearest lexical ancestor containing a marker; remote walks use one exec.
use gpui_shell::{HostError, HostValue};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
};
type Answer = Result<Option<String>, String>;
type Key = (u64, String, String, String);
enum Entry {
    Ready(Option<String>),
    Pending(Vec<async_channel::Sender<Answer>>),
}
#[derive(Default)]
struct Cache {
    epoch: u64,
    entries: HashMap<Key, Entry>,
}
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(Mutex::default)
}

pub fn invalidate() {
    let mut cache = cache().lock().unwrap();
    cache.epoch += 1;
    cache
        .entries
        .retain(|_, entry| matches!(entry, Entry::Pending(_)));
}

fn marker(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.contains('\0')
        || name.contains('\\')
        || name.contains('/')
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("findUp name must be one entry name, not a path".into());
    }
    Ok(())
}

fn lexical(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("findUp folder must be absolute".into());
    }
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}

fn local(name: &str, folder: &str, home: &Path) -> Answer {
    let mut folder = lexical(Path::new(folder))?;
    // Looking up "." requires search permission on the starting folder
    // without changing the app process's current directory.
    let start = std::fs::metadata(folder.join("."))
        .map_err(|error| format!("cannot enter findUp folder {}: {error}", folder.display()))?;
    if !start.is_dir() {
        return Err("findUp folder is not a directory".into());
    }
    let home = lexical(home)?;
    let stop = folder.starts_with(&home).then_some(home);
    loop {
        match std::fs::symlink_metadata(folder.join(name)) {
            Ok(_) => return Ok(Some(folder.to_string_lossy().into_owned())),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(e) => return Err(format!("findUp {}: {e}", folder.display())),
        }
        if stop.as_ref() == Some(&folder) || !folder.pop() {
            return Ok(None);
        }
    }
}

const REMOTE_WALK: &str = r#"p=$1; name=$2
cd -- "$p" 2>/dev/null || { printf 'cannot enter findUp folder: %s' "$p" >&2; exit 2; }
p=${p%/}; [ -n "$p" ] || p=/
stop=/
case "$p/" in "$HOME/"*) stop=$HOME ;; esac
n=0
while :; do
  if [ -e "$p/$name" ] || [ -L "$p/$name" ]; then printf '%s' "$p"; exit 0; fi
  [ "$p" = "$stop" ] && exit 3
  [ "$p" = / ] && exit 3
  p=${p%/*}; [ -n "$p" ] || p=/
  n=$((n+1)); [ "$n" -le 1024 ] || exit 2
done"#;

fn remote(name: &str, folder: &str, machine: &str) -> Answer {
    if !folder.starts_with('/') || folder.contains('\0') {
        return Err("findUp currently supports POSIX remote folders only".into());
    }
    // Positional arguments avoid shell interpolation. Entering the folder
    // proves its lexical ancestors are searchable before testing one name.
    let mut parts = Vec::new();
    for part in folder.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let folder = format!("/{}", parts.join("/"));
    let (code, out, err) = crate::mods::run_remote(
        machine,
        "sh",
        &[
            "-c".into(),
            REMOTE_WALK.into(),
            "herdr-find-up".into(),
            folder.into(),
            name.into(),
        ],
        None,
    )?;
    match code {
        0 => String::from_utf8(out).map(Some).map_err(|e| e.to_string()),
        3 => Ok(None),
        _ => Err(format!(
            "findUp on {machine}: {}",
            String::from_utf8_lossy(&err)
        )),
    }
}

pub fn find(
    name: String,
    folder: Option<String>,
    machine: Option<String>,
) -> impl std::future::Future<Output = Result<HostValue, HostError>> + Send {
    let (tx, rx) = async_channel::bounded(1);
    let input =
        marker(&name).and_then(|()| folder.ok_or_else(|| "findUp has no folder context".into()));
    match (input, machine) {
        (Err(error), _) => {
            let _ = tx.try_send(Err(error));
        }
        (Ok(folder), None) => {
            std::thread::spawn(move || {
                let _ = tx.send_blocking(local(&name, &folder, &crate::mods::home()));
            });
        }
        (Ok(folder), Some(machine)) => {
            let mut cache = cache().lock().unwrap();
            let key = (cache.epoch, machine, folder, name);
            match cache.entries.get_mut(&key) {
                Some(Entry::Ready(answer)) => {
                    let _ = tx.try_send(Ok(answer.clone()));
                }
                Some(Entry::Pending(waiters)) => waiters.push(tx),
                None => {
                    if cache.entries.len() >= 256 {
                        cache
                            .entries
                            .retain(|_, entry| matches!(entry, Entry::Pending(_)));
                    }
                    cache.entries.insert(key.clone(), Entry::Pending(vec![tx]));
                    std::thread::spawn(move || {
                        let answer = remote(&key.3, &key.2, &key.1);
                        let mut cache = self::cache().lock().unwrap();
                        let Some(Entry::Pending(waiters)) = cache.entries.remove(&key) else {
                            return;
                        };
                        if key.0 == cache.epoch {
                            if let Ok(value) = &answer {
                                cache.entries.insert(key, Entry::Ready(value.clone()));
                            }
                        }
                        for waiter in waiters {
                            let _ = waiter.try_send(answer.clone());
                        }
                    });
                }
            }
        }
    }
    async move {
        let answer = rx
            .recv()
            .await
            .map_err(|_| HostError::new("findUp worker stopped"))?
            .map_err(HostError::new)?;
        Ok(answer.map_or(HostValue::Null, HostValue::Str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn lexical_parents_do_not_follow_symlinks() {
        assert_eq!(lexical(Path::new("/a/b/../c")).unwrap(), Path::new("/a/c"));
        assert!(marker("../marker").is_err());
        assert!(marker("/marker").is_err());
        assert!(marker(".wandb").is_ok());
        assert!(marker("dir/marker").is_err());
        assert!(marker(".").is_err());
        assert!(marker("..").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn remote_and_local_reject_unsearchable_start_and_find_symlink_entries() {
        use std::os::unix::{
            fs::{PermissionsExt, symlink},
            process::CommandExt,
        };
        let base = if cfg!(target_os = "linux") {
            PathBuf::from("/var/tmp")
        } else {
            std::env::temp_dir()
        };
        let root = base.join(format!("find-up-remote-{}", std::process::id()));
        let denied = root.join("denied");
        let child = denied.join("sub");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0)).unwrap();
        let mut command = std::process::Command::new("sh");
        command.args([
            "-c",
            REMOTE_WALK,
            "find-up-test",
            child.to_str().unwrap(),
            ".wandb",
        ]);
        let root_user = std::process::Command::new("id")
            .arg("-u")
            .output()
            .unwrap()
            .stdout
            == b"0\n";
        if root_user {
            command.uid(65534).gid(65534);
        }
        let outcome = command.output().unwrap();
        let local_outcome = local(".wandb", child.to_str().unwrap(), &root);
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            outcome.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&outcome.stderr)
        );
        assert!(String::from_utf8_lossy(&outcome.stderr).contains("cannot enter"));
        if !root_user {
            assert!(local_outcome.is_err());
        }
        symlink(root.join("missing-target"), root.join(".wandb")).unwrap();
        assert_eq!(
            local(".wandb", root.to_str().unwrap(), &root).unwrap(),
            Some(root.to_string_lossy().into_owned())
        );
        let outcome = std::process::Command::new("sh")
            .args([
                "-c",
                REMOTE_WALK,
                "find-up-test",
                root.to_str().unwrap(),
                ".wandb",
            ])
            .output()
            .unwrap();
        assert_eq!(outcome.status.code(), Some(0));
        assert_eq!(
            String::from_utf8(outcome.stdout).unwrap(),
            root.to_string_lossy()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nearest_marker_and_home_boundary() {
        let base = if cfg!(target_os = "linux") {
            PathBuf::from("/var/tmp")
        } else {
            std::env::temp_dir()
        };
        let root = base.join(format!("find-up-{}", std::process::id()));
        let home = root.join("home");
        let child = home.join("project/sub");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(root.join("marker"), "").unwrap();
        assert_eq!(
            local("marker", child.to_str().unwrap(), &home).unwrap(),
            None
        );
        std::fs::write(home.join("marker"), "").unwrap();
        assert_eq!(
            local("marker", child.to_str().unwrap(), &home).unwrap(),
            Some(home.to_string_lossy().into_owned())
        );
        std::fs::create_dir(child.parent().unwrap().join("marker")).unwrap();
        assert_eq!(
            local("marker", child.to_str().unwrap(), &home).unwrap(),
            Some(child.parent().unwrap().to_string_lossy().into_owned())
        );
        assert_eq!(
            local("marker", root.to_str().unwrap(), &home).unwrap(),
            Some(root.to_string_lossy().into_owned())
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
