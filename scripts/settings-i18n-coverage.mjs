// Walks index.html's <aside id="settings"> block and answers one question per
// text-bearing element: does it carry a data-i18n* attribute, or is it on the
// named exemption list below? The older check in scripts/i18n.test.mjs only
// looked at elements that already HAD a data-i18n* attribute (it confirmed
// the key exists in en.json) -- a row with no such attribute at all was
// simply never visited, which is exactly how the apiFeeds toggle and its
// hint paragraph shipped in English with no way to translate them. This file
// enumerates the DOM instead of the keys, so a new row with no data-i18n* on
// it fails loudly instead of shipping silently.
//
// Two passes over the same walk: element TEXT (findSettingsTextNodes /
// checkSettingsPanelI18nCoverage) and the `title` / `aria-label` ATTRIBUTES
// an element's opening tag can carry (findSettingsAttributeNodes /
// checkSettingsPanelAttrI18nCoverage). A tooltip is just as user-visible as
// the label beside it -- the renewal-reminder / session-nudge / weekly-digest
// labels shipped their `title` text hardcoded for the same reason apiFeeds
// did: nothing walked the DOM asking whether it had a translation attribute
// at all, only whether an attribute already there pointed at a real key.
//
// Not a general HTML parser: it is exactly enough to walk this one file's
// hand-authored, well-formed markup (a stack keyed on tag name, attributes
// read with a simple quoted-value regex, void/self-closed elements never
// pushed). The text pass only inspects LEAF text -- a run of text whose
// immediate parent has no child elements -- inside the tags a Settings row
// actually uses for language content: label, span, p, option, button, h4.
// Anything else in the panel (div, select, input, form, …) never carries
// direct text here and is not checked. The attribute pass looks at every
// opening tag, since `title` and `aria-label` can land on any element.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const VOID_ELEMENTS = new Set([
  "input", "br", "hr", "img", "meta", "link", "source", "track", "wbr", "col", "area", "base", "embed", "param",
]);
const CHECKED_TAGS = new Set(["label", "span", "p", "option", "button", "h4"]);
const I18N_ATTRS = ["data-i18n", "data-i18n-html"];

// Elements whose static text intentionally carries no data-i18n* key, each
// for its own stated reason. Both categories here are the same kind of
// exemption: the text is not language content, so there is nothing in it
// FOR a translator to change.
//
// - The eleven key-row labels are brand names (OpenRouter, Z.ai, …); a
//   product name is never translated, same rule as a URL.
// - spend-alert's six options are bare currency amounts ("$10" … "$500") --
//   no natural-language word sits in any of them, so there is nothing in
//   them TO translate, again like a URL or a version number.
//
// This list used to also carry eight entries for hardcoded English that
// this check exposed but an earlier change had not yet keyed (the
// renewal-reminder / session-nudge / weekly-digest rows and the
// api-keys-reveal button + note). Those are now keyed in all nine locale
// files like everything else in the panel, so they were removed from here
// rather than left as permanent exemptions -- an exemption that stops being
// true is a bug in this list, not a style choice.
export const SETTINGS_I18N_EXEMPT = {
  "key-openrouter": "brand name (OpenRouter), never translated",
  "key-zai": "brand name (Z.ai / GLM), never translated",
  "key-minimax": "brand name (MiniMax), never translated",
  "key-deepseek": "brand name (DeepSeek), never translated",
  "key-kimi": "brand name (Kimi Code), never translated",
  "key-moonshot": "brand name (Kimi API / Moonshot), never translated",
  "key-elevenlabs": "brand name (ElevenLabs), never translated",
  "key-codebuff": "brand name (Codebuff), never translated",
  "key-kilo": "brand name (Kilo), never translated",
  "key-aihubmix": "brand name (AihubMix), never translated",
  "key-qwen": "brand name (Qwen Code), never translated",
  "spend-alert>option": "bare currency amounts ($10 … $500), no words to translate",
};

