use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

// Synchronize only sources; keep package downloads and compiler caches across
// rebuilds. The pinned submodule and its package directory are read-only here.
pub fn prepare(src: &Path, out: &Path, manifest: &Path) -> PathBuf {
    let dst = out.join("renderer-source");
    let packages = out.join("renderer-packages");
    let old_packages = dst.join("zig-pkg");
    if !packages.exists() && old_packages.is_dir() && !old_packages.is_symlink() {
        fs::rename(&old_packages, &packages).expect("preserve renderer packages");
    }
    fs::create_dir_all(&dst).unwrap();
    fs::create_dir_all(&packages).unwrap();
    let mut sync = Command::new("rsync");
    sync.args(["-a", "--checksum", "--delete"]);
    for exclude in [".git", ".zig-cache", "zig-out", "zig-pkg"] {
        sync.arg(format!("--exclude={exclude}"));
    }
    let status = sync
        .arg(format!("{}/", src.display()))
        .arg(&dst)
        .status()
        .expect("sync Ghostty sources");
    assert!(status.success(), "sync Ghostty sources failed");
    // Seed pre-fetched packages, without overwriting or deleting other pinned
    // dependencies downloaded by an earlier renderer build.
    if src.join("zig-pkg").is_dir() {
        let status = Command::new("rsync")
            .args(["-a", "--ignore-existing"])
            .arg(format!("{}/zig-pkg/", src.display()))
            .arg(&packages)
            .status()
            .unwrap();
        assert!(status.success(), "seed Ghostty packages failed");
    }
    if !old_packages.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&packages, &old_packages).unwrap();
    }
    let status = Command::new("patch")
        .current_dir(&dst)
        .args(["--batch", "--forward", "--fuzz=0", "-p1", "-i"])
        .arg(manifest.join("renderer/ghostty.patch"))
        .status()
        .expect("run patch");
    assert!(
        status.success(),
        "Ghostty renderer patch no longer matches the pinned submodule"
    );
    fs::copy(
        manifest.join("renderer/herdr_renderer.zig"),
        dst.join("src/herdr_renderer.zig"),
    )
    .unwrap();
    dst
}
