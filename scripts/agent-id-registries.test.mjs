// src/agents.ts hand-maintains two id lists: AGENT_FINDING_IDS (which
// Opportunities the Agents view's "Worth a look" section shows) and
// AGENT_GUARDRAIL_CHECK_IDS (which Audit checks count into its
// failing-guardrail line). Both are copied from ids the Rust core actually
// defines (crates/core/src/inventory.rs and coaching.rs's FINDING_IDS test
// registries, and crates/core/src/audit.rs's agent_checks()), so they can go
// stale the moment Rust adds or renames an agent-related id without the
// TypeScript side changing to match. This reads the Rust sources as plain
// text (no need to compile or run Rust for this) and fails in both
// directions: an id Rust defines that TypeScript is missing, and an id
// TypeScript names that Rust no longer defines.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

async function rustSource(rel) {
  return readFile(new URL(`../${rel}`, import.meta.url), "utf8");
}

/// Pulls the quoted string literals out of a `pub(crate) const NAME: &[&str]
/// = &[ ... ];` array, single-line or multi-line alike.
function findingIdsArray(source, file) {
  const m = source.match(/\bFINDING_IDS\s*:\s*&\[&str\]\s*=\s*&\[([\s\S]*?)\];/);
  assert.ok(m, `${file}: could not find a FINDING_IDS array`);
  return [...m[1].matchAll(/"([a-z][a-z0-9-]*)"/g)].map((x) => x[1]);
}

/// The text of one function's body, braces balanced -- used to scope the
/// check-id search to exactly agent_checks(), not every check() call in
/// audit.rs (most of which are about MCP servers, permissions or usage, not
/// agents).
function functionBody(source, file, signature) {
  const start = source.search(signature);
  assert.ok(start !== -1, `${file}: could not find a function matching ${signature}`);
  const braceStart = source.indexOf("{", start);
  assert.ok(braceStart !== -1, `${file}: found the signature but no opening brace after it`);
  let depth = 0;
  for (let i = braceStart; i < source.length; i++) {
    if (source[i] === "{") depth++;
    else if (source[i] === "}") {
      depth--;
      if (depth === 0) return source.slice(braceStart, i + 1);
    }
  }
  throw new Error(`${file}: unbalanced braces reading the function body`);
}

/// Loads AGENT_FINDING_IDS and AGENT_GUARDRAIL_CHECK_IDS straight out of
/// src/agents.ts -- not re-typed here, so this test can only ever fail when
/// the real, exported sets disagree with Rust, never when this file's own
/// copy of them drifts.
async function loadAgentIdSets() {
  const source = await readFile(new URL("../src/agents.ts", import.meta.url), "utf8");
  // Only the two exported Sets are needed; every import is dropped rather
  // than resolved; nothing at this module's top level calls anything they
  // would have provided; everything else in the file (types, functions) is
  // still valid to declare without them, since none of it runs unless called.
  const stripped = source
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { money } from "./format";', "")
    .replace('import { plural, t } from "./i18n";', "")
    .replace(/import \{[\s\S]*?\} from "\.\/inventory";/, "");
  if (stripped === source) throw new Error("no substitution matched -- src/agents.ts's source shape moved under this test");
  const code = ts.transpileModule(stripped, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

test("AGENT_FINDING_IDS matches, in both directions, every 'agent' id in inventory.rs's and coaching.rs's FINDING_IDS", async () => {
  const inventoryRs = await rustSource("crates/core/src/inventory.rs");
  const coachingRs = await rustSource("crates/core/src/coaching.rs");
  const rustIds = new Set(
    [...findingIdsArray(inventoryRs, "inventory.rs"), ...findingIdsArray(coachingRs, "coaching.rs")].filter((id) =>
      id.includes("agent"),
    ),
  );
  assert.ok(rustIds.size > 0, "parsed zero agent-related finding ids out of Rust -- the FINDING_IDS array shape probably moved");

  const { AGENT_FINDING_IDS } = await loadAgentIdSets();
  const missingFromTs = [...rustIds].filter((id) => !AGENT_FINDING_IDS.has(id));
  const extraInTs = [...AGENT_FINDING_IDS].filter((id) => !rustIds.has(id));
  assert.deepEqual(
    { missingFromTs, extraInTs },
    { missingFromTs: [], extraInTs: [] },
    "src/agents.ts's AGENT_FINDING_IDS has drifted from the agent ids Rust's FINDING_IDS registries actually define",
  );
});

test("AGENT_GUARDRAIL_CHECK_IDS matches, in both directions, the check ids audit.rs's agent_checks() can actually emit with status \"attention\"", async () => {
  const auditRs = await rustSource("crates/core/src/audit.rs");
  const body = functionBody(auditRs, "audit.rs", /fn agent_checks\(/);
  // Every check(...) call inside agent_checks() -- check_from_opportunity()
  // is never used in this function, so this pattern (which requires the
  // literal text "check(", not "check_from_opportunity(") does not need to
  // exclude it separately.
  const allIds = [...body.matchAll(/\bcheck\(\s*"([a-z][a-z0-9-]*)"/g)].map((m) => m[1]);
  assert.ok(allIds.length > 0, "parsed zero check ids out of agent_checks() -- its shape probably moved");

  // Only an id with at least one "attention" branch belongs in the
  // TypeScript failing-count set at all: crates/core/src/audit.rs gives
  // "agent-model" status "consider" ONLY (never "attention" -- there is no
  // such branch for it), so counting it into a failing-guardrail total would
  // count something that can never happen. Scoped per id, not per line: an
  // id showing up once with "attention" and once with "pass" (agent-tools,
  // deny-shell) still belongs in the set.
  const canFail = new Set();
  for (const m of body.matchAll(/\bcheck\(\s*"([a-z][a-z0-9-]*)"\s*,\s*"attention"/g)) canFail.add(m[1]);
  assert.ok(canFail.size > 0, "parsed zero \"attention\"-capable check ids out of agent_checks() -- its shape probably moved");
  assert.ok(new Set(allIds).has("agent-model") && !canFail.has("agent-model"), "agent-model should be a real id in agent_checks() with no \"attention\" branch -- if this fails, audit.rs's agent-model status rules changed and AGENT_GUARDRAIL_CHECK_IDS's exclusion of it needs re-checking by hand, not just by this test");

  const { AGENT_GUARDRAIL_CHECK_IDS } = await loadAgentIdSets();
  const missingFromTs = [...canFail].filter((id) => !AGENT_GUARDRAIL_CHECK_IDS.has(id));
  const extraInTs = [...AGENT_GUARDRAIL_CHECK_IDS].filter((id) => !canFail.has(id));
  assert.deepEqual(
    { missingFromTs, extraInTs },
    { missingFromTs: [], extraInTs: [] },
    "src/agents.ts's AGENT_GUARDRAIL_CHECK_IDS has drifted from the ids audit.rs's agent_checks() can actually fail with",
  );
});
