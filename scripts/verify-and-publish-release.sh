#!/usr/bin/env bash
# A tag push leaves a DRAFT release behind; nothing has told an installer
# about it yet. Before that draft becomes the thing `releases/latest`
# points at, this reads it back the way an installer will: the exact
# asset names, the update feed's own latest.json (every platform's url and
# signature), and the changelog section a person will read. Only once all
# of that checks out does it write release notes and flip the draft to
# published. Any check failing prints why and leaves the draft exactly as
# it was -- it never deletes or re-uploads an asset, and never runs `git
# push`.
#
# Usage:
#   verify-and-publish-release.sh vX.Y.Z [--dry-run]
#   verify-and-publish-release.sh vX.Y.Z --fixture DIR [--dry-run]
#
# Plain: reads the real draft for the given tag with `gh`, and on success
# publishes it.
# --dry-run: runs every check and prints the notes it would publish,
# without touching the release.
# --fixture DIR: reads DIR/release.json (shaped like `gh release view
# --json isDraft,assets`), DIR/latest.json, and DIR/CHANGELOG.md (falling
# back to this repo's own CHANGELOG.md if the fixture has none) instead of
# calling `gh`/`curl` against the real repository. A fixture run is always
# a dry run -- synthetic data has nothing real to publish -- which makes
# this the way to exercise every failure case offline.

set -euo pipefail

usage() {
  echo "usage: $(basename "$0") vX.Y.Z [--dry-run] [--fixture DIR]" >&2
}

TAG=""
DRY_RUN=0
FIXTURE_DIR=""

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --fixture)
      [ $# -ge 2 ] || { echo "--fixture needs a directory" >&2; exit 2; }
      FIXTURE_DIR="$2"
      shift 2
      ;;
    --fixture=*)
      FIXTURE_DIR="${1#*=}"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    -*)
      echo "unknown flag: $1" >&2
      usage
      exit 2
      ;;
    *)
      if [ -n "$TAG" ]; then
        echo "unexpected argument: $1" >&2
        usage
        exit 2
      fi
      TAG="$1"
      shift
      ;;
  esac
done

if [ -z "$TAG" ]; then
  usage
  exit 2
fi