// Which data-i18n-* attribute covers which plain attribute, mirroring
// applyStaticI18n() in src/i18n.ts (data-i18n-title -> el.title, data-i18n-aria
// -> el.setAttribute("aria-label", …)). Checked in this order for each tag.
const ATTR_I18N = [
  ["title", "data-i18n-title"],
  ["aria-label", "data-i18n-aria"],
];

// Same idea as SETTINGS_I18N_EXEMPT, for `title` / `aria-label` text instead
// of element text. Empty today: every title/aria-label inside the panel
// that carries real words is keyed. Kept as a named export (not inlined)
// so a future one has one obvious place to go, with its own one-line
// reason, rather than a bare `true` sprinkled into the walker.
export const SETTINGS_ATTR_I18N_EXEMPT = {};

function parseAttrs(tagInner) {
  const attrs = {};
  const re = /([a-zA-Z_:][a-zA-Z0-9_:.-]*)(?:\s*=\s*"([^"]*)")?/g;
  let m;
  while ((m = re.exec(tagInner))) {
    if (m[1] === "/") continue;
    attrs[m[1]] = m[2] ?? "";
  }
  return attrs;
}

// One token is a comment, a closing tag, an opening tag (attrs captured
// whole, self-close flag separate), or a run of plain text.
const TOKEN_RE =
  /<!--[\s\S]*?-->|<\/([a-zA-Z][a-zA-Z0-9-]*)\s*>|<([a-zA-Z][a-zA-Z0-9-]*)((?:\s+[a-zA-Z_:][a-zA-Z0-9_:.-]*(?:\s*=\s*"[^"]*")?)*)\s*(\/?)>|[^<]+/g;

/// Every leaf text-bearing element inside the given Settings `<aside>` inner
/// HTML, whether or not it is covered -- the caller decides what to do with
/// exemptions. Each entry: { tag, locator, hasI18n, text }.
export function findSettingsTextNodes(settingsInnerHtml) {
  const stack = []; // { tag, attrs }
  const found = [];
  let m;
  TOKEN_RE.lastIndex = 0;
  while ((m = TOKEN_RE.exec(settingsInnerHtml))) {
    const whole = m[0];
    if (whole.startsWith("<!--")) continue;
    if (whole.startsWith("</")) {
      const name = m[1];
      for (let i = stack.length - 1; i >= 0; i--) {
        if (stack[i].tag === name) {
          stack.length = i;
          break;
        }
      }
      continue;
    }
    if (whole.startsWith("<")) {
      const tag = m[2];
      const attrs = parseAttrs(m[3] || "");
      const selfClose = m[4] === "/";
      if (!selfClose && !VOID_ELEMENTS.has(tag)) stack.push({ tag, attrs });
      continue;
    }
    const text = whole.replace(/\s+/g, " ").trim();
    if (!text) continue;
    const parent = stack[stack.length - 1];
    if (!parent || !CHECKED_TAGS.has(parent.tag)) continue;
    // The accordion chevron ("&#8964;") is a glyph, not language content.
    if ((parent.attrs.class || "").split(/\s+/).includes("chev")) continue;

    const hasI18n = I18N_ATTRS.some((a) => a in parent.attrs);

    // Locator for the exemption table: the element's own id; a <label
    // for="x"> uses x; otherwise the nearest open ancestor's id, joined with
    // this tag name (covers an <option> under a <select id=x>, or a bare
    // <span> beside a sibling with the useful id, like api-keys-reveal-row's
    // note).
    let locator = parent.attrs.id;
    if (!locator && parent.tag === "label" && parent.attrs.for) locator = parent.attrs.for;
    if (!locator) {
      for (let i = stack.length - 2; i >= 0; i--) {
        if (stack[i].attrs.id) {
          locator = `${stack[i].attrs.id}>${parent.tag}`;
          break;
        }
      }
    }
    found.push({ tag: parent.tag, locator: locator || null, hasI18n, text });
  }
  return found;
}

/// Violations = checked leaf text with no data-i18n* and no matching
/// exemption entry. exempt defaults to the module's own table so callers
/// normally just pass html; a caller proving the pre-fix file would have
/// failed can still see the raw list by passing {} instead.
export function checkSettingsPanelI18nCoverage(html, exempt = SETTINGS_I18N_EXEMPT) {
  const asideMatch = html.match(/<aside id="settings"[^>]*>([\s\S]*?)\n {4}<\/aside>/);
  if (!asideMatch) throw new Error('checkSettingsPanelI18nCoverage: no <aside id="settings">...</aside> block found');
  const nodes = findSettingsTextNodes(asideMatch[1]);
  return nodes.filter((n) => !n.hasI18n && !(n.locator && n.locator in exempt));
}

/// Every `title` / `aria-label` attribute inside the given Settings inner
/// HTML that carries real text, whether or not it is covered. Each entry:
/// { attr: "title" | "aria-label", tag, locator, hasI18n, text }. Locator
/// rules match findSettingsTextNodes (own id, a <label for="x"> falling
/// back to x, otherwise the nearest open ancestor's id) so the same id
/// means the same thing in both exemption tables' error messages, even
/// though the two tables are never looked up against each other.
export function findSettingsAttributeNodes(settingsInnerHtml) {
  const stack = []; // { tag, attrs }
  const found = [];
  let m;
  TOKEN_RE.lastIndex = 0;
  while ((m = TOKEN_RE.exec(settingsInnerHtml))) {
    const whole = m[0];
    if (whole.startsWith("<!--")) continue;
    if (whole.startsWith("</")) {
      const name = m[1];
      for (let i = stack.length - 1; i >= 0; i--) {
        if (stack[i].tag === name) {
          stack.length = i;
          break;
        }
      }
      continue;
    }
    if (whole.startsWith("<")) {
      const tag = m[2];
      const attrs = parseAttrs(m[3] || "");
      const selfClose = m[4] === "/";

      for (const [plainAttr, i18nAttr] of ATTR_I18N) {
        const raw = attrs[plainAttr];
        if (raw === undefined) continue;
        const text = raw.replace(/\s+/g, " ").trim();
        if (!text) continue;
        const hasI18n = i18nAttr in attrs;

        // Locator computed against currently-OPEN ancestors: this tag's own
        // opening attributes are known but it has not been pushed yet.
        let locator = attrs.id;
        if (!locator && tag === "label" && attrs.for) locator = attrs.for;
        if (!locator) {
          for (let i = stack.length - 1; i >= 0; i--) {
            if (stack[i].attrs.id) {
              locator = `${stack[i].attrs.id}>${tag}`;
              break;
            }
          }
        }
        found.push({ attr: plainAttr, tag, locator: locator || null, hasI18n, text });
      }

      if (!selfClose && !VOID_ELEMENTS.has(tag)) stack.push({ tag, attrs });
      continue;
    }
    // Text runs are the other pass's job (findSettingsTextNodes).
  }
  return found;
}

/// Violations = a title/aria-label with real text, no matching data-i18n-*
/// attribute, and no exemption. Same shape and defaulting as
/// checkSettingsPanelI18nCoverage.
export function checkSettingsPanelAttrI18nCoverage(html, exempt = SETTINGS_ATTR_I18N_EXEMPT) {
  const asideMatch = html.match(/<aside id="settings"[^>]*>([\s\S]*?)\n {4}<\/aside>/);
  if (!asideMatch) throw new Error('checkSettingsPanelAttrI18nCoverage: no <aside id="settings">...</aside> block found');
  const nodes = findSettingsAttributeNodes(asideMatch[1]);
  return nodes.filter((n) => !n.hasI18n && !(n.locator && n.locator in exempt));
}

// Convenience for callers outside the test runner (e.g. a one-off check
// against a historical revision) that want the real index.html's content
// without re-deriving the path themselves.
export function readIndexHtml() {
  return readFileSync(fileURLToPath(new URL("../index.html", import.meta.url)), "utf8");
}
