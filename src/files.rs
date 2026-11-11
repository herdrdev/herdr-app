//! Watching the home folder for the files people and agents edit (settings,
//! keys, programs, themes, layouts, mods), so changes apply live without
//! looking at the disk on a timer. Native watchers also follow linked mod roots.

use notify::{RecursiveMode, Watcher as _};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// What changed, relative to the home folder.
pub type Changed = Vec<PathBuf>;

#[derive(Clone)]
pub struct FileEvents {
    pub changes: async_channel::Receiver<Changed>,
    root: PathBuf,
    revisions: Arc<Mutex<Revisions>>,
}

#[derive(Default)]
struct Revisions {
    next: u64,
    all: u64,
    roots: std::collections::BTreeMap<PathBuf, u64>,
}

impl Revisions {
    fn changed(&mut self, changed: &Changed, rescan: bool) {
        if rescan {
            self.next += 1;
            self.all = self.next;
        }
        for root in changed.iter().filter_map(|path| mod_of(path).map(|name| PathBuf::from("mods").join(name))).collect::<HashSet<_>>() {
            self.next += 1;
            self.roots.insert(root, self.next);
        }
    }
}

impl FileEvents {
    pub fn revision_provider(&self) -> impl Fn(&Path) -> Option<u64> + 'static {
        let base = self.root.clone();
        let revisions = self.revisions.clone();
        move |root| {
            let relative = root.strip_prefix(&base).ok()?;
            if relative.components().count() != 2 || !relative.starts_with("mods") {
                return None;
            }
            let mut revisions = revisions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let all = revisions.all;
            Some((*revisions.roots.entry(relative.to_path_buf()).or_insert(all)).max(all))
        }
    }
}

fn changes(base: &Path, event: notify::Result<notify::Event>) -> (Changed, bool) {
    let Ok(event) = event else { return (Vec::new(), true) };
    if !event.need_rescan() && matches!(event.kind, notify::EventKind::Access(_)) {
        return (Vec::new(), false);
    }
    let rescan = event.need_rescan() || event.paths.iter().any(|path| {
        base.starts_with(path) || path.strip_prefix(base).is_ok_and(|relative| relative.starts_with("mods") && mod_of(relative).is_none())
    });
    let changed = event.paths.iter().filter_map(|path| path.strip_prefix(base).ok())
        .filter(|path| watched(path)).map(Path::to_path_buf).collect();
    (changed, rescan)
}

/// Starts watching `home`; None if the OS watcher can't start (the app then
/// polls as before).
pub fn watch(home: &Path, #[cfg(test)] executor: gpui::BackgroundExecutor) -> Option<FileEvents> {
    static WATCHER: OnceLock<std::sync::mpsc::Sender<()>> = OnceLock::new();
    let (tx, rx) = async_channel::unbounded();
    #[cfg(test)]
    let tx = test_notifications(tx, executor);
    let root = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let revisions = Arc::new(Mutex::new(Revisions::default()));
    let event_revisions = revisions.clone();
    let watcher = watch_home(&root, move |changed, rescan| {
        let mut revisions = event_revisions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        revisions.changed(&changed, rescan);
        drop(revisions);
        if !changed.is_empty() {
            #[cfg(not(test))]
            let _ = tx.send_blocking(changed);
            #[cfg(test)]
            let _ = tx.send(changed);
        }
    }).map_err(|e| tracing::warn!("file watching unavailable ({e}); polling instead")).ok()?;
    WATCHER.set(watcher).ok()?;
    Some(FileEvents { changes: rx, root, revisions })
}

