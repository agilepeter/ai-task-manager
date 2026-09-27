#!/usr/bin/env python3
"""Renders the changelog page from CHANGELOG.md and keeps it honest.

The product page and the changelog page share one visual system. Rather
than hand-maintain a second copy of the nav, the footer and the tm-*
styles, this reads them straight out of task-manager/index.html every time
it runs, the same way make-demo-html.py derives demo.html from index.html
instead of a second hand-edited copy that would drift the first time
someone forgot the second edit. A relative link lifted from a page one
directory shallower needs one more '../' to still resolve from here; that
single mechanical fix is applied wherever such a link is reused below,
exactly as it already has to be for the workshop.css and favicon links in
the head.

    python3 scripts/changelog-page.py <path-to-task-manager/index.html>
        Writes <site-root>/task-manager/changelog/index.html. The first
        time only, also adds two empty marker comments to the product page
        for a later change to fill in; once they exist this never touches
        them again.

    python3 scripts/changelog-page.py --check <path-to-task-manager/index.html>
        The drift gate: renders to memory and exits 1 if the page already
        on disk would come out any different today.

    python3 scripts/changelog-page.py --selftest
        Exercises a fixture changelog and a fixture product page against a
        temp directory. Stdlib only; there is no pytest here to run.
"""
from __future__ import annotations

import html
import json
import pathlib
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass

SCRIPT_PATH = pathlib.Path(__file__).resolve()
REPO_URL = "https://github.com/agilepeter/ai-task-manager"
CHANGELOG_URL = "https://staas.fund/task-manager/changelog/"
DEFAULT_CHANGELOG = SCRIPT_PATH.parent.parent / "CHANGELOG.md"
MARK_START = "<!-- whats-new:start -->"
MARK_END = "<!-- whats-new:end -->"


@dataclass
class Release:
    version: str
    date: str
    sections: dict[str, list[str]]  # keys in file order, e.g. "Added", "Changed", "Fixed"


# ---------------------------------------------------------------------------
# Parsing CHANGELOG.md
# ---------------------------------------------------------------------------

_VERSION_RE = re.compile(r"^## (\S+) — (\d{4}-\d{2}-\d{2})\s*$", re.MULTILINE)
_SECTION_RE = re.compile(r"^### (.+?)\s*$", re.MULTILINE)


def parse_changelog(text: str) -> list[Release]:
    """One Release per '## <version> — <date>' heading. The file is
    already written newest first, so this only ever preserves that order;
    it never sorts by version number itself. The intro paragraph above the
    first heading, and every other section kind a version happens to use
    (Added, Changed, Fixed, Removed, whatever heading is actually there),
    is left for the caller to decide what to do with."""
    headers = list(_VERSION_RE.finditer(text))
    releases = []
    for i, m in enumerate(headers):
        body_end = headers[i + 1].start() if i + 1 < len(headers) else len(text)
        body = text[m.end():body_end]
        releases.append(Release(version=m.group(1), date=m.group(2), sections=_parse_sections(body)))
    return releases


def _parse_sections(body: str) -> dict[str, list[str]]:
    headers = list(_SECTION_RE.finditer(body))
    sections: dict[str, list[str]] = {}
    for i, m in enumerate(headers):
        end = headers[i + 1].start() if i + 1 < len(headers) else len(body)
        sections[m.group(1).strip()] = _parse_bullets(body[m.end():end])
    return sections


def _parse_bullets(text: str) -> list[str]:
    """A Markdown bullet can wrap across several lines; every line that is
    not itself a new bullet is folded onto the previous one with a single
    space, the way a soft-wrapped paragraph reads back as one sentence."""
    bullets: list[str] = []
    for line in text.splitlines():
        if line.startswith("- "):
            bullets.append(line[2:].rstrip())
        elif line.strip() and bullets:
            bullets[-1] += " " + line.strip()
    return bullets


# ---------------------------------------------------------------------------
# Inline Markdown -> HTML
# ---------------------------------------------------------------------------

