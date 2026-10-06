// The demo's Agents view budgets are not typed into src/demo/mock.ts: they are what the real
// engine answered for the fictional machine's own agent_watch.json (`live_agent_watch`, run by
// scripts/make-demo-fixture.py into src/demo-fixture.json). Held here, against that committed
// fixture itself: the saved budgets, the engine's calendar-month figure beside each, and the
// names a budget may take all come from it; and what a visitor does in the demo still behaves as
// it does in the app -- a budget added on an agent with spend this month reads that spend, one
// on an agent with none reads $0, a name that is not offered is refused.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { loadDemoBackend } from "./demo-backend.mjs";

const fixture = JSON.parse(await readFile(new URL("../src/demo-fixture.json", import.meta.url), "utf8"));
const engine = fixture.agentWatch;

/** A fresh page load of the demo, over the committed fixture. */
const freshDemo = () => loadDemoBackend(fixture);

const en = JSON.parse(await readFile(new URL("../src/locales/en.json", import.meta.url), "utf8"));

test("the demo's budgets are the engine's answer for the fictional machine", async () => {
  const { handle } = await freshDemo();
  const view = handle("get_agent_watch");
  assert.deepEqual(view.watch, engine.watch, "what is saved");
  assert.deepEqual(view.budgets, engine.budgets, "each budget against the calendar month, as the engine summed it: the same figure its monthSpend holds");
  assert.deepEqual(view.known, engine.known, "the names a budget may take");
  assert.ok(view.budgets.some((b) => b.monthToDate >= b.monthlyBudget) && view.budgets.some((b) => b.monthToDate < b.monthlyBudget));
  // The hint is the demo's own running agent's pace.
  assert.ok(view.liveHint > 0);
  // The view is a copy: painting it cannot change what is saved.
  view.watch.budgets.length = 0;
  assert.equal(handle("get_agent_watch").watch.budgets.length, engine.watch.budgets.length);
});

