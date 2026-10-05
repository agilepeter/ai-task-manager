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
//
// What these tests do NOT cover: the DOM wiring in src/main.ts. That is
// paintAgentLive() (disabling the two Settings selects, setting their value,
// inserting and removing the hint and error lines), the `change` listener in
// setupAgentLive() (reading the select, restoring its focus after a save,
// writing and clearing the footer line), and the call to paintAgentLive() from
// applyLocale(), which is only checked as source text. The decisions behind
// them (what a change sends, what the select must show afterwards) live in
// changeLiveRule() in src/agents.ts and are tested here; the wiring itself is
// checked by pressing keys in WebKit. The listeners setupAgents() registers on
// #agents-body are likewise not run; what they call is.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

let cachedCode = null;
let cachedModule = null;
let freshCount = 0;
async function loadAgentsModule() {
  cachedCode ??= buildAgentsCode();
  if (!cachedModule) cachedModule = importCode(await cachedCode);
  return cachedModule;
}

/// A private copy of the module with none of the state (the loaded watch, the
/// draft, a save in flight) another test left behind. The trailing comment
/// makes the data: URL, and so the module instance, unique.
async function freshAgentsModule() {
  cachedCode ??= buildAgentsCode();
  freshCount += 1;
  return importCode(`${await cachedCode}\n//${freshCount}`);
}

