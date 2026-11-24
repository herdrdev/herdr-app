use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(super) struct EvaluatedCode {
    pub(super) class: Persistent<Object<'static>>,
    pub(super) lease: ApplicationModuleLease,
    pub(super) _application: Rc<ApplicationGeneration>,
}

pub(super) struct CachedCode {
    pub(super) result: std::result::Result<EvaluatedCode, String>,
    pub(super) sources: Rc<SourceFiles>,
}

#[derive(Default)]
pub(super) struct SourceFiles {
    files: RefCell<BTreeMap<std::path::PathBuf, [u8; 32]>>,
    pub(super) transient_failure: Cell<bool>,
}

impl SourceFiles {
    pub(super) fn record(&self, path: &Path) {
        self.files
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| stamp(path));
    }

    pub(super) fn current(&self) -> bool {
        self.files
            .borrow()
            .iter()
            .all(|(path, previous)| stamp(path) == *previous)
    }
}

pub(super) fn fingerprint(
    root: &Path,
    entry: &str,
    dependencies: &BTreeMap<String, MaterializedDependency>,
    revision: Option<u64>,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    field(&mut hash, root.as_os_str().as_encoded_bytes());
    field(&mut hash, entry.as_bytes());
    field(&mut hash, host_modules::link_key().as_bytes());
    field(
        &mut hash,
        format!("{:?}", scope::policy().capabilities()).as_bytes(),
    );
    if let Some(revision) = revision {
        hash.update([1]);
        hash.update(revision.to_le_bytes());
    } else {
        hash.update([0]);
        tree(root, &mut hash);
    }
    // Materialized Git roots include the immutable commit identity. Only files
    // actually imported need re-stating; never walk the dependency checkout.
    for (name, dependency) in dependencies {
        field(&mut hash, name.as_bytes());
        field(&mut hash, dependency.root.as_os_str().as_encoded_bytes());
        field(&mut hash, dependency.entry.as_os_str().as_encoded_bytes());
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Tree {
        path: std::path::PathBuf,
        _directory: tempfile::TempDir,
    }
    impl Tree {
        fn new() -> Self {
            #[cfg(target_os = "linux")]
            let scratch = std::path::PathBuf::from("/var/tmp");
            #[cfg(not(target_os = "linux"))]
            let scratch = std::env::temp_dir();
            let directory = tempfile::Builder::new()
                .prefix("shell-code-cache-")
                .tempdir_in(scratch)
                .unwrap();
            Self { path: directory.path().to_path_buf(), _directory: directory }
        }
        fn write(&self, name: &str, source: impl AsRef<[u8]>) {
            let path = self.path.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
    }

    fn value(runtime: &Rc<ShellRuntime>, tree: &Tree) -> String {
        let class = runtime.load_app(&tree.path, "main.js").unwrap();
        runtime
            .with_js(|ctx| class.value.clone().restore(ctx)?.get("value"))
            .unwrap()
    }

    #[test]
    fn imported_files_in_ignored_directories_and_missed_events_are_revalidated() {
        for watched in [false, true] {
            for directory in ["target", "node_modules", ".private"] {
                let runtime = ShellRuntime::new_isolated().unwrap();
                if watched {
                    runtime.set_source_revision_provider(|_| Some(0));
                }
                let tree = Tree::new();
                tree.write("main.js", format!("import {{value}} from './{directory}/part.js'; export default class {{ static value = value; }}"));
                tree.write(
                    &format!("{directory}/part.js"),
                    "export const value = 'one';",
                );
                assert_eq!(value(&runtime, &tree), "one");
                tree.write(
                    &format!("{directory}/part.js"),
                    "export const value = 'second';",
                );
                assert_eq!(value(&runtime, &tree), "second");
                tree.write(
                    "main.js",
                    "export default class { static value = 'new entry'; }",
                );
                assert_eq!(value(&runtime, &tree), "new entry");
            }
        }
    }

    #[test]
    fn hidden_state_and_editor_files_do_not_mint_code_revisions() {
        let runtime = ShellRuntime::new_isolated().unwrap();
        let tree = Tree::new();
        tree.write("main.js", "globalThis.loads = (globalThis.loads || 0) + 1; export default class { static value = String(globalThis.loads); }");
        for i in 0..20 {
            tree.write(".state", i.to_string());
            tree.write("main.js~", i.to_string());
            tree.write(".main.js.swp", i.to_string());
            assert_eq!(value(&runtime, &tree), "1");
        }
        assert_eq!(runtime.app_modules.next_generation.get(), 1);
    }

    #[test]
    fn watcher_revision_invalidates_assets_without_a_tree_walk() {
        let runtime = ShellRuntime::new_isolated().unwrap();
        let revision = Rc::new(Cell::new(0));
        let provider = revision.clone();
        runtime.set_source_revision_provider(move |_| Some(provider.get()));
        let tree = Tree::new();
        tree.write("main.js", "globalThis.loads = (globalThis.loads || 0) + 1; export default class { static value = String(globalThis.loads); }");
        assert_eq!(value(&runtime, &tree), "1");
        tree.write("asset.bin", "changed but no watcher event yet");
        assert_eq!(value(&runtime, &tree), "1");
        revision.set(1);
        assert_eq!(value(&runtime, &tree), "2");
        assert_eq!(value(&runtime, &tree), "2");
    }

    #[test]
    fn interrupted_and_out_of_memory_evaluations_can_retry_unchanged_source() {
        for oom in [false, true] {
            let runtime = ShellRuntime::new_isolated().unwrap();
            let tree = Tree::new();
            tree.write(
                "main.js",
                "for(let i=0;i<100000;i++) {} export default class { static value = 'ok'; }",
            );
            if oom {
                runtime.js_runtime.set_memory_limit(1);
            } else {
                runtime
                    .js_runtime
                    .set_interrupt_handler(Some(Box::new(|| true)));
            }
            assert!(runtime.load_app(&tree.path, "main.js").is_err());
            assert!(
                runtime.code_cache.borrow().is_empty(),
                "transient failure must not be cached"
            );
            runtime
                .js_runtime
                .set_memory_limit(sandbox::memory_limit_bytes());
            runtime
                .js_runtime
                .set_interrupt_handler(Some(Box::new(sandbox::interrupt_handler())));
            assert_eq!(value(&runtime, &tree), "ok");
        }
    }

    #[test]
    fn deterministic_source_errors_are_cached_and_edits_recover() {
        for oversized in [false, true] {
            let runtime = ShellRuntime::new_isolated().unwrap();
            let tree = Tree::new();
            tree.write("main.js", "import './part.js'; export default class { static value = 'ok'; }");
            tree.write("part.js", [0xff, 0xfe]);
            if oversized {
                std::fs::File::create(tree.path.join("part.js")).unwrap()
                    .set_len(MAX_MODULE_BYTES + 1).unwrap();
            }
            for _ in 0..20 {
                assert!(runtime.load_app(&tree.path, "main.js").is_err());
            }
            assert_eq!(runtime.app_modules.next_generation.get(), 1);
            tree.write("part.js", "// repaired");
            assert_eq!(value(&runtime, &tree), "ok");
        }
    }

    #[cfg(unix)]
    #[test]
    fn denied_source_is_cached_and_permission_changes_invalidate() {
        use std::os::unix::fs::PermissionsExt;
        let runtime = ShellRuntime::new_isolated().unwrap();
        let tree = Tree::new();
        tree.write("main.js", "import './part.js'; export default class { static value = 'ok'; }");
        tree.write("part.js", "// valid source");
        let path = tree.path.join("part.js");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
        for _ in 0..20 {
            assert!(runtime.load_app(&tree.path, "main.js").is_err());
        }
        assert_eq!(runtime.app_modules.next_generation.get(), 1);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(value(&runtime, &tree), "ok");
    }

    #[test]
    fn source_io_caches_only_known_deterministic_kinds() {
        use std::io::{Error, ErrorKind};
        for kind in [ErrorKind::Interrupted, ErrorKind::WouldBlock, ErrorKind::TimedOut, ErrorKind::OutOfMemory, ErrorKind::ResourceBusy, ErrorKind::Other, ErrorKind::Unsupported, ErrorKind::UnexpectedEof] {
            assert!(transient_source_io(&Error::from(kind)), "{kind:?}");
        }
        for kind in [ErrorKind::InvalidData, ErrorKind::PermissionDenied, ErrorKind::NotFound] {
            assert!(!transient_source_io(&Error::from(kind)), "{kind:?}");
        }
        #[cfg(unix)]
        for code in [libc::EMFILE, libc::ENFILE, libc::EIO] {
            assert!(transient_source_io(&Error::from_raw_os_error(code)), "{code}");
        }
        #[cfg(windows)]
        for code in [32, 33] {
            assert!(transient_source_io(&Error::from_raw_os_error(code)));
        }
    }

    #[test]
    fn source_io_failures_recover_without_source_or_revision_changes() {
        let mut errors = vec![std::io::Error::from(std::io::ErrorKind::Other)];
        #[cfg(unix)]
        errors.extend([libc::EMFILE, libc::ENFILE, libc::EIO].map(std::io::Error::from_raw_os_error));
        for error in errors {
            let runtime = ShellRuntime::new_isolated().unwrap();
            runtime.set_source_revision_provider(|_| Some(0));
            let tree = Tree::new();
            tree.write("main.js", "import './part.js'; export default class { static value = 'ok'; }");
            tree.write("part.js", "// unchanged dependency");
            let sources = SourceFiles::default();
            sources.record(&tree.path.join("main.js"));
            sources.record(&tree.path.join("part.js"));
            SOURCE_READ_FAILURE.with(|failure| {
                *failure.borrow_mut() = Some((tree.path.join("part.js").canonicalize().unwrap(), error));
            });
            assert!(runtime.load_app(&tree.path, "main.js").is_err());
            assert!(SOURCE_READ_FAILURE.with(|failure| failure.borrow().is_none()));
            assert!(runtime.code_cache.borrow().is_empty(), "unknown IO must not poison the revision");
            assert!(sources.current(), "source stamps did not change");
            assert_eq!(value(&runtime, &tree), "ok");
            assert!(sources.current(), "recovery needed no edit");
            assert_eq!(value(&runtime, &tree), "ok");
            assert_eq!(runtime.app_modules.next_generation.get(), 2);
        }
    }

    #[test]
    fn syntax_link_thrown_and_stack_overflow_errors_are_cached() {
        for source in [
            "export default ;",
            "import './missing.js'; export default class {}",
            "throw Error('bad code'); export default class {}",
            "throw 'bad code'; export default class {}",
            "throw {}; export default class {}",
            "function recurse() { recurse(); } recurse(); export default class {}",
        ] {
            let runtime = ShellRuntime::new_isolated().unwrap();
            let tree = Tree::new();
            tree.write("main.js", source);
            for _ in 0..20 {
                assert!(runtime.load_app(&tree.path, "main.js").is_err());
            }
            assert_eq!(runtime.app_modules.next_generation.get(), 1, "{source}");
        }
    }
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn stamp(path: &Path) -> [u8; 32] {
    let mut hash = Sha256::new();
    match std::fs::metadata(path) {
        Ok(metadata) => {
            hash.update(metadata.len().to_le_bytes());
            field(
                &mut hash,
                format!("{:?}", metadata.permissions()).as_bytes(),
            );
            field(&mut hash, format!("{:?}", metadata.modified()).as_bytes());
        }
        Err(error) => field(&mut hash, format!("{:?}", error.kind()).as_bytes()),
    }
    hash.finalize().into()
}

fn tree(root: &Path, hash: &mut Sha256) {
    let mut pending = BTreeSet::from([(root.to_path_buf(), 0usize)]);
    let mut visited = BTreeSet::from([root.to_path_buf()]);
    let mut count = 0;
    while let Some((directory, depth)) = pending.pop_first() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                field(hash, format!("{:?}", error.kind()).as_bytes());
                continue;
            }
        };
        let mut sorted = Vec::new();
        for entry in entries.take(4096 - count) {
            count += 1;
            match entry {
                Ok(entry) => sorted.push(entry),
                Err(error) => field(hash, format!("{:?}", error.kind()).as_bytes()),
            }
        }
        let mut entries = sorted;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path);
            if !crate::application_file_counts(relative) {
                continue;
            }
            field(hash, relative.as_os_str().as_encoded_bytes());
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => {
                    field(hash, format!("{:?}", error.kind()).as_bytes());
                    continue;
                }
            };
            let directory = if kind.is_symlink() {
                match path.canonicalize() {
                    Ok(target) if target.starts_with(root) => {
                        field(hash, target.as_os_str().as_encoded_bytes());
                        target.is_dir().then_some(target)
                    }
                    _ => {
                        field(hash, b"unresolved-or-outside");
                        continue;
                    }
                }
            } else {
                kind.is_dir().then(|| path.clone())
            };
            if let Some(directory) = directory {
                if depth < 8 && visited.insert(directory.clone()) {
                    pending.insert((directory, depth + 1));
                }
            } else {
                hash.update(stamp(&path));
            }
        }
        if count == 4096 {
            field(hash, b"scan-limit");
            return;
        }
    }
}
