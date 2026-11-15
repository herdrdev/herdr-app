//! Finding herdr-core (`hc`) on this machine. The app doesn't carry one: it
//! uses the core people installed, or installs the core's own release.
//!
//! Where it looks, in order: `core` in settings.json; this process's PATH;
//! the person's login shell's PATH (an app opened from the Dock or a
//! launcher doesn't get it); then the usual install places. A candidate
//! counts only if `--version` says `hc …`: some window managers ship an
//! unrelated `hc`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

#[cfg(any(target_os = "macos", all(test, unix)))]
mod cli_link;

static BIN: OnceLock<PathBuf> = OnceLock::new();

#[cfg(target_os = "macos")]
pub fn install_cli_link() {
    if let Err(e) = std::thread::Builder::new().name("cli-link".into()).spawn(|| {
        let result = std::env::current_exe().and_then(|exe| cli_link::ensure(&crate::mods::home(), &exe));
        if let Err(e) = result {
            tracing::warn!("cannot install herdr-app CLI link: {e}");
        }
    }) {
        tracing::warn!("cannot start herdr-app CLI setup: {e}");
    }
}

/// The core the app runs (`hc` until one is found).
pub fn path() -> PathBuf {
    BIN.get().cloned().unwrap_or_else(|| PathBuf::from("hc"))
}

/// Uses this core from now on, and puts its folder on PATH ahead of any
/// other `hc`, so mods' `hc()` and anything the app runs find the same one.
pub fn use_core(bin: PathBuf) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(joined) = path_for_core(&path, &bin, &hc_names()) {
        // SAFETY: set once at start, before other threads read the environment.
        unsafe { std::env::set_var("PATH", joined) };
    }
    let _ = BIN.set(bin);
}

/// PATH where `hc` finds `bin`, or None if it already does. The folder goes
/// just before the first entry that may hold another `hc`, so NixOS keeps
/// /run/wrappers/bin ahead of it.
fn path_for_core(path: &std::ffi::OsStr, bin: &Path, hc_names: &[String]) -> Option<std::ffi::OsString> {
    let dir = bin.parent()?;
    let dirs: Vec<PathBuf> = if path.is_empty() { Vec::new() } else { std::env::split_paths(path).collect() };
    // Relative entries (``, `.`) are another folder in each pane: assume an `hc` there.
    // Shells pick different names, so another folder is ours only if every `hc` in it is the core.
    let hcs_in = |d: &Path| hc_names.iter().map(|name| d.join(name)).filter(|hc| hc.is_file()).collect::<Vec<_>>();
    let stop = dirs.iter().position(|d| d.is_relative() || !hcs_in(d).is_empty());
    if let Some(d) = stop.map(|i| &dirs[i])
        && d.is_absolute()
        && (d == dir || bin.canonicalize().is_ok_and(|core| hcs_in(d).iter().all(|hc| hc.canonicalize().is_ok_and(|hc| hc == core))))
    {
        return None;
    }
    let (before, after) = dirs.split_at(stop.unwrap_or(0));
    let others = |d: &&PathBuf| *d != dir;
    let dirs = before.iter().filter(others).map(PathBuf::as_path).chain([dir]).chain(after.iter().filter(others).map(PathBuf::as_path));
    std::env::join_paths(dirs).ok()
}

/// The file names a shell runs for `hc`.
fn hc_names() -> Vec<String> {
    if cfg!(windows) { pathext_names(&std::env::var("PATHEXT").unwrap_or_default()) } else { vec!["hc".into()] }
}

/// Windows shells try each PATHEXT extension, and PowerShell also runs
/// `.ps1` scripts, so an earlier `hc.cmd` or `hc.ps1` wins over our `hc.exe`.
fn pathext_names(pathext: &str) -> Vec<String> {
    let pathext = if pathext.trim().is_empty() { ".COM;.EXE;.BAT;.CMD" } else { pathext };
    let exts = pathext.split(';').map(str::trim).filter(|ext| !ext.is_empty());
    exts.chain([".ps1"]).map(|ext| format!("hc{}", ext.to_ascii_lowercase())).collect()
}

/// Finder and Dock launches don't inherit the person's shell PATH.
#[cfg(target_os = "macos")]
pub fn fill_path() {
    if std::os::unix::process::parent_id() != 1 {
        return;
    }
    let home = crate::mods::home();
    let current = std::env::var_os("PATH").unwrap_or_default();
    if let Some(path) = merged_path(login_shell_path().as_deref(), &usual_dirs(&home)[..3], &current) {
        // SAFETY: called at startup, before other threads read the environment.
        unsafe { std::env::set_var("PATH", path) };
    }
}

