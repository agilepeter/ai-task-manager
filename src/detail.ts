// Detail view: one tool, in depth, inside the same window. Limits charted
// over time from the local history store, and spend broken down by model,
// project or day. Opened by clicking a card's name.
//
// Chart rules followed here: one axis; series colours are assigned in fixed
// order and validated for colour-blind separation on this app's surfaces; text
// always wears ink tokens, never a series colour; two or more series get a
// legend with live values; line charts get a crosshair, bars a per-mark tip.

import { invoke } from "@tauri-apps/api/core";

interface Metric {
  label: string;
  kind: string;
  used_percent: number | null;
  detail: string | null;
  value: string | null;
  resets_at: number | null;
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

interface ProviderSpend {
  id: string;
  today: SpendWindow;
  yesterday: SpendWindow;
  last30: SpendWindow;
  daily_cost?: number[];
  projects?: { project: string; today: SpendWindow; yesterday: SpendWindow; last30: SpendWindow }[];
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
type GroupKey = "model" | "project" | "day";

const RANGES: [hours: number, label: string][] = [
  [24, "Last 24 hours"],
  [24 * 7, "Last 7 days"],
  [24 * 30, "Last 30 days"],
  [24 * 90, "Last 90 days"],
];
const WINDOWS: [WindowKey, string][] = [
  ["today", "Today"],
  ["yesterday", "Yesterday"],
  ["last30", "Last 30 days"],
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
/** Wide mode: the window is twice as wide and this page is a fixed right column. */
let wide = false;

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

/// Whole dollars from $10 up, cents below: a column of values then reads at
/// one precision per magnitude instead of "$929" beside "$11.0".
function money(n: number): string {
  return n >= 10 ? `$${Math.round(n).toLocaleString()}` : `$${n.toFixed(2)}`;
}

function tokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(0)}K`;
  return String(Math.round(n));
}

function when(ms: number, spanHours: number): string {
  const d = new Date(ms);
  return spanHours <= 48
    ? d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
    : d.toLocaleDateString([], { month: "short", day: "numeric" });
}

/// "resets in 2h 5m", or "resetting now" once the time has passed.
function resetText(ms: number): string {
  return ms - Date.now() <= 0 ? "resetting now" : `resets in ${until(ms)}`;
}

function until(ms: number): string {
  const left = ms - Date.now();
  if (left <= 0) return "0m";
  const m = Math.floor(left / 60_000);
  if (m >= 1440) return `${Math.floor(m / 1440)}d ${Math.floor((m % 1440) / 60)}h`;
  if (m >= 60) return `${Math.floor(m / 60)}h ${m % 60}m`;
  return `${m}m`;
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
    return `<p class="dt-empty">History starts now. A reading is saved on this computer at each refresh, so this fills in over the next few hours.</p>`;
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
            return `<span class="dt-key"><i style="background:var(${SERIES_VARS[s.slot]})"></i>${esc(s.metric)} <b>${last.used.toFixed(0)}%</b></span>`;
          })
          .join("")}</div>`
      : "";
  return `${legend}
    <div class="dt-plot" data-t0="${t0}" data-t1="${t1}" data-pl="${pad.l}" data-pr="${pad.r}" data-w="${W}">
      <svg viewBox="0 0 ${W} ${H}" width="${W}" height="${H}" role="img" aria-label="Percent of each limit used over time">
        ${grid}${lines}
        <line class="dt-cross" x1="0" x2="0" y1="${pad.t}" y2="${H - pad.b}" visibility="hidden"/>
        <text class="dt-axis" x="${pad.l}" y="${H - 5}">${esc(when(t0, hours))}</text>
        <text class="dt-axis" x="${W - pad.r}" y="${H - 5}" text-anchor="end">now</text>
      </svg>
      <div class="dt-tip" hidden></div>
    </div>
    <p class="dt-caption">Percent of each limit used. A drop to zero is a reset.</p>`;
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
          ? `<div><i style="background:var(${SERIES_VARS[s.slot]})"></i>${esc(s.metric)} <b>${p.used.toFixed(0)}%</b></div>`
          : "";
      })
      .join("");
    tip.innerHTML = `<div class="dt-tip-when">${esc(new Date(nearest).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" }))}</div>${rows}`;
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
// Spend
// ---------------------------------------------------------------------------

