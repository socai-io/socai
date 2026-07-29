---
name: socai-release
description: Create, monitor, troubleshoot, and verify socai desktop GitHub releases from the command line with gh. Use when publishing a new release, triggering .github/workflows/release.yml, choosing patch/minor/major bumps, checking release workflow runs, or validating the latest macOS DMG/download redirect without using the GitHub UI.
---

# socai release

Use this skill whenever the task is to create or inspect a socai GitHub Release, trigger the release workflow, publish a new macOS DMG, or avoid clicking through the GitHub Actions / Releases UI.

The command-line release path is the source of truth:

```bash
gh workflow run release.yml --repo socai-io/socai --ref main -f release_type=patch
```

A helper script wraps this command, finds the created run, watches it, and prints the resulting release:

```bash
.claude/skills/socai-release/scripts/create-release.sh patch
```

## What the release workflow does

- Workflow: `.github/workflows/release.yml`
- Trigger: manual `workflow_dispatch`
- Required input: `release_type` = `patch`, `minor`, or `major`
- Production ref: `main`
- Test ref: `fix/release-*` branches only; build runs but publish job is skipped
- Version source: latest strict semver tag matching `vMAJOR.MINOR.PATCH`; if no tag exists, app version from `app/src-tauri/tauri.conf.json`
- Desktop artifacts: `socai-macos-universal.dmg` and `socai-windows-x86_64-setup.exe`, plus the auto-updater set `socai-macos-universal.app.tar.gz` + `.sig`, `socai-windows-x86_64-setup.exe.sig`, and `latest.json` (darwin + windows platforms)
- CLI artifacts: `install.sh`, `socai-cli-macos-universal.tar.gz` + `.sha256`, `install.ps1`, and `socai-cli-windows-x86_64.zip` + `.sha256`
- Alibaba Cloud OSS mirror: every asset is also published to `https://socai-download.oss-cn-beijing.aliyuncs.com/releases/` (`vars.SOCAI_OSS_PUBLIC_BASE_URL` in the workflow) — an immutable `releases/vX.Y.Z/` copy staged before the GitHub release, and a mutable `releases/latest/` copy promoted after it. The OSS copy of `latest.json` is rewritten so its platform URLs point at the OSS `releases/vX.Y.Z/` assets instead of GitHub.
- Production publish steps on `main`:
  1. Build on GitHub Actions, in parallel: universal macOS app + DMG, universal macOS CLI, Windows CLI, and the Windows NSIS installer.
  2. Require Developer ID signing + notarization secrets, and the updater minisign keypair (`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).
  3. Merge the Windows updater fragment into `latest.json`, then stage the versioned release on OSS (`.github/scripts/publish-oss-release.py stage` → `releases/vX.Y.Z/`) before anything is pushed to GitHub.
  4. Update app version files with `.github/scripts/set-app-version.py` and commit `chore: release socai vX.Y.Z` to `main` if needed.
  5. Tag the release commit as `vX.Y.Z`.
  6. Create a draft GitHub Release with generated notes (China-mirror OSS links + GitHub links) and all desktop + CLI assets.
  7. Push updated `main`.
  8. Publish the release by clearing draft status.
  9. Promote the OSS latest release (`publish-oss-release.py promote` → overwrite `releases/latest/`, `latest.json` last).
  10. Run the post-publish verify jobs: verify latest CLI installer (macOS + Windows), verify latest windows desktop installer, and verify desktop updater manifest (`latest.json` from both the OSS mirror and GitHub).

## Safety rules

- Do not use the GitHub web UI for routine releases; use `gh`.
- Do not manually create GitHub releases or upload DMGs unless the workflow is broken and the user explicitly approves a fallback.
- Ask/confirm the release bump before production publishing if the user did not specify `patch`, `minor`, or `major`.
- Production releases must be dispatched from `main`; the workflow rejects other refs except `fix/release-*` test branches.
- Do not cancel an in-progress production release unless the user explicitly asks.
- Do not delete tags/releases unless the workflow failed and the user explicitly approves cleanup.
- If local files are dirty, do not include unrelated changes in release work. The workflow publishes from the remote ref, not local uncommitted files.
- The website redeploys automatically: Vercel Git integration is connected to `socai-io/socai` with production branch `main`, so the `chore: release socai vX.Y.Z` push triggers a production `socai-site` build with no manual step. Do not alter or rerun the release workflow to update `socai.io`.
- Verifying `socai.io` (the `/download`, `/download/macos`, `/download/windows`, and `/github` redirects) is a **mandatory** part of every production release — the release is not done until the redirects are confirmed resolving to the new tag's assets. Fall back to a manual `socai-site-deployment` redeploy only if that verification fails.

## Preflight checks

Run from the repo root when possible.

```bash
gh auth status --hostname github.com
gh repo view socai-io/socai --json nameWithOwner,defaultBranchRef,url

git fetch origin main --tags
git status --short
git log --oneline --decorate -5 origin/main
```

Optional: preview the next version locally using the same bump semantics as the workflow:

```bash
release_type=patch  # patch | minor | major
latest_tag="$(git tag --list 'v*' --sort=-v:refname | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1 || true)"
base_version="${latest_tag#v}"
if [ -z "${base_version}" ]; then
  base_version="$(python3 - <<'PY'
import json
from pathlib import Path
print(json.loads(Path('app/src-tauri/tauri.conf.json').read_text())['version'])
PY
)"
fi
python3 - "${base_version}" "${release_type}" <<'PY'
import sys
base, bump = sys.argv[1:]
major, minor, patch = map(int, base.split('.'))
if bump == 'major':
    major, minor, patch = major + 1, 0, 0