#[cfg(any(target_os = "macos", test))]
fn merged_path(login: Option<&std::ffi::OsStr>, usual: &[PathBuf], current: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let mut dirs: Vec<PathBuf> = login.map(|path| dirs_on(path).collect()).unwrap_or_default();
    dirs.extend_from_slice(usual);
    dirs.extend(dirs_on(current));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    std::env::join_paths(dirs).ok()
}

/// `hc X.Y.Z` if this binary is herdr-core and runs.
pub fn version(bin: &Path) -> Option<String> {
    let out = crate::spawn::command(bin).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && text.starts_with("hc ")).then_some(text)
}

/// The first herdr-core found, or None.
pub fn find(configured: Option<&str>) -> Option<PathBuf> {
    let home = crate::mods::home();
    let pick = |candidates: Vec<PathBuf>| candidates.into_iter().filter(|c| c.is_file()).find(|c| version(c).is_some());
    if let Some(found) = pick(configured.map(|c| vec![expand(c, &home)]).unwrap_or_default()) {
        return Some(found);
    }
    // Tests: HERDR_CORE_DIRS replaces every place Herdr would look.
    if let Some(dirs) = std::env::var_os("HERDR_CORE_DIRS") {
        return pick(std::env::split_paths(&dirs).flat_map(|d| names(&d)).collect());
    }
    // The quick places first; the login shell (slow) only if they have none.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let quick: Vec<PathBuf> = dirs_on(&path).chain(usual_dirs(&home)).flat_map(|d| names(&d)).collect();
    pick(quick).or_else(|| pick(dirs_on(&login_shell_path()?).flat_map(|d| names(&d)).collect()))
}

fn expand(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

fn names(dir: &Path) -> [PathBuf; 2] {
    let exe = std::env::consts::EXE_SUFFIX;
    [dir.join(format!("herdr-core{exe}")), dir.join(format!("hc{exe}"))]
}

fn dirs_on(path: &std::ffi::OsStr) -> impl Iterator<Item = PathBuf> + '_ {
    std::env::split_paths(path)
}

/// Where herdr-core gets installed: its own installer and Herdr 1.x's
/// (~/.local/bin), Homebrew, mise, Nix, cargo.
fn usual_dirs(home: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Local"));
        return vec![local.join("Programs/herdr-core"), home.join(".cargo/bin")];
    }
    let mut dirs = vec![
        home.join(".local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".local/share/mise/shims"),
        home.join(".nix-profile/bin"),
        PathBuf::from("/run/current-system/sw/bin"),
        home.join(".cargo/bin"),
    ];
    if cfg!(target_os = "macos") {
        dirs.push(home.join(".local/share/herdr-core/current/Contents/MacOS"));
        // Older installers placed the bundle directly in herdr-core.
        dirs.push(home.join(".local/share/herdr-core/Herdr Core.app/Contents/MacOS"));
    }
    dirs
}

