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
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { PLURAL_SUFFIXES } from "./plural-suffixes.mjs";
import { inlineLocaleImports } from "./inline-locales.mjs";
import { checkSettingsPanelI18nCoverage, findSettingsTextNodes, SETTINGS_I18N_EXEMPT } from "./settings-i18n-coverage.mjs";

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

// Plural-aware machinery. A count-sensitive key is stored as key.one /
// key.few / key.many / key.other (src/i18n.ts's t(key, vars, count));
// English always carries exactly key.one + key.other, so that pair is what
// marks a key as a plural family in the first place. PLURAL_SUFFIXES itself
// lives in ./plural-suffixes.mjs, shared with check-demo-fixture.test.mjs.

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
assert.ok(pluralBases.size > 0, "parsed zero plural base keys out of en.json's .one/.other pairs");

// PLURAL_FORMS (locale -> which forms it carries) is parsed out of the real
// src/i18n.ts rather than re-typed here, so this file and that table cannot
// silently drift apart. Same parse, independently, on the Rust side
// (crates/core/src/i18n.rs's plural_forms_match_the_typescript_table) --
// each reads its own language's source rather than one reading the other's.
function parsePluralForms(source) {
  // \b...\b, not a bare substring match: an earlier decoy identifier that
  // merely starts with "PLURAL_FORMS" (e.g. a hypothetical
  // PLURAL_FORMS_BY_ROOT) would otherwise match first and hand back ITS
  // object body instead of the real table's -- see the decoy test below.
  const block = source.match(/\bPLURAL_FORMS\b[^=]*=\s*\{([\s\S]*?)\n\};/);
  assert.ok(block, "could not find a `PLURAL_FORMS = { ... };` block in src/i18n.ts");
  const table = {};
  for (const row of block[1].matchAll(/"?([\w-]+)"?:\s*\[([^\]]*)\]/g)) {
    table[row[1]] = [...row[2].matchAll(/"([a-z]+)"/g)].map((m) => m[1]);
  }
  return table;
}

test("parsePluralForms is not fooled by an earlier PLURAL_FORMS_BY_ROOT decoy", () => {
  const decoySource = [
    "const PLURAL_FORMS_BY_ROOT = {",
    '  en: ["decoy-should-not-be-parsed"],',
    "};",
    "",
    "export const PLURAL_FORMS: Record<Locale, readonly string[]> = {",
    '  en: ["one", "other"],',
    '  ru: ["one", "few", "many"],',
    "};",
  ].join("\n");
  assert.deepEqual(parsePluralForms(decoySource), { en: ["one", "other"], ru: ["one", "few", "many"] });
});

test("parsePluralForms reads a quoted, hyphenated locale row", () => {
  // A locale code with a hyphen needs object-literal quoting ("pt-BR": [...]),
  // unlike every bare-identifier row before it -- the row regex has to accept
  // both an optional leading/trailing quote and the hyphen itself.
  const source = [
    "export const PLURAL_FORMS: Record<Locale, readonly string[]> = {",
    '  en: ["one", "other"],',
    '  "pt-BR": ["one", "other"],',
    "};",
  ].join("\n");
  assert.deepEqual(parsePluralForms(source), { en: ["one", "other"], "pt-BR": ["one", "other"] });
});

