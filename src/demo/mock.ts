// The demo's stand-in for the Rust backend, so the real interface can run in
// a plain browser tab with nothing installed.
//
// Two rules. The data is fictional: an invented freelancer, invented clients.
// And it is not hand-typed: `demo-fixture.json` is what the REAL engine
// computed from a fictional machine (scripts/make-demo-fixture.py), so the
// inventory, spend, sessions, audit and agent budgets agree with each other
// the way real ones do. Only live things the engine cannot fake (limits that
// move, charts that end "now") are generated here, relative to the viewer's
// clock.
// Nothing in the demo leaves the page: exports and config edits are pretend.

import fixture from "../demo-fixture.json";
import { t } from "../i18n";
import {
  auditWithFindings,
  buildDuplicateProcessesRow,
  buildLimitTimeRow,
  buildOverBudgetRow,
  buildUsageRows,
  LIMIT_WINDOW_MS,
  limitTimeOrder,
  STATE_FINDING_IDS,
  thin,
  timeAtLimit,
  type LimitTimeRow,
} from "./synthetic";

// Set by the build from package.json (vite.config.ts), never typed in here.
declare const __APP_VERSION__: string;

type Args = Record<string, any>;
const HOUR = 3_600_000;
const now = () => Date.now();

// A small seeded generator keeps the charts identical between visits.
function rng(seed: number): () => number {
  let s = seed;
  return () => ((s = (s * 1664525 + 1013904223) % 4294967296) / 4294967296);
}

const config: Args = {
  refreshMinutes: 1, disabled: [], pinned: null, trayProviders: [], pacingAlways: true,
  notifyAlmostOut: true, notifyCuttingClose: true, notifyWillRunOut: true, notifyReset: true,
  burnAlertPoints: 15, dailySpendAlert: 0, renewalReminderDays: 3, weeklyDigest: "mon", sessionNudgeDays: 7,
  wideMode: false, trustLookup: true, apiFeeds: false, auditSeen: true,
  spendTab: "today", spendMetric: "cost", showUsed: false, resetExact: false, timeFormat: "auto",
  layout: null, appearance: "dark", density: "compact", minimal: false, glassEffects: true, shortcut: "",
  locale: "en", showTotalSpend: true, reduceAnimations: false, welcomeDismissed: true,
  lastSeenVersion: __APP_VERSION__,
};

/** The cards as the clock reads `at`. A caller that also needs the clock (the readings below)
 *  reads it once and hands it in: reading it again here, a few milliseconds later, would hang
 *  every reset time a few milliseconds off the instants the caller works from. */
function snapshots(at: number = now()) {
  const metric = (label: string, used: number, resetInHours: number, periodHours: number, detail: string | null = null) => ({
    label, kind: "progress", used_percent: used, detail, value: null, resets_at: at + resetInHours * HOUR, period_ms: periodHours * HOUR,
  });
  // Limits drift a little with the clock so the demo does not look frozen.
  const wobble = Math.round((Math.sin(at / (20 * 60_000)) + 1) * 3);
  const card = (id: string, name: string, plan: string, metrics: unknown[]) => ({
    id, name, plan, status: "ok", error: null, metrics, stale: false, warning: null, fetched_at: at,
  });
  return [
    card("claude", "Claude", "max", [metric("Session", 38 + wobble, 2.4, 5), metric("Weekly", 61, 52, 168), metric("Opus weekly", 78, 52, 168)]),
    card("codex", "Codex", "plus", [metric("Session", 12, 3.1, 5), metric("Weekly", 24, 96, 168)]),
    card("cursor", "Cursor", "pro", [metric("Included usage", 47, 11 * 24, 720, "$9.40 of $20.00")]),
    card("copilot", "Copilot", "individual", [
      { label: "Credits", kind: "text", used_percent: null, detail: null, value: "Not included in plan", resets_at: null, period_ms: null },
      metric("Chat", 6, 9 * 24, 720, "188 of 200 left"),
      metric("Completions", 3, 9 * 24, 720, "1940 of 2000 left"),
    ]),
  ];
}

