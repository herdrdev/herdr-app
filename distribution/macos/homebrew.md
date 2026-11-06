# Homebrew release coordination

The cask is `Casks/h/herdr-app.rb` in [Homebrew/homebrew-cask](https://github.com/Homebrew/homebrew-cask/blob/master/Casks/h/herdr-app.rb), not a Herdr-owned tap. A Herdr app PR cannot change that repository. The existing cask has one ARM checksum/URL and `depends_on arch: :arm64`.

The app release produces notarized `Herdr-macos-aarch64.dmg` and `Herdr-macos-x86_64.dmg` on their native runners. Each successful job publishes its versioned and latest download. ARM also preserves the existing `Herdr.dmg` URLs byte for byte. Neither DMG job blocks tarball/app-update publishing. The DMG-specific `app/dmg/latest.json` advances only after both jobs uploaded their images; it contains a version and per-target immutable URL and SHA256, computed from the final signed/stapled bytes. A failed or incomplete Intel job cannot advertise a complete new DMG release. The old ARM website download can still advance independently.

The DMG publisher uses S3 conditional writes, not cache headers as overwrite protection. Versioned images, the versioned ARM alias and versioned `dmg.json` are create-only. A retry of the same source commit reuses the already committed image bytes, including their original signing timestamps; a different commit or unverified provenance cannot replace them. The retained Actions artifact is updated to those canonical bytes before the finalizer computes checksums. The finalizer also reads back every versioned image/ARM alias before advertising the pair.

Latest downloads and `app/dmg/latest.json` use ETag compare-and-swap and semantic version checks. An older tag cannot roll them back, and conflicting same-version bytes are rejected. The first update of an old alias without version metadata verifies its bytes against actual versioned history; if its version cannot be identified, it is left unchanged and publication reports an error. There is no unconditional write fallback. The release credential therefore needs its existing object read/write and bucket-list access, and the installed AWS CLI must support conditional `PutObject`. No token, bucket policy or signing permission is changed by this patch; these prerequisites require validation in the team's release environment.

After the first official pair is available, a separate cask PR should:

1. Use `arch arm: "aarch64", intel: "x86_64"` and the versioned `Herdr-macos-#{arch}.dmg` URL.
2. Supply distinct `sha256 arm:` and `intel:` values from that released pair, verified against downloaded bytes. Never use an ARM checksum for Intel or `:no_check`.
3. Remove `depends_on arch: :arm64`, and express the actual minimum with `depends_on macos: :ventura` (the default comparison is `>=`, macOS 13.0, as declared by this app's pinned dependency/build configuration).
4. Change the JSON livecheck URL to `https://releases.herdr.dev/app/dmg/latest.json`, retaining its version lookup. The existing app `latest.json` can advance before notarized DMGs are ready.
5. Preserve `app "Herdr.app"`, the `herdr-app` binary link, existing zap paths and `auto_updates true`. Homebrew normally skips auto-updating casks during upgrades; `brew upgrade --cask --greedy herdr-app` includes them. The app's own updater continues using its architecture-specific tarball manifest.

Future cask version/checksum changes still require Homebrew's normal update process; publishing a Herdr manifest does not itself merge a cask update. Check the cask's current source and [contribution rules](https://github.com/Homebrew/homebrew-cask/blob/master/CONTRIBUTING.md) at submission time. Its required online audit, style and actual install/uninstall validation must pass before submitting; offline DSL and DMG fixtures are not substitutes. Human review and the required AI disclosure apply. Do not submit against an unreleased Intel URL or invent a checksum.

The app maintainer must agree to the extra Intel signing/notarization work and the public URL/manifest contract. Only the existing maintainer/tag/release-environment gates can publish these files; this patch does not add credentials or automate a cross-repository write.
