//! Watching the home folder for the files people and agents edit (settings,
//! keys, programs, themes, layouts, mods), so changes apply live without
//! looking at the disk on a timer. One OS watcher (FSEvents, inotify) for all.

use notify::{RecursiveMode, Watcher as _};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// What changed, relative to the home folder.
pub type Changed = Vec<PathBuf>;

/// Starts watching `home`; None if the OS watcher can't start (the app then
/// polls as before).
pub fn watch(home: &Path) -> Option<async_channel::Receiver<Changed>> {
    static WATCHER: OnceLock<std::sync::Mutex<notify::RecommendedWatcher>> = OnceLock::new();
    let (tx, rx) = async_channel::unbounded();
    let root = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let base = root.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else { return };
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return;
        }
        let changed: Changed = event.paths.iter().filter_map(|p| p.strip_prefix(&base).ok()).filter(|p| watched(p)).map(Path::to_path_buf).collect();
        if !changed.is_empty() {
            let _ = tx.send_blocking(changed);
        }
    })
    .map_err(|e| tracing::warn!("file watching unavailable ({e}); polling instead"))
    .ok()?;
    watcher.watch(&root, RecursiveMode::Recursive).map_err(|e| tracing::warn!("cannot watch {} ({e}); polling instead", root.display())).ok()?;
    WATCHER.set(std::sync::Mutex::new(watcher)).ok()?;
    Some(rx)
}

/// The files that change what the app shows. The app's own state files
/// (view.json, shared.json, data/) change all the time and aren't.
fn watched(rel: &Path) -> bool {
    let mut parts = rel.components();
    let first = parts.next().and_then(|c| c.as_os_str().to_str()).unwrap_or_default();
    match first {
        "mods" => mod_file_counts(parts),
        "layouts" | "themes" => true,
        "settings.json" | "keys.json" | "programs.json" => parts.next().is_none(),
        _ => false,
    }
}

/// A change inside `mods/` that should reload a mod. The runtime writes its
/// typings and jsconfig into each mod on load, and editors drop hidden and
/// backup files: none of those are the mod's code.
fn mod_file_counts(rest: std::path::Components) -> bool {
    let parts: Vec<&str> = rest.filter_map(|c| c.as_os_str().to_str()).collect();
    if parts.iter().any(|p| p.starts_with('.') || p.ends_with('~') || *p == "node_modules") {
        return false;
    }
    !matches!(parts.as_slice(), [_, "jsconfig.json"] | [_, .., "gpui-kit.d.ts"])
}

/// Folders mods watch (`watchFolder`): one OS watcher each, shared by every
/// mod waiting on it. Bounded invalidations collect even while nobody waits.
struct Folder {
    _watcher: std::sync::mpsc::Sender<()>,
    waiters: Vec<async_channel::Sender<Vec<String>>>,
    pending: HashSet<String>,
}

static FOLDERS: OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, Folder>>> = OnceLock::new();
/// Changes are batched this long (an editor's save is several events).
const BATCH: std::time::Duration = std::time::Duration::from_millis(150);
/// At most this many folders watched for mods at once.
const MAX_FOLDERS: usize = 64;
const MAX_PENDING: usize = 1024;
#[cfg(target_os = "linux")]
const MAX_DIRECTORIES: usize = 4096;

fn folders() -> std::sync::MutexGuard<'static, std::collections::HashMap<PathBuf, Folder>> {
    let registry = FOLDERS.get_or_init(Default::default);
    registry.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("recovering poisoned folder watch registry");
        registry.clear_poison();
        poisoned.into_inner()
    })
}

