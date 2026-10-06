// The demo's "time at the limit" is not typed in: it is worked out from the same readings
// the Detail view charts (`readings` in src/demo/mock.ts), by the rule the app applies to
// its own history (`from_points` in crates/core/src/limit_time.rs), ported in
// src/demo/synthetic.ts as `timeAtLimit`. Three things are held here.
//
//  - The port. The cases below are the Rust tests' own, input for input and answer for
//    answer (`time_at_the_limit_skips_gaps_the_app_did_not_watch`, `a_span_ends_at_the_reset`,
//    `a_limit_never_reached_says_nothing`, and the other `from_points` tests whose inputs
//    JavaScript can hold), so that one side's rule cannot change without the other failing.
//    The one Rust test left out feeds it i64::MIN and i64::MAX, which a double cannot hold.
//  - The constants. AT_LIMIT, MAX_GAP_MS, WINDOW_MS and FINDING_AT_MS are read out of
//    limit_time.rs, compared with the port's own, and the edge cases below run on the Rust
//    gap, so a change to the rule in Rust fails here whichever constant it touches.
//  - The demo. What `get_limit_time` answers for the card is what the chart `get_history`
//    draws shows: a limit at 100 percent on the chart is a stretch counted in the figure,
//    from its first reading to the reset, and a limit that never reaches it has no row.
//
// The browser demo's module is loaded by scripts/demo-backend.mjs, with the real i18n and
// synthetic modules it imports.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { loadDemoBackend as loadFreshDemoBackend } from "./demo-backend.mjs";

const MIN = 60_000;
const HOUR = 60 * MIN;
const T0 = 1_790_000_000_000;

const limitTimeRust = await readFile(new URL("../crates/core/src/limit_time.rs", import.meta.url), "utf8");
const historyRust = await readFile(new URL("../crates/core/src/history.rs", import.meta.url), "utf8");

/** A numeric `const NAME: type = a * b * c;` out of a Rust source, evaluated: what a rule is made of. */
function rustConstant(source, name) {
  const found = source.match(new RegExp(`const ${name}:\\s*\\w+\\s*=\\s*([0-9_.\\s*]+);`));
  assert.ok(found, `could not find the constant ${name}`);
  const value = found[1].split("*").map((factor) => Number(factor.replace(/_/g, "").trim())).reduce((a, b) => a * b, 1);
  assert.ok(Number.isFinite(value), `${name} = ${found[1].trim()} is not a plain product of numbers`);
  return value;
}

// The Rust rule's constants, as its source states them.
const AT_LIMIT = rustConstant(limitTimeRust, "AT_LIMIT");
const MAX_GAP = rustConstant(limitTimeRust, "MAX_GAP_MS");
const WINDOW = rustConstant(limitTimeRust, "WINDOW_MS");
const FINDING_AT = rustConstant(limitTimeRust, "FINDING_AT_MS");

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