elif bump == 'minor':
    minor, patch = minor + 1, 0
elif bump == 'patch':
    patch += 1
else:
    raise SystemExit(f'bad release_type: {bump}')
print(f'v{major}.{minor}.{patch}')
PY
```

## Create a production release

Preferred helper:

```bash
.claude/skills/socai-release/scripts/create-release.sh patch
```

Equivalent raw `gh` path:

```bash
gh workflow run release.yml \
  --repo socai-io/socai \
  --ref main \
  -f release_type=patch

sleep 8
run_id="$(gh run list \
  --repo socai-io/socai \
  --workflow release.yml \
  --branch main \
  --event workflow_dispatch \
  --limit 1 \
  --json databaseId \
  --jq '.[0].databaseId')"

gh run watch "${run_id}" --repo socai-io/socai --exit-status
```

Use `minor` or `major` instead of `patch` only when requested.

## Verify the website deployment (mandatory)

The website is **not** an optional follow-up — verifying it is a required release gate.

Vercel Git integration is connected to `socai-io/socai` with production branch `main` (project `socai-site`, scope `socai-d83824c8`). The release workflow's push of `chore: release socai vX.Y.Z` to `main` therefore triggers a production `socai-site` build automatically, so no manual deploy is normally needed. The site does not display a version number (the hero release-meta line was removed); the `/download*` redirects point at the Alibaba Cloud OSS mirror's mutable `releases/latest/` objects, which the release workflow's `promote OSS latest release` step overwrites right after the GitHub release is published — the downloads serve the new version as soon as that promotion completes, with no site change involved.

After the GitHub release verification, confirm the live site responds and every redirect resolves. The auto-deploy usually finishes within ~1 minute of the `main` push; allow for that and re-check if it is still mid-build.

```bash
curl -sI https://socai.io/                  | grep -iE '^HTTP'             # 200
curl -sI https://www.socai.io/              | grep -iE '^HTTP|^location'   # 308 -> https://socai.io/
curl -sI https://socai.io/download          | grep -iE '^HTTP|^location'   # 307 -> OSS releases/latest/socai-macos-universal.dmg
curl -sI https://socai.io/download/macos    | grep -iE '^HTTP|^location'   # 307 -> OSS releases/latest/socai-macos-universal.dmg
curl -sI https://socai.io/download/windows  | grep -iE '^HTTP|^location'   # 307 -> OSS releases/latest/socai-windows-x86_64-setup.exe
curl -sI https://socai.io/github            | grep -iE '^HTTP|^location'   # 307 -> github.com/socai-io/socai
```

The release is complete only when all six checks above pass **and** the `/download*` targets serve the just-published tag's build (see the redirect-resolution check in [Verify the published release](#verify-the-published-release)). The OSS `latest/` URLs are stable and do not embed the tag, so that tag check is a content-identity comparison, not a URL match.

If the site has **not** updated after the auto-deploy should have finished, fall back to a manual redeploy: switch to the `socai-site-deployment` skill and deploy the production `socai-site` project with `SOCAI_RELEASE_VERSION=X.Y.Z`. Do not rerun the GitHub release workflow to fix a site-only lag, and do not add website-deploy steps to the release workflow — the Git integration already performs the deploy. Note the redirect destinations are static OSS URLs in `site/vercel.json`: if the redirects respond correctly but `/download*` serves a stale build, the problem is the OSS `releases/latest/` objects (the workflow's `promote OSS latest release` step), and a site redeploy will not fix it.

## Test the release workflow without publishing

Only use a branch named `fix/release-*`. The workflow will build, use ad-hoc signing if production signing secrets are unavailable, and skip the publish job.

```bash
.claude/skills/socai-release/scripts/create-release.sh patch --ref fix/release-some-branch
```

or:

```bash
gh workflow run release.yml \
  --repo socai-io/socai \
  --ref fix/release-some-branch \
  -f release_type=patch