// Linked roots need their own watcher: some backends follow links at startup,
// but do not start following a link created later. Keep it separate so removing
// a target watch cannot remove a home watch on the same inode.
fn watch_home(root: &Path, publish: impl Fn(Changed, bool) + Send + 'static) -> notify::Result<std::sync::mpsc::Sender<()>> {
    let (tx, events) = std::sync::mpsc::channel();
    let callback = move |event: notify::Result<notify::Event>| {
        if event.as_ref().is_ok_and(|e| !e.need_rescan() && matches!(e.kind, notify::EventKind::Access(_))) {
            return;
        }
        let _ = tx.send(event);
    };
    let mut home = notify::recommended_watcher(callback.clone())?;
    home.watch(root, RecursiveMode::Recursive)?;
    let mut targets = notify::recommended_watcher(callback)?;
    let mut links = linked_mods(root);
    let mut watched_targets = BTreeMap::new();
    sync_link_watches(&mut targets, &links, &mut watched_targets, &HashSet::new());
    let root = root.to_path_buf();
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _home = home;
        while !matches!(stopped.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)) {
            let event = match events.recv_timeout(BATCH) {
                Ok(event) => event,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            let refresh = event.as_ref().map_or(true, |e| e.need_rescan() || e.paths.iter().any(|path| {
                path.parent() == Some(root.join("mods").as_path()) || links.iter().any(|link| path == &link.target || link.symlinks.contains(path))
            }));
            let rearm: HashSet<PathBuf> = links.iter().filter(|link| {
                event.as_ref().map_or(true, |e| e.need_rescan() || e.paths.iter().any(|path| path == &link.target))
            }).map(|link| link.target.clone()).collect();
            let event = event.map(|mut event| {
                let mut mapped = Vec::new();
                for path in &event.paths {
                    for link in &links {
                        if let Ok(relative) = path.strip_prefix(&link.target) {
                            mapped.push(link.alias.join(relative));
                        } else if link.symlinks.contains(path) {
                            mapped.push(link.alias.clone());
                        }
                    }
                }
                event.paths.extend(mapped);
                event
            });
            if refresh {
                links = linked_mods(&root);
                sync_link_watches(&mut targets, &links, &mut watched_targets, &rearm);
            }
            let (changed, rescan) = changes(&root, event);
            if rescan || !changed.is_empty() {
                publish(changed, rescan);
            }
        }
    });
    Ok(stop)
}

struct LinkedMod {
    target: PathBuf,
    alias: PathBuf,
    symlinks: HashSet<PathBuf>,
}

fn linked_mods(root: &Path) -> Vec<LinkedMod> {
    std::fs::read_dir(root.join("mods")).into_iter().flatten().flatten().filter_map(|entry| {
        let alias = entry.path();
        let relative = alias.strip_prefix(root).ok()?;
        if !watched(relative) || !entry.file_type().ok()?.is_symlink() {
            return None;
        }
        let mut symlinks = HashSet::new();
        let target = resolve_mod_link(&alias, &mut symlinks)?;
        Some(LinkedMod { target, alias, symlinks })
    }).collect()
}

fn resolve_mod_link(path: &Path, symlinks: &mut HashSet<PathBuf>) -> Option<PathBuf> {
    use std::path::Component;
    let mut pending: std::collections::VecDeque<_> = path.components().map(|part| part.as_os_str().to_os_string()).collect();
    let mut resolved = PathBuf::new();
    let mut followed = 0;
    while let Some(part) = pending.pop_front() {
        match Path::new(&part).components().next()? {
            Component::CurDir => continue,
            Component::ParentDir => { resolved.pop(); continue; }
            _ => {}
        }
        let next = resolved.join(&part);
        if let Ok(target) = std::fs::read_link(&next) {
            followed += 1;
            if followed > 40 { return None; }
            symlinks.insert(next);
            if target.is_absolute() { resolved.clear(); }
            for component in target.components().rev() {
                pending.push_front(component.as_os_str().to_os_string());
            }
        } else {
            resolved = next;
        }
    }
    Some(resolved)
}

fn sync_link_watches(watcher: &mut impl notify::Watcher, links: &[LinkedMod], watched: &mut BTreeMap<PathBuf, RecursiveMode>, rearm: &HashSet<PathBuf>) {
    let mut paths = BTreeMap::new();
    for link in links {
        for path in link.symlinks.iter().chain(std::iter::once(&link.target)) {
            if let Some(parent) = path.parent() {
                paths.entry(parent.to_path_buf()).or_insert(RecursiveMode::NonRecursive);
            }
        }
        if link.target.is_dir() {
            paths.insert(link.target.clone(), RecursiveMode::Recursive);
        }
    }
    // A recursive watch already owns its descendants. Registering them again
    // can downgrade or remove that inherited coverage in native backends.
    let recursive: HashSet<_> = paths.iter().filter(|(_, mode)| **mode == RecursiveMode::Recursive)
        .map(|(path, _)| path.clone()).collect();
    paths.retain(|path, _| !path.ancestors().skip(1).any(|parent| recursive.contains(parent)));
    let removed: Vec<_> = watched.iter().filter(|(path, mode)| {
        paths.get(*path) != Some(*mode) || rearm.iter().any(|root| path.starts_with(root))
    }).map(|(path, _)| path.clone()).collect();
    // Linux may have recursively registered descendants through an overlapping
    // ancestor watch. Restore them whenever an ancestor is removed.
    let mut removed: Vec<_> = watched.keys().filter(|path| removed.iter().any(|root| path.starts_with(root))).cloned().collect();
    removed.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for path in removed {
        let _ = watcher.unwatch(&path);
        watched.remove(&path);
    }
    for (path, mode) in paths {
        if watched.get(&path) == Some(&mode) { continue; }
        match watcher.watch(&path, mode) {
            Ok(()) => { watched.insert(path, mode); }
            Err(e) => tracing::warn!("cannot watch linked mod target {} ({e})", path.display()),
        }
    }
}

