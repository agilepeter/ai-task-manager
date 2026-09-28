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
        Writes <site-root>/task-manager/changelog/index.html, then keeps
        six spots on the product page current for the newest release: the
        "what's new" strip under the facts grid, the "See what changed"
        link inside the Version cell, the Version cell's own number, the
        "Version X.Y.Z is on GitHub Releases for" sentence, the direct
        .dmg/.exe/.msi download links themselves, and the JSON-LD
        SoftwareApplication block's softwareVersion. The first two use a
        marker-comment pair, added once the first time it is missing, with
        the content between a pair rewritten on every run. The other four
        need no marker: each is a bare version number or a versioned URL
        found by its own fixed structural anchor (see
        sync_release_version()), and is refused rather than guessed at if
        that anchor is missing, duplicated, or -- for the download links --
        internally inconsistent or disagreeing with each other.

    python3 scripts/changelog-page.py --check <path-to-task-manager/index.html>
        The drift gate: renders to memory and exits 1 if the changelog
        page or any of the six spots on disk would come out any different
        today.

    python3 scripts/changelog-page.py --selftest
        Exercises a fixture changelog and a fixture product page against a
        temp directory. Stdlib only; there is no pytest here to run.
"""
from __future__ import annotations

import html
import json
import os
import pathlib
import re
import urllib.parse
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from html.parser import HTMLParser

SCRIPT_PATH = pathlib.Path(__file__).resolve()
REPO_URL = "https://github.com/agilepeter/ai-task-manager"
CHANGELOG_URL = "https://staas.fund/task-manager/changelog/"
DEFAULT_CHANGELOG = SCRIPT_PATH.parent.parent / "CHANGELOG.md"
MARK_START = "<!-- whats-new:start -->"
MARK_END = "<!-- whats-new:end -->"
# The indentation ensure_whats_new_markers() writes both before the start
# marker and between the two markers. Naming it once means the writer and
# a first, still-empty --check run can never quietly disagree about what
# "nothing has been spliced in yet" looks like.
_WHATS_NEW_GAP = "\n      "
# The Version cell's own marker pair, narrower than the one above: it
# wraps only the "See what changed" link inside a sentence of otherwise
# hand-written prose, never the sentence itself.
LINK_MARK_START = "<!-- whats-new-link:start -->"
LINK_MARK_END = "<!-- whats-new-link:end -->"


@dataclass
class Release:
    version: str
    date: str
    sections: dict[str, list[str]]  # keys in file order, e.g. "Added", "Changed", "Fixed"


# ---------------------------------------------------------------------------
# Parsing CHANGELOG.md
# ---------------------------------------------------------------------------

_VERSION_RE = re.compile(r"^## (\S+) — (\d{4}-\d{2}-\d{2})\s*$", re.MULTILINE)
_HEADING_RE = re.compile(r"^## .*$", re.MULTILINE)
_SECTION_RE = re.compile(r"^### (.+?)\s*$", re.MULTILINE)


def parse_changelog(text: str) -> list[Release]:
    """One Release per dated version heading, kept in the order the file
    already lists them newest first; this never sorts by version number
    itself. Every line starting with '## ' has to be one of those matched
    headings. Left unchecked, an undated or otherwise malformed one is
    invisible as a boundary, so its whole block -- version, date, every
    bullet under it -- would silently fold into whichever release came
    before it instead of forming one of its own; a repeated section kind
    inside one release would just as quietly replace the list already
    collected for it, since sections are keyed by name. Both are treated
    as a malformed file and raise, rather than losing data underneath the
    caller without saying so."""
    headers = list(_VERSION_RE.finditer(text))
    matched_starts = {m.start() for m in headers}
    for m in _HEADING_RE.finditer(text):
        if m.start() not in matched_starts:
            raise ValueError(f"changelog heading is not a valid release heading: {m.group(0)!r}")
    releases = []
    for i, m in enumerate(headers):
        body_end = headers[i + 1].start() if i + 1 < len(headers) else len(text)
        body = text[m.end():body_end]
        releases.append(Release(version=m.group(1), date=m.group(2), sections=_parse_sections(body, m.group(1))))
    return releases


def _parse_sections(body: str, version: str) -> dict[str, list[str]]:
    headers = list(_SECTION_RE.finditer(body))
    sections: dict[str, list[str]] = {}
    for i, m in enumerate(headers):
        end = headers[i + 1].start() if i + 1 < len(headers) else len(body)
        name = m.group(1).strip()
        if name in sections:
            raise ValueError(f"'### {name}' appears twice in the {version} release")
        sections[name] = _parse_bullets(body[m.end():end])
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
# A single asterisk opens emphasis only when the character right after it is
# not whitespace, and closes only when the character right before it is not
# whitespace either -- the same flanking rule real Markdown uses to keep
# "2 * 3 * 4" from reading as emphasis. Run only after _BOLD_RE has already
# consumed every "**...**" pair, so the two asterisks of a bold span are
# never still lying around for this pattern to mistake for two single ones.
_EM_RE = re.compile(r"\*(?!\s)(.+?)(?<!\s)\*")
_LINK_RE = re.compile(r"\[([^\]]+)\]\(([^)]+)\)")
# Three asterisks on each side are bold and emphasis at once. Matched before
# bold, so the bold pass never splits the run and leaves tags that open in one
# order and close in another.
_BOLD_EM_RE = re.compile(r"\*\*\*(.+?)\*\*\*")
# Code spans and links are found in ONE left-to-right pass, whichever starts
# first winning: a code sample that shows link syntax stays code, and a link
# whose address holds a backtick or an asterisk stays a link.
# The address may hold one level of balanced parentheses, as real URLs do; a
# link whose parentheses do not balance is not matched at all, so an address
# is either taken whole or left as the text it was, never cut short.
_CODE_OR_LINK_RE = re.compile(r"`([^`]+)`|\[([^\]]+)\]\(((?:[^()]|\([^()]*\))+)\)")
_STASH_RE = re.compile("\x00(\\d+)\x00")


def on_the_site(address: str) -> str:
    """A link written for the repository, made to work on the page, or refused.

    CHANGELOG.md links to files beside it (`docs/privacy.md`). On the site
    there is no such file beside the page, so the same link answers 404. A
    link with no scheme that is not an anchor or a site path is a file in the
    repository, and points there.

    The page is public, so only three kinds of address with a scheme go out:
    http, https and mailto. Anything else (`javascript:`, `data:`), and an
    address that begins `//` and so names another host while looking like a
    path, stops the run: a changelog is written by hand, and a link like
    that in one is a mistake worth a loud failure."""
    address = address.strip()
    if not address:
        raise ValueError("a link in the changelog has no address")
    if address.startswith("//"):
        raise ValueError(f"a link in the changelog names another host without a scheme: {address!r}")
    scheme = re.match(r"([a-zA-Z][a-zA-Z0-9+.-]*):", address)
    if scheme:
        if scheme.group(1).lower() not in ("http", "https", "mailto"):
            raise ValueError(f"a link in the changelog uses a scheme the page does not publish: {address!r}")
        return address
    if address.startswith(("#", "/")):
        return address
    path = address[2:] if address.startswith("./") else address
    return f"{REPO_URL}/blob/main/{urllib.parse.quote(path, safe='/#?=&%')}"


def render_inline(md: str) -> str:
    """**bold** leads, *emphasis*, `code`, and [text](url) links become
    their HTML; everything else comes out as escaped text. An em dash and
    an arrow are part of the changelog's own prose style, not this page's,
    so both become plain punctuation here instead of carrying the glyph
    through. Code spans are pulled out before any of the rest run, and put
    back afterwards, so a stray '**', '*' or '[' inside a code sample can
    never be mistaken for Markdown around it. Bold is matched, and fully
    replaced, before emphasis ever looks at the text, so the two asterisks
    of a bold span can never be misread as a pair of single ones; a lone or
    unclosed asterisk that survives both passes was never Markdown to begin
    with and is left exactly as escaping produced it. A link's address is
    set aside whole, like a code span, so nothing inside a URL is ever read
    as Markdown; and whatever the input, the result is checked with a real
    HTML parser, falling back to the asterisks as written if emphasis would
    have produced tags that cross."""
    # A NUL byte has no place in a changelog bullet, and the placeholders below
    # are built from one: dropping it on the way in means nothing a writer
    # typed can ever be mistaken for a placeholder on the way out.
    md = md.replace("\x00", "")
    text = html.escape(md, quote=False).replace("—", " - ").replace("→", "->")
    stashed: list[str] = []

    def keep(fragment: str) -> str:
        stashed.append(fragment)
        return f"\x00{len(stashed) - 1}\x00"

    def restore(rendered: str) -> str:
        return _STASH_RE.sub(lambda m: stashed[int(m.group(1))], rendered)

    def code_or_link(m: re.Match) -> str:
        if m.group(1) is not None:
            return keep(f"<code>{m.group(1)}</code>")
        # A link's address goes out of reach whole: nothing inside a URL is
        # Markdown, whatever it happens to contain. Its label stays in place
        # and is rendered like any other prose, code spans included.
        label = _CODE_RE.sub(lambda c: keep(f"<code>{c.group(1)}</code>"), m.group(2))
        address = on_the_site(m.group(3)).replace('"', "&quot;")
        return f'<a href="{keep(address)}" rel="noopener">{label}</a>'

    text = _CODE_OR_LINK_RE.sub(code_or_link, text)
    without_emphasis = text
    text = _BOLD_EM_RE.sub(lambda m: f"<strong><em>{m.group(1)}</em></strong>", text)
    text = _BOLD_RE.sub(lambda m: f"<strong>{m.group(1)}</strong>", text)
    text = _EM_RE.sub(lambda m: f"<em>{m.group(1)}</em>", text)
    rendered = restore(text)
    if not _well_nested(rendered):
        # Asterisks that cross each other have no sensible reading as
        # emphasis. Print them as written instead of emitting tags that open
        # in one order and close in another.
        rendered = restore(without_emphasis)
    return rendered


class _NestingChecker(HTMLParser):
    """Records whether every tag closes in the reverse of the order it opened."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.open_tags: list[str] = []
        self.ok = True

    def handle_starttag(self, tag: str, attrs: list) -> None:
        self.open_tags.append(tag)

    def handle_endtag(self, tag: str) -> None:
        if not self.open_tags or self.open_tags.pop() != tag:
            self.ok = False


