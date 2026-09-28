// renderChanges() (src/inventory.ts) paints the Inventory tab's Changes
// list. A row whose Change is a guardrail weakening (the same rows
// changes::opportunities() on the Rust side counts under "guardrail-removed")
// gets the app's existing amber "tighten" dot (`.inv-opp-tighten .inv-dot` in
// src/styles.css) plus a screen-reader label, so colour alone never carries
// the distinction; a neutral row keeps the same layout slot with no visible
// dot, so the two kinds of row stay aligned.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

// Same trick as scripts/mcp-usage-note.test.mjs: src/i18n.ts's locale JSON
// imports are inlined first, src/format.ts is inlined next with its own
// `./i18n` import dropped (localeTag, plural and t are already in scope from
// the inlined i18n source above it), and src/inventory.ts is appended last
// with its imports of the Tauri bridge, ledger.ts, i18n.ts and format.ts all
// dropped in turn -- none of them is reachable from renderChanges(). The
// tab's own top-level `function render(): void` is renamed so it cannot
// collide with i18n.ts's exported `render(locale, msg)`; only the
// declaration moves, never a call site, which is safe because nothing this
// file calls -- renderChanges() and the plain helpers under it -- ever calls
// the tab's own render().
let cachedModule = null;
async function loadInventoryModule() {
  if (!cachedModule) cachedModule = buildInventoryModule();
  return cachedModule;
}

async function buildInventoryModule() {
  const i18nSource = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const localesDir = new URL("../src/locales/", import.meta.url);
  const inlinedI18n = await inlineLocaleImports(i18nSource, localesDir);
  const formatSource = await readFile(new URL("../src/format.ts", import.meta.url), "utf8");
  const strippedFormat = formatSource.replace('import { localeTag, plural, t } from "./i18n";', "");
  if (strippedFormat === formatSource) throw new Error("no substitution matched -- src/format.ts's source shape moved under this test");
  const inventorySource = await readFile(new URL("../src/inventory.ts", import.meta.url), "utf8");
  const stripped = inventorySource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { showLedger } from "./ledger";', "")
    .replace('import { localeTag, plural, t, tm, type Msg } from "./i18n";', "")
    .replace('import { byteSize, money, relativeDay, tokens } from "./format";', "")
    .replace("function render(): void {", "function __unusedInventoryRender(): void {");
  if (stripped === inventorySource) throw new Error("no substitution matched -- src/inventory.ts's source shape moved under this test");
  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${stripped}`, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

function change(overrides = {}) {
  return {
    kind: "changed",
    what: "server",
    name: "docs",
    guardrail: false,
    msg: { key: "changes.server.transport", vars: { name: "docs" }, count: null },
    text: "docs now connects another way",
    ...overrides,
  };
}

const GUARDRAIL_CHANGE = change({
  kind: "removed",
  what: "hook",
  name: "PreToolUse",
  guardrail: true,
  msg: { key: "changes.hook.removed", vars: { event: "PreToolUse" }, count: null },
  text: "The PreToolUse hook disappeared",
});

test("a guardrail row gets the amber tighten dot and a screen-reader label; a neutral row gets neither", async () => {
  const { renderChanges, setActiveLocale, openSections } = await loadInventoryModule();
  setActiveLocale("en");
  openSections.add("changes");
  const setupChanges = { since: "2026-09-01", changes: [GUARDRAIL_CHANGE, change()], daysOfHistory: 7 };
  const html = renderChanges(setupChanges, "");

  // Reused, not reinvented: the exact class styles.css already keys its amber
  // color off for a "tighten" Opportunity.
  const guardrailDotCount = (html.match(/inv-change-dot inv-opp-tighten/g) ?? []).length;
  assert.equal(guardrailDotCount, 1, `expected exactly one marked row: ${html}`);
  assert.match(html, /role="img" aria-label="Guardrail removed"/, "the guardrail dot needs a non-colour label too");

  // The neutral row still carries a same-shaped dot span (for alignment), just
  // never the amber class and never a label pretending it means something.
  const neutralDotCount = (html.match(/inv-change-dot"[^>]*aria-hidden="true"/g) ?? []).length;
  assert.equal(neutralDotCount, 1, `expected exactly one unmarked row: ${html}`);
  assert.ok(!html.includes('inv-change-dot aria-hidden'), "sanity: the regex above must not silently match zero rows");
});

test("renderChanges shows the setup history error, then the loading hint, before any data arrives", async () => {
  const { renderChanges, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  assert.match(renderChanges(null, "disk is full"), /disk is full/);
  assert.match(renderChanges(null, ""), /Loading/);
});

test("the guardrail mark's aria-label renders cleanly in every locale, with no leftover {var} or raw key", async () => {
  const { renderChanges, setActiveLocale, LOCALES, openSections } = await loadInventoryModule();
  openSections.add("changes");
  for (const locale of LOCALES) {
    setActiveLocale(locale);
    const setupChanges = { since: "2026-09-01", changes: [GUARDRAIL_CHANGE], daysOfHistory: 7 };
    const html = renderChanges(setupChanges, "");
    const label = html.match(/aria-label="([^"]*)"/)?.[1];
    assert.ok(label, `${locale}: no aria-label rendered on the guardrail dot`);
    assert.ok(!/[{}]/.test(label), `${locale}: left a {var} unfilled: "${label}"`);
    assert.ok(!label.startsWith("changes."), `${locale}: rendered as its own raw key: "${label}"`);
  }
});
