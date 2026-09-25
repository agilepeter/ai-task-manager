// src/demo-fixture.json is committed, generated data (npm run fixture:demo
// runs the real engine against a fictional machine and writes the result).
// Nothing regenerates it on every build, so nothing else would notice if a
// future engine change quietly stopped attaching titleMsg/detailMsg to a
// finding, or pointed one at a key that does not exist -- the demo would
// just fall back to English and look fine until someone switches its locale
// by hand. This file reads the committed JSON directly (no cargo, no build)
// so that gap fails `npm test` instead of waiting to be noticed by eye.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { PLURAL_SUFFIXES } from "./plural-suffixes.mjs";

const fixturePath = fileURLToPath(new URL("../src/demo-fixture.json", import.meta.url));
const enPath = fileURLToPath(new URL("../src/locales/en.json", import.meta.url));
const fixture = JSON.parse(readFileSync(fixturePath, "utf8"));
const en = JSON.parse(readFileSync(enPath, "utf8"));

// A key travels as a bare value ("section.setup") or as a family of plural
// forms ("finding.foo.one" / ".other" / ...) -- never both (src/i18n.ts's
// own dictionary tests enforce that on every locale file already), so
// existence here means either shape is present in English.
function keyExists(key) {
  if (key in en) return true;
  return PLURAL_SUFFIXES.some((suffix) => `${key}.${suffix}` in en);
}

// Every Msg key on the wire is namespaced by what emitted it: a finding
// (inventory/coaching/procs/drift), a check or section on the audit, or a
// shared noun a sentence interpolates (unit.process, unit.times). Anything
// else is a typo'd or hand-written key that never went through Msg::new.
const KEY_PREFIXES = ["finding.", "check.", "section.", "unit."];

// Walks a Msg and, recursively, any Msg hiding inside its own vars: a
// sentence that needs two independent counts (e.g. a server count and a
// process count inside it) carries the second one as a nested Msg under a
// var, most often keyed under unit.*, rather than folding both into one
// plural selection. Yields every {key, vars, count} node found, each paired
// with a human-readable location for error messages.
function* msgNodes(msg, where) {
  yield [msg, where];
  if (msg && typeof msg === "object" && msg.vars && typeof msg.vars === "object") {
    for (const [varName, value] of Object.entries(msg.vars)) {
      if (value && typeof value === "object" && !Array.isArray(value) && typeof value.key === "string") {
        yield* msgNodes(value, `${where}.vars.${varName}`);
      }
    }
  }
}

function checkMsg(msg, where, errors) {
  if (msg === null || typeof msg !== "object" || Array.isArray(msg)) {
    errors.push(`${where}: expected a Msg object ({key, vars}), got ${JSON.stringify(msg)}`);
    return;
  }
  for (const [node, loc] of msgNodes(msg, where)) {
    if (typeof node.key !== "string" || node.key.length === 0) {
      errors.push(`${loc}: missing or empty "key"`);
      continue;
    }
    if (node.vars === null || typeof node.vars !== "object" || Array.isArray(node.vars)) {
      errors.push(`${loc} (key "${node.key}"): "vars" is missing or not an object`);
    }
    if (!KEY_PREFIXES.some((prefix) => node.key.startsWith(prefix))) {
      errors.push(`${loc}: key "${node.key}" does not start with ${KEY_PREFIXES.join(", ")}`);
    }
    if (!keyExists(node.key)) {
      errors.push(`${loc}: key "${node.key}" is not in src/locales/en.json, bare or with plural forms`);
    }
  }
}

// The "check.<id>.<status>" a title key reduces to once its trailing
// ".title" (bare, or its .one/.other plural pair) is stripped -- the same
// shape both en.json's own check.* keys and a check row's titleMsg.key
// share. Null for a key that is not shaped that way at all.
function titlePrefix(key) {
  for (const suffix of [".title.one", ".title.other", ".title"]) {
    if (key.endsWith(suffix)) return key.slice(0, -suffix.length);
  }
  return null;
}