/// The next changes under `path` (recursive), as absolute paths: at once if
/// some came since the last call, else when the next batch lands.
pub async fn next_change(path: String) -> Result<Vec<String>, String> {
    let root = Path::new(&path).canonicalize().map_err(|e| format!("{path}: {e}"))?;
    let rx = {
        let mut all = folders();
        if !all.contains_key(&root) {
            if all.len() >= MAX_FOLDERS {
                return Err(format!("already watching {MAX_FOLDERS} folders"));
            }
            all.insert(root.clone(), Folder { _watcher: start_folder(&root)?, waiters: Vec::new(), pending: HashSet::new() });
        }
        let f = all.get_mut(&root).unwrap();
        if !f.pending.is_empty() {
            return Ok(f.pending.drain().collect());
        }
        f.waiters.retain(|w| !w.is_closed());
        let (tx, rx) = async_channel::bounded(1);
        f.waiters.push(tx);
        rx
    };
    rx.recv().await.map_err(|_| "stopped watching".to_owned())
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    /// Git supplies its own ignore semantics, including global and nested rules.
    fn ignored_directories(root: &Path) -> HashSet<PathBuf> {
        let Ok(output) = std::process::Command::new("git").arg("-C").arg(root).args(["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"]).output() else {
            return HashSet::new();
        };
        if !output.status.success() {
            return HashSet::new();
        }
        output.stdout.split(|b| *b == 0).filter(|p| p.ends_with(b"/")).map(|p| root.join(path_from_bytes(p))).collect()
    }

    fn path_from_bytes(bytes: &[u8]) -> PathBuf {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }

    pub(super) fn watch_directories(root: &Path) -> Result<HashSet<PathBuf>, String> {
        let ignored = ignored_directories(root);
        let mut paths = HashSet::from([root.to_path_buf()]);
        let mut git_roots = HashSet::new();
        if let Ok(output) = std::process::Command::new("git").arg("-C").arg(root).args(["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"]).output()
            && output.status.success()
        {
            for line in output.stdout.split(|b| *b == b'\n').filter(|p| !p.is_empty()) {
                git_roots.insert(path_from_bytes(line));
            }
        }
        let mut todo = vec![root.to_path_buf()];
        for dir in &git_roots {
            if paths.insert(dir.clone()) {
                todo.push(dir.clone());
            }
        }
        while let Some(dir) = todo.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|t| t.is_dir()) || ignored.contains(&entry.path()) {
                    continue;
                }
                // Git's object database can dwarf the working tree. HEAD/index and refs
                // still invalidate git panels; objects themselves cannot change status.
                if (git_roots.contains(&dir) || dir.file_name().is_some_and(|n| n == ".git")) && entry.file_name() != "refs" && entry.file_name() != "info" {
                    continue;
                }
                if paths.insert(entry.path()) {
                    if paths.len() > MAX_DIRECTORIES {
                        return Err(format!("{} exceeds {MAX_DIRECTORIES} watch directories; live folder updates unavailable, use periodic refresh", root.display()));
                    }
                    todo.push(entry.path());
                }
            }
        }
        Ok(paths)
    }
}

fn watch_directories(root: &Path) -> Result<HashSet<PathBuf>, String> {
    #[cfg(target_os = "linux")]
    {
        linux::watch_directories(root)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(HashSet::from([root.to_path_buf()]))
    }
}

fn folder_watch_mode() -> RecursiveMode {
    // Inotify consumes a watch per directory; FSEvents and Windows recurse natively.
    if cfg!(target_os = "linux") { RecursiveMode::NonRecursive } else { RecursiveMode::Recursive }
}

fn add_pending(pending: &mut HashSet<String>, root: &Path, paths: impl IntoIterator<Item = PathBuf>) {
    let root = root.to_string_lossy().into_owned();
    for path in paths {
        if pending.contains(&root) {
            break;
        }
        if pending.len() >= MAX_PENDING {
            // A root invalidation means refresh everything, not a partial path list.
            pending.clear();
            pending.insert(root.clone());
            break;
        }
        pending.insert(path.to_string_lossy().into_owned());
    }
}

