// describeAgent() (src/inventory.ts) is the only place a live agent row's
// three strings get composed -- renderAgents() just paints whatever it
// returns. This renders it for three fixtures in every shipped locale and
// fails if a sentence still carries an unfilled {var}, fell back to its own
// raw key, or leaked a fact the row must never show (a pid, or a client name
// on a row that has none).
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

// Same trick as scripts/demo-synthetic.test.mjs: ts.transpileModule handles
// one file at a time, so src/i18n.ts's locale JSON imports are inlined first
// and src/inventory.ts is appended after, as one combined source. Unlike
// synthetic.ts, inventory.ts also imports the Tauri bridge and ledger.ts --
// neither is reachable from describeAgent(), so both import lines are
// dropped rather than resolved. describeAgent() does reach src/format.ts's
// money()/tokens(), so that file is inlined the same way i18n.ts is: its own
// `./i18n` import is dropped (localeTag is already in scope from the inlined
// i18n source above it) and its body is appended ahead of inventory.ts's, with
// inventory.ts's own import of the two functions dropped in turn. Its own
// top-level `function render(): void` (the DOM orchestrator) would otherwise
// collide with i18n.ts's exported `render(locale, msg)`; only the declaration
// is renamed, not its call sites, which is safe because nothing this file
// calls -- describeAgent() and the plain helpers under it -- ever calls the
// tab's own render().
// Cached after the first build: every test below calls loadInventoryModule()
// again, and re-reading, re-inlining and re-transpiling this combined source
// from scratch each time was most of this file's runtime. Sharing one built
// module is safe because every test awaits setActiveLocale() right before it
// reads anything -- nothing here depends on whatever locale a previous test
// left active -- and node:test runs a file's top-level tests one at a time,
// never overlapping two of these calls.
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
  const strippedFormat = formatSource.replace('import { localeTag } from "./i18n";', "");
  if (strippedFormat === formatSource) throw new Error("no substitution matched -- src/format.ts's source shape moved under this test");
  const inventorySource = await readFile(new URL("../src/inventory.ts", import.meta.url), "utf8");
  const stripped = inventorySource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { showLedger } from "./ledger";', "")
    .replace('import { localeTag, plural, t, tm, type Msg } from "./i18n";', "")
    .replace('import { money, tokens } from "./format";', "")
    .replace("function render(): void {", "function __unusedInventoryRender(): void {");
  if (stripped === inventorySource) throw new Error("no substitution matched -- src/inventory.ts's source shape moved under this test");
  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${stripped}`, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const { LOCALES } = await loadInventoryModule();

// Three shapes: a priced pace with a client (area implies the client), an
// unpriced pace with an area but no client rule matched it, and a Windows
// row -- no cwd, so no folder, no area, no pace either (attach_context in
// procs.rs never gets a folder to look a session up for).
const PRICED_WITH_CLIENT = {
  tool: "Claude Code", pid: 54321, elapsedSecs: 5400, rssBytes: 300 * 1048576, cpuPercent: 4.2,
  cwd: "/Users/pat/dev/acme-webapp", area: "acme-portal/web", client: "Acme Co",
  pace: { sessionId: "session-a", tokens10m: 12000, cost10m: 0.45, priced: true, idleSecs: 12, model: "claude-opus-4-1", area: "acme-portal/web" },
};
const UNPRICED_AREA_ONLY = {
  tool: "Codex", pid: 61234, elapsedSecs: 900, rssBytes: 150 * 1048576, cpuPercent: null,
  cwd: "/Users/pat/dev/scratchpad", area: "scratchpad", client: null,
  pace: { sessionId: "session-b", tokens10m: 500, cost10m: 0, priced: false, idleSecs: 5, model: "gpt-5-codex", area: "scratchpad" },
};
const WINDOWS_NO_CWD = {
  tool: "Gemini CLI", pid: 70009, elapsedSecs: 300, rssBytes: 90 * 1048576, cpuPercent: null,
  cwd: null, area: null, client: null, pace: null,
};
const FIXTURES = [
  ["priced pace with client", PRICED_WITH_CLIENT],
  ["unpriced pace, area only", UNPRICED_AREA_ONLY],
  ["windows row, no cwd and no pace", WINDOWS_NO_CWD],
];

function hasLeftoverBraces(s) {
  return /[{}]/.test(s);
}

function looksLikeARawKey(s) {
  return s.startsWith("inventory.");
}

for (const locale of LOCALES) {
  test(`describeAgent() renders cleanly in ${locale}`, async () => {
    const { describeAgent, setActiveLocale } = await loadInventoryModule();
    setActiveLocale(locale);
    for (const [label, fixture] of FIXTURES) {
      const d = describeAgent(fixture);
      const strings = [["title", d.title], ["place", d.place], ["pace", d.pace], ["tip", d.tip]];
      for (const [field, value] of strings) {
        if (value == null) continue;
        assert.ok(!hasLeftoverBraces(value), `${locale} ${label} ${field} left a {var} unfilled: "${value}"`);
        assert.ok(!looksLikeARawKey(value), `${locale} ${label} ${field} rendered as its own raw key: "${value}"`);
        assert.ok(!value.includes(String(fixture.pid)), `${locale} ${label} ${field} leaked the pid: "${value}"`);
      }
    }
  });
}

test("describeAgent() only names the client on the fixture that has one", async () => {
  const { describeAgent, setActiveLocale, LOCALES: locales } = await loadInventoryModule();
  for (const locale of locales) {
    setActiveLocale(locale);
    const withClient = describeAgent(PRICED_WITH_CLIENT);
    assert.ok(withClient.place.includes("Acme Co"), `${locale}: place should name the client when the row has one: "${withClient.place}"`);
    for (const [label, fixture] of [["unpriced pace, area only", UNPRICED_AREA_ONLY], ["windows row, no cwd and no pace", WINDOWS_NO_CWD]]) {
      const d = describeAgent(fixture);
      for (const [field, value] of [["title", d.title], ["place", d.place], ["pace", d.pace], ["tip", d.tip]]) {
        if (value == null) continue;
        assert.ok(!value.includes("Acme Co"), `${locale} ${label} ${field} named a client it does not have: "${value}"`);
      }
    }
  }
});

test("describeAgent() has no third line for a row with no pace, and an idle line for a quiet one", async () => {
  const { describeAgent, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  assert.equal(describeAgent(WINDOWS_NO_CWD).pace, null);
  assert.equal(describeAgent(WINDOWS_NO_CWD).tip, null);
  const quiet = describeAgent({ ...PRICED_WITH_CLIENT, pace: { ...PRICED_WITH_CLIENT.pace, tokens10m: 0, idleSecs: 90 } });
  assert.equal(quiet.pace, "idle 1 min");
});

test("describeAgent() falls back through folder -> area -> client in en", async () => {
  const { describeAgent, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  assert.equal(describeAgent(WINDOWS_NO_CWD).place, "folder unknown");
  assert.equal(describeAgent(UNPRICED_AREA_ONLY).place, "dev/scratchpad · scratchpad");
  assert.equal(describeAgent(PRICED_WITH_CLIENT).place, "dev/acme-webapp · acme-portal/web · for Acme Co");
});

// renderAgents() (src/inventory.ts) shows this sentence instead of
// empty.runningAgents when a scan of the running agents failed -- a failure
// must say so rather than reading exactly like the "nothing running" case.
// This calls the module's own t() with the fully-qualified key, the same way
// renderAgents() reaches it through its local T() alias, so a missing key or
// a var-name typo in any locale's register fails here.
test("empty.runningAgentsError carries the error message with no leftover {error} in every locale", async () => {
  const { t: translate, setActiveLocale, LOCALES: locales } = await loadInventoryModule();
  for (const locale of locales) {
    setActiveLocale(locale);
    const rendered = translate("inventory.empty.runningAgentsError", { error: "permission denied" });
    assert.ok(!hasLeftoverBraces(rendered), `${locale}: left a {var} unfilled: "${rendered}"`);
    assert.ok(!looksLikeARawKey(rendered), `${locale}: rendered as its own raw key: "${rendered}"`);
    assert.ok(rendered.includes("permission denied"), `${locale}: error message missing from the rendered sentence: "${rendered}"`);
  }
});
