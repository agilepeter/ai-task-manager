// A client's month to date is worked out once, by the scan (crates/core/src/spend.rs),
// from the whole per-day map: a 30-slot daily series is a day short on the 31st of a
// 31-day month, so the frontend must never rebuild it from `daily_cost` when the scan
// has already given the figure. The figure is `{ month: "YYYY-MM", cost }`: it is a
// number fixed at scan time, so it says which month it is for. The app uses it only
// while that is the current month and counts one for another month as zero, because a
// scan from an earlier month holds none of this month's spend. Two places in the
// frontend handle it:
//
//  - the detail page hands the areas it received straight back to the backend, and a
//    copy rebuilt field by field would drop `month_to_date` and send the backend down
//    its fallback. Only the `client_rollup` path is driven here (`loadClients`); the
//    export button calls `export_clients_csv` with the same `allAreas(sp)`, so what is
//    held is the one function both callers share;
//  - the browser demo's stand-in for `client_rollup` (src/demo/mock.ts) uses the figure
//    when it is for the current month and otherwise cuts its own month out of the series,
//    as it does for an area with no figure (the committed fixture predates the field).
//    It differs from the app on purpose: the app has a scan that can be stale, the demo
//    has none, its fixture is frozen at one month and its series is anchored to the
//    viewer's today, so a label from another month says nothing about this one, and
//    zeroing on it would show "$0 of $600" beside bars of hundreds.
//
// Same combined-module technique as scripts/detail-focus.test.mjs.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

const win = (cost) => ({ cost, tokens: cost * 10, cache_read: 0, models: [] });
/** What the scan sends for a work area: dollars per day over 30 slots, today last. */
const area = (name, extra = {}) => ({
  area: name,
  today: win(1),
  yesterday: win(1),
  last30: win(30),
  daily_cost: Array(30).fill(1),
  week: null,
  ...extra,
});

// ---------------------------------------------------------------------------
// The demo's stand-in backend
// ---------------------------------------------------------------------------

let demoModule = null;
async function loadDemoBackend() {
  if (!demoModule) demoModule = buildDemoBackend();
  return demoModule;
}