/// The PATH the person's login shell sets up, or None if it doesn't answer quickly.
fn login_shell_path() -> Option<std::ffi::OsString> {
    if cfg!(windows) {
        return None;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let mut child = Command::new(shell)
        .args(["-lc", "printf %s \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // A slow or interactive shell profile must not hold up the app.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    Some(out.into())
}

/// Installs the core's latest release with its own installer (into
/// ~/.local/bin, or HC_INSTALL_DIR), then finds it. Err: what went wrong.
pub fn install() -> Result<PathBuf, String> {
    let releases = std::env::var("HERDR_RELEASES_URL").unwrap_or_else(|_| "https://releases.herdr.dev".into());
    let mut cmd = if cfg!(windows) {
        let mut c = crate::spawn::command("powershell");
        c.args(["-NoProfile", "-Command", &format!("irm {releases}/core/install.ps1 | iex")]);
        c
    } else {
        let mut c = crate::spawn::command("sh");
        c.args(["-c", &format!("curl -fsSL {releases}/core/install.sh | sh")]);
        c
    };
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| format!("cannot run the installer: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if err.is_empty() { "the installer failed".into() } else { err });
    }
    let home = crate::mods::home();
    let dir = std::env::var_os("HC_INSTALL_DIR").map(PathBuf::from).unwrap_or_else(|| {
        if cfg!(windows) {
            usual_dirs(&home).remove(0)
        } else {
            home.join(".local/bin")
        }
    });
    let bin = dir.join(format!("herdr-core{}", std::env::consts::EXE_SUFFIX));
    version(&bin).map(|_| bin).ok_or_else(|| "installed, but herdr-core doesn't run".into())
}

/// Updates the core to its latest release (`hc update`: same generation only,
/// terminals kept). Ok: the version it runs now. Err: what it said, e.g.
/// "belongs to Homebrew: brew upgrade herdr-core".
pub fn update() -> Result<String, String> {
    update_to(None)
}

/// `hc update --version X`: at least X (a newer core stays). Cores before 0.1.5 lack it.
pub fn update_to(at_least: Option<&str>) -> Result<String, String> {
    let mut cmd = crate::spawn::command(path());
    cmd.args(["update", "--json"]);
    if let Some(v) = at_least {
        cmd.args(["--version", v]);
    }
    let out = cmd.stdin(Stdio::null()).output().map_err(|e| format!("cannot run hc update: {e}"))?;
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    if out.status.success() {
        let now = reply.get("now").and_then(|v| v.as_str()).unwrap_or("the latest").to_owned();
        #[cfg(target_os = "macos")]
        ensure_core_bundle(&now)?;
        return Ok(now);
    }
    let err: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap_or_default();
    let message = err.pointer("/error/message").and_then(|v| v.as_str()).map(str::to_owned);
    let hint = err.pointer("/error/hint").and_then(|v| v.as_str()).map(|h| format!(" ({h})")).unwrap_or_default();
    Err(message.map(|m| m + &hint).unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).trim().to_owned()))
}

#[cfg(any(target_os = "macos", all(test, unix)))]
const BUNDLE_INSTALL_SCRIPT: &str = "installer=$(curl -fsSL \"$1/core/install.sh\") && case \"$installer\" in *HC_BUNDLE_ONLY*) printf '%s\\n' \"$installer\" | sh ;; *) exit 0 ;; esac";

#[cfg(any(target_os = "macos", all(test, unix)))]
fn bundle_completion_path(bin: &Path, home: &Path) -> bool {
    bin.starts_with(home)
        && !bin.ends_with("Contents/MacOS/herdr-core")
        && !["/Cellar/", "/nix/store/", "/mise/installs/"]
            .iter().any(|marker| bin.to_string_lossy().contains(marker))
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn bundle_completion_writable(bin: &Path) -> bool {
    bin.parent().is_some_and(|dir| {
        Command::new("/bin/test").arg("-w").arg(dir).status().is_ok_and(|status| status.success())
    })
}

/// Old core updaters copied only the bare executable. Finish that upgrade
/// with the current installer, which retains the complete signed app bundle.
#[cfg(target_os = "macos")]
fn ensure_core_bundle(version: &str) -> Result<(), String> {
    let bin = path();
    let resolved = bin.canonicalize().unwrap_or_else(|_| bin.clone());
    if !bundle_completion_path(&resolved, &crate::mods::home()) || !bundle_completion_writable(&resolved) {
        return Ok(());
    }
    // Never replace a contributor's build, package-manager core, or a host app.
    let official = crate::spawn::command("/usr/bin/codesign")
        .args(["--verify", "--strict", "-R", "=identifier \"dev.herdr.core\" and anchor apple generic and certificate leaf[subject.OU] = \"2W9652Q7QR\""])
        .arg(&bin).output().is_ok_and(|out| out.status.success());
    if !official {
        return Ok(());
    }
    let releases = std::env::var("HERDR_RELEASES_URL").unwrap_or_else(|_| "https://releases.herdr.dev".into());
    let mut cmd = crate::spawn::command("sh");
    cmd.args([
        "-c",
        BUNDLE_INSTALL_SCRIPT,
        "sh",
        &releases,
    ]);
    cmd.env("HC_BUNDLE_ONLY", "1");
    if let Some(v) = version.strip_prefix("hc ") {
        cmd.env("HC_VERSION", v);
    }
    if resolved.is_absolute()
        && let Some(dir) = resolved.parent()
    {
        cmd.env("HC_INSTALL_DIR", dir);
    }
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot finish core bundle installation: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn the_independent_core_bundle_is_a_discovery_fallback() {
        let home = Path::new("/Users/test");
        let dir = home.join(".local/share/herdr-core/current/Contents/MacOS");
        let legacy = home.join(".local/share/herdr-core/Herdr Core.app/Contents/MacOS");
        let dirs = usual_dirs(home);
        assert_eq!(&dirs[dirs.len() - 2..], &[dir.clone(), legacy]);
        assert_eq!(names(&dir)[0], dir.join("herdr-core"));
    }

    #[test]
    fn startup_path_keeps_shell_order_and_falls_back_to_usual_dirs() {
        let usual = [PathBuf::from("/home/test/.local/bin"), PathBuf::from("/opt/homebrew/bin")];
        let current = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        let login = std::env::join_paths(["/custom/bin", "/opt/homebrew/bin", "/usr/bin"]).unwrap();
        let merged = merged_path(Some(&login), &usual, &current).unwrap();
        assert_eq!(std::env::split_paths(&merged).collect::<Vec<_>>(), ["/custom/bin", "/opt/homebrew/bin", "/usr/bin", "/home/test/.local/bin", "/bin"].map(PathBuf::from));
        let fallback = merged_path(None, &usual, &current).unwrap();
        assert_eq!(std::env::split_paths(&fallback).collect::<Vec<_>>(), ["/home/test/.local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    }

    #[cfg(unix)]
    #[test]
    fn bundle_completion_skips_managed_and_system_paths() {
        let home = Path::new("/Users/test");
        assert!(bundle_completion_path(&home.join(".local/bin/herdr-core"), home));
        for bin in [
            "/usr/local/bin/herdr-core",
            "/opt/homebrew/Cellar/herdr-core/1/bin/herdr-core",
            "/nix/store/abc/bin/herdr-core",
            "/Users/test/.local/share/mise/installs/herdr-core/1/herdr-core",
            "/Users/test/.local/share/herdr-core/current/Contents/MacOS/herdr-core",
        ] {
            assert!(!bundle_completion_path(Path::new(bin), home), "{bin}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn bundle_completion_skips_unwritable_directories() {
        assert!(bundle_completion_writable(&std::env::temp_dir().join("herdr-core")));
        assert!(!bundle_completion_writable(Path::new("/nonexistent/herdr-core")));
        let root = Command::new("id").arg("-u").output().unwrap().stdout == b"0\n";
        if !root {
            assert!(!bundle_completion_writable(Path::new("/usr/bin/herdr-core")));
        }
    }

    #[cfg(unix)]
    #[test]
    fn bundle_completion_does_not_run_a_legacy_installer() {
        for (installer, expected) in [("echo restarted", ""), ("# HC_BUNDLE_ONLY\necho bundled", "bundled\n")] {
            let out = Command::new("sh")
                .args(["-c", &format!("curl() {{ printf '%s\\n' \"$INSTALLER\"; }}; {BUNDLE_INSTALL_SCRIPT}")])
                .env("INSTALLER", installer)
                .output().unwrap();
            assert!(out.status.success());
            assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
        }
    }

    // Fakes the binary with a shell script.
    #[cfg(unix)]
    #[test]
    fn only_a_binary_that_says_hc_counts() {
        // Runtime writes can leave a writer fd in a parallel fork, causing ETXTBSY.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core-bin");
        assert_eq!(version(&dir.join("hc")), None);
        let real = dir.join("herdr-core");
        assert_eq!(version(&real).as_deref(), Some("hc 0.1.1"));
        assert_eq!(find(Some(real.to_str().unwrap())), Some(real));
    }

    /// Folders under a fresh temp root, each holding the named (empty) programs.
    fn bin_dirs(test: &str, dirs: &[(&str, &[&str])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("herdr-app-core-path-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, programs) in dirs {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            for program in *programs {
                std::fs::write(root.join(dir).join(format!("{program}{}", std::env::consts::EXE_SUFFIX)), "").unwrap();
            }
        }
        root
    }

    fn path_of(root: &Path, dirs: &[&str]) -> std::ffi::OsString {
        std::env::join_paths(dirs.iter().map(|d| if d.is_empty() { PathBuf::new() } else { root.join(d) })).unwrap()
    }

    fn core_in(root: &Path, dir: &str) -> PathBuf {
        root.join(dir).join(format!("herdr-core{}", std::env::consts::EXE_SUFFIX))
    }

    #[test]
    fn path_stays_when_the_core_is_already_first() {
        let root = bin_dirs("first", &[("core", &["herdr-core", "hc"]), ("wrappers", &["sudo"])]);
        assert_eq!(path_for_core(&path_of(&root, &["core", "wrappers"]), &core_in(&root, "core"), &hc_names()), None);
    }

    #[test]
    fn path_keeps_nixos_wrappers_ahead_of_the_core() {
        let root = bin_dirs("nixos", &[("wrappers", &["sudo"]), ("sw", &["herdr-core", "hc", "sudo"])]);
        assert_eq!(path_for_core(&path_of(&root, &["wrappers", "sw"]), &core_in(&root, "sw"), &hc_names()), None);
    }

    #[test]
    fn path_moves_the_core_just_ahead_of_another_hc() {
        let root = bin_dirs("shadow", &[("wrappers", &["sudo"]), ("wm", &["hc"]), ("core", &["herdr-core", "hc"])]);
        let path = path_for_core(&path_of(&root, &["wrappers", "wm", "core"]), &core_in(&root, "core"), &hc_names());
        assert_eq!(path, Some(path_of(&root, &["wrappers", "core", "wm"])));
    }

    #[test]
    fn windows_counts_every_pathext_hc_and_powershell_scripts() {
        assert_eq!(pathext_names(".COM;.EXE;.BAT;.CMD;.PY"), ["hc.com", "hc.exe", "hc.bat", "hc.cmd", "hc.py", "hc.ps1"]);
        assert_eq!(pathext_names(""), ["hc.com", "hc.exe", "hc.bat", "hc.cmd", "hc.ps1"]);
    }

    #[test]
    fn path_moves_the_core_ahead_of_an_earlier_powershell_script() {
        let root = bin_dirs("ps1", &[("tools", &[]), ("core", &["herdr-core"])]);
        std::fs::write(root.join("tools/hc.ps1"), "").unwrap();
        std::fs::write(root.join("core/hc.exe"), "").unwrap();
        let path = path_for_core(&path_of(&root, &["tools", "core"]), &core_in(&root, "core"), &pathext_names(""));
        assert_eq!(path, Some(path_of(&root, &["core", "tools"])));
    }

    #[cfg(unix)]
    #[test]
    fn path_stays_when_an_earlier_hc_is_the_same_core() {
        let root = bin_dirs("link", &[("core", &["herdr-core"]), ("links", &[])]);
        std::os::unix::fs::symlink(core_in(&root, "core"), root.join("links/hc")).unwrap();
        assert_eq!(path_for_core(&path_of(&root, &["links", "core"]), &core_in(&root, "core"), &hc_names()), None);
    }

    /// cmd may run the linked `hc.exe` while PowerShell runs the other `hc.ps1`.
    #[cfg(unix)]
    #[test]
    fn path_moves_the_core_ahead_of_a_link_folder_with_another_hc() {
        let root = bin_dirs("mixed", &[("core", &["herdr-core"]), ("links", &[])]);
        std::os::unix::fs::symlink(core_in(&root, "core"), root.join("links/hc.exe")).unwrap();
        let path = path_of(&root, &["links", "core"]);
        assert_eq!(path_for_core(&path, &core_in(&root, "core"), &pathext_names("")), None);
        std::fs::write(root.join("links/hc.ps1"), "").unwrap();
        assert_eq!(path_for_core(&path, &core_in(&root, "core"), &pathext_names("")), Some(path_of(&root, &["core", "links"])));
    }

    /// `""` and `tools` are a different folder in each pane, so either may hold another `hc`.
    #[test]
    fn path_moves_the_core_ahead_of_relative_entries_and_keeps_them() {
        let root = bin_dirs("relative", &[("wrappers", &["sudo"]), ("core", &["herdr-core", "hc"])]);
        let path_with = |dirs: [&Path; 4]| std::env::join_paths(dirs).unwrap();
        let (wrappers, core) = (root.join("wrappers"), root.join("core"));
        let path = path_for_core(&path_with([&wrappers, "".as_ref(), "tools".as_ref(), &core]), &core_in(&root, "core"), &hc_names());
        assert_eq!(path, Some(path_with([&wrappers, &core, "".as_ref(), "tools".as_ref()])));
    }

    #[test]
    fn path_gets_a_missing_core_first() {
        let root = bin_dirs("missing", &[("wrappers", &["sudo"]), ("other", &[]), ("core", &["herdr-core", "hc"])]);
        let path = path_for_core(&path_of(&root, &["wrappers", "other"]), &core_in(&root, "core"), &hc_names());
        assert_eq!(path, Some(path_of(&root, &["core", "wrappers", "other"])));
    }

    #[test]
    fn path_is_just_the_core_when_empty() {
        let root = bin_dirs("empty", &[("core", &["herdr-core", "hc"])]);
        let path = path_for_core(std::ffi::OsStr::new(""), &core_in(&root, "core"), &hc_names());
        assert_eq!(path, Some(root.join("core").into_os_string()));
    }
}
