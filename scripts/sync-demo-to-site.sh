#!/usr/bin/env bash
# Rebuild the browser demo and copy it into the staas.fund product page.
# Writes the demo to <site>/task-manager/demo/, regenerates
# <site>/task-manager/changelog/ from CHANGELOG.md, and keeps the product
# page's version-specific spots current for the newest release: the
# what's-new strip under the facts grid, the Version cell's "See what
# changed" link and its own version number, the "Version X.Y.Z is on
# GitHub Releases for" sentence, the two direct download links, and the
# JSON-LD SoftwareApplication block's softwareVersion. It never commits or
# pushes: a push to that repo is a deploy, and that stays a human decision.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
site="${1:-$here/../staasfund}"
dest="$site/task-manager/demo"
[ -f "$site/task-manager/index.html" ] || { echo "No product page at $site/task-manager/. Pass the site folder as the first argument." >&2; exit 1; }
cd "$here"
npm run build:demo
[ -f dist-demo/demo.html ] || { echo "npm run build:demo produced no dist-demo/demo.html" >&2; exit 1; }
# Nothing on the site is removed until the build is known to hold what
# replaces it: a build that left no bundle would otherwise empty the demo.
ls dist-demo/assets/demo-*.js >/dev/null 2>&1 || { echo "npm run build:demo produced no dist-demo/assets/demo-*.js" >&2; exit 1; }
ls dist-demo/assets/demo-*.css >/dev/null 2>&1 || { echo "npm run build:demo produced no dist-demo/assets/demo-*.css" >&2; exit 1; }
# Replace only what a build produces: the page and its hashed bundles. Anything
# else that ever lands in the site folder is not ours to delete.
mkdir -p "$dest/assets"
rm -f "$dest/index.html" "$dest"/assets/demo-*.css "$dest"/assets/demo-*.js
cp dist-demo/assets/demo-*.css dist-demo/assets/demo-*.js "$dest/assets/"
cp dist-demo/demo.html "$dest/index.html"
python3 "$here/scripts/bump-demo-version.py" "$site/task-manager/index.html"
python3 "$here/scripts/changelog-page.py" "$site/task-manager/index.html"

echo "Demo copied to $dest. Review, then commit and push the site yourself."
