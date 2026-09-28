// renderMcp() (src/inventory.ts) draws the MCP section's "measured for
// Claude Code only" caption unconditionally today, even when nothing in the
// list actually carries a `usage` figure -- a setup with no Claude Code
// servers, or one whose servers all fell into `attach`'s "no figure" cases,
// still shows a coverage note about a measurement nobody sees. This checks
// the note appears only once at least one server in the list has `usage`.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

// Same trick as scripts/running-agents.test.mjs: src/i18n.ts's locale JSON
// imports are inlined first, src/format.ts is inlined next with its own
// `./i18n` import dropped (localeTag, plural and t are already in scope from
// the inlined i18n source above it), and src/inventory.ts is appended last
// with its imports of the Tauri bridge, ledger.ts, i18n.ts and format.ts all
// dropped in turn -- none of them is reachable from renderMcp(). inventory.ts's
// own top-level `function render(): void` (the DOM orchestrator) is renamed
// so it cannot collide with i18n.ts's exported `render(locale, msg)`; only
// the declaration moves, never a call site, which is safe because nothing
// this file calls -- renderMcp() and the plain helpers under it -- ever
// calls the tab's own render().
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
  // src/focus.ts has no imports of its own, so it is appended as is --
  // inventory.ts's own `import { focusOrFallback } from "./focus";` is
  // dropped below, since this one copy already puts it in scope.
  const focusSource = await readFile(new URL("../src/focus.ts", import.meta.url), "utf8");

  const inventorySource = await readFile(new URL("../src/inventory.ts", import.meta.url), "utf8");
  const stripped = inventorySource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { focusOrFallback } from "./focus";', "")
    .replace('import { showLedger } from "./ledger";', "")
    .replace('import { localeTag, plural, t, tm, type Msg } from "./i18n";', "")
    .replace('import { byteSize, money, relativeDay, tokens } from "./format";', "")
    .replace("function render(): void {", "function __unusedInventoryRender(): void {");
  if (stripped === inventorySource) throw new Error("no substitution matched -- src/inventory.ts's source shape moved under this test");
  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${focusSource}\n${stripped}`, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

function server(overrides = {}) {
  return {
    name: "acme",
    client: "Claude Code",
    scope: "user",
    project: null,
    transport: "stdio",
    target: "npx acme-mcp",
    package: "acme-mcp",
    envCount: 0,
    pinTo: null,
    ...overrides,
  };
}

test("the coverage note is hidden when no server in the list has a usage figure", async () => {
  const { renderMcp, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  const html = renderMcp([server(), server({ name: "other", client: "Claude Desktop" })]);
  assert.ok(!html.includes("inv-mcp-usage-note"), "no server has `usage` -- the note must not render at all");
});

test("the coverage note appears once at least one server has a usage figure", async () => {
  const { renderMcp, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  const html = renderMcp([server({ usage: { calls: 42, resultBytes: 3 * 1024 * 1024 } }), server({ name: "quiet" })]);
  assert.ok(html.includes("inv-mcp-usage-note"), "one server has a usage figure -- the note must render");
});

test("a usage figure whose bytes cannot be sized leaves the whole chip off the row", async () => {
  // byteSize() (src/format.ts) returns "" for a count it cannot honestly
  // size (not finite, or negative) -- renderMcp() must drop the fact
  // entirely rather than splice that empty string into "N calls, {size} in
  // 30 days".
  const { renderMcp, setActiveLocale } = await loadInventoryModule();
  setActiveLocale("en");
  const html = renderMcp([server({ usage: { calls: 42, resultBytes: NaN } })]);
  assert.ok(!html.includes("in 30 days"), "byteSize gives no figure -- the calls/bytes chip must not render at all");
});