function importCode(code) {
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

async function buildAgentsCode() {
  const i18nSource = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const localesDir = new URL("../src/locales/", import.meta.url);
  const inlinedI18n = await inlineLocaleImports(i18nSource, localesDir);

  const formatSource = await readFile(new URL("../src/format.ts", import.meta.url), "utf8");
  const strippedFormat = formatSource.replace('import { localeTag, plural, t } from "./i18n";', "");
  if (strippedFormat === formatSource) throw new Error("no substitution matched -- src/format.ts's source shape moved under this test");

  // src/focus.ts has no imports of its own, so it is appended as is -- both
  // inventory.ts's and agents.ts's own `import { focusOrFallback } from
  // "./focus";` are dropped below, since the one copy here already puts it
  // in scope for both.
  const focusSource = await readFile(new URL("../src/focus.ts", import.meta.url), "utf8");

  // src/panels.ts has no imports of its own either -- agents.ts's own
  // `import { focusAfterClose, isTopPanel, syncPanels } from "./panels";` is
  // dropped below for the same reason, and the real module (not a stub) is
  // used: it is what now applies `inert` on openAgents()/close().
  const panelsSource = await readFile(new URL("../src/panels.ts", import.meta.url), "utf8");

  const inventorySource = await readFile(new URL("../src/inventory.ts", import.meta.url), "utf8");
  const strippedInventory = inventorySource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { showLedger } from "./ledger";', "")
    .replace('import { focusOrFallback } from "./focus";', "")
    .replace('import { localeTag, plural, t, tm, type Msg } from "./i18n";', "")
    .replace('import { byteSize, money, relativeDay, tokens } from "./format";', "")
    // A whole-word rename, not just a declaration move: every bare `render()`
    // call site AND every bare `render` callback reference (inventory.ts's
    // own `.then(render)`, with no trailing "()" of its own) would otherwise
    // still resolve to i18n's own exported `render(locale, msg)` once the two
    // sources are concatenated, since that is now the only "render" binding
    // left standing. Harmless for the pure-function tests below (none of them
    // ever reach a call site), but fatal the moment a test actually calls an
    // exported function that runs load() end to end (see applyRescan()'s own
    // behavioural test further down) -- `render(locale, msg)` called with too
    // few arguments throws on `msg.vars`. `\brender\b` catches the
    // declaration, every `render()` call and every bare `.then(render)`
    // reference in one pass; it can never touch `renderAgents`/
    // `renderAgentsDoor`/etc (no word boundary before their own trailing
    // text) nor `rerender` (no word boundary before "render" inside it).
    .replace(/\brender\b/g, "__unusedInventoryRender");
  if (strippedInventory === inventorySource) throw new Error("no substitution matched -- src/inventory.ts's source shape moved under this test");

  const agentsSource = await readFile(new URL("../src/agents.ts", import.meta.url), "utf8");
  const strippedAgents = agentsSource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { money, wholeMoney } from "./format";', "")
    .replace('import { focusOrFallback } from "./focus";', "")
    .replace('import { focusAfterClose, isTopPanel, syncPanels } from "./panels";', "")
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
  type RescanResult,
  type RunningAgent,
} from "./inventory";`,
      "",
    )
    // Same whole-word fix as inventory.ts's own rename above, for the same
    // reason (agents.ts has no bare `.then(render)` reference today, but the
    // broader pattern costs nothing and stays correct if one is ever added).
    .replace(/\brender\b/g, "__unusedAgentsRender")
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

  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${focusSource}\n${panelsSource}\n${strippedInventory}\n${strippedAgents}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext },
  }).outputText;
  return code;
}

// A minimal stand-in for `document`, just capable enough for openAgents() /
// applyRescan() / reloadAgents() / render() to run end to end: a body
// classList real code can toggle "agents-open" on (isOpen() reads it back the
// same way), and querySelector() handing back one persistent fake element per
// selector (so writing #agents-body's innerHTML in one call and reading it
// back in a later one sees the same object) rather than a fresh, disconnected
// one every time. Every fake element carries the handful of members this
// module's code actually touches on one: `.innerHTML`, `.focus()`,
// `.classList`, `.addEventListener()`, and `.setAttribute()`/
// `.removeAttribute()` as no-ops (openAgents()/close() toggle the panel's
// `inert` attribute alongside the body class -- see src/agents.ts's own
// comment) -- new members are added here only when some code path is
// actually found to need them, never speculatively.
function makeFakeDocument() {
  const bodyClasses = new Set();
  const elements = new Map();
  function elementFor(selector) {
    if (!elements.has(selector)) {
      elements.set(selector, {
        innerHTML: "",
        focus() {},
        addEventListener() {},
        setAttribute() {},
        removeAttribute() {},
        classList: { contains: () => false, add() {}, remove() {} },
      });
    }
    return elements.get(selector);
  }
  return {
    elements,
    body: {
      // close()'s own fallback (`?? document.body`, see index.html's comment
      // on body's tabindex="-1") calls .focus() on this when nothing else
      // panned out -- a no-op here is enough, since these tests never open
      // this module's private close() with no real opener AND no fallback.
      focus() {},
      classList: {
        contains: (c) => bodyClasses.has(c),
        add: (c) => bodyClasses.add(c),
        remove: (c) => bodyClasses.delete(c),
      },
    },
    querySelector: (selector) => elementFor(selector),
    querySelectorAll: () => [],
    contains: () => false,
    addEventListener: () => {},
  };
}

// A recording `invoke()` stand-in: every call is pushed to `calls` (command
// name only -- these tests never need to inspect args) before it resolves
// (or rejects) from `fixtures`, so a test can assert both WHAT was called and
// in WHAT ORDER. `delays` lets one specific command's promise settle after an
// extra microtask/timer tick, for proving something else really did wait for
// it rather than merely happening to run after it once.
function makeRecordingInvoke(fixtures, delays = {}) {
  const calls = [];
  const invoke = async (cmd, args) => {
    calls.push(cmd);
    if (delays[cmd]) await delays[cmd]();
    if (!(cmd in fixtures)) throw new Error(`makeRecordingInvoke: no fixture registered for "${cmd}"`);
    const v = fixtures[cmd];
    if (v instanceof Error) throw v;
    return typeof v === "function" ? v(args) : v;
  };
  return { calls, invoke };
}

// Drains the microtask queue completely (every pending .then()/await, however
// many hops deep), unlike a fixed chain of `.then().then()…` which only
// covers as many hops as it happens to name. A setTimeout callback is only
// ever run once nothing microtask-queued is left, so this is the simplest
// reliable "let everything that has already started actually finish" for a
// test with no real Tauri round-trip to await.
function flushMicrotasks() {
  return new Promise((resolve) => setTimeout(resolve, 0));
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

const NO_DRAFT = { agent: "", amount: "" };

/** A budgets view as the backend answers: `rows` are [agent, monthToDate,
 *  monthlyBudget] in saved order, `watch` is derived from them. */
function watchView({ rows = [["general-purpose", 14.74, 10], ["deploy-checker", 1.69, 5]], live = { hourlyPaceUsd: null, maxMinutes: null }, known, liveHint = null } = {}) {
  return {
    watch: { budgets: rows.map(([agent, , monthlyBudget]) => ({ agent, monthlyBudget })), live },
    budgets: rows.map(([agent, monthToDate, monthlyBudget]) => ({ agent, monthToDate, monthlyBudget })),
    runaways: [],
    liveHint,
    known: known ?? ["Explore", "Plan", "deploy-checker", "general-purpose"],
  };
}

/** The `<option>` values of the add row's picker, in order. */
function pickerValues(html) {
  const select = html.match(/<select id="agent-budget-pick"[\s\S]*?<\/select>/);
  return select ? [...select[0].matchAll(/<option value="([^"]*)"/g)].map((m) => m[1]) : null;
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

  const html = renderAgentsView(inv, running, "", spend, "", 0, NOW_MS);

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

  const html = renderAgentsView(inv, [], "", spend, "", 0, NOW_MS);

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
  const doorHtml = renderAgentsDoor(inventory(), 0, 0, "");
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

  const html = renderAgentsView(inv, running, "", spend, "", 0, NOW_MS);

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

  const html = renderAgentsView(inventory(), [], "", [], "", 0, NOW_MS);

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

  const clean = renderAgentsView(inv, [], "", [], "", 0, NOW_MS);
  assert.ok(!clean.includes("agents-open-audit"), "no failing guardrail checks should mean no Open Audit button at all");
  assert.ok(!clean.includes("guardrail"), "no failing guardrail checks should mean no guardrail line at all");

  const oneFailing = renderAgentsView(inv, [], "", [], "", 1, NOW_MS);
  assert.match(oneFailing, /1 agent guardrail check needs attention/, "one failing check should be named in the singular, with its count");
  assert.ok(oneFailing.includes('id="agents-open-audit"'), "a failing check should carry the Open Audit button");

  const threeFailing = renderAgentsView(inv, [], "", [], "", 3, NOW_MS);
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

  const runningErrorHtml = renderAgentsView(inventory(), [], hostile, [], "", 0, NOW_MS);
  assert.ok(!runningErrorHtml.includes(hostile), "the raw running-agents error string leaked into the rendered page");
  assert.ok(runningErrorHtml.includes(escaped), "the escaped running-agents error string is missing");
});

// A failed get_agent_spend used to collapse to an empty array with no error
// at all, which read as "$0.00" and "Never run" -- both confident, specific
// and wrong. spendError now carries the failure through to every place spend
// would otherwise show: the stats row's own figure, "Your agents"'s spend
// sub-line, the built-in section, and src/inventory.ts's door row.
test("a failed agent-spend read never shows $0.00 or 'Never run': the stats figure, Your agents, the built-in section and the door row all name the error instead", async () => {
  const { renderAgentsView, renderAgentsDoor, agentRows, setActiveLocale, t } = await loadAgentsModule();
  setActiveLocale("en");
  const hostile = "<img src=x onerror=alert(1)>";
  const escaped = "&lt;img src=x onerror=alert(1)&gt;";

  const agents = [definition({ name: "deploy-checker" })];
  const inv = inventory({ agents });
  const html = renderAgentsView(inv, [], "", [], hostile, 0, NOW_MS);

  // The stats row: "?" (this app's own convention for a figure it could not
  // read -- src/inventory.ts's trustChip() falls back to the same glyph),
  // never a confident, specific "$0.00".
  assert.match(html, /ag-stat-n">\?<\/span><span class="ag-stat-label">Subagent spend/, "the stats row must show ? for spend, not a number, once the read failed");
  assert.ok(!html.includes("$0.00"), "no $0.00 must appear anywhere in the view once the spend read failed");

  // Your agents: agentRows(..., null, ...) drops the spend sub-line entirely
  // -- never "Never run", a claim about the agent that is not what actually
  // happened (this view simply could not check).
  assert.ok(!html.includes(t("inventory.agents.neverRun")), "a spend-read failure must never render as 'Never run'");
  assert.ok(!/inv-sub inv-sub-wrap/.test(agentRows(agents, null, NOW_MS)), "agentRows(list, null, now) must render no spend sub-line at all");

  // Built-in agents: the error itself, escaped, replacing both the rows and
  // the usual "no runs in 30 days" empty state -- a read failure is not the
  // same fact as a genuinely quiet 30 days.
  assert.ok(!html.includes(hostile), "the raw, unescaped spend-read error must never reach the page");
  assert.ok(html.includes(escaped), "the escaped spend-read error is missing from the built-in section");
  assert.ok(!html.includes(t("agents.empty.noSubagentRuns")), "a read failure must not be shown as if 30 days were genuinely quiet");

  // The Inventory door row (src/inventory.ts) shows the same placeholder.
  const doorHtml = renderAgentsDoor(inv, 0, 0, hostile);
  assert.ok(doorHtml.includes("?"), "the door row must show ? for spend once the read failed");
  assert.ok(!doorHtml.includes("$0.00"), "the door row must not show $0.00 once the read failed");
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

// The failing-guardrail sentence and its Open Audit button used to share one
// <p>, which wrapped a different way in every language (beside the text in
// English, under it and indented in German and Russian, crowded against the
// last word in Portuguese). They are now two siblings in one wrapper: the
// sentence its own <p>, the button its own element after it -- never a
// button inside a paragraph -- so the button always starts its own line
// regardless of how long the sentence wraps.
test("the failing-guardrail line is a block, not a button inside a sentence", async () => {
  const { renderAgentsView, setActiveLocale } = await loadAgentsModule();
  setActiveLocale("en");
  const oneFailing = renderAgentsView(inventory(), [], "", [], "", 1, NOW_MS);
  assert.match(
    oneFailing,
    /<div class="ag-guardrail"><p class="dt-caption">[^<]*<\/p><button class="inv-learn" id="agents-open-audit">/,
    "the sentence must close its own <p> before the Open Audit button starts -- no button inside the paragraph",
  );
});

// shouldReload() (src/agents.ts) is the pure decision reloadAgents() makes on
// every "popover-shown" -- reopening the popover happens far more often than
// this view's data actually changes, so a fresh load should not be re-fetched
// just because the popover closed and reopened a second later. Exercised
// directly, with no DOM, exactly the way this suite's other pure functions
// are (closeFocusTarget() above). Extended with `loading` and `lastFailed`
// (item 2's fix): a load in flight must never be joined by a second one, and
// a failed load must retry the moment the popover is shown again rather than
// sitting inside the 60s freshness window a failure used to leave behind.
test("shouldReload(): closed never reloads, open+fresh does not, open+stale does, a backwards clock reloads, loading blocks a second load, a failure retries at once", async () => {
  const { shouldReload, RELOAD_FRESHNESS_MS } = await loadAgentsModule();
  const now = 1_790_000_000_000;

  assert.equal(shouldReload(now - 1, now, false, false, false), false, "a closed view must never reload, no matter how stale");
  assert.equal(shouldReload(now - 1, now, false, false, true), false, "closed must never reload even with a failed last load");
  assert.equal(shouldReload(now, now, true, false, false), false, "just succeeded (age 0) must not reload");
  assert.equal(shouldReload(now - 10_000, now, true, false, false), false, "succeeded 10s ago must not reload");
  assert.equal(shouldReload(now - (RELOAD_FRESHNESS_MS - 1), now, true, false, false), false, "one millisecond inside the freshness window must not reload");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS, now, true, false, false), true, "exactly at the freshness window must reload");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS - 1, now, true, false, false), true, "past the freshness window must reload");
  assert.equal(shouldReload(now - 61_000, now, true, false, false), true, "succeeded 61s ago must reload");
  assert.equal(shouldReload(now + 1, now, true, false, false), true, "a clock that moved backwards must reload rather than trust the (negative) age");
  assert.equal(shouldReload(now, now, true, true, false), false, "a load already in flight must never be joined by a second one, even at age 0 (irrelevant) or beyond the window");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS - 1, now, true, true, false), false, "loading must block a reload even when the last success is stale");
  assert.equal(shouldReload(now, now, true, false, true), true, "a failed load must retry at once, shown again a moment later, even though its own timestamp reads as fresh");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS - 1, now, true, false, true), true, "a failed, stale load must also reload (failure wins either way)");
});

// applyRescan() (src/agents.ts) exists so a rescan hands this view the data
// src/inventory.ts's load() already fetched, instead of this view
// re-invoking get_inventory / get_running_agents / get_agent_spend a second
// time for the same event -- the actual bug this fixes (a view left open
// used to cause a full second scan on every rescan). Exercised for real this
// time: openAgents() first (its own loadData() makes the view's normal four
// calls), then applyRescan() with fresh data, checking that the ONLY new
// invoke() call it causes is get_audit (via loadFailingGuardrails() -- a
// rescan's own data carries nothing about the Audit's checks, so that one
// call cannot be avoided), and that the panel actually redraws from the
// argument it was handed rather than from a second scan.
test("applyRescan() re-invokes only get_audit, never get_inventory/get_running_agents/get_agent_spend", async () => {
  const { openAgents, applyRescan } = await loadAgentsModule();
  const fakeDocument = makeFakeDocument();
  globalThis.document = fakeDocument;

  const initialInv = inventory({ agents: [definition({ name: "initial-agent" })] });
  const { calls, invoke } = makeRecordingInvoke({
    get_inventory: initialInv,
    get_running_agents: [],
    get_agent_spend: [],
    get_audit: { sections: [] },
    get_agent_watch: watchView(),
  });
  globalThis.invoke = invoke;

  try {
    openAgents();
    // loadData()'s own invoke() calls (the budgets' get_agent_watch among
    // them), plus loadFailingGuardrails()'s own get_audit, are all
    // fire-and-forget promises openAgents() never awaits -- let their
    // microtasks drain before moving on.
    await flushMicrotasks();
    assert.deepEqual(
      [...calls].sort(),
      ["get_agent_spend", "get_agent_watch", "get_audit", "get_inventory", "get_running_agents"].sort(),
      "openAgents()'s own initial load did not make the five calls this test's baseline assumes",
    );
    calls.length = 0; // only calls made by applyRescan() itself matter from here

    const rescannedInv = inventory({ agents: [definition({ name: "rescanned-agent" })] });
    applyRescan({
      inventory: rescannedInv,
      loadError: "",
      runningAgents: [],
      runningAgentsError: "",
      agentSpend: [],
      agentSpendError: "",
    });
    await flushMicrotasks();

    assert.deepEqual(calls, ["get_audit"], "applyRescan() must cause exactly one new invoke() call, get_audit, and nothing else");
    const body = fakeDocument.elements.get("#agents-body");
    assert.ok(body.innerHTML.includes("rescanned-agent"), "the panel must redraw from applyRescan()'s own argument, not from a fresh scan");
    assert.ok(!body.innerHTML.includes("initial-agent"), "the panel must not still show the pre-rescan data");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

// render() (src/agents.ts) must not write into #agents-body once the panel
// has closed -- a load that was in flight when the panel closed must not
// still paint over it when it lands late. Exercised as the real race:
// openAgents() fires get_inventory but its promise is held back a tick; the
// panel is closed before it settles; once it does settle, #agents-body must
// still read whatever it held at close time, never the freshly loaded data.
test("render() bails out before touching the DOM when the panel is closed", async () => {
  const { openAgents } = await loadAgentsModule();
  const fakeDocument = makeFakeDocument();
  globalThis.document = fakeDocument;

  let releaseInventory;
  const held = new Promise((resolve) => { releaseInventory = resolve; });
  const { invoke } = makeRecordingInvoke(
    {
      get_inventory: inventory({ agents: [definition({ name: "late-agent" })] }),
      get_running_agents: [],
      get_agent_spend: [],
      get_audit: { sections: [] },
    },
    { get_inventory: () => held },
  );
  globalThis.invoke = invoke;

  try {
    openAgents();
    // The panel is open and loading, but get_inventory has not settled yet --
    // #agents-body must still show the loading placeholder, not late-agent.
    const bodyWhileLoading = fakeDocument.elements.get("#agents-body").innerHTML;
    assert.ok(!bodyWhileLoading.includes("late-agent"), "get_inventory resolved before this test released it");

    fakeDocument.body.classList.remove("agents-open"); // the user closed the panel
    releaseInventory();
    await flushMicrotasks();

    const bodyAfterClose = fakeDocument.elements.get("#agents-body").innerHTML;
    assert.equal(bodyAfterClose, bodyWhileLoading, "render() must not overwrite #agents-body once the panel has closed, even for a load that was already in flight");
    assert.ok(!bodyAfterClose.includes("late-agent"), "a load that settled after close must never reach the DOM");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

// ---------------------------------------------------------------------------
// The Budgets section: src/agents.ts's renderBudgetsSection() is pure (the
// last view, the load error, the add row's draft in; HTML out), and the
// load/save functions behind it keep one shared copy of what the backend last
// said. The module under test is one instance for the whole file, so every
// behavioural test below starts by loading the state it needs.
// ---------------------------------------------------------------------------

/** An invoke for a panel that is open: the view's own loads answer with
 *  fixtures, `state.view` is what get_agent_watch returns, and set_agent_watch
 *  is `save(args)` (by default it echoes the sent budgets back). */
function panelInvoke(state, save) {
  return async (cmd, args) => {
    if (cmd === "get_inventory") return inventory();
    if (cmd === "get_agent_watch") {
      if (state.loadFails) throw "could not read";
      return state.view;
    }
    if (cmd === "set_agent_watch") {
      if (save) return save(args);
      return watchView({ rows: args.watch.budgets.map((b) => [b.agent, 0, b.monthlyBudget]), live: args.watch.live, known: state.view.known });
    }
    if (cmd === "get_audit") return { sections: [] };
    return [];
  };
}

/** One recording invoke whose set_agent_watch echoes back a view built from
 *  what it was sent, the way the real command does. */
function watchInvoke(initial, extra = {}) {
  const calls = [];
  const sent = [];
  const invoke = async (cmd, args) => {
    calls.push(cmd);
    if (cmd === "get_agent_watch") return initial;
    if (cmd === "set_agent_watch") {
      sent.push(args.watch);
      return watchView({
        rows: args.watch.budgets.map((b) => [b.agent, 0, b.monthlyBudget]),
        live: args.watch.live,
        known: initial.known,
      });
    }
    if (cmd in extra) return extra[cmd];
    throw new Error(`no fixture for ${cmd}`);
  };
  return { calls, sent, invoke };
}

test("a budget with no spend still shows", async () => {
  const { renderBudgetsSection, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  // "gone-agent" is not in `known` at all: its definition was deleted, but its
  // budget is still saved, so the row must still be there to be removed.
  const view = watchView({ rows: [["Plan", 0, 20], ["gone-agent", 0, 7]] });
  const html = renderBudgetsSection(view, "", NO_DRAFT);

  assert.equal((html.match(/class="inv-row ag-budget-row"/g) ?? []).length, 2, "a budget with no spend must still get its row");
  assert.ok(html.includes("No spend this month, against a budget of $20"), "a zero-spend row must say there is no spend, with the budget");
  assert.ok(html.includes("No spend this month, against a budget of $7"), "a budget on an agent that no longer exists must still show");
  assert.ok(html.includes('data-budget-remove="gone-agent"'), "every row has its own Remove button");
  assert.match(html, /aria-label="Remove the budget for gone-agent"/, "Remove's accessible name must include the agent's name");
  assert.ok(!html.includes("This month:"), "a zero-spend row must not claim a figure for the month");
});

test("an over-budget row is marked and an under-budget row is not", async () => {
  const { renderBudgetsSection, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  // Over, exactly at (the client budget line counts "at" as over), and under.
  const view = watchView({ rows: [["over-agent", 14.74, 10], ["at-agent", 10, 10], ["under-agent", 1.69, 5]] });
  const html = renderBudgetsSection(view, "", NO_DRAFT);
  const rows = html.split('<div class="inv-row ag-budget-row">').slice(1);
  assert.equal(rows.length, 3);
  const marked = (agent) => rows.find((r) => r.includes(`>${agent}</span>`)).includes("dt-forecast-hit");

  assert.equal(marked("over-agent"), true, "a row past its budget must carry the budget-hit emphasis");
  assert.equal(marked("at-agent"), true, "a row exactly at its budget counts as over, like the client budget line");
  assert.equal(marked("under-agent"), false, "a row under its budget must not be marked");
  assert.ok(html.includes("This month: $15 of $10"), "the figure for the month must read as this month's spend of the budget");
  assert.ok(html.includes("This month: $1.69 of $5"));
});

test("a whole-dollar budget shows without cents and a fractional one keeps them", async () => {
  const { renderBudgetsSection, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const view = watchView({ rows: [["a", 0.85, 25], ["b", 1.69, 5], ["c", 0, 5], ["d", 3, 2.5], ["e", 20, 1500]] });
  const html = renderBudgetsSection(view, "", NO_DRAFT);
  assert.ok(html.includes("This month: $0.85 of $25<"), "a whole budget of $25 reads $25");
  assert.ok(html.includes("This month: $1.69 of $5<"), "a whole budget of $5 reads $5, not $5.00");
  assert.ok(html.includes("No spend this month, against a budget of $5<"), "the no-spend line reads the same way");
  assert.ok(html.includes("This month: $3.00 of $2.50<"), "a budget that is not whole goes through money()");
  assert.ok(html.includes("of $1,500<"), "larger budgets keep money()'s grouping");
  assert.ok(!html.includes("$5.00"));
});

test("the picker offers only known names without a budget", async () => {
  const { renderBudgetsSection, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const view = watchView({ rows: [["Plan", 1, 5]], known: ["Explore", "Plan", "general-purpose"] });
  const html = renderBudgetsSection(view, "", NO_DRAFT);

  assert.deepEqual(pickerValues(html), ["", "Explore", "general-purpose"], "a name that already has a budget must not be offered again");
  assert.match(html, /<option value="" disabled selected>Choose an agent<\/option>/, "the first option is a disabled placeholder");
  assert.match(html, /<input id="agent-budget-amount" type="text" inputmode="numeric"/, "the amount is a text field with a numeric keyboard, so its caret can be restored");
  assert.ok(html.includes('id="agent-budget-add"'));

  // The draft survives a redraw: the picked name and the typed amount come back.
  const redrawn = renderBudgetsSection(view, "", { agent: "Explore", amount: "25" });
  assert.match(redrawn, /<option value="Explore" selected>/);
  assert.match(redrawn, /value="25"/);

  // Nothing left to pick: a line replaces the add row.
  const all = renderBudgetsSection(watchView({ rows: [["Plan", 1, 5]], known: ["Plan"] }), "", NO_DRAFT);
  assert.equal(pickerValues(all), null);
  assert.ok(all.includes("Every agent the app knows already has a budget."));
  assert.ok(!all.includes("agent-budget-add"));

  // At the cap of 50 budgets, another line replaces it, whatever is still known.
  const rows = Array.from({ length: 50 }, (_, i) => [`agent-${i}`, 0, 5]);
  const full = renderBudgetsSection(watchView({ rows, known: [...rows.map((r) => r[0]), "one-more"] }), "", NO_DRAFT);
  assert.equal(pickerValues(full), null);
  assert.ok(full.includes("50 budgets is the most the app keeps."));
});

test("budget controls are absent until the watch has loaded", async () => {
  const { renderBudgetsSection, loadAgentWatch, saveAgentWatch, saveAgentLive, addBudget, removeBudget, noteBudgetDraft, agentWatchState, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const hostile = "<img src=x onerror=alert(1)>";

  for (const [error, expected] of [["", "Loading"], [hostile, "&lt;img src=x onerror=alert(1)&gt;"]]) {
    const html = renderBudgetsSection(null, error, NO_DRAFT);
    assert.doesNotMatch(html, /<select|<input|<button/, "no control may exist before the watch has loaded");
    assert.ok(html.includes(expected), `the section must say ${expected}`);
    assert.ok(!html.includes(hostile));
    assert.doesNotMatch(html, /<span class="plan">/, "no count is known yet");
  }

  // The state behind it: a first load that failed leaves no view, so Add,
  // Remove and the Settings selects send nothing.
  globalThis.document = makeFakeDocument();
  const calls = [];
  globalThis.invoke = async (cmd) => {
    calls.push(cmd);
    throw "the file is locked";
  };
  try {
    await loadAgentWatch();
    assert.equal(agentWatchState().view, null);
    assert.equal(agentWatchState().error, "the file is locked");

    calls.length = 0;
    noteBudgetDraft("agent", "Plan");
    noteBudgetDraft("amount", "10");
    await addBudget(true);
    await removeBudget("Plan", true);
    assert.deepEqual(await saveAgentLive({ hourlyPaceUsd: 5 }), { outcome: "notLoaded" });
    assert.deepEqual(await saveAgentWatch(() => ({ budgets: [], live: { hourlyPaceUsd: null, maxMinutes: null } })), { outcome: "notLoaded" });
    assert.deepEqual(calls, [], "nothing may be sent before a load has answered");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a failed reload keeps the last good view and shows the error", async () => {
  const { renderBudgetsSection, loadAgentWatch, saveAgentLive, agentWatchState, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  const state = { view: watchView(), loadFails: false };
  const sent = [];
  globalThis.invoke = panelInvoke(state, (args) => { sent.push(args.watch); return state.view; });
  try {
    await loadAgentWatch();
    state.loadFails = true;
    await loadAgentWatch();
    const now = agentWatchState();
    assert.ok(now.view, "one transient error must not take away a view that was good");
    assert.equal(now.error, "could not read");

    const html = renderBudgetsSection(now.view, now.error, NO_DRAFT);
    assert.match(html, /<select id="agent-budget-pick"/, "the controls stay");
    assert.ok(html.includes("Could not read the budgets: could not read"), "the error shows beside them");
    assert.deepEqual(await saveAgentLive({ maxMinutes: 60 }), { outcome: "saved" }, "a save from that view is still one built from the backend's last answer");
    assert.equal(sent.length, 1);
    assert.equal(agentWatchState().error, "", "a successful save clears the error");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a hostile agent name in a budget row renders as text", async () => {
  const { renderBudgetsSection, loadAgentWatch, removeBudget, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const hostile = "<img src=x onerror=alert(1)>";
  const quote = '" onfocus="alert(1)';
  const view = watchView({ rows: [[hostile, 3, 5], [quote, 0, 5]], known: [hostile, quote, 'free"><b>x</b>'] });
  const html = renderBudgetsSection(view, "", { agent: "", amount: '"><b>' });

  assert.ok(!html.includes(hostile), "the raw hostile name reached the page");
  assert.ok(html.includes("&lt;img src=x onerror=alert(1)&gt;"), "the escaped name is missing from its row");
  assert.ok(!html.includes(quote), "a name with a quote must not break out of an attribute");
  assert.ok(!html.includes("<b>"), "no tag from a name or from the draft may reach the page");
  assert.ok(html.includes("free&quot;&gt;&lt;b&gt;x&lt;/b&gt;"), "an offered name is escaped in the picker too");

  // The focus step after a keyboard Remove finds the next row's button by
  // comparing data values, never by a selector made of the name: a newline, a
  // quote, a backslash or a bracket in a file name would make one throw.
  const nasty = 'a\n"b\\]c\r\u0000';
  const doc = makeFakeDocument();
  const focused = [];
  const button = (name) => ({ dataset: { budgetRemove: name }, focus: () => focused.push(name) });
  doc.querySelectorAll = (sel) => (sel === "[data-budget-remove]" ? [button("other"), button(nasty)] : []);
  const plain = doc.querySelector.bind(doc);
  doc.querySelector = (sel) => {
    if (/[\n\r\u0000\\"\]]/.test(sel)) throw new Error(`a selector was built from user data: ${sel}`);
    return plain(sel);
  };
  globalThis.document = doc;
  const state = { view: watchView({ rows: [["first", 0, 5], [nasty, 0, 5]], known: ["first", nasty, "other"] }) };
  globalThis.invoke = panelInvoke(state, () => watchView({ rows: [[nasty, 0, 5]], known: state.view.known }));
  try {
    await loadAgentWatch();
    await removeBudget("first", true);
    assert.deepEqual(focused, [nasty], "focus goes to the row that moved up, found without a selector");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("the budgets section calls only set_agent_watch", async () => {
  const { loadAgentWatch, addBudget, removeBudget, noteBudgetDraft, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  const { calls, sent, invoke } = watchInvoke(watchView({ live: { hourlyPaceUsd: 25, maxMinutes: 480 } }));
  globalThis.invoke = invoke;
  try {
    await loadAgentWatch();
    calls.length = 0;

    noteBudgetDraft("agent", "Plan");
    noteBudgetDraft("amount", "30");
    await addBudget(false);
    await removeBudget("general-purpose", false);

    assert.deepEqual(calls, ["set_agent_watch", "set_agent_watch"], "adding and removing may call set_agent_watch and nothing else");
    assert.deepEqual(
      sent[0],
      {
        budgets: [{ agent: "general-purpose", monthlyBudget: 10 }, { agent: "deploy-checker", monthlyBudget: 5 }, { agent: "Plan", monthlyBudget: 30 }],
        live: { hourlyPaceUsd: 25, maxMinutes: 480 },
      },
      "an add sends the saved budgets plus the new one, with the live rule untouched",
    );
    assert.deepEqual(
      sent[1],
      {
        budgets: [{ agent: "deploy-checker", monthlyBudget: 5 }, { agent: "Plan", monthlyBudget: 30 }],
        live: { hourlyPaceUsd: 25, maxMinutes: 480 },
      },
      "a remove sends what is left, from the view the add's answer returned",
    );

    // A bad figure is caught before any call.
    calls.length = 0;
    noteBudgetDraft("agent", "Explore");
    noteBudgetDraft("amount", "0");
    await addBudget(false);
    noteBudgetDraft("amount", "2.5");
    await addBudget(false);
    noteBudgetDraft("amount", "1000001");
    await addBudget(false);
    noteBudgetDraft("agent", "");
    noteBudgetDraft("amount", "5");
    await addBudget(false);
    assert.deepEqual(calls, [], "a figure outside 1 to 1,000,000, or no agent, must make no call");
  } finally {
    noteBudgetDraft("agent", "");
    noteBudgetDraft("amount", "");
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("an older load's answer is not painted", async () => {
  const { loadAgentWatch, saveAgentLive, agentWatchState, onAgentWatchChange, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  // Saves never overlap, and a load is skipped during one, so the older answer
  // that can still arrive late is a load's: a refresh asked for before a save,
  // answered after it. It describes the file as it was.
  const before = watchView();
  const answers = [];
  let loads = 0;
  globalThis.invoke = (cmd, args) => {
    if (cmd === "get_agent_watch") {
      loads += 1;
      if (loads === 1) return Promise.resolve(before);
      return new Promise((resolve) => answers.push(() => resolve(before)));
    }
    return Promise.resolve(watchView({ live: args.watch.live }));
  };
  const painted = [];
  onAgentWatchChange(() => painted.push(agentWatchState().view?.watch.live.hourlyPaceUsd));
  try {
    await loadAgentWatch();
    painted.length = 0;
    const slowLoad = loadAgentWatch(); // asked first, answers last
    assert.deepEqual(await saveAgentLive({ hourlyPaceUsd: 50 }), { outcome: "saved" });
    const paintedBySave = painted.length;
    answers[0]();
    await slowLoad;
    assert.equal(painted.length, paintedBySave, "the older answer must not be painted");
    assert.equal(agentWatchState().view.watch.live.hourlyPaceUsd, 50, "a late older answer must not replace the newer view");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("an older failed load does not show its error over a newer answer", async () => {
  const { loadAgentWatch, agentWatchState, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  const good = watchView();
  const settle = [];
  let n = 0;
  globalThis.invoke = () => {
    n += 1;
    if (n === 2) return new Promise((_, reject) => settle.push(() => reject("slow failure")));
    return Promise.resolve(good);
  };
  try {
    await loadAgentWatch();
    const slow = loadAgentWatch();
    await loadAgentWatch();
    settle[0]();
    await slow;
    assert.equal(agentWatchState().error, "", "a failure from a superseded load must not be painted");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("the budgets heading counts the rows shown", async () => {
  const { renderBudgetsSection, renderAgentsView, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  for (const n of [0, 1, 3]) {
    const rows = Array.from({ length: n }, (_, i) => [`agent-${i}`, i, 5]);
    // Many more known names than budgets, so a count taken from anywhere but
    // the rows themselves cannot match by accident.
    const view = watchView({ rows, known: [...rows.map((r) => r[0]), "a", "b", "c", "d", "e", "f"] });
    const html = renderBudgetsSection(view, "", NO_DRAFT);
    const shown = (html.match(/class="inv-row ag-budget-row"/g) ?? []).length;
    assert.equal(shown, n);
    assert.ok(html.includes(`Budgets <span class="plan">${n}</span>`), `the heading must count ${n} rows`);
  }

  // Placement: after Built-in agents, before Worth a look.
  const full = renderAgentsView(inventory(), [], "", [agentSpendRow({ name: "general-purpose" })], "", 0, NOW_MS, watchView(), "", NO_DRAFT, "");
  const at = (needle) => full.indexOf(needle);
  assert.ok(at("Built-in agents") < at('data-section="agent-budgets"') && at('data-section="agent-budgets"') < at("Worth a look"));
  assert.match(full, /<p class="inv-note">A budget covers the calendar month\./, "the note line sits under the title");
});

test("the live rule selects send the loaded budgets back unchanged", async () => {
  const { loadAgentWatch, saveAgentLive, livePayload, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const loaded = watchView({ live: { hourlyPaceUsd: 25, maxMinutes: 480 } });

  assert.deepEqual(livePayload(loaded, { hourlyPaceUsd: null }), {
    budgets: loaded.watch.budgets,
    live: { hourlyPaceUsd: null, maxMinutes: 480 },
  });

  globalThis.document = makeFakeDocument();
  const { sent, invoke } = watchInvoke(loaded);
  globalThis.invoke = invoke;
  try {
    await loadAgentWatch();
    assert.deepEqual(await saveAgentLive({ hourlyPaceUsd: null }), { outcome: "saved" });
    assert.deepEqual(await saveAgentLive({ maxMinutes: 60 }), { outcome: "saved" });
    assert.deepEqual(sent[0], { budgets: loaded.watch.budgets, live: { hourlyPaceUsd: null, maxMinutes: 480 } }, "only the changed figure may differ; the budgets go back as loaded");
    assert.deepEqual(sent[1], { budgets: loaded.watch.budgets, live: { hourlyPaceUsd: null, maxMinutes: 60 } }, "the second change builds on the answer to the first");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("an unlisted saved figure leaves the select blank", async () => {
  const { liveSelectValue, LIVE_PACE_PRESETS, LIVE_OPEN_PRESETS, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  assert.equal(liveSelectValue(7, LIVE_PACE_PRESETS), null, "a pace that is not a preset must leave the select blank");
  assert.equal(liveSelectValue(90, LIVE_OPEN_PRESETS), null, "a duration that is not a preset must leave the select blank");
  assert.equal(liveSelectValue(null, LIVE_PACE_PRESETS), "", "nothing set is the Off option");
  assert.equal(liveSelectValue(25, LIVE_PACE_PRESETS), "25");
  assert.equal(liveSelectValue(480, LIVE_OPEN_PRESETS), "480");

  // The presets and the options in index.html are one list: an option the code
  // does not know would always show blank, and a preset with no option could
  // never be shown.
  const html = await readFile(new URL("../index.html", import.meta.url), "utf8");
  const optionValues = (id) => {
    const select = html.match(new RegExp(`<select id="${id}"[^>]*>([\\s\\S]*?)</select>`));
    return [...select[1].matchAll(/<option value="([^"]*)"/g)].map((m) => m[1]);
  };
  assert.deepEqual(optionValues("agent-pace"), ["", ...LIVE_PACE_PRESETS.map(String)]);
  assert.deepEqual(optionValues("agent-open"), ["", ...LIVE_OPEN_PRESETS.map(String)]);
});

test("checkBudgetInput and the remove focus order", async () => {
  const { checkBudgetInput, budgetRemoveFocus } = await freshAgentsModule();
  const free = ["Plan", "Explore"];
  for (const good of ["5", " 5 ", "05", "２５", "1000000", "　７　"]) {
    assert.equal(checkBudgetInput("Plan", good, free).ok, true, `"${good}" is a whole number from 1 to 1,000,000`);
  }
  assert.deepEqual(checkBudgetInput("Plan", "２５", free), { ok: true, amount: 25 }, "full-width digits typed through an input method count");
  assert.deepEqual(checkBudgetInput("", "25", free), { ok: false, key: "error.agentWatch.pick" });
  assert.deepEqual(checkBudgetInput("Gone", "25", free), { ok: false, key: "error.agentWatch.pick" }, "an agent the picker does not offer is not sent");
  for (const bad of ["", "0", "1e3", "1.0", "1,000", "-5", "+5", "0x10", "1000001", "abc", "5 5", "9".repeat(400)]) {
    assert.deepEqual(checkBudgetInput("Plan", bad, free), { ok: false, key: "error.agentWatch.figure" }, `"${bad.slice(0, 12)}" is not a whole number from 1 to 1,000,000`);
  }

  const names = ["a", "b", "c"];
  const rm = (n) => ({ remove: n });
  const picker = { picker: true };
  assert.deepEqual(budgetRemoveFocus(names, "a", true), [rm("b"), picker], "the first row hands focus to the one that moved up");
  assert.deepEqual(budgetRemoveFocus(names, "b", true), [rm("c"), rm("a"), picker], "next row, then previous, then the picker");
  assert.deepEqual(budgetRemoveFocus(names, "c", true), [rm("b"), picker], "the last row hands focus to the one before it");
  assert.deepEqual(budgetRemoveFocus(["a"], "a", true), [picker], "no rows left: the picker");
  assert.deepEqual(budgetRemoveFocus(names, "b", false), [], "a pointer click leaves nothing focused");
  assert.deepEqual(budgetRemoveFocus(names, "zzz", true), [picker]);
});

test("the live rule hint follows a language switch", async () => {
  const { t, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const en = t("settings.agentPaceHint", { pace: "$4" });
  setActiveLocale("de");
  const de = t("settings.agentPaceHint", { pace: "$4" });
  setActiveLocale("en");
  assert.notEqual(en, de, "the hint must be a translated string, so a switch changes it");
  // The hint and the error line are painted with t(), not data-i18n, so the
  // switch only reaches them if applyLocale() repaints the Settings rows.
  const main = await readFile(new URL("../src/main.ts", import.meta.url), "utf8");
  const body = main.match(/function applyLocale\(\): void \{[\s\S]*?\n\}\n/)[0];
  assert.match(body, /paintAgentLive\(\)/, "applyLocale() must repaint the live rule's hint and error");
});

test("a budget note follows a language switch", async () => {
  const { openAgents, noteBudgetDraft, addBudget, __agentsRerender, loadAgentWatch, t, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const doc = makeFakeDocument();
  globalThis.document = doc;
  let refuse = false;
  const view = watchView();
  globalThis.invoke = async (cmd) => {
    if (cmd === "get_inventory") return inventory();
    if (cmd === "get_agent_watch") return view;
    if (cmd === "set_agent_watch") {
      if (refuse) throw "Refused by the backend.";
      return view;
    }
    if (cmd === "get_audit") return { sections: [] };
    return [];
  };
  try {
    openAgents();
    await flushMicrotasks();
    const body = () => doc.elements.get("#agents-body").innerHTML;

    noteBudgetDraft("agent", "");
    noteBudgetDraft("amount", "5");
    await addBudget(false);
    assert.ok(body().includes(esc(t("error.agentWatch.pick"))), "the pick note shows");
    setActiveLocale("de");
    __agentsRerender();
    assert.ok(body().includes(esc(t("error.agentWatch.pick"))) && !body().includes("Pick each agent"), "the pick note must be repainted in the new language");

    setActiveLocale("en");
    noteBudgetDraft("agent", "Plan");
    refuse = true;
    await addBudget(false);
    assert.ok(body().includes("Refused by the backend."), "the backend's refusal shows as it came");
    setActiveLocale("de");
    __agentsRerender();
    assert.ok(!body().includes("Refused by the backend."), "a refusal that was translated by the backend cannot follow the language, so it is dropped");
  } finally {
    setActiveLocale("en");
    noteBudgetDraft("agent", "");
    noteBudgetDraft("amount", "");
    doc.body.classList.remove("agents-open");
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a second save cannot start while one is in flight", async () => {
  const { loadAgentWatch, saveAgentWatch, removeBudget, addBudget, noteBudgetDraft, agentWatchState, renderBudgetsSection, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  const base = watchView();
  const release = [];
  const sentCmds = [];
  globalThis.invoke = (cmd, args) => {
    sentCmds.push(cmd);
    if (cmd === "get_agent_watch") return Promise.resolve(base);
    return new Promise((resolve) => release.push(() => resolve(watchView({ rows: args.watch.budgets.map((b) => [b.agent, 0, b.monthlyBudget]), known: base.known }))));
  };
  try {
    await loadAgentWatch();
    sentCmds.length = 0;
    const first = removeBudget("general-purpose", false);
    assert.equal(agentWatchState().saving, true, "a save in flight is visible to every control");
    assert.deepEqual(await saveAgentWatch((last) => ({ budgets: [], live: last.watch.live })), { outcome: "busy" });
    await removeBudget("deploy-checker", false);
    noteBudgetDraft("agent", "Plan");
    noteBudgetDraft("amount", "5");
    await addBudget(false);
    assert.deepEqual(sentCmds, ["set_agent_watch"], "a second save must not be sent while the first is in flight");

    // Every control that could start one is disabled meanwhile.
    const html = renderBudgetsSection(base, "", { agent: "", amount: "" }, "", true);
    const controls = (html.match(/<(button|select|input)\b[^>]*>/g) ?? []);
    assert.equal(controls.length, 5, "two Remove buttons, picker, amount and Add");
    assert.ok(controls.every((c) => / disabled[ >]/.test(c)), "every budget control is disabled while a save is in flight");
    assert.ok(!/<(button|select|input)\b(?![^>]*disabled)[^>]*>/.test(html));
    const free = renderBudgetsSection(base, "", { agent: "", amount: "" }, "", false);
    assert.ok(!/ disabled[ >]/.test(free.replace('value="" disabled', "")), "nothing is disabled when no save is in flight");

    release[0]();
    await first;
    assert.equal(agentWatchState().saving, false, "the controls come back once the answer is painted");
    assert.deepEqual(agentWatchState().view.budgets.map((b) => b.agent), ["deploy-checker"]);
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a load requested during a save does not discard the save's answer", async () => {
  const { loadAgentWatch, saveAgentLive, agentWatchState, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  const base = watchView();
  const cmds = [];
  let release;
  globalThis.invoke = (cmd, args) => {
    cmds.push(cmd);
    if (cmd === "get_agent_watch") return Promise.resolve(base);
    return new Promise((resolve) => { release = () => resolve(watchView({ live: args.watch.live })); });
  };
  try {
    await loadAgentWatch();
    cmds.length = 0;
    const save = saveAgentLive({ hourlyPaceUsd: 50 });
    await loadAgentWatch(); // Settings opening mid-save
    assert.deepEqual(cmds, ["set_agent_watch"], "a load must be skipped while a save is in flight");
    release();
    assert.deepEqual(await save, { outcome: "saved" }, "the save's answer is the fresh view, not a stale one");
    assert.equal(agentWatchState().view.watch.live.hourlyPaceUsd, 50);
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a drafted agent that is no longer free is not sent", async () => {
  const { loadAgentWatch, addBudget, noteBudgetDraft, checkBudgetInput, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  const sent = [];
  const state = { view: watchView({ rows: [], known: ["Explore", "Plan"] }) };
  globalThis.invoke = panelInvoke(state, (args) => { sent.push(args.watch); return state.view; });
  try {
    await loadAgentWatch();
    noteBudgetDraft("agent", "Plan");
    noteBudgetDraft("amount", "5");
    // Plan gets a budget elsewhere (the other window, a hand edit)...
    state.view = watchView({ rows: [["Plan", 0, 9]], known: ["Explore", "Plan"] });
    await loadAgentWatch();
    // ...and later is free again: the stale pick must not come back by itself.
    state.view = watchView({ rows: [], known: ["Explore", "Plan"] });
    await loadAgentWatch();
    await addBudget(false);
    assert.deepEqual(sent, [], "a name that was dropped from the picker must not be sent");

    // The add itself also checks the free list, not only the draft.
    assert.deepEqual(checkBudgetInput("Plan", "5", ["Explore"]), { ok: false, key: "error.agentWatch.pick" });
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("the caret in the amount field survives a redraw", async () => {
  const { openAgents, loadAgentWatch, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const doc = makeFakeDocument();
  globalThis.document = doc;
  const state = { view: watchView() };
  globalThis.invoke = panelInvoke(state);
  try {
    openAgents();
    await flushMicrotasks();
    const field = doc.querySelector("#agent-budget-amount");
    const events = [];
    field.id = "agent-budget-amount";
    field.focus = () => { events.push("focus"); doc.activeElement = field; };
    field.setSelectionRange = (a, b) => events.push(`range ${a},${b}`);

    // Typing "2|5": the caret sits between the digits when an answer redraws.
    field.selectionStart = 1;
    field.selectionEnd = 1;
    doc.activeElement = field;
    await loadAgentWatch();
    assert.deepEqual(events, ["focus", "range 1,1"], "the redraw must hand focus and the caret back");

    // Focus somewhere else: the redraw takes nothing.
    events.length = 0;
    doc.activeElement = { id: "other" };
    await loadAgentWatch();
    assert.deepEqual(events, []);
  } finally {
    doc.body.classList.remove("agents-open");
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a redraw waits for a composition to end", async () => {
  const { openAgents, loadAgentWatch, noteComposition, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const doc = makeFakeDocument();
  globalThis.document = doc;
  const state = { view: watchView() };
  globalThis.invoke = panelInvoke(state);
  try {
    openAgents();
    await flushMicrotasks();
    const body = () => doc.elements.get("#agents-body").innerHTML;
    assert.ok(body().includes("general-purpose"));

    noteComposition(true);
    state.view = watchView({ rows: [["late-arrival", 0, 5]], known: ["late-arrival"] });
    await loadAgentWatch();
    assert.ok(!body().includes("late-arrival"), "the field is being composed in: the section must not be replaced under it");

    noteComposition(false);
    assert.ok(body().includes("late-arrival"), "the held redraw runs when the composition ends");
  } finally {
    doc.body.classList.remove("agents-open");
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("Enter during a composition does not add", async () => {
  const { enterAddsBudget } = await freshAgentsModule();
  const id = "agent-budget-amount";
  assert.equal(enterAddsBudget("Enter", false, false, 13, id), true);
  assert.equal(enterAddsBudget("Enter", false, true, 13, id), false, "the Enter that confirms a composition is not Add");
  assert.equal(enterAddsBudget("Enter", false, false, 229, id), false, "keyCode 229 marks a composition event too");
  assert.equal(enterAddsBudget("Enter", true, false, 13, id), false, "a held key does not add again");
  assert.equal(enterAddsBudget("a", false, false, 65, id), false);
  assert.equal(enterAddsBudget("Enter", false, false, 13, "agent-budget-pick"), false);
});

test("add and remove end to end", async () => {
  const { openAgents, loadAgentWatch, addBudget, removeBudget, noteBudgetDraft, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const doc = makeFakeDocument();
  globalThis.document = doc;
  const state = { view: watchView({ rows: [["general-purpose", 14.74, 10]], known: ["Explore", "Plan", "general-purpose"] }) };
  const sent = [];
  let refuse = null;
  globalThis.invoke = panelInvoke(state, (args) => {
    sent.push(args.watch);
    if (refuse) throw refuse;
    return watchView({ rows: args.watch.budgets.map((b) => [b.agent, 0, b.monthlyBudget]), live: args.watch.live, known: state.view.known });
  });
  try {
    openAgents();
    await flushMicrotasks();
    const body = () => doc.elements.get("#agents-body").innerHTML;

    // A refusal keeps the draft and shows the backend's own text.
    refuse = "The agent budgets are full.";
    noteBudgetDraft("agent", "Plan");
    noteBudgetDraft("amount", "25");
    await addBudget(false);
    assert.deepEqual(sent[0], {
      budgets: [{ agent: "general-purpose", monthlyBudget: 10 }, { agent: "Plan", monthlyBudget: 25 }],
      live: { hourlyPaceUsd: null, maxMinutes: null },
    });
    assert.ok(body().includes("The agent budgets are full."), "the rejection text is shown");
    assert.match(body(), /<option value="Plan" selected>/, "the picked agent is kept after a refusal");
    assert.match(body(), /value="25"/, "the typed amount is kept after a refusal");

    // The next success clears the note and the draft.
    refuse = null;
    await addBudget(false);
    assert.ok(!body().includes("The agent budgets are full."), "the next success clears the note");
    assert.ok(!/<option value="Plan" selected>/.test(body()) && !/id="agent-budget-amount"[^>]*value="25"/.test(body()), "a saved add empties the draft");
    assert.ok(body().includes('data-budget-remove="Plan"'), "the new row is painted from the answer");

    await removeBudget("Plan", false);
    assert.deepEqual(sent[sent.length - 1].budgets, [{ agent: "general-purpose", monthlyBudget: 10 }]);
  } finally {
    doc.body.classList.remove("agents-open");
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("focus waits on the heading during a save and then lands where the steps say", async () => {
  const { loadAgentWatch, removeBudget, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const doc = makeFakeDocument();
  globalThis.document = doc;
  const log = [];
  const heading = doc.querySelector("#agents-heading");
  heading.id = "agents-heading";
  heading.focus = () => { log.push("heading"); doc.activeElement = heading; };
  const next = { dataset: { budgetRemove: "b" }, focus: () => { log.push("next"); doc.activeElement = next; } };
  doc.querySelectorAll = (sel) => (sel === "[data-budget-remove]" ? [next] : []);
  let release;
  const view = watchView({ rows: [["a", 0, 5], ["b", 0, 5]], known: ["a", "b"] });
  globalThis.invoke = (cmd) => (cmd === "get_agent_watch" ? Promise.resolve(view) : new Promise((resolve) => { release = () => resolve(watchView({ rows: [["b", 0, 5]], known: ["a", "b"] })); }));
  try {
    await loadAgentWatch();
    // The Remove button the keyboard user pressed has focus, inside the section.
    const pressed = { closest: (sel) => (sel === '[data-section="agent-budgets"]' ? {} : null) };
    doc.activeElement = pressed;
    const done = removeBudget("a", true);
    assert.deepEqual(log, ["heading"], "the disabled control loses focus, so it waits on the heading, never on nothing");
    release();
    await done;
    assert.deepEqual(log, ["heading", "next"], "after the answer focus lands on the row that moved up");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a live rule dropdown change is sent as the loaded budgets plus one figure", async () => {
  const { loadAgentWatch, changeLiveRule, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  const loaded = watchView({ live: { hourlyPaceUsd: 25, maxMinutes: 480 } });
  const sent = [];
  let refuse = null;
  globalThis.invoke = async (cmd, args) => {
    if (cmd === "get_agent_watch") return loaded;
    sent.push(args.watch);
    if (refuse) throw refuse;
    return watchView({ rows: args.watch.budgets.map((b) => [b.agent, 0, b.monthlyBudget]), live: args.watch.live });
  };
  try {
    await loadAgentWatch();
    assert.deepEqual(await changeLiveRule("hourlyPaceUsd", ""), { outcome: "saved", error: "", show: "" }, "Off is saved as null and shows the Off option");
    assert.deepEqual(sent[0], { budgets: loaded.watch.budgets, live: { hourlyPaceUsd: null, maxMinutes: 480 } });
    assert.deepEqual(await changeLiveRule("maxMinutes", "720"), { outcome: "saved", error: "", show: "720" }, "a preset is sent as its number");
    assert.deepEqual(sent[1], { budgets: loaded.watch.budgets, live: { hourlyPaceUsd: null, maxMinutes: 720 } });

    refuse = "Enter a figure above zero, up to 1,000,000.";
    assert.deepEqual(await changeLiveRule("hourlyPaceUsd", "100"), { outcome: "rejected", error: refuse, show: "" }, "a refusal returns the saved value, to put the select back to");
    // The select shows blank when the saved figure is not a preset.
    refuse = null;
    globalThis.invoke = async (cmd) => {
      if (cmd === "get_agent_watch") return watchView({ live: { hourlyPaceUsd: 7, maxMinutes: null } });
      throw "refused";
    };
    await loadAgentWatch();
    assert.deepEqual(await changeLiveRule("maxMinutes", "60"), { outcome: "rejected", error: "refused", show: "" });
    assert.equal((await changeLiveRule("hourlyPaceUsd", "50")).show, null, "an unlisted saved pace leaves the select blank");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a payload builder that throws does not leave the controls disabled", async () => {
  const { loadAgentWatch, saveAgentWatch, agentWatchState, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  globalThis.invoke = async () => watchView();
  try {
    await loadAgentWatch();
    assert.throws(() => saveAgentWatch(() => { throw new Error("bad payload"); }), /bad payload/);
    assert.equal(agentWatchState().saving, false, "a save that never started must not keep every control disabled");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a composition cannot hold redraws back for good", async () => {
  const { openAgents, applyRescan, loadAgentWatch, noteComposition, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const doc = makeFakeDocument();
  globalThis.document = doc;
  const state = { view: watchView() };
  globalThis.invoke = panelInvoke(state);
  const body = () => doc.elements.get("#agents-body").innerHTML;
  const rescan = (over) => ({ inventory: inventory(), loadError: "", runningAgents: [], runningAgentsError: "", agentSpend: [], agentSpendError: "", ...over });
  try {
    openAgents();
    await flushMicrotasks();

    // A composition is under way when a rescan arrives whose load failed: the
    // body is replaced by the error and the amount field is destroyed, so no
    // compositionend will ever come.
    noteComposition(true);
    applyRescan(rescan({ inventory: null, loadError: "scan failed" }));
    assert.ok(body().includes("scan failed"));

    // The next redraws must still paint.
    applyRescan(rescan({ inventory: inventory({ agents: [definition({ name: "after-composition" })] }) }));
    assert.ok(body().includes("after-composition"), "a destroyed field must not leave redraws held back");
    state.view = watchView({ rows: [["saved-after", 0, 5]], known: ["saved-after"] });
    await loadAgentWatch();
    assert.ok(body().includes("saved-after"), "a save's result must paint");
  } finally {
    doc.body.classList.remove("agents-open");
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("the text a composition commits reaches the draft", async () => {
  const { openAgents, loadAgentWatch, noteComposition, finishComposition, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  const doc = makeFakeDocument();
  globalThis.document = doc;
  const state = { view: watchView() };
  globalThis.invoke = panelInvoke(state);
  try {
    openAgents();
    await flushMicrotasks();
    const body = () => doc.elements.get("#agents-body").innerHTML;

    noteComposition(true);
    state.view = watchView({ rows: [["x", 0, 5]], known: ["x", "y"] });
    await loadAgentWatch(); // held back
    finishComposition("２５"); // the field's value at compositionend
    assert.match(body(), /id="agent-budget-amount"[^>]*value="２５"/, "the committed text must be in the redrawn field");
  } finally {
    doc.body.classList.remove("agents-open");
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("a settings dropdown takes focus back only when focus was lost", async () => {
  const { selectTakesFocusBack } = await freshAgentsModule();
  assert.equal(selectTakesFocusBack(true, true, true, false), true, "focus fell to nothing");
  assert.equal(selectTakesFocusBack(true, true, false, true), true, "focus is still on the select");
  assert.equal(selectTakesFocusBack(true, true, false, false), false, "the user moved elsewhere: leave it");
  assert.equal(selectTakesFocusBack(true, false, true, false), false, "Settings was closed meanwhile");
  assert.equal(selectTakesFocusBack(false, true, true, false), false, "it never had focus");
});

test("a failed changeLiveRule puts the dropdown back", async () => {
  const { loadAgentWatch, changeLiveRule, agentWatchState, setActiveLocale } = await freshAgentsModule();
  setActiveLocale("en");
  globalThis.document = makeFakeDocument();
  // A view whose watch is unusable makes the payload builder throw.
  const broken = { ...watchView({ live: { hourlyPaceUsd: 25, maxMinutes: null } }), watch: null };
  globalThis.invoke = async () => broken;
  try {
    await loadAgentWatch();
    const result = await changeLiveRule("hourlyPaceUsd", "50");
    assert.equal(result.outcome, "rejected", "a throw must come back as a result, not as an unhandled rejection");
    assert.equal(result.show, null, "the select is put back to what is saved (nothing usable here, so blank)");
    assert.equal(agentWatchState().saving, false);
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});
