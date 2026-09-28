// The Audit panel's own reload-freshness rule (src/audit.ts's shouldReload(),
// mirroring src/agents.ts's identical function -- each view keeps its own
// tiny copy rather than share one, same convention as esc() and
// closeFocusTarget() elsewhere in both files). Reopening the popover happens
// far more often than the underlying setup actually changes, so
// reloadAudit(false) (the popover-shown path) skips its own get_audit() call
// within this window; reloadAudit(true) (the rescan path) always applies,
// since a rescan is a real change, not a "maybe".
//
// This file needs none of i18n.ts's locale JSON (shouldReload() calls
// neither t() nor plural()), so unlike the other harnesses in this suite it
// transpiles src/audit.ts alone, with its two imports dropped -- nothing
// else in the file is called at module-evaluation time, so the now-undefined
// `invoke`/`plural`/`t`/`tm` identifiers never actually run.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

let cachedModule = null;
async function loadAuditModule() {
  if (!cachedModule) cachedModule = buildAuditModule();
  return cachedModule;
}

async function buildAuditModule() {
  const auditSource = await readFile(new URL("../src/audit.ts", import.meta.url), "utf8");
  const stripped = auditSource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { plural, t, tm, type Msg } from "./i18n";', "");
  if (stripped === auditSource) throw new Error("no substitution matched -- src/audit.ts's source shape moved under this test");
  const code = ts.transpileModule(stripped, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

test("audit.ts shouldReload(): closed never reloads, open+fresh does not, open+stale does, a backwards clock reloads", async () => {
  const { shouldReload, RELOAD_FRESHNESS_MS } = await loadAuditModule();
  const now = 1_790_000_000_000;

  assert.equal(shouldReload(now - 1, now, false), false, "a closed panel must never reload, no matter how stale");
  assert.equal(shouldReload(now, now, true), false, "just loaded (age 0) must not reload");
  assert.equal(shouldReload(now - (RELOAD_FRESHNESS_MS - 1), now, true), false, "one millisecond inside the freshness window must not reload");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS, now, true), true, "exactly at the freshness window must reload");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS - 1, now, true), true, "past the freshness window must reload");
  assert.equal(shouldReload(now + 1, now, true), true, "a clock that moved backwards must reload rather than trust the (negative) age");
});

// render() (src/audit.ts) must not write into #audit-body once the panel has
// closed -- a get_audit() call still in flight when the panel closed must
// not paint over it when it lands late (same fix as src/agents.ts's own
// render()). No DOM exists in this harness to prove that behaviourally, so
// this checks the guard is the first statement, at the source level.
test("render() bails out before touching the DOM when the panel is closed", async () => {
  const source = await readFile(new URL("../src/audit.ts", import.meta.url), "utf8");
  const match = source.match(/function render\(\): void \{\n( {2}.*\n)+?\}/);
  assert.ok(match, "render() not found, or its source shape moved under this test");
  assert.match(match[0].split("\n")[1], /^\s*if \(!isOpen\(\)\) return;/, "render()'s first statement must bail out when the panel is not open");
});

// reloadAudit(force) must always proceed when force is true (a rescan is a
// real change, never a "maybe") and must consult shouldReload() when it is
// not (the popover-shown path) -- checked at the source level, the same way
// applyRescan()'s own body is checked in scripts/agents-view.test.mjs.
test("reloadAudit(): force bypasses shouldReload(), the default does not", async () => {
  const source = await readFile(new URL("../src/audit.ts", import.meta.url), "utf8");
  const match = source.match(/export function reloadAudit\(force = false\): void \{[\s\S]*?\n\}/);
  assert.ok(match, "reloadAudit(force = false) not found, or its source shape moved under this test");
  assert.match(match[0], /!force && !shouldReload\(/, "reloadAudit() must skip its shouldReload() check when force is true");
});

test("audit.ts and agents.ts agree on the freshness window", async () => {
  const { RELOAD_FRESHNESS_MS: auditMs } = await loadAuditModule();
  const agentsSource = await readFile(new URL("../src/agents.ts", import.meta.url), "utf8");
  const match = agentsSource.match(/export const RELOAD_FRESHNESS_MS = ([\d_]+);/);
  assert.ok(match, "src/agents.ts no longer exports a plain numeric RELOAD_FRESHNESS_MS -- update this test's pattern");
  assert.equal(auditMs, Number(match[1].replace(/_/g, "")), "the Audit and the Agents view must apply the same freshness window");
});
