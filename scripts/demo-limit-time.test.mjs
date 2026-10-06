// The demo's "time at the limit" is not typed in: it is worked out from the same readings
// the Detail view charts (`readings` in src/demo/mock.ts), by the rule the app applies to
// its own history (`from_points` in crates/core/src/limit_time.rs), ported there as
// `timeAtLimit`. Two things are held here.
//
//  - The port. The cases below are the Rust tests' own, input for input and answer for
//    answer (`time_at_the_limit_skips_gaps_the_app_did_not_watch`, `a_span_ends_at_the_reset`,
//    `a_limit_never_reached_says_nothing`, and the other `from_points` tests whose inputs
//    JavaScript can hold), so that one side's rule cannot change without the other failing.
//    The one Rust test left out feeds it i64::MIN and i64::MAX, which a double cannot hold.
//  - The demo. What `get_limit_time` answers for the card is what the chart `get_history`
//    draws shows: a limit at 100 percent on the chart is a stretch counted in the figure,
//    from its first reading to the reset, and a limit that never reaches it has no row.
//
// The browser demo's module is loaded by scripts/demo-backend.mjs, with inert stand-ins for the
// imports none of these commands reaches.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { loadDemoBackend as loadFreshDemoBackend } from "./demo-backend.mjs";

const MIN = 60_000;
const HOUR = 60 * MIN;
const T0 = 1_790_000_000_000;
// limit_time.rs: MAX_GAP_MS, 90 minutes.
const MAX_GAP = 90 * MIN;

let demoModule = null;
async function loadDemoBackend() {
  if (!demoModule) demoModule = loadFreshDemoBackend();
  return demoModule;
}

/** A reading `atMin` minutes after T0, carrying no reset. */
const pt = (atMin, used) => ({ at: T0 + atMin * MIN, used, resetsAt: null });
/** A reading that carries the reset time `resetMin` minutes after T0. */
const ptReset = (atMin, used, resetMin) => ({ at: T0 + atMin * MIN, used, resetsAt: T0 + resetMin * MIN });
/** What the three figures of `from_points`' answer are, or null for "never reached". */
const figures = (row) => (row === null ? null : [row.times, row.totalMs, row.longestMs]);

test("time at the limit skips gaps the app did not watch", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  const points = [
    pt(0, 100),
    pt(60, 100), // 60 min counted
    pt(120, 100), // 60 more
    pt(400, 100), // 280 min later: the app was not watching, nothing counted
    pt(460, 100), // 60 more, a new stretch
  ];
  const got = timeAtLimit("p", "Weekly", points);
  assert.equal(got.totalMs, 180 * MIN);
  assert.equal(got.times, 2, "the unwatched gap ends the stretch");
  assert.equal(got.longestMs, 120 * MIN);
  assert.deepEqual([got.provider, got.metric], ["p", "Weekly"]);
  // Exactly at the gap counts; one millisecond over does not.
  const edge = [{ at: 0, used: 100, resetsAt: null }, { at: MAX_GAP, used: 100, resetsAt: null }];
  assert.equal(timeAtLimit("p", "m", edge).totalMs, MAX_GAP);
  const over = [{ at: 0, used: 100, resetsAt: null }, { at: MAX_GAP + 1, used: 100, resetsAt: null }];
  assert.equal(timeAtLimit("p", "m", over).totalMs, 0);
});

test("a span ends at the reset", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  // At the limit at minute 0 with a reset at minute 40; the next reading, at 60, is back
  // down. The limit held for 40 minutes.
  assert.deepEqual(figures(timeAtLimit("p", "Session", [ptReset(0, 100, 40), pt(60, 3)])), [1, 40 * MIN, 40 * MIN]);
  // Still at the limit after the reset: that reading starts a new stretch.
  const again = timeAtLimit("p", "Session", [ptReset(0, 100, 40), pt(60, 100), pt(100, 100)]);
  assert.equal(again.times, 2);
  assert.equal(again.totalMs, 40 * MIN + 40 * MIN, "40 to the reset, then 40 from the new stretch");
  assert.equal(again.longestMs, 40 * MIN);
  // A reset at the later reading's own instant, or already past, is not between them.
  assert.equal(timeAtLimit("p", "s", [ptReset(0, 100, 60), pt(60, 100)]).totalMs, 60 * MIN);
  assert.equal(timeAtLimit("p", "s", [ptReset(30, 100, 10), pt(60, 100)]).totalMs, 30 * MIN);
});

test("a limit never reached says nothing", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  assert.equal(timeAtLimit("p", "Weekly", []), null);
  assert.equal(timeAtLimit("p", "Weekly", [pt(0, 40), pt(30, 99.9), pt(60, 12)]), null);
  assert.equal(timeAtLimit("p", "Weekly", [pt(0, NaN)]), null);
});