function spend() {
  const claude = (fixture as any).spend as any[];
  // Two light extra cards so the ledger's "plan loses" line has something to say.
  const thin = (id: string, name: string, cost: number) => ({
    id, name, today: { cost: cost / 30, tokens: 40_000, models: [] }, yesterday: { cost: cost / 28, tokens: 38_000, models: [] },
    last30: { cost, tokens: 1_200_000, models: [{ model: id === "codex" ? "gpt-5-codex" : "cursor-auto", cost, tokens: 1_200_000 }] },
    trend: Array(30).fill(40_000), unpriced: 0, unpriced_models: [], daily_cost: Array(30).fill(cost / 30), projects: [],
  });
  // The real app derives this after the scan; the demo does the same sum so
  // the fortnight line is never out of step with the day bars behind it.
  const week = (daily: number[]) => {
    if (!daily || daily.length < 14) return null;
    const cut = daily.length - 7;
    const thisWeek = daily.slice(cut).reduce((a, b) => a + b, 0);
    const lastWeek = daily.slice(cut - 7, cut).reduce((a, b) => a + b, 0);
    return { thisWeek, lastWeek, changePercent: lastWeek > 0 ? ((thisWeek - lastWeek) / lastWeek) * 100 : null };
  };
  const all = [...claude, thin("codex", "Codex", 31), thin("cursor", "Cursor", 9.4)];
  for (const card of all) card.week = week(card.daily_cost);
  return all;
}

/** One reading the demo's history store holds: the value, and the reset time it carried. */
type Reading = { at: number; used: number; resetsAt: number };

/** Past periods in which the fictional Claude account used a limit all the way up and then
 *  waited for the reset: per limit, the periods back from the live one (1 is the one that has
 *  just ended) and the minutes each sat at 100 percent before its reset. Every other past
 *  period peaks short of it. Nothing else here is typed: the readings are the sawtooth below,
 *  and the time at the limit is worked out from them. */
const WALLS: Record<string, Record<number, number>> = {
  "claude/Weekly": { 1: 200, 3: 110, 4: 40 },
  "claude/Session": { 21: 35, 96: 45 },
};

/** One reading every half hour, each a little before its half hour. The time-at-the-limit rule
 *  pairs readings only when they are 90 minutes apart or less, so a longer step would count
 *  no time at all. 30 days of this is 1,440 readings, under the 1,500 the app's own chart is
 *  cut to; 90 days is 4,320, which `history` thins to that cap the way the app does. No reading
 *  falls on a reset, as a real one almost never does (the rule treats a reset on the later
 *  reading's own instant as not between the two): every reset here is a whole number of six
 *  minutes from a half hour, and the offset is under six and never zero. */
const READING_STEP = 30 * 60_000;
const READING_OFFSET = 5 * 60_000;
/** The share of a short limit's past windows that go unused, and the most such a window reaches. */
const IDLE_WINDOW_SHARE = 0.45;
const IDLE_PEAK = 8;
/** The history store keeps 90 days, and the app's own command reads no more than that. */
const MAX_HISTORY_HOURS = 24 * 90;

/** One card's progress limits as the demo's history store holds them over the last `hours`,
 *  oldest reading first. The chart (`history`) and the time at the limit (`limitTime`) both
 *  read this, so the one cannot say what the other does not show. A sawtooth that lands exactly
 *  on each limit's live value, built back from one reading of the clock: each earlier period
 *  climbs to a height of its own, and the same instants and values come back whatever the range,
 *  so a shorter range is the newest part of a longer one. */
