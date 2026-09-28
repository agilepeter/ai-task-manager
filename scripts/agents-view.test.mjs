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
    .replace('import { money } from "./format";', "")
    .replace('import { plural, t } from "./i18n";', "")
    .replace(
      `import {
  agentRows,
  builtInAgentCount,
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
  // not something that only appears once there is something to report. Its
  // line is the same three labelled facts joined by " · " the Agents view's
  // own stats row shows (see agentStats()/T("stat.*") in src/agents.ts),
  // never the old one-sentence summary this replaced.
  const doorHtml = renderAgentsDoor(inventory(), 0, 0);
  assert.ok(doorHtml.includes("Your agents: 0"), "the door row does not show a real 'Your agents' count at zero");
  assert.ok(doorHtml.includes("Running now: 0"), "the door row does not show a real 'Running now' count at zero");
  assert.ok(doorHtml.includes("Subagent spend, 30 days: $0.00"), "the door row does not show a real spend figure at zero");
  assert.ok(doorHtml.includes('id="agents-door-btn"'), "the door row's button id moved or disappeared");
});

test("the_summary_counts_equal_the_rows_shown: the stats row and every section heading match what is actually rendered", async () => {
  const { renderAgentsView, agentStats, money, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");

  // Three DISTINCT counts on purpose (2 defined, 3 running, 4 spend rows) --
  // a test built on equal numbers everywhere could pass even if two of them
  // were silently swapped (defined shown where running belongs, say). Two
  // of the four spend rows match a custom agent by name ("Your agents"),
  // one is a named built-in and one is unattributed (both "Built-in
  // agents") -- so "Your agents" shows 2 rows, "Built-in agents" shows 2,
  // and nothing is left over uncounted.
  const running = [runningAgent(), runningAgent({ tool: "Codex" }), runningAgent({ tool: "Gemini CLI" })];
  const agents = [definition({ name: "agent-a" }), definition({ name: "agent-b" })];
  // Uneven, sub-cent numbers: summing each row's OWN rounded display
  // ($0.00 x4 = $0.00) would read differently from summing the raw costs
  // first and rounding once ($0.015 -> $0.02... rounds to $0.01, see
  // toFixed(2)) -- proving the header uses the latter, per item 2's brief.
  const spend = [
    agentSpendRow({ name: "agent-a", cost: 0.004 }),
    agentSpendRow({ name: "agent-b", cost: 0.004 }),
    agentSpendRow({ name: "general-purpose", cost: 0.004 }),
    agentSpendRow({ name: "", cost: 0.003 }),
  ];
  const inv = inventory({ agents });

  const stats = agentStats(inv, running, spend);
  assert.deepEqual(stats, { yours: 2, runningNow: 3, spend30: 0.015 }, "agentStats() computed the wrong raw numbers");
  assert.equal(money(stats.spend30), "$0.01", "money() of the raw sum should be $0.01, not the sum of four already-rounded $0.00 rows");

  const html = renderAgentsView(inv, running, "", spend, 0, NOW_MS);

  // The stats row's own three numbers.
  assert.match(html, /ag-stat-n">2<\/span><span class="ag-stat-label">Your agents/, "stats row does not show 2 for Your agents");
  assert.match(html, /ag-stat-n">3<\/span><span class="ag-stat-label">Running now/, "stats row does not show 3 for Running now");
  assert.match(html, /ag-stat-n">\$0\.01<\/span><span class="ag-stat-label">Subagent spend/, "stats row does not show the raw-summed $0.01 spend");

  // Every section heading's own count equals the rows actually shown beneath
  // it -- one shape, per item 6, not three: "Title <count>" in every case.
  assert.match(html, /Running now <span class="plan">3<\/span>/, "Running now heading count does not match its 3 rows");
  assert.match(html, /Your agents <span class="plan">2<\/span>/, "Your agents heading count does not match its 2 rows");
  assert.match(html, /Built-in agents <span class="plan">2<\/span>/, "Built-in agents heading count does not match its 2 rows");

  // And the rows actually shown agree: one .inv-row per running agent in
  // Running now, one per defined agent in Your agents, one per remaining
  // spend row in Built-in agents. The exact class value, not the
  // "inv-row-main"/"inv-row-sub"/"inv-row-meta" children that also start
  // with the same prefix -- a plain word-boundary match on `inv-row(` with
  // no following hyphen.
  const rowCount = (html.match(/class="inv-row(?:"| )/g) ?? []).length;
  assert.equal(rowCount, running.length + agents.length + 2, "total agent rows shown do not equal running + defined + built-ins");
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

// closeFocusTarget() (src/agents.ts, and audit.ts's own identical copy) is
// the pure decision close() makes about where to send focus -- pulled out of
// the DOM-touching close() specifically so it can be exercised here with
// plain placeholder objects standing in for elements, no real DOM required.
// A WebKit mouse click never focuses the button it clicked (activeElement
// stays <body>), which is why this can no longer just restore "whatever had
// focus before": openAgents() now takes the actual opener element, and this
// is what decides whether it is still good to use when the panel closes.
test("closeFocusTarget: returns to the opener when it is still in the document, else the fallback, else nothing", async () => {
  const { closeFocusTarget } = await loadAgentsModule();
  const opener = { tag: "opener" };
  const fallback = { tag: "fallback" };
  assert.equal(closeFocusTarget(opener, true, fallback), opener, "a live opener should win over the fallback");
  assert.equal(closeFocusTarget(opener, false, fallback), fallback, "a detached opener should fall back");
  assert.equal(closeFocusTarget(null, false, fallback), fallback, "no opener at all should fall back");
  assert.equal(closeFocusTarget(null, false, null), null, "no opener and no fallback should return null, not throw");
  // A stale reference that HAPPENS to still be attached (e.g. the Audit's own
  // "Open" button, which is not removed from the document when the Audit
  // panel just slides off-screen) is exactly the shape openAgents(null) is
  // meant to avoid ever reaching this function with -- but if it somehow did,
  // "still in the document" is the only signal this pure function has, so it
  // would return it. That risk is why main.ts's goTo() passes no opener at
  // all for that path (see openAgents()'s own comment), not something this
  // function can guard against on its own.
});

// The failing-guardrail line (src/agents.ts's renderAgentsView(), the
// `failing` parameter) is the one place this view still talks about the
// Audit's own agent guardrail checks -- present with the right count when at
// least one has failed, entirely absent when none have.
test("the failing-guardrail line: present with the right count when checks fail, absent when none do", async () => {
  const { renderAgentsView, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");
  const inv = inventory();

  const clean = renderAgentsView(inv, [], "", [], 0, NOW_MS);
  assert.ok(!clean.includes("agents-open-audit"), "no failing guardrail checks should mean no Open Audit button at all");
  assert.ok(!clean.includes("guardrail"), "no failing guardrail checks should mean no guardrail line at all");

  const oneFailing = renderAgentsView(inv, [], "", [], 1, NOW_MS);
  assert.match(oneFailing, /1 agent guardrail check needs attention/, "one failing check should be named in the singular, with its count");
  assert.ok(oneFailing.includes('id="agents-open-audit"'), "a failing check should carry the Open Audit button");

  const threeFailing = renderAgentsView(inv, [], "", [], 3, NOW_MS);
  assert.match(threeFailing, /3 agent guardrail checks need attention/, "three failing checks should be named in the plural, with their count");
});

// The button's own id has to agree between where renderAgentsView() paints it
// and where setupAgents()'s click handler looks for it -- a plain source-text
// check, not a click simulation (this suite has no real DOM to click
// against), but one that fails the moment either side is renamed without the
// other, which is the actual risk "clicking it opens the Audit" protects
// against.
test("the Open Audit button's id agrees between renderAgentsView() and setupAgents()'s click handler", async () => {
  const source = await readFile(new URL("../src/agents.ts", import.meta.url), "utf8");
  const rendered = source.match(/id="(agents-open-audit)"/);
  const handled = source.match(/target\.closest\("(#agents-open-audit)"\)/);
  assert.ok(rendered, "renderAgentsView() no longer renders a button with id=\"agents-open-audit\"");
  assert.ok(handled, "setupAgents()'s click handler no longer looks for #agents-open-audit");
  assert.equal(`#${rendered[1]}`, handled[1], "the rendered id and the handled id have drifted apart");
  // And the click handler really does close this view and hand off to the
  // Audit, not just detect the click.
  assert.match(source, /target\.closest\("#agents-open-audit"\)\)\s*\{\s*close\(\);\s*host\?\.openAudit\(\);/, "the Open Audit click handler no longer closes this view and calls host.openAudit()");
});