test("two runs at the limit count as two times", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  const points = [
    pt(0, 100),
    pt(30, 100), // 30
    ptReset(60, 100, 70), // 30 more
    pt(90, 20), // reset at 70: 10 more, then it is below the limit
    pt(120, 50),
    pt(150, 100), // a second run
    pt(200, 100), // 50
  ];
  // The first run held 30 + 30 + 10, the second only 50.
  assert.deepEqual(figures(timeAtLimit("p", "Weekly", points)), [2, 120 * MIN, 70 * MIN]);
});

test("a drop below the limit with no reset counts nothing", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  // The last half hour is unknown, not counted.
  assert.deepEqual(figures(timeAtLimit("p", "Weekly", [pt(0, 100), pt(30, 100), pt(60, 80)])), [1, 30 * MIN, 30 * MIN]);
  // A reset time that is not between the two does not rescue it.
  assert.equal(timeAtLimit("p", "Weekly", [ptReset(0, 100, 90), pt(60, 80)]).totalMs, 0);
});

test("a single reading at the limit is one time and no time", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  assert.deepEqual(figures(timeAtLimit("p", "Weekly", [pt(0, 100)])), [1, 0, 0]);
  const beside = timeAtLimit("p", "Weekly", [pt(0, 10), pt(30, 100), pt(60, 10)]);
  assert.deepEqual([beside.times, beside.totalMs], [1, 0]);
});

test("a reset on either reading is not between them", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  // Reset at A's own instant: not strictly after it, so the whole gap counts.
  assert.equal(timeAtLimit("p", "s", [ptReset(0, 100, 0), pt(60, 100)]).totalMs, 60 * MIN);
  // Reset at A's instant and B below: nothing is known, nothing counts.
  assert.equal(timeAtLimit("p", "s", [ptReset(0, 100, 0), pt(60, 50)]).totalMs, 0);
  // Reset at B's instant and B below: not strictly before B, so nothing counts.
  assert.equal(timeAtLimit("p", "s", [ptReset(0, 100, 60), pt(60, 50)]).totalMs, 0);
  // Readings exactly 90 minutes apart still count.
  assert.equal(timeAtLimit("p", "s", [pt(0, 100), pt(90, 100)]).totalMs, 90 * MIN);
});

test("a shuffled series gives a fixed answer", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  // Minutes 0, 30, 20, 20, 50 all at the limit, no resets. Pairs: 0->30 counts 30; 30->20
  // goes backwards and ends the run; 20->20 counts nothing; 20->50 counts 30.
  const points = [pt(0, 100), pt(30, 100), pt(20, 100), pt(20, 100), pt(50, 100)];
  assert.deepEqual(figures(timeAtLimit("p", "s", points)), [2, 60 * MIN, 30 * MIN]);
  // Disordered readings never count a negative time.
  const shuffled = [pt(60, 100), pt(0, 100), pt(30, 100), pt(30, 100), ptReset(40, 100, 5), pt(10, 100)];
  const got = timeAtLimit("p", "Weekly", shuffled);
  assert.ok(got.totalMs >= 0 && got.longestMs >= 0 && got.longestMs <= got.totalMs, JSON.stringify(got));
  assert.ok(got.times >= 1);
  assert.equal(timeAtLimit("p", "Weekly", [pt(5, 100), pt(5, 100)]).totalMs, 0);
});

// ---------------------------------------------------------------------------
// The demo's answer
// ---------------------------------------------------------------------------

/** The demo's clock, pinned to one instant. */
function setClock(t, ms) {
  t.mock.timers.reset();
  t.mock.timers.enable({ apis: ["Date"], now: ms });
}

const NOON = new Date(2026, 9, 6, 12, 0, 0).getTime();

/** The finding's threshold in the Rust source: a total at or over it makes the finding. */
async function findingThresholdMs() {
  const rust = await readFile(new URL("../crates/core/src/limit_time.rs", import.meta.url), "utf8");
  const found = rust.match(/const FINDING_AT_MS:\s*i64\s*=\s*(\d+)\s*\*\s*3_600_000;/);
  assert.ok(found, "could not find FINDING_AT_MS in crates/core/src/limit_time.rs");
  return Number(found[1]) * HOUR;
}