function readings(provider: string, hours: number): { metric: string; points: Reading[] }[] {
  const end = now();
  const snap = snapshots(end).find((s) => s.id === provider);
  if (!snap) return [];
  const start = end - Math.min(Number(hours) || 0, MAX_HISTORY_HOURS) * HOUR;
  const settle = (x: number) => Math.round(Math.max(0, Math.min(100, x)) * 10) / 10;
  return (snap.metrics as any[])
    .filter((m) => m.kind === "progress")
    .map((m, slot) => {
      const period = m.period_ms as number;
      const livePeriodStart = (m.resets_at as number) - period;
      const liveAge = end - livePeriodStart;
      const walls = WALLS[`${provider}/${m.label}`] ?? {};
      // Two streams. Each reading draws its own offset and noise, the same two draws every time, so
      // what one reading says never depends on how far back the range goes. The height an earlier
      // period climbs to is drawn once for the period, the first time a reading meets it, and the
      // periods are met in order, so each keeps its own height whatever the range. A 5-hour window
      // often goes unused (the account is not working round the clock), which leaves the 30- and
      // 90-day line a sawtooth with gaps instead of a solid band; a week or a month always sees use.
      const rand = rng(provider.length * 7919 + slot * 104729);
      const peakRand = rng(provider.length * 6007 + slot * 90001 + 3);
      const idleShare = period < 24 * HOUR ? IDLE_WINDOW_SHARE : 0;
      const peaks: number[] = [];
      const peakOf = (back: number) => {
        while (peaks.length < back) {
          const u = peakRand();
          peaks.push(u < idleShare ? (u / idleShare) * IDLE_PEAK : 55 + ((u - idleShare) / (1 - idleShare)) * 40);
        }
        return peaks[back - 1];
      };
      const points: Reading[] = [];
      for (let j = 0; ; j++) {
        const offset = 1 + Math.floor(rand() * (READING_OFFSET - 1));
        const noise = (rand() - 0.5) * 2.4;
        const at = j === 0 ? end : end - j * READING_STEP - offset;
        if (!(at >= start)) break;
        const intoPeriod = (((at - livePeriodStart) % period) + period) % period;
        const periodStart = at - intoPeriod;
        const back = Math.round((livePeriodStart - periodStart) / period);
        const ramp = (top: number, span: number) => (top * intoPeriod) / Math.max(span, 1) + noise;
        const held = back > 0 ? walls[back] : undefined;
        const wallAt = held === undefined ? Infinity : period - held * 60_000;
        let used: number;
        if (j === 0) used = m.used_percent;
        else if (intoPeriod >= wallAt) used = 100;
        else if (held !== undefined) used = Math.min(99.9, settle(ramp(100, wallAt)));
        else if (back === 0) used = settle(ramp(m.used_percent, liveAge));
        else used = settle(ramp(peakOf(back), period));
        points.push({ at, used, resetsAt: periodStart + period });
      }
      return { metric: m.label as string, points: points.reverse() };
    });
}

/** Limit readings for one card over the last `hours`, for the chart, a long range cut to the cap
 *  the app cuts it to. */
function history(provider: string, hours: number) {
  return readings(provider, hours).map(({ metric, points }) => ({ metric, points: thin(points.map(({ at, used }) => ({ at, used }))) }));
}

function burnProfile(provider: string) {
  const snap = snapshots().find((s) => s.id === provider);
  if (!snap) return [];
  const rand = rng(42);
  return (snap.metrics as any[])
    .filter((m) => m.kind === "progress")
    .map((m) => ({
      metric: m.label,
      daysObserved: 23,
      cells: Array.from({ length: 7 }, (_, d) =>
        Array.from({ length: 24 }, (_, h) => {
          const working = d < 5 && h >= 9 && h <= 19;
          const base = working ? (Math.sin(((h - 8) / 12) * Math.PI) + 0.25) * (d === 1 || d === 3 ? 5.5 : 3.2) : d === 6 && h >= 20 && h <= 21 ? 2 : 0;
          return Math.round(Math.max(0, base * (0.7 + rand() * 0.6)) * 10) / 10;
        }),
      ),
    }));
}

/** Time spent at 100 percent over the last 30 days, one row per limit that reached it, most time
 *  first. Worked out from the readings the chart draws (`readings`), the way the app works it out
 *  from its history store, so the Detail view's line and its chart cannot disagree. */
function limitTime(provider: string): LimitTimeRow[] {
  return readings(provider, LIMIT_WINDOW_MS / HOUR)
    .map(({ metric, points }) => timeAtLimit(provider, metric, points))
    .filter((row): row is LimitTimeRow => row !== null)
    .sort(limitTimeOrder);
}

/** The findings the demo works out from its own state, in the order the app lists them (the
 *  budgets, then time at the limit), null where there is nothing to say. As the app does on every
 *  load, so a budget the visitor removes or adds changes the first at once. The time at the limit
 *  is read over every card's limits at once, from the same rows the Detail view's line is made of:
 *  the fictional machine keeps no history of limit readings for the engine to read. */
function stateFindings() {
  return [buildOverBudgetRow(budgetRows()), buildLimitTimeRow(snapshots().flatMap((s) => limitTime(s.id)))];
}

function forecast(metrics: any[]) {
  return metrics.map((m) => {
    const hoursToReset = m.resetsAt ? (m.resetsAt - now()) / HOUR : null;
    const rate = m.periodMs && m.periodMs > 100 * HOUR ? 0.95 : 7.5;
    const hoursToWall = (100 - m.used) / rate;
    const hits = hoursToReset !== null && hoursToWall < hoursToReset ? now() + hoursToWall * HOUR : null;
    return {
      metric: m.label, basis: "recent", windowHours: rate < 2 ? 24 : 1, ratePerHour: rate, hitsLimitAt: hits,
      projectedAtReset: hoursToReset === null ? null : Math.min(100, m.used + rate * hoursToReset),
    };
  });
}