def _well_nested(fragment: str) -> bool:
    checker = _NestingChecker()
    checker.feed(fragment)
    checker.close()
    return checker.ok and not checker.open_tags


def anchor_id(version: str) -> str:
    return "v" + version.replace(".", "-")


_BOLD_LEAD_RE = re.compile(r"^\*\*(.+?)\*\*")


def _lead_text(bullet: str) -> str:
    """The short label a whats-new item shows for one bullet: its own
    **bold lead** markdown (kept as markdown, so render_inline still
    turns it into <strong> the same way it does on the full changelog
    page) with the trailing period trimmed, since every item in that
    strip reads as a short label rather than a full sentence, and with
    the rest of the bullet's own explanation left out entirely -- that
    continuation belongs to the fuller changelog page, not a three-item
    strip on the product page. A bullet with no bold lead of its own
    (this changelog always writes one, but nothing here enforces that)
    falls back to the whole bullet, period trimmed the same way, rather
    than contributing an empty item to a strip that promised three."""
    m = _BOLD_LEAD_RE.match(bullet)
    lead = f"**{m.group(1)}**" if m else bullet
    if lead.endswith(".**"):
        return lead[:-3] + "**"
    if lead.endswith("."):
        return lead[:-1]
    return lead


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
    markers = f"{_WHATS_NEW_GAP}{MARK_START}{_WHATS_NEW_GAP}{MARK_END}"
    return product_page_html[:close] + markers + product_page_html[close:]


def render_whats_new(latest: Release) -> str:
    """The strip's inner HTML for the most recent release: a small-caps
    title naming the version and its date, up to three short items, then
    the two links out to the fuller history. Added is the section almost
    every release actually writes, but a release that for once has none
    still needs a strip, so this falls back to whichever section
    parse_changelog listed first for that release -- the same file order
    the changelog itself already reads in, never re-sorted by name or
    guessed at."""
    kind = "Added" if "Added" in latest.sections else next(iter(latest.sections), None)
    bullets = latest.sections[kind][:3] if kind else []
    items = "".join(f"<li>{render_inline(_lead_text(b))}</li>" for b in bullets)
    tag_url = f"{REPO_URL}/releases/tag/v{latest.version}"
    return (
        '<div class="tm-whatsnew">'
        f'<p class="tm-whatsnew-title">What&rsquo;s new in {html.escape(latest.version)} '
        f'- <span class="tm-whatsnew-date">{html.escape(latest.date)}</span></p>'
        f'<ul class="tm-whatsnew-list">{items}</ul>'
        '<p class="tm-whatsnew-links">'
        f'<a href="changelog/#{anchor_id(latest.version)}" rel="noopener">All changes</a>'
        f' &middot; <a href="{html.escape(tag_url)}" rel="noopener">Release notes on GitHub</a></p>'
        '</div>'
    )


