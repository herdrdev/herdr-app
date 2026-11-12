//! Assets an application ships with itself.
//!
//! `svg(path)` names a file, and the file has to come from somewhere. It comes
//! from the application directory and nowhere else — the same root that bounds
//! module resolution — so an application carries its own icons and cannot read
//! an image from outside the directory the user pointed the runtime at.
//!
//! Note the asymmetry, because it surprises people: `import "./counter.js"`
//! resolves against the *importing file*, the way every JavaScript module
//! system does, while `svg("icons/check.svg")` resolves against the
//! *application root*, the way a web application's public directory does. A
//! runtime cannot tell which module called `svg`, so per-file asset paths are
//! not available to it. The rule is therefore stated in the README, and a
//! missing asset says exactly where it was looked for rather than drawing
//! nothing.

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashSet, VecDeque},
    path::PathBuf,
};

use cap_std::{ambient_authority, fs::Dir};
use gpui::{AssetSource, SharedString};

const MAX_ASSET_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ASSET_LIST_ENTRIES: usize = 10_000;
const MAX_ASSET_LIST_NAME_BYTES: usize = 1024 * 1024;
const MAX_REPORTED_MISSING_ASSETS: usize = 256;

/// Serves files from one application directory.
#[derive(Clone, Debug)]
pub struct AppAssets {
    root: PathBuf,
    follow_links: bool,
}

impl AppAssets {
    pub fn new(root: PathBuf) -> Self {
        Self { root, follow_links: false }
    }

    /// HERDR: also serves a file that a link inside the directory points outside it
    /// (dotfiles, Nix and Home Manager link the user's files in). For a host
    /// whose scripts already have more than that, such as running programs, the
    /// refusal protects nothing and breaks those setups. `..` stays refused.
    pub fn following_links(mut self) -> Self {
        self.follow_links = true;
        self
    }

    /// The handle refused because a link leads out of the directory: with
    /// `follow_links`, the plain path, when it is only names (a Windows prefix
    /// like `C:` or `\\server` is refused the same way, and would replace the root).
    fn escaped(&self, error: &std::io::Error, resolved: &std::path::Path) -> Option<PathBuf> {
        let names = resolved.components().all(|c| matches!(c, std::path::Component::Normal(_)));
        (self.follow_links && names && error.kind() == std::io::ErrorKind::PermissionDenied).then(|| self.root.join(resolved))
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// The cache identity of a local asset, using the same spelling as its
    /// resolved path. This is lexical so missing/deleted files keep their key.
    pub fn cache_key(path: &str) -> SharedString {
        Self::relative_path(path)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned())
            .into()
    }

    fn relative_path(path: &str) -> Option<PathBuf> {
        let mut resolved = PathBuf::new();
        for component in std::path::Path::new(path).components() {
            match component {
                std::path::Component::ParentDir => return None,
                std::path::Component::CurDir | std::path::Component::RootDir => {}
                other => resolved.push(other),
            }
        }
        (!resolved.as_os_str().is_empty()).then_some(resolved)
    }

    /// Opens the application directory and answers the path within it.
    ///
    /// The same shape the `fs` capability uses, and for the same reason: a
    /// handle that cannot be made to name something outside itself, rather than
    /// a string that has to be judged and then trusted. An `icons` that is a
    /// link somewhere else is lexically inside the root and reads from outside
    /// it, and a link that appears between the judging and the reading is worse
    /// still.
    ///
    /// The lexical pass stays as the cheap half: it turns `../..` into a refusal
    /// here rather than an `errno` from below.
    fn resolve(&self, path: &str) -> Option<(Dir, PathBuf)> {
        let resolved = Self::relative_path(path)?;
        let dir = Dir::open_ambient_dir(&self.root, ambient_authority()).ok()?;
        Some((dir, resolved))
    }
}

impl AssetSource for AppAssets {
    fn image_resource(&self, resource: &gpui::Resource) -> gpui::Resource {
        match resource {
            // Path reads have no size limit; embedded reads enforce the mod limit.
            gpui::Resource::Embedded(path) => gpui::Resource::Embedded(Self::cache_key(path)),
            _ => resource.clone(),
        }
    }

    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let Some((dir, resolved)) = self.resolve(path) else {
            anyhow::bail!("`{path}` is outside the application directory");
        };