// esc() (src/agents.ts) is this view's own private copy, same convention as
// every other view in this app -- exercised here on the two places this
// module interpolates a string it did not itself construct: the load error
// (renderLoadError(), pulled out of render() specifically so this is
// testable without a DOM) and the running-agents scan error (already a
// renderAgentsView() parameter).
test("esc() escapes a hostile load-error and a hostile running-agents error, never rendering them as markup", async () => {
  const { renderAgentsView, renderLoadError, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");
  const hostile = "<img src=x onerror=alert(1)>";
  const escaped = "&lt;img src=x onerror=alert(1)&gt;";

  const loadErrorHtml = renderLoadError(hostile);
  assert.ok(!loadErrorHtml.includes(hostile), "the raw load-error string leaked into the rendered page");
  assert.ok(loadErrorHtml.includes(escaped), "the escaped load-error string is missing");

  const runningErrorHtml = renderAgentsView(inventory(), [], hostile, [], 0, NOW_MS);
  assert.ok(!runningErrorHtml.includes(hostile), "the raw running-agents error string leaked into the rendered page");
  assert.ok(runningErrorHtml.includes(escaped), "the escaped running-agents error string is missing");
});

// The one line under an agent's name (its 30-day spend) must wrap instead of
// truncating with an ellipsis -- src/styles.css's `.inv-sub.inv-sub-wrap`
// rule is what does that, so this checks the class actually lands on both
// row shapes that carry a spend line: a custom agent's own row (agentRows())
// and a built-in's row (builtInAgentRows()). A plain MCP-server `.inv-sub`
// elsewhere in the app is deliberately NOT part of this class, so this only
// asserts the class is present here, not that every `.inv-sub` carries it.
test("an agent's spend line carries inv-sub-wrap, so it wraps instead of ending in an ellipsis", async () => {
  const { agentRows, builtInAgentRows, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");
  const d = definition({ name: "deploy-checker" });
  const spendMap = new Map([["deploy-checker", agentSpendRow({ name: "deploy-checker" })]]);
  const yours = agentRows([d], spendMap, NOW_MS);
  assert.match(yours, /class="inv-sub inv-sub-wrap"/, "a custom agent's row does not wrap its spend line");

  const builtIn = builtInAgentRows([], [agentSpendRow({ name: "general-purpose" })], NOW_MS);
  assert.match(builtIn, /class="inv-sub inv-sub-wrap"/, "a built-in agent's row does not wrap its spend line");
});
