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

/** A fresh page load of the demo, over the real fixture's agent watch alone. */
const freshDemo = () => loadDemoBackend({ agentWatch: engine });

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
  assert.equal(refused([{ agent: "an-agent-nobody-has", monthlyBudget: 5 }]), "error.agentWatch.pick", "a name that is not offered");
  assert.equal(refused([{ agent: first.agent, monthlyBudget: 5 }, { agent: first.agent, monthlyBudget: 6 }]), "error.agentWatch.duplicate");
  assert.equal(refused([{ agent: first.agent, monthlyBudget: 0 }]), "error.agentWatch.figure", "a figure of zero");
  assert.equal(refused([{ agent: first.agent, monthlyBudget: 1_000_001 }]), "error.agentWatch.figure", "a figure above the cap");
  assert.equal(refused([{ agent: engine.known[0], monthlyBudget: 5 }]), null, "a name the engine offers is accepted");
  // A save that failed changed nothing.
  assert.deepEqual(handle("get_agent_watch").watch.budgets, [{ agent: engine.known[0], monthlyBudget: 5 }]);
});
