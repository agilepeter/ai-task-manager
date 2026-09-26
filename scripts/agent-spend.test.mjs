// describeAgentSpend() (src/inventory.ts) is the only place a custom agent's
// 30-day spend row becomes the sentence a Definition row (or a built-in
// agent row) shows under its name. This renders it for three shapes -- a
// used agent with a priced model, a used agent whose model has no public
// price, and an agent with no spend row at all -- in every shipped locale,
// and fails if a sentence still carries an unfilled {var} or fell back to
// its own raw key.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

// Same trick as scripts/running-agents.test.mjs, loading the very same
// combined module (describeAgentSpend and describeAgent live in the same
// src/inventory.ts): src/i18n.ts's locale JSON imports are inlined first,
// src/format.ts is inlined next with its own `./i18n` import dropped
// (localeTag, plural and t are already in scope from the inlined i18n
// source above it), and src/inventory.ts is appended last with its imports
// of the Tauri bridge, ledger.ts, i18n.ts and format.ts all dropped in turn
// -- neither the Tauri bridge nor ledger.ts is reachable from
// describeAgentSpend(), and the other two are already in scope from the two
// inlined sources ahead of it. inventory.ts's own top-level
// `function render(): void` (the DOM orchestrator) is renamed so it cannot
// collide with i18n.ts's exported `render(locale, msg)`; only the
// declaration moves, never a call site, which is safe because nothing this
// file calls -- describeAgentSpend() and the plain helpers under it --
// ever calls the tab's own render().
// Cached after the first build, same reasoning as running-agents.test.mjs:
// every test below awaits setActiveLocale() right before it reads anything,
// so sharing one built module across this file's tests is safe.
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
    .replace('import { money, relativeDay, tokens } from "./format";', "")
    .replace("function render(): void {", "function __unusedInventoryRender(): void {");
  if (stripped === inventorySource) throw new Error("no substitution matched -- src/inventory.ts's source shape moved under this test");
  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${stripped}`, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const { LOCALES } = await loadInventoryModule();

// A fixed "now" so the day-bucketing inside relativeDay() (src/format.ts)
// is deterministic across runs rather than depending on when the suite runs.
const NOW_MS = Date.UTC(2026, 8, 24, 12, 0, 0);

// Three shapes: a used agent on a priced model a few days ago (exercises
// money(), the runCount plural and the "N days ago" branch of
// relativeDay()), a used agent whose model has no public price so cost
// is zero (exercises money(0) rather than an empty string), and an agent
// with no AgentSpend row at all -- the 30-day scan never saw it run. Neither
// carries client data, so neither ever grows a "mostly for" clause.
const USED_PRICED = { name: "deploy-checker", runs: 12, cost: 4.2, tokens: 82_000, lastUsedMs: NOW_MS - 3 * 86_400_000, topModel: "claude-sonnet-5", byClient: [] };
const USED_UNPRICED = { name: "release-notes", runs: 1, cost: 0, tokens: 5_100, lastUsedMs: NOW_MS - 20 * 60_000, topModel: "some-unpriced-model", byClient: [] };
const NEVER_RUN = undefined;

// Two more shapes for the "mostly for {client}" clause: one client strictly
// over half the 30-day cost (Acme Co's 7 of 10 -- must be named), and an
// even split (5 of 10 each -- exactly half is not a majority, so neither is
// named). byClient arrives sorted largest first, same as the real engine.
const USED_DOMINANT_CLIENT = {
  name: "deploy-checker", runs: 8, cost: 10, tokens: 60_000, lastUsedMs: NOW_MS - 86_400_000, topModel: "claude-sonnet-5",
  byClient: [["Acme Co", 7], ["Northwind", 3]],
};
const USED_NO_DOMINANT_CLIENT = {
  name: "general-purpose", runs: 4, cost: 10, tokens: 20_000, lastUsedMs: NOW_MS - 2 * 86_400_000, topModel: "claude-haiku-4-5",
  byClient: [["Acme Co", 5], ["Northwind", 5]],
};

const FIXTURES = [
  ["used, priced, a few days ago", USED_PRICED],
  ["used, unpriced model, just now", USED_UNPRICED],
  ["never run (no spend row at all)", NEVER_RUN],
  ["used, one client strictly over half the cost", USED_DOMINANT_CLIENT],
  ["used, an even split between two clients", USED_NO_DOMINANT_CLIENT],
];

function hasLeftoverBraces(s) {
  return /[{}]/.test(s);
}

function looksLikeARawKey(s) {
  return s.startsWith("inventory.");
}

for (const locale of LOCALES) {
  test(`describeAgentSpend() renders cleanly in ${locale}`, async () => {
    const { describeAgentSpend, setActiveLocale } = await loadInventoryModule();
    setActiveLocale(locale);
    for (const [label, fixture] of FIXTURES) {
      const line = describeAgentSpend(fixture, NOW_MS);
      assert.ok(!hasLeftoverBraces(line), `${locale} ${label} left a {var} unfilled: "${line}"`);
      assert.ok(!looksLikeARawKey(line), `${locale} ${label} rendered as its own raw key: "${line}"`);
    }
  });
}

test("describeAgentSpend() reads a zero-run or missing row as never run, in en", async () => {
  const { describeAgentSpend, setActiveLocale, t } = await loadInventoryModule();
  setActiveLocale("en");
  const neverRun = t("inventory.agents.neverRun");
  assert.equal(describeAgentSpend(NEVER_RUN, NOW_MS), neverRun);
  assert.equal(describeAgentSpend({ ...USED_PRICED, runs: 0 }, NOW_MS), neverRun, "a row with zero runs reads the same as no row at all");
});

test("describeAgentSpend() names the run count, the cost and a relative last-used time, in en", async () => {
  const { describeAgentSpend, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  assert.equal(describeAgentSpend(USED_PRICED, NOW_MS), "30 days: 12 runs · $4.20 · last used 3 days ago");
  assert.equal(describeAgentSpend(USED_UNPRICED, NOW_MS), "30 days: 1 run · $0.00 · last used today");
});

test("describeAgentSpend() names the dominant client when one holds strictly more than half the cost, in en", async () => {
  const { describeAgentSpend, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  assert.equal(describeAgentSpend(USED_DOMINANT_CLIENT, NOW_MS), "30 days: 8 runs · $10 · last used yesterday mostly for Acme Co");
});

test("describeAgentSpend() names no client on an even split, in en", async () => {
  const { describeAgentSpend, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  const line = describeAgentSpend(USED_NO_DOMINANT_CLIENT, NOW_MS);
  assert.equal(line, "30 days: 4 runs · $10 · last used 2 days ago");
  assert.ok(!line.includes("mostly for"), `an even split must never name a client: "${line}"`);
});

test("builtInAgentRows() renders an unattributed row last, under its own label, for spend with no attribution line, in en", async () => {
  const { builtInAgentRows, setActiveLocale, t } = await loadInventoryModule();
  setActiveLocale("en");
  // A named built-in and an empty-name row together: proves the empty-name
  // row is not just shown but shown AFTER every named one, per the group's
  // own "last row" contract.
  const NAMED = { name: "Explore", runs: 5, cost: 2.5, tokens: 40_000, lastUsedMs: NOW_MS - 86_400_000, topModel: "claude-sonnet-5", byClient: [] };
  const UNATTRIBUTED = { name: "", runs: 3, cost: 1.23, tokens: 9_000, lastUsedMs: NOW_MS - 3 * 86_400_000, topModel: "claude-sonnet-5", byClient: [] };
  const html = builtInAgentRows([], [NAMED, UNATTRIBUTED], NOW_MS);
  const label = t("inventory.agents.unattributed");
  assert.ok(html.includes(label), "the unattributed row's label never rendered");
  assert.ok(html.includes("$1.23") && html.includes("3 days ago"), "the unattributed row's own spend line never rendered");
  assert.ok(!/inv-name">\s*<\/span>/.test(html), "an empty name leaked into a row instead of the unattributed label");
  assert.ok(html.indexOf(label) > html.indexOf("Explore"), "the unattributed row must render after every named built-in");
});