fn collect_event(event: notify::Event, root: &Path, directories: &HashSet<PathBuf>, pending: &mut HashSet<String>, removed: &mut HashSet<PathBuf>) -> bool {
    if event.need_rescan() {
        pending.clear();
        pending.insert(root.to_string_lossy().into_owned());
        // With dropped events any directory may have been replaced.
        removed.extend(directories.iter().cloned());
        return true;
    }
    if matches!(event.kind, notify::EventKind::Remove(_) | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))) {
        removed.extend(event.paths.iter().filter(|p| directories.contains(*p)).cloned());
    }
    let refresh = event.paths.iter().any(|p| {
        p.file_name().is_some_and(|n| n == ".gitignore" || n == "exclude")
            || (matches!(event.kind, notify::EventKind::Create(_)) && p.is_dir())
            || (matches!(event.kind, notify::EventKind::Remove(_)) && directories.contains(p))
    }) || matches!(event.kind, notify::EventKind::Modify(notify::event::ModifyKind::Name(_)));
    add_pending(pending, root, event.paths);
    refresh
}

fn start_folder(root: &Path) -> Result<std::sync::mpsc::Sender<()>, String> {
    let mut directories = watch_directories(root).inspect_err(|e| tracing::warn!("{e}"))?;
    let (tx, events) = std::sync::mpsc::sync_channel(1024);
    let overflow = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lost = overflow.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.as_ref().is_ok_and(|e| !e.need_rescan() && matches!(e.kind, notify::EventKind::Access(_))) {
            return;
        }
        if tx.try_send(event).is_err() {
            lost.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    })
    .map_err(|e| format!("cannot watch: {e}"))?;
    for path in &directories {
        watcher.watch(path, folder_watch_mode()).map_err(|e| format!("cannot watch {}: {e}", path.display()))?;
    }
    let root = root.to_path_buf();
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        'watching: loop {
            if matches!(stopped.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)) {
                break;
            }
            let deadline = std::time::Instant::now() + BATCH;
            let mut pending = HashSet::new();
            let mut refresh = false;
            let mut removed = HashSet::new();
            while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
                match events.recv_timeout(left) {
                    Ok(Ok(event)) => {
                        refresh |= collect_event(event, &root, &directories, &mut pending, &mut removed);
                    }
                    Ok(Err(_)) => {
                        removed.extend(directories.iter().cloned());
                        refresh = true;
                        pending.clear();
                        pending.insert(root.to_string_lossy().into_owned());
                    }
                    Err(_) => break,
                }
            }
            if overflow.swap(false, std::sync::atomic::Ordering::Relaxed) {
                removed.extend(directories.iter().cloned());
                refresh = true;
                pending.clear();
                pending.insert(root.to_string_lossy().into_owned());
            }
            if cfg!(target_os = "linux") && refresh {
                // Rescan off the notify callback thread: adding watches blocks on it.
                match watch_directories(&root) {
                    Ok(next) => {
                        // A removed inode can return at the same path before this batch
                        // finishes. Its descendants also lost their inotify watches.
                        let replaced = |path: &PathBuf| path.ancestors().any(|p| removed.contains(p));
                        for path in &directories {
                            if !next.contains(path) || replaced(path) {
                                let _ = watcher.unwatch(path);
                            }
                        }
                        for path in &next {
                            if directories.contains(path) && !replaced(path) {
                                continue;
                            }
                            if let Err(e) = watcher.watch(path, folder_watch_mode()) {
                                tracing::warn!("cannot watch {}: {e}; live folder updates unavailable, use periodic refresh", path.display());
                                folders().remove(&root);
                                break 'watching;
                            }
                        }
                        directories = next;
                    }
                    Err(e) => {
                        tracing::warn!("{e}");
                        // Do not leave a partial recursive watch pretending to be live.
                        let mut all = folders();
                        all.remove(&root);
                        break;
                    }
                }
            }
            if pending.is_empty() {
                continue;
            }
            if !deliver_pending(&root, pending) {
                break;
            }
        }
    });
    Ok(stop)
}

