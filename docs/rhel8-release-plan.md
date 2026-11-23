# Plan: RHEL 8-only release build in `release.yml`

Status: the app-only RHEL 8 build is **green** (run 3). The herdr-core step
below is **planned but not yet implemented** in the workflow.

## Goal

Replace the existing `.github/workflows/release.yml` with a **single-job
release build** for **Linux x86_64 on RHEL 8**: build both `herdr-app` and
`herdr-core` with `cargo build --release`, pack them into one tarball, verify,
and upload it as a GitHub Actions artifact. The core is required: without it a
user cannot run the whole Herdr environment, even though the app itself starts.
Trigger with `gh`, watch the run, fix failures, and repeat until it succeeds.

Explicitly out of scope: full CI/tests, Windows, macOS, aarch64, signing, DMG,
and publishing to the release bucket. `ci.yml` stays untouched.

## Why the current `release.yml` is being replaced

- Its Linux target builds on `ubuntu-22.04` (glibc 2.35) and says in a comment
  that this sets the distro floor. RHEL 8 has glibc 2.28, so that binary will
  not start there.
- It also builds `linux-aarch64`, `macos-aarch64`, and `windows-x86_64`, plus
  signing/DMG/publish jobs for those platforms. None of that is wanted here.
- The pinned AlmaLinux 8 image has glibc 2.28; the Ubuntu package name
  `libxkbcommon-x11-devel` does **not** exist there (use
  `libxkbcommon-devel` + `libxkbcommon-x11`).
- The app's own installer normally downloads herdr-core separately
  (`distribution/install.sh`); an offline artifact for a user to run the whole
  environment must therefore carry a core binary too.

## Decisions

| Question | Decision |
|---|---|
| Which file | **Replace `.github/workflows/release.yml` in place.** No new workflow file. |
| Platform | One job only, Linux x86_64. No matrix, no Windows/macOS/aarch64 jobs. |
| Environment | GitHub-hosted runner + `container:` pinned to `quay.io/almalinux/almalinux:8@sha256:b068b59649ddecf7c95cea3d3f16941eaf1d8a77fcf6ce94c779abae7552e2ea` (glibc 2.28). |
| Trigger | `workflow_dispatch` so `gh workflow run release.yml --ref <branch>` starts it directly. No tag/publish trigger. |
| App build | `scripts/package x86_64-unknown-linux-gnu` → `dist/herdr-app-linux-x86_64.tar.gz`. |
| Core build | Clone upstream `https://github.com/herdrdev/herdr-core` (shallow), checkout the tag matching `distribution/core-version` (currently `0.1.11`, tag `v0.1.11`), init its submodules, and build `cargo build --locked --release -p herdr-core` with the same Rust/Zig toolchains in the same container. Upstream master is acceptable if that tag is not buildable; no changes to herdr-core itself. |
| Output | One tarball, `dist/herdr-app-rhel8-x86_64.tar.gz`: the `scripts/package` app tree plus `herdr-core` and an `hc` symlink in `herdr-app/`. Upload as artifact `herdr-app-rhel8-x86_64`. Nothing goes to the release bucket or manifest. |
| Where to work | Branch `build/rhel8-release-gha` on fork `shawn111/herdr-app`; no pushes to `upstream`. |
| Repo docs | `defaults/AGENTS.md`, `src/SKILL.md`, and `CHANGELOG.md` unchanged: no user-facing app behavior, and changelog entries are written at release time. |

## Steps

### 1. Commit this plan first — done

Commit `9a3c18f` on branch `build/rhel8-release-gha`.

### 2. Rewrite `.github/workflows/release.yml`

- `name: rhel8 release build`; `permissions: contents: read`, no
  `environment:` (works from a fork branch without release secrets).
- `on: workflow_dispatch`; concurrency group `rhel8-release-${{ github.ref }}`,
  `cancel-in-progress: true`; 60-minute timeout.
- `container:` pinned digest above; `defaults.run.shell: bash`.
- **Bootstrap before checkout** (minimal image; `actions/checkout` needs git):
  refresh `/etc/pki/rpm-gpg/RPM-GPG-KEY-AlmaLinux` from the official HTTPS URL
  and `rpm --import` it (the pinned image only has an old key), then `dnf
  install -y ca-certificates curl tar gzip git python3 binutils` with one
  bounded retry. Never use `nogpgcheck`.
- Checkout with `submodules: true`, `persist-credentials: false`, pinned to the
  same SHA used in `ci.yml`; mark `$GITHUB_WORKSPACE` as a git safe directory
  (container root vs runner-owned workspace).
- Toolchain: Rust `1.95.0` and Zig `0.16.0`, same versions and pinned action
  SHAs as in `ci.yml`. Every `uses:` stays pinned to a commit SHA.
- Build deps via `dnf`: `gcc gcc-c++ pkgconf-pkg-config clang clang-devel
  libxcb-devel libxkbcommon-devel libxkbcommon-x11 wayland-devel vulkan-devel
  fontconfig-devel freetype-devel`.
- `scripts/ci-sources restore`, then `scripts/package
  x86_64-unknown-linux-gnu` for the app (already working in run 3).
