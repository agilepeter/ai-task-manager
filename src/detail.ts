// Detail view: one tool, in depth, inside the same window. Limits charted
// over time from the local history store, and spend broken down by model,
// project or day. Opened by clicking a card's name.
//
// Chart rules followed here: one axis; series colours are assigned in fixed
// order and validated for colour-blind separation on this app's surfaces; text
// always wears ink tokens, never a series colour; two or more series get a
// legend with live values; line charts get a crosshair, bars a per-mark tip.

import { invoke } from "@tauri-apps/api/core";
import { displayMetricDetail, displayMetricLabel, localeTag, plural, t } from "./i18n";

interface Metric {
  label: string;
  kind: string;
  used_percent: number | null;
  detail: string | null;
  value: string | null;
  resets_at: number | null;
  period_ms?: number | null;
}

interface BurnProfile {
  metric: string;
  cells: number[][];
  daysObserved: number;
}

interface Forecast {
  metric: string;
  basis: "recent" | "period";
  windowHours: number;
  ratePerHour: number;
  hitsLimitAt: number | null;
  projectedAtReset: number | null;
}

interface Snapshot {
  id: string;
  name: string;
  plan: string | null;
  status: string;
  metrics: Metric[];
}

interface SpendWindow {
  cost: number;
  tokens: number;
  models: { model: string; cost: number; tokens: number }[];
}

/** The last 7 days against the 7 before. Absent until a fortnight exists. */
interface WeekDelta {
  thisWeek: number;
  lastWeek: number;
  changePercent: number | null;
}

interface ProviderSpend {
  id: string;
  today: SpendWindow;
  yesterday: SpendWindow;
  last30: SpendWindow;
  daily_cost?: number[];
  projects?: ProjectSpend[];
  week?: WeekDelta | null;
}

/** 30 days of spend in a folder against 30 days of commits there. */
interface AreaEffort {
  area: string;
  cost: number;
  commits: number | null;
  costPerCommit: number | null;
}

interface AreaSpend {
  area: string;
  today: SpendWindow;
  yesterday: SpendWindow;
  last30: SpendWindow;
  daily_cost: number[];
}

interface SessionSpend {
  id: string;
  project: string;
  startedMs: number | null;
  endedMs: number | null;
  cost: number;
  tokens: number;
  /** Log file size on disk. Absent in fixtures made before it existed. */
  bytes?: number;
  topModel: string | null;
  areas: [string, number][];
  dayCost: number | null;
}

interface ClientRule {
  client: string;
  patterns: string[];
  monthlyBudget?: number | null;
}

interface ClientSpend {
  client: string;
  today: SpendWindow;
  yesterday: SpendWindow;
  last30: SpendWindow;
  monthToDate: number;
  areas: string[];
}

interface ClientView {
  rules: ClientRule[];
  rows: ClientSpend[];
}

interface ProjectSpend {
  project: string;
  today: SpendWindow;
  yesterday: SpendWindow;
  last30: SpendWindow;
  areas?: AreaSpend[];
}

interface Series {
  metric: string;
  points: { at: number; used: number }[];
}

export interface DetailSource {
  snapshot(id: string): Snapshot | undefined;
  spend(id: string): ProviderSpend | undefined;
  /** The card to show when wide mode opens with nothing picked yet. */
  firstId(): string | undefined;
  /** Saved wide-mode choice, and how to save a new one. */
  wide(): boolean;
  saveWide(wide: boolean): void;
}

type WindowKey = "today" | "yesterday" | "last30";
type GroupKey = "model" | "client" | "area" | "project" | "day" | "session";

/** Rows shown before the rest fold into one "Other" row. */
const MAX_BAR_ROWS = 12;

// Values are i18n keys, translated at the point of use (inside render(),
// never baked in here) so a locale switch is picked up on the next redraw.
const RANGES: [hours: number, key: string][] = [
  [24, "detail.range.last24h"],
  [24 * 7, "detail.range.last7d"],
  [24 * 30, "detail.range.last30d"],
  [24 * 90, "detail.range.last90d"],
];
const WINDOWS: [WindowKey, string][] = [
  ["today", "detail.range.today"],
  ["yesterday", "detail.range.yesterday"],
  ["last30", "detail.range.last30d"],
];
/** Series slots 1 to 4, fixed order. Validated per theme in styles.css. */
const SERIES_VARS = ["--viz-1", "--viz-2", "--viz-3", "--viz-4"];

let source: DetailSource | null = null;
let openId: string | null = null;
let hours = 24 * 7;
let metricFilter = "__all__";
let windowKey: WindowKey = "last30";
let groupKey: GroupKey = "model";
let history: Series[] = [];
let historyFor = "";
let loading = false;
let lastLoad = 0;
/** Work areas shown as top-level folders, or at the two levels the log keeps. */
let areaDepth: "1" | "2" = "1";
let clientView: ClientView | null = null;
/** Rules being edited; null while the saved ones are shown. */
let draftRules: ClientRule[] | null = null;
let clientNote = "";
/** Every session of the last 30 days, for Group by → Session; null until loaded. */
let sessionsAll: SessionSpend[] | null = null;
let sessionNote = "";
/** Where "Reveal" puts the file. Named so the button says what will open. A
 * function, not a module-level const, so a locale switch picks it up. */
function revealWord(): string {
  return /Mac|iPhone|iPad/.test(navigator.platform)
    ? t("detail.session.revealFinder")
    : t("detail.session.revealOther");
}
/** What the Spend section is showing, kept so Export CSV saves exactly that. */
let lastTable: { name: string; headers: string[]; rows: string[][] } | null = null;
/** Forecasts per card, refreshed with the history. */
const forecasts = new Map<string, Forecast[]>();
let burn: BurnProfile[] = [];
let burnFor = "";
let burnMetric = "";
/** The drill-down in view: sessions behind an area or a day. */
let drill: { title: string; area?: string; day?: string; sessions: SessionSpend[] | null } | null = null;
let effort: AreaEffort[] = [];
/** Wide mode: the window is twice as wide and this page is a fixed right column. */
let wide = false;

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

/// Whole dollars from $10 up, cents below: a column of values then reads at
/// one precision per magnitude instead of "$929" beside "$11.0".
/// $ stays a symbol, but the digit grouping follows the app's language, not the OS's.
function money(n: number): string {
  return n >= 10 ? `$${Math.round(n).toLocaleString(localeTag())}` : `$${n.toFixed(2)}`;
}