_CODE_RE = re.compile(r"`([^`]+)`")
_BOLD_RE = re.compile(r"\*\*(.+?)\*\*")
_LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)]+)\)")
_STASH_RE = re.compile("\x00(\\d+)\x00")


def render_inline(md: str) -> str:
    """**bold** leads, `code`, and [text](url) links become their HTML;
    everything else comes out as escaped text. An em dash and an arrow are
    part of the changelog's own prose style, not this page's, so both
    become plain punctuation here instead of carrying the glyph through.
    Code spans are pulled out before bold and link parsing run, and put
    back afterwards, so a stray '**' or '[' inside a code sample can never
    be mistaken for Markdown around it."""
    text = html.escape(md, quote=False).replace("—", " - ").replace("→", "->")
    stashed: list[str] = []

    def stash(m: re.Match) -> str:
        stashed.append(f"<code>{m.group(1)}</code>")
        return f"\x00{len(stashed) - 1}\x00"

    text = _CODE_RE.sub(stash, text)
    text = _LINK_RE.sub(lambda m: f'<a href="{m.group(2)}" rel="noopener">{m.group(1)}</a>', text)
    text = _BOLD_RE.sub(lambda m: f"<strong>{m.group(1)}</strong>", text)
    return _STASH_RE.sub(lambda m: stashed[int(m.group(1))], text)


def anchor_id(version: str) -> str:
    return "v" + version.replace(".", "-")


# ---------------------------------------------------------------------------
# Pulling shared markup out of the product page, instead of a second
# hand-maintained copy that could say something the real page no longer does
# ---------------------------------------------------------------------------

def _extract(html_text: str, open_marker: str, close_marker: str) -> str:
    start = html_text.index(open_marker)
    end = html_text.index(close_marker, start) + len(close_marker)
    return html_text[start:end]


def _find_href(html_text: str, filename: str) -> str:
    m = re.search(r'(?:href|src)="([^"]*' + re.escape(filename) + r')"', html_text)
    if not m:
        raise ValueError(f"product page has no link to {filename!r}")
    return m.group(1)


def _one_level_deeper(rel: str) -> str:
    """The changelog page lives one directory below the product page, so a
    same-site relative link taken from there needs one more '../' to still
    land on the right file; a root-relative or absolute one needs nothing
    done to it at all."""
    return "../" + rel if rel.startswith("../") else rel


def _rebase_relative_links(fragment: str) -> str:
    """Applies that same one-more-'../' fix across a whole block of markup
    lifted from the product page (the nav, the footer), rather than
    leaving every relative link inside it one level too shallow to
    resolve from the changelog page's own, deeper, address."""
    return re.sub(r'(href|src)="\.\./', r'\1="../../', fragment)


def _extract_software_application(product_page_html: str) -> dict:
    blocks = re.findall(r'<script type="application/ld\+json">(.*?)</script>', product_page_html, re.DOTALL)
    for block in blocks:
        try:
            data = json.loads(block)
        except json.JSONDecodeError:
            continue
        if data.get("@type") == "SoftwareApplication":
            return data
    raise ValueError("product page has no SoftwareApplication ld+json block")


# ---------------------------------------------------------------------------
# Page assembly
# ---------------------------------------------------------------------------

