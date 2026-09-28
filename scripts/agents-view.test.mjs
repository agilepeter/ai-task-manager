// The Agents view (src/agents.ts) gathers what used to take six looks --
// Inventory's Running now, Setup's custom and built-in agent lists,
// Opportunities, the Audit's agent guardrail checks, and a session's Detail
// line -- into one panel. renderAgentsView() is its whole body, pure (no
// DOM, no invoke), the same way src/inventory.ts's renderChanges() is: takes
// its data as parameters instead of reading module state, so it can be
// exercised here exactly like every other pure renderer in this suite.
//
// Same combined-module trick as scripts/agent-spend.test.mjs and
// scripts/inventory-changes.test.mjs, one file further: src/i18n.ts's locale
// JSON imports are inlined first, src/format.ts is inlined next with its own
// `./i18n` import dropped, src/inventory.ts is appended with its four
// imports dropped and its own top-level `function render(): void` renamed so
// it cannot collide with i18n.ts's exported `render(locale, msg)`, and
// src/agents.ts is appended last with its three imports dropped (everything
// they name -- invoke, plural, t, and inventory.ts's exports -- is already
// in scope from the sources above it) and its OWN top-level
// `function render(): void` renamed in turn, so neither it nor
// inventory.ts's collides with i18n's or each other's. Both files also
// declare their own private `esc()` (the same convention audit.ts and
// about.ts follow -- each view keeps a tiny copy rather than share one), so
// agents.ts's every `esc` identifier is renamed to `__agentsEsc` -- the
// declaration and every call site alike, a plain word-boundary rename, not a
// single substitution -- before it is appended after inventory.ts's own.
// Only declarations move, never a call site's meaning, which is safe because
// nothing this file calls -- renderAgentsView() and the plain helpers under
// it -- ever calls either tab's own DOM-writing render().
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

let cachedModule = null;
async function loadAgentsModule() {
  if (!cachedModule) cachedModule = buildAgentsModule();
  return cachedModule;
}