```

## Monitor or troubleshoot an existing run

List recent release runs:

```bash
gh run list --repo socai-io/socai --workflow release.yml --limit 10
```

Watch a run:

```bash
gh run watch RUN_ID --repo socai-io/socai --exit-status
```

View details/logs:

```bash
gh run view RUN_ID --repo socai-io/socai
gh run view RUN_ID --repo socai-io/socai --log-failed
```

Common failure notes:

- `main moved while the release was building`: rerun from the latest `main` after confirming the move was expected.
- Missing Apple secrets on `main`: production release cannot proceed until signing/notarization secrets are configured.
- Build/notarization failure after no `main` push: inspect logs; the workflow attempts to clean up draft release/tag state.
- Failure after `main was already updated`: leave state for manual inspection; do not delete the release/tag without explicit approval.
- Failure in `stage versioned release on Alibaba Cloud OSS`: nothing was published yet (no tag, no release, no `main` push); fix the OSS issue and rerun the workflow from scratch.
- Failure in `promote OSS latest release`: the GitHub release is already live but OSS `releases/latest/` — and therefore the site `/download*` redirects — still serve the previous build. Rerunning the release job trips the `main moved` guard; surface it and only promote manually (`.github/scripts/publish-oss-release.py promote`) with explicit approval.
- A post-publish `verify *` job failure means the release is already published; each verifier retries internally (~2 minutes), so a hard failure is usually a real asset/manifest problem, not propagation lag. Diagnose before touching release state.
- If the `/download*` redirects serve a stale build after a release, check the run's `promote OSS latest release` step first — do not rerun the release for that. Use the `socai-site-deployment` skill (redeploy with `SOCAI_RELEASE_VERSION` set to the published version) only for site-level failures such as bad redirect rules or HTTP errors on `socai.io`.

## Verify the published release

After a successful production run:

```bash
gh release view --repo socai-io/socai \
  --json tagName,name,url,isDraft,isPrerelease,publishedAt,assets \
  --jq '{tagName,name,url,isDraft,isPrerelease,publishedAt,assets:[.assets[].name]}'
```

Expected:

- `isDraft: false`
- `isPrerelease: false`
- Asset list includes the desktop set — `socai-macos-universal.dmg`, `socai-macos-universal.app.tar.gz` + `.sig`, `socai-windows-x86_64-setup.exe` + `.sig`, `latest.json` — and the CLI set — `install.sh`, `socai-cli-macos-universal.tar.gz` + `.sha256`, `install.ps1`, `socai-cli-windows-x86_64.zip` + `.sha256` (12 assets total)

Verify download redirects. The site's `/download*` redirects resolve to the OSS mirror's stable `releases/latest/` URLs, which do not embed the tag — so "redirects resolve to the new tag's assets" means each `latest/` object is byte-identical to the immutable `releases/vX.Y.Z/` copy staged for this release (compare `ETag` / `x-oss-hash-crc64ecma` / `Content-Length`), and/or matches the GitHub asset size:

```bash
tag="$(gh release view --repo socai-io/socai --json tagName --jq '.tagName')"
oss_base="https://socai-download.oss-cn-beijing.aliyuncs.com/releases"