// --- things the visitor can edit live in memory ------------------------------
let subscriptions: any[] = [
  { id: "sub-1", name: "Claude Max", price: 200, cycle: "monthly", renewsOn: null, provider: "claude", notes: null },
  { id: "sub-2", name: "Cursor Pro", price: 20, cycle: "monthly", renewsOn: null, provider: "cursor", notes: "Mostly tab completion now" },
  { id: "sub-3", name: "Codex Plus", price: 240, cycle: "yearly", renewsOn: null, provider: "codex", notes: null },
];
// Renewals are set relative to today so the countdowns always read well.
const inDays = (n: number) => new Date(now() + n * 24 * HOUR).toISOString().slice(0, 10);
subscriptions[0].renewsOn = inDays(2);
subscriptions[1].renewsOn = inDays(11);
subscriptions[2].renewsOn = inDays(140);

function ledger(usage30: Record<string, number>) {
  const items = subscriptions
    .map((s) => {
      const monthlyCost = s.cycle === "yearly" ? s.price / 12 : s.price;
      const usage = s.provider && usage30[s.provider] !== undefined ? usage30[s.provider] : null;
      const daysLeft = s.renewsOn ? Math.round((new Date(`${s.renewsOn}T00:00:00`).getTime() - new Date(new Date().toDateString()).getTime()) / (24 * HOUR)) : null;
      let whatIf = null;
      if (usage !== null && monthlyCost > 0 && usage >= 1) {
        if (usage >= monthlyCost) whatIf = { kind: "plan-wins", apiCost: usage, planCost: monthlyCost, difference: usage - monthlyCost };
        else if (usage < monthlyCost * 0.5) whatIf = { kind: "plan-loses", apiCost: usage, planCost: monthlyCost, difference: monthlyCost - usage };
      }
      return {
        ...s, monthlyCost, nextRenewal: s.renewsOn, daysLeft, usage30: usage,
        valueRatio: usage !== null && monthlyCost > 0 ? usage / monthlyCost : null,
        idle: usage !== null && usage < 1 && monthlyCost > 0, whatIf,
      };
    })
    .sort((a, b) => (a.daysLeft ?? 1e9) - (b.daysLeft ?? 1e9));
  const monthly = items.reduce((n, i) => n + i.monthlyCost, 0);
  return { items, monthly, yearly: monthly * 12, idleMonthly: items.filter((i) => i.idle).reduce((n, i) => n + i.monthlyCost, 0) };
}

let clientRules: any[] = [
  { client: "Acme Co", patterns: ["acme-portal"], monthlyBudget: 600 },
  { client: "Northwind", patterns: ["northwind-api"], monthlyBudget: null },
];

function glob(pattern: string, text: string): boolean {
  const p = pattern.replace(/^\/+|\/+$/g, "").toLowerCase();
  const t = text.toLowerCase();
  if (!p) return false;
  if (!p.includes("*")) return t === p || t.startsWith(`${p}/`);
  return new RegExp(`^${p.split("*").map((x) => x.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*")}$`).test(t);
}

/** "YYYY-MM" in the viewer's local time: how the engine labels the month a figure is for.
 *  Mirrors `month_key` in crates/core/src/spend.rs, the one spelling the engine uses. */
