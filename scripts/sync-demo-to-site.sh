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
rm -rf "$dest"
mkdir -p "$dest"
cp -R dist-demo/assets "$dest/assets"
cp dist-demo/demo.html "$dest/index.html"
echo "Demo copied to $dest. Review, then commit and push the site yourself."
