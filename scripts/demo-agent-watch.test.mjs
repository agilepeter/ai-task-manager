// The demo's Agents view budgets are not typed into src/demo/mock.ts: they are what the real
// engine answered for the fictional machine's own agent_watch.json (`live_agent_watch`, run by
// scripts/make-demo-fixture.py into src/demo-fixture.json). Held here, against that committed
// fixture itself: the saved budgets, the engine's calendar-month figure beside each, and the
// names a budget may take all come from it; and what a visitor does in the demo still behaves as
// it does in the app -- a saved budget keeps its figure, one with none reads $0, a name that is
// not offered is refused.
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
  assert.deepEqual(view.budgets, engine.budgets, "each budget against the calendar month, as the engine summed it");
  assert.deepEqual(view.known, engine.known, "the names a budget may take");
  assert.ok(view.budgets.some((b) => b.monthToDate >= b.monthlyBudget) && view.budgets.some((b) => b.monthToDate < b.monthlyBudget));
  // The live rule is unset, so nothing runs away; the hint is the demo's own running agent's pace.
  assert.deepEqual(view.runaways, []);
  assert.ok(view.liveHint > 0);
  // The view is a copy: painting it cannot change what is saved.
  view.watch.budgets.length = 0;
  assert.equal(handle("get_agent_watch").watch.budgets.length, engine.watch.budgets.length);
});

test("a budget saved in the demo keeps its figure, and one the fixture has none for reads zero", async () => {
  const { handle } = await freshDemo();
  const [first] = engine.watch.budgets;
  const other = engine.known.find((n) => !engine.watch.budgets.some((b) => b.agent === n));
  assert.ok(other, "the fixture offers a name with no budget, or this test has nothing to add");

  const saved = handle("set_agent_watch", {
    watch: { budgets: [{ agent: first.agent, monthlyBudget: 99 }, { agent: other, monthlyBudget: 7 }], live: { hourlyPaceUsd: 3, maxMinutes: 90 } },
  });
  const figureOf = (agent) => engine.budgets.find((b) => b.agent === agent)?.monthToDate ?? 0;
  assert.deepEqual(saved.budgets, [
    { agent: first.agent, monthToDate: figureOf(first.agent), monthlyBudget: 99 },
    { agent: other, monthToDate: 0, monthlyBudget: 7 },
  ]);
  assert.deepEqual(saved.watch.live, { hourlyPaceUsd: 3, maxMinutes: 90 });
  assert.deepEqual(handle("get_agent_watch"), saved, "a later read says what the save said");
  assert.equal(engine.watch.budgets.find((b) => b.agent === first.agent).monthlyBudget, 10, "the fixture itself is never written to");

  // Clearing leaves no budgets and every name still offered.
  const cleared = handle("set_agent_watch", { watch: {} });
  assert.deepEqual([cleared.budgets, cleared.watch.live], [[], { hourlyPaceUsd: null, maxMinutes: null }]);
  assert.deepEqual(cleared.known, engine.known);
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