/// B/M/K stay English: format tokens, not prose, like money()'s $ and fileSize()'s
/// MB/KB — "tokens" itself is this codebase's house loanword in every zh/ru string.
function tokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(0)}K`;
  return String(Math.round(n));
}

function when(ms: number, spanHours: number): string {
  const d = new Date(ms);
  return spanHours <= 48
    ? d.toLocaleTimeString(localeTag(), { hour: "numeric", minute: "2-digit" })
    : d.toLocaleDateString(localeTag(), { month: "short", day: "numeric" });
}

/// "resets in 2h 5m", or "resetting now" once the time has passed.
function resetText(ms: number): string {
  return ms - Date.now() <= 0 ? t("detail.now.resettingNow") : t("detail.now.resetsIn", { time: until(ms) });
}

/// Reuses the app-wide time.* duration keys (same ones main.ts's fmtDuration
/// uses) rather than a detail-only duplicate, unpadded to match this file's
/// original look ("2h 5m", not "2h 05m").
function until(ms: number): string {
  const left = ms - Date.now();
  if (left <= 0) return t("time.mins", { m: 0 });
  const m = Math.floor(left / 60_000);
  if (m >= 1440) return t("time.daysHours", { d: Math.floor(m / 1440), h: Math.floor((m % 1440) / 60) });
  if (m >= 60) return t("time.hoursMins", { h: Math.floor(m / 60), m: m % 60 });
  return t("time.mins", { m });
}

/// "/Users/me/work/acme" → "acme"; an unresolved folder name stays as it is.
function projectLabel(path: string): string {
  if (!/[\\/]/.test(path)) return path;
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/// Readings inside the window, plus the last one before it as the value the
/// window opens with. Older readings would all pile up on the left border.
function clip(points: Series["points"], t0: number): Series["points"] {
  const first = points.findIndex((p) => p.at >= t0);
  if (first === -1) return points.slice(-1);
  return points.slice(Math.max(first - 1, 0));
}

function select(id: string, options: [string, string][], current: string): string {
  return `<select id="${id}">${options
    .map(([v, l]) => `<option value="${esc(v)}"${v === current ? " selected" : ""}>${esc(l)}</option>`)
    .join("")}</select>`;
}

// ---------------------------------------------------------------------------
// Limits over time
// ---------------------------------------------------------------------------

function lineChart(all: Series[], width: number): string {
  const shown = all
    .map((s, slot) => ({ ...s, slot }))
    .filter((s) => s.slot < SERIES_VARS.length)
    .filter((s) => metricFilter === "__all__" || s.metric === metricFilter)
    .filter((s) => s.points.length > 0);
  const count = shown.reduce((n, s) => n + s.points.length, 0);
  if (count < 2) {
    return `<p class="dt-empty">${esc(t("detail.chart.historyStarts"))}</p>`;
  }
  const W = Math.max(260, width);
  const H = 150;
  const pad = { l: 30, r: 10, t: 8, b: 20 };
  const t1 = Date.now();
  const t0 = t1 - hours * 3_600_000;
  for (const s of shown) s.points = clip(s.points, t0);
  const x = (at: number) => pad.l + ((Math.max(at, t0) - t0) / (t1 - t0)) * (W - pad.l - pad.r);
  const y = (used: number) => pad.t + (1 - used / 100) * (H - pad.t - pad.b);

  const grid = [0, 50, 100]
    .map(
      (v) => `<line class="dt-grid" x1="${pad.l}" x2="${W - pad.r}" y1="${y(v)}" y2="${y(v)}"/>
        <text class="dt-axis" x="${pad.l - 6}" y="${y(v) + 3}" text-anchor="end">${v}%</text>`,
    )
    .join("");
  const lines = shown
    .map((s) => {
      const d = s.points.map((p, i) => `${i ? "L" : "M"}${x(p.at).toFixed(1)} ${y(p.used).toFixed(1)}`).join("");
      const last = s.points[s.points.length - 1];
      return `<path class="dt-line" style="stroke:var(${SERIES_VARS[s.slot]})" d="${d}"/>
        <circle class="dt-end" style="fill:var(${SERIES_VARS[s.slot]})" cx="${x(last.at).toFixed(1)}" cy="${y(last.used).toFixed(1)}" r="4"/>`;
    })
    .join("");
  // One series is named by the dropdown and the title; two or more get a legend.
  const legend =
    shown.length > 1
      ? `<div class="dt-legend">${shown
          .map((s) => {
            const last = s.points[s.points.length - 1];
            return `<span class="dt-key"><i style="background:var(${SERIES_VARS[s.slot]})"></i>${esc(displayMetricLabel(s.metric))} <b>${last.used.toFixed(0)}%</b></span>`;
          })
          .join("")}</div>`
      : "";
  return `${legend}
    <div class="dt-plot" data-t0="${t0}" data-t1="${t1}" data-pl="${pad.l}" data-pr="${pad.r}" data-w="${W}">
      <svg viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" role="img" aria-label="${esc(t("detail.chart.ariaLabel"))}">
        ${grid}${lines}
        <line class="dt-cross" x1="0" x2="0" y1="${pad.t}" y2="${H - pad.b}" visibility="hidden"/>
        <text class="dt-axis" x="${pad.l}" y="${H - 5}">${esc(when(t0, hours))}</text>
        <text class="dt-axis" x="${W - pad.r}" y="${H - 5}" text-anchor="end">${esc(t("detail.chart.now"))}</text>
      </svg>
      <div class="dt-tip" hidden></div>
    </div>
    <p class="dt-caption">${esc(t("detail.chart.caption"))}</p>`;
}

/** Crosshair: snaps to the nearest reading in time and lists every series there. */
function wireCrosshair(root: HTMLElement): void {
  const plot = root.querySelector<HTMLElement>(".dt-plot");
  if (!plot) return;
  const cross = plot.querySelector<SVGLineElement>(".dt-cross")!;
  const tip = plot.querySelector<HTMLElement>(".dt-tip")!;
  const t0 = Number(plot.dataset.t0);
  const t1 = Number(plot.dataset.t1);
  const pl = Number(plot.dataset.pl);
  const pr = Number(plot.dataset.pr);
  const W = Number(plot.dataset.w);
  const shown = history
    .map((s, slot) => ({ ...s, slot }))
    .filter((s) => s.slot < SERIES_VARS.length)
    .filter((s) => metricFilter === "__all__" || s.metric === metricFilter);
  const times = [...new Set(shown.flatMap((s) => s.points.map((p) => p.at)))].sort((a, b) => a - b);

  plot.addEventListener("pointermove", (e) => {
    const box = plot.getBoundingClientRect();
    const px = e.clientX - box.left;
    const at = t0 + ((px - pl) / (W - pl - pr)) * (t1 - t0);
    let nearest = times[0];
    for (const t of times) if (Math.abs(t - at) < Math.abs(nearest - at)) nearest = t;
    if (nearest === undefined) return;
    const cx = pl + ((Math.max(nearest, t0) - t0) / (t1 - t0)) * (W - pl - pr);
    cross.setAttribute("x1", String(cx));
    cross.setAttribute("x2", String(cx));
    cross.setAttribute("visibility", "visible");
    const rows = shown
      .map((s) => {
        // The reading in force at that moment: the latest one not after it.
        const p = [...s.points].reverse().find((q) => q.at <= nearest);
        return p
          ? `<div><i style="background:var(${SERIES_VARS[s.slot]})"></i>${esc(displayMetricLabel(s.metric))} <b>${p.used.toFixed(0)}%</b></div>`
          : "";
      })
      .join("");
    tip.innerHTML = `<div class="dt-tip-when">${esc(new Date(nearest).toLocaleString(localeTag(), { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }))}</div>${rows}`;
    tip.hidden = false;
    const tw = tip.offsetWidth;
    tip.style.left = `${Math.min(Math.max(cx - tw / 2, 0), W - tw)}px`;
  });
  plot.addEventListener("pointerleave", () => {
    cross.setAttribute("visibility", "hidden");
    tip.hidden = true;
  });
}

// ---------------------------------------------------------------------------
// Forecast
// ---------------------------------------------------------------------------

async function loadForecast(id: string): Promise<void> {
  const snap = source?.snapshot(id);
  if (!snap) return;
  const metrics = snap.metrics
    .filter((m) => m.kind === "progress" && m.used_percent !== null)
    .map((m) => ({ label: m.label, used: m.used_percent, resetsAt: m.resets_at, periodMs: m.period_ms ?? null }));
  try {
    forecasts.set(id, await invoke<Forecast[]>("get_forecast", { providerId: id, metrics }));
  } catch {
    forecasts.delete(id);
  }
}

// Reuses the app-wide time.today / time.dateAt keys (same shape main.ts's
// fmtExact uses for "today at 6:38 PM" / "Sat, Jul 11 at 9:00 AM").
function atText(ms: number): string {
  const d = new Date(ms);
  const sameDay = d.toDateString() === new Date().toDateString();
  const time = d.toLocaleTimeString(localeTag(), { hour: "numeric", minute: "2-digit" });
  if (sameDay) return t("time.today", { time });
  const weekday = d.toLocaleDateString(localeTag(), { weekday: "short" });
  return t("time.dateAt", { date: weekday, time });
}

/// One forecast as a sentence. `short` is for the card, where space is tight.
/// Each branch is one full translated sentence (never a translated fragment
/// glued to another) so word order can move freely per language.
function forecastText(f: Forecast, short = false): string {
  const metric = displayMetricLabel(f.metric);
  if (f.hitsLimitAt !== null) {
    const when = atText(f.hitsLimitAt);
    if (short) return t("detail.forecast.hitsAtShort", { metric, when });
    if (f.basis === "recent") {
      return f.windowHours >= 1.5
        ? t("detail.forecast.hitsAt", { metric, when, hours: Math.round(f.windowHours) })
        : t("detail.forecast.hitsAtHour", { metric, when });
    }
    return t("detail.forecast.hitsAtPeriod", { metric, when });
  }
  if (f.projectedAtReset === null) return "";
  if (short) return "";
  const pct = f.projectedAtReset.toFixed(0);
  if (f.ratePerHour === 0) return t("detail.forecast.flat", { metric, pct });
  if (f.basis === "recent") {
    return f.windowHours >= 1.5
      ? t("detail.forecast.reaches", { metric, pct, hours: Math.round(f.windowHours) })
      : t("detail.forecast.reachesHour", { metric, pct });
  }
  return t("detail.forecast.reachesPeriod", { metric, pct });
}

function forecastSection(id: string): string {
  const lines = (forecasts.get(id) ?? []).map((f) => ({ f, text: forecastText(f) })).filter((l) => l.text);
  if (!lines.length) return "";
  return `<div class="dt-forecast">${lines
    .map((l) => `<p class="${l.f.hitsLimitAt !== null ? "dt-forecast-hit" : ""}">${esc(l.text)}</p>`)
    .join("")}</div>`;
}

// ---------------------------------------------------------------------------
// Your week: when the limit gets used, and when it resets
// ---------------------------------------------------------------------------

/// Weekday short names from a real date (2024-01-01 was a Monday), never a
/// hardcoded "Mon".."Sun" table, so they follow the app's language.
function weekdayShort(mondayIndex: number): string {
  const d = new Date(2024, 0, 1 + mondayIndex);
  return new Intl.DateTimeFormat(localeTag(), { weekday: "short" }).format(d);
}

function hourLabel(h: number): string {
  return new Date(2000, 0, 1, h).toLocaleTimeString(localeTag(), { hour: "numeric" });
}

function weekSection(snap: Snapshot): string {
  const usable = burn.filter((b) => b.cells.some((row) => row.some((v) => v > 0)));
  if (!usable.length) {
    return `<p class="dt-empty">${esc(t("detail.week.empty"))}</p>`;
  }
  if (!usable.some((b) => b.metric === burnMetric)) {
    // The longest window is the one worth planning a week around.
    burnMetric = (usable.find((b) => /week/i.test(b.metric)) ?? usable[0]).metric;
  }
  const profile = usable.find((b) => b.metric === burnMetric)!;
  const metricLabel = displayMetricLabel(profile.metric);
  const max = Math.max(...profile.cells.flat(), 0.0001);
  const live = snap.metrics.find((m) => m.label === profile.metric);
  const reset = live?.resets_at ? new Date(live.resets_at) : null;
  const resetDay = reset ? (reset.getDay() + 6) % 7 : -1;
  const resetHour = reset ? reset.getHours() : -1;

  const rows = profile.cells
    .map((row, d) => {
      const cells = row
        .map((v, h) => {
          // Sequential, one hue: more burn is more of the same blue.
          const level = v <= 0 ? 0 : Math.max(0.16, v / max);
          const isReset = d === resetDay && h === resetHour;
          const day = weekdayShort(d);
          const hour = hourLabel(h);
          const tip =
            v > 0
              ? isReset
                ? t("detail.week.cellPointsReset", { day, hour, points: v.toFixed(v < 10 ? 1 : 0), metric: metricLabel })
                : t("detail.week.cellPoints", { day, hour, points: v.toFixed(v < 10 ? 1 : 0), metric: metricLabel })
              : isReset
                ? t("detail.week.cellEmptyReset", { day, hour })
                : t("detail.week.cellEmpty", { day, hour });
          return `<span class="dt-heat${isReset ? " dt-heat-reset" : ""}" style="--level:${level.toFixed(3)}" title="${esc(tip)}"></span>`;
        })
        .join("");
      return `<span class="dt-heat-day">${esc(weekdayShort(d))}</span>${cells}`;
    })
    .join("");
  const busiest = profile.cells
    .flatMap((row, d) => row.map((v, h) => ({ v, d, h })))
    .sort((a, b) => b.v - a.v)[0];
  const note =
    busiest && busiest.v > 0
      ? reset
        ? t("detail.week.hardestWithReset", {
            day: weekdayShort(busiest.d),
            hour: hourLabel(busiest.h),
            resetDay: weekdayShort(resetDay),
            resetHour: hourLabel(resetHour),
          })
        : t("detail.week.hardest", { day: weekdayShort(busiest.d), hour: hourLabel(busiest.h) })
      : "";
  const fromDays = plural("detail.week.fromDays", profile.daysObserved);
  return `
    <div class="dt-controls">
      ${usable.length > 1 ? `<label>${esc(t("detail.control.limit"))} ${select("dt-burn-metric", usable.map((b): [string, string] => [b.metric, displayMetricLabel(b.metric)]), burnMetric)}</label>` : ""}
    </div>
    <div class="dt-heat-grid" role="img" aria-label="${esc(t("detail.week.ariaLabel", { metric: metricLabel }))}">
      <span></span>${[0, 6, 12, 18].map((h) => `<span class="dt-heat-hour" style="grid-column:${h + 2} / span 6">${esc(hourLabel(h))}</span>`).join("")}
      ${rows}
    </div>
    <div class="dt-heat-key"><span>${esc(t("detail.week.less"))}</span><i style="--level:0.16"></i><i style="--level:0.45"></i><i style="--level:0.75"></i><i style="--level:1"></i><span>${esc(t("detail.week.more"))}</span></div>
    <p class="dt-caption">${esc(note)} ${esc(fromDays)}</p>`;
}

// ---------------------------------------------------------------------------
// Clients
// ---------------------------------------------------------------------------

function allAreas(sp: ProviderSpend): AreaSpend[] {
  return (sp.projects ?? []).flatMap((p) => p.areas ?? []);
}

/// Runs a `git log` per work area, so it is fetched once when the Work area
/// view is first opened rather than on every render.
let effortLoaded = false;
async function loadEffort(): Promise<void> {
  if (effortLoaded) return;
  effortLoaded = true;
  try {
    effort = await invoke<AreaEffort[]>("get_effort");
  } catch {
    effort = [];
  }
  render();
}

async function loadClients(): Promise<void> {
  const sp = openId ? source?.spend(openId) : undefined;
  if (!sp) return;
  try {
    clientView = await invoke<ClientView>("client_rollup", { areas: allAreas(sp) });
  } catch (err) {
    clientNote = String(err);
  }
  render();
}

function clientSection(): string {
  if (!clientView) return `<p class="dt-empty">${esc(t("detail.loading"))}</p>`;
  const rows = clientView.rows
    .map((c) => ({
      // "Unassigned" is a fixed sentinel this app's Rust side sends (never a
      // user-typed name): stays as it arrives, like "(unsorted)" for areas.
      label: c.client,
      tip: `${c.client} (${c.areas.slice(0, 6).join(", ")}${c.areas.length > 6 ? ", …" : ""})`,
      cost: c[windowKey].cost,
      tokens: c[windowKey].tokens,
    }))
    .filter((r) => r.cost > 0.004 || r.tokens > 0);
  const chart = rows.length ? bars(rows) : `<p class="dt-empty">${esc(t("detail.spend.nothing"))}</p>`;
  const budgets = clientView.rules
    .filter((r) => r.monthlyBudget)
    .map((r) => {
      const spent = clientView!.rows.find((c) => c.client === r.client)?.monthToDate ?? 0;
      const over = spent >= r.monthlyBudget!;
      const key = over ? "detail.client.budgetOver" : "detail.client.budgetStatus";
      return `<p class="dt-caption${over ? " dt-forecast-hit" : ""}">${esc(t(key, { client: r.client, spent: money(spent), budget: money(r.monthlyBudget!) }))}</p>`;
    })
    .join("");

  const rules = draftRules ?? clientView.rules;
  const editing = draftRules !== null;
  const unassigned = clientView.rows.find((c) => c.client === "Unassigned");
  const hint =
    unassigned && unassigned.areas.length
      ? `<p class="dt-caption">${esc(
          unassigned.areas.length > 8
            ? plural("detail.client.unassignedHintMore", unassigned.areas.length - 8, {
                areas: unassigned.areas.slice(0, 8).join(", "),
              })
            : t("detail.client.unassignedHint", { areas: unassigned.areas.slice(0, 8).join(", ") }),
        )}</p>`
      : "";
  const editor = editing
    ? `<div class="dt-rules">${rules
        .map(
          (r, i) => `
        <div class="dt-rule">
          <input data-rule-client="${i}" type="text" maxlength="96" placeholder="${esc(t("detail.group.client"))}" value="${esc(r.client)}" aria-label="${esc(t("detail.client.nameAria"))}" />
          <input data-rule-patterns="${i}" type="text" placeholder="${esc(t("detail.client.patternsPh"))}" value="${esc(r.patterns.join(", "))}" aria-label="${esc(t("detail.client.patterns"))}" />
          <input data-rule-budget="${i}" type="number" min="0" step="1" placeholder="${esc(t("detail.client.budgetPh"))}" value="${r.monthlyBudget ? r.monthlyBudget : ""}" aria-label="${esc(t("detail.client.budgetAria"))}" title="${esc(t("detail.client.budgetTip"))}" />
        </div>`,
        )
        .join("")}
        <p class="dt-caption">${esc(t("detail.client.rulesCaption"))}</p>
        ${clientNote ? `<p class="lg-error" role="alert">${esc(clientNote)}</p>` : ""}
        <div class="dt-rule-actions">
          <button class="inv-learn" id="dt-rule-add">${esc(t("detail.client.add"))}</button>
          <span class="spacer"></span>
          <button class="inv-learn" id="dt-rule-cancel">${esc(t("dialog.cancel"))}</button>
          <button class="lg-save" id="dt-rule-save">${esc(t("settings.save"))}</button>
        </div></div>`
    : `<div class="dt-rule-actions">
        <button class="inv-learn" id="dt-rule-edit">${rules.length ? esc(t("detail.client.edit", { n: rules.length })) : esc(t("detail.client.setUp"))}</button>
        <span class="spacer"></span>
        <button class="inv-learn" id="dt-export" title="${esc(t("detail.csv.saveTableTip"))}">${esc(t("detail.csv.export"))}</button>
      </div>${clientNote ? `<p class="dt-caption">${esc(clientNote)}</p>` : ""}`;
  return `${chart}${budgets}${hint}${editor}`;
}

function readDraft(root: HTMLElement): ClientRule[] {
  const names = [...root.querySelectorAll<HTMLInputElement>("[data-rule-client]")];
  return names.map((el) => {
    const i = el.dataset.ruleClient!;
    const patterns = root.querySelector<HTMLInputElement>(`[data-rule-patterns="${i}"]`)?.value ?? "";
    const budget = Number(root.querySelector<HTMLInputElement>(`[data-rule-budget="${i}"]`)?.value ?? "");
    return {
      client: el.value,
      patterns: patterns.split(",").map((p) => p.trim()).filter(Boolean),
      monthlyBudget: Number.isFinite(budget) && budget > 0 ? budget : null,
    };
  });
}

// ---------------------------------------------------------------------------
// Sessions drill-down
// ---------------------------------------------------------------------------

/// [when it started, how long from first to last message]. A session can sit
/// open for weeks, so the second is a span, not time worked.
function span(s: SessionSpend): [string, string] {
  if (s.startedMs === null || s.endedMs === null) return ["", ""];
  const start = new Date(s.startedMs);
  const mins = Math.max(Math.round((s.endedMs - s.startedMs) / 60_000), 1);
  // mins >= 2880 (48h) here, so the day count is always >= 2; detail.session.spanDays.one
  // exists for task 8's real plural rules and is unreachable through this branch today.
  const length =
    mins >= 2880
      ? t("detail.session.spanDays.other", { n: Math.round(mins / 1440) })
      : mins >= 60
        ? t("time.hoursMins", { h: Math.floor(mins / 60), m: mins % 60 })
        : t("time.mins", { m: mins });
  const day = start.toLocaleDateString(localeTag(), { month: "short", day: "numeric" });
  const time = start.toLocaleTimeString(localeTag(), { hour: "numeric", minute: "2-digit" });
  return [t("detail.session.startedAt", { day, time }), t("detail.session.spans", { length })];
}

/// "active today", "active yesterday", "last active 12 days ago": when the
/// session last wrote a line. An old session still being appended to is the
/// one worth finding, and this is what gives it away.
function lastActive(s: SessionSpend): string {
  if (s.endedMs === null) return "";
  const days = Math.floor((Date.now() - s.endedMs) / 86_400_000);
  if (days <= 0) return t("detail.session.activeToday");
  if (days === 1) return t("detail.session.activeYesterday");
  // days is always >= 2 here (0 and 1 handled above); detail.session.lastActive.one
  // exists for task 8's real plural rules and is unreachable through this branch today.
  return t("detail.session.lastActive.other", { n: days });
}

/// MB/KB stay English: format tokens, not prose, same as tokens()'s B/M/K and money()'s $.
function fileSize(bytes: number): string {
  return bytes >= 1_048_576 ? `${(bytes / 1_048_576).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/// One row per session. Reveal shows the log file; nothing here deletes.
function sessionRows(sessions: SessionSpend[]): string {
  return sessions
    .map((s) => {
      const shown = s.dayCost ?? s.cost;
      const where = s.areas.slice(0, 3).map(([a]) => a).join(", ");
      const [started, length] = span(s);
      const facts = [
        length,
        lastActive(s),
        s.topModel ? t("detail.session.mostly", { model: s.topModel }) : "",
        where,
        s.dayCost !== null ? t("detail.session.over30Days", { money: money(s.cost) }) : "",
        s.bytes ? fileSize(s.bytes) : "",
      ]
        .filter(Boolean)
        .map(esc)
        .join(" · ");
      return `
      <div class="dt-session">
        <div class="lg-item-head"><span class="inv-name">${esc(started || t("detail.session.timeNotRecorded"))}</span><span class="lg-price">${money(shown)}</span></div>
        <div class="dt-session-sub">${facts} <button class="inv-chip" data-reveal-session="${esc(s.id)}" title="${esc(t("detail.session.revealTitle", { where: revealWord() }))}">${esc(t("detail.session.reveal"))}</button></div>
      </div>`;
    })
    .join("");
}

function drillSection(): string {
  const d = drill!;
  const head = `<div class="dt-drill-head"><button class="inv-learn" id="dt-drill-back">${esc(t("detail.panel.back"))}</button><span>${esc(t("detail.session.drillTitle", { title: d.title }))}</span></div>`;
  if (!d.sessions) return `${head}<p class="dt-empty">${esc(t("detail.loading"))}</p>`;
  if (!d.sessions.length) return `${head}<p class="dt-empty">${esc(t("detail.session.drillEmpty"))}</p>`;
  return `${head}${sessionRows(d.sessions)}<p class="dt-caption">${esc(t("detail.session.drillCaption"))}</p>`;
}

/// Group by → Session: every session of the last 30 days, heaviest first.
/// The janitor's view, minus the broom: the app shows you the file and you
/// decide, because deleting a session log also deletes the spend it produced.
function sessionsSection(): string {
  if (!sessionsAll) return `<p class="dt-empty">${esc(t("detail.loading"))}</p>`;
  if (!sessionsAll.length) return `<p class="dt-empty">${esc(t("detail.session.noneIn30"))}</p>`;
  lastTable = {
    // The CSV file name is not user-facing chrome; it stays English.
    name: "sessions last 30 days",
    headers: [
      t("detail.csv.started"),
      t("detail.csv.lastActive"),
      t("detail.csv.length"),
      t("detail.csv.costUsd"),
      t("detail.csv.tokens"),
      t("detail.csv.topModel"),
      t("detail.csv.areas"),
      t("detail.csv.sizeBytes"),
      t("detail.csv.sessionId"),
    ],
    rows: sessionsAll.map((s) => {
      const [started, length] = span(s);
      return [
        started,
        s.endedMs === null ? "" : new Date(s.endedMs).toISOString(),
        length,
        s.cost.toFixed(4),
        String(Math.round(s.tokens)),
        s.topModel ?? "",
        s.areas.map(([a]) => a).join("; "),
        s.bytes === undefined ? "" : String(s.bytes),
        s.id,
      ];
    }),
  };
  const note = sessionNote ? `<p class="dt-caption">${esc(sessionNote)}</p>` : "";
  return `${sessionRows(sessionsAll)}${note}<p class="dt-caption">${esc(t("detail.session.allCaption", { where: revealWord() }))}</p>`;
}

async function loadSessions(): Promise<void> {
  try {
    sessionsAll = await invoke<SessionSpend[]>("get_sessions", { area: null, day: null });
  } catch (err) {
    sessionsAll = [];
    sessionNote = String(err);
  }
  render();
}

async function openDrill(next: { title: string; area?: string; day?: string }): Promise<void> {
  drill = { ...next, sessions: null };
  render();
  try {
    const sessions = await invoke<SessionSpend[]>("get_sessions", { area: next.area ?? null, day: next.day ?? null });
    if (drill && drill.title === next.title) drill.sessions = sessions;
  } catch {
    if (drill) drill.sessions = [];
  }
  render();
}

// ---------------------------------------------------------------------------
// Spend
// ---------------------------------------------------------------------------

function bars(rows: { label: string; tip: string; cost: number; tokens: number; drillArea?: string }[]): string {
  const groupHeader =
    groupKey === "model"
      ? t("detail.group.model")
      : groupKey === "client"
        ? t("detail.group.client")
        : groupKey === "project"
          ? t("detail.group.project")
          : t("detail.group.area");
  lastTable = {
    // The CSV file name is not user-facing chrome; it stays English.
    name: `spend by ${groupKey} ${windowKey}`,
    headers: [groupHeader, t("detail.csv.costUsd"), t("detail.csv.tokens")],
    rows: rows.map((r) => [r.tip, r.cost.toFixed(2), String(Math.round(r.tokens))]),
  };
  const max = Math.max(...rows.map((r) => r.cost), 0.0001);
  return `<div class="dt-bars">${rows
    .map((r) => {
      const tip = r.drillArea
        ? t("detail.spend.barTipDrill", { label: r.tip, money: money(r.cost), tokens: tokens(r.tokens) })
        : t("detail.spend.barTip", { label: r.tip, money: money(r.cost), tokens: tokens(r.tokens) });
      return `
      <div class="dt-bar-row${r.drillArea ? " dt-drillable" : ""}"${r.drillArea ? ` data-drill-area="${esc(r.drillArea)}" role="button" tabindex="0"` : ""} title="${esc(tip)}">
        <span class="dt-bar-label">${esc(r.label)}</span>
        <span class="dt-bar-track"><span class="dt-bar" style="width:${Math.max((r.cost / max) * 100, r.cost > 0 ? 1.5 : 0)}%"></span></span>
        <span class="dt-bar-value">${money(r.cost)}</span>
      </div>`;
    })
    .join("")}</div>`;
}

function dayBars(daily: number[]): string {
  const max = Math.max(...daily, 0.0001);
  const today = new Date();
  lastTable = {
    // The CSV file name is not user-facing chrome; it stays English.
    name: "spend by day",
    headers: [t("detail.csv.date"), t("detail.csv.costUsd")],
    rows: daily.map((cost, i) => {
      const d = new Date(today);
      d.setDate(today.getDate() - (daily.length - 1 - i));
      return [`${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`, cost.toFixed(2)];
    }),
  };
  const cols = daily
    .map((cost, i) => {
      const d = new Date(today);
      d.setDate(today.getDate() - (daily.length - 1 - i));
      const label = d.toLocaleDateString(localeTag(), { month: "short", day: "numeric" });
      const iso = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
      const can = cost > 0.004;
      const tip = can
        ? t("detail.spend.dayTipDrill", { label, money: money(cost) })
        : t("detail.spend.dayTip", { label, money: money(cost) });
      return `<span class="dt-day${can ? " dt-drillable" : ""}"${can ? ` data-drill-day="${iso}" data-drill-label="${esc(label)}" role="button" tabindex="0"` : ""} title="${esc(tip)}"><span style="height:${Math.max((cost / max) * 100, cost > 0 ? 2 : 0)}%"></span></span>`;
    })
    .join("");
  const total = daily.reduce((a, b) => a + b, 0);
  return `<div class="dt-days">${cols}</div>
    <div class="dt-days-axis"><span>${esc(t("detail.spend.axisStart"))}</span><span>${esc(t("detail.spend.axisTotal", { total: money(total), peak: money(max) }))}</span><span>${esc(t("detail.spend.axisToday"))}</span></div>`;
}

/// Dollars per commit for the folders that are in git. A ratio, not a verdict:
/// one commit can be a day's refactor and ten can be typo fixes, so the copy
/// gives the number and the caveat and draws no conclusion.
function effortCaption(): string {
  const rows = effort.filter((e) => e.costPerCommit !== null).slice(0, 4);
  if (!rows.length) return "";
  const parts = rows.map((e) => {
    const commits = plural("detail.effort.commits", e.commits!);
    return t("detail.effort.row", { area: esc(e.area), money: money(e.costPerCommit!), commits });
  });
  // Not esc()-wrapped: each part already went through t()+esc(area) above, and
  // "&middot;" is a deliberate HTML entity — escaping again would show it literally.
  return `<p class="dt-caption">${t("detail.effort.caption", { parts: parts.join(" &middot; ") })}</p>`;
}

/// "up 22% on last week". Absent when there is no fortnight to compare, and
/// wordless when last week was zero: a rise from nothing has no percentage.
function weekLine(week: WeekDelta | null | undefined): string {
  if (!week) return "";
  if (week.changePercent === null) {
    return week.thisWeek > 0
      ? `<p class="dt-caption">${esc(t("detail.spend.weekNoPrior", { money: money(week.thisWeek) }))}</p>`
      : "";
  }
  const pct = week.changePercent;
  const cls = pct >= 0 ? "dt-week-up" : "dt-week-down";
  const dir =
    pct >= 0 ? t("detail.spend.dirUp", { pct: Math.abs(pct).toFixed(0) }) : t("detail.spend.dirDown", { pct: Math.abs(pct).toFixed(0) });
  // The coloured span is built here, not in the JSON, so the three locale
  // values stay plain text with {money}/{dir}/{lastMoney} and never carry markup.
  const dirHtml = `<span class="${cls}">${esc(dir)}</span>`;
  // Not esc()-wrapped: money/lastMoney are already esc()'d above and dirHtml is
  // deliberate markup, not text — escaping the t() result would turn its <span> literal.
  return `<p class="dt-caption">${t("detail.spend.weekChange", { money: esc(money(week.thisWeek)), dir: dirHtml, lastMoney: esc(money(week.lastWeek)) })}</p>`;
}

function spendSection(sp: ProviderSpend | undefined): string {
  if (!sp || (sp.last30.cost < 0.005 && sp.last30.tokens <= 0)) {
    return `<p class="dt-empty">${esc(t("detail.spend.noLogs"))}</p>`;
  }
  if (drill) return drillSection();
  const hasProjects = (sp.projects?.length ?? 0) > 0;
  const hasAreas = (sp.projects ?? []).some((p) => (p.areas?.length ?? 0) > 0);
  if (groupKey === "project" && !hasProjects) groupKey = "model";
  if (groupKey === "area" && !hasAreas) groupKey = "model";
  if (groupKey === "client" && !hasAreas) groupKey = "model";
  const groups: [string, string][] = [["model", t("detail.group.model")]];
  if (hasAreas) groups.push(["client", t("detail.group.client")], ["area", t("detail.group.area")]);
  if (hasProjects) groups.push(["project", t("detail.group.project")]);
  groups.push(["day", t("detail.group.day")], ["session", t("detail.group.session")]);
  const controls = `<div class="dt-controls">
      <label>${esc(t("detail.control.groupBy"))} ${select("dt-group", groups, groupKey)}</label>
      ${groupKey === "day" || groupKey === "session" ? "" : `<label>${esc(t("detail.control.period"))} ${select("dt-window", WINDOWS.map(([v, k]): [string, string] => [v, t(k)]), windowKey)}</label>`}
      ${groupKey === "area" ? `<label>${esc(t("detail.control.detail"))} ${select("dt-depth", [["1", t("detail.control.topFolders")], ["2", t("detail.control.twoLevels")]], areaDepth)}</label>` : ""}
    </div>`;

  const week = weekLine(sp.week);
  let body: string;
  if (groupKey === "day") {
    body = dayBars(sp.daily_cost ?? []);
  } else if (groupKey === "session") {
    body = sessionsSection();
  } else if (groupKey === "client") {
    body = clientSection();
  } else if (groupKey === "area") {
    const many = (sp.projects?.length ?? 0) > 1;
    // The log keeps two folder levels; "Top folders" adds them back up.
    const merged = new Map<string, { label: string; tip: string; cost: number; tokens: number }>();
    for (const p of sp.projects ?? []) {
      for (const a of p.areas ?? []) {
        const name = areaDepth === "1" ? a.area.split("/")[0] : a.area;
        // With several projects an area name alone is ambiguous.
        const label = many ? `${projectLabel(p.project)} / ${name}` : name;
        const row = merged.get(label) ?? { label, tip: `${p.project} / ${name}`, cost: 0, tokens: 0, drillArea: name };
        row.cost += a[windowKey].cost;
        row.tokens += a[windowKey].tokens;
        merged.set(label, row);
      }
    }
    const all = [...merged.values()]
      .filter((r) => r.cost > 0.004 || r.tokens > 0)
      .sort((a, b) => b.cost - a.cost);
    const top = all.slice(0, MAX_BAR_ROWS);
    const rest = all.slice(MAX_BAR_ROWS);
    if (rest.length) {
      top.push({
        label: plural("detail.other", rest.length),
        tip: plural("detail.spend.smallerAreas", rest.length),
        cost: rest.reduce((n, r) => n + r.cost, 0),
        tokens: rest.reduce((n, r) => n + r.tokens, 0),
      });
    }
    body = top.length ? bars(top) : `<p class="dt-empty">${esc(t("detail.spend.nothing"))}</p>`;
    body += effortCaption();
    body += `<p class="dt-caption">${esc(t("detail.spend.areaCaption"))}</p>`;
  } else if (groupKey === "project") {
    const rows = (sp.projects ?? [])
      .map((p) => ({ label: projectLabel(p.project), tip: p.project, cost: p[windowKey].cost, tokens: p[windowKey].tokens }))
      .filter((r) => r.cost > 0.004 || r.tokens > 0)
      .sort((a, b) => b.cost - a.cost);
    body = rows.length ? bars(rows) : `<p class="dt-empty">${esc(t("detail.spend.nothing"))}</p>`;
    if (rows.length === 1) {
      body += `<p class="dt-caption">${esc(t("detail.spend.projectSingleCaption"))}</p>`;
    }
  } else {
    const rows = [...sp[windowKey].models]
      .sort((a, b) => b.cost - a.cost)
      .slice(0, 12)
      .map((m) => ({ label: m.model, tip: m.model, cost: m.cost, tokens: m.tokens }));
    body = rows.length ? bars(rows) : `<p class="dt-empty">${esc(t("detail.spend.nothing"))}</p>`;
  }
  const w = sp[windowKey];
  const headline =
    groupKey === "day"
      ? ""
      : `<div class="dt-headline"><b>${money(w.cost)}</b><span>${esc(t("card.tokens", { n: tokens(w.tokens) }))}</span></div>`;
  const exportBtn =
    groupKey === "client" || !lastTable
      ? ""
      : `<div class="dt-rule-actions"><span class="spacer"></span><button class="inv-learn" id="dt-export-table" title="${esc(t("detail.csv.saveShownTip"))}">${esc(t("detail.csv.export"))}</button></div>${clientNote ? `<p class="dt-caption">${esc(clientNote)}</p>` : ""}`;
  return `${controls}${headline}${week}${body}${exportBtn}
    <p class="dt-caption">${esc(t("detail.spend.footer"))}</p>`;
}

// ---------------------------------------------------------------------------
// Card expander: the short version, inline under an expanded card
// ---------------------------------------------------------------------------

const SPARK_HOURS = 24;
/// Models listed on the card before the rest fold into "Other".
const MINI_MODELS = 4;
const sparkCache = new Map<string, { at: number; series: Series[] }>();
const sparkLoading = new Set<string>();

function miniHtml(id: string): string {
  const cached = sparkCache.get(id);
  // Copies: clipping below must not eat into the cached readings.
  const series = (cached?.series ?? [])
    .slice(0, SERIES_VARS.length)
    .map((s) => ({ ...s }))
    .filter((s) => s.points.length > 1);
  let chart: string;
  if (!cached) {
    chart = `<p class="dt-mini-note">${esc(t("detail.card.loading24h"))}</p>`;
  } else if (series.length === 0) {
    chart = `<p class="dt-mini-note">${esc(t("detail.card.noTrend"))}</p>`;
  } else {
    const W = 300;
    const H = 36;
    const t1 = Date.now();
    const t0 = t1 - SPARK_HOURS * 3_600_000;
    for (const s of series) s.points = clip(s.points, t0);
    const x = (at: number) => ((Math.max(at, t0) - t0) / (t1 - t0)) * W;
    const y = (used: number) => 2 + (1 - used / 100) * (H - 4);
    const paths = series
      .map((s, slot) => {
        const d = s.points.map((p, i) => `${i ? "L" : "M"}${x(p.at).toFixed(1)} ${y(p.used).toFixed(1)}`).join("");
        return `<path class="dt-line dt-spark-line" style="stroke:var(${SERIES_VARS[slot]})" d="${d}"/>`;
      })
      .join("");
    // Identity is never colour alone: each line is named with its value.
    const keys = series
      .map((s, slot) => {
        const last = s.points[s.points.length - 1];
        return `<span class="dt-key"><i style="background:var(${SERIES_VARS[slot]})"></i>${esc(displayMetricLabel(s.metric))} <b>${last.used.toFixed(0)}%</b></span>`;
      })
      .join("");
    chart = `<svg class="dt-spark" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img" aria-label="${esc(t("detail.card.sparkAriaLabel"))}">${paths}</svg>
      <div class="dt-legend dt-mini-legend"><span class="dt-mini-span">${esc(t("detail.card.sparkLegend"))}</span>${keys}</div>`;
  }
  const sp = source?.spend(id);
  const wall = (forecasts.get(id) ?? [])
    .filter((f) => f.hitsLimitAt !== null)
    .sort((a, b) => a.hitsLimitAt! - b.hitsLimitAt!)[0];
  const warn = wall ? `<p class="dt-mini-note dt-forecast-hit">${esc(forecastText(wall, true))}</p>` : "";
  const facts =
    sp && sp.today.cost > 0.004 ? `<span>${esc(t("detail.card.todaySpend", { money: money(sp.today.cost) }))}</span>` : `<span></span>`;
  return `${chart}${warn}${modelSplit(sp)}<div class="dt-mini-foot">${facts}<button class="inv-learn dt-mini-open" data-detail="${esc(id)}">${esc(t("detail.card.detailsBtn"))}</button></div>`;
}

async function loadSpark(id: string): Promise<void> {
  if (sparkLoading.has(id)) return;
  sparkLoading.add(id);
  try {
    const [series] = await Promise.all([
      invoke<Series[]>("get_history", { providerId: id, hours: SPARK_HOURS }),
      loadForecast(id),
    ]);
    sparkCache.set(id, { at: Date.now(), series });
  } catch {
    sparkCache.set(id, { at: Date.now(), series: [] });
  }
  sparkLoading.delete(id);
  // Patch in place: a full re-render here would loop back into this load.
  document.querySelectorAll<HTMLElement>("[data-mini]").forEach((el) => {
    if (el.dataset.mini === id) el.innerHTML = miniHtml(id);
  });
}

/// The inline extras for an expanded card. Synchronous for the card
/// renderer: it draws from cache and refreshes the cache in the background.
/// Every model the local logs saw today, not just the busiest one. The vendor
/// only publishes a limit for one model at a time, so this is the only place
/// the whole picture exists. Shares are of today's cost; a model with cost but
/// no measurable share still gets a row, because "it ran" is the useful fact.
function modelSplit(sp: ProviderSpend | undefined): string {
  if (!sp || sp.today.cost <= 0.004) return "";
  const all = [...sp.today.models].sort((a, b) => b.cost - a.cost).filter((m) => m.cost > 0 || m.tokens > 0);
  if (all.length < 2) return "";
  const shown = all.slice(0, MINI_MODELS);
  const rest = all.slice(MINI_MODELS);
  if (rest.length) {
    shown.push({
      model: plural("detail.other", rest.length),
      cost: rest.reduce((n, m) => n + m.cost, 0),
      tokens: rest.reduce((n, m) => n + m.tokens, 0),
    });
  }
  const total = sp.today.cost || 1;
  const rows = shown
    .map((m) => {
      const pct = Math.round((m.cost / total) * 100);
      const tip = t("detail.card.modelTip", { model: m.model, money: money(m.cost), tokens: tokens(m.tokens) });
      return `<div class="dt-ms-row" title="${esc(tip)}">
        <span class="dt-ms-name">${esc(m.model)}</span>
        <span class="dt-ms-bar"><i style="--w:${Math.max(2, (m.cost / total) * 100).toFixed(1)}%"></i></span>
        <span class="dt-ms-val">${pct}%</span>
      </div>`;
    })
    .join("");
  return `<div class="dt-ms"><div class="dt-ms-head">${esc(t("detail.card.modelsToday"))}</div>${rows}</div>`;
}

export function cardExtras(id: string): string {
  const cached = sparkCache.get(id);
  if (!cached || Date.now() - cached.at > 5 * 60_000) void loadSpark(id);
  return `<div class="dt-mini" data-mini="${esc(id)}">${miniHtml(id)}</div>`;
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

function nowSection(snap: Snapshot): string {
  const rows = snap.metrics
    .map((m) => {
      if (m.kind === "progress" && m.used_percent !== null) {
        const used = Math.min(Math.max(m.used_percent, 0), 100);
        const reset = m.resets_at ? resetText(m.resets_at) : "";
        const detail = displayMetricDetail(m.detail ?? "");
        return `<div class="dt-now-row"><span class="dt-now-label">${esc(displayMetricLabel(m.label))}</span>
          <span class="dt-now-facts">${[t("card.pctUsed", { n: used.toFixed(0) }), detail, reset].filter(Boolean).map(esc).join(" · ")}</span></div>`;
      }
      if (m.kind === "text" && m.value) {
        return `<div class="dt-now-row"><span class="dt-now-label">${esc(displayMetricLabel(m.label))}</span><span class="dt-now-facts">${esc(m.value)}</span></div>`;
      }
      return "";
    })
    .join("");
  return rows || `<p class="dt-empty">${esc(t("detail.now.empty"))}</p>`;
}

/// `fromRefresh`: the redraw was caused by new data, not by something the
/// user did. Only then is the rules editor read back first, so typing
/// survives a refresh; after a click the handler has already set the draft
/// (reading the screen back would undo a row that was just added).
function render(fromRefresh = false): void {
  const el = document.querySelector<HTMLElement>("#detail-body");
  const title = document.querySelector<HTMLElement>("#detail-title");
  if (!el || !openId || !source) return;
  if (fromRefresh && draftRules !== null && el.querySelector("[data-rule-client]")) {
    draftRules = readDraft(el);
  }
  const snap = source.snapshot(openId);
  if (!snap) {
    close();
    return;
  }
  const plan = snap.plan ? snap.plan.charAt(0).toUpperCase() + snap.plan.slice(1) : "";
  if (title) title.textContent = plan ? `${snap.name} · ${plan}` : snap.name;
  const metrics: [string, string][] = [
    ["__all__", t("detail.control.allLimits")],
    ...history.map((s): [string, string] => [s.metric, s.metric]),
  ];
  if (!metrics.some(([v]) => v === metricFilter)) metricFilter = "__all__";
  const width = Math.floor(el.clientWidth) - 28;
  el.innerHTML = `
    <section class="dt-section">
      <h3>${esc(t("detail.section.limitsOverTime"))}</h3>
      <div class="dt-controls">
        <label>${esc(t("detail.control.range"))} ${select("dt-range", RANGES.map(([h, l]): [string, string] => [String(h), t(l)]), String(hours))}</label>
        ${history.length > 1 ? `<label>${esc(t("detail.control.limit"))} ${select("dt-metric", metrics, metricFilter)}</label>` : ""}
      </div>
      ${loading && historyFor !== openId ? `<p class="dt-empty">${esc(t("detail.loading"))}</p>` : lineChart(history, width)}
      ${forecastSection(openId)}
    </section>
    <section class="dt-section">
      <h3>${esc(t("detail.section.yourWeek"))}</h3>
      ${burnFor === openId ? weekSection(snap) : `<p class="dt-empty">${esc(t("detail.loading"))}</p>`}
    </section>
    <section class="dt-section">
      <h3>${esc(t("detail.section.spend"))}</h3>
      ${spendSection(source.spend(openId))}
    </section>
    <section class="dt-section">
      <h3>${esc(t("detail.section.rightNow"))}</h3>
      ${nowSection(snap)}
    </section>`;
  wireCrosshair(el);
}

async function loadHistory(): Promise<void> {
  if (!openId) return;
  const id = openId;
  loading = true;
  lastLoad = Date.now();
  try {
    const [got, week] = await Promise.all([
      invoke<Series[]>("get_history", { providerId: id, hours }),
      invoke<BurnProfile[]>("get_burn_profile", { providerId: id }).catch(() => [] as BurnProfile[]),
      loadForecast(id),
    ]);
    if (openId !== id) return;
    burn = week;
    burnFor = id;
    history = got;
    historyFor = id;
  } catch {
    history = [];
  }
  loading = false;
  render(true);
}

function markSelected(): void {
  document.querySelectorAll<HTMLElement>("#providers [data-provider]").forEach((card) => {
    card.classList.toggle("dt-selected", wide && card.dataset.provider === openId);
  });
}

function open(id: string): void {
  if (openId !== id) {
    drill = null;
    sessionsAll = null;
    sessionNote = "";
  }
  openId = id;
  if (historyFor !== id) history = [];
  document.body.classList.add("detail-open");
  markSelected();
  render();
  void loadHistory();
}

function close(): void {
  // In wide mode the column is permanent: there is nothing to go back to.
  if (wide) return;
  openId = null;
  document.body.classList.remove("detail-open");
}

/// Grows or shrinks the one window. The native resize comes first so the
/// page never lays out for a width the window does not have yet.
async function setWide(next: boolean, save: boolean): Promise<void> {
  try {
    await invoke("set_wide", { wide: next });
  } catch {
    return; // the window did not change, so neither does the layout
  }
  wide = next;
  document.body.classList.toggle("wide", wide);
  const btn = document.querySelector<HTMLElement>("#detail-wide");
  if (btn) {
    btn.textContent = wide ? t("detail.panel.narrow") : t("detail.panel.wide");
    btn.setAttribute("aria-pressed", String(wide));
  }
  if (save) source?.saveWide(wide);
  if (wide && !openId) {
    const first = source?.firstId();
    if (first) open(first);
  } else {
    markSelected();
    if (openId) render();
  }
}

/// Keeps an open detail page current. The app re-renders on a timer as well
/// as on new data, so reloads are spaced out: redrawing under the pointer
/// would keep wiping the crosshair.
export function refreshDetail(): void {
  // Cards were just re-rendered, so the selection mark has to go back on.
  markSelected();
  if (wide && !openId) {
    const first = source?.firstId();
    if (first) open(first);
    return;
  }
  if (!openId) return;
  if (groupKey === "session") void loadSessions();
  if (Date.now() - lastLoad < 60_000) return;
  void loadHistory();
}

/// Redraws the open detail page in place, e.g. after a locale switch (task 7
/// wires this into the locale-change handler). A no-op while nothing is open.
export function rerender(): void {
  if (openId) render();
}

/// Restores the saved wide-mode choice. Call once the config has loaded:
/// `setupDetail` runs before that, when the choice still reads as its default.
export function applySavedWide(): void {
  if (source?.wide()) void setWide(true, false);
}

export function setupDetail(src: DetailSource): void {
  source = src;
  // A card's name is the way in. Keyboard users get the same through Enter.
  document.querySelector("#providers")?.addEventListener("click", (e) => {
    const more = (e.target as HTMLElement).closest<HTMLElement>("[data-detail]");
    if (more?.dataset.detail) {
      open(more.dataset.detail);
      return;
    }
    const name = (e.target as HTMLElement).closest<HTMLElement>(".provider-name");
    const card = name?.closest<HTMLElement>("[data-provider]");
    if (card?.dataset.provider) open(card.dataset.provider);
  });
  document.querySelector("#detail-close")?.addEventListener("click", close);
  document.querySelector("#detail-wide")?.addEventListener("click", () => void setWide(!wide, true));
  document.querySelector("#wide-btn")?.addEventListener("click", () => void setWide(!wide, true));
  // Esc backs out of the page before it is allowed to hide the whole window.
  document.addEventListener(
    "keydown",
    (e) => {
      // Wide mode has no page to back out of, so Esc keeps its usual job.
      if (e.key === "Escape" && openId && !wide) {
        e.stopImmediatePropagation();
        e.preventDefault();
        close();
      }
    },
    true,
  );
  document.querySelector("#detail-body")?.addEventListener("change", (e) => {
    const t = e.target as HTMLSelectElement;
    if (t.id === "dt-range") {
      hours = Number(t.value);
      void loadHistory();
      return;
    }
    if (t.id === "dt-group" && t.value === "client") {
      groupKey = "client";
      void loadClients();
      return;
    }
    if (t.id === "dt-group" && t.value === "session") {
      groupKey = "session";
      sessionNote = "";
      void loadSessions();
      return;
    }
    clientNote = "";
    if (t.id === "dt-burn-metric") burnMetric = t.value;
    else if (t.id === "dt-depth") areaDepth = t.value as "1" | "2";
    else if (t.id === "dt-metric") metricFilter = t.value;
    else if (t.id === "dt-window") windowKey = t.value as WindowKey;
    else if (t.id === "dt-group") {
      groupKey = t.value as GroupKey;
      if (groupKey === "area") void loadEffort();
    } else return;
    render();
  });
  document.querySelector("#detail-body")?.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const root = document.querySelector<HTMLElement>("#detail-body")!;
    const revealId = target.closest<HTMLElement>("[data-reveal-session]")?.dataset.revealSession;
    if (revealId) {
      void invoke("reveal_session", { id: revealId }).catch((err) => {
        sessionNote = String(err);
        render();
      });
      return;
    }
    const drillArea = target.closest<HTMLElement>("[data-drill-area]")?.dataset.drillArea;
    const drillDay = target.closest<HTMLElement>("[data-drill-day]");
    if (target.closest("#dt-drill-back")) {
      drill = null;
    } else if (drillArea) {
      void openDrill({ title: drillArea, area: drillArea });
      return;
    } else if (drillDay?.dataset.drillDay) {
      void openDrill({ title: drillDay.dataset.drillLabel ?? drillDay.dataset.drillDay, day: drillDay.dataset.drillDay });
      return;
    } else if (target.closest("#dt-rule-edit")) {
      draftRules = clientView?.rules.length ? clientView.rules.map((r) => ({ ...r })) : [{ client: "", patterns: [] }];
      clientNote = "";
    } else if (target.closest("#dt-rule-add")) {
      draftRules = [...readDraft(root), { client: "", patterns: [] }];
    } else if (target.closest("#dt-rule-cancel")) {
      draftRules = null;
      clientNote = "";
    } else if (target.closest("#dt-rule-save")) {
      const rules = readDraft(root);
      void invoke("save_clients", { rules }).then(
        () => {
          draftRules = null;
          clientNote = "";
          return loadClients();
        },
        (err) => {
          draftRules = rules; // keep what was typed
          clientNote = String(err);
          render();
        },
      );
      return;
    } else if (target.closest("#dt-export-table")) {
      if (!lastTable) return;
      void invoke<string>("export_table", lastTable).then(
        (path) => { clientNote = t("detail.csv.saved", { path }); render(); },
        (err) => { clientNote = String(err); render(); },
      );
      return;
    } else if (target.closest("#dt-export")) {
      const sp = openId ? source?.spend(openId) : undefined;
      if (!sp) return;
      void invoke<string>("export_clients_csv", { areas: allAreas(sp) }).then(
        (path) => { clientNote = t("detail.csv.saved", { path }); render(); },
        (err) => { clientNote = String(err); render(); },
      );
      return;
    } else {
      return;
    }
    render();
  });
  document.querySelector("#detail-body")?.addEventListener("keydown", (e) => {
    const key = (e as KeyboardEvent).key;
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-drill-area], [data-drill-day]");
    if (el && (key === "Enter" || key === " ")) {
      e.preventDefault();
      el.click();
    }
  });
  new ResizeObserver(() => openId && draftRules === null && render()).observe(document.body);
}