// Native callbacks cannot wake GPUI's deterministic test scheduler directly.
#[cfg(test)]
fn test_notifications(tx: async_channel::Sender<Changed>, executor: gpui::BackgroundExecutor) -> std::sync::mpsc::Sender<Changed> {
    let (pending, events) = std::sync::mpsc::channel();
    let clock = executor.clone();
    executor.spawn(async move {
        while !tx.is_closed() {
            clock.timer(std::time::Duration::from_millis(10)).await;
            while let Ok(changed) = events.try_recv() {
                if tx.try_send(changed).is_err() { return; }
            }
        }
    }).detach();
    pending
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
    let mut rest = rest;
    let Some(name) = rest.next() else { return true };
    gpui_shell::application_file_counts(Path::new(name.as_os_str()))
        && gpui_shell::application_file_counts(rest.as_path())
}

/// Folders mods watch (`watchFolder`): one OS watcher each, shared by every
/// mod waiting on it. Bounded invalidations collect even while nobody waits.
struct Folder {
    _watcher: std::sync::mpsc::Sender<()>,
    generation: Arc<()>,
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
                reclaim_idle(&mut all);
            }
            if all.len() >= MAX_FOLDERS {
                return Err(format!("already watching {MAX_FOLDERS} folders"));
            }
            let generation = Arc::new(());
            all.insert(root.clone(), Folder { _watcher: start_folder(&root, generation.clone())?, generation, waiters: Vec::new(), pending: HashSet::new() });
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

fn reclaim_idle(all: &mut std::collections::HashMap<PathBuf, Folder>) {
    all.retain(|_, folder| {
        folder.waiters.retain(|waiter| !waiter.is_closed());
        !folder.waiters.is_empty()
    });
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

fn start_folder(root: &Path, generation: Arc<()>) -> Result<std::sync::mpsc::Sender<()>, String> {
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
                                remove_folder(&root, &generation);
                                break 'watching;
                            }
                        }
                        directories = next;
                    }
                    Err(e) => {
                        tracing::warn!("{e}");
                        // Do not leave a partial recursive watch pretending to be live.
                        remove_folder(&root, &generation);
                        break;
                    }
                }
            }
            if pending.is_empty() {
                continue;
            }
            if !deliver_pending(&root, &generation, pending) {
                break;
            }
        }
    });
    Ok(stop)
}

fn remove_folder(root: &Path, generation: &Arc<()>) {
    let mut all = folders();
    if all.get(root).is_some_and(|f| Arc::ptr_eq(&f.generation, generation)) {
        all.remove(root);
    }
}