fn deliver_pending(root: &Path, pending: HashSet<String>) -> bool {
    let mut all = folders();
    let Some(f) = all.get_mut(root) else { return false };
    add_pending(&mut f.pending, root, pending.into_iter().map(PathBuf::from));
    f.waiters.retain(|w| !w.is_closed());
    if !f.waiters.is_empty() {
        let batch: Vec<_> = f.pending.drain().collect();
        for w in f.waiters.drain(..) {
            let _ = w.try_send(batch.clone());
        }
    }
    true
}

/// The mod a changed path belongs to (`mods/<name>/...`).
pub fn mod_of(rel: &Path) -> Option<String> {
    let mut parts = rel.components();
    (parts.next()?.as_os_str() == "mods").then_some(())?;
    parts.next()?.as_os_str().to_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_people_edit_counts() {
        assert!(watched(Path::new("settings.json")));
        assert!(watched(Path::new("mods/agents/main.js")));
        assert!(watched(Path::new("layouts/main.json")));
        assert!(!watched(Path::new("view.json")));
        assert!(!watched(Path::new("shared.json")));
        assert!(!watched(Path::new("data/x")));
        assert_eq!(mod_of(Path::new("mods/agents/main.js")).as_deref(), Some("agents"));
    }

    #[test]
    fn files_the_runtime_or_an_editor_writes_into_a_mod_do_not_reload_it() {
        assert!(watched(Path::new("mods/probe")));
        assert!(watched(Path::new("mods/probe/ui.js")));
        assert!(watched(Path::new("mods/probe/lib/jsconfig.json")));
        assert!(!watched(Path::new("mods/probe/jsconfig.json")));
        assert!(!watched(Path::new("mods/probe/gpui-kit.d.ts")));
        assert!(!watched(Path::new("mods/probe/.main.js.swp")));
        assert!(!watched(Path::new("mods/probe/.vscode/settings.json")));
        assert!(!watched(Path::new("mods/probe/main.js~")));
        assert!(!watched(Path::new("mods/probe/lib/gpui-kit.d.ts")));
        assert!(!watched(Path::new("mods/probe/node_modules/x/index.js")));
        assert!(!watched(Path::new("mods/probe/.git/HEAD")));
        assert!(watched(Path::new("mods/probe/mod.json")));
        assert_eq!(mod_of(Path::new("layouts/main.json")), None);
    }

    #[test]
    fn pending_paths_deduplicate_and_overflow_to_a_root_invalidation() {
        let root = Path::new("/project");
        let mut pending = HashSet::new();
        add_pending(&mut pending, root, [root.join("same"), root.join("same")]);
        assert_eq!(pending.len(), 1);
        add_pending(&mut pending, root, (0..MAX_PENDING * 10).map(|i| root.join(i.to_string())));
        assert_eq!(pending, HashSet::from(["/project".to_owned()]));
    }

    fn test_repo(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("herdr-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(std::process::Command::new("git").arg("init").arg("-q").arg(&root).status().unwrap().success());
        root.canonicalize().unwrap()
    }

    // Backends may invalidate a parent rather than name every changed child.
    fn invalidates(changes: &[String], path: &Path) -> bool {
        changes.iter().any(|changed| path.starts_with(changed))
    }

    fn install_watch(root: &Path) {
        let mut wait = Box::pin(next_change(root.to_string_lossy().into_owned()));
        assert!(wait.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())).is_pending());
    }

    fn wait_for_change(root: &Path, path: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let changes = block_on_until(next_change(root.to_string_lossy().into_owned()), deadline).unwrap();
            if invalidates(&changes, path) {
                return;
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn git_ignored_directories_are_pruned_but_tracked_files_and_refs_stay_watched() {
        let root = test_repo("ignore");
        std::fs::create_dir_all(root.join("target/deep")).unwrap();
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::create_dir_all(root.join("tracked")).unwrap();
        std::fs::write(root.join("tracked/keep"), "tracked").unwrap();
        assert!(std::process::Command::new("git").arg("-C").arg(&root).args(["add", "tracked/keep"]).status().unwrap().success());
        std::fs::write(root.join(".gitignore"), "target/\ntracked/\n").unwrap();
        let paths = watch_directories(&root).unwrap();
        assert!(!paths.contains(&root.join("target")));
        assert!(paths.contains(&root.join("src/nested")));
        assert!(paths.contains(&root.join("tracked")));
        assert!(paths.contains(&root.join(".git/refs/heads")));
        assert!(!paths.contains(&root.join(".git/objects")));
        assert!(paths.contains(&root.join(".git/info")));
        // Starting directly in an ignored folder still watches what was requested.
        assert!(watch_directories(&root.join("target")).unwrap().contains(&root.join("target/deep")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn new_directories_and_changed_ignore_rules_update_live_watches() {
        use std::time::{Duration, Instant};
        let root = test_repo("dynamic");
        std::fs::create_dir_all(root.join("target/deep")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        // Poll once to install the watch, then abandon the waiter like a closed mod.
        install_watch(&root);
        std::fs::create_dir_all(root.join("fresh/nested")).unwrap();
        // Delivery follows watch registration, not an assumed backend delay.
        wait_for_change(&root, &root.join("fresh/nested"));
        std::fs::write(root.join("fresh/nested/file"), "one").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if folders().get(&root).unwrap().pending.iter().any(|p| p.ends_with("nested/file")) {
                break;
            }
            assert!(Instant::now() < deadline, "new directory was not watched");
            std::thread::sleep(BATCH);
        }
        folders().get_mut(&root).unwrap().pending.clear();
        std::fs::write(root.join("target/deep/ignored"), "ignored").unwrap();
        std::thread::sleep(BATCH * 3);
        assert!(folders().get(&root).unwrap().pending.is_empty());
        std::fs::write(root.join(".gitignore"), "").unwrap();
        wait_for_change(&root, &root.join(".gitignore"));
        std::fs::write(root.join("target/deep/now-visible"), "yes").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if folders().get(&root).unwrap().pending.iter().any(|p| p.ends_with("now-visible")) {
                break;
            }
            assert!(Instant::now() < deadline, "ignore change did not update watches");
            std::thread::sleep(BATCH);
        }
        folders().remove(&root);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn abandoned_waiters_do_not_leave_an_unbounded_queue() {
        let root = test_repo("overflow");
        install_watch(&root);
        // Inject worker batches: native backends may coalesce real writes, so a
        // filesystem flood cannot deterministically exercise queue overflow.
        for i in 0..6000 {
            assert!(deliver_pending(&root, HashSet::from([root.join(format!("file-{i}")).to_string_lossy().into_owned()])));
        }
        let (pending, waiters) = {
            let all = folders();
            let folder = all.get(&root).unwrap();
            (folder.pending.clone(), folder.waiters.len())
        };
        assert_eq!(pending, HashSet::from([root.to_string_lossy().into_owned()]));
        assert_eq!(waiters, 0, "closed waiters must be removed on delivery");
        assert!(invalidates(&block_on(next_change(root.to_string_lossy().into_owned())).unwrap(), &root));
        folders().remove(&root);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn separate_git_metadata_is_watched_without_its_object_tree() {
        let root = test_repo("separate");
        let git = root.with_extension("metadata");
        let _ = std::fs::remove_dir_all(&git);
        assert!(std::process::Command::new("git").arg("init").arg("-q").arg("--separate-git-dir").arg(&git).arg(&root).status().unwrap().success());
        let paths = watch_directories(&root).unwrap();
        assert!(paths.contains(&git));
        assert!(paths.contains(&git.join("refs/heads")));
        assert!(!paths.contains(&git.join("objects")));
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(git).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn oversized_trees_reject_instead_of_silently_missing_nested_changes() {
        let root = std::env::temp_dir().join(format!("herdr-watch-limit-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..MAX_DIRECTORIES {
            std::fs::create_dir_all(root.join(i.to_string())).unwrap();
        }
        assert!(watch_directories(&root).unwrap_err().contains("live folder updates unavailable"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn native_recursive_backends_register_only_the_root() {
        let root = test_repo("native-recursive");
        std::fs::create_dir_all(root.join("target/deep")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        assert_eq!(folder_watch_mode(), RecursiveMode::Recursive);
        assert_eq!(watch_directories(&root).unwrap(), HashSet::from([root.clone()]));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backend_rescan_invalidates_root_and_refreshes_watches() {
        let root = Path::new("/project");
        let mut pending = HashSet::from(["/project/stale".to_owned()]);
        let directories = HashSet::from([root.to_path_buf(), root.join("sub"), PathBuf::from("/external/git")]);
        let mut removed = HashSet::new();
        let event = notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan).add_path(root.join("partial"));
        assert!(collect_event(event, root, &directories, &mut pending, &mut removed));
        assert_eq!(pending, HashSet::from(["/project".to_owned()]));
        assert_eq!(removed, directories);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn exact_cap_counts_each_git_directory_once() {
        let root = test_repo("exact-cap");
        let count = watch_directories(&root).unwrap().len();
        for i in count..MAX_DIRECTORIES {
            std::fs::create_dir(root.join(format!("dir-{i}"))).unwrap();
        }
        assert_eq!(watch_directories(&root).unwrap().len(), MAX_DIRECTORIES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renamed_directory_and_descendants_keep_reporting_changes() {
        check_replaced_directory(true);
    }

    #[test]
    fn replaced_directory_and_descendants_keep_reporting_changes() {
        check_replaced_directory(false);
    }

    fn check_replaced_directory(rename: bool) {
        let root = test_repo(if rename { "renamed" } else { "replaced" });
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        install_watch(&root);
        // The old paths survive in the selected set, but now name new inodes.
        if rename {
            std::fs::rename(root.join("sub"), root.join("moved")).unwrap();
        } else {
            std::fs::remove_dir_all(root.join("sub")).unwrap();
        }
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        wait_for_change(&root, &root.join("sub"));
        std::fs::write(root.join("sub/deep/after"), "yes").unwrap();
        wait_for_change(&root, &root.join("sub/deep/after"));
        if rename {
            std::fs::write(root.join("moved/deep/after"), "yes").unwrap();
            wait_for_change(&root, &root.join("moved/deep/after"));
        }
        folders().remove(&root);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Drives a future to its end on this thread (the tests have no executor).
    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        block_on_until(f, std::time::Instant::now() + std::time::Duration::from_secs(10))
    }

    fn block_on_until<F: std::future::Future>(f: F, deadline: std::time::Instant) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
        let mut f = std::pin::pin!(f);
        loop {
            match f.as_mut().poll(&mut Context::from_waker(&waker)) {
                Poll::Ready(v) => return v,
                Poll::Pending => {
                    assert!(std::time::Instant::now() < deadline, "timed out waiting for a folder invalidation");
                    std::thread::park_timeout(std::time::Duration::from_millis(10));
                }
            }
        }
    }

    #[test]
    fn a_watched_folder_reports_changes_and_misses_none_between_waits() {
        let dir = test_repo("between-waits");
        let mut first = Box::pin(next_change(dir.to_string_lossy().into_owned()));
        assert!(first.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())).is_pending());
        std::fs::write(dir.join("a.md"), "one").unwrap();
        let changes = block_on(first).unwrap();
        if !invalidates(&changes, &dir.join("a.md")) {
            wait_for_change(&dir, &dir.join("a.md"));
        }
        // No waiter exists during this write. Wait for its actual delivery,
        // not a fixed sleep or an unrelated leftover batch from the first save.
        std::fs::write(dir.join("b.md"), "two").unwrap();
        wait_for_change(&dir, &dir.join("b.md"));
        folders().remove(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
