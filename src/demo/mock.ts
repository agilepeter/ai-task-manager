// The demo's stand-in for the Rust backend, so the real interface can run in
// a plain browser tab with nothing installed.
//
// Two rules. The data is fictional: an invented freelancer, invented clients.
// And it is not hand-typed: `demo-fixture.json` is what the REAL engine
// computed from a fictional machine (scripts/make-demo-fixture.py), so the
// inventory, spend, sessions and audit agree with each other the way real
// ones do. Only live things the engine cannot fake (limits that move, charts
// that end "now") are generated here, relative to the viewer's clock.
// Nothing in the demo leaves the page: exports and config edits are pretend.

import fixture from "../demo-fixture.json";
import { getLocale, setActiveLocale, tm, type Msg } from "../i18n";

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
  lastSeenVersion: "0.1.0",
};

function metric(label: string, used: number, resetInHours: number, periodHours: number, detail: string | null = null) {
  return { label, kind: "progress", used_percent: used, detail, value: null, resets_at: now() + resetInHours * HOUR, period_ms: periodHours * HOUR };
}

function snapshots() {
  // Limits drift a little with the clock so the demo does not look frozen.
  const wobble = Math.round((Math.sin(now() / (20 * 60_000)) + 1) * 3);
  const card = (id: string, name: string, plan: string, metrics: unknown[]) => ({
    id, name, plan, status: "ok", error: null, metrics, stale: false, warning: null, fetched_at: now(),
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

function history(provider: string, hours: number) {
  const snap = snapshots().find((s) => s.id === provider);
  if (!snap) return [];
  const rand = rng(provider.length * 7919);
  return (snap.metrics as any[])
    .filter((m) => m.kind === "progress")
    .map((m) => {
      const period = m.period_ms as number;
      const end = now();
      const start = end - hours * HOUR;
      const step = Math.max((hours * HOUR) / 220, 10 * 60_000);
      // Work back from the live value: a sawtooth that lands exactly on it.
      const points: { at: number; used: number }[] = [];
      const resetAt = m.resets_at as number;
      for (let at = end; at >= start; at -= step) {
        const intoPeriod = ((at - (resetAt - period)) % period + period) % period;
        const livePeriodAge = ((end - (resetAt - period)) % period + period) % period;
        const samePeriod = end - at <= livePeriodAge;
        const peak = samePeriod ? m.used_percent : 55 + rand() * 40;
        const span = samePeriod ? livePeriodAge : period;
        const used = Math.max(0, Math.min(100, (peak * intoPeriod) / Math.max(span, 1) + (rand() - 0.5) * 2.4));
        points.unshift({ at, used: Math.round(used * 10) / 10 });
      }
      if (points.length) points[points.length - 1].used = m.used_percent;
      return { metric: m.label, points };
    });
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

function clientRollup(areas: any[]) {
  const rows: Record<string, any> = {};
  const month = new Date().getMonth();
  for (const a of [...areas].sort((x, y) => y.last30.cost - x.last30.cost)) {
    const rule = clientRules.find((r) => r.patterns.some((p: string) => glob(p, a.area)));
    const name = rule ? rule.client : "Unassigned";
    const zero = () => ({ cost: 0, tokens: 0, models: [] });
    const row = (rows[name] ??= { client: name, today: zero(), yesterday: zero(), last30: zero(), monthToDate: 0, areas: [] });
    for (const k of ["today", "yesterday", "last30"] as const) {
      row[k].cost += a[k].cost;
      row[k].tokens += a[k].tokens;
    }
    const daily: number[] = a.daily_cost ?? [];
    daily.forEach((c, i) => {
      const d = new Date(now() - (daily.length - 1 - i) * 24 * HOUR);
      if (d.getMonth() === month) row.monthToDate += c;
    });
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

/** A finding mock.ts authors by hand rather than reading one the engine
 *  already rendered off the fixture. titleMsg/detailMsg are required, never
 *  optional, because the demo has to translate every finding the same way
 *  the real app does, and these hand-authored rows are the only findings
 *  that do not already carry a Msg from the engine -- a future row built
 *  without one now fails to typecheck instead of silently staying English. */
type SyntheticOpportunity = {
  id: string;
  kind: "tighten" | "learn";
  title: string;
  detail: string;
  titleMsg: Msg;
  detailMsg: Msg | null;
  learnUrl: string | null;
};

/** Renders a Msg in English regardless of the viewer's current locale, by
 *  flipping the shared active-locale flag and flipping it straight back --
 *  synchronously, so nothing else observes the flip. Lets a hand-authored
 *  row's English fall out of its own Msg instead of existing twice. */
function renderEnglish(msg: Msg): string {
  const current = getLocale();
  setActiveLocale("en");
  try {
    return tm(msg);
  } finally {
    setActiveLocale(current);
  }
}

function inventory() {
  const inv = structuredClone((fixture as any).inventory);
  for (const s of inv.mcpServers) {
    const pin = PINS[s.name];
    if (pin && pinned.has(s.name)) s.package = pin[0];
    else if (pin) s.pinTo = pin[0];
  }
  // The app adds the usage findings to the setup ones; the audit carries
  // them, Msg and all, so they translate exactly as they do on that panel.
  const usage: SyntheticOpportunity[] = ((fixture as any).audit.sections as any[])
    .flatMap((sec) => sec.checks)
    .filter((c) => c.status === "consider" && !inv.opportunities.some((o: any) => o.id === c.id))
    .map(
      (c): SyntheticOpportunity => ({
        id: c.id,
        kind: "learn",
        title: c.title,
        detail: c.detail,
        titleMsg: c.titleMsg,
        detailMsg: c.detailMsg ?? null,
        learnUrl: "https://staas.fund/classroom/",
      }),
    );
  inv.opportunities.push(...usage);
  // The real app computes this one from the live process list; the demo has
  // a fixed process list, so derive it the same way rather than hard-coding
  // text -- same keys and vars as procs.rs's own opportunities(), including
  // the nested unit.times Msg for "running N times", so it translates like
  // every other finding instead of being the one row stuck in English.
  const dupes = RUNNING.filter((r) => r.instances > 1)
    .slice()
    .sort((a, b) => b.rssBytes - a.rssBytes); // "worst" = heaviest RSS, not first configured
  if (dupes.length) {
    const worst = dupes[0];
    const names = dupes.map((r) => r.name).join(", ");
    const wasted = dupes.reduce((sum, r) => sum + (r.rssBytes - Math.floor(r.rssBytes / r.instances)), 0);
    const titleMsg: Msg = { key: "finding.mcp-duplicate-processes.title", vars: {}, count: dupes.length };
    const detailMsg: Msg = {
      key: "finding.mcp-duplicate-processes.detail",
      vars: {
        names,
        worstName: worst.name,
        times: { key: "unit.times", vars: {}, count: worst.instances },
        mb: String(Math.floor(worst.rssBytes / 1048576)),
        wasted: String(Math.floor(wasted / 1048576)),
      },
      count: null,
    };
    const dupRow: SyntheticOpportunity = {
      id: "mcp-duplicate-processes",
      kind: "tighten",
      title: renderEnglish(titleMsg),
      detail: renderEnglish(detailMsg),
      titleMsg,
      detailMsg,
      learnUrl: "https://staas.fund/mcp/",
    };
    inv.opportunities.unshift(dupRow);
  }
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
    case "end_task": {
      if (!RUNNING.some((r) => r.name === args.name)) throw "that server is not running any more";
      endedServers.add(String(args.name));
      return 1;
    }
    case "get_diagnosis": return structuredClone(SIGN_INS);
    case "get_effort": return structuredClone(EFFORT);
    case "get_history": return history(args.providerId, args.hours);
    case "get_burn_profile": return burnProfile(args.providerId);
    case "get_forecast": return forecast(args.metrics ?? []);
    case "get_sessions": return args.day ? (fixture as any).sessions.day.slice(0, 12) : (fixture as any).sessions.area;
    case "get_audit": return (fixture as any).audit;
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
    case "plugin:app|version": return "0.1.0";
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