function monthLabel(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}`;
}

function clientRollup(areas: any[]) {
  const rows: Record<string, any> = {};
  const month = new Date().getMonth();
  const thisMonth = monthLabel(now());
  for (const a of [...areas].sort((x, y) => y.last30.cost - x.last30.cost)) {
    const rule = clientRules.find((r) => r.patterns.some((p: string) => glob(p, a.area)));
    const name = rule ? rule.client : "Unassigned";
    const zero = () => ({ cost: 0, tokens: 0, models: [] });
    const row = (rows[name] ??= { client: name, today: zero(), yesterday: zero(), last30: zero(), monthToDate: 0, areas: [] });
    for (const k of ["today", "yesterday", "last30"] as const) {
      row[k].cost += a[k].cost;
      row[k].tokens += a[k].tokens;
    }
    const figure = a.month_to_date;
    if (figure && typeof figure.cost === "number" && figure.month === thisMonth) {
      // The engine's own figure for this month, summed from every day it read: the
      // series below is a day short on the 31st of a 31-day month.
      row.monthToDate += figure.cost;
    } else {
      // No figure for this month: cut the month out of the series, as for a fixture
      // with no figure at all. The app counts a figure for another month as zero,
      // because a stale scan holds none of this month's spend. The demo has no scan:
      // its fixture is frozen at one month and its series is anchored to the viewer's
      // today, so a label from another month says nothing about this one, and zeroing
      // on it would show "$0 of $600" beside bars of hundreds.
      const daily: number[] = a.daily_cost ?? [];
      daily.forEach((c, i) => {
        const d = new Date(now() - (daily.length - 1 - i) * 24 * HOUR);
        if (d.getMonth() === month) row.monthToDate += c;
      });
    }
    row.areas.push(a.area);
  }
  const list = Object.values(rows).sort((a: any, b: any) => Number(a.client === "Unassigned") - Number(b.client === "Unassigned") || b.last30.cost - a.last30.cost);
  return { rules: clientRules, rows: list };
}

const PINS: Record<string, [string, string]> = {
  playwright: ["@playwright/mcp@0", "0.0.41"],
  postgres: ["pg-readonly-mcp@1", "1.4.2"],
  notes: ["notes-mcp@0.9.3", "0.9.3"],
};
const pinned = new Set<string>();

/** A plausible live process picture for the fictional machine. Fixed numbers:
 *  a demo that drifts every second reads as broken, not live. */
const RUNNING = [
  { name: "chrome-devtools", configured: true, client: "Claude Code", package: "chrome-devtools-mcp", instances: 3, rssBytes: 812 * 1048576, elapsedSecs: 129600, pids: [1130, 1210, 1539, 1893, 2044, 2101] },
  { name: "notes", configured: true, client: "Claude Desktop", package: "obsidian-mcp", instances: 2, rssBytes: 276 * 1048576, elapsedSecs: 129540, pids: [2812, 2904, 3011] },
  { name: "playwright", configured: true, client: "Claude Code", package: "@playwright/mcp", instances: 1, rssBytes: 188 * 1048576, elapsedSecs: 7200, pids: [8801, 8812] },
  { name: "postgres", configured: true, client: "Claude Code", package: "pg-readonly-mcp", instances: 1, rssBytes: 64 * 1048576, elapsedSecs: 3300, pids: [9120] },
] as const;

/** Two agent hosts on the same fictional machine, covering the two shapes
 *  describeAgent() (src/inventory.ts) has to render: one with a priced live
 *  pace in a client-billed folder, one with only a folder and nothing else
 *  known about it. Acme Co (see clientRules above) is this demo's own
 *  invented client, never a real one. */
const AGENTS = [
  {
    tool: "Claude Code", pid: 4821, elapsedSecs: 5400, rssBytes: 342 * 1048576, cpuPercent: 6.4,
    cwd: "/Users/jordan/dev/acme-webapp", area: "acme-portal/web", client: "Acme Co",
    pace: {
      sessionId: "8f2c1e40-9b7a-4c3d-9e2f-1a2b3c4d5e6f", tokens10m: 18400, cost10m: 0.62,
      priced: true, idleSecs: 40, model: "claude-opus-4-1", area: "acme-portal/web",
    },
  },
  {
    tool: "Codex", pid: 7710, elapsedSecs: 900, rssBytes: 210 * 1048576, cpuPercent: null,
    cwd: "/Users/jordan/dev/lambda-notifier", area: null, client: null, pace: null,
  },
] as const;

/** Servers the viewer has ended in this demo session. */
const endedServers = new Set<string>();

const SIGN_INS = [
  { id: "claude", name: "Claude", verifiedHere: true, hint: "Run `claude` in a terminal and sign in.",
    probes: [ { kind: "file", location: "~/.claude/.credentials.json", found: true } ] },
  { id: "codex", name: "Codex", verifiedHere: false, hint: "Run `codex login` in a terminal.",
    probes: [ { kind: "file", location: "~/.codex/auth.json", found: true } ] },
  { id: "copilot", name: "GitHub Copilot", verifiedHere: true, hint: "Sign in to Copilot in your editor, or run `gh auth login`.",
    probes: [ { kind: "file", location: "~/.config/github-copilot/apps.json", found: true },
              { kind: "file", location: "~/.config/github-copilot/hosts.json", found: false } ] },
  { id: "cursor", name: "Cursor", verifiedHere: false, hint: "Open Cursor and sign in; the app reads its local database.",
    probes: [ { kind: "file", location: "~/Library/Application Support/Cursor/User/globalStorage/state.vscdb", found: false } ] },
] as const;

const EFFORT = [
  { area: "acme-portal", cost: 692, commits: 148, costPerCommit: 692 / 148 },
  { area: "northwind-api", cost: 406, commits: 61, costPerCommit: 406 / 61 },
  { area: "internal-tools", cost: 181, commits: 12, costPerCommit: 181 / 12 },
  { area: "blog", cost: 14, commits: null, costPerCommit: null },
] as const;

function inventory() {
  const inv = structuredClone((fixture as any).inventory);
  for (const s of inv.mcpServers) {
    const pin = PINS[s.name];
    if (pin && pinned.has(s.name)) s.package = pin[0];
    else if (pin) s.pinTo = pin[0];
  }
  // The app adds the usage findings to the setup ones; the audit carries
  // them, Msg and all, so they translate exactly as they do on that panel. The
  // engine's checks for the findings built from the demo's state below are not
  // lifted: they describe the machine as it was generated.
  const liftable = new Set<string>([...inv.opportunities.map((o: any) => o.id), ...STATE_FINDING_IDS]);
  inv.opportunities.push(...buildUsageRows((fixture as any).audit.sections, liftable));
  // Those, last, as the app lists the learn findings after the tighten ones.
  inv.opportunities.push(...stateFindings().filter((row) => row !== null));
  // The real app computes this one from the live process list; the demo has
  // a fixed process list, so derive it the same way rather than hard-coding
  // text -- see synthetic.ts for the shared keys and vars with procs.rs.
  const dupRow = buildDuplicateProcessesRow(RUNNING);
  if (dupRow) inv.opportunities.unshift(dupRow);
  if ([...pinned].length) {
    const left = inv.mcpServers.filter((s: any) => s.pinTo).length;
    inv.opportunities = inv.opportunities.filter((o: any) => o.id !== "mcp-unpinned" || left > 0);
  }
  return inv;
}

const TRUST: Record<string, [string, string, number]> = {
  "@upstash/context7-mcp": ["Context7", "recommended", 91],
  "@playwright/mcp": ["Playwright", "enterprise-verified", 93],
  "figma-context-mcp": ["Figma Context", "emerging", 58],
};

// Agent budgets and the live rule, in memory. The real command refuses what
// this one refuses (a name that is not offered, a repeated name, a bad figure)
// with the same translated keys, so the Budgets section and the two Settings
// dropdowns behave in the browser exactly as they do in the app. Nothing here
// is written anywhere: a reload starts again from what the fixture holds.
const MAX_WATCH_BUDGETS = 50;
const MAX_WATCH_USD = 1_000_000;
const MAX_WATCH_MINUTES = 1_000_000_000;

type AgentWatch = { budgets: { agent: string; monthlyBudget: number }[]; live: { hourlyPaceUsd: number | null; maxMinutes: number | null } };

/** What the real engine answered for the fictional machine's own agent_watch.json
 *  (scripts/make-demo-fixture.py, `live_agent_watch`): the saved budgets and live rule, each
 *  saved budget's figure for the calendar month, the names a budget may take, and `monthSpend`,
 *  the month's spend of every agent that has any, by name: what a budget is read against, the
 *  ones the fixture saved and the ones a visitor adds alike. Read when first asked for, not
 *  when this module loads. */
const fixtureWatch = () =>
  (fixture as any).agentWatch as {
    watch: AgentWatch;
    budgets: { agent: string; monthToDate: number }[];
    known: string[];
    monthSpend: Record<string, number>;
  };

/** What is saved: the fixture's watch until the visitor saves one of their own. */
let agentWatch: AgentWatch | null = null;
const savedWatch = (): AgentWatch => (agentWatch ??= structuredClone(fixtureWatch().watch));

/** The engine's own list of the names a budget may take, plus any name just saved: the real
 *  command rebuilds the list from what was saved, and every name this demo can save is one the
 *  engine already offered, so nothing is typed here and the two lists agree. */
function agentWatchKnown(saved: { agent: string }[]): string[] {
  return [...new Set([...fixtureWatch().known, ...saved.map((b) => b.agent)])].sort();
}

/** The saved budgets as the app lists them: one row each, in the order saved. A budget reads its
 *  agent's spend for the calendar month, as the engine summed it for every agent that has any (a
 *  Map, so an agent named like an Object property is no special case): a budget the visitor adds on
 *  an agent with spend reads that spend, and one on an agent with none reads $0, as in the app. */
function budgetRows() {
  const monthToDate = new Map(Object.entries(fixtureWatch().monthSpend));
  return savedWatch().budgets.map((b) => ({ agent: b.agent, monthToDate: monthToDate.get(b.agent) ?? 0, monthlyBudget: b.monthlyBudget }));
}

function agentWatchView() {
  const saved = savedWatch();
  // The same sum the real app makes: the fastest priced, non-idle running pace,
  // a dollar figure an hour (the last ten minutes carried over an hour).
  const paces = AGENTS.map((a) => a.pace as { priced: boolean; idleSecs: number; tokens10m: number; cost10m: number } | null)
    .filter((p) => p && p.priced && !(p.idleSecs >= 60 && p.tokens10m === 0))
    .map((p) => p!.cost10m * 6);
  return structuredClone({
    watch: saved,
    budgets: budgetRows(),
    runaways: [],
    liveHint: paces.length ? Math.max(...paces) : null,
    known: agentWatchKnown(saved.budgets),
  });
}

function goodWatchFigure(x: unknown): x is number {
  return typeof x === "number" && Number.isFinite(x) && x > 0 && x <= MAX_WATCH_USD;
}

/// Mirrors watch_from_json and validate in crates/core/src/agent_watch.rs.
function saveAgentWatch(v: any) {
  if (v === null || typeof v !== "object" || Array.isArray(v)) throw t("error.agentWatch.figure");
  const budgets: { agent: string; monthlyBudget: number }[] = [];
  if (v.budgets != null) {
    if (!Array.isArray(v.budgets)) throw t("error.agentWatch.figure");
    for (const entry of v.budgets) {
      if (typeof entry?.agent !== "string" || !entry.agent) throw t("error.agentWatch.pick");
      if (typeof entry.monthlyBudget !== "number") throw t("error.agentWatch.figure");
      budgets.push({ agent: entry.agent, monthlyBudget: entry.monthlyBudget });
    }
  }
  const live = { hourlyPaceUsd: null as number | null, maxMinutes: null as number | null };
  if (v.live != null) {
    if (typeof v.live !== "object" || Array.isArray(v.live)) throw t("error.agentWatch.figure");
    if (v.live.hourlyPaceUsd != null) {
      if (typeof v.live.hourlyPaceUsd !== "number") throw t("error.agentWatch.figure");
      live.hourlyPaceUsd = v.live.hourlyPaceUsd;
    }
    if (v.live.maxMinutes != null) {
      const m = v.live.maxMinutes;
      if (typeof m !== "number" || !Number.isInteger(m) || m < 1 || m > MAX_WATCH_MINUTES) throw t("error.agentWatch.figure");
      live.maxMinutes = m;
    }
  }
  if (budgets.length > MAX_WATCH_BUDGETS) throw t("error.agentWatch.tooMany", { max: MAX_WATCH_BUDGETS });
  const known = agentWatchKnown(savedWatch().budgets);
  const seen = new Set<string>();
  for (const b of budgets) {
    if (!known.includes(b.agent)) throw t("error.agentWatch.pick");
    if (seen.has(b.agent)) throw t("error.agentWatch.duplicate");
    seen.add(b.agent);
    if (!goodWatchFigure(b.monthlyBudget)) throw t("error.agentWatch.figure");
  }
  if ((live.hourlyPaceUsd !== null && !goodWatchFigure(live.hourlyPaceUsd))) throw t("error.agentWatch.figure");
  agentWatch = { budgets, live };
  return agentWatchView();
}

const DEMO_NOTE = "This is the demo: nothing is written to disk. The real app saves this to your Downloads folder.";

export function handle(cmd: string, args: Args = {}): unknown {
  switch (cmd) {
    case "get_config": return config;
    case "set_config": Object.assign(config, args.patch ?? {}); return config;
    case "cached_usage":
    case "fetch_usage": return snapshots();
    case "fetch_spend": return spend();
    case "get_inventory": return inventory();
    case "get_running": return structuredClone(RUNNING.filter((r) => !endedServers.has(r.name)));
    case "get_running_agents": return structuredClone(AGENTS);
    case "get_agent_spend": return (fixture as any).agentSpend ?? [];
    case "get_agent_watch": return agentWatchView();
    case "set_agent_watch": return saveAgentWatch(args.watch);
    case "get_setup_changes": return (fixture as any).setupChanges;
    case "end_task": {
      if (!RUNNING.some((r) => r.name === args.name)) throw "that server is not running any more";
      endedServers.add(String(args.name));
      return 1;
    }
    case "get_diagnosis": return structuredClone(SIGN_INS);
    case "get_effort": return structuredClone(EFFORT);
    case "get_history": return history(args.providerId, args.hours);
    case "get_burn_profile": return burnProfile(args.providerId);
    case "get_limit_time": return limitTime(args.providerId);
    case "get_forecast": return forecast(args.metrics ?? []);
    case "get_sessions": return args.day ? (fixture as any).sessions.day.slice(0, 12) : (fixture as any).sessions.area;
    case "get_audit": return auditWithFindings((fixture as any).audit, stateFindings());
    case "get_ledger": return ledger(args.usage30 ?? {});
    case "save_subscription": {
      const s = { ...args.subscription };
      if (!String(s.name ?? "").trim()) throw "Give the subscription a name.";
      if (!(Number(s.price) >= 0)) throw "Enter the price as a number, zero or more.";
      if (s.id) subscriptions = subscriptions.map((x) => (x.id === s.id ? s : x));
      else subscriptions.push({ ...s, id: `sub-${subscriptions.length + 1}-${now()}` });
      return s;
    }
    case "delete_subscription": subscriptions = subscriptions.filter((x) => x.id !== args.id); return null;
    case "client_rollup": return clientRollup(args.areas ?? []);
    case "save_clients": {
      const rules = (args.rules as any[]).filter((r) => r.client.trim() || r.patterns.length);
      if (rules.some((r) => !r.client.trim())) throw "Every rule needs a client name.";
      clientRules = rules;
      return clientRules;
    }
    case "get_trust": {
      if (!config.trustLookup) return { enabled: false, fetchedAt: null, listed: 0, ratings: [], error: null };
      const ratings = (args.packages as string[]).map((p) => {
        const name = (p.startsWith("@") ? `@${p.slice(1).split("@")[0]}` : p.split("@")[0]).toLowerCase();
        const hit = TRUST[name];
        return { package: p, listedAs: hit?.[0] ?? null, tier: hit?.[1] ?? null, score: hit?.[2] ?? null };
      });
      return { enabled: true, fetchedAt: now() - 5 * HOUR, listed: 38, ratings, error: null };
    }
    case "pin_preview": {
      const pin = PINS[args.name];
      const server = (fixture as any).inventory.mcpServers.find((s: any) => s.name === args.name);
      if (!pin || !server) throw "that server is not unpinned any more";
      const file = args.client === "Claude Desktop" ? "/Users/dana/Library/Application Support/Claude/claude_desktop_config.json" : "/Users/dana/.claude.json";
      return { file, package: server.package, installedVersion: pin[1], from: server.package, to: pin[0], occurrences: 1, fileLen: 1, fileMtimeMs: 1 };
    }
    case "pin_apply": pinned.add(args.name); return "/Users/dana/.claude.json.aitm-backup (in the real app)";
    case "export_table":
    case "export_clients_csv":
    case "export_audit": throw DEMO_NOTE;
    case "open_link": window.open(String(args.url), "_blank", "noopener"); return null;
    case "set_wide": window.parent?.postMessage({ aitmDemoWide: Boolean(args.wide) }, "*"); return null;
    case "get_autostart": return false;
    case "check_update": return null;
    case "system_ui_locale": return "en";
    case "plugin:app|version": return __APP_VERSION__;
    case "plugin:event|listen": return 0;
    case "plugin:event|unlisten": return null;
    default:
      if (/list_sites|_list_/.test(cmd)) return [];
      return null;
  }
}

export function install(): void {
  const w = window as any;
  w.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    transformCallback: (cb: (v: unknown) => void) => {
      const id = Math.floor(Math.random() * 1e9);
      w[`_${id}`] = cb;
      return id;
    },
    unregisterCallback: () => {},
    convertFileSrc: (p: string) => p,
    invoke: async (cmd: string, args: Args) => handle(cmd, args),
  };
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  document.documentElement.dataset.demo = "true";
}
