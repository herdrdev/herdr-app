//! Files dropped from a macOS sandboxed drag, such as a live screenshot
//! thumbnail, arrive as paths in `.../TemporaryItems/NSIRD_*/` that only the
//! process receiving the drop may read. The programs in Herdr's terminals run
//! under herdr-core, so the app copies those files into a private folder and
//! hands over the copy. On other systems, and for other paths, nothing changes.

/// `paths`, each sandboxed drop swapped for a private copy.
#[cfg(not(target_os = "macos"))]
pub fn stage(paths: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    paths
}

/// Starts the hourly removal of old copies.
#[cfg(not(target_os = "macos"))]
pub fn start() {}

#[cfg(target_os = "macos")]
pub use mac::{stage, start};

#[cfg(all(test, not(target_os = "macos")))]
#[test]
fn elsewhere_drops_are_left_alone() {
    start();
    let path = std::path::PathBuf::from("/tmp/TemporaryItems/NSIRD_x/a.png");
    assert_eq!(stage(vec![path.clone()]), [path]);
}

#[cfg(target_os = "macos")]
mod mac {
    use std::io::{Error, ErrorKind, Read};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    /// `paths`, each sandboxed drop swapped for a private copy (or kept if
    /// copying fails).
    pub fn stage(paths: Vec<PathBuf>) -> Vec<PathBuf> {
        stage_with(paths, &dir(), is_sandboxed)
    }

    /// Hourly, while the app runs, removes day-old copies.
    pub fn start() {
        prune_every(dir(), std::time::Duration::from_secs(3600));
    }

    fn prune_every(dir: PathBuf, every: std::time::Duration) {
        std::thread::spawn(move || loop {
            let _ = prune(&dir);
            std::thread::sleep(every);
        });
    }

    fn dir() -> PathBuf {
        std::env::temp_dir().join("herdr-drops")
    }

    /// A file directly in a sandboxed drag folder, `.../TemporaryItems/NSIRD_*/`.
    fn is_sandboxed(path: &Path) -> bool {
        path.components().zip(path.components().skip(1)).any(|(a, b)| a.as_os_str() == "TemporaryItems" && b.as_os_str().as_encoded_bytes().starts_with(b"NSIRD_"))
    }

    fn stage_with(paths: Vec<PathBuf>, dir: &Path, staged: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        let copy = |p: PathBuf| copy_into(&p, dir, 256 << 20).unwrap_or_else(|e| {
            tracing::warn!("cannot copy dropped file {}: {e}", p.display());
            p
        });
        paths.into_iter().map(|p| if staged(&p) { copy(p) } else { p }).collect()
    }

    /// Checks `dir` is ours alone (a 0700 directory, not a link), then removes
    /// the copies there over a day old.
    fn prune(dir: &Path) -> std::io::Result<()> {
        let d = std::fs::symlink_metadata(dir)?;
        if !d.is_dir() || d.uid() != unsafe { libc::geteuid() } || d.mode() & 0o777 != 0o700 {
            return Err(Error::other("drop folder is not our own private directory"));
        }
        for e in std::fs::read_dir(dir)?.flatten() {
            let old = e.metadata().ok().filter(|m| m.is_file()).and_then(|m| m.modified().ok()?.elapsed().ok());
            if e.file_name().to_string_lossy().starts_with("herdr-drop-") && old.is_some_and(|a| a.as_secs() > 86_400) {
                let _ = std::fs::remove_file(e.path());
            }
        }
        Ok(())
    }