async function buildAgentsModule() {
  const i18nSource = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const localesDir = new URL("../src/locales/", import.meta.url);
  const inlinedI18n = await inlineLocaleImports(i18nSource, localesDir);

  const formatSource = await readFile(new URL("../src/format.ts", import.meta.url), "utf8");
  const strippedFormat = formatSource.replace('import { localeTag, plural, t } from "./i18n";', "");
  if (strippedFormat === formatSource) throw new Error("no substitution matched -- src/format.ts's source shape moved under this test");

  const inventorySource = await readFile(new URL("../src/inventory.ts", import.meta.url), "utf8");
  const strippedInventory = inventorySource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { showLedger } from "./ledger";', "")
    .replace('import { localeTag, plural, t, tm, type Msg } from "./i18n";', "")
    .replace('import { byteSize, money, relativeDay, tokens } from "./format";', "")
    .replace("function render(): void {", "function __unusedInventoryRender(): void {");
  if (strippedInventory === inventorySource) throw new Error("no substitution matched -- src/inventory.ts's source shape moved under this test");

  const agentsSource = await readFile(new URL("../src/agents.ts", import.meta.url), "utf8");
  const strippedAgents = agentsSource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { plural, t } from "./i18n";', "")
    .replace(
      `import {
  agentRows,
  agentsSummaryLine,
  builtInAgentRows,
  renderAgents,
  renderOpportunityRows,
  type AgentSpend,
  type Inventory,
  type RunningAgent,
} from "./inventory";`,
      "",
    )
    .replace("function render(): void {", "function __unusedAgentsRender(): void {")
    .replace(/\besc\b/g, "__agentsEsc")
    // Both files export their own rerender() (redraw in place after a
    // locale switch); this test never calls either one, so the export is
    // simply renamed out of the way rather than needed under any name.
    .replace(/\brerender\b/g, "__agentsRerender")
    // Both files also alias their own key prefix to a local `T`. transpileModule
    // has no explicit `target` here (matching every other harness in this
    // suite), so `const T = …` downlevels to `var T = …` -- and a duplicate
    // `var` at combined-module top level is legal JS, not a SyntaxError like
    // the two collisions above: it silently overwrites, so every "inventory."
    // key inventory.ts's own functions read through T() would silently read
    // through agents.ts's "agents." alias instead. Renamed the same way, so
    // agents.ts's `T` stays its own binding.
    .replace(/\bT\b/g, "__agentsT");
  if (strippedAgents === agentsSource) throw new Error("no substitution matched -- src/agents.ts's source shape moved under this test");

  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${strippedInventory}\n${strippedAgents}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const NOW_MS = 1_790_000_000_000;

/** The same escaping the view itself applies to every interpolated string --
 *  used here to build the EXPECTED text for a plain-English fixture value
 *  (like an apostrophe in "agent's"), never to sanitize anything the test
 *  asserts against. */
function esc(s) {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
}

function definition(overrides = {}) {
  return { name: "deploy-checker", scope: "user", project: null, model: "sonnet", tools: ["Bash", "Read"], ...overrides };
}

function agentSpendRow(overrides = {}) {
  return { name: "deploy-checker", runs: 2, cost: 1.69, tokens: 180799, lastUsedMs: NOW_MS - 86400000, topModel: "claude-sonnet-5", byClient: [], ...overrides };
}

function runningAgent(overrides = {}) {
  return {
    tool: "Claude Code", pid: 4821, elapsedSecs: 5400, rssBytes: 342 * 1048576, cpuPercent: 6.4,
    cwd: "/Users/jordan/dev/acme-webapp", area: "acme-portal/web", client: "Acme Co",
    pace: null,
    ...overrides,
  };
}

function inventory(overrides = {}) {
  return {
    mcpServers: [], agents: [], skills: [], hooks: [],
    permissions: { defaultMode: null, allow: 0, ask: 0, deny: 0 },
    model: null, projects: 1, tools: [], opportunities: [],
    ...overrides,
  };
}

function opportunity(overrides = {}) {
  return { id: "agents-none", kind: "learn", title: "No custom agents defined", detail: "detail", learnUrl: null, ...overrides };
}

test("the_agents_view_shows_what_inventory_showed: same rows, same text, for a fixture with running agents, custom agents and built-ins", async () => {
  const { renderAgentsView, renderAgents, agentRows, builtInAgentRows, renderOpportunityRows, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");

  const running = [runningAgent()];
  const agents = [definition({ name: "deploy-checker" }), definition({ name: "release-notes", model: null, tools: null })];
  const spend = [
    agentSpendRow({ name: "deploy-checker" }),
    agentSpendRow({ name: "general-purpose", runs: 3, cost: 4.2 }), // built-in
  ];
  // One agent finding and one that is not (an MCP finding, exactly the kind
  // Opportunities shows that this view must never invent or duplicate) --
  // proves the "Worth a look" section shows the first with the same
  // renderer Opportunities uses, and leaves the second out entirely.
  const agentFinding = opportunity({ id: "agent-unused", kind: "learn", title: "1 agent has not run in 30 days" });
  const mcpFinding = opportunity({ id: "mcp-remote", kind: "learn", title: "A server connects remotely" });
  const inv = inventory({ agents, opportunities: [agentFinding, mcpFinding] });

  const html = renderAgentsView(inv, running, "", spend, 0, NOW_MS);

  // Reused verbatim, not reimplemented: the view's HTML contains the exact
  // same markup these renderers produce when called directly on the same
  // data, byte for byte.
  const spendByName = new Map(spend.map((s) => [s.name, s]));
  assert.ok(html.includes(renderAgents(running)), "Running now rows are not the exact renderAgents() output");
  assert.ok(html.includes(agentRows(agents, spendByName, NOW_MS)), "Your agents rows are not the exact agentRows() output");
  assert.ok(html.includes(builtInAgentRows(agents, spend, NOW_MS)), "Built-in agents rows are not the exact builtInAgentRows() output");
  assert.ok(html.includes(renderOpportunityRows([agentFinding])), "Worth a look's finding is not the exact renderOpportunityRows() output");
  assert.ok(!html.includes(mcpFinding.title), "a non-agent finding leaked into Worth a look");
});

test("the_view_offers_no_action_on_an_agent: no button starts, stops, ends or edits an agent row", async () => {
  const { renderAgents, agentRows, builtInAgentRows, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");

  const running = [runningAgent(), runningAgent({ tool: "Codex", cwd: null, area: null, client: null })];
  const agents = [definition(), definition({ name: "release-notes", model: null, tools: null })];
  const spend = [agentSpendRow(), agentSpendRow({ name: "general-purpose", runs: 3, cost: 4.2 })];
  const spendByName = new Map(spend.map((s) => [s.name, s]));

  // Each of the three row renderers the Agents view reuses, checked on its
  // own: none of them ever emits a <button> at all -- there is no End task
  // for an agent (only src/inventory.ts's MCP server rows have one), and no
  // start/stop/edit control was ever built for these rows either.
  for (const html of [renderAgents(running), agentRows(agents, spendByName, NOW_MS), builtInAgentRows(agents, spend, NOW_MS)]) {
    assert.doesNotMatch(html, /<button/, `an agent row renderer emitted a <button>: ${html}`);
  }
});

test("names_are_escaped_in_the_agents_view: a hostile agent name renders as text, never as markup", async () => {
  const { renderAgentsView, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");

  const hostile = "<img src=x onerror=alert(1)>";
  const agents = [definition({ name: hostile })];
  const spend = [agentSpendRow({ name: hostile })];
  const inv = inventory({ agents });

  const html = renderAgentsView(inv, [], "", spend, 0, NOW_MS);

  assert.ok(!html.includes(hostile), "the raw, unescaped hostile name appears in the rendered view");
  assert.ok(html.includes("&lt;img src=x onerror=alert(1)&gt;"), "the escaped form of the hostile name is missing");
});

test("inventory_no_longer_lists_agents_but_offers_the_door: Setup carries no agent rows, the door row shows even at zero", async () => {
  const { renderSetup, renderAgentsDoor, setActiveLocale, openSections } = await loadAgentsModule();
  setActiveLocale("en");
  // Setup ships collapsed (its id is absent from openSections' default set,
  // same as scripts/layout-check.mjs has to account for) -- section()
  // renders no body at all for a closed section, so without opening it here
  // this test would trivially "pass" no matter what renderSetup() puts in
  // that body, agent rows included.
  openSections.add("setup");

  const secretName = "totally-secret-agent-name";
  const agents = [definition({ name: secretName })];
  const inv = inventory({ agents });

  const setupHtml = renderSetup(inv, []);
  assert.ok(!setupHtml.includes(secretName), "an agent's name leaked into Setup's own rendered HTML");
  assert.ok(!setupHtml.toLowerCase().includes("agent"), "Setup's rendered HTML still mentions agents somewhere");

  // The door row shows even for a machine with zero agents, zero running and
  // zero 30-day cost -- it is the door to the Agents view's own empty state,
  // not something that only appears once there is something to report.
  const doorHtml = renderAgentsDoor(inventory(), 0, 0);
  assert.ok(doorHtml.includes("0 agents"), "the door row does not show a real number at zero");
  assert.ok(doorHtml.includes('id="agents-door-btn"'), "the door row's button id moved or disappeared");
});

test("the_summary_counts_equal_the_rows_shown: the headline numbers match what each section actually renders", async () => {
  const { renderAgentsView, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");

  const running = [runningAgent(), runningAgent({ tool: "Codex", cwd: null, area: null, client: null })];
  const agents = [definition({ name: "deploy-checker" }), definition({ name: "release-notes", model: null, tools: null })];
  const spend = [agentSpendRow({ name: "deploy-checker", cost: 1.5 }), agentSpendRow({ name: "release-notes", cost: 2.5 })];
  const inv = inventory({ agents });

  const html = renderAgentsView(inv, running, "", spend, 0, NOW_MS);

  // The summary line's own numbers, in en, spell out the exact counts.
  assert.ok(html.includes("2 agents"), "summary does not name the 2 defined agents");
  assert.ok(html.includes("2 running"), "summary does not name the 2 running agents");
  assert.ok(html.includes("$4.00"), "summary does not name the combined 30-day cost");

  // And the rows actually shown agree: one .inv-row per running agent in
  // Running now, one per defined agent in Your agents. Both custom agents'
  // names also carry the only two spend rows, so builtInAgentRows()
  // contributes nothing here -- every "inv-row" belongs to one of those two
  // sections, and the total is exactly running.length + agents.length.
  // The exact class value, not the "inv-row-main"/"inv-row-sub"/"inv-row-meta"
  // children that also start with the same prefix -- a plain word-boundary
  // match on `inv-row(` with no following hyphen.
  const rowCount = (html.match(/class="inv-row(?:"| )/g) ?? []).length;
  assert.equal(rowCount, running.length + agents.length, "total agent rows shown do not equal running + defined");
});

test("an_empty_machine_gets_the_teaching_states: each empty state says what would make it appear", async () => {
  const { renderAgentsView, setActiveLocale, t } = await loadAgentsModule();
  setActiveLocale("en");

  const html = renderAgentsView(inventory(), [], "", [], 0, NOW_MS);

  assert.ok(html.includes(esc(t("inventory.empty.agents"))), "no agents defined: teaching text is missing");
  assert.ok(html.includes(esc(t("inventory.empty.runningAgents"))), "none running: teaching text is missing");
  assert.ok(html.includes(esc(t("agents.empty.noSubagentRuns"))), "no subagent runs in 30 days: teaching text is missing");

  // Each is a real sentence about what would change, not just an absence:
  // the "none running" and "no subagent runs" texts both explain the
  // trigger condition in the same sentence.
  assert.match(t("inventory.empty.runningAgents"), /Claude Code|Codex|agent host/);
  assert.match(t("agents.empty.noSubagentRuns"), /next time|runs/);
});
