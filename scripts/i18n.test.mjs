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

for (const locale of otherLocales) {
  test(`${locale}: has exactly the keys English has`, () => {
    const enKeys = new Set(Object.keys(dicts.en));
    const localeKeys = new Set(Object.keys(dicts[locale]));
    const missing = [...enKeys].filter((k) => !localeKeys.has(k));
    const extra = [...localeKeys].filter((k) => !enKeys.has(k));
    assert.deepEqual({ missing, extra }, { missing: [], extra: [] }, `${locale}: key set differs from English`);
  });

  test(`${locale}: every value keeps the {tokens} its English value has`, () => {
    const mismatches = [];
    for (const [key, value] of Object.entries(dicts[locale])) {
      const enValue = dicts.en[key];
      if (enValue === undefined) continue; // extra key, already reported above
      const enTokens = tokensOf(enValue);
      const localeTokens = tokensOf(value);
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
}