_PAGE_STYLE = """
    /* Layout for the changelog's own sections, on top of the shared .tm
       system copied from the product page above: that block already sets
       the colours, the type and the .tm-sec rhythm; this just teaches it
       what a release section and its lists look like. */
    .tm-changelog-note { max-width: var(--measure); margin-top: 18px; }
    .tm-sec h3 { font-size: 13px; letter-spacing: .08em; text-transform: uppercase;
                 color: var(--ink-3); font-weight: 500; margin: 28px 0 14px; }
    .tm-release .tm-sec-head { margin-bottom: 8px; }
    .tm-release h3:first-of-type { margin-top: 0; }
    .tm-release ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 10px; }
    .tm-release li { padding-left: 18px; position: relative; color: var(--ink-2);
                      line-height: 1.6; font-size: 15.5px; }
    .tm-release li::before { content: ""; position: absolute; left: 0; top: .68em;
                              width: 7px; height: 1px; background: var(--ink-3); }
    .tm-release li strong { color: var(--ink); font-weight: 600; }
    .tm-release li code { font-size: .92em; background: var(--bg-3); border: 1px solid var(--line);
                           border-radius: 4px; padding: .1em .35em; }
    .tm-release-link { display: inline-block; margin-top: 22px; font-size: 14px; font-weight: 500; }
"""


def _render_release_section(release: Release) -> str:
    heading = f"{html.escape(release.version)} - {html.escape(release.date)}"
    parts = [
        f'<section class="tm-sec tm-release" id="{anchor_id(release.version)}">',
        '<div class="tm-wrap">',
        f'<div class="tm-sec-head"><h2>{heading}</h2></div>',
    ]
    for kind, bullets in release.sections.items():
        parts.append(f"<h3>{html.escape(kind)}</h3>")
        parts.append("<ul>")
        parts.extend(f"<li>{render_inline(b)}</li>" for b in bullets)
        parts.append("</ul>")
    tag_url = f"{REPO_URL}/releases/tag/v{release.version}"
    parts.append(f'<p><a class="tm-release-link" href="{html.escape(tag_url)}" rel="noopener">Release notes on GitHub</a></p>')
    parts.append("</div></section>")
    return "\n".join(parts)


def render_page(releases: list[Release], product_page_html: str) -> str:
    nav = _rebase_relative_links(_extract(product_page_html, "<header", "</header>"))
    footer = _rebase_relative_links(_extract(product_page_html, "<footer", "</footer>"))
    shared_style = _extract(product_page_html, "<style>", "</style>")
    workshop_css = _one_level_deeper(_find_href(product_page_html, "workshop.css"))
    apple_touch = _one_level_deeper(_find_href(product_page_html, "halperbot-touch.png"))
    favicon = _one_level_deeper(_find_href(product_page_html, "halperbot-favicon.png"))

    ld = _extract_software_application(product_page_html)
    ld["releaseNotes"] = CHANGELOG_URL
    ld["url"] = CHANGELOG_URL
    ld_json = json.dumps(ld, indent=2)

    upstream_url = f"{REPO_URL}/blob/main/docs/upstream-changelog.md"
    sections_html = "\n".join(_render_release_section(r) for r in releases)

    return f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>AI Task Manager: Changelog</title>
  <meta name="description" content="Every release of AI Task Manager, version by version: what was added, changed and fixed.">
  <meta property="og:title" content="AI Task Manager: Changelog">
  <meta property="og:description" content="Every release of AI Task Manager, version by version: what was added, changed and fixed.">
  <meta property="og:type" content="website">
  <meta property="og:url" content="{CHANGELOG_URL}">
  <meta property="og:image" content="https://staas.fund/task-manager/icon-512.png">
  <meta property="og:image:alt" content="AI Task Manager app icon: a silver robot face with a smile made of usage bars">
  <meta property="og:site_name" content="StaaS Fund">
  <meta property="og:locale" content="en_US">
  <meta name="twitter:card" content="summary">
  <meta name="twitter:title" content="AI Task Manager: Changelog">
  <meta name="twitter:description" content="Every release of AI Task Manager, version by version: what was added, changed and fixed.">
  <meta name="twitter:image" content="https://staas.fund/task-manager/icon-512.png">
  <meta name="twitter:site" content="@agilepeter">
  <meta name="twitter:creator" content="@agilepeter">
  <meta name="author" content="Peter Saddington">
  <meta name="robots" content="index, follow, max-image-preview:large, max-snippet:-1">
  <link rel="canonical" href="{CHANGELOG_URL}">
  <link rel="apple-touch-icon" href="{apple_touch}">
  <link rel="preconnect" href="https://fonts.googleapis.com">
  <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&family=Fraunces:ital,opsz,wght@0,9..144,400..600;1,9..144,400..500&display=swap" rel="stylesheet">
  <link rel="stylesheet" href="{workshop_css}">
  <link rel="icon" type="image/png" href="{favicon}">
  <script type="application/ld+json">
  {ld_json}
  </script>
  {shared_style}
  <style>{_PAGE_STYLE}</style>
