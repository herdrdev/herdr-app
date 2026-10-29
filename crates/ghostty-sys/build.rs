//! Builds libghostty-vt from vendor/ghostty with Zig and generates bindings from its headers.

use std::path::PathBuf;
use std::process::Command;

// Zig builds for the machine it runs on unless told otherwise: its CPU's
// newest instructions, which crash older CPUs, and never another target's.
fn zig_target(target: &str) -> &'static str {
    match target {
        "x86_64-unknown-linux-gnu" => "x86_64-linux-gnu",
        "aarch64-unknown-linux-gnu" => "aarch64-linux-gnu",
        "x86_64-unknown-openbsd" => "x86_64-openbsd",
        "x86_64-apple-darwin" => "x86_64-macos",
        "aarch64-apple-darwin" => "aarch64-macos",
        "x86_64-pc-windows-msvc" => "x86_64-windows-msvc",
        "aarch64-pc-windows-msvc" => "aarch64-windows-msvc",
        other => panic!("ghostty-sys: no Zig target for {other}"),
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest
        .join("../../vendor/ghostty")
        .canonicalize()
        .expect("vendor/ghostty missing: run `git submodule update --init --depth 1`");
    // Windows' canonical form is `\\?\C:\...`, which clang can't resolve includes against.
    let src = PathBuf::from(src.to_string_lossy().trim_start_matches(r"\\?\").to_owned());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    for path in ["src", "build.zig", "build.zig.zon", "include"] {
        println!("cargo:rerun-if-changed={}", src.join(path).display());
    }
    println!("cargo:rerun-if-env-changed=ZIG");

    let zig = std::env::var("ZIG").unwrap_or_else(|_| "zig".into());
    let status = Command::new(&zig)
        .current_dir(&src)
        .args(["build", "-Demit-lib-vt", "-Doptimize=ReleaseFast"])
        .arg(format!("-Dtarget={}", zig_target(&std::env::var("TARGET").unwrap())))
        .arg("--prefix")
        .arg(out.join("zig-out"))
        .arg("--cache-dir")
        .arg(out.join("zig-cache"))
        .status()
        .unwrap_or_else(|e| panic!("cannot run {zig:?} (Zig 0.16 required): {e}"));
    assert!(status.success(), "zig build of libghostty-vt failed: {status}");

    // Only the static archive in the search path, so the linker can't pick the shared one.
    let lib = out.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        std::fs::copy(out.join("zig-out/lib/ghostty-vt-static.lib"), lib.join("ghostty-vt.lib")).unwrap();
    } else {
        std::fs::copy(out.join("zig-out/lib/libghostty-vt.a"), lib.join("libghostty-vt.a")).unwrap();
    }
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");

    // libclang without a full clang install lacks its builtin headers; Zig ships the same ones.
    let zig_env = Command::new(&zig).arg("env").output().expect("zig env");
    let zig_env = String::from_utf8_lossy(&zig_env.stdout);
    let zig_lib = zig_env
        .lines()
        .find_map(|l| l.trim().strip_prefix(".lib_dir = \""))
        .and_then(|l| l.strip_suffix("\","))
        .expect("zig env has no lib_dir");

    let include = src.join("include");
    let mut builder = bindgen::Builder::default()
        .header(include.join("ghostty/vt.h").to_string_lossy())
        .clang_arg(format!("-I{}", include.display()));
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Apple's libclang has its builtin headers but needs the SDK for the C
        // library (Zig's builtin headers clash with it there).
        if let Ok(sdk) = Command::new("xcrun").args(["--show-sdk-path"]).output() {
            builder = builder.clang_arg(format!("-isysroot{}", String::from_utf8_lossy(&sdk.stdout).trim()));
        }
    } else if Command::new("clang").arg("--version").output().is_err() {
        // With a clang installed, bindgen asks it for its own builtin headers,
        // which match its libclang; Zig's newer ones break older ones (Ubuntu
        // 22.04's clang 14 can't parse them).
        builder = builder.clang_arg(format!("-isystem{zig_lib}/include"));
    }
    builder
        .allowlist_function("ghostty_.*")
        .allowlist_type("Ghostty.*")
        .allowlist_var("GHOSTTY_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .generate()
        .expect("bindgen failed on ghostty/vt.h")
        .write_to_file(out.join("bindings.rs"))
        .unwrap();
}