        let opened = match dir.open(&resolved) {
            Err(error) => match self.escaped(&error, &resolved) {
                Some(path) => std::fs::File::open(path).map(cap_std::fs::File::from_std),
                None => Err(error),
            },
            ok => ok,
        };
        match opened {
            Ok(mut file) => {
                use std::io::Read as _;

                let size = file.metadata()?.len();
                if size > MAX_ASSET_BYTES {
                    anyhow::bail!(
                        "asset `{path}` is {size} bytes, over the {MAX_ASSET_BYTES}-byte limit"
                    );
                }
                let mut bytes = Vec::with_capacity(size as usize);
                file.by_ref()
                    .take(MAX_ASSET_BYTES + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_ASSET_BYTES {
                    anyhow::bail!("asset `{path}` grew over the {MAX_ASSET_BYTES}-byte limit");
                }
                Ok(Some(Cow::Owned(bytes)))
            }
            // A missing asset cannot be an error: GPUI asks for assets it may
            // not need, and returning one would fail the frame. But an icon
            // that silently does not appear is the hardest kind of mistake to
            // find, so it is reported — once per path, because this runs on
            // every paint.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                report_missing(path, &self.root.join(&resolved));
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let Some((dir, resolved)) = self.resolve(path) else {
            return Ok(Vec::new());
        };

        let mut names = Vec::new();
        let mut name_bytes = 0;
        let listing: Box<dyn Iterator<Item = std::io::Result<std::ffi::OsString>>> = match dir.read_dir(&resolved) {
            Ok(listing) => Box::new(listing.map(|e| e.map(|e| e.file_name()))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => match self.escaped(&error, &resolved) {
                Some(path) => Box::new(std::fs::read_dir(path)?.map(|e| e.map(|e| e.file_name()))),
                None => return Err(error.into()),
            },
        };
        for entry in listing {
            let name = entry?.to_string_lossy().into_owned();
            name_bytes += name.len();
            check_asset_list_budget(names.len(), name_bytes)
                .map_err(|error| anyhow::anyhow!("asset list `{path}` {error}"))?;
            names.push(SharedString::from(name));
        }
        names.sort();
        Ok(names)
    }
}

fn check_asset_list_budget(entries: usize, name_bytes: usize) -> Result<(), &'static str> {
    if entries == MAX_ASSET_LIST_ENTRIES {
        return Err("exceeded its entry limit");
    }
    if name_bytes > MAX_ASSET_LIST_NAME_BYTES {
        return Err("exceeded its name-byte limit");
    }
    Ok(())
}

#[derive(Default)]
struct MissingAssetReports {
    paths: HashSet<String>,
    order: VecDeque<String>,
}

impl MissingAssetReports {
    fn insert(&mut self, path: String) -> bool {
        if self.paths.contains(&path) {
            return false;
        }
        if self.paths.len() == MAX_REPORTED_MISSING_ASSETS
            && let Some(oldest) = self.order.pop_front()
        {
            self.paths.remove(&oldest);
        }
        self.order.push_back(path.clone());
        self.paths.insert(path)
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.paths.clear();
        self.order.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.paths.len()
    }
}

thread_local! {
    static REPORTED: RefCell<MissingAssetReports> = RefCell::new(MissingAssetReports::default());
}

fn report_missing(requested: &str, resolved: &std::path::Path) {
    let first_time = REPORTED.with(|reported| reported.borrow_mut().insert(requested.to_owned()));
    if first_time {
        tracing::warn!(
            "asset `{requested}` was not found at {}; asset paths resolve against the \
             application directory, not against the file that asked for them",
            resolved.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("gpui-shell-assets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("app")).expect("an application");
        base
    }

    #[test]
    fn traversal_is_refused() {
        let base = sandbox("traversal");
        let assets = AppAssets::new(base.join("app"));

        assert!(assets.resolve("../secret.svg").is_none());
        let (_, path) = assets.resolve("icons/check.svg").expect("a path inside");
        assert_eq!(path, PathBuf::from("icons/check.svg"));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_asset_is_not_an_error() {
        let assets = AppAssets::new(std::env::temp_dir());
        assert!(assets.load("definitely-not-here.svg").unwrap().is_none());
    }

    #[test]
    fn missing_asset_report_history_is_bounded() {
        REPORTED.with(|reported| {
            let mut reported = reported.borrow_mut();
            reported.clear();
            for index in 0..257 {
                reported.insert(format!("missing-{index}.svg"));
            }
            assert!(!reported.insert("missing-256.svg".to_owned()));
            assert!(reported.insert("missing-0.svg".to_owned()));
            assert!(
                reported.len() <= MAX_REPORTED_MISSING_ASSETS,
                "kept {} paths",
                reported.len()
            );
        });
    }

    #[test]
    fn an_oversized_asset_is_refused_before_it_is_buffered() {
        let base = sandbox("oversized");
        let path = base.join("app/huge.svg");
        let file = std::fs::File::create(&path).expect("asset");
        file.set_len(MAX_ASSET_BYTES + 1).expect("sparse asset");

        let error = AppAssets::new(base.join("app"))
            .load("huge.svg")
            .expect_err("oversized asset must fail");
        assert!(error.to_string().contains("asset") && error.to_string().contains("limit"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn asset_lists_bound_entries_and_aggregate_name_bytes() {
        assert!(check_asset_list_budget(MAX_ASSET_LIST_ENTRIES, 1).is_err());
        assert!(check_asset_list_budget(0, MAX_ASSET_LIST_NAME_BYTES + 1).is_err());
    }

    /// An asset path is a grant like any other: it names the application's own
    /// directory, and a link inside it must not turn that into the filesystem.
    ///
    /// The refusal is at the read rather than at the resolution, because what
    /// protects the directory is the handle, not the judgement.
    #[test]
    #[cfg(unix)]
    fn an_asset_behind_a_symlink_is_refused() {
        let base = sandbox("symlink");
        let root = base.join("app");
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).expect("somewhere outside");
        std::fs::write(outside.join("secret.svg"), b"secret").expect("a secret");
        std::fs::write(root.join("real.svg"), b"real").expect("an asset");
        std::os::unix::fs::symlink(&outside, root.join("icons")).expect("a symlink");

        let assets = AppAssets::new(root);

        assert!(
            assets.load("icons/secret.svg").is_err(),
            "an asset was read through a symlink out of the application directory"
        );
        assert_eq!(
            assets
                .load("real.svg")
                .expect("an ordinary asset")
                .expect("its bytes")
                .as_ref(),
            b"real"
        );
        assert!(assets.resolve("../outside/secret.svg").is_none());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[cfg(unix)]
    fn following_links_serves_a_linked_folder_but_not_traversal() {
        let base = sandbox("following");
        let root = base.join("app");
        let outside = base.join("dotfiles");
        std::fs::create_dir_all(&outside).expect("somewhere outside");
        std::fs::write(outside.join("check.svg"), b"check").expect("an icon");
        std::fs::write(base.join("secret.svg"), b"secret").expect("a secret");
        std::os::unix::fs::symlink(&outside, root.join("icons")).expect("a linked folder");
        std::os::unix::fs::symlink(outside.join("check.svg"), root.join("one.svg")).expect("a linked file");

        let assets = AppAssets::new(root).following_links();

        let read = |path: &str| assets.load(path).expect("loads").expect("its bytes").into_owned();
        assert_eq!(read("icons/check.svg"), b"check");
        assert_eq!(read("one.svg"), b"check");
        assert_eq!(assets.list("icons").expect("lists"), vec![SharedString::from("check.svg")]);
        assert!(assets.load("../secret.svg").is_err(), "`..` still leaves the directory");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    #[cfg(windows)]
    fn following_links_still_refuses_a_drive_path() {
        let base = sandbox("following-drive");
        let assets = AppAssets::new(base.join("app")).following_links();

        assert!(assets.load(r"\\?\C:\Windows\win.ini").is_err(), "a drive path leaves the directory");

        let _ = std::fs::remove_dir_all(&base);
    }
}