// ts.transpileModule doesn't bundle src/i18n.ts's JSON dictionary imports
// (it transpiles one file at a time), so inlineLocaleImports()
// (scripts/inline-locales.mjs) turns them into plain object literals first;
// the same helper backs the loaders in scripts/sub2api-display.test.mjs and
// scripts/demo-synthetic.test.mjs, which have their own module-assembly
// steps on top of it.
async function loadI18nModule() {
  const source = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const localesDir = new URL("../src/locales/", import.meta.url);
  const inlined = await inlineLocaleImports(source, localesDir);
  const code = ts.transpileModule(inlined, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

test("loadI18nModule inlines a hyphenated locale file under its non-hyphenated import name", async () => {
  const { LOCALES, t, setActiveLocale } = await loadI18nModule();
  assert.ok(LOCALES.includes("pt-BR"), "pt-BR must be registered in LOCALES for this test to mean anything");
  setActiveLocale("pt-BR");
  // tray.quit resolves through the inlined pt-BR dictionary, not the English
  // fallback -- proves the "pt-BR.json" -> "ptBR" import line was actually
  // matched and replaced, not silently skipped.
  assert.equal(t("tray.quit"), "Sair do AI Task Manager");
});

// Same 12 values as crates/core/src/i18n.rs's plural_forms_match_the_typescript_table
// samples array, kept textually identical by hand so the two lists read as
// one list split across languages, not two that happen to agree today.
const PLURAL_FORM_SAMPLES = [0, 1, 2, 3, 5, 11, 12, 21, 22, 25, 100, 101];

// Expected pluralForm() output at each sample above, worked out by hand
// against the CLDR rule -- not by re-running pluralForm()'s own logic back
// at itself, which would only prove the function agrees with itself. Covers
// every locale code pluralForm()'s rule families recognise today, not just
// the three keyed in PLURAL_FORMS/LOCALES.
const EXPECTED_PLURAL_FORMS = {
  en: ["other", "one", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"],
  es: ["other", "one", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"],
  de: ["other", "one", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"],
  // Brazilian Portuguese, like French, uses the singular for zero (CLDR),
  // so its row is 0 -> "one" too, not the plain-default 0 -> "other" every
  // other alphabetic locale in this table uses. The app only ever calls
  // pluralForm() with an integer, so a half sample (0.5, 1.5) is never
  // relevant here.
  "pt-BR": ["one", "one", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"],
  fr: ["one", "one", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"],
  zh: ["other", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"],
  ja: ["other", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"],
  ko: ["other", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"],
  ru: ["many", "one", "few", "few", "many", "many", "many", "one", "few", "many", "many", "one"],
};

test("pluralForm() picks the exact CLDR form, per locale, for every sample", async () => {
  const { pluralForm } = await loadI18nModule();
  const mismatches = [];
  for (const [locale, forms] of Object.entries(EXPECTED_PLURAL_FORMS)) {
    PLURAL_FORM_SAMPLES.forEach((n, i) => {
      const got = pluralForm(locale, n);
      if (got !== forms[i]) mismatches.push(`${locale}(${n}): got "${got}", want "${forms[i]}"`);
    });
  }
  assert.deepEqual(mismatches, [], `pluralForm() mismatches: ${mismatches.join("; ")}`);
});

test("pluralForm(pt-BR, n) uses the singular for both zero and one, like French", async () => {
  // Brazilian Portuguese, like French, uses the singular for zero (CLDR).
  const { pluralForm } = await loadI18nModule();
  assert.equal(pluralForm("pt-BR", 0), "one");
  assert.equal(pluralForm("pt-BR", 1), "one");
  assert.equal(pluralForm("pt-BR", 2), "other");
});

test("check.tools.info.title renders correctly in pt-BR at zero, one and several", async () => {
  // audit.rs calls this key with count(0) when no AI tools are found -- the
  // one spot in the app that actually reaches a locale's ".one" form with a
  // count of zero, so it is the one hardcoded-singular title that had to
  // become a real {count} sentence instead of a bare "1".
  const { render } = await loadI18nModule();
  const msg = (n) => ({ key: "check.tools.info.title", vars: {}, count: n });
  assert.equal(render("pt-BR", msg(0)), "0 ferramenta de IA encontrada");
  assert.equal(render("pt-BR", msg(1)), "1 ferramenta de IA encontrada");
  assert.equal(render("pt-BR", msg(3)), "3 ferramentas de IA encontradas");
});

// render(locale, msg) exists so a caller can render a Msg in a locale other
// than whatever the popover is currently showing (a hand-built demo row's
// English fallback, say) without disturbing that locale for anything else
// that calls t()/tm() afterwards. Proven directly: set the active locale to
// something other than the one being asked for, render in the other one,
// and check the active locale never moved.
test("render(locale, msg) renders in the given locale without disturbing whichever locale is active", async () => {
  const { render, setActiveLocale, getLocale } = await loadI18nModule();
  setActiveLocale("en");
  const msg = { key: "unit.times", vars: {}, count: 2 };
  const want = dicts.ru["unit.times.few"].replace("{count}", "2");
  assert.equal(render("ru", msg), want, "render() did not pick ru's own plural form for the given locale");
  assert.equal(getLocale(), "en", "render() must not leave the active locale changed");
});

// navigator.language is read-only on the prototype but configurable, so a
// plain own-property override on the shared `navigator` object shadows it
// for the length of one test; deleting the override restores the prototype
// getter afterwards no matter how the test finishes.
async function withNavigatorLanguage(language, fn) {
  const original = Object.getOwnPropertyDescriptor(navigator, "language");
  Object.defineProperty(navigator, "language", { value: language, configurable: true });
  try {
    await fn();
  } finally {
    if (original) Object.defineProperty(navigator, "language", original);
    else delete navigator.language;
  }
}

test("detectSystemLocale maps every casing of pt-BR, plus bare pt and pt-PT, to pt-BR", async () => {
  const { detectSystemLocale, LOCALES } = await loadI18nModule();
  assert.ok(LOCALES.includes("pt-BR"), "pt-BR must be registered in LOCALES for this test to mean anything");
  const cases = {
    "pt-BR": "pt-BR",
    "pt-br": "pt-BR",
    "PT-BR": "pt-BR",
    "pt-PT": "pt-BR", // the only Portuguese this app ships; any region falls to it
    pt: "pt-BR", // a bare language tag with no region at all
  };
  for (const [language, want] of Object.entries(cases)) {
    await withNavigatorLanguage(language, async () => {
      assert.equal(detectSystemLocale(), want, `navigator.language "${language}" should resolve to "${want}"`);
    });
  }
});

test("detectSystemLocale still matches single-subtag locales case-insensitively (no pt-BR regression)", async () => {
  const { detectSystemLocale } = await loadI18nModule();
  await withNavigatorLanguage("DE-AT", async () => {
    assert.equal(detectSystemLocale(), "de");
  });
  await withNavigatorLanguage("ja", async () => {
    assert.equal(detectSystemLocale(), "ja");
  });
});

test("asLocale and normalizeLocalePref round-trip \"pt-BR\" from config unchanged", async () => {
  const { asLocale, normalizeLocalePref } = await loadI18nModule();
  assert.equal(asLocale("pt-BR"), "pt-BR");
  assert.equal(normalizeLocalePref("pt-BR"), "pt-BR");
  // Neither helper is supposed to lowercase its input today, but if one ever
  // did, it must still hand back the canonical LOCALES casing, not the
  // lowercased string -- a lowercased "pt-br" is not a valid Locale.
  assert.notEqual(asLocale("pt-BR"), "pt-br");
});

// The set is complete at nine locales now that ko has landed. Pinning the
// exact list AND its order here means a future tenth language is a
// deliberate edit to this assertion, not a silent drift -- the per-locale
// tests below already derive their targets from LOCALES/the locales
// directory, so nothing else in this file needed a hand-maintained list.
test("LOCALES is exactly the nine shipped locales, in order", async () => {
  const { LOCALES } = await loadI18nModule();
  assert.deepEqual(LOCALES, ["en", "zh", "ru", "es", "fr", "de", "ja", "pt-BR", "ko"]);
});

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

// A base key is either a bare value or a set of plural forms, never both: a
// key that carries both is two different sentences hiding under one base
// (a no-count reading via the bare value, a counted reading via the forms),
// which reads as one thing everywhere else this file reasons about keys --
// pluralBases above, and every per-base "has exactly the forms it needs"
// test it drives, only ever look at .one/.few/.many/.other, so a coexisting
// bare sibling is invisible to them, not rejected by them.
test("no base key is both bare and pluralised", () => {
  const violations = [];
  for (const [locale, dict] of Object.entries(dicts)) {
    for (const key of Object.keys(dict)) {
      if (PLURAL_SUFFIXES.some((suffix) => key.endsWith(`.${suffix}`))) continue;
      if (PLURAL_SUFFIXES.some((suffix) => `${key}.${suffix}` in dict)) violations.push(`${locale}.${key}`);
    }
  }
  assert.deepEqual(violations, [], `keys that are both a bare value and a set of plural forms: ${violations.join(", ")}`);
});

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

// The check above only ever looks at an attribute that is already there --
// it would not have caught the apiFeeds row shipping with no data-i18n at
// all, because nothing referenced a key for it to check. This one inverts
// the direction: walk every label, hint, option, button and heading inside
// the Settings panel and fail on the first one with no data-i18n* and no
// entry on SETTINGS_I18N_EXEMPT (scripts/settings-i18n-coverage.mjs), so a
// future hardcoded row fails npm test instead of shipping silently in
// English.
test("every text-bearing element inside the Settings panel has data-i18n* or a named exemption", () => {
  const html = readFileSync(fileURLToPath(new URL("../index.html", import.meta.url)), "utf8");
  const violations = checkSettingsPanelI18nCoverage(html);
  const detail = violations.map((v) => `[${v.tag}] ${v.locator ?? "(no id in scope)"}: "${v.text}"`);
  assert.deepEqual(detail, [], `Settings panel text with no data-i18n* and no exemption:\n${detail.join("\n")}`);
});

test("every SETTINGS_I18N_EXEMPT entry still matches a real element in index.html", () => {
  // The reverse check: an exemption that stops matching anything (the row
  // was fixed, renamed, or removed) should be deleted, not left to quietly
  // exempt nothing. Recomputes the unfiltered node list so a name that
  // matches an already-translated element (not a violation, so invisible to
  // the test above) still counts as "seen" here.
  const html = readFileSync(fileURLToPath(new URL("../index.html", import.meta.url)), "utf8");
  const asideMatch = html.match(/<aside id="settings"[^>]*>([\s\S]*?)\n {4}<\/aside>/);
  assert.ok(asideMatch, 'no <aside id="settings">...</aside> block found');
  const allLocators = new Set(findSettingsTextNodes(asideMatch[1]).map((n) => n.locator));
  const stale = Object.keys(SETTINGS_I18N_EXEMPT).filter((id) => !allLocators.has(id));
  assert.deepEqual(stale, [], `SETTINGS_I18N_EXEMPT entries matching no element any more: ${stale.join(", ")}`);
});

// Each view that owns a key prefix gets one row here, not a copy of this
// test. task 5+: add ["../src/inventory.ts", "inventory."] etc.
const KEYED_SOURCES = [
  ["../src/detail.ts", "detail."],
  ["../src/inventory.ts", "inventory."],
  ["../src/audit.ts", "audit."],
  ["../src/audit.ts", "section."],
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
    // literal; reconstruct it here rather than leave this row checking nothing. A file
    // can carry a second row for a DIFFERENT, fully-qualified prefix it reads with the
    // plain t() rather than its own T() (audit.ts's "section." row: sectionLabel() calls
    // t(key) directly, never T()), so this only reconstructs T(...) calls when the row's
    // own prefix is the one the file's T alias actually expands to -- otherwise every
    // T("x") in the file would be misread as "<other prefix>x" and fail for keys nothing
    // ever references under that prefix.
    const aliasMatch = source.match(/const T = \([^)]*\)\s*=>\s*t\(`([a-zA-Z][a-zA-Z0-9_]*)\.\$\{k\}`/);
    const aliasPrefix = aliasMatch ? `${aliasMatch[1]}.` : null;
    const aliased = prefix === aliasPrefix ? [...stripped.matchAll(/\bT\(\s*["']([a-zA-Z0-9_.]+)["']/g)] : [];
    const referenced = new Set([
      ...[...stripped.matchAll(qualified)].map((m) => m[1]),
      ...aliased.map((m) => `${prefix}${m[1]}`),
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