</head>
<body>
  <a class="tm-skip" href="#main">Skip to content</a>
  {nav}
  <main class="tm" id="main">
    <section class="tm-sec" aria-labelledby="h-changelog-hero">
      <div class="tm-wrap">
        <div class="tm-sec-head">
          <h1 id="h-changelog-hero">What&rsquo;s <em>changed</em>.</h1>
          <p class="tm-changelog-note">This app&rsquo;s own numbering restarts at 0.1.0 from a clean copy of Pane 0.4.52; Pane&rsquo;s release history up to that point is kept verbatim in the <a href="{upstream_url}" rel="noopener">upstream changelog on GitHub</a>.</p>
        </div>
      </div>
    </section>
{sections_html}
  </main>
  {footer}
</body>
</html>
"""


# ---------------------------------------------------------------------------
# The product page's own "what's new" strip: markers now, content later
# ---------------------------------------------------------------------------

def ensure_whats_new_markers(product_page_html: str) -> str:
    """Adds the two empty marker comments right after the facts strip, the
    one time they are missing, so a later change has a fixed place to
    splice a summary into without ever having to touch this generator
    again. Once they exist, this leaves the file alone; it never looks at
    what, if anything, a later run has put between them."""
    if MARK_START in product_page_html:
        return product_page_html
    anchor = '<dl class="tm-facts">'
    start = product_page_html.index(anchor)
    close = product_page_html.index("</dl>", start) + len("</dl>")
    markers = f"\n      {MARK_START}\n      {MARK_END}"
    return product_page_html[:close] + markers + product_page_html[close:]


def render_whats_new(latest: Release) -> str:
    """The strip's inner HTML for the most recent release. Nothing splices
    this into the product page yet, only ever the two empty markers are,
    but it is defined now so the change that does the splicing later has
    something real, already exercised by this file's own selftest, to
    call rather than writing it from scratch under time pressure."""
    items = [b for bullets in latest.sections.values() for b in bullets]
    lis = "".join(f"<li>{render_inline(b)}</li>" for b in items)
    return (
        f'<p class="tm-whats-new-version">Version {html.escape(latest.version)} '
        f'<span class="tm-whats-new-date">{html.escape(latest.date)}</span></p>'
        f'<ul class="tm-whats-new-list">{lis}</ul>'
        f'<p class="tm-whats-new-link"><a href="{CHANGELOG_URL}#{anchor_id(latest.version)}" rel="noopener">'
        f'Full changelog</a></p>'
    )


def splice(product_page_html: str, inner: str) -> str:
    """Replaces whatever sits between the two marker comments with inner,
    leaving the rest of the page untouched. A missing or repeated marker
    means the page is not in the shape this expects, so it refuses rather
    than guessing where the strip belongs."""
    if product_page_html.count(MARK_START) != 1 or product_page_html.count(MARK_END) != 1:
        raise ValueError("whats-new markers are missing or appear more than once")
    start = product_page_html.index(MARK_START) + len(MARK_START)
    end = product_page_html.index(MARK_END)
    if end < start:
        raise ValueError("whats-new end marker appears before its start marker")
    return product_page_html[:start] + "\n" + inner + "\n" + product_page_html[end:]


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def _usage() -> str:
    return (
        "usage: changelog-page.py <product-page-path> [--changelog PATH] [--site-root PATH]\n"
        "       changelog-page.py --check <product-page-path> [--changelog PATH] [--site-root PATH]\n"
        "       changelog-page.py --selftest"
    )


def _parse_args(argv: list[str]):
    check = False
    changelog = None
    site_root = None
    positional = []
    i = 0
    while i < len(argv):
        arg = argv[i]
        if arg == "--check":
            check = True
        elif arg == "--changelog":
            i += 1
            changelog = argv[i]
        elif arg == "--site-root":
            i += 1
            site_root = argv[i]
        else:
            positional.append(arg)
        i += 1
    if len(positional) != 1:
        sys.exit(_usage())
    return check, positional[0], changelog, site_root


def _changelog_target(site_root: pathlib.Path) -> pathlib.Path:
    return site_root / "task-manager" / "changelog" / "index.html"


def _check(releases: list[Release], product_page_html: str, site_root: pathlib.Path) -> int:
    rendered = render_page(releases, product_page_html)
    target = _changelog_target(site_root)
    if not target.exists():
        print(f"stale: {target} does not exist; run changelog-page.py to generate it", file=sys.stderr)
        return 1
    if target.read_text() != rendered:
        print(f"stale: {target} does not match what CHANGELOG.md renders today", file=sys.stderr)
        return 1
    return 0


def main(argv: list[str]) -> int:
    if argv[:1] == ["--selftest"]:
        return selftest()

    check, product_page_arg, changelog_arg, site_root_arg = _parse_args(argv)
    product_page_path = pathlib.Path(product_page_arg).resolve()
    changelog_path = pathlib.Path(changelog_arg).resolve() if changelog_arg else DEFAULT_CHANGELOG
    if not product_page_path.exists():
        print(f"no such product page: {product_page_path}", file=sys.stderr)
        return 1
    if not changelog_path.exists():
        print(f"no such changelog: {changelog_path}", file=sys.stderr)
        return 1
    site_root = pathlib.Path(site_root_arg).resolve() if site_root_arg else product_page_path.parent.parent

    releases = parse_changelog(changelog_path.read_text())
    product_page_html = product_page_path.read_text()

    if check:
        return _check(releases, product_page_html, site_root)

    target = _changelog_target(site_root)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(render_page(releases, product_page_html))
    print(f"wrote {target}")

    updated = ensure_whats_new_markers(product_page_html)
    if updated != product_page_html:
        product_page_path.write_text(updated)
        print(f"wrote {product_page_path} (added the whats-new markers)")
    return 0


# ---------------------------------------------------------------------------
# Selftest
# ---------------------------------------------------------------------------

FIXTURE_CHANGELOG = """# Changelog

