// Mechanical checks over src/locales/*.json. Plain fs.readFileSync + JSON.parse
// on purpose (not import assertions): CI runs Node 22, local is Node 25, and
// import-assertion syntax has moved between the two — this stays portable.
//
// This is the only thing that turns a missing key into a loud failure: t() in
// src/i18n.ts silently falls back to English on any gap, so a hole here would
// otherwise ship as quietly-wrong text. Per-locale key sets are therefore
// checked for exact equality against English, both ways — six more locales
// will be guarded by this same file.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { test } from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";

const localesDir = fileURLToPath(new URL("../src/locales/", import.meta.url));
const dicts = {};
for (const file of readdirSync(localesDir).filter((f) => f.endsWith(".json")).sort()) {
  const locale = path.basename(file, ".json");
  try {
    dicts[locale] = JSON.parse(readFileSync(path.join(localesDir, file), "utf8"));
  } catch (e) {
    throw new Error(`${file}: ${e.message}`);
  }
}
assert.ok(dicts.en, "src/locales/en.json must exist as the reference locale");
const otherLocales = Object.keys(dicts).filter((l) => l !== "en").sort();

function tokensOf(value) {
  return new Set([...value.matchAll(/\{([a-zA-Z0-9_]+)\}/g)].map((m) => m[1]));
}

// Plural-aware machinery (task 8). A count-sensitive key is stored as
// key.one / key.few / key.many / key.other (src/i18n.ts's t(key, vars,
// count)); English always carries exactly key.one + key.other, so that pair
// is what marks a key as a plural family in the first place.
const PLURAL_SUFFIXES = ["one", "few", "many", "other"];

function stripPluralSuffix(key) {
  for (const suffix of PLURAL_SUFFIXES) {
    if (key.endsWith(`.${suffix}`)) return key.slice(0, -(suffix.length + 1));
  }
  return key;
}

const pluralBases = new Set();
for (const key of Object.keys(dicts.en)) {
  if (key.endsWith(".one")) {
    const base = key.slice(0, -".one".length);
    if (`${base}.other` in dicts.en) pluralBases.add(base);
  }
}

// PLURAL_FORMS (locale -> which forms it carries) is parsed out of the real
// src/i18n.ts rather than re-typed here, so this file and that table cannot
// silently drift apart. Same parse, independently, on the Rust side
// (crates/core/src/i18n.rs's plural_forms_match_the_typescript_table) --
// each reads its own language's source rather than one reading the other's.
function parsePluralForms(source) {
  const block = source.match(/PLURAL_FORMS[^=]*=\s*\{([\s\S]*?)\n\};/);
  assert.ok(block, "could not find a `PLURAL_FORMS = { ... };` block in src/i18n.ts");
  const table = {};
  for (const row of block[1].matchAll(/([\w-]+):\s*\[([^\]]*)\]/g)) {
    table[row[1]] = [...row[2].matchAll(/"([a-z]+)"/g)].map((m) => m[1]);
  }
  return table;
}

const i18nSource = readFileSync(fileURLToPath(new URL("../src/i18n.ts", import.meta.url)), "utf8");
const PLURAL_FORMS = parsePluralForms(i18nSource);

// {n} and {count} are always allowed to appear or not: English's own .one
// form sometimes drops the number word entirely ("In {file}:" vs "In
// {file}, {n} places:"), and a locale is free to do the same or to use
// {count} instead of {n} for the same slot -- t(key, vars, count)
// substitutes both.
const PLURAL_TOKEN_EXEMPT = new Set(["n", "count"]);

for (const base of pluralBases) {
  test(`${base}: every locale has exactly the forms it needs`, () => {
    const violations = [];
    for (const [locale, dict] of Object.entries(dicts)) {
      const expected = new Set(PLURAL_FORMS[locale]);
      assert.ok(expected.size > 0, `no PLURAL_FORMS entry parsed for locale "${locale}"`);
      const actual = new Set(
        Object.keys(dict)
          .filter((k) => k.startsWith(`${base}.`))
          .map((k) => k.slice(base.length + 1))
          .filter((suffix) => PLURAL_SUFFIXES.includes(suffix)),
      );
      const missing = [...expected].filter((f) => !actual.has(f));
      const extra = [...actual].filter((f) => !expected.has(f));
      if (missing.length || extra.length) violations.push(`${locale} (missing ${missing}, extra ${extra})`);
    }
    assert.deepEqual(violations, [], `${base}: forms don't match PLURAL_FORMS -- ${violations.join("; ")}`);
  });
}

