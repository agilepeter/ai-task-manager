#!/usr/bin/env python3
"""Pin the product page's demo iframe to a fresh version query.

The demo is served from a directory index, and Cloudflare caches that bare URL
for a week: a redeployed demo stayed invisible behind the previous copy. A
version query makes every refreshed copy its own cache key.
"""
import datetime
import pathlib
import re
import sys

page = pathlib.Path(sys.argv[1])
stamp = datetime.datetime.now().strftime("%Y%m%d%H%M")
text = page.read_text()
new = re.sub(r'iframe src="demo/(\?v=\d+)?"', f'iframe src="demo/?v={stamp}"', text, count=1)
if new == text:
    sys.exit(f"could not find the demo iframe in {page}")
page.write_text(new)
print(f"  iframe pinned to ?v={stamp}")