Intro paragraph, never rendered on the page.

## 0.1.2 — 2026-09-26

### Added

- **Bold lead.** Text with `inline code`, a [link](https://example.com/page),
  plus an em dash—here and an arrow→there.

### Changed

- Handles `<Config>` safely, even with A & B mixed in.

### Fixed

- A plain fix with no frills.

## 0.1.1 — 2026-09-25

### Added

- Earlier release, one bullet.
"""

FIXTURE_PRODUCT_PAGE = """<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <link rel="apple-touch-icon" href="../ai-workshop/halperbot-touch.png">
  <link rel="stylesheet" href="../workshop.css">
  <link rel="icon" type="image/png" href="../ai-workshop/halperbot-favicon.png">
  <script type="application/ld+json">
  {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    "name": "AI Task Manager",
    "softwareVersion": "0.1.2",
    "url": "https://staas.fund/task-manager/"
  }
  </script>
  <style>
    .tm { --ink: #fff; }
    .tm-sec { padding: 40px 0; }
  </style>
</head>
<body>
  <a class="tm-skip" href="#main">Skip to content</a>
  <header class="workshop-header">
    <nav class="workshop-nav" aria-label="Workshop navigation">
      <a href="/" class="nav-logo">staas.fund</a>
      <img src="../ai-workshop/halperbot-nav.webp" alt="HalperBot" class="nav-halperbot">
      <a href="../library/" class="nav-tool">Library</a>
    </nav>
  </header>
  <main class="tm" id="main">
    <div class="tm-wrap">
      <dl class="tm-facts">
        <div><dt>Price</dt><dd>Free</dd></div>
      </dl>
    </div>
  </main>
  <footer class="workshop-footer">
    <a href="../library/">Library</a>
  </footer>
</body>
</html>
"""


def selftest() -> int:
    failures: list[str] = []

    def check(condition: bool, message: str) -> None:
        if not condition:
            failures.append(message)

    releases = parse_changelog(FIXTURE_CHANGELOG)
    check(len(releases) == 2, f"expected 2 releases, got {len(releases)}")
    check(releases[0].version == "0.1.2", f"newest release should come first, got {releases[0].version!r}")
    check(releases[0].date == "2026-09-26", f"wrong date parsed: {releases[0].date!r}")
    check(list(releases[0].sections) == ["Added", "Changed", "Fixed"],
          f"section order not preserved: {list(releases[0].sections)!r}")
    check(releases[1].version == "0.1.1", f"second release should be 0.1.1, got {releases[1].version!r}")

    added = releases[0].sections["Added"][0]
    check(added.startswith("**Bold lead.**") and added.endswith("plus an em dash—here and an arrow→there."),
          f"a bullet wrapped across lines was not folded back into one: {added!r}")

    rendered_bullet = render_inline(added)
    check("—" not in rendered_bullet, f"em dash leaked through: {rendered_bullet!r}")
    check("→" not in rendered_bullet, f"arrow leaked through: {rendered_bullet!r}")
    check("dash - here and an arrow->there." in rendered_bullet,
          f"dash/arrow were not turned into plain punctuation: {rendered_bullet!r}")
    check("<strong>Bold lead.</strong>" in rendered_bullet, f"bold lead not rendered: {rendered_bullet!r}")
    check("<code>inline code</code>" in rendered_bullet, f"inline code not preserved: {rendered_bullet!r}")
    check('<a href="https://example.com/page" rel="noopener">link</a>' in rendered_bullet,
          f"link not rendered: {rendered_bullet!r}")

    escaped_bullet = render_inline(releases[0].sections["Changed"][0])
    check("<code>&lt;Config&gt;</code>" in escaped_bullet, f"code content not escaped: {escaped_bullet!r}")
    check("A &amp; B" in escaped_bullet, f"surrounding text not escaped: {escaped_bullet!r}")

    check(anchor_id("0.1.2") == "v0-1-2", "anchor_id must turn dots into hyphens")

    page = render_page(releases, FIXTURE_PRODUCT_PAGE)
    check('id="v0-1-2"' in page and 'id="v0-1-1"' in page, "release sections need their version anchors")
    check("<h2>0.1.2 - 2026-09-26</h2>" in page, "h2 must read '<version> - <date>' with a plain hyphen")
    check(f"{REPO_URL}/releases/tag/v0.1.2" in page and f"{REPO_URL}/releases/tag/v0.1.1" in page,
          "each release needs its own GitHub release-notes link")
    check('href="../../library/"' in page, "a nav/footer link one level too shallow was not rebased")
    check('src="../../ai-workshop/halperbot-nav.webp"' in page, "the nav image src was not rebased")
    check('href="../../workshop.css"' in page, "workshop.css must resolve one level deeper than the product page")
    check(re.search(r"—|→", page) is None, "no em dash or arrow may survive onto the rendered page")

    ld_blocks = [json.loads(b) for b in re.findall(
        r'<script type="application/ld\+json">(.*?)</script>', page, re.DOTALL)]
    ld = next((b for b in ld_blocks if b.get("@type") == "SoftwareApplication"), None)
    check(ld is not None, "SoftwareApplication ld+json did not survive into the rendered page")
    if ld is not None:
        check(ld.get("releaseNotes") == CHANGELOG_URL, f"releaseNotes must point at the changelog page, got {ld.get('releaseNotes')!r}")
        check(ld.get("url") == CHANGELOG_URL, f"url must point at the changelog page, got {ld.get('url')!r}")
        check(ld.get("name") == "AI Task Manager", "unrelated ld+json fields must survive the copy untouched")

    marked_once = ensure_whats_new_markers(FIXTURE_PRODUCT_PAGE)
    check(marked_once.count(MARK_START) == 1 and marked_once.count(MARK_END) == 1,
          "whats-new markers must be inserted exactly once")
    check(marked_once.index(MARK_START) > marked_once.index('<dl class="tm-facts">'),
          "whats-new markers must land after the facts strip, not before it")
    marked_twice = ensure_whats_new_markers(marked_once)
    check(marked_twice == marked_once, "a second run must not duplicate the whats-new markers")

    try:
        splice(FIXTURE_PRODUCT_PAGE, "<p>x</p>")
        failures.append("splice must raise when the markers are absent")
    except ValueError:
        pass
    spliced = splice(marked_once, "<p>hello</p>")
    check("<p>hello</p>" in spliced and MARK_START in spliced and MARK_END in spliced,
          "splice must insert between the markers and keep both of them in place")
    duplicated = marked_once.replace(MARK_START, MARK_START + MARK_START)
    try:
        splice(duplicated, "<p>x</p>")
        failures.append("splice must raise when a marker is duplicated")
    except ValueError:
        pass

    strip = render_whats_new(releases[0])
    check("0.1.2" in strip, "the strip must name the version it summarises")
    check("<strong>Bold lead.</strong>" in strip, "the strip must render its bullets through the same inline rules")
    check(re.search(r"—|→", strip) is None, "no em dash or arrow may survive into the strip either")

    # End to end through the real CLI: a fresh write agrees with --check,
    # and a one-byte edit to the file on disk makes the drift gate fail.
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        product_page_path = tmp_path / "task-manager" / "index.html"
        product_page_path.parent.mkdir(parents=True)
        product_page_path.write_text(FIXTURE_PRODUCT_PAGE)
        changelog_path = tmp_path / "CHANGELOG.md"
        changelog_path.write_text(FIXTURE_CHANGELOG)

        write_argv = [sys.executable, str(SCRIPT_PATH), str(product_page_path),
                      "--changelog", str(changelog_path), "--site-root", str(tmp_path)]
        result = subprocess.run(write_argv, capture_output=True, text=True)
        check(result.returncode == 0, f"a plain write run must exit 0: {result.stderr}")

        changelog_out = tmp_path / "task-manager" / "changelog" / "index.html"
        check(changelog_out.exists(), "the write run must produce the changelog page")
        check(MARK_START in product_page_path.read_text(), "the write run must add the whats-new markers")

        check_argv = [sys.executable, str(SCRIPT_PATH), "--check", str(product_page_path),
                      "--changelog", str(changelog_path), "--site-root", str(tmp_path)]
        fresh = subprocess.run(check_argv, capture_output=True, text=True)
        check(fresh.returncode == 0, f"--check must pass right after a write: {fresh.stderr}")

        changelog_out.write_text(changelog_out.read_text() + "x")  # one-byte edit
        stale = subprocess.run(check_argv, capture_output=True, text=True)
        check(stale.returncode == 1, "--check must fail once the page on disk no longer matches")
        check(len(stale.stderr.strip().splitlines()) == 1, f"--check must report the failure in one line: {stale.stderr!r}")

        rerun = subprocess.run(write_argv, capture_output=True, text=True)
        check(rerun.returncode == 0, "a second write run must still succeed")
        check(product_page_path.read_text().count(MARK_START) == 1,
              "a second write run must not duplicate the whats-new markers on a real file")

    if failures:
        for f in failures:
            print(f"FAIL: {f}", file=sys.stderr)
        print(f"{len(failures)} selftest assertion(s) failed", file=sys.stderr)
        return 1
    print("selftest passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