def render_version_link(latest: Release) -> str:
    """The "See what changed" link spliced into the Version cell's own
    <small>, inside the Information section -- a different <dl> from the
    facts strip above, kept current the same way splice() keeps the strip
    current: the anchor always follows whichever version is newest
    today."""
    return f'<a href="changelog/#{anchor_id(latest.version)}" rel="noopener">See what changed</a>'


def ensure_whats_new_link_markers(product_page_html: str) -> str:
    """Adds the narrower marker pair around just the "See what changed"
    link inside the Version cell's <small>, the one time it is missing.
    Everything else in that <small> -- the release month, the
    unsigned-installers note -- is hand-written prose this generator does
    not own; wrapping only the link keeps every future run's rewrite
    scoped to the one fragment that is actually its to change. The single
    leading space is written once, here, outside the markers, so
    "unsigned. See what changed" always reads as two sentences and no
    later run ever has to remember to re-add it."""
    if LINK_MARK_START in product_page_html:
        return product_page_html
    anchor = product_page_html.index("<dt>Version</dt>")
    close = product_page_html.index("</small>", anchor)
    return product_page_html[:close] + " " + LINK_MARK_START + LINK_MARK_END + product_page_html[close:]


def splice(html_text: str, start: str, end: str, inner: str) -> str:
    """Replaces whatever sits between a start/end marker pair with inner
    exactly as given, leaving the rest of the page untouched. Shared by
    both marker pairs on the product page, so there is exactly one
    implementation of "replace between two comments" rather than one per
    pair. A missing or duplicated marker, or an end that comes before its
    own start, means the page is not in the shape this expects, so this
    refuses outright rather than guessing where the managed content
    belongs. Whether inner carries its own leading/trailing newlines is
    the caller's call: the block-level whats-new strip reads better on
    its own source lines, while the Version cell's link sits inline in a
    sentence and must not gain one."""
    if html_text.count(start) != 1 or html_text.count(end) != 1:
        raise ValueError(f"markers {start!r} / {end!r} are missing or appear more than once")
    s = html_text.index(start) + len(start)
    e = html_text.index(end)
    if e < s:
        raise ValueError(f"end marker {end!r} appears before its start marker {start!r}")
    return html_text[:s] + inner + html_text[e:]


# ---------------------------------------------------------------------------
# Four more spots that restate the release version in plain text or in a
# URL, rather than through the marker/splice machinery above: none of them
# holds generated content of varying shape, so each is found by its own
# fixed structural anchor -- a tag pair or a URL shape that appears
# nowhere else on the page -- and simply has its version number swapped in.
# 0.1.3 shipped with two of these updated by hand and the site's JSON-LD
# left stating 0.1.2, exactly the drift this exists to close.
# ---------------------------------------------------------------------------

_VERSION_CELL_RE = re.compile(r"(<dt>Version</dt><dd>)(\d+\.\d+\.\d+)")
_DOWNLOAD_INTRO_RE = re.compile(r"(<p>Version )(\d+\.\d+\.\d+)( is on GitHub Releases for)")
_JSON_LD_VERSION_RE = re.compile(r'("softwareVersion":\s*")(\d+\.\d+\.\d+)(")')
_DOWNLOAD_LINK_RE = re.compile(
    re.escape(REPO_URL) + r"/releases/download/v(\d+\.\d+\.\d+)/AI\.Task\.Manager_(\d+\.\d+\.\d+)_"
    r"([A-Za-z0-9_.-]+\.(?:dmg|exe|msi))"
)


def _rewrite_one(html_text: str, pattern: re.Pattern, group: int, new_value: str, what: str) -> str:
    """Finds `pattern` exactly once and replaces capture group `group`
    with new_value; `what` names the spot for the refusal message. Zero
    matches and two-or-more matches are refused alike -- a page this
    generator cannot read unambiguously is not one it silently guesses
    at -- and a match already holding new_value is returned untouched, so
    an already-current page comes back byte for byte."""
    matches = list(pattern.finditer(html_text))
    if len(matches) != 1:
        raise ValueError(f"{what} is missing or appears {len(matches)} times on the product page (expected exactly 1)")
    start, end = matches[0].span(group)
    if html_text[start:end] == new_value:
        return html_text
    return html_text[:start] + new_value + html_text[end:]


def _rewrite_download_links(html_text: str, new_version: str) -> str:
    """The two (or more) direct .dmg/.exe/.msi links under 'Download it, or
    make it yours.'. Every match's own path version and filename version
    must agree with each other, and every match on the page must agree
    with every other one -- a page with, say, one link already bumped and
    one still on the old version is not safe to finish silently, so that
    is refused as mixed rather than guessed at. An empty page-wide match
    is refused too: a download section that lost its links entirely is
    not something to render as if nothing were wrong."""
    matches = list(_DOWNLOAD_LINK_RE.finditer(html_text))
    if not matches:
        raise ValueError("no direct download links (releases/download/vX.Y.Z/...) found on the product page")
    versions = set()
    for m in matches:
        path_ver, file_ver = m.group(1), m.group(2)
        if path_ver != file_ver:
            raise ValueError(
                f"a download link's URL version v{path_ver} does not match its own filename version {file_ver}: {m.group(0)!r}"
            )
        versions.add(path_ver)
    if len(versions) > 1:
        raise ValueError(
            "download links disagree on version, mixed versions on the page: "
            + ", ".join(f"v{v}" for v in sorted(versions))
        )
    current = next(iter(versions))
    if current == new_version:
        return html_text

    def _bump(m: re.Match) -> str:
        return f"{REPO_URL}/releases/download/v{new_version}/AI.Task.Manager_{new_version}_{m.group(3)}"

    return _DOWNLOAD_LINK_RE.sub(_bump, html_text)