test("the demo's view has the fields the app's has, and the view's type names the same ones", async () => {
  // `AgentWatchView` in src-tauri/src/lib.rs is what the command serializes; src/agents.ts types what
  // the view reads. A field only one of the three carries is a field painted, or sent, for nothing.
  const lib = await readFile(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
  const struct = lib.match(/struct AgentWatchView \{([\s\S]*?)\n\}/);
  assert.ok(struct, "struct AgentWatchView moved in src-tauri/src/lib.rs");
  const camel = (name) => name.replace(/_([a-z])/g, (_, c) => c.toUpperCase());
  const rust = [...struct[1].matchAll(/^\s*(?:pub\s+)?([a-z_]+):/gm)].map((m) => camel(m[1])).sort();
  const agents = await readFile(new URL("../src/agents.ts", import.meta.url), "utf8");
  const iface = agents.match(/export interface AgentWatchView \{([\s\S]*?)\n\}/);
  assert.ok(iface, "interface AgentWatchView moved in src/agents.ts");
  const ts = [...iface[1].matchAll(/^\s*([a-zA-Z]+)\??:/gm)].map((m) => m[1]).sort();
  assert.deepEqual(rust, ["budgets", "known", "liveHint", "watch"], "the app's view");
  assert.deepEqual(ts, rust, "the view's type");
  const { handle } = await freshDemo();
  assert.deepEqual(Object.keys(handle("get_agent_watch")).sort(), rust, "the demo's view");
  assert.deepEqual(Object.keys(handle("set_agent_watch", { watch: engine.watch })).sort(), rust, "the demo's view after a save");
});

test("a budget saved in the demo keeps its figure, and what it was saved with reads back", async () => {
  const { handle } = await freshDemo();
  const [first] = engine.watch.budgets;
  const saved = handle("set_agent_watch", {
    watch: { budgets: [{ agent: first.agent, monthlyBudget: 99 }], live: { hourlyPaceUsd: 3, maxMinutes: 90 } },
  });
  assert.deepEqual(saved.budgets, [{ agent: first.agent, monthToDate: engine.monthSpend[first.agent], monthlyBudget: 99 }]);
  assert.deepEqual(saved.watch.live, { hourlyPaceUsd: 3, maxMinutes: 90 });
  assert.deepEqual(handle("get_agent_watch"), saved, "a later read says what the save said");
  assert.equal(engine.watch.budgets.find((b) => b.agent === first.agent).monthlyBudget, 10, "the fixture itself is never written to");

  // Clearing leaves no budgets and every name still offered.
  const cleared = handle("set_agent_watch", { watch: {} });
  assert.deepEqual([cleared.budgets, cleared.watch.live], [[], { hourlyPaceUsd: null, maxMinutes: null }]);
  assert.deepEqual(cleared.known, engine.known);
});

test("a budget added in the demo reads its agent's spend this month, and zero when the agent has none", async () => {
  const { handle } = await freshDemo();
  const budgeted = new Set(engine.watch.budgets.map((b) => b.agent));
  // An agent the engine knows spend for this month that has no budget yet, and one it knows no spend for.
  const withSpend = Object.keys(engine.monthSpend).find((name) => !budgeted.has(name));
  const withoutSpend = engine.known.find((name) => !budgeted.has(name) && !Object.hasOwn(engine.monthSpend, name));
  assert.ok(withSpend && engine.monthSpend[withSpend] > 0, "the fixture has an agent with spend and no budget, or this test has nothing to add");
  assert.ok(withoutSpend, "the fixture offers an agent with no spend, or this test has nothing to add");

  const [first] = engine.watch.budgets;
  const view = handle("set_agent_watch", {
    watch: { budgets: [{ agent: first.agent, monthlyBudget: 50 }, { agent: withSpend, monthlyBudget: 1 }, { agent: withoutSpend, monthlyBudget: 2 }] },
  });
  assert.deepEqual(view.budgets, [
    { agent: first.agent, monthToDate: engine.monthSpend[first.agent], monthlyBudget: 50 },
    { agent: withSpend, monthToDate: engine.monthSpend[withSpend], monthlyBudget: 1 },
    { agent: withoutSpend, monthToDate: 0, monthlyBudget: 2 },
  ]);
  // The figure is the same one the agent shows in Your agents when its whole 30 days fall in the
  // month, as they do for this fixture: a budget never reads $0 beside an agent that shows spend.
  const shown = fixture.agentSpend.find((row) => row.name === withSpend);
  assert.ok(shown, "the agent with month spend is listed in the agents' 30-day spend");
  assert.ok(Math.abs(view.budgets[1].monthToDate - shown.cost) < 1e-6, `${withSpend}: ${view.budgets[1].monthToDate} beside ${shown.cost}`);
});

test("the demo refuses what the app refuses", async () => {
  const { handle } = await freshDemo();
  const [first] = engine.watch.budgets;
  const refused = (budgets) => {
    try {
      handle("set_agent_watch", { watch: { budgets } });
    } catch (e) {
      return e;
    }
    return null;
  };
  // The refusals are the app's own translated messages, not keys.
  assert.equal(refused([{ agent: "an-agent-nobody-has", monthlyBudget: 5 }]), en["error.agentWatch.pick"], "a name that is not offered");
  assert.equal(refused([{ agent: first.agent, monthlyBudget: 5 }, { agent: first.agent, monthlyBudget: 6 }]), en["error.agentWatch.duplicate"]);
  assert.equal(refused([{ agent: first.agent, monthlyBudget: 0 }]), en["error.agentWatch.figure"], "a figure of zero");
  assert.equal(refused([{ agent: first.agent, monthlyBudget: 1_000_001 }]), en["error.agentWatch.figure"], "a figure above the cap");
  assert.equal(refused([{ agent: engine.known[0], monthlyBudget: 5 }]), null, "a name the engine offers is accepted");
  // A save that failed changed nothing.
  assert.deepEqual(handle("get_agent_watch").watch.budgets, [{ agent: engine.known[0], monthlyBudget: 5 }]);
});

// ---------------------------------------------------------------------------
// The finding that goes with the budgets
// ---------------------------------------------------------------------------

test("the audit carries the over-budget finding for exactly the budgets that are over", () => {
  const checks = fixture.audit.sections.flatMap((s) => s.checks);
  const finding = checks.find((c) => c.id === "agent-over-budget");
  assert.ok(finding, "a budget is over its figure, so the engine's audit has the finding: live_audit has to read agent_watch.json");
  assert.equal(finding.status, "consider", "an unscored row: a budget passed is worth a look, not a failing");
  assert.equal(checks.filter((c) => c.id === "agent-over-budget").length, 1);

  const over = engine.budgets.filter((b) => b.monthToDate >= b.monthlyBudget);
  assert.ok(over.length > 0 && over.length < engine.budgets.length, "the demo shows a budget over and one that is not");
  // The title is a count and nothing else, so consistency runs through the budgets: how many are
  // over, and which, from the detail's own list, "name ($spent / $budget)", figures to the cent.
  assert.equal(finding.titleMsg.count, over.length);
  assert.deepEqual(finding.titleMsg.vars, {});
  const listed = finding.detailMsg.vars.names.split(", ").map((entry) => {
    const parts = entry.match(/^(.+) \(\$(\d+\.\d\d) \/ \$(\d+\.\d\d)\)$/);
    assert.ok(parts, `not a "name ($spent / $budget)" entry: ${entry}`);
    return { agent: parts[1], spent: Number(parts[2]), budget: Number(parts[3]) };
  });
  assert.deepEqual(listed.map((e) => e.agent).sort(), over.map((b) => b.agent).sort(), "the agents it names are the ones over in the budgets");
  for (const entry of listed) {
    const budget = over.find((b) => b.agent === entry.agent);
    assert.ok(Math.abs(entry.spent - budget.monthToDate) < 0.005 + 1e-9, `${entry.agent}: spent ${entry.spent} against ${budget.monthToDate}`);
    assert.ok(Math.abs(entry.budget - budget.monthlyBudget) < 0.005 + 1e-9, `${entry.agent}: budget ${entry.budget} against ${budget.monthlyBudget}`);
  }
});

test("the demo lists the over-budget finding in the Inventory and the Audit, with no Learn more link", async () => {
  const { handle } = await freshDemo();
  const row = handle("get_inventory").opportunities.find((o) => o.id === "agent-over-budget");
  assert.ok(row, "the Agents view's Worth a look reads this list");
  const check = fixture.audit.sections.flatMap((s) => s.checks).find((c) => c.id === "agent-over-budget");
  assert.deepEqual([row.title, row.detail, row.titleMsg, row.detailMsg], [check.title, check.detail, check.titleMsg, check.detailMsg], "the engine's own words, not a second copy");
  assert.deepEqual([row.kind, row.learnUrl], ["learn", null], "the app's finding has no Learn more link");
  assert.equal(handle("get_inventory").opportunities.filter((o) => o.id === "agent-over-budget").length, 1);
  const usage = handle("get_audit").sections.find((s) => s.nameKey === "section.usage");
  assert.equal(usage.checks.filter((c) => c.id === "agent-over-budget").length, 1);
});

test("the over-budget finding reads whole in every language", async () => {
  const { render, LOCALES } = await freshDemo();
  const check = fixture.audit.sections.flatMap((s) => s.checks).find((c) => c.id === "agent-over-budget");
  for (const locale of LOCALES) {
    for (const [label, msg] of [["title", check.titleMsg], ["detail", check.detailMsg]]) {
      const text = render(locale, msg);
      assert.ok(!/[{}]/.test(text), `${locale} ${label} left a {var} unfilled: ${text}`);
      assert.notEqual(text, msg.key, `${locale} ${label} rendered as its own key`);
    }
  }
});

// ---------------------------------------------------------------------------
// The finding follows the visitor's budgets
// ---------------------------------------------------------------------------
//
// The app works the finding out from the saved watch on every load. The demo does the same with
// `buildOverBudgetRow` (src/demo/synthetic.ts), a twin of the budget half of
// `agent_watch::opportunities`, over the saved budgets and the engine's month spend; the engine's
// own check stays in the fixture file, as the machine was generated, and is not lifted.

const row = (agent, monthToDate, monthlyBudget) => ({ agent, monthToDate, monthlyBudget });

test("the over-budget finding built here is the engine's, word for word", async () => {
  const { buildOverBudgetRow } = await freshDemo();
  const check = fixture.audit.sections.flatMap((s) => s.checks).find((c) => c.id === "agent-over-budget");
  const built = buildOverBudgetRow(engine.budgets);
  assert.deepEqual(
    [built.id, built.kind, built.learnUrl, built.title, built.detail, built.titleMsg, built.detailMsg],
    ["agent-over-budget", "learn", null, check.title, check.detail, check.titleMsg, check.detailMsg],
    "the twin and the engine make the same finding from the same budgets",
  );
});

test("the over-budget finding follows the Rust rule's cases", async () => {
  const { buildOverBudgetRow } = await freshDemo();
  // crates/core/src/agent_watch.rs, `findings_count_what_their_detail_names`.
  const found = buildOverBudgetRow([row("reviewer", 12.5, 10), row("Explore", 30, 30), row("Plan", 1, 2)]);
  assert.equal(found.titleMsg.count, 2, "two budgets are over; the third is under");
  assert.deepEqual(found.titleMsg.vars, {}, "a title carries a count only");
  assert.equal(found.detailMsg.vars.names, "reviewer ($12.50 / $10.00), Explore ($30.00 / $30.00)");
  assert.equal(found.title, "2 agents are over their monthly budget");
  assert.equal(buildOverBudgetRow([row("Plan", 3, 2)]).title, "1 agent is over its monthly budget");
  // Only what is over now is a finding; at the budget counts, a cent under does not.
  assert.equal(buildOverBudgetRow([]), null);
  assert.equal(buildOverBudgetRow([row("Plan", 1, 2)]), null);
  assert.equal(buildOverBudgetRow([row("Plan", 1.99, 2)]), null);
  assert.equal(buildOverBudgetRow([row("Plan", 2, 2)]).titleMsg.count, 1);
  // The budgets are named in the order they were saved.
  assert.equal(buildOverBudgetRow([row("b", 5, 1), row("a", 5, 1)]).detailMsg.vars.names, "b ($5.00 / $1.00), a ($5.00 / $1.00)");
});

/** The findings the demo lists about budgets right now: in the Inventory, and as a check of the Audit. */
function budgetFindings(handle) {
  const inventory = handle("get_inventory").opportunities.filter((o) => o.id === "agent-over-budget");
  const audit = handle("get_audit").sections.flatMap((s) => s.checks).filter((c) => c.id === "agent-over-budget");
  return { inventory, audit };
}

test("removing the budget that is over removes the finding, in the Inventory and in the Audit", async () => {
  const { handle } = await freshDemo();
  const before = budgetFindings(handle);
  assert.equal(before.inventory.length, 1, "the finding is there to start with");
  assert.equal(before.audit.length, 1);

  const over = new Set(engine.budgets.filter((b) => b.monthToDate >= b.monthlyBudget).map((b) => b.agent));
  const kept = engine.watch.budgets.filter((b) => !over.has(b.agent));
  assert.ok(kept.length > 0, "a budget under its figure stays, so this is not the same as having none");
  handle("set_agent_watch", { watch: { budgets: kept } });
  const after = budgetFindings(handle);
  assert.deepEqual([after.inventory.length, after.audit.length], [0, 0], "Worth a look, the Inventory row and the Audit check all follow the saved budgets");
  // Nothing else about the Audit moved, and the other finding the demo builds is still there.
  const ids = handle("get_audit").sections.flatMap((s) => s.checks).map((c) => c.id);
  assert.ok(ids.includes("limit-time") && ids.includes("mix-top-heavy"));
  const report = handle("get_audit");
  assert.deepEqual([report.passed, report.attention, report.score], [fixture.audit.passed, fixture.audit.attention, fixture.audit.score]);

  // No budgets at all is no finding either, and putting the saved ones back brings the engine's finding back.
  handle("set_agent_watch", { watch: {} });
  assert.deepEqual([budgetFindings(handle).inventory.length, budgetFindings(handle).audit.length], [0, 0]);
  handle("set_agent_watch", { watch: engine.watch });
  const restored = budgetFindings(handle);
  assert.deepEqual([restored.inventory[0].titleMsg, restored.inventory[0].detailMsg], [before.inventory[0].titleMsg, before.inventory[0].detailMsg]);
  assert.deepEqual(restored.audit[0], before.audit[0], "the Audit's check is the same one again");
});

test("a budget added below an agent's month spend joins the finding, and one above it does not", async () => {
  const { handle } = await freshDemo();
  const budgeted = new Set(engine.watch.budgets.map((b) => b.agent));
  const withSpend = Object.keys(engine.monthSpend).find((name) => !budgeted.has(name));
  assert.ok(withSpend && engine.monthSpend[withSpend] > 0, "the fixture has an agent with spend and no budget, or this test has nothing to add");
  const spent = engine.monthSpend[withSpend];
  const withBudget = (monthlyBudget) => ({ budgets: [...engine.watch.budgets, { agent: withSpend, monthlyBudget }] });
  const state = () => {
    const { inventory, audit } = budgetFindings(handle);
    return { count: inventory[0]?.titleMsg.count ?? 0, names: inventory[0]?.detailMsg.vars.names ?? "", audit: audit.length };
  };
  const engineNames = engine.budgets
    .filter((b) => b.monthToDate >= b.monthlyBudget)
    .map((b) => `${b.agent} ($${b.monthToDate.toFixed(2)} / $${b.monthlyBudget.toFixed(2)})`)
    .join(", ");
  assert.deepEqual(state(), { count: 1, names: engineNames, audit: 1 }, "the engine's own finding to start with");

  // Above its spend: no new finding. Exactly at its spend: it counts, as a client budget does.
  handle("set_agent_watch", { watch: withBudget(Math.ceil(spent) + 1) });
  assert.deepEqual(state(), { count: 1, names: engineNames, audit: 1 });
  handle("set_agent_watch", { watch: withBudget(spent) });
  assert.equal(state().count, 2, "a budget at the agent's spend is over");
  // Below it: named after the budgets saved before it, to the cent, in the Inventory and the Audit alike.
  handle("set_agent_watch", { watch: withBudget(0.5) });
  const now = state();
  assert.equal(now.count, 2);
  assert.equal(now.names, `${engineNames}, ${withSpend} ($${spent.toFixed(2)} / $0.50)`);
  assert.equal(now.audit, 1, "one check, not one per budget");
  const { inventory, audit } = budgetFindings(handle);
  assert.deepEqual([audit[0].titleMsg, audit[0].detailMsg, audit[0].title, audit[0].detail], [inventory[0].titleMsg, inventory[0].detailMsg, inventory[0].title, inventory[0].detail]);
});