function bars(rows: { label: string; tip: string; cost: number; tokens: number }[]): string {
  const max = Math.max(...rows.map((r) => r.cost), 0.0001);
  return `<div class="dt-bars">${rows
    .map(
      (r) => `
      <div class="dt-bar-row" title="${esc(`${r.tip}: ${money(r.cost)}, ${tokens(r.tokens)} tokens`)}">
        <span class="dt-bar-label">${esc(r.label)}</span>
        <span class="dt-bar-track"><span class="dt-bar" style="width:${Math.max((r.cost / max) * 100, r.cost > 0 ? 1.5 : 0)}%"></span></span>
        <span class="dt-bar-value">${money(r.cost)}</span>
      </div>`,
    )
    .join("")}</div>`;
}

function dayBars(daily: number[]): string {
  const max = Math.max(...daily, 0.0001);
  const today = new Date();
  const cols = daily
    .map((cost, i) => {
      const d = new Date(today);
      d.setDate(today.getDate() - (daily.length - 1 - i));
      const label = d.toLocaleDateString([], { month: "short", day: "numeric" });
      return `<span class="dt-day" title="${esc(`${label}: ${money(cost)}`)}"><span style="height:${Math.max((cost / max) * 100, cost > 0 ? 2 : 0)}%"></span></span>`;
    })
    .join("");
  const total = daily.reduce((a, b) => a + b, 0);
  return `<div class="dt-days">${cols}</div>
    <div class="dt-days-axis"><span>30 days ago</span><span>${money(total)} total · peak ${money(max)}</span><span>today</span></div>`;
}

function spendSection(sp: ProviderSpend | undefined): string {
  if (!sp || (sp.last30.cost < 0.005 && sp.last30.tokens <= 0)) {
    return `<p class="dt-empty">No local spend logs for this tool. Spend is read from the logs a command-line tool writes on this computer.</p>`;
  }
  const hasProjects = (sp.projects?.length ?? 0) > 0;
  if (groupKey === "project" && !hasProjects) groupKey = "model";
  const groups: [string, string][] = [["model", "Model"]];
  if (hasProjects) groups.push(["project", "Project"]);
  groups.push(["day", "Day"]);
  const controls = `<div class="dt-controls">
      <label>Group by ${select("dt-group", groups, groupKey)}</label>
      ${groupKey === "day" ? "" : `<label>Period ${select("dt-window", WINDOWS, windowKey)}</label>`}
    </div>`;

  let body: string;
  if (groupKey === "day") {
    body = dayBars(sp.daily_cost ?? []);
  } else if (groupKey === "project") {
    const rows = (sp.projects ?? [])
      .map((p) => ({ label: projectLabel(p.project), tip: p.project, cost: p[windowKey].cost, tokens: p[windowKey].tokens }))
      .filter((r) => r.cost > 0.004 || r.tokens > 0)
      .sort((a, b) => b.cost - a.cost);
    body = rows.length ? bars(rows) : `<p class="dt-empty">Nothing in this period.</p>`;
    if (rows.length === 1) {
      body += `<p class="dt-caption">Projects are the folders Claude Code was started in. Everything here ran from one folder, so there is one row.</p>`;
    }
  } else {
    const rows = [...sp[windowKey].models]
      .sort((a, b) => b.cost - a.cost)
      .slice(0, 12)
      .map((m) => ({ label: m.model, tip: m.model, cost: m.cost, tokens: m.tokens }));
    body = rows.length ? bars(rows) : `<p class="dt-empty">Nothing in this period.</p>`;
  }
  const w = sp[windowKey];
  const headline =
    groupKey === "day" ? "" : `<div class="dt-headline"><b>${money(w.cost)}</b><span>${tokens(w.tokens)} tokens</span></div>`;
  return `${controls}${headline}${body}
    <p class="dt-caption">From local logs, priced at API rates. On a flat-rate plan this is equivalent value, not a charge.</p>`;
}

// ---------------------------------------------------------------------------
// Card expander: the short version, inline under an expanded card
// ---------------------------------------------------------------------------