for (const locale of otherLocales) {
  test(`${locale}: has exactly the base keys English has`, () => {
    const enKeys = new Set(Object.keys(dicts.en).map(stripPluralSuffix));
    const localeKeys = new Set(Object.keys(dicts[locale]).map(stripPluralSuffix));
    const missing = [...enKeys].filter((k) => !localeKeys.has(k));
    const extra = [...localeKeys].filter((k) => !enKeys.has(k));
    assert.deepEqual({ missing, extra }, { missing: [], extra: [] }, `${locale}: key set differs from English`);
  });

  test(`${locale}: every value keeps the {tokens} its English value has`, () => {
    const mismatches = [];
    for (const [key, value] of Object.entries(dicts[locale])) {
      const base = stripPluralSuffix(key);
      const isPluralForm = base !== key && pluralBases.has(base);
      // Every form of a plural key is checked against English's .other --
      // not its own same-named form -- because English's .one sometimes has
      // a different token set than its .other (see PLURAL_TOKEN_EXEMPT
      // above), and ru's .few/.many have no same-named English form to
      // compare against in the first place.
      const enKey = isPluralForm ? `${base}.other` : key;
      const enValue = dicts.en[enKey];
      if (enValue === undefined) continue; // extra key, already reported above
      let enTokens = tokensOf(enValue);
      let localeTokens = tokensOf(value);
      if (isPluralForm) {
        enTokens = new Set([...enTokens].filter((t) => !PLURAL_TOKEN_EXEMPT.has(t)));
        localeTokens = new Set([...localeTokens].filter((t) => !PLURAL_TOKEN_EXEMPT.has(t)));
      }
      const same = enTokens.size === localeTokens.size && [...enTokens].every((t) => localeTokens.has(t));
      if (!same) mismatches.push(key);
    }
    assert.deepEqual(mismatches, [], `${locale}: these keys lost or gained {tokens} vs. English`);
  });

  test(`${locale}: differs from English on at least 80% of its keys`, () => {
    const keys = Object.keys(dicts[locale]);
    const differing = keys.filter((k) => dicts[locale][k] !== dicts.en[k]);
    const ratio = differing.length / keys.length;
    assert.ok(ratio >= 0.8, `only ${(ratio * 100).toFixed(1)}% of ${keys.length} ${locale} keys differ from English`);
  });
}

test("no value is empty or whitespace-only", () => {
  const violations = [];
  for (const [locale, dict] of Object.entries(dicts)) {
    for (const [key, value] of Object.entries(dict)) {
      if (typeof value !== "string" || value.trim() === "") violations.push(`${locale}.${key}`);
    }
  }
  assert.deepEqual(violations, []);
});

test("language endonyms are identical across locales", () => {
  const locales = Object.keys(dicts);
  for (const locale of locales) {
    const key = `settings.lang${locale[0].toUpperCase()}${locale.slice(1)}`;
    const values = locales.map((l) => dicts[l][key]);
    assert.ok(
      values.every((v) => v === values[0]),
      `${key} is not identical across locales: ${JSON.stringify(Object.fromEntries(locales.map((l, i) => [l, values[i]])))}`,
    );
  }
});

// index.html has no t()/T() calls, so it is not in KEYED_SOURCES below and none of
// those checks see it; its data-i18n* attributes are the only reference to a key. t()
// falls back to the raw key string on any miss, so a typo'd attribute (e.g.
// data-i18n="usge.tab") would silently paint "usge.tab" on the tab with nothing failing.
test("every data-i18n* key referenced in index.html exists in en.json", () => {
  const html = readFileSync(fileURLToPath(new URL("../index.html", import.meta.url)), "utf8");
  const attrs = ["data-i18n", "data-i18n-html", "data-i18n-title", "data-i18n-placeholder", "data-i18n-aria"];
  const pattern = new RegExp(`(?:${attrs.join("|")})="([a-zA-Z0-9_.]+)"`, "g");
  const referenced = new Set([...html.matchAll(pattern)].map((m) => m[1]));
  const missing = [...referenced].filter((k) => !(k in dicts.en));
  assert.deepEqual(missing, [], "index.html: referenced data-i18n* keys missing from en.json");
});