def sync_release_version(html_text: str, target_version: str) -> str:
    """Brings four more spots that each restate the current release
    version to target_version -- the newest dated heading in
    CHANGELOG.md, the same version render_whats_new() and
    render_version_link() above already use -- so the product page can
    never state a version the changelog does not agree with:

      1. the Information section's Version cell (the bare number, not the
         "See what changed" link already owned by the markers above),
      2. the "Version X.Y.Z is on GitHub Releases for" sentence that
         introduces the two installer links,
      3. those direct download links themselves, and
      4. the JSON-LD SoftwareApplication block's softwareVersion.

    Its sibling downloadUrl is a version-less "releases/latest" URL and is
    left alone on purpose -- there is nothing in it to go stale. Each spot
    is refused, rather than guessed at, when it is missing, duplicated, or
    (for the download links) internally inconsistent or disagreeing with
    each other; refusing raises ValueError naming the spot and leaves
    html_text -- and by extension any file the caller has not yet written
    -- untouched. A page whose four spots already state target_version
    comes back byte for byte."""
    html_text = _rewrite_one(html_text, _VERSION_CELL_RE, 2, target_version, "the Information section's Version cell")
    html_text = _rewrite_one(
        html_text, _DOWNLOAD_INTRO_RE, 2, target_version,
        'the "Version X.Y.Z is on GitHub Releases for" sentence',
    )
    html_text = _rewrite_download_links(html_text, target_version)
    html_text = _rewrite_one(
        html_text, _JSON_LD_VERSION_RE, 2, target_version,
        "the JSON-LD SoftwareApplication block's softwareVersion",
    )
    return html_text


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
    # The expected product page is computed with the exact same calls
    # main() writes with below -- never a second, hand-rolled copy of the
    # marker, splice or release-version format to keep in sync by hand. A
    # page that is already current comes back byte-identical to itself, so
    # idempotence IS the check; a missing, duplicated or reversed marker
    # pair, or any sync_release_version() refusal, is reported the same
    # way any other staleness is. The changelog page is then rendered from
    # this same expected page, not the raw one on disk, so its own JSON-LD
    # softwareVersion is checked against the corrected version too --
    # exactly the field that stayed wrong through 0.1.3 because nothing
    # checked it against the changelog's own idea of the newest release.
    try:
        latest = releases[0]
        expected = ensure_whats_new_markers(product_page_html)
        expected = ensure_whats_new_link_markers(expected)
        expected = splice(expected, MARK_START, MARK_END, "\n" + render_whats_new(latest) + "\n")
        expected = splice(expected, LINK_MARK_START, LINK_MARK_END, render_version_link(latest))
        expected = sync_release_version(expected, latest.version)
        rendered = render_page(releases, expected)
    except ValueError as e:
        print(f"stale: {e}", file=sys.stderr)
        return 1

    target = _changelog_target(site_root)
    if not target.exists():
        print(f"stale: {target} does not exist; run changelog-page.py to generate it", file=sys.stderr)
        return 1
    if target.read_text() != rendered:
        print(f"stale: {target} does not match what CHANGELOG.md renders today", file=sys.stderr)
        return 1

    if expected != product_page_html:
        print(
            "stale: the product page's whats-new strip, Version-cell link, or release-version spots "
            "do not match what belongs there today",
            file=sys.stderr,
        )
        return 1
    return 0


def _read_utf8(path: pathlib.Path) -> str:
    """Reads path as strict UTF-8, raising ValueError (the same exception
    every other refusal in this file raises) with a message naming the
    file, rather than letting a UnicodeDecodeError escape as a traceback."""
    try:
        return path.read_bytes().decode("utf-8")
    except UnicodeDecodeError as e:
        raise ValueError(f"{path} is not valid UTF-8: {e}") from e


def _atomic_write(path: pathlib.Path, content: str) -> None:
    """Writes content to a temp file beside path, then renames it over
    path, so a write that is interrupted partway (a full disk, a killed
    process) never leaves the real file half-written. The temp file lives
    in the same directory so the rename is guaranteed to stay on one
    filesystem."""
    tmp = path.with_name(f".{path.name}.tmp{os.getpid()}")
    tmp.write_text(content, encoding="utf-8")
    tmp.replace(path)


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

    try:
        changelog_text = _read_utf8(changelog_path)
        releases = parse_changelog(changelog_text)
        product_page_html = _read_utf8(product_page_path)
    except ValueError as e:
        print(str(e), file=sys.stderr)
        return 1

    if check:
        return _check(releases, product_page_html, site_root)

    # Everything is computed in memory, and can still refuse here, before
    # either file on disk is touched -- a refusal from any step, marker
    # splicing or the release-version sync alike, writes nothing at all.
    latest = releases[0]
    try:
        updated = ensure_whats_new_markers(product_page_html)
        updated = ensure_whats_new_link_markers(updated)
        updated = splice(updated, MARK_START, MARK_END, "\n" + render_whats_new(latest) + "\n")
        updated = splice(updated, LINK_MARK_START, LINK_MARK_END, render_version_link(latest))
        updated = sync_release_version(updated, latest.version)
        # Rendered from the corrected page, and inside the same refusal: a
        # link in the changelog that the page will not publish stops the
        # run here, in one line, with nothing written.
        rendered_changelog = render_page(releases, updated)
    except ValueError as e:
        print(str(e), file=sys.stderr)
        return 1
    # The changelog page is rendered from the corrected page (updated),
    # not the raw one just read, so its own JSON-LD softwareVersion is
    # never a stale copy of whatever the product page said before this run.

    target = _changelog_target(site_root)
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.exists() and target.read_bytes() == rendered_changelog.encode("utf-8"):
        print(f"{target} is already current")
    else:
        _atomic_write(target, rendered_changelog)
        print(f"wrote {target}")

    if updated != product_page_html:
        _atomic_write(product_page_path, updated)
        print(f"wrote {product_page_path} (refreshed the whats-new strip, the Version-cell link, and the release-version spots)")
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
- **Second lead.** Extra detail that must never reach the strip.
- **Third lead.** Even more detail that must never reach the strip either.
- **Fourth lead.** This one must never render; the strip caps at three.

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
    <dl class="tm-info">
      <div><dt>Version</dt><dd>0.1.2<small>Sept 2026. Installers unsigned.</small></dd></div>
    </dl>
    <section class="tm-sec">
      <div class="tm-wrap tm-close">
        <div>
          <p>Version 0.1.2 is on GitHub Releases for <a href="https://github.com/agilepeter/ai-task-manager/releases/download/v0.1.2/AI.Task.Manager_0.1.2_universal.dmg" rel="noopener">macOS</a> and <a href="https://github.com/agilepeter/ai-task-manager/releases/download/v0.1.2/AI.Task.Manager_0.1.2_x64-setup.exe" rel="noopener">Windows</a>.</p>
        </div>
      </div>
    </section>
  </main>
  <footer class="workshop-footer">
    <a href="../library/">Library</a>
  </footer>
</body>
</html>
"""

FIXTURE_BAD_HEADING = """## 0.2.0 — 2026-01-02

### Added

- Real bullet for 0.2.0.

## 1.9.0

### Added

- An undated heading must not silently fold into the release above it.

## 0.1.0 — 2026-01-01

### Added

- Real bullet for 0.1.0.
"""

FIXTURE_REPEATED_SECTION = """## 0.3.0 — 2026-01-03

### Added

- First Added bullet.

### Added

- A second Added heading in the same release must not replace the first list.
"""

FIXTURE_NO_ADDED = """## 0.4.0 — 2026-02-01

### Fixed

- **First fixed thing.** Detail that stays out of the strip.
- **Second fixed thing.** More detail that stays out too.
- Plain bullet with no bold lead at all.
- **Fourth fixed thing.** Never reached; the strip still caps at three.

### Removed