# Anchored on purpose: a `case` glob would let "v1.0.0; anything" through,
# because `*` there matches the rest of the line. Nothing downstream is
# interpolated into a shell command, but a script that can publish should
# refuse a tag it does not fully recognise.
if [[ ! "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?$ ]]; then
  echo "FAIL: '$TAG' does not look like a vX.Y.Z tag" >&2
  exit 1
fi
VERSION="${TAG#v}"

if [ -n "$FIXTURE_DIR" ]; then
  # Synthetic data is never a real release: it can only be inspected.
  DRY_RUN=1
fi

HERE="$(cd "$(dirname "$0")/.." && pwd)"
CHANGELOG_PATH="$HERE/CHANGELOG.md"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

fail() {
  echo "FAIL: $1" >&2
  if [ -z "$FIXTURE_DIR" ]; then
    echo "Draft for $TAG left untouched." >&2
  fi
  exit 1
}

# The exact names tauri-action's bundlers and the updater step produce.
# Bash owns this template so the destination check later uses the same
# names the checks below were run against -- one source, not two.
DMG_NAME="AI.Task.Manager_${VERSION}_universal.dmg"
EXE_NAME="AI.Task.Manager_${VERSION}_x64-setup.exe"
MSI_NAME="AI.Task.Manager_${VERSION}_x64_en-US.msi"
LATEST_NAME="latest.json"

if [ -n "$FIXTURE_DIR" ]; then
  [ -f "$FIXTURE_DIR/release.json" ] || { echo "FAIL: fixture has no release.json" >&2; exit 2; }
  [ -f "$FIXTURE_DIR/latest.json" ] || { echo "FAIL: fixture has no latest.json" >&2; exit 2; }
  RELEASE_JSON_FILE="$FIXTURE_DIR/release.json"
  LATEST_JSON_FILE="$FIXTURE_DIR/latest.json"
  [ -f "$FIXTURE_DIR/CHANGELOG.md" ] && CHANGELOG_PATH="$FIXTURE_DIR/CHANGELOG.md"
  if [ -f "$FIXTURE_DIR/repo.txt" ]; then
    REPO_SLUG="$(cat "$FIXTURE_DIR/repo.txt")"
  else
    REPO_SLUG="agilepeter/ai-task-manager"
  fi
else
  REPO_SLUG="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
  RELEASE_JSON_FILE="$WORKDIR/release.json"
  if ! gh release view "$TAG" --json isDraft,assets >"$RELEASE_JSON_FILE" 2>"$WORKDIR/release-view.err"; then
    fail "could not read release $TAG: $(cat "$WORKDIR/release-view.err")"
  fi
  LATEST_JSON_FILE="$WORKDIR/$LATEST_NAME"
  # Tolerate this failing here: if latest.json is genuinely missing from
  # the draft, the asset-name check below reports that clearly. If it is
  # listed but this download still failed, the check after it says so.
  gh release download "$TAG" --pattern "$LATEST_NAME" --dir "$WORKDIR" --clobber \
    >"$WORKDIR/download.log" 2>&1 || true
fi

NOTES_FILE="$WORKDIR/notes.md"

# Every check below reads only local files (the release/asset listing and
# latest.json already fetched above, plus CHANGELOG.md); nothing here
# calls `gh` or the network, which is what makes --fixture a genuine
# offline stand-in for the real thing.
if ! TAG="$TAG" VERSION="$VERSION" REPO_SLUG="$REPO_SLUG" \
     DMG_NAME="$DMG_NAME" EXE_NAME="$EXE_NAME" MSI_NAME="$MSI_NAME" LATEST_NAME="$LATEST_NAME" \
     RELEASE_JSON_FILE="$RELEASE_JSON_FILE" LATEST_JSON_FILE="$LATEST_JSON_FILE" \
     CHANGELOG_PATH="$CHANGELOG_PATH" NOTES_OUT="$NOTES_FILE" \
     python3 <<'PYEOF'
import json
import os
import re
import sys


def fail(msg):
    sys.stderr.write("FAIL: " + msg + "\n")
    sys.exit(1)


tag = os.environ["TAG"]
version = os.environ["VERSION"]
repo_slug = os.environ["REPO_SLUG"]
dmg_name = os.environ["DMG_NAME"]
exe_name = os.environ["EXE_NAME"]
msi_name = os.environ["MSI_NAME"]
latest_name = os.environ["LATEST_NAME"]
changelog_path = os.environ["CHANGELOG_PATH"]
notes_out_path = os.environ["NOTES_OUT"]

with open(os.environ["RELEASE_JSON_FILE"], encoding="utf-8") as f:
    release = json.load(f)

# 1. A published release is never touched again by this script.
if release.get("isDraft") is not True:
    fail(f"release {tag} is not a draft; refusing to touch a published release")

asset_names = {a["name"] for a in release.get("assets", [])}

# 2. The three installers and the update feed itself, by exact name.
for want in (dmg_name, exe_name, msi_name, latest_name):
    if want not in asset_names:
        fail(f"release asset missing from the draft: {want}")

latest_json_path = os.environ["LATEST_JSON_FILE"]
if not os.path.isfile(latest_json_path):
    fail(f"{latest_name} is listed on the draft but could not be read locally")

with open(latest_json_path, encoding="utf-8") as f:
    latest = json.load(f)

# 3. latest.json's own version must match the tag being published.
if latest.get("version") != version:
    fail(f"latest.json version is {latest.get('version')!r}, expected {version!r}")

platforms = latest.get("platforms")
if not platforms:
    fail("latest.json has no platforms")

# 4. The three keys that point at real installers (as opposed to the
# macOS "-app" update-bundle aliases) must be present.
for key in ("windows-x86_64", "windows-x86_64-msi", "windows-x86_64-nsis"):
    if key not in platforms:
        fail(f"latest.json is missing the {key} platform entry")

# 5. Every platform entry: its url is this repo's own download url for
# this tag, the file it names is really on the draft, and it carries a
# signature.
prefix = f"https://github.com/{repo_slug}/releases/download/{tag}/"
referenced = set()
for key in sorted(platforms):
    entry = platforms[key]
    url = entry.get("url", "")
    if not url.startswith(prefix):
        fail(f"platform {key} url does not start with {prefix}: {url}")
    basename = url.rsplit("/", 1)[-1]
    if basename not in asset_names:
        fail(f"platform {key} url points at {basename}, which is not an asset on the draft")
    if not entry.get("signature"):
        fail(f"platform {key} has an empty signature")
    referenced.add(basename)

# 6. Every file latest.json points at also has its own detached .sig
# asset (the standalone signature file tauri-action uploads beside it).
for basename in sorted(referenced):
    sig_name = basename + ".sig"
    if sig_name not in asset_names:
        fail(f"{sig_name} is missing from the draft, but latest.json references {basename}")

# 7. The changelog has a dated section for this version, with a body.
with open(changelog_path, encoding="utf-8") as f:
    lines = f.readlines()

heading_index = None
trailing = ""
for i, line in enumerate(lines):
    m = re.match(r'^##\s+(\S+)(?:\s+(.*?))?\s*$', line)
    if m and m.group(1) == version:
        heading_index = i
        trailing = (m.group(2) or "").strip()
        break

if heading_index is None:
    fail(f"CHANGELOG.md has no '## {version}' section")

date_match = re.match(r'^[-–—]\s*(\d{4}-\d{2}-\d{2})\s*$', trailing)
if not date_match:
    fail(f"CHANGELOG.md section for {version} is not dated (found heading text: {trailing!r})")

body_lines = []
for line in lines[heading_index + 1:]:
    if re.match(r'^##\s+\S', line):
        break
    body_lines.append(line)
body = "".join(body_lines).strip("\n")
if not body.strip():
    fail(f"CHANGELOG.md section for {version} has no content")


def declaw(text):
    # House style: no em dashes, no arrow glyphs, in anything a stranger reads.
    return text.replace("—", "-").replace("→", "->")


install_block = f"""## Install

**macOS, Intel and Apple Silicon (one universal build):** download `{dmg_name}`, open it and drag AI Task Manager to Applications. The app is not notarized yet, so the first launch needs a right-click on the app and Open, once; or run `xattr -dr com.apple.quarantine "/Applications/AI Task Manager.app"`.

**Windows 10 and 11:** download `{exe_name}` (or the `.msi`). The installer is not signed yet, so SmartScreen will warn: More info, then Run anyway.

Installs with update checks on are offered this release at their next launch or popover open, or within four hours. Update checks stay off by default; with them off, download and install over the old copy.

Both installers were built by this repository's own GitHub Actions workflow from the tagged commit. Nothing leaves your computer: [docs/privacy.md](https://github.com/{repo_slug}/blob/{tag}/docs/privacy.md) lists every network call the app can make and the commands that prove it."""

notes = declaw(install_block) + f"\n\n## What is in {version}\n\n" + declaw(body) + "\n"

with open(notes_out_path, "w", encoding="utf-8") as f:
    f.write(notes)
PYEOF
then
  if [ -z "$FIXTURE_DIR" ]; then
    echo "Draft for $TAG left untouched." >&2
  fi
  exit 1
fi

if [ "$DRY_RUN" -eq 1 ]; then
  if [ -n "$FIXTURE_DIR" ]; then
    echo "FIXTURE OK ($FIXTURE_DIR): every check passed. Composed notes follow."
  else
    echo "DRY RUN OK ($TAG): every check passed; nothing was published. Composed notes follow."
  fi
  echo "---"
  cat "$NOTES_FILE"
  exit 0
fi

gh release edit "$TAG" \
  --title "AI Task Manager $VERSION" \
  --notes-file "$NOTES_FILE" \
  --draft=false \
  --latest

echo "Published $TAG. Verifying the update feed and installers resolve at the destination..."

read -r LATEST_REDIRECT_CODE LATEST_REDIRECT_URL <<<"$(
  curl -sI -o /dev/null -w '%{http_code} %{redirect_url}' \
    "https://github.com/$REPO_SLUG/releases/latest/download/$LATEST_NAME"
)"
EXPECTED_LATEST_REDIRECT="https://github.com/$REPO_SLUG/releases/download/$TAG/$LATEST_NAME"
if [ "$LATEST_REDIRECT_CODE" != "302" ] || [ "$LATEST_REDIRECT_URL" != "$EXPECTED_LATEST_REDIRECT" ]; then
  fail "releases/latest/download/$LATEST_NAME did not resolve to $TAG (got $LATEST_REDIRECT_CODE -> $LATEST_REDIRECT_URL)"
fi

for name in "$DMG_NAME" "$EXE_NAME" "$MSI_NAME"; do
  code="$(curl -s -o /dev/null -w '%{http_code}' "https://github.com/$REPO_SLUG/releases/download/$TAG/$name")"
  if [ "$code" != "302" ]; then
    fail "releases/download/$TAG/$name returned $code, expected 302"
  fi
done

echo "Verified: latest.json resolves to $TAG and all three installers are downloadable."