// Every "check.<id>.<status>" prefix whose title exists in en.json but
// whose detail does not: nothing will ever render a detail sentence for
// it, so a null detailMsg on a row at that exact prefix is the only shape
// possible, not a bug. Derived straight from the dictionary instead of
// hand-listed, so a check gaining or losing its detail key changes this
// set by itself instead of quietly falling out of step with it. Entry for
// entry, this is meant to equal NO_DETAIL in crates/core/src/i18n.rs's
// no_detail_registry_matches_the_english_dictionary test -- read the two
// together whenever either changes.
const NULLABLE_DETAIL_PREFIXES = new Set(
  Object.keys(en)
    .filter((k) => k.startsWith("check."))
    .map(titlePrefix)
    .filter((prefix) => prefix !== null && !keyExists(`${prefix}.detail`)),
);

// "check.tools.info" cannot be derived the same way: en.json DOES define
// check.tools.info.detail (the sentence used when the tool list is empty),
// but the non-empty-list branch reuses that very same title key with the
// joined tool names as plain data instead, so its detailMsg is null too
// even though the key exists. Which branch ran is a runtime fact en.json
// has no way to encode, so this one id stays a literal.
const LEGACY_NULLABLE_PREFIXES = new Set(["check.tools.info"]);

test("every inventory.opportunities[] entry carries a titleMsg and a real detailMsg", () => {
  const opportunities = fixture?.inventory?.opportunities;
  assert.ok(Array.isArray(opportunities) && opportunities.length > 0, "fixture.inventory.opportunities is missing, empty, or not an array");
  const errors = [];
  for (const o of opportunities) {
    const where = `opportunity "${o.id}"`;
    checkMsg(o.titleMsg, `${where}.titleMsg`, errors);
    if (o.detailMsg === null || o.detailMsg === undefined) {
      errors.push(`${where}: detailMsg is ${o.detailMsg === null ? "null" : "missing"}; every opportunity has a real detail sentence`);
    } else {
      checkMsg(o.detailMsg, `${where}.detailMsg`, errors);
    }
  }
  assert.deepEqual(errors, [], `\n${errors.join("\n")}`);
});

test("every audit.sections[].checks[] entry carries a titleMsg, and a null detailMsg only where none is due", () => {
  const sections = fixture?.audit?.sections;
  assert.ok(Array.isArray(sections) && sections.length > 0, "fixture.audit.sections is missing, empty, or not an array");
  const errors = [];
  for (const section of sections) {
    assert.ok(Array.isArray(section.checks), `section "${section.name}": checks is not an array`);
    for (const c of section.checks) {
      const where = `check "${c.id}" (status "${c.status}", section "${section.name}")`;
      checkMsg(c.titleMsg, `${where}.titleMsg`, errors);
      if (c.detailMsg === null) {
        const prefix = c.titleMsg && typeof c.titleMsg.key === "string" ? titlePrefix(c.titleMsg.key) : null;
        if (!prefix || (!NULLABLE_DETAIL_PREFIXES.has(prefix) && !LEGACY_NULLABLE_PREFIXES.has(prefix))) {
          const named = prefix ?? c.titleMsg?.key ?? "(no titleMsg.key)";
          errors.push(`${where}: detailMsg is null, which is only expected when en.json has no "${named}.detail" key (or for ${[...LEGACY_NULLABLE_PREFIXES].join(", ")})`);
        }
      } else if (c.detailMsg === undefined) {
        errors.push(`${where}: detailMsg is missing (should be a Msg object, or explicit null for a pure-data/empty detail)`);
      } else {
        checkMsg(c.detailMsg, `${where}.detailMsg`, errors);
      }
    }
  }
  assert.deepEqual(errors, [], `\n${errors.join("\n")}`);
});

test("every audit.sections[] entry carries a section.* nameKey that exists in en.json", () => {
  const sections = fixture?.audit?.sections;
  assert.ok(Array.isArray(sections) && sections.length > 0, "fixture.audit.sections is missing, empty, or not an array");
  const errors = [];
  for (const section of sections) {
    const where = `section "${section.name}"`;
    if (typeof section.nameKey !== "string" || section.nameKey.length === 0) {
      errors.push(`${where}: missing or empty nameKey`);
      continue;
    }
    if (!section.nameKey.startsWith("section.")) {
      errors.push(`${where}: nameKey "${section.nameKey}" does not start with "section."`);
    }
    if (!keyExists(section.nameKey)) {
      errors.push(`${where}: nameKey "${section.nameKey}" is not in src/locales/en.json`);
    }
  }
  assert.deepEqual(errors, [], `\n${errors.join("\n")}`);
});