- Something removed, never reached because Fixed comes first.
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
    for written, served in [
        ("docs/privacy.md", f"{REPO_URL}/blob/main/docs/privacy.md"),
        ("./SHIPPING.md#cutting-a-release", f"{REPO_URL}/blob/main/SHIPPING.md#cutting-a-release"),
        ("https://example.com/a", "https://example.com/a"),
        ("mailto:someone@example.com", "mailto:someone@example.com"),
        ("#v0-1-0", "#v0-1-0"),
        ("/task-manager/", "/task-manager/"),
        ("HTTPS://EXAMPLE.COM/A", "HTTPS://EXAMPLE.COM/A"),
        ("docs/a b.md", f"{REPO_URL}/blob/main/docs/a%20b.md"),
    ]:
        got = render_inline(f"See [the file]({written}).")
        check(f'href="{served}"' in got, f"a link to {written!r} should be served as {served!r}: {got!r}")
    for refused in ["javascript:alert(1)", "JavaScript:alert(1)", "data:text/html,x", "//evil.example/x", "vbscript:x", " "]:
        try:
            got = render_inline(f"See [the file]({refused}).")
        except ValueError:
            continue
        # Markdown with a space for an address is not a link at all, and may come back as prose.
        check("href=" not in got, f"a link to {refused!r} must be refused, not published: {got!r}")

    escaped_bullet = render_inline(releases[0].sections["Changed"][0])
    check("<code>&lt;Config&gt;</code>" in escaped_bullet, f"code content not escaped: {escaped_bullet!r}")
    check("A &amp; B" in escaped_bullet, f"surrounding text not escaped: {escaped_bullet!r}")

    # Single-asterisk emphasis, added alongside the existing **bold**: bold
    # still wins where the two could be confused, code still shields its
    # own asterisks, and an asterisk that was never really Markdown -- lone,
    # unclosed, or a multiplication sign with spaces on both sides -- comes
    # through exactly as escaping left it.
    check(render_inline("*a*") == "<em>a</em>", f"a single-asterisk span must become <em>: {render_inline('*a*')!r}")
    check(render_inline("**b**") == "<strong>b</strong>", f"a double-asterisk span must still become <strong>: {render_inline('**b**')!r}")
    combined = render_inline("**b** and *a*")
    check(combined == "<strong>b</strong> and <em>a</em>",
          f"bold and emphasis together in one bullet must both render, bold never mistaken for emphasis: {combined!r}")
    code_star = render_inline("a `2 * 3` span")
    check("<code>2 * 3</code>" in code_star and "<em>" not in code_star,
          f"an asterisk inside a code span must stay literal, not become emphasis: {code_star!r}")
    check(render_inline("a * lone one") == "a * lone one",
          f"a lone asterisk must stay literal: {render_inline('a * lone one')!r}")
    check(render_inline("*unclosed") == "*unclosed",
          f"an unclosed asterisk must stay literal: {render_inline('*unclosed')!r}")
    check(render_inline("2 * 3 * 4") == "2 * 3 * 4",
          f"asterisks with spaces on both sides (multiplication, not emphasis) must stay literal: {render_inline('2 * 3 * 4')!r}")

    triple = render_inline("***bold and em***")
    check(triple == "<strong><em>bold and em</em></strong>",
          f"three asterisks each side must nest bold around emphasis: {triple!r}")
    star_url = render_inline("[text](https://example.com/*star*/page)")
    check(star_url == '<a href="https://example.com/*star*/page" rel="noopener">text</a>',
          f"an asterisk inside a link address must stay part of the address: {star_url!r}")
    bold_url = render_inline("[t](https://example.com/**x**/y)")
    check('href="https://example.com/**x**/y"' in bold_url and "<strong>" not in bold_url,
          f"double asterisks inside a link address must stay part of the address: {bold_url!r}")
    tick_url = render_inline("[t](https://example.com/`x`/y)")
    check('href="https://example.com/`x`/y"' in tick_url and "<code>" not in tick_url,
          f"a backtick inside a link address must stay part of the address: {tick_url!r}")
    quote_url = render_inline('[t](https://example.com/?q="x")')
    check('href="https://example.com/?q=&quot;x&quot;"' in quote_url,
          f"a double quote inside a link address must be escaped for the attribute: {quote_url!r}")
    code_label = render_inline("[`cfg` file](https://example.com/cfg)")
    check(code_label == '<a href="https://example.com/cfg" rel="noopener"><code>cfg</code> file</a>',
          f"code inside a link label must render as code inside the link: {code_label!r}")
    em_label = render_inline("[*a*](https://example.com)")
    check(em_label == '<a href="https://example.com" rel="noopener"><em>a</em></a>',
          f"emphasis inside a link label must render inside the link: {em_label!r}")
    code_with_link_syntax = render_inline("write `[text](url)` to link")
    check(code_with_link_syntax == "write <code>[text](url)</code> to link",
          f"link syntax shown inside a code span must stay code: {code_with_link_syntax!r}")
    paren_url = render_inline("[text](https://example.com/a(b)c)")
    check(paren_url == '<a href="https://example.com/a(b)c" rel="noopener">text</a>',
          f"balanced parentheses inside a link address must stay in the address: {paren_url!r}")
    open_paren_url = render_inline("[text](https://example.com/a(b)")
    check("<a " not in open_paren_url and open_paren_url == "[text](https://example.com/a(b)",
          f"an address whose parentheses do not balance must stay literal text, never a cut-short link: {open_paren_url!r}")
    nul_collision = render_inline("a\x000\x00b `code`")
    check(nul_collision == "a0b <code>code</code>",
          f"NUL bytes in the input must be dropped, never read as a placeholder: {nul_collision!r}")
    crossed = render_inline("*a **b* c**")
    check(crossed == "*a **b* c**",
          f"asterisks that cross must print as written, never as tags that cross: {crossed!r}")
    for sample in (
        "*a*", "**b**", "**b** and *a*", "a `2 * 3` span", "a * lone one", "*unclosed", "2 * 3 * 4",
        "***bold and em***", "[text](https://example.com/*star*/page)", "[t](https://example.com/**x**/y)",
        "[t](https://example.com/`x`/y)", "[`cfg` file](https://example.com/cfg)", "[*a*](https://example.com)",
        "write `[text](url)` to link", "*a **b* c**", "**a *b** c*", "one *two* three*", "(*a*), *b*.",
        "[text](https://example.com/a(b)c)", "[text](https://example.com/a(b)", "[a](https://x.y/1) and [b](https://x.y/2)",
        "[a](https://x.y)*em*", "[a [b] c](https://x.y)",
        "**A *lively* `feature`.** The rest.",
    ):
        check(_well_nested(render_inline(sample)),
              f"every rendering must close its tags in the order it opened them: {sample!r} -> {render_inline(sample)!r}")

    # _lead_text() and render_whats_new() work from the bold lead's raw
    # markdown, not render_inline()'s output, so emphasis or code sitting
    # inside that lead must still come through once render_whats_new()
    # eventually does call render_inline() on it.
    lead_with_emphasis = _lead_text("**A *lively* `feature`.** The rest of the sentence never reaches the strip.")
    check(render_inline(lead_with_emphasis) == "<strong>A <em>lively</em> <code>feature</code></strong>",
          f"a bold lead containing both emphasis and code must still render correctly: {render_inline(lead_with_emphasis)!r}")

    check(anchor_id("0.1.2") == "v0-1-2", "anchor_id must turn dots into hyphens")

    try:
        parse_changelog(FIXTURE_BAD_HEADING)
        failures.append("parse_changelog must raise on an undated '## ' heading instead of folding it into the release above")
    except ValueError as e:
        check("1.9.0" in str(e), f"the bad-heading error must name the offending line: {e}")

    try:
        parse_changelog(FIXTURE_REPEATED_SECTION)
        failures.append("parse_changelog must raise when a section kind repeats inside one release instead of overwriting it")
    except ValueError as e:
        check("Added" in str(e) and "0.3.0" in str(e), f"the repeated-section error must name the kind and the release: {e}")

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
    marked_once = ensure_whats_new_link_markers(marked_once)
    check(marked_once.count(MARK_START) == 1 and marked_once.count(MARK_END) == 1,
          "whats-new markers must be inserted exactly once")
    check(marked_once.count(LINK_MARK_START) == 1 and marked_once.count(LINK_MARK_END) == 1,
          "whats-new-link markers must be inserted exactly once")
    check(marked_once.index(MARK_START) > marked_once.index('<dl class="tm-facts">'),
          "whats-new markers must land after the facts strip, not before it")
    check(marked_once.index(LINK_MARK_START) > marked_once.index("<dt>Version</dt>"),
          "whats-new-link markers must land inside the Version cell, not before it")
    check(" " + LINK_MARK_START in marked_once,
          "the whats-new-link markers need a leading space so the link never runs into the sentence before it")
    marked_twice = ensure_whats_new_link_markers(ensure_whats_new_markers(marked_once))
    check(marked_twice == marked_once, "a second run must not duplicate either marker pair")

    try:
        splice(FIXTURE_PRODUCT_PAGE, MARK_START, MARK_END, "<p>x</p>")
        failures.append("splice must raise when the markers are absent")
    except ValueError:
        pass
    spliced = splice(marked_once, MARK_START, MARK_END, "<p>hello</p>")
    check("<p>hello</p>" in spliced and MARK_START in spliced and MARK_END in spliced,
          "splice must insert between the markers and keep both of them in place")
    duplicated = marked_once.replace(MARK_START, MARK_START + MARK_START)
    try:
        splice(duplicated, MARK_START, MARK_END, "<p>x</p>")
        failures.append("splice must raise when a marker is duplicated")
    except ValueError:
        pass

    # render_whats_new: the common case is three Added leads, but a
    # release can write fewer, or none at all.
    strip = render_whats_new(releases[0])
    check("What&rsquo;s new in 0.1.2" in strip, "the strip must name the version it summarises")
    check("2026-09-26" in strip, "the strip must give the release date in plain text")
    check("<strong>Bold lead</strong>" in strip, "a bold lead's trailing period must be dropped from the strip")
    check("<strong>Second lead</strong>" in strip and "<strong>Third lead</strong>" in strip,
          "the strip must carry the second and third Added leads too")
    check("Fourth lead" not in strip, "the strip must cap at three items even when a release lists more")
    check(strip.count("<li>") == 3, "three available Added leads must produce exactly three items")
    check(re.search(r"—|→", strip) is None, "no em dash or arrow may survive into the strip either")
    check(f'href="changelog/#{anchor_id("0.1.2")}"' in strip and ">All changes<" in strip,
          "the strip must link to this release's own anchor on the changelog page")
    check(f'href="{REPO_URL}/releases/tag/v0.1.2"' in strip and "Release notes on GitHub" in strip,
          "the strip must link to this release's GitHub release notes")

    thin_strip = render_whats_new(releases[1])  # 0.1.1: exactly one Added bullet, no bold lead
    check(thin_strip.count("<li>") == 1, "fewer than three Added bullets must render only what exists, never padded")
    check("<li>Earlier release, one bullet</li>" in thin_strip,
          "a bullet with no bold lead of its own must still show as an item, period trimmed")

    no_added = parse_changelog(FIXTURE_NO_ADDED)[0]
    check("Added" not in no_added.sections, "fixture setup: this release must have no Added section")
    fallback_strip = render_whats_new(no_added)
    check(fallback_strip.count("<li>") == 3, "the no-Added fallback must still cap at three items")
    check("<strong>First fixed thing</strong>" in fallback_strip and "<strong>Second fixed thing</strong>" in fallback_strip,
          "with no Added section, the strip must fall back to the first section the release actually lists")
    check("Fourth fixed thing" not in fallback_strip and "Something removed" not in fallback_strip,
          "the fallback must use only the first three bullets of the FIRST section present, never a later one")

    link = render_version_link(releases[0])
    check(link == '<a href="changelog/#v0-1-2" rel="noopener">See what changed</a>',
          f"the Version cell's link text and target must match the newest release exactly: {link!r}")

    def _freshly_spliced(product_page_html: str, latest: Release) -> str:
        """The page exactly as a real run would leave it: both marker
        pairs present and both holding today's content, and the four
        release-version spots synced too. Building it this one way -- by
        calling the very functions --check itself calls -- means a
        fixture claiming to be "fresh" can never quietly drift from what
        --check considers fresh."""
        page = ensure_whats_new_link_markers(ensure_whats_new_markers(product_page_html))
        page = splice(page, MARK_START, MARK_END, "\n" + render_whats_new(latest) + "\n")
        page = splice(page, LINK_MARK_START, LINK_MARK_END, render_version_link(latest))
        return sync_release_version(page, latest.version)

    fresh_page = _freshly_spliced(FIXTURE_PRODUCT_PAGE, releases[0])

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
        written_page = product_page_path.read_text()
        check(MARK_START in written_page and LINK_MARK_START in written_page,
              "the write run must add both whats-new marker pairs")
        check("What&rsquo;s new in 0.1.2" in written_page and "<strong>Bold lead</strong>" in written_page,
              "the write run must splice real content into the whats-new strip, not leave it empty")
        check('href="changelog/#v0-1-2" rel="noopener">See what changed<' in written_page,
              "the write run must splice the Version cell's link to the newest release")
        check(written_page == fresh_page,
              "the CLI's write output must match splicing the same content in directly, in-process")

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
        rerun_page = product_page_path.read_text()
        check(rerun_page.count(MARK_START) == 1 and rerun_page.count(LINK_MARK_START) == 1,
              "a second write run must not duplicate either whats-new marker pair on a real file")
        check(rerun_page == written_page, "a second write run must reproduce byte-identical output")

    # A malformed changelog must stop the CLI before it writes anything,
    # rather than silently folding a heading into the wrong release or
    # losing a repeated section's bullets.
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        product_page_path = tmp_path / "task-manager" / "index.html"
        product_page_path.parent.mkdir(parents=True)
        product_page_path.write_text(FIXTURE_PRODUCT_PAGE)
        changelog_path = tmp_path / "CHANGELOG.md"
        changelog_path.write_text(FIXTURE_BAD_HEADING)

        argv = [sys.executable, str(SCRIPT_PATH), str(product_page_path),
                "--changelog", str(changelog_path), "--site-root", str(tmp_path)]
        result = subprocess.run(argv, capture_output=True, text=True)
        check(result.returncode == 1, f"a malformed changelog must exit 1, got {result.returncode}")
        check(len(result.stderr.strip().splitlines()) == 1, f"the failure must be one stderr line: {result.stderr!r}")
        check(not (tmp_path / "task-manager" / "changelog" / "index.html").exists(),
              "a malformed changelog must write nothing")
        check(product_page_path.read_text() == FIXTURE_PRODUCT_PAGE,
              "a malformed changelog must leave the product page untouched")

    # --check must cover the product page's whats-new strip and Version
    # link too, not only the separate changelog page file.
    def _check_against(product_page_html: str) -> subprocess.CompletedProcess:
        """Runs --check in a fresh temp dir where the changelog page is
        already correct for this exact product_page_html, so a failure can
        only come from the whats-new strip or Version-cell link this test
        is exercising."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            product_page_path = tmp_path / "task-manager" / "index.html"
            product_page_path.parent.mkdir(parents=True)
            product_page_path.write_text(product_page_html)
            changelog_path = tmp_path / "CHANGELOG.md"
            changelog_path.write_text(FIXTURE_CHANGELOG)
            target = tmp_path / "task-manager" / "changelog" / "index.html"
            target.parent.mkdir(parents=True)
            target.write_text(render_page(releases, product_page_html))
            argv = [sys.executable, str(SCRIPT_PATH), "--check", str(product_page_path),
                    "--changelog", str(changelog_path), "--site-root", str(tmp_path)]
            return subprocess.run(argv, capture_output=True, text=True)

    result = _check_against(FIXTURE_PRODUCT_PAGE)
    check(result.returncode == 1, f"--check must fail when both whats-new marker pairs are missing: {result.stderr!r}")
    check(len(result.stderr.strip().splitlines()) == 1, f"--check must report a missing marker in one line: {result.stderr!r}")

    result = _check_against(marked_once)
    check(result.returncode == 1,
          f"--check must fail when both marker pairs exist but nothing has been spliced in yet: {result.stderr!r}")

    # The remaining corruptions are each applied to an otherwise-fresh page,
    # so a failure can only be attributed to the one thing this test broke.
    duplicated_marker = fresh_page.replace(MARK_START, MARK_START + MARK_START, 1)
    result = _check_against(duplicated_marker)
    check(result.returncode == 1, f"--check must fail when a whats-new marker is duplicated: {result.stderr!r}")

    duplicated_link_marker = fresh_page.replace(LINK_MARK_START, LINK_MARK_START + LINK_MARK_START, 1)
    result = _check_against(duplicated_link_marker)
    check(result.returncode == 1, f"--check must fail when a whats-new-link marker is duplicated: {result.stderr!r}")

    start_i = fresh_page.index(MARK_START)
    end_i = fresh_page.index(MARK_END)
    strip_inner_now = fresh_page[start_i + len(MARK_START):end_i]
    reversed_order = (fresh_page[:start_i] + MARK_END + strip_inner_now + MARK_START
                       + fresh_page[end_i + len(MARK_END):])
    result = _check_against(reversed_order)
    check(result.returncode == 1, f"--check must fail when the end marker comes before the start marker: {result.stderr!r}")
    check("before its start marker" in result.stderr,
          f"reversed markers must be refused for being reversed, not for some other reason: {result.stderr!r}")

    stale_content = fresh_page.replace(MARK_END, "<p>leftover</p>" + MARK_END, 1)
    result = _check_against(stale_content)
    check(result.returncode == 1, f"--check must fail when something stale has been left between the whats-new markers: {result.stderr!r}")

    stale_link = fresh_page.replace(LINK_MARK_END, '<a href="old">old</a>' + LINK_MARK_END, 1)
    result = _check_against(stale_link)
    check(result.returncode == 1, f"--check must fail when the Version cell's link is stale: {result.stderr!r}")

    result = _check_against(fresh_page)
    check(result.returncode == 0, f"--check must pass once both marker pairs hold today's real content: {result.stderr!r}")

    # sync_release_version(): the four more spots that need no marker
    # comment. fresh_page already has all four stating "0.1.2", the same
    # version FIXTURE_CHANGELOG's newest heading names, so syncing it
    # again must be a true no-op.
    already_current = sync_release_version(fresh_page, "0.1.2")
    check(already_current == fresh_page,
          "a page whose four release-version spots already state the target version must come back byte for byte")

    def _rolled_back(page: str) -> str:
        """page with only the four spots sync_release_version owns set
        back to 0.1.1; everything else (the whats-new strip and the
        Version-cell link, both already correctly naming 0.1.2 as the
        newest release) is left exactly as fresh_page has it."""
        for old, new in (
            ("<dt>Version</dt><dd>0.1.2", "<dt>Version</dt><dd>0.1.1"),
            ("<p>Version 0.1.2 is on GitHub Releases for", "<p>Version 0.1.1 is on GitHub Releases for"),
            ("v0.1.2/AI.Task.Manager_0.1.2_universal.dmg", "v0.1.1/AI.Task.Manager_0.1.1_universal.dmg"),
            ("v0.1.2/AI.Task.Manager_0.1.2_x64-setup.exe", "v0.1.1/AI.Task.Manager_0.1.1_x64-setup.exe"),
            ('"softwareVersion": "0.1.2"', '"softwareVersion": "0.1.1"'),
        ):
            page = page.replace(old, new, 1)
        return page

    bumped = sync_release_version(_rolled_back(fresh_page), "0.1.2")
    check('<dt>Version</dt><dd>0.1.2<small>' in bumped, "sync_release_version must bump the Version cell's own number")
    check('<p>Version 0.1.2 is on GitHub Releases for' in bumped,
          "sync_release_version must bump the download-intro sentence")
    check("v0.1.2/AI.Task.Manager_0.1.2_universal.dmg" in bumped and "v0.1.2/AI.Task.Manager_0.1.2_x64-setup.exe" in bumped,
          "sync_release_version must bump both download links, path and filename alike")
    check('"softwareVersion": "0.1.2"' in bumped, "sync_release_version must bump the JSON-LD softwareVersion")
    check("0.1.1" not in bumped, "no trace of the old version may remain once every spot is bumped")

    def _mutated(what: str) -> str:
        return {
            "no version cell": fresh_page.replace("<dt>Version</dt><dd>0.1.2", "<dt>Ver</dt><dd>0.1.2", 1),
            "version cell twice": fresh_page.replace("</body>", "<dt>Version</dt><dd>0.1.2</dd></body>", 1),
            "no download links": _DOWNLOAD_LINK_RE.sub("removed", fresh_page),
            "a link's own path/filename disagree": fresh_page.replace(
                "v0.1.2/AI.Task.Manager_0.1.2_universal.dmg", "v0.1.9/AI.Task.Manager_0.1.2_universal.dmg", 1),
            "links disagree with each other": fresh_page.replace(
                "v0.1.2/AI.Task.Manager_0.1.2_universal.dmg", "v0.1.1/AI.Task.Manager_0.1.1_universal.dmg", 1),
            "no download-intro sentence": fresh_page.replace("is on GitHub Releases for", "is available for", 1),
            "download-intro sentence twice": fresh_page.replace(
                "<p>Version 0.1.2 is on GitHub Releases for",
                "<p>Version 0.1.2 is on GitHub Releases for</p><p>Version 0.1.2 is on GitHub Releases for", 1),
            "no JSON-LD softwareVersion": fresh_page.replace('"softwareVersion": "0.1.2",', "", 1),
            "JSON-LD softwareVersion twice": fresh_page.replace(
                '"softwareVersion": "0.1.2",', '"softwareVersion": "0.1.2", "softwareVersion": "0.1.2",', 1),
        }[what]

    for what, needle in (
        ("no version cell", "Version cell"),
        ("version cell twice", "Version cell"),
        ("no download links", "no direct download links"),
        ("a link's own path/filename disagree", "does not match its own filename version"),
        ("links disagree with each other", "mixed versions"),
        ("no download-intro sentence", "GitHub Releases for"),
        ("download-intro sentence twice", "GitHub Releases for"),
        ("no JSON-LD softwareVersion", "softwareVersion"),
        ("JSON-LD softwareVersion twice", "softwareVersion"),
    ):
        try:
            sync_release_version(_mutated(what), "0.1.2")
            failures.append(f"sync_release_version must refuse when {what}")
        except ValueError as e:
            check(needle in str(e), f"the refusal for {what!r} must name what it could not find, got: {e}")

    # End to end through the real CLI, from an older stated version: the
    # scenario this generator exists for. The whats-new strip and Version
    # link are already correct in fresh_page (both point at 0.1.2, the
    # newest release); only the four new spots are rolled back to 0.1.1,
    # as if a hand edit for a previous release had never caught up.
    older_page = _rolled_back(fresh_page)

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        product_page_path = tmp_path / "task-manager" / "index.html"
        product_page_path.parent.mkdir(parents=True)
        product_page_path.write_text(older_page)
        changelog_path = tmp_path / "CHANGELOG.md"
        changelog_path.write_text(FIXTURE_CHANGELOG)

        write_argv = [sys.executable, str(SCRIPT_PATH), str(product_page_path),
                      "--changelog", str(changelog_path), "--site-root", str(tmp_path)]
        result = subprocess.run(write_argv, capture_output=True, text=True)
        check(result.returncode == 0, f"bumping an older page through the real CLI must exit 0: {result.stderr}")

        written = product_page_path.read_text()
        check('<dt>Version</dt><dd>0.1.2<small>' in written, "the CLI must bump the Version cell for real")
        check('<p>Version 0.1.2 is on GitHub Releases for' in written, "the CLI must bump the download-intro sentence for real")
        check("v0.1.2/AI.Task.Manager_0.1.2_universal.dmg" in written and "v0.1.2/AI.Task.Manager_0.1.2_x64-setup.exe" in written,
              "the CLI must bump both download links for real")
        check('"softwareVersion": "0.1.2"' in written, "the CLI must bump the JSON-LD softwareVersion for real")
        check("0.1.1" not in written, "no trace of 0.1.1 may remain in the written product page")

        changelog_out = tmp_path / "task-manager" / "changelog" / "index.html"
        check('"softwareVersion": "0.1.2"' in changelog_out.read_text(),
              "the changelog page's own JSON-LD must be rendered from the CORRECTED product page, "
              "not carry forward the stale 0.1.1 that was on disk before this run")

        # A second run against the now-current file changes nothing.
        rerun = subprocess.run(write_argv, capture_output=True, text=True)
        check(rerun.returncode == 0, "a second run against an already-current page must still exit 0")
        check(product_page_path.read_text() == written, "a second run against an already-current page must change nothing")

        # No stray temp file from the atomic write survives a normal run.
        leftovers = [p.name for p in product_page_path.parent.iterdir() if ".tmp" in p.name]
        check(leftovers == [], f"the atomic write must not leave a temp file behind: {leftovers}")

    # A product page that is not valid UTF-8 is refused before anything is
    # written, the same as any other refusal.
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        product_page_path = tmp_path / "task-manager" / "index.html"
        product_page_path.parent.mkdir(parents=True)
        product_page_path.write_bytes(b"<!DOCTYPE html>\n<html>\xff\xfe broken</html>\n")
        changelog_path = tmp_path / "CHANGELOG.md"
        changelog_path.write_text(FIXTURE_CHANGELOG)

        argv = [sys.executable, str(SCRIPT_PATH), str(product_page_path),
                "--changelog", str(changelog_path), "--site-root", str(tmp_path)]
        result = subprocess.run(argv, capture_output=True, text=True)
        check(result.returncode == 1, f"a non-UTF-8 product page must exit 1, got {result.returncode}")
        check("UTF-8" in result.stderr, f"the refusal must say the page is not valid UTF-8: {result.stderr!r}")
        check(len(result.stderr.strip().splitlines()) == 1, f"the failure must be one stderr line: {result.stderr!r}")
        check(not (tmp_path / "task-manager" / "changelog" / "index.html").exists(),
              "a non-UTF-8 product page must write nothing")

    if failures:
        for f in failures:
            print(f"FAIL: {f}", file=sys.stderr)
        print(f"{len(failures)} selftest assertion(s) failed", file=sys.stderr)
        return 1
    print("selftest passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
