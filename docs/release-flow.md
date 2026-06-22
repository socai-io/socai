# Release flow

Socai releases are produced by the GitHub Actions workflow at
[`.github/workflows/release.yml`](../.github/workflows/release.yml). The normal
operator entrypoint is the release helper documented in
[`.claude/skills/socai-release/SKILL.md`](../.claude/skills/socai-release/SKILL.md):

```bash
.claude/skills/socai-release/scripts/create-release.sh patch
```

Use `minor` or `major` only when the release intentionally changes that part of
semver. Do not create releases or upload assets manually unless the workflow is
broken and the fallback is explicitly approved.

## High-level shape

The production release workflow is manually dispatched from `main` and fans out
platform builds after computing the next version:

```text
prepare
  ├─ build-macos-app-dmg    # macOS app bundle + signed/notarized DMG
  ├─ build-macos-cli        # macOS universal CLI
  └─ build-windows-cli      # Windows x86_64 CLI

release                    # waits for all platform build jobs
  ├─ verify-cli-installer          # macOS latest-release installer smoke
  ├─ verify-windows-cli-installer  # Windows latest-release installer smoke
  └─ verify-desktop-updater        # desktop latest.json manifest check
```

The macOS app/DMG, macOS CLI, and Windows CLI build jobs run in parallel. The
`release` job stays centralized so all platform artifacts are collected into one
GitHub Release atomically.

## Versioning

`prepare` determines the next strict semver tag from the latest `vMAJOR.MINOR.PATCH`
tag and the requested bump:

- `patch` → `vX.Y.(Z+1)`
- `minor` → `vX.(Y+1).0`
- `major` → `v(X+1).0.0`

Each build applies that version with `.github/scripts/set-app-version.py` before
compiling, so `socai --version` and the desktop metadata match the release tag.
The publish job commits the same version metadata back to `main` as:

```text
chore: release socai vX.Y.Z
```

and tags that commit as `vX.Y.Z`.

## Build outputs

### macOS app/DMG build job

`build-macos-app-dmg` runs on `macos-14` and produces:

- signed/notarized universal desktop DMG
- `socai-macos-universal.dmg`
- `socai-macos-universal.app.tar.gz` — the auto-updater bundle (the notarized +
  stapled `.app`, tarred)
- `socai-macos-universal.app.tar.gz.sig` — its minisign signature
- `latest.json` — the updater manifest the desktop app polls (both
  `darwin-aarch64` and `darwin-x86_64` point at the one universal tarball)

The job verifies:

- app bundle and DMG signatures/notarization
- DMG can be mounted
- mounted app bundle verifies successfully
- app binary contains both `arm64` and `x86_64`

The `.app` is notarized **and stapled** in its own right (not just inside the
DMG) because the in-app updater swaps the bundle in directly — see
[Update behavior](#update-behavior).

#### Updater signing secrets

On `main`, in addition to the Apple Developer ID / notarization secrets, the job
**requires** a Tauri minisign keypair for the desktop auto-updater:

- `TAURI_SIGNING_PRIVATE_KEY` — minisign private key (from `tauri signer generate`)
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — its password

The matching public key is committed in
[`app/src-tauri/tauri.conf.json`](../app/src-tauri/tauri.conf.json) under
`plugins.updater.pubkey`. `fix/release-*` test branches skip the updater
artifacts when the key is absent.

### macOS CLI build job

`build-macos-cli` runs on `macos-14` and produces:

- universal macOS CLI binary built from:
  - `aarch64-apple-darwin`
  - `x86_64-apple-darwin`
- `socai-cli-macos-universal.tar.gz`
- `socai-cli-macos-universal.tar.gz.sha256`
- `install.sh`

The job verifies:

- CLI contains both `arm64` and `x86_64`
- `socai --version == socai X.Y.Z`
- archive contains `socai` and `manifest.json`
- checksum and manifest metadata

### Windows CLI build job

`build-windows-cli` runs on `windows-latest` and produces:

- `socai-cli-windows-x86_64.zip`
- `socai-cli-windows-x86_64.zip.sha256`
- `install.ps1`

The job verifies:

- `socai.exe --version == socai X.Y.Z`
- native Windows daemon starts and writes `rust-daemon-endpoint.json`
- `socai.exe stop` stops the daemon and the daemon exits successfully
- zip extraction works
- extracted `socai.exe --version` matches the release version
- manifest metadata matches version, target, base SHA, and build SHA

## GitHub Release assets

The publish job requires every expected artifact before it creates the release.
A production release should contain:

```text
install.sh
socai-cli-macos-universal.tar.gz
socai-cli-macos-universal.tar.gz.sha256
install.ps1
socai-cli-windows-x86_64.zip
socai-cli-windows-x86_64.zip.sha256
socai-macos-universal.dmg
socai-macos-universal.app.tar.gz
socai-macos-universal.app.tar.gz.sig
latest.json
```

The release notes link to the app DMG, macOS CLI installer/archive/checksum, and
Windows CLI installer/archive/checksum. `latest.json`,
`socai-macos-universal.app.tar.gz`, and its `.sig` back the in-app auto-updater
and are served via the same `releases/latest/download/...` alias.

## Installer smoke tests

Installer smoke tests run **after** the GitHub Release is published because they
use GitHub's real `releases/latest/download/...` URLs.

### macOS installer verification

`verify-cli-installer` runs on `macos-14` and executes:

```bash
curl -fsSL https://github.com/socai-io/socai/releases/latest/download/install.sh | sh
```

with a temporary `HOME` and `SOCAI_INSTALL_DIR`. It verifies:

- installed `socai --version` matches the release version
- `socai --help` works
- the installer wrote the expected PATH line to the temp shell rc file

### Windows installer verification

`verify-windows-cli-installer` runs on `windows-latest` and executes:

```powershell
$installer = Join-Path $env:TEMP 'socai-install.ps1'; Invoke-WebRequest -UseBasicParsing https://github.com/socai-io/socai/releases/latest/download/install.ps1 -OutFile $installer; Unblock-File $installer; & $installer
```

with a temporary `SOCAI_INSTALL_DIR`. It verifies:

- installed `socai.exe --version` matches the release version
- `socai.exe --help` works
- `socai.exe version --no-check` works

### Desktop updater verification

`verify-desktop-updater` runs on `ubuntu-latest` and fetches
`releases/latest/download/latest.json`. It verifies:

- the manifest `version` matches the release version
- each `darwin-*` platform has a non-empty signature
- each platform's updater tarball URL resolves (HTTP `HEAD`)

## Update behavior

- **Desktop app (macOS):** the app checks `latest.json` on launch via the Tauri
  updater plugin and surfaces an "update available" affordance in the header.
  Accepting it downloads the signed `socai-macos-universal.app.tar.gz`, verifies
  it against the bundled minisign public key, swaps the bundle in place, and
  relaunches. The first updater-enabled release cannot be auto-updated *to* — it
  must be installed manually; every release after it auto-updates.
- macOS managed CLI installs can use `socai update`.
- Windows managed installs should rerun the PowerShell installer for now. Native
  Windows `socai update` is intentionally not enabled until we add a detached
  updater that can safely replace a running `socai.exe`.
- Source/Cargo installs are updated from the checkout with `cargo install --path
  cli --force`.

## Website follow-up

The release workflow only publishes GitHub Release assets. The marketing site is
a separate Vercel deployment documented in
[`docs/website-deployment.md`](website-deployment.md). After a successful release,
verify whether `socai.io` has deployed the release commit and shows the new
version; deploy or redeploy the site separately if needed.