test("a stretch that runs to the last reading is counted whole", async () => {
  const { timeAtLimit } = await loadDemoBackend();
  // limit_time.rs, `a_stretch_that_runs_to_the_last_reading_is_counted_whole`: no later reading ends
  // the stretch, so what it held is taken when the readings run out.
  assert.deepEqual(figures(timeAtLimit("p", "Weekly", [pt(0, 100), pt(30, 100), pt(60, 100)])), [1, 60 * MIN, 60 * MIN]);
  // And it is the longest one when a shorter stretch came before it.
  const afterAShorter = [pt(0, 100), pt(30, 100), pt(60, 80), pt(90, 100), pt(120, 100), pt(150, 100)];
  assert.deepEqual(figures(timeAtLimit("p", "Weekly", afterAShorter)), [2, 90 * MIN, 60 * MIN]);
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

test("the port's constants are the Rust rule's, read from its source", async () => {
  const demo = await loadDemoBackend();
  assert.equal(demo.LIMIT_AT_PERCENT, AT_LIMIT, "AT_LIMIT");
  assert.equal(demo.LIMIT_MAX_GAP_MS, MAX_GAP, "MAX_GAP_MS");
  assert.equal(demo.LIMIT_WINDOW_MS, WINDOW, "WINDOW_MS");
  assert.equal(demo.LIMIT_TIME_FINDING_MS, FINDING_AT, "FINDING_AT_MS");
  // The rule runs on them: a reading at the limit counts and one a hair under does not, and a gap
  // of the Rust length counts while a millisecond more does not.
  const at = (used, atMs) => ({ at: atMs, used, resetsAt: null });
  assert.ok(demo.timeAtLimit("p", "m", [at(AT_LIMIT, 0)]), "a reading at the limit is a time");
  assert.equal(demo.timeAtLimit("p", "m", [at(AT_LIMIT - 0.001, 0)]), null, "a hair under is not");
  assert.equal(demo.timeAtLimit("p", "m", [at(AT_LIMIT, 0), at(AT_LIMIT, MAX_GAP)]).totalMs, MAX_GAP);
  assert.equal(demo.timeAtLimit("p", "m", [at(AT_LIMIT, 0), at(AT_LIMIT, MAX_GAP + 1)]).totalMs, 0);
});

test("the demo gives one limit over the finding's threshold and one under", async (t) => {
  const { handle } = await loadDemoBackend();
  setClock(t, NOON);
  const threshold = FINDING_AT;
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
  for (const [shorter, longer] of [[day, month]]) {
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

test("each earlier period climbs smoothly to a height of its own, and a short limit's unused windows leave gaps", async (t) => {
  const { handle } = await loadDemoBackend();
  setClock(t, NOON);
  const claude = handle("cached_usage").find((c) => c.id === "claude");
  const chart = handle("get_history", { providerId: "claude", hours: 24 * 30 });
  for (const series of chart) {
    const metric = claude.metrics.find((m) => m.label === series.metric);
    // Every reading of one period gives the same number here; a reset starts the next.
    const periodOf = (at) => Math.ceil((metric.resets_at - at) / metric.period_ms);
    // The most a reading can climb on the one before it inside a period: the steepest ramp (to a wall
    // that starts at 80 percent of the period at the earliest) over the widest gap between readings,
    // plus the scatter of two readings. A height drawn per reading, and not per period, jumps by tens.
    const steepest = (100 * 35 * MIN) / (0.8 * metric.period_ms) + 2.6;
    const highest = new Map();
    series.points.forEach((p, i) => {
      highest.set(periodOf(p.at), Math.max(highest.get(periodOf(p.at)) ?? 0, p.used));
      const before = series.points[i - 1];
      if (before && periodOf(before.at) === periodOf(p.at)) {
        assert.ok(Math.abs(p.used - before.used) <= steepest, `${series.metric}: ${before.used} then ${p.used} inside one period (at most ${steepest.toFixed(1)})`);
      }
    });
    // Whole earlier periods only: the live one is still climbing, and the oldest is cut by the range.
    const whole = [...highest.entries()].filter(([period]) => period > 1 && period < Math.max(...highest.keys()));
    const unused = whole.filter(([, top]) => top < 12).length;
    const climbed = whole.filter(([, top]) => top > 45).length;
    if (metric.period_ms < 24 * HOUR) {
      assert.ok(unused >= 20 && climbed >= 20, `${series.metric}: ${unused} windows unused, ${climbed} climbed: a line with gaps needs both`);
    } else {
      assert.equal(unused, 0, `${series.metric}: a ${metric.period_ms / HOUR}-hour period is always used`);
    }
  }
});

test("a long range is thinned to the app's cap, keeping real readings and the newest", async (t) => {
  const { handle, HISTORY_MAX_POINTS } = await loadDemoBackend();
  setClock(t, NOON);
  assert.equal(HISTORY_MAX_POINTS, rustConstant(historyRust, "MAX_POINTS"), "MAX_POINTS in crates/core/src/history.rs");
  const month = handle("get_history", { providerId: "claude", hours: 24 * 30 });
  const quarter = handle("get_history", { providerId: "claude", hours: 24 * 90 });
  const live = handle("cached_usage").find((c) => c.id === "claude").metrics;
  for (const series of quarter) {
    const name = series.metric;
    assert.ok(series.points.length <= HISTORY_MAX_POINTS + 1, `${name}: ${series.points.length} points`);
    assert.ok(series.points.length > month.find((m) => m.metric === name).points.length * 0.9, `${name}: thinned, not cut down to a stub`);
    series.points.slice(1).forEach((p, i) => assert.ok(p.at > series.points[i].at, `${name}: in time order`));
    const last = series.points[series.points.length - 1];
    assert.equal(last.at, NOON, `${name}: the newest reading is kept`);
    assert.equal(last.used, live.find((m) => m.label === name).used_percent);
    // Thinning only drops readings: every point inside the last 30 days is one the 30-day series has, unchanged.
    const inMonth = new Map(month.find((m) => m.metric === name).points.map((p) => [p.at, p.used]));
    const start = month.find((m) => m.metric === name).points[0].at;
    for (const p of series.points.filter((q) => q.at >= start)) assert.equal(inMonth.get(p.at), p.used, `${name}: a reading at ${p.at} the 30-day series does not have`);
  }
});

test("a long range is thinned as the app thins it", async () => {
  const { thin, HISTORY_MAX_POINTS } = await loadDemoBackend();
  // crates/core/src/history.rs, `long_ranges_are_thinned_and_keep_the_newest_reading`.
  const points = Array.from({ length: 4_000 }, (_, i) => ({ at: i, used: i }));
  const thinned = thin(points);
  assert.ok(thinned.length <= HISTORY_MAX_POINTS + 1, String(thinned.length));
  assert.equal(thinned[0].at, 0);
  assert.equal(thinned.at(-1).at, 3_999);
  assert.deepEqual(thinned.slice(0, 4).map((p) => p.at), [0, 3, 6, 9], "every ceil(4000 / 1500) = 3rd reading");
  assert.equal(points.length, 4_000, "the array passed in is not touched");
  // At the cap nothing is dropped; one over it, every second reading goes and the newest stays.
  assert.equal(thin(points.slice(0, HISTORY_MAX_POINTS)).length, HISTORY_MAX_POINTS);
  const over = thin(points.slice(0, HISTORY_MAX_POINTS + 1));
  assert.deepEqual([over.length, over[1].at, over.at(-1).at], [751, 2, HISTORY_MAX_POINTS]);
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

// ---------------------------------------------------------------------------
// The finding
// ---------------------------------------------------------------------------
//
// `buildLimitTimeRow` (src/demo/synthetic.ts) mirrors `limit_time::opportunities` and `messages`
// for a machine whose limit history the engine cannot read. The cases below are the Rust tests'
// own, with the same numbers and the same English, so the two cannot change apart.

/** A row as the Rust tests build one: two times, the longest stretch half the total. */
const lt = (provider, metric, totalMs) => ({ provider, metric, times: 2, totalMs, longestMs: Math.floor(totalMs / 2) });
const stretch = (provider, metric, totalMs, longestMs) => ({ provider, metric, times: 1, totalMs, longestMs });

test("a duration is chosen by size, as the app's own is", async () => {
  const { durationMsg } = await loadDemoBackend();
  // crates/core/src/i18n.rs, `duration_msg_picks_the_unit_by_size`: the key and the vars, by name.
  const shape = (minutes) => {
    const m = durationMsg(minutes);
    return `${m.key} ${Object.keys(m.vars).sort().map((k) => `${k}=${m.vars[k]}`).join(",")}`;
  };
  assert.equal(shape(0), "time.mins m=1", "under a minute reads as one");
  assert.equal(shape(59), "time.mins m=59");
  assert.equal(shape(60), "time.hoursMins h=1,m=0");
  assert.equal(shape(1439), "time.hoursMins h=23,m=59");
  assert.equal(shape(1440), "time.daysHours d=1,h=0");
  assert.equal(shape(59.9), "time.mins m=59", "whole minutes: the rest is dropped");
  assert.equal(shape(0.4), "time.mins m=1", "and a fraction of a minute reads as one");
  assert.equal(durationMsg(130).count, null, "no count: it is a length, not a quantity");
});

test("the finding needs the threshold the Rust rule has, to the minute", async () => {
  const { buildLimitTimeRow, LIMIT_TIME_FINDING_MS } = await loadDemoBackend();
  assert.equal(LIMIT_TIME_FINDING_MS, FINDING_AT, "the same figure as FINDING_AT_MS in crates/core/src/limit_time.rs");
  assert.equal(LIMIT_TIME_FINDING_MS, 2 * HOUR, "the strings say two hours in words");
  assert.equal(buildLimitTimeRow([]), null);
  assert.equal(buildLimitTimeRow([lt("claude", "Weekly", LIMIT_TIME_FINDING_MS - 1)]), null, "a millisecond under says nothing");
  assert.equal(buildLimitTimeRow([lt("claude", "Weekly", LIMIT_TIME_FINDING_MS - MIN)]), null, "a minute under says nothing");
  const found = buildLimitTimeRow([lt("claude", "Weekly", LIMIT_TIME_FINDING_MS)]);
  assert.ok(found, "at the threshold is a finding");
  assert.deepEqual([found.id, found.kind, found.learnUrl], ["limit-time", "learn", null]);
  assert.equal(found.title, "1 limit was fully used for two hours or more in the last 30 days");
});

test("the finding counts the limits over and details the longest", async () => {
  const { buildLimitTimeRow } = await loadDemoBackend();
  const rows = [lt("claude", "Weekly", 3 * HOUR), lt("codex", "Session", 5 * HOUR + 10 * MIN), lt("claude", "Session", HOUR)];
  const found = buildLimitTimeRow(rows);
  assert.deepEqual(found.titleMsg, { key: "finding.limit-time.title", vars: {}, count: 2 }, "two limits are over; the third is not");
  assert.equal(found.title, "2 limits were fully used for two hours or more in the last 30 days");
  assert.equal(
    found.detail,
    "Over the last 30 days, the limit that stayed fully used longest was that way for 5h 10m (2 times); its longest stretch was 2h 35m. While a limit is fully used, that tool cannot be used on your plan. Each provider's page shows which limit and when.",
  );
  assert.deepEqual(found.detailMsg.vars.times, { key: "unit.times", vars: {}, count: 2 }, "the count is a Msg, so each language words it");
  assert.equal(found.detailMsg.vars.total.key, "time.hoursMins");
  assert.equal(found.detailMsg.vars.longest.key, "time.hoursMins");
});

test("the finding's title carries a count only", async () => {
  const { buildLimitTimeRow } = await loadDemoBackend();
  const found = buildLimitTimeRow([lt("northwind-seat-card@ab12cd34", "Northwind Reviewer weekly", 9 * HOUR)]);
  assert.ok(found, "the finding has to exist, or this proves nothing");
  assert.deepEqual(found.titleMsg.vars, {}, "a title carries a count only");
  for (const text of [found.title, found.detail]) assert.ok(!/northwind/i.test(text), text);
});

test("ties pick the same limit every time", async () => {
  const { buildLimitTimeRow, limitTimeOrder } = await loadDemoBackend();
  const rows = [
    stretch("b", "Weekly", 3 * HOUR, HOUR),
    stretch("a", "Weekly", 3 * HOUR, HOUR),
    stretch("a", "Session", 3 * HOUR, HOUR),
    stretch("z", "Session", 3 * HOUR, 2 * HOUR),
    stretch("z", "Weekly", 4 * HOUR, 0),
  ];
  const ordered = [...rows].sort(limitTimeOrder).map((r) => [r.provider, r.metric]);
  assert.deepEqual(ordered, [["z", "Weekly"], ["z", "Session"], ["a", "Session"], ["a", "Weekly"], ["b", "Weekly"]]);
  // The finding takes the first of that order whichever way the rows arrive.
  assert.deepEqual(buildLimitTimeRow(rows).detailMsg, buildLimitTimeRow([...rows].reverse()).detailMsg);
  const tied = [stretch("b", "Weekly", 3 * HOUR, HOUR), stretch("a", "Weekly", 3 * HOUR, 2 * HOUR)];
  assert.deepEqual(buildLimitTimeRow(tied).detailMsg, buildLimitTimeRow([tied[1], tied[0]]).detailMsg);
  assert.equal(Object.keys(buildLimitTimeRow(tied).detailMsg.vars).length, 3);
});

test("the finding reads whole in every language and size", async () => {
  const { buildLimitTimeRow, render, LOCALES } = await loadDemoBackend();
  const durations = [["2 hours 10 minutes", 2 * HOUR + 10 * MIN], ["1 day 3 hours", 27 * HOUR], ["5 days", 120 * HOUR]];
  for (const locale of LOCALES) {
    for (const [name, total] of durations) {
      for (const times of [1, 2, 5, 21]) {
        const found = buildLimitTimeRow([{ provider: "p", metric: "m", times, totalMs: total, longestMs: Math.floor(total / 3) }]);
        const detail = render(locale, found.detailMsg);
        const title = render(locale, found.titleMsg);
        assert.ok(!/[{}]/.test(detail) && !/finding\.|unit\.|time\./.test(detail), `${locale} ${name} ${times}: ${detail}`);
        assert.ok(!/[{}]/.test(title) && !title.includes("finding."), `${locale}: ${title}`);
        if (locale !== "en") {
          assert.ok(!detail.includes("While a limit is fully used"), `${locale} detail still reads in English: ${detail}`);
          assert.notEqual(title, render("en", found.titleMsg), `${locale} title still reads in English`);
        }
      }
    }
  }
});

test("the finding has no Learn more link, as the Rust one has none", () => {
  assert.match(limitTimeRust, /Opportunity::from_msgs\("limit-time", "learn", title, Some\(detail\), None\)/, "limit_time.rs now builds the finding with a link");
});

test("the demo's own readings make exactly one finding, in the Inventory and in the Audit", async (t) => {
  const { handle, durationMsg, limitTimeOrder, render, LOCALES } = await loadDemoBackend();
  setClock(t, NOON);
  const threshold = FINDING_AT;
  // Every card's rows, as the app's finding reads them: the same ones the Detail view's lines are made of.
  const cards = handle("cached_usage").map((c) => c.id);
  const rows = cards.flatMap((providerId) => handle("get_limit_time", { providerId }));
  const over = rows.filter((r) => r.totalMs >= threshold);
  assert.equal(over.length, 1, "one limit is over the threshold, which is what this machine shows");
  const top = [...over].sort(limitTimeOrder)[0];

  const inventory = handle("get_inventory");
  const found = inventory.opportunities.filter((o) => o.id === "limit-time");
  assert.equal(found.length, 1, "exactly one row, however often the Inventory is read");
  assert.equal(handle("get_inventory").opportunities.filter((o) => o.id === "limit-time").length, 1);
  const [row] = found;
  assert.deepEqual([row.kind, row.learnUrl], ["learn", null]);
  assert.equal(row.titleMsg.count, over.length);
  assert.deepEqual(row.detailMsg.vars.total, durationMsg(Math.floor(top.totalMs / MIN)), "the longest limit's total");
  assert.deepEqual(row.detailMsg.vars.times, { key: "unit.times", vars: {}, count: top.times });
  assert.deepEqual(row.detailMsg.vars.longest, durationMsg(Math.floor(top.longestMs / MIN)));
  // A learn finding is listed after the tighten ones, as the app sorts them; this one is the last.
  const kinds = inventory.opportunities.map((o) => o.kind);
  assert.ok(kinds.includes("tighten"), "there are tighten findings, or the order proves nothing");
  assert.ok(kinds.slice(kinds.indexOf("learn")).every((k) => k === "learn"), kinds.join(","));
  assert.equal(inventory.opportunities.at(-1).id, "limit-time");

  // The Audit: the same words as one check of the Usage section, unscored, where the engine puts it.
  const report = handle("get_audit");
  const usage = report.sections.find((s) => s.nameKey === "section.usage");
  const ids = usage.checks.map((c) => c.id);
  assert.equal(ids.filter((id) => id === "limit-time").length, 1);
  const check = usage.checks.find((c) => c.id === "limit-time");
  assert.deepEqual(check, { id: "limit-time", status: "consider", title: row.title, detail: row.detail, titleMsg: row.titleMsg, detailMsg: row.detailMsg });
  assert.equal(ids[ids.indexOf("limit-time") - 1], "agent-over-budget", "straight after the engine's own present-only checks");
  assert.equal(ids[ids.indexOf("limit-time") + 1], "mix-top-heavy", "ahead of the checks that depend on spend");
  const fixtureAudit = JSON.parse(await readFile(new URL("../src/demo-fixture.json", import.meta.url), "utf8")).audit;
  assert.deepEqual([report.passed, report.attention, report.score], [fixtureAudit.passed, fixtureAudit.attention, fixtureAudit.score], "unscored");

  // The report is a copy: nothing done to it, and nothing the app does on a second read, reaches the fixture.
  usage.checks.length = 0;
  report.sections.length = 0;
  const again = handle("get_audit").sections.find((s) => s.nameKey === "section.usage");
  assert.equal(again.checks.filter((c) => c.id === "limit-time").length, 1, "once, not once more for every read");
  assert.equal(again.checks.length, ids.length);

  // Both Msgs, nested ones included, read whole in every language.
  for (const locale of LOCALES) {
    for (const [label, msg] of [["title", row.titleMsg], ["detail", row.detailMsg], ["total", row.detailMsg.vars.total], ["times", row.detailMsg.vars.times]]) {
      const text = render(locale, msg);
      assert.ok(!/[{}]/.test(text), `${locale} ${label} left a {var} unfilled: ${text}`);
      assert.notEqual(text, msg.key, `${locale} ${label} rendered as its own key`);
    }
  }
});

test("the audit carries the findings the demo built from its state, and the engine's checks for them go", async () => {
  const { auditWithFindings, buildLimitTimeRow, buildOverBudgetRow } = await loadDemoBackend();
  const fixtureAudit = JSON.parse(await readFile(new URL("../src/demo-fixture.json", import.meta.url), "utf8")).audit;
  const idsOf = (report) => report.sections.flatMap((s) => s.checks).map((c) => c.id);
  assert.ok(idsOf(fixtureAudit).includes("agent-over-budget"), "the fixture has the engine's check, or this test proves nothing");

  // Nothing built: the engine's frozen checks for the two are dropped, and everything else is as it was.
  const none = auditWithFindings(fixtureAudit, [null, null]);
  assert.deepEqual(idsOf(none), idsOf(fixtureAudit).filter((id) => id !== "agent-over-budget"));
  assert.notStrictEqual(none, fixtureAudit, "a copy");
  assert.ok(idsOf(fixtureAudit).includes("agent-over-budget"), "the argument is never written to");

  // Both built: each once, unscored, where the engine puts them, and asking again changes nothing.
  const over = buildOverBudgetRow([{ agent: "general-purpose", monthToDate: 14.7362, monthlyBudget: 10 }]);
  const limit = buildLimitTimeRow([{ provider: "claude", metric: "Weekly", times: 3, totalMs: 5 * HOUR, longestMs: 3 * HOUR }]);
  const both = auditWithFindings(fixtureAudit, [over, limit]);
  const usage = both.sections.find((s) => s.nameKey === "section.usage").checks;
  assert.deepEqual(usage.slice(0, 3).map((c) => [c.id, c.status]), [["pricing-cache-ttl", "pass"], ["agent-over-budget", "consider"], ["limit-time", "consider"]]);
  assert.equal(idsOf(both).filter((id) => id === "agent-over-budget" || id === "limit-time").length, 2);
  assert.deepEqual(auditWithFindings(both, [over, limit]), both);
  // The score does not move: a "consider" check is unscored.
  assert.deepEqual([both.passed, both.attention, both.score], [fixtureAudit.passed, fixtureAudit.attention, fixtureAudit.score]);
  // A finding the demo does not build is not added, whatever the report held before.
  const onlyLimit = auditWithFindings(both, [null, limit]);
  assert.deepEqual(idsOf(onlyLimit).filter((id) => id === "agent-over-budget" || id === "limit-time"), ["limit-time"]);
});

test("the Audit opens the Usage tab for the time-at-the-limit check", async () => {
  // src/audit.ts's WHERE map is what puts an "Open ..." link under a check; a finding missing from
  // it shows a row with nowhere to go.
  const source = await readFile(new URL("../src/audit.ts", import.meta.url), "utf8");
  assert.match(source, /"limit-time": "usage"/);
  assert.match(source, /"agent-over-budget": "agents"/);
});