const SPARK_HOURS = 24;
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
    chart = `<p class="dt-mini-note">Loading the last 24 hours…</p>`;
  } else if (series.length === 0) {
    chart = `<p class="dt-mini-note">The 24-hour trend appears once a few readings are saved.</p>`;
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
        return `<span class="dt-key"><i style="background:var(${SERIES_VARS[slot]})"></i>${esc(s.metric)} <b>${last.used.toFixed(0)}%</b></span>`;
      })
      .join("");
    chart = `<svg class="dt-spark" viewBox="0 0 ${W} ${H}" preserveAspectRatio="none" role="img" aria-label="Percent used over the last 24 hours">${paths}</svg>
      <div class="dt-legend dt-mini-legend"><span class="dt-mini-span">24h, % used</span>${keys}</div>`;
  }
  const sp = source?.spend(id);
  const top = sp ? [...sp.today.models].sort((a, b) => b.cost - a.cost)[0] : undefined;
  const facts =
    sp && sp.today.cost > 0.004
      ? `<span>Today ${money(sp.today.cost)}${top ? ` · mostly ${esc(top.model)}` : ""}</span>`
      : `<span></span>`;
  return `${chart}<div class="dt-mini-foot">${facts}<button class="inv-learn dt-mini-open" data-detail="${esc(id)}">Details</button></div>`;
}

async function loadSpark(id: string): Promise<void> {
  if (sparkLoading.has(id)) return;
  sparkLoading.add(id);
  try {
    const series = await invoke<Series[]>("get_history", { providerId: id, hours: SPARK_HOURS });
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
        return `<div class="dt-now-row"><span class="dt-now-label">${esc(m.label)}</span>
          <span class="dt-now-facts">${[`${used.toFixed(0)}% used`, m.detail ?? "", reset].filter(Boolean).map(esc).join(" · ")}</span></div>`;
      }
      if (m.kind === "text" && m.value) {
        return `<div class="dt-now-row"><span class="dt-now-label">${esc(m.label)}</span><span class="dt-now-facts">${esc(m.value)}</span></div>`;
      }
      return "";
    })
    .join("");
  return rows || `<p class="dt-empty">No live readings.</p>`;
}

function render(): void {
  const el = document.querySelector<HTMLElement>("#detail-body");
  const title = document.querySelector<HTMLElement>("#detail-title");
  if (!el || !openId || !source) return;
  const snap = source.snapshot(openId);
  if (!snap) {
    close();
    return;
  }
  const plan = snap.plan ? snap.plan.charAt(0).toUpperCase() + snap.plan.slice(1) : "";
  if (title) title.textContent = plan ? `${snap.name} · ${plan}` : snap.name;
  const metrics: [string, string][] = [["__all__", "All limits"], ...history.map((s): [string, string] => [s.metric, s.metric])];
  if (!metrics.some(([v]) => v === metricFilter)) metricFilter = "__all__";
  const width = Math.floor(el.clientWidth) - 28;
  el.innerHTML = `
    <section class="dt-section">
      <h3>Limits over time</h3>
      <div class="dt-controls">
        <label>Range ${select("dt-range", RANGES.map(([h, l]): [string, string] => [String(h), l]), String(hours))}</label>
        ${history.length > 1 ? `<label>Limit ${select("dt-metric", metrics, metricFilter)}</label>` : ""}
      </div>
      ${loading && historyFor !== openId ? `<p class="dt-empty">Loading…</p>` : lineChart(history, width)}
    </section>
    <section class="dt-section">
      <h3>Spend</h3>
      ${spendSection(source.spend(openId))}
    </section>
    <section class="dt-section">
      <h3>Right now</h3>
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
    const got = await invoke<Series[]>("get_history", { providerId: id, hours });
    if (openId !== id) return;
    history = got;
    historyFor = id;
  } catch {
    history = [];
  }
  loading = false;
  render();
}

function markSelected(): void {
  document.querySelectorAll<HTMLElement>("#providers [data-provider]").forEach((card) => {
    card.classList.toggle("dt-selected", wide && card.dataset.provider === openId);
  });
}

function open(id: string): void {
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
    btn.textContent = wide ? "Narrow" : "Wide";
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
  if (!openId || Date.now() - lastLoad < 60_000) return;
  void loadHistory();
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
    if (t.id === "dt-metric") metricFilter = t.value;
    else if (t.id === "dt-window") windowKey = t.value as WindowKey;
    else if (t.id === "dt-group") groupKey = t.value as GroupKey;
    else return;
    render();
  });
  new ResizeObserver(() => openId && render()).observe(document.body);
}
