# Plan: RHEL 8-only release build in `release.yml`

Status: **plan only** — the workflow has not been changed yet.

## Goal

Replace the existing `.github/workflows/release.yml` with a **single-job
release build** for **Linux x86_64 on RHEL 8**: build with
`cargo build --release`, pack with `scripts/package`, verify, and upload the
tarball as a GitHub Actions artifact. Trigger with `gh`, watch the run, fix
failures, and repeat until it succeeds.

Explicitly out of scope: full CI/tests, Windows, macOS, aarch64, signing, DMG,
and publishing to the release bucket. `ci.yml` stays untouched.

## Why the current `release.yml` is being replaced

- Its Linux target builds on `ubuntu-22.04` (glibc 2.35) and says in a comment
  that this sets the distro floor. RHEL 8 has glibc 2.28, so that binary will
  not start there.
- It also builds `linux-aarch64`, `macos-aarch64`, and `windows-x86_64`, plus
  signing/DMG/publish jobs for those platforms. None of that is wanted here.
- Local container check already confirmed the target image has glibc 2.28 and
  that the Ubuntu package name `libxkbcommon-x11-devel` does **not** exist on
  AlmaLinux 8 (use `libxkbcommon-devel` + `libxkbcommon-x11`, verify by build).

## Decisions

| Question | Decision |
|---|---|
| Which file | **Replace `.github/workflows/release.yml` in place.** No new workflow file. |
| Platform | One job only, Linux x86_64. No matrix, no Windows/macOS/aarch64 jobs. |
| Environment | GitHub-hosted runner + `container:` pinned to `quay.io/almalinux/almalinux:8@sha256:b068b59649ddecf7c95cea3d3f16941eaf1d8a77fcf6ce94c779abae7552e2ea` (glibc 2.28). |
| Trigger | `workflow_dispatch` so `gh workflow run release.yml --ref <branch>` starts it directly. No tag/publish trigger. |
| Output | `dist/herdr-app-linux-x86_64.tar.gz` uploaded as artifact `herdr-app-rhel8-x86_64`; nothing written to the release bucket or manifest. |
| Where to work | Branch `build/rhel8-release-gha` on fork `shawn111/herdr-app`; no pushes to `upstream`. |
| Repo docs | `defaults/AGENTS.md`, `src/SKILL.md`, and `CHANGELOG.md` unchanged: no user-facing app behavior, and changelog entries are written at release time. |

## Steps

### 1. Commit this plan first

```sh
git switch -c build/rhel8-release-gha
git add docs/rhel8-release-plan.md
git commit -m "docs: plan RHEL 8 release build"
```

### 2. Rewrite `.github/workflows/release.yml`

- Keep `name: build and release` or rename to `rhel8 release build`;
  `permissions: contents: read`, no `environment:` (works from a fork branch
  without release secrets).
- `on: workflow_dispatch`; concurrency group `rhel8-release-${{ github.ref }}`,
  `cancel-in-progress: true`; 60-minute timeout.
- `container:` pinned digest above; `defaults.run.shell: bash`.
- **Bootstrap before checkout** (minimal image; `actions/checkout` needs git):
  `dnf install -y ca-certificates curl tar gzip git python3 binutils`, with a
  bounded retry loop so a broken mirror fails in minutes, not at the timeout.
- Checkout with `submodules: true`, `persist-credentials: false`, pinned to the
  same SHA used in `ci.yml`.
- Toolchain: Rust `1.95.0` and Zig `0.16.0`, same versions and pinned action
  SHAs as in `ci.yml`. If a pinned JS action cannot run in the container, fall
  back to `rustup` plus the official Zig tarball via `curl`, keeping versions.
  Every `uses:` stays pinned to a commit SHA.
- Build deps via `dnf`: `gcc gcc-c++ pkgconf-pkg-config clang clang-devel
  libxcb-devel libxkbcommon-devel libxkbcommon-x11 wayland-devel vulkan-devel
  fontconfig-devel freetype-devel`. Confirm by the actual build that the
  xkbcommon X11 headers GPUI needs are present; adjust the package list if not.