curl -I -L --max-time 30 -o /dev/null -w 'code=%{http_code}\nfinal=%{url_effective}\n' https://socai.io/download
# code=200, final=${oss_base}/latest/socai-macos-universal.dmg

# The OSS latest updater manifest embeds the promoted version:
curl -fsSL -H 'Cache-Control: no-cache' "${oss_base}/latest/latest.json" \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])'   # X.Y.Z of ${tag}

# latest/ objects must be identical to the versioned copies:
for asset in socai-macos-universal.dmg socai-windows-x86_64-setup.exe; do
  latest="$(curl -sI "${oss_base}/latest/${asset}" | tr -d '\r' | grep -iE '^(etag|x-oss-hash-crc64ecma|content-length):' | sort)"
  tagged="$(curl -sI "${oss_base}/${tag}/${asset}" | tr -d '\r' | grep -iE '^(etag|x-oss-hash-crc64ecma|content-length):' | sort)"
  if [ -n "${latest}" ] && [ "${latest}" = "${tagged}" ]; then
    echo "ok: latest/${asset} matches ${tag}"
  else
    printf 'MISMATCH %s\nlatest:\n%s\ntagged:\n%s\n' "${asset}" "${latest}" "${tagged}"
  fi
done

# GitHub's own latest pointer must also resolve to the new tag:
curl -sSI https://github.com/socai-io/socai/releases/latest/download/socai-macos-universal.dmg | grep -F "/releases/download/${tag}/socai-macos-universal.dmg"
curl -sSI https://github.com/socai-io/socai/releases/latest/download/socai-windows-x86_64-setup.exe | grep -F "/releases/download/${tag}/socai-windows-x86_64-setup.exe"
```

Cross-check against GitHub asset sizes if anything looks off (each `size` must equal the matching OSS `Content-Length`):

```bash
gh release view "${tag}" --repo socai-io/socai --json assets \
  --jq '.assets[] | select(.name == "socai-macos-universal.dmg" or .name == "socai-windows-x86_64-setup.exe") | "\(.name) \(.size)"'
```

The OSS checks confirm the mirror the site actually serves; the GitHub checks confirm the GitHub latest pointer (used by the updater fallback and the CLI installers). Confirm the site checks as a mandatory step (see [Verify the website deployment](#verify-the-website-deployment-mandatory)).

Optional artifact check:

```bash
tag="$(gh release view --repo socai-io/socai --json tagName --jq '.tagName')"
mkdir -p "/tmp/socai-release-${tag}"
gh release download "${tag}" \
  --repo socai-io/socai \
  --pattern socai-macos-universal.dmg \
  --dir "/tmp/socai-release-${tag}" \
  --clobber
shasum -a 256 "/tmp/socai-release-${tag}/socai-macos-universal.dmg"
```

## Reporting back

Include:

- Release type (`patch`, `minor`, or `major`)
- Ref used (`main` for production)
- GitHub Actions run URL and conclusion
- Published tag/version and release URL
- Asset presence (desktop: `socai-macos-universal.dmg` + `socai-windows-x86_64-setup.exe`, updater `socai-macos-universal.app.tar.gz` + `.sig`, `socai-windows-x86_64-setup.exe.sig`, `latest.json`; CLI: `install.sh`, `socai-cli-macos-universal.tar.gz` + `.sha256`, `install.ps1`, `socai-cli-windows-x86_64.zip` + `.sha256`)
- `/download` + `/download/macos` + `/download/windows` verification summary
- `socai.io` site verification summary (mandatory): `/`, `www`, `/download`, `/download/macos`, `/download/windows`, `/github` all behave as expected, and the OSS `releases/latest/` download targets match the published tag's staged copies
- Whether the Git-integration auto-deploy was sufficient, or a manual `socai-site-deployment` redeploy fallback was needed
- Any failures, cleanup performed, or manual blockers
