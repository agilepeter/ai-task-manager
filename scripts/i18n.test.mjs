// Mechanical checks over src/locales/*.json. Plain fs.readFileSync + JSON.parse
// on purpose (not import assertions): CI runs Node 22, local is Node 25, and
// import-assertion syntax has moved between the two — this stays portable.
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { test } from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";

const localesDir = fileURLToPath(new URL("../src/locales/", import.meta.url));
const dicts = {};
for (const file of readdirSync(localesDir).filter((f) => f.endsWith(".json")).sort()) {
  const locale = path.basename(file, ".json");
  dicts[locale] = JSON.parse(readFileSync(path.join(localesDir, file), "utf8"));
}
assert.ok(dicts.en, "src/locales/en.json must exist as the reference locale");
const otherLocales = Object.keys(dicts).filter((l) => l !== "en").sort();

// Exactly the keys Russian lacks today. The set-equality test below still
// fails on any OTHER gap (an undeclared missing key) and on any extra key
// ru has that English doesn't. When ru's coverage improves, prune this list
// to match — a stale entry here just hides a passing key, it can't hide a
// real gap.
const KNOWN_GAPS = {
  ru: [
    "detail.disabled",
    "detail.exhausted",
    "detail.expired",
    "detail.keyQuota",
    "detail.overdue",
    "detail.subscription",
    "detail.unknown",
    "detail.unknownType",
    "detail.wallet",
    "footer.onenewapiDeleted",
    "footer.onenewapiDuplicate",
    "footer.onenewapiFailed",
    "footer.onenewapiKeySaved",
    "footer.onenewapiNotCompatible",
    "footer.onenewapiProbeFailed",
    "footer.onenewapiSaved",
    "footer.sub2apiFailed",
    "label.Expiry",
    "label.Today actual cost",
    "label.Today requests",
    "label.Today tokens",
    "label.Total actual cost",
    "label.Total requests",
    "label.Total tokens",
    "metric.primaryQuota",
    "settings.onenewapi",
    "settings.onenewapiAdd",
    "settings.onenewapiAddKey",
    "settings.onenewapiDelete",
    "settings.onenewapiDeleteBody",
    "settings.onenewapiDeleteConfirm",
    "settings.onenewapiDeleteKey",
    "settings.onenewapiDeleteTitle",
    "settings.onenewapiEdit",
    "settings.onenewapiFamily",
    "settings.onenewapiFamilyTip",
    "settings.onenewapiKeyKeepHint",
    "settings.onenewapiKeyLabelPh",
    "settings.onenewapiKeySecretPh",
    "settings.onenewapiMigrateBody",
    "settings.onenewapiMigrateConfirm",
    "settings.onenewapiMigrateTitle",
    "settings.onenewapiNamePh",
    "settings.onenewapiNoKeys",
    "settings.onenewapiNote",
    "settings.onenewapiSaveKey",
    "settings.onenewapiUrlPh",
    "settings.onenewapiUrlRequired",
    "settings.siteInvalidUrl",
    "settings.sub2api",
    "settings.sub2apiFamily",
    "settings.sub2apiNote",
  ],
};

function tokensOf(value) {
  return new Set([...value.matchAll(/\{([a-zA-Z0-9_]+)\}/g)].map((m) => m[1]));
}

for (const locale of otherLocales) {
  test(`${locale}: has exactly the keys English has, modulo KNOWN_GAPS`, () => {
    const allowedMissing = new Set(KNOWN_GAPS[locale] ?? []);
    const enKeys = new Set(Object.keys(dicts.en));
    const localeKeys = new Set(Object.keys(dicts[locale]));
    const missing = [...enKeys].filter((k) => !localeKeys.has(k) && !allowedMissing.has(k));
    const extra = [...localeKeys].filter((k) => !enKeys.has(k));
    assert.deepEqual(missing, [], `${locale} is missing keys not covered by KNOWN_GAPS`);
    assert.deepEqual(extra, [], `${locale} has keys English does not have`);
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
    assert.deepEqual(mismatches, []);
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