fn deliver_pending(root: &Path, generation: &Arc<()>, pending: HashSet<String>) -> bool {
    let mut all = folders();
    let Some(f) = all.get_mut(root) else { return false };
    // A reclaimed path can already have a new watcher while the old worker exits.
    if !Arc::ptr_eq(&f.generation, generation) {
        return false;
    }
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
    fn linked_watch_refreshes_only_register_new_or_invalidated_paths() {
        #[derive(Default)]
        struct RecordingWatcher {
            active: BTreeMap<PathBuf, RecursiveMode>,
            registrations: usize,
            removals: usize,
        }
        impl notify::Watcher for RecordingWatcher {
            fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> { Ok(Self::default()) }
            fn kind() -> notify::WatcherKind { notify::WatcherKind::PollWatcher }
            fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
                assert!(!self.active.contains_key(path), "duplicate registration: {path:?}");
                self.registrations += 1;
                self.active.insert(path.to_path_buf(), mode);
                Ok(())
            }
            fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
                let recursive = self.active.remove(path) == Some(RecursiveMode::Recursive);
                if recursive { self.active.retain(|child, _| !child.starts_with(path)); }
                self.removals += 1;
                Ok(())
            }
        }
        let scratch = test_repo("linked-watch-registrations");
        let source = scratch.join("source");
        let child = source.join("child");
        std::fs::create_dir_all(&child).unwrap();
        let mut links = vec![
            LinkedMod { target: source.clone(), alias: scratch.join("first"), symlinks: HashSet::new() },
            LinkedMod { target: child.clone(), alias: scratch.join("second"), symlinks: HashSet::new() },
        ];
        let mut watcher = RecordingWatcher::default();
        let mut watched = BTreeMap::new();
        sync_link_watches(&mut watcher, &links, &mut watched, &HashSet::new());
        assert_eq!(watcher.registrations, 2);
        for _ in 0..20 {
            sync_link_watches(&mut watcher, &links, &mut watched, &HashSet::new());
        }
        assert_eq!(watcher.registrations, 2);
        assert_eq!(watcher.removals, 0);
        // Removing the outer target replaces its recursive watch with a
        // non-recursive parent watch and a recursive watch for the child.
        links.remove(0);
        sync_link_watches(&mut watcher, &links, &mut watched, &HashSet::new());
        assert_eq!(watcher.registrations, 4);
        assert_eq!(watcher.removals, 2);
        assert_eq!(watcher.active, watched);
        assert_eq!(watched.get(&source), Some(&RecursiveMode::NonRecursive));
        assert_eq!(watched.get(&child), Some(&RecursiveMode::Recursive));
        sync_link_watches(&mut watcher, &links, &mut watched, &HashSet::from([child.clone()]));
        assert_eq!(watcher.registrations, 5);
        assert_eq!(watcher.removals, 3);
        assert_eq!(watcher.active, watched);
        sync_link_watches(&mut watcher, &[], &mut watched, &HashSet::new());
        assert!(watcher.active.is_empty());
        assert!(watched.is_empty());
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn nested_linked_mods_keep_recursive_coverage(remove_outer: bool) {
        use std::os::unix::fs::symlink;
        let scratch = test_repo(if remove_outer { "nested-remove-outer" } else { "nested-remove-inner" });
        let root = scratch.join("home");
        let source = scratch.join("source");
        let child = source.join("lib/child");
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("main.js"), "before").unwrap();
        let (tx, events) = std::sync::mpsc::channel();
        let stop = watch_home(&root, move |changed, _| { let _ = tx.send(changed); }).unwrap();
        let wait = |expected: &str| {
            let expected = PathBuf::from(expected);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let changed = events.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .unwrap_or_else(|e| panic!("missing {expected:?}: {e}"));
                if changed.contains(&expected) { break; }
            }
        };
        symlink(&source, root.join("mods/outer")).unwrap();
        wait("mods/outer");
        symlink(&child, root.join("mods/inner")).unwrap();
        wait("mods/inner");
        // Adding the inner alias must not downgrade lib's inherited recursive watch.
        std::fs::create_dir(source.join("lib/fresh")).unwrap();
        wait("mods/outer/lib/fresh");
        std::fs::write(source.join("lib/fresh/new.js"), "added").unwrap();
        wait("mods/outer/lib/fresh/new.js");
        let removed = if remove_outer { "outer" } else { "inner" };
        std::fs::remove_file(root.join("mods").join(removed)).unwrap();
        wait(&format!("mods/{removed}"));
        std::fs::write(child.join("main.js"), "after").unwrap();
        let remaining = if remove_outer { "mods/inner" } else { "mods/outer/lib/child" };
        wait(&format!("{remaining}/main.js"));
        std::fs::create_dir(child.join("nested")).unwrap();
        wait(&format!("{remaining}/nested"));
        std::fs::write(child.join("nested/new.js"), "nested").unwrap();
        wait(&format!("{remaining}/nested/new.js"));
        drop(stop);
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn adding_and_removing_inner_link_preserves_outer_recursive_watch() {
        nested_linked_mods_keep_recursive_coverage(false);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn removing_outer_link_restores_inner_recursive_watch() {
        nested_linked_mods_keep_recursive_coverage(true);
    }

    #[cfg(unix)]
    #[test]
    fn intermediate_mod_links_retarget_and_watch_the_new_source() {
        use std::os::unix::fs::symlink;
        let scratch = test_repo("intermediate-mod-links");
        let root = scratch.join("home");
        let pointers = scratch.join("pointers");
        let first = scratch.join("first");
        let second = scratch.join("second");
        for path in [root.join("mods"), pointers.clone(), first.clone(), second.clone()] {
            std::fs::create_dir_all(path).unwrap();
        }
        symlink("../first", pointers.join("current")).unwrap();
        // Exercise a link in an intermediate path component, not just the final component.
        symlink(&pointers, scratch.join("indirect")).unwrap();
        symlink(scratch.join("indirect/current"), root.join("mods/chained")).unwrap();
        let observed = second.join("main.js");
        let (tx, events) = std::sync::mpsc::channel();
        let stop = watch_home(&root, move |changed, _| {
            let _ = tx.send((changed, std::fs::read_to_string(&observed).ok()));
        }).unwrap();
        let wait = |path: &str, content: Option<&str>| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let (changed, actual) = events.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).unwrap();
                if changed.contains(&PathBuf::from(path)) && actual.as_deref() == content { break; }
            }
        };
        std::fs::write(first.join("main.js"), "first").unwrap();
        wait("mods/chained/main.js", None);
        symlink("../second", pointers.join(".next")).unwrap();
        std::fs::rename(pointers.join(".next"), pointers.join("current")).unwrap();
        wait("mods/chained", None);
        std::fs::write(second.join("main.js"), "second").unwrap();
        wait("mods/chained/main.js", Some("second"));
        let links = linked_mods(&root);
        assert_eq!(links[0].target, second);
        // Cycles are bounded and cannot stall subsequent home events.
        symlink("cycle", pointers.join("cycle")).unwrap();
        symlink(pointers.join("cycle"), root.join("mods/cycle")).unwrap();
        assert_eq!(linked_mods(&root).len(), 1);
        drop(stop);
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn linked_mods_added_while_watching_reload_all_aliases_and_survive_atomic_saves() {
        use std::os::unix::fs::symlink;
        let scratch = test_repo("linked-mods");
        let root = scratch.join("home");
        let source = scratch.join("source");
        std::fs::create_dir_all(root.join("mods/ordinary")).unwrap();
        std::fs::create_dir_all(source.join("lib")).unwrap();
        std::fs::write(source.join("main.js"), "before").unwrap();
        symlink(&source, root.join("mods/early")).unwrap();
        let (tx, events) = std::sync::mpsc::channel();
        let replacement_file = scratch.join("other/after.js");
        let stop = watch_home(&root, move |changed, _| {
            let content = std::fs::read_to_string(&replacement_file).ok();
            let _ = tx.send((changed, content));
        }).unwrap();
        let wait = |expected: &[&str]| {
            let mut missing: HashSet<PathBuf> = expected.iter().map(PathBuf::from).collect();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !missing.is_empty() {
                let (changed, _) = events.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .unwrap_or_else(|e| panic!("missing {missing:?}: {e}"));
                for path in changed { missing.remove(&path); }
            }
        };
        std::fs::write(root.join("mods/ordinary/main.js"), "ordinary").unwrap();
        wait(&["mods/ordinary/main.js"]);
        std::fs::write(source.join("main.js"), "early").unwrap();
        wait(&["mods/early/main.js"]);
        symlink(&source, root.join("mods/late")).unwrap();
        wait(&["mods/late"]);
        std::fs::write(source.join("lib/message.js"), "dependency").unwrap();
        wait(&["mods/early/lib/message.js", "mods/late/lib/message.js"]);
        std::fs::write(source.join(".saved.js"), "new entry").unwrap();
        std::fs::rename(source.join(".saved.js"), source.join("main.js")).unwrap();
        wait(&["mods/early/main.js", "mods/late/main.js"]);
        let other = scratch.join("other");
        std::fs::create_dir(&other).unwrap();
        symlink(&other, root.join("mods/.replacement")).unwrap();
        std::fs::rename(root.join("mods/.replacement"), root.join("mods/late")).unwrap();
        wait(&["mods/late"]);
        std::fs::write(other.join("main.js"), "repointed").unwrap();
        wait(&["mods/late/main.js"]);
        // Replacing the target directory must re-arm watches on its new inode.
        std::fs::rename(&other, scratch.join("old-other")).unwrap();
        wait(&["mods/late"]);
        std::fs::create_dir(&other).unwrap();
        wait(&["mods/late"]);
        std::fs::write(other.join("after.js"), "replacement").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let (changed, content) = events.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).unwrap();
            // A backend can invalidate the replaced root instead of its new child.
            // Capture the content at delivery so an older queued event cannot pass.
            if content.as_deref() == Some("replacement")
                && changed.iter().any(|path| Path::new("mods/late/after.js").starts_with(path))
            {
                break;
            }
        }
        drop(stop);
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn source_revisions_survive_closed_copies_and_rescan_invalidates_all() {
        let (_, changes) = async_channel::unbounded();
        let events = FileEvents {
            changes,
            root: PathBuf::from("/home/probe"),
            revisions: Default::default(),
        };
        let revision = events.revision_provider();
        let first = Path::new("/home/probe/mods/one");
        let other = Path::new("/home/probe/mods/other");
        assert_eq!(revision(first), Some(0));
        assert_eq!(revision(other), Some(0));
        events.revisions.lock().unwrap().changed(&vec!["mods/one/main.js".into()], false);
        assert_eq!(revision(first), Some(1));
        assert_eq!(revision(other), Some(0));
        // No copy registration is needed for a root edited while closed.
        events.revisions.lock().unwrap().changed(&vec!["mods/closed/main.js".into()], false);
        assert_eq!(revision(Path::new("/home/probe/mods/closed")), Some(2));
        let changed = Vec::new();
        events.revisions.lock().unwrap().changed(&changed, true);
        assert!(revision(first).unwrap() > 2);
        assert!(revision(other).unwrap() > 2);
        assert!(changed.is_empty(), "a rescan must not request mod hot reloads");
        assert_eq!(revision(Path::new("/outside")), None);
    }

    #[test]
    fn unattributed_events_revalidate_code_without_reloading_mods() {
        let base = Path::new("/home/probe");
        let mut paths = vec![base.to_path_buf(), base.join("mods"), base.parent().unwrap().to_path_buf()];
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            paths.push(base.join("mods").join(std::ffi::OsString::from_vec(vec![0xff])));
        }
        let mut events: Vec<notify::Result<notify::Event>> = paths.into_iter()
            .map(|path| Ok(notify::Event::new(notify::EventKind::Any).add_path(path))).collect();
        events.push(Err(notify::Error::generic("missed events")));
        events.push(Ok(notify::Event::new(notify::EventKind::Any).set_flag(notify::event::Flag::Rescan)));
        for event in events {
            let mut revisions = Revisions::default();
            revisions.roots.insert("mods/one".into(), 0);
            revisions.roots.insert("mods/two".into(), 0);
            let (changed, rescan) = changes(base, event);
            revisions.changed(&changed, rescan);
            assert!(revisions.all > 0);
            assert!(!changed.contains(&PathBuf::from("mods/one")));
            assert!(!changed.contains(&PathBuf::from("mods/two")));
        }
    }

    #[test]
    fn rescan_keeps_real_edits_without_reloading_other_mods() {
        let base = Path::new("/home/probe");
        let mut revisions = Revisions::default();
        revisions.roots.insert("mods/one".into(), 0);
        revisions.roots.insert("mods/two".into(), 0);
        let event = notify::Event::new(notify::EventKind::Any)
            .set_flag(notify::event::Flag::Rescan)
            .add_path(base.join("mods/one/main.js"));
        let (changed, rescan) = changes(base, Ok(event));
        revisions.changed(&changed, rescan);
        assert_eq!(changed, vec![PathBuf::from("mods/one/main.js")]);
        assert!(revisions.all > 0);
        assert!(revisions.roots[Path::new("mods/one")] > revisions.all);
        assert_eq!(revisions.roots[Path::new("mods/two")], 0);
    }

    #[test]
    fn revision_provider_does_not_keep_receiver_alive_and_recovers_poison() {
        let (tx, changes) = async_channel::unbounded();
        let events = FileEvents {
            changes,
            root: PathBuf::from("/home/probe"),
            revisions: Default::default(),
        };
        let revision = events.revision_provider();
        let state = events.revisions.clone();
        drop(events);
        assert!(tx.is_closed());
        assert!(std::panic::catch_unwind(|| {
            let _lock = state.lock().unwrap();
            panic!("poison revision state");
        }).is_err());
        assert_eq!(revision(Path::new("/home/probe/mods/one")), Some(0));
    }

    #[gpui::test]
    fn native_notifications_reach_a_waiting_test_receiver(cx: &mut gpui::TestAppContext) {
        let (tx, rx) = async_channel::unbounded();
        let pending = test_notifications(tx, cx.background_executor.clone());
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let received = result.clone();
        cx.background_executor.spawn(async move {
            *received.lock().unwrap() = rx.recv().await.ok();
        }).detach();
        cx.run_until_parked();
        let paths = vec![PathBuf::from("settings.json")];
        let sent = paths.clone();
        std::thread::spawn(move || pending.send(sent).unwrap()).join().unwrap();
        cx.executor().advance_clock(std::time::Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(*result.lock().unwrap(), Some(paths));
    }

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
        assert!(watched(Path::new("mods/probe/target/output.js")));
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
    fn reclaiming_idle_folders_keeps_live_waiters_and_stops_idle_workers() {
        let mut all = std::collections::HashMap::new();
        let mut live = Vec::new();
        let mut workers = Vec::new();
        for i in 0..MAX_FOLDERS {
            let (stop, stopped) = std::sync::mpsc::channel();
            let (tx, rx) = async_channel::bounded(1);
            if i % 3 == 0 {
                drop(rx); // A mod copy closed or its host future was cancelled.
            } else if i % 3 == 1 {
                live.push(rx);
            } else {
                tx.try_send(vec!["changed".into()]).unwrap();
                drop(tx); // The batch was delivered; nothing awaits more changes.
                all.insert(PathBuf::from(i.to_string()), Folder {
                    _watcher: stop, generation: Arc::new(()), waiters: Vec::new(),
                    pending: HashSet::from(["another change".into()]),
                });
                workers.push(stopped);
                continue;
            }
            all.insert(PathBuf::from(i.to_string()), Folder {
                _watcher: stop, generation: Arc::new(()), waiters: vec![tx], pending: HashSet::new(),
            });
            workers.push(stopped);
        }
        reclaim_idle(&mut all);
        assert_eq!(all.len(), live.len());
        for (i, stopped) in workers.iter().enumerate() {
            assert_eq!(stopped.try_recv().unwrap_err(), if i % 3 == 1 {
                std::sync::mpsc::TryRecvError::Empty
            } else {
                std::sync::mpsc::TryRecvError::Disconnected
            });
        }
        // Genuine demand remains bounded; reclamation must not interrupt it.
        reclaim_idle(&mut all);
        assert_eq!(all.len(), live.len());
        drop(live);
        reclaim_idle(&mut all);
        assert!(all.is_empty());
    }

    #[test]
    fn retired_folder_workers_cannot_touch_replacements() {
        let root = test_repo("replacement");
        let retired = Arc::new(());
        let generation = Arc::new(());
        let (stop, _stopped) = std::sync::mpsc::channel();
        let (tx, rx) = async_channel::bounded(1);
        folders().insert(root.clone(), Folder {
            _watcher: stop, generation: generation.clone(), waiters: vec![tx], pending: HashSet::new(),
        });
        remove_folder(&root, &retired);
        assert!(!deliver_pending(&root, &retired, HashSet::from(["stale".into()])));
        assert!(rx.try_recv().is_err());
        assert!(deliver_pending(&root, &generation, HashSet::from(["fresh".into()])));
        assert_eq!(rx.try_recv().unwrap(), vec!["fresh".to_owned()]);
        remove_folder(&root, &generation);
        assert!(!folders().contains_key(&root));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn abandoned_waiters_do_not_leave_an_unbounded_queue() {
        let root = test_repo("overflow");
        install_watch(&root);
        // Inject worker batches: native backends may coalesce real writes, so a
        // filesystem flood cannot deterministically exercise queue overflow.
        let generation = folders().get(&root).unwrap().generation.clone();
        for i in 0..6000 {
            assert!(deliver_pending(&root, &generation, HashSet::from([root.join(format!("file-{i}")).to_string_lossy().into_owned()])));
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