async function buildDemoBackend() {
  const source = await readFile(new URL("../src/demo/mock.ts", import.meta.url), "utf8");
  // None of the three imports is reachable from `client_rollup`: the fixture and the two
  // synthetic-finding builders are read only by other commands, and `t` only by the
  // agent-watch validation, so each becomes an inert stand-in.
  const stripped = source
    .replace('import fixture from "../demo-fixture.json";', "const fixture = {};")
    .replace('import { t } from "../i18n";', "const t = (key) => key;")
    .replace(
      'import { buildDuplicateProcessesRow, buildUsageRows } from "./synthetic";',
      "const buildDuplicateProcessesRow = () => null;\nconst buildUsageRows = () => [];",
    );
  if (stripped === source) throw new Error("no substitution matched -- src/demo/mock.ts's imports moved under this test");
  // The build hands the demo its version through a `declare const`, which transpiling erases.
  const code = ts.transpileModule(`const __APP_VERSION__ = "test";\n${stripped}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

/** The demo's clock, pinned: local noon on the given day. */
function setClock(t, year, month, day) {
  t.mock.timers.reset();
  t.mock.timers.enable({ apis: ["Date"], now: new Date(year, month - 1, day, 12, 0, 0).getTime() });
}

test("the demo rollup uses the month figure the scan computed", async (t) => {
  const { handle } = await loadDemoBackend();
  // October 15: a 30-slot series of one-dollar days then holds fifteen dollars in October
  // (the 1st through the 15th), which is what the demo's own cut reports, so it is easy to
  // tell apart from the figures given below.
  setClock(t, 2026, 10, 15);
  const rows = (areas) => Object.fromEntries(handle("client_rollup", { areas }).rows.map((r) => [r.client, r]));
  const october = (cost) => ({ month: "2026-10", cost });

  // The scan's figure for this month is added up as it is: not the series' cut (15 each).
  const given = rows([area("acme-portal", { month_to_date: october(123.5) }), area("acme-portal/web", { month_to_date: october(6.5) })]);
  assert.equal(given["Acme Co"].monthToDate, 130);

  // Zero is a figure too: a month that has spent nothing is not a missing one.
  assert.equal(rows([area("acme-portal", { month_to_date: october(0) })])["Acme Co"].monthToDate, 0);

  // An area with no figure (the committed fixture has none yet), or a null one, keeps
  // the demo's own cut of the series.
  const missing = rows([area("northwind-api"), area("acme-portal", { month_to_date: null })]);
  assert.equal(missing["Northwind"].monthToDate, 15);
  assert.equal(missing["Acme Co"].monthToDate, 15);

  // A figure for another month is not this month's, however it got here, and the demo reads
  // its series instead, as for an area with no figure: 15 each, 30 for the two. Neither
  // September's 999 nor November's 5 is used, and nothing is zeroed on a label (the app
  // would count both as zero; see the header for why the demo does not).
  const other = rows([
    area("acme-portal", { month_to_date: { month: "2026-09", cost: 999 } }),
    area("acme-portal/web", { month_to_date: { month: "2026-11", cost: 5 } }),
  ]);
  assert.equal(other["Acme Co"].monthToDate, 30);

  // A value that is not a figure at all, a bare number say, takes the cut too.
  const bare = rows([area("northwind-api", { month_to_date: 999 })]);
  assert.equal(bare["Northwind"].monthToDate, 15);

  // The engine writes the month as two digits, and a single-digit month must still match.
  setClock(t, 2026, 3, 5);
  const march = rows([area("acme-portal", { month_to_date: { month: "2026-03", cost: 77 } })]);
  assert.equal(march["Acme Co"].monthToDate, 77);
});

// ---------------------------------------------------------------------------
// The detail page
// ---------------------------------------------------------------------------

let detailModule = null;
async function loadDetailModule() {
  if (!detailModule) detailModule = buildDetailModule();
  return detailModule;
}

async function buildDetailModule() {
  const i18nSource = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const inlinedI18n = await inlineLocaleImports(i18nSource, new URL("../src/locales/", import.meta.url));
  const formatSource = await readFile(new URL("../src/format.ts", import.meta.url), "utf8");
  const strippedFormat = formatSource.replace('import { localeTag, plural, t } from "./i18n";', "");
  if (strippedFormat === formatSource) throw new Error("src/format.ts's import line moved under this test");
  const focusSource = await readFile(new URL("../src/focus.ts", import.meta.url), "utf8");
  const panelsSource = await readFile(new URL("../src/panels.ts", import.meta.url), "utf8");
  const detailSource = await readFile(new URL("../src/detail.ts", import.meta.url), "utf8");
  const strippedDetail = detailSource
    .replace(
      'import { invoke } from "@tauri-apps/api/core";',
      'export const __invoke = { current: async () => { throw new Error("invoke() is not stubbed in this test"); } };\nconst invoke = (...args) => __invoke.current(...args);',
    )
    .replace('import { focusOrFallback } from "./focus";', "")
    .replace('import { focusAfterClose, isTopPanel, syncPanels } from "./panels";', "")
    .replace('import { displayMetricDetail, displayMetricLabel, localeTag, plural, t } from "./i18n";', "")
    .replace('import { money, relativeActivity, tokens } from "./format";', "")
    .replace(/\brender\b/g, "__detailRender");
  if (strippedDetail === detailSource) throw new Error("src/detail.ts's source shape moved under this test");
  // Opening a card is a click through the whole page's DOM. What this test needs of it is
  // only the two module variables it sets, so the test's own copy of the module gets a
  // way to set them, and a way to call the one function that reads them.
  const hooks = [
    "export { loadClients as __loadClients };",
    "export const __opened = { set(id, src) { openId = id; source = src; } };",
  ].join("\n");
  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${focusSource}\n${panelsSource}\n${strippedDetail}\n${hooks}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

test("areas go back to the client rollup with every field they came with", async () => {
  const { __invoke, __loadClients, __opened } = await loadDetailModule();
  // Two projects, so the areas are gathered across them. `month_to_date` is the field the
  // backend needs, month and all; `from_a_newer_build` stands for whichever field comes
  // next, which a copy rebuilt field by field would drop the same way.
  const acme = area("acme-portal", { month_to_date: { month: "2026-10", cost: 24 }, from_a_newer_build: "kept" });
  const northwind = area("northwind-api", {
    month_to_date: { month: "2026-10", cost: 0 },
    week: { thisWeek: 1, lastWeek: 2, changePercent: -50 },
  });
  const received = [acme, northwind];
  const before = structuredClone(received);
  const spend = {
    id: "claude",
    projects: [
      { project: "/w/one", areas: [acme] },
      { project: "/w/two", areas: [northwind] },
    ],
  };

  let sent = null;
  __invoke.current = async (cmd, args) => {
    assert.equal(cmd, "client_rollup");
    sent = args;
    return { rules: [], rows: [] };
  };
  // loadClients() ends in render(), which finds no #detail-body here and returns.
  globalThis.document = { querySelector: () => null };
  __opened.set("claude", { spend: (id) => (id === "claude" ? spend : undefined) });
  try {
    await __loadClients();
  } finally {
    __opened.set(null, null);
    delete globalThis.document;
    __invoke.current = async () => { throw new Error("invoke() is not stubbed in this test"); };
  }

  assert.ok(sent, "the client_rollup command was never called");
  assert.equal(sent.areas.length, 2, "every area of every project goes");
  sent.areas.forEach((a, i) => assert.strictEqual(a, received[i], `area ${i} must be the object that was received, not a rebuilt copy`));
  assert.deepEqual(received, before, "sending them must not change them either");
  assert.deepEqual(sent.areas[0].month_to_date, { month: "2026-10", cost: 24 }, "the month goes back with its figure");
  assert.deepEqual(sent.areas[1].month_to_date, { month: "2026-10", cost: 0 }, "a zero figure goes back as zero");
});