- **Build herdr-core in the same job** (still to implement):
  - Clone upstream into a temp dir:
    `git clone --depth 1 --branch "v$(cat distribution/core-version)" https://github.com/herdrdev/herdr-core.git <tmp>/herdr-core`.
  - `git -C <tmp>/herdr-core submodule update --init --depth 1`.
  - `cargo build --locked --release -p herdr-core` from that checkout (uses the
    Rust/Zig toolchains already installed; herdr-core's CI also pins
    Rust 1.95.0 and Zig 0.16).
  - Unpack `dist/herdr-app-linux-x86_64.tar.gz`, copy
    `<tmp>/herdr-core/target/release/herdr-core` into `herdr-app/`, add an `hc`
    symlink to it, and repack as `dist/herdr-app-rhel8-x86_64.tar.gz`. Leave
    `scripts/package` itself unchanged.
- Delete the old `ci-passed`, `sign-windows`, `mac-dmg`, and `publish` jobs;
  they only exist for the platforms/artifacts being removed.
- Verification steps:
  - `ldd --version | head -1` reports glibc 2.28 (proves the container, not the
    Ubuntu host, built it).
  - Highest `GLIBC_*` symbol used by the app and core binaries is <= 2.28.
  - Final tarball lists `herdr-app/{herdr-app,herdr-core,hc,herdr-app.svg,LICENSE,THIRD-PARTY-NOTICES}`;
    notices still `cmp` against the repo copies.
  - Smoke tests with scratch dirs: app `HERDR_APP_HOME=<scratch> herdr-app --help`
    and core `herdr-core --version` (both only talk; no window or real user data).
- Upload the final tarball with pinned `actions/upload-artifact`,
  `if-no-files-found: error`, 7-day retention. No cache yet; add one only after
  the full app+core run is green.

### 3. Commit, push, trigger, iterate

```sh
git add .github/workflows/release.yml
git commit -m "ci: build RHEL 8 release on x86_64 only"
git push -u origin build/rhel8-release-gha
gh workflow run release.yml --repo shawn111/herdr-app --ref build/rhel8-release-gha
gh run list --workflow release.yml --repo shawn111/herdr-app --limit 5
gh run watch <run-id> --repo shawn111/herdr-app --exit-status
```

On failure: `gh run view <run-id> --repo shawn111/herdr-app --log-failed`, fix
the workflow on the same branch, commit with a lowercase conventional message,
push, re-trigger, and repeat **until the run succeeds**. Keep the scope
single-platform while debugging — don't restore Windows/macOS/CI/publishing to
work around a failure.

### 4. Handoff

Report the final green run URL, artifact name, glibc check output, and branch.
Publishing and a real RHEL 8 desktop smoke test remain separate decisions.

## Run log

| # | Commit | Run URL | Result | Fix |
|---|---|---|---|---|
| 1 | `ffeaf41` | https://github.com/shawn111/herdr-app/actions/runs/45259164686 | failed in `Bootstrap container tools`: `GPG check FAILED` (pinned AlmaLinux 8 image has only the old `C21AD6EA` key; current packages use newer keys) | Refresh `/etc/pki/rpm-gpg/RPM-GPG-KEY-AlmaLinux` from `https://repo.almalinux.org/almalinux/RPM-GPG-KEY-AlmaLinux` over HTTPS and `rpm --import` it before `dnf install` (no `nogpgcheck`). |
| 2 | `3756df7` | https://github.com/shawn111/herdr-app/actions/runs/45259697562 | failed in `Keep unchanged sources fresh`: `fatal: detected dubious ownership in repository at '/__w/herdr-app/herdr-app'` (container root vs runner-owned workspace) | Add `git config --global --add safe.directory "$GITHUB_WORKSPACE"` after checkout. |
| 3 | `cb80aaf` | https://github.com/shawn111/herdr-app/actions/runs/45260188232 | **success** (11m43s), app only: `ldd (GNU libc) 2.28`, `highest GLIBC symbol version required: 2.28`, tarball/notices/`--help` checks passed, artifact uploaded | None needed; next step is adding herdr-core. |
| 4 | — | — | pending (app + herdr-core) | — |

## Risks / fallbacks

- **Package names**: RHEL 8 differs from Ubuntu; `libxkbcommon-x11-devel` is
  absent. If X11 headers are still missing, check what `libxkbcommon-devel` and
  `libxkbcommon-x11` provide and adjust.
- **Old image key**: refresh the official GPG keyring from HTTPS; do not
  disable signature checks.
- **JS actions inside the container**: if a pinned action fails on glibc 2.28,
  use plain `run:` steps (`git clone --recurse-submodules`) and `rustup`/`curl`
  for toolchains.
- **herdr-core build**: it uses LTO + `strip` in release mode, so the build may
  take several minutes; keep the 60-minute job timeout. If `v0.1.11` does not
  build with the pinned toolchains, fall back to upstream master (per the
  decision above), and note the core version in the handoff.
- **Runtime compatibility**: glibc 2.28 is the floor, not a guarantee; the app
  still needs a desktop session and a working Vulkan driver (README.md). Test
  the artifact on a real RHEL 8 machine before any future publication.

## Possible follow-ups (not this task)

1. RHEL 8 aarch64 build.
2. Decide whether this tarball joins the release manifest/installer.
3. Optional RPM package.
