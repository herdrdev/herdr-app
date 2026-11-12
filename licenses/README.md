# Release notices

`THIRD-PARTY-NOTICES` ships with every release beside `LICENSE`. It includes Cargo licenses for the resolved graph on every platform (including build/dev dependencies), plus the native code, fonts and icons in `inventory.json`. Unused optional packages left in Cargo.lock are not linked into the app.

Install [cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.3 and [Bun](https://bun.sh). The notices include licenses from the `vendor/ghostty` submodule, so check it out first. Then run:

```sh
cargo install --locked cargo-about --version 0.9.3 --features cli
git submodule update --init vendor/ghostty
just notices
```

`CARGO_ABOUT` can point to a separately installed cargo-about binary. Generation uses the locked graph and full upstream license texts. Review warnings and the diff, especially copyright notices. Don't add blanket license overrides to get a dependency through.

`cargo test --locked --test notices` checks that the notices match Cargo.lock and the recorded inputs, and contain every inventoried license in full. New dependencies fail that check until the notices are regenerated. New bundled components also need an inventory entry and their exact upstream notices. The lockfile fingerprint covers each third-party package's name, version, source and checksum, including vendored path packages, but excludes workspace packages. The manifest fingerprint also excludes `[package].version`, so release version bumps don't invalidate notices. This is a freshness check, not a substitute for reviewing licenses.

The copies here come from the sources pinned in `inventory.json`. simdutf's embedded version is 9.0.0 even though Ghostty's Zig package wrapper still says 5.2.8; its TH-derived CPU detection notice is included too. Highway's Apache and BSD notices are both retained. uucode's own license and both nested licenses are included. Zig's MIT license and musl's copyright file cover the bundled Zig runtime and its musl-derived math routines, as well as builds that contain musl itself. The released Linux app uses the system glibc. The core is downloaded separately and ships its own notices.

Cargo-about covers the vendored GPUI crates as part of the Cargo graph. The separate native inventory covers Ghostty's VT build, not the unrelated libraries used only by Ghostty's full desktop app.