// Each view that owns a key prefix gets one row here, not a copy of this
// test. task 5+: add ["../src/inventory.ts", "inventory."] etc.
const KEYED_SOURCES = [
  ["../src/detail.ts", "detail."],
  ["../src/inventory.ts", "inventory."],
  ["../src/audit.ts", "audit."],
  ["../src/about.ts", "about."],
  ["../src/ledger.ts", "ledger."],
];

function escapeForRegex(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

for (const [rel, prefix] of KEYED_SOURCES) {
  test(`every ${prefix}* key referenced in ${rel} exists in en.json`, () => {
    const source = readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
    // A key reference sitting only in a comment (e.g. leftover from a rename) must not
    // count: strip block comments, then line comments, before either regex runs. `//` is
    // only a comment starter when it is not part of a `://` URL scheme — this file's own
    // sources have a live https://staas.fund/... call right beside a real T() reference
    // on the same line, and a plain `//.*$` strip silently eats everything after it.
    const stripped = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(?<!:)\/\/.*$/gm, "");
    // Direct, fully-qualified references: t("detail.foo") or plural("inventory.foo", n).
    const qualified = new RegExp(`["'](${escapeForRegex(prefix)}[a-zA-Z0-9_.]+)["']`, "g");
    // Views newer than detail.ts alias T(k) => t(`prefix${k}`, v) for their own keys
    // (inventory.ts's top-of-file convention), so the prefix never appears in a quoted
    // literal; reconstruct it here rather than leave this row checking nothing.
    const aliased = /\bT\(\s*["']([a-zA-Z0-9_.]+)["']/g;
    const referenced = new Set([
      ...[...stripped.matchAll(qualified)].map((m) => m[1]),
      ...[...stripped.matchAll(aliased)].map((m) => `${prefix}${m[1]}`),
    ]);
    // plural(key, n)'s key argument is a family's base ("detail.other"), not a
    // literal key: it's valid if the key itself exists, or its ".one" form does.
    const missing = [...referenced].filter((k) => !(k in dicts.en) && !(`${k}.one` in dicts.en));
    assert.deepEqual(missing, [], `${rel}: referenced ${prefix}* keys missing from en.json`);
  });

  // Forward guard: the qualified/aliased regexes above only ever match a plain
  // '"..."'/"'...'" literal right after t(/T(. A template-literal call —
  // T(`egg.${x}`) was a real bug caught by hand in about.ts — silently drops
  // its key out of coverage instead of failing loudly, because neither regex
  // matches a backtick. This catches that shape directly, independent of
  // whether the key it hides happens to exist in en.json.
  test(`${rel}: no t()/T() call is hidden from the coverage check above behind a template literal`, () => {
    const source = readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
    const stripped = source
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/(?<!:)\/\/.*$/gm, "")
      // The T alias's own definition legitimately builds its key with a
      // template literal (`` `prefix.${k}` ``, inventory.ts's convention) —
      // that's the one call site this guard must not flag.
      .replace(/const T = \([^)]*\)\s*=>\s*t\(`[a-zA-Z][a-zA-Z0-9_]*\.\$\{k\}`(?:,\s*v)?\);?/g, "");
    assert.doesNotMatch(
      stripped,
      /\b[tT]\(\s*`/,
      `${rel}: a t()/T() call takes a template literal, which the coverage check above cannot see through — use a literal string per branch instead (see about.ts's eggLine() for the fix)`,
    );
  });
}

// Guard against a regression task 8's follow-up fixed by hand (src/detail.ts
// used to spell "detail.session.spanDays.other" / "...lastActive.other"
// literally, which broke the moment ru.json's .other was renamed to .many).
// A plural form is only ever picked by plural()/t(key, vars, count) — never
// a literal ".one"/".few"/".many"/".other" suffix typed into a t()/T() call
// — same spirit as the template-literal guard above, one combined check
// across every KEYED_SOURCES file rather than a copy per file.
test("no keyed source spells a plural form by hand", () => {
  const handSpelledForm = /\b[tT]\(\s*["']([a-zA-Z0-9_.]+\.(?:one|few|many|other))["']/g;
  const violations = [];
  for (const [rel] of KEYED_SOURCES) {
    const source = readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
    const stripped = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(?<!:)\/\/.*$/gm, "");
    for (const m of stripped.matchAll(handSpelledForm)) {
      violations.push(`${rel}: ${m[0].trim()}`);
    }
  }
  assert.deepEqual(
    violations,
    [],
    "a plural form must be picked by plural()/t(key, vars, count), not spelled by hand in a t()/T() call: " +
      violations.join("; "),
  );
});