test("the demo gives one limit over the finding's threshold and one under", async (t) => {
  const { handle } = await loadDemoBackend();
  setClock(t, NOON);
  const threshold = await findingThresholdMs();
  const rows = handle("get_limit_time", { providerId: "claude" });
  assert.deepEqual(rows.map((r) => r.metric), ["Weekly", "Session"], "most time first; Opus weekly never reached it");
  const [weekly, session] = rows;
  assert.ok(weekly.totalMs >= threshold, `Weekly ${weekly.totalMs} is under the finding's ${threshold}`);
  assert.ok(session.totalMs > 0 && session.totalMs < threshold, `Session ${session.totalMs} is not under the finding's ${threshold}`);
  for (const row of rows) {
    assert.equal(row.provider, "claude");
    assert.ok(row.times >= 1 && row.longestMs > 0 && row.longestMs <= row.totalMs, JSON.stringify(row));
  }
  // The other cards never reached a limit.
  for (const providerId of ["codex", "cursor", "copilot", "nobody"]) {
    assert.deepEqual(handle("get_limit_time", { providerId }), [], providerId);
  }
});

test("what the figure counts is what the chart shows", async (t) => {
  const { handle } = await loadDemoBackend();
  setClock(t, NOON);
  const cards = handle("cached_usage");
  const claude = cards.find((c) => c.id === "claude");
  const rows = handle("get_limit_time", { providerId: "claude" });
  const chart = handle("get_history", { providerId: "claude", hours: 24 * 30 });
  const progress = claude.metrics.filter((m) => m.kind === "progress");
  assert.deepEqual(chart.map((s) => s.metric), progress.map((m) => m.label), "one series per progress limit");

  for (const series of chart) {
    const metric = progress.find((m) => m.label === series.metric);
    // Every reading is within the pairing rule's gap: a longer one would count no time at all.
    series.points.slice(1).forEach((p, i) => {
      assert.ok(p.at - series.points[i].at <= MAX_GAP, `${series.metric}: readings ${p.at - series.points[i].at} ms apart`);
    });
    // The reset a reading carried: the end of the period it falls in.
    const resetOf = (at) => metric.resets_at - metric.period_ms * (Math.ceil((metric.resets_at - at) / metric.period_ms) - 1);
    // Runs of consecutive readings at 100 percent, as the chart draws them.
    const runs = [];
    let open = null;
    for (const p of series.points) {
      if (p.used < 100) open = null;
      else if (open) open.last = p;
      else runs.push((open = { first: p, last: p }));
    }
    const row = rows.find((r) => r.metric === series.metric);
    if (!runs.length) {
      assert.equal(row, undefined, `${series.metric} never reaches 100 percent on the chart, so it has no figure`);
      continue;
    }
    assert.ok(row, `${series.metric} reaches 100 percent on the chart but has no figure`);
    // Each run is a stretch that held until its reset: from its first reading to the reset.
    const held = runs.map((run) => resetOf(run.last.at) - run.first.at);
    assert.equal(row.times, runs.length, `${series.metric}: stretches on the chart`);
    assert.equal(row.totalMs, held.reduce((a, b) => a + b, 0), `${series.metric}: time on the chart`);
    assert.equal(row.longestMs, Math.max(...held), `${series.metric}: longest stretch on the chart`);
  }
});

test("a shorter range is the newest part of a longer one", async (t) => {
  const { handle } = await loadDemoBackend();
  setClock(t, NOON);
  const day = handle("get_history", { providerId: "claude", hours: 24 });
  const month = handle("get_history", { providerId: "claude", hours: 24 * 30 });
  const quarter = handle("get_history", { providerId: "claude", hours: 24 * 90 });
  for (const [shorter, longer] of [[day, month], [month, quarter]]) {
    shorter.forEach((series, i) => {
      assert.equal(series.metric, longer[i].metric);
      assert.ok(series.points.length >= 2 && series.points.length < longer[i].points.length, series.metric);
      assert.deepEqual(series.points, longer[i].points.slice(-series.points.length), `${series.metric}: the same readings, not a redraw`);
    });
  }
  // The newest reading is the live value, now.
  const live = handle("cached_usage").find((c) => c.id === "claude").metrics;
  for (const series of day) {
    const last = series.points[series.points.length - 1];
    assert.equal(last.at, NOON);
    assert.equal(last.used, live.find((m) => m.label === series.metric).used_percent);
  }
});

test("the figures do not move with the clock", async (t) => {
  const { handle } = await loadDemoBackend();
  // Every period and every reading hangs off one clock reading, so a page that redraws a
  // minute later, or a day later, must say the same thing, to the millisecond.
  setClock(t, NOON);
  const expected = handle("get_limit_time", { providerId: "claude" });
  assert.ok(expected.length > 0);
  for (const later of [1, 7 * MIN, 3 * HOUR + 11, 26 * HOUR + 40 * MIN]) {
    setClock(t, NOON + later);
    assert.deepEqual(handle("get_limit_time", { providerId: "claude" }), expected, `${later} ms later`);
  }
});
