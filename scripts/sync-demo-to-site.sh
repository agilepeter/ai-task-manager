#!/usr/bin/env bash
# Rebuild the browser demo and copy it into the staas.fund product page.
# Writes to exactly one folder: <site>/task-manager/demo/. It never commits or
# pushes: a push to that repo is a deploy, and that stays a human decision.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
site="${1:-$here/../staasfund}"
dest="$site/task-manager/demo"
[ -f "$site/task-manager/index.html" ] || { echo "No product page at $site/task-manager/. Pass the site folder as the first argument." >&2; exit 1; }
cd "$here"
npm run build:demo
[ -f dist-demo/demo.html ] || { echo "npm run build:demo produced no dist-demo/demo.html" >&2; exit 1; }
# Replace only what a build produces: the page and its hashed bundles. Anything
# else that ever lands in the site folder is not ours to delete.
mkdir -p "$dest/assets"
rm -f "$dest/index.html" "$dest"/assets/demo-*.css "$dest"/assets/demo-*.js
cp dist-demo/assets/demo-*.css dist-demo/assets/demo-*.js "$dest/assets/"
cp dist-demo/demo.html "$dest/index.html"
python3 "$here/scripts/bump-demo-version.py" "$site/task-manager/index.html"

echo "Demo copied to $dest. Review, then commit and push the site yourself."