- `scripts/ci-sources restore`, then `scripts/package
  x86_64-unknown-linux-gnu`, which sets `HERDR_APP_RELEASE=1`, runs
  `cargo build --release --locked`, and packs the binary, icon, `LICENSE`, and
  `THIRD-PARTY-NOTICES` into `dist/herdr-app-linux-x86_64.tar.gz`.
- Delete the old `ci-passed`, `sign-windows`, `mac-dmg`, and `publish` jobs;
  they only exist for the platforms/artifacts being removed.
- Verification steps:
  - `ldd --version | head -1` reports glibc 2.28 (proves the container, not the
    Ubuntu host, built it).
  - Highest `GLIBC_*` symbol used by the binary is <= 2.28.
  - `tar -tzf` lists `herdr-app/{herdr-app,herdr-app.svg,LICENSE,THIRD-PARTY-NOTICES}`;
    extract both notices and `cmp` against the repo copies.
  - Smoke test with a scratch home: `HERDR_APP_HOME=/tmp/herdr-app-rhel8-check
    <unpacked>/herdr-app --help` (subcommand only talks; no window).
- Upload with pinned `actions/upload-artifact`, `if-no-files-found: error`,
  7-day retention. No cache on the first pass; add one only after a green run.

### 3. Commit, push, trigger, iterate

```sh
git add .github/workflows/release.yml
git commit -m "ci: build RHEL 8 release on x86_64 only"
git push -u origin build/rhel8-release-gha
gh workflow run release.yml --ref build/rhel8-release-gha
gh run list --workflow release.yml --limit 5
gh run watch <run-id> --exit-status
```

On failure: `gh run view <run-id> --log-failed`, fix the workflow on the same
branch, commit with a lowercase conventional message, push, re-trigger, and
repeat **until the run succeeds**. Keep the scope single-platform while
debugging — don't restore Windows/macOS/CI/publishing to work around a failure.

### 4. Handoff

Report the final green run URL, artifact name, glibc check output, and branch.
Publishing and a real RHEL 8 desktop smoke test remain separate decisions.

## Run log

| # | Commit | Run URL | Result | Fix |
|---|---|---|---|---|
| 1 | `ffeaf41` | https://github.com/shawn111/herdr-app/actions/runs/45259164686 | failed in `Bootstrap container tools`: `GPG check FAILED` (pinned AlmaLinux 8 image has only the old `C21AD6EA` key; current packages use newer keys) | Refresh `/etc/pki/rpm-gpg/RPM-GPG-KEY-AlmaLinux` from `https://repo.almalinux.org/almalinux/RPM-GPG-KEY-AlmaLinux` over HTTPS and `rpm --import` it before `dnf install` (no `nogpgcheck`). |
| 2 | `3756df7` | https://github.com/shawn111/herdr-app/actions/runs/45259697562 | failed in `Keep unchanged sources fresh`: `fatal: detected dubious ownership in repository at '/__w/herdr-app/herdr-app'` (container root vs runner-owned workspace) | Add `git config --global --add safe.directory "$GITHUB_WORKSPACE"` after checkout. |
| 3 | — | — | pending | — |

## Risks / fallbacks

- **Package names**: RHEL 8 differs from Ubuntu; `libxkbcommon-x11-devel` is
  absent. If X11 headers are still missing after the build, check what
  `libxkbcommon-devel` and `libxkbcommon-x11` provide and adjust.
- **JS actions inside the container**: if `actions/checkout` or other pinned
  actions fail on glibc 2.28, use plain `run:` steps (`git clone
  --recurse-submodules`) and `rustup`/`curl` for toolchains.
- **Toolchain support**: verify Rust 1.95.0 and Zig 0.16.0 install on 8.x;
  report a blocker rather than relaxing the glibc check.
- **Runtime compatibility**: glibc 2.28 is the floor, not a guarantee; the app
  still needs a desktop session and a working Vulkan driver (README.md). Test
  the artifact on a real RHEL 8 machine before any future publication.

## Possible follow-ups (not this task)

1. RHEL 8 aarch64 build.
2. Decide whether this tarball joins the release manifest/installer.
3. Optional RPM package.
