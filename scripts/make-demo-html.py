#!/usr/bin/env python3
"""Derive demo.html from index.html.

The two were hand-maintained copies and drifted: the browser demo was still
telling people their keys live in `%APPDATA%\\Pane` long after the app stopped
saying it, because every markup fix had to be made twice and the second copy
was forgotten. The demo differs from the app in exactly two ways, so those are
the only two edits applied here.
"""
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent
index = (root / "index.html").read_text()

out = index
# 1. The demo is a public page; it must never be indexed.
out = out.replace(
    "<head>", '<head>\n    <meta name="robots" content="noindex" />', 1
)
# 2. It boots the mocked backend instead of the real one.
before = out
out = out.replace('src="/src/main.ts"', 'src="/src/demo/boot.ts"', 1)
if out == before:
    sys.exit("index.html no longer references /src/main.ts — update this script")

target = root / "demo.html"
if target.exists() and target.read_text() == out:
    print("demo.html already current")
else:
    target.write_text(out)
    print(f"demo.html regenerated from index.html ({len(out.splitlines())} lines)")