    /// Copies `src` into `dir` (made ours, see `prune`) as
    /// `herdr-drop-<nanos>[.ext]`, 0600, a name safe to type into a shell.
    /// Refuses links, non-files and files over `cap` bytes, leaving no partial copy.
    fn copy_into(src: &Path, dir: &Path, cap: u64) -> std::io::Result<PathBuf> {
        if let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(dir)
            && e.kind() != ErrorKind::AlreadyExists
        {
            return Err(e);
        }
        prune(dir)?;
        // Read here, in the app, which holds the drop's access; the core can't.
        let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(src)?;
        let ext = src.extension().and_then(|e| e.to_str()).filter(|e| e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()));
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let mut dest = dir.join(format!("herdr-drop-{stamp}"));
        if let Some(ext) = ext {
            dest.set_extension(ext);
        }
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&dest)?;
        let copied = file.metadata().and_then(|m| {
            if !m.is_file() || m.len() > cap {
                return Err(Error::other("not a regular file under the size cap"));
            }
            let n = std::io::copy(&mut file.take(cap + 1), &mut out)?;
            if n > cap { Err(Error::other("file grew past the size cap")) } else { Ok(dest.clone()) }
        });
        if copied.is_err() {
            let _ = std::fs::remove_file(&dest);
        }
        copied
    }

    #[cfg(test)]
    mod tests {
        use std::path::Path;

        #[test]
        fn sandboxed_drag_paths_are_recognised() {
            for p in [
                "/var/folders/ab/xyz/T/TemporaryItems/NSIRD_screencaptureui_Q1w2E3/Screenshot 2026-11-02 at 3.10.04\u{202f}PM.png",
                "/T/TemporaryItems/NSIRD_other_x/a.png",
            ] {
                assert!(super::is_sandboxed(Path::new(p)), "{p}");
            }
        }

        #[test]
        fn other_paths_are_not_sandboxed_drags() {
            for p in [
                "/Users/me/Desktop/Screenshot 2026-11-02 at 3.10.04\u{202f}PM.png",
                "/var/folders/ab/T/TemporaryItems/other/a.png",
                "/var/folders/ab/T/NSIRD_screencaptureui_x/a.png",
                "/var/folders/ab/T/TemporaryItems/xNSIRD_x/a.png",
                "/var/folders/ab/T/TemporaryItems/sub/NSIRD_x/a.png",
                "NSIRD_screencaptureui_x/TemporaryItems/a.png",
                "",
            ] {
                assert!(!super::is_sandboxed(Path::new(p)), "{p}");
            }
            use std::os::unix::ffi::OsStrExt;
            let between = Path::new("/T/TemporaryItems").join(std::ffi::OsStr::from_bytes(b"\xff")).join("NSIRD_x/a.png");
            assert!(!super::is_sandboxed(&between), "a non-UTF-8 folder in between");
        }

        /// A fresh scratch directory; never named `TemporaryItems`, which macOS won't let us remove.
        fn scratch(name: &str) -> std::path::PathBuf {
            let root = std::env::temp_dir().join(format!("herdr-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir(&root).unwrap();
            root
        }

        #[test]
        fn copy_into_makes_a_private_safely_named_copy() {
            use std::os::unix::fs::PermissionsExt;
            let root = scratch("copy");
            let drops = root.join("drops");
            let src = root.join("a\nb; rm -rf ~.png");
            std::fs::write(&src, b"png-bytes").unwrap();
            let copy = super::copy_into(&src, &drops, 1024).unwrap();
            let name = copy.file_name().unwrap().to_str().unwrap();
            assert!(copy.starts_with(&drops) && name.strip_prefix("herdr-drop-").and_then(|n| n.strip_suffix(".png")).is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())), "{name}");
            assert_eq!(std::fs::read(&copy).unwrap(), b"png-bytes");
            assert_eq!(std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&drops).unwrap().permissions().mode() & 0o777, 0o700);
            let odd = root.join("x.p$g");
            std::fs::write(&odd, b"x").unwrap();
            assert!(super::copy_into(&odd, &drops, 1024).unwrap().extension().is_none(), "an unsafe extension is dropped");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn copy_into_refuses_links_fifos_and_big_files_without_leftovers() {
            let root = scratch("refuse");
            let drops = root.join("drops");
            let real = root.join("real.png");
            std::fs::write(&real, b"0123456789").unwrap();
            let link = root.join("link.png");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let fifo = root.join("fifo.png");
            let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            assert!(super::copy_into(&link, &drops, 1024).is_err(), "symlink");
            assert!(super::copy_into(&fifo, &drops, 1024).is_err(), "fifo, without blocking on a writer");
            assert!(super::copy_into(&real, &drops, 9).is_err(), "over the cap");
            assert_eq!(std::fs::read_dir(&drops).unwrap().count(), 0, "no partial copy is left");
            let fake = root.join("fake-drops");
            std::os::unix::fs::symlink(&drops, &fake).unwrap();
            assert!(super::copy_into(&real, &fake, 1024).is_err(), "a symlinked drop folder");
            let open = root.join("open");
            std::fs::create_dir(&open).unwrap();
            std::fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            assert!(super::copy_into(&real, &open, 1024).is_err(), "a drop folder others can read");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn copy_into_prunes_only_day_old_copies() {
            let root = scratch("prune");
            let drops = root.join("drops");
            let src = root.join("a.png");
            std::fs::write(&src, b"x").unwrap();
            let fresh = super::copy_into(&src, &drops, 1024).unwrap();
            let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
            for name in ["herdr-drop-1.png", "other.png"] {
                std::fs::File::create(drops.join(name)).unwrap().set_modified(two_days_ago).unwrap();
            }
            super::copy_into(&src, &drops, 1024).unwrap();
            assert!(!drops.join("herdr-drop-1.png").exists(), "a day-old copy goes");
            assert!(fresh.exists() && drops.join("other.png").exists(), "fresh copies and other files stay");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn the_pruner_keeps_running_and_removes_day_old_copies() {
            let root = scratch("pruner");
            let drops = root.join("drops");
            std::fs::create_dir(&drops).unwrap();
            std::fs::set_permissions(&drops, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
            let old = drops.join("herdr-drop-1.png");
            let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
            std::fs::File::create(&old).unwrap().set_modified(two_days_ago).unwrap();
            super::prune_every(drops.clone(), std::time::Duration::from_millis(10));
            let gone = (0..200).any(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                !old.exists()
            });
            assert!(gone, "the pruner removed the day-old copy");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn start_prunes_the_real_drop_folder() {
            use std::os::unix::fs::DirBuilderExt;
            let dir = super::dir();
            let _ = std::fs::DirBuilder::new().mode(0o700).create(&dir);
            let old = dir.join(format!("herdr-drop-test-{}", std::process::id()));
            let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
            std::fs::File::create(&old).unwrap().set_modified(two_days_ago).unwrap();
            super::start();
            let gone = (0..200).any(|_| {
                std::thread::sleep(std::time::Duration::from_millis(10));
                !old.exists()
            });
            assert!(gone, "the pruner started at launch removed the day-old copy");
        }

        #[test]
        fn other_drops_pass_through_untouched() {
            let plain = std::path::PathBuf::from("/Users/me/Desktop/a b.png");
            assert_eq!(super::stage(vec![plain.clone()]), [plain]);
            assert!(super::dir().ends_with("herdr-drops"));
        }

        #[test]
        fn only_sandboxed_drops_are_swapped_for_copies() {
            let root = scratch("stage");
            let drops = root.join("drops");
            let thumb = root.join("shot.png");
            std::fs::write(&thumb, b"x").unwrap();
            let plain = root.join("a b.txt");
            let staged = super::stage_with(vec![thumb.clone(), plain.clone()], &drops, |p| p == thumb);
            let copy = std::fs::read_dir(&drops).unwrap().next().unwrap().unwrap().path();
            assert_eq!(staged, [copy, plain]);
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
