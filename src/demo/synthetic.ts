// Pure builders and ports for what the demo works out by hand rather than
// reading the engine's own answer off the fixture: the findings it authors
// itself, and the engine's rules that it applies to its own readings and
// budgets. Kept free of window/document (mock.ts's own boot flag and command
// handlers) so a plain Node test can import this file, build these rows and
// check them in every locale without touching a browser.
import { render, type Msg } from "../i18n";

/** A finding built here rather than read off the fixture. titleMsg/detailMsg
 *  are required, never optional, because the demo has to translate every
 *  finding the same way the real app does, and these hand-authored rows are
 *  the only findings that do not already carry a Msg from the engine -- a
 *  future row built without one now fails to typecheck instead of silently
 *  staying English. */
export type SyntheticOpportunity = {
  id: string;
  kind: "tighten" | "learn";
  title: string;
  detail: string;
  titleMsg: Msg;
  detailMsg: Msg | null;
  learnUrl: string | null;
};

type AuditCheck = { id: string; status: string; title: string; detail: string; titleMsg: Msg; detailMsg?: Msg | null };
type AuditSection = { nameKey?: string; checks: AuditCheck[] };

/** Audit-check ids whose "consider" fact is already told, under a
 *  *different* id, by an Inventory-tab opportunity -- so comparing ids
 *  directly misses the overlap and the same fact shows up twice. A check
 *  that shares its id with the opportunity it mirrors (mcp-remote, for
 *  one) needs no entry: plain id equality already catches it. This map is
 *  only for the pairs that drifted apart on purpose, such as agent-model:
 *  the audit row stays a short id/title pointing at the Inventory tab,
 *  while the agents-model-unset opportunity carries the longer
 *  explanation and the learn link. Exported so the test can drive real
 *  pairs instead of a hand-copied id string. */
export const AUDIT_ID_ALIASES: Readonly<Record<string, string>> = {
  "agent-model": "agents-model-unset",
};

/** Findings the engine builds with no Learn more link, among the checks `buildUsageRows` lifts
 *  into the Inventory tab: the `None` last argument of `Opportunity::from_msgs` in
 *  crates/core/src/agent_watch.rs and crates/core/src/changes.rs, the two modules whose findings
 *  reach the demo through the audit alone. The other checks that are lifted are usage-coaching
 *  ones, which crates/core/src/coaching.rs gives the classroom link. `agent-unused` also has no
 *  link but never passes through here (it comes in the fixture's own opportunities), and neither
 *  does `limit-time` (built by `buildLimitTimeRow`). Held to the Rust sources, every such id of
 *  the two modules, by scripts/demo-synthetic.test.mjs. */
export const NO_LEARN_LINK: ReadonlySet<string> = new Set(["agent-over-budget", "agent-runaway", "guardrail-removed", "setup-changed"]);

/** The app adds the usage findings to the setup ones; the audit carries
 *  them, Msg and all, so they translate exactly as they do on that panel. */
export function buildUsageRows(sections: readonly AuditSection[], existingIds: ReadonlySet<string>): SyntheticOpportunity[] {
  return sections
    .flatMap((sec) => sec.checks)
    .filter((c) => c.status === "consider" && !existingIds.has(c.id) && !existingIds.has(AUDIT_ID_ALIASES[c.id]))
    .map(
      (c): SyntheticOpportunity => ({
        id: c.id,
        kind: "learn",
        title: c.title,
        detail: c.detail,
        titleMsg: c.titleMsg,
        detailMsg: c.detailMsg ?? null,
        learnUrl: NO_LEARN_LINK.has(c.id) ? null : "https://staas.fund/classroom/",
      }),
    );
}

type RunningServer = { name: string; instances: number; rssBytes: number };

/** The real app computes this one from the live process list; the demo has
 *  a fixed process list, so derive it the same way rather than hard-coding
 *  text -- same keys and vars as procs.rs's own opportunities(), including
 *  the nested unit.times Msg for "running N times", so it translates like
 *  every other finding instead of being the one row stuck in English.
 *  English is rendered directly from the same Msgs (render("en", …)) rather
 *  than existing a second time as its own literal. */
export function buildDuplicateProcessesRow(running: readonly RunningServer[]): SyntheticOpportunity | null {
  const dupes = running
    .filter((r) => r.instances > 1)
    .slice()
    .sort((a, b) => b.rssBytes - a.rssBytes); // "worst" = heaviest RSS, not first configured
  if (!dupes.length) return null;
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
  return {
    id: "mcp-duplicate-processes",
    kind: "tighten",
    title: render("en", titleMsg),
    detail: render("en", detailMsg),
    titleMsg,
    detailMsg,
    learnUrl: "https://staas.fund/mcp/",
  };
}

/** A length of time as the locale's own short duration, chosen by size the way `i18n::duration_msg`
 *  does (crates/core/src/i18n.rs): days and hours, hours and minutes, or minutes. Whole minutes (the
 *  rest is dropped), and anything under a minute reads as one. A Msg and not text, so that each
 *  language words it itself wherever the sentence it sits in is painted: the choosers in detail.ts,
 *  inventory.ts and main.ts return finished text, which a Msg cannot carry. */
export function durationMsg(minutes: number): Msg {
  const whole = Math.floor(minutes);
  const d = Math.floor(whole / 1440);
  const h = Math.floor((whole % 1440) / 60);
  const m = whole % 60;
  if (d > 0) return { key: "time.daysHours", vars: { d: String(d), h: String(h) }, count: null };
  if (h > 0) return { key: "time.hoursMins", vars: { h: String(h), m: String(m) }, count: null };
  return { key: "time.mins", vars: { m: String(Math.max(m, 1)) }, count: null };
}

// ---------------------------------------------------------------------------
// Time at the limit: the rule, and the finding
// ---------------------------------------------------------------------------

/** One reading of a limit, as the rule reads it: when, how full, and the reset time it carried. */
export type LimitReading = { at: number; used: number; resetsAt: number | null };

/** One limit's time at 100 percent, as `get_limit_time` answers it. */
export type LimitTimeRow = { provider: string; metric: string; times: number; totalMs: number; longestMs: number };

// The rule's constants, one for one with crates/core/src/limit_time.rs; scripts/demo-limit-time.test.mjs
// reads each out of that file and fails when a pair differs.

/** A reading at or above this is a limit that has been reached: `AT_LIMIT`. */
export const LIMIT_AT_PERCENT = 100;
/** Two readings further apart than this say nothing about the time between them: `MAX_GAP_MS`. */
export const LIMIT_MAX_GAP_MS = 90 * 60_000;
/** The span the Detail view's line and the finding look back over: `WINDOW_MS`. */
export const LIMIT_WINDOW_MS = 30 * 24 * 3_600_000;
/** A limit's total at 100 percent that is worth a finding: `FINDING_AT_MS`. */
export const LIMIT_TIME_FINDING_MS = 2 * 3_600_000;

/** The stretches one limit spent at 100 percent, from its readings, oldest first: the rule of
 *  `from_points` in crates/core/src/limit_time.rs, which scripts/demo-limit-time.test.mjs holds
 *  this to case for case. Walking consecutive pairs A then B, a pair counts only when A is at the
 *  limit and the two are within `LIMIT_MAX_GAP_MS`: up to A's own reset when that falls strictly
 *  between them (the limit held until then, and the stretch ends), else the whole gap when B is at
 *  the limit too, else nothing (B is lower with no reset between, so nobody knows when it dropped).
 *  A longer gap, or readings out of order, count nothing and end the stretch. `null` when the
 *  limit was never reached. */
export function timeAtLimit(provider: string, metric: string, points: readonly LimitReading[]): LimitTimeRow | null {
  let times = 0, total = 0, longest = 0, run = 0, open = false;
  for (let i = 0; i < points.length; i++) {
    const { at, used, resetsAt } = points[i];
    if (!(used >= LIMIT_AT_PERCENT)) {
      longest = Math.max(longest, run);
      open = false;
      continue;
    }
    if (!open) {
      times++;
      open = true;
      run = 0;
    }
    const next = points[i + 1];
    if (!next) break;
    const gap = next.at - at;
    let continues = false;
    if (gap >= 0 && gap <= LIMIT_MAX_GAP_MS) {
      let counted: number | null = null;
      if (resetsAt !== null && resetsAt > at && resetsAt < next.at) counted = resetsAt - at;
      else if (next.used >= LIMIT_AT_PERCENT) {
        continues = true;
        counted = gap;
      }
      if (counted !== null) {
        total += counted;
        run += counted;
      }
    }
    if (!continues) {
      longest = Math.max(longest, run);
      open = false;
    }
  }
  longest = Math.max(longest, run);
  return times > 0 ? { provider, metric, times, totalMs: total, longestMs: longest } : null;
}

/** The one order limits are listed and chosen in, `limit_time::order`: most time at the limit, then
 *  the longest single stretch, then provider, then metric. It is total, so a tie never depends on
 *  the order the rows arrive in. */
export function limitTimeOrder(a: LimitTimeRow, b: LimitTimeRow): number {
  const by = (x: string, y: string) => (x < y ? -1 : x > y ? 1 : 0);
  return b.totalMs - a.totalMs || b.longestMs - a.longestMs || by(a.provider, b.provider) || by(a.metric, b.metric);
}

/** The time-at-the-limit finding, as `limit_time::opportunities` and its `messages` build it, for a
 *  machine whose limit history the engine cannot read (the fictional one keeps none): absent unless
 *  some limit's total is at or over `LIMIT_TIME_FINDING_MS`. The title is a count of such limits and
 *  nothing else; the detail carries the figures of the one that spent longest, every duration and
 *  count a nested Msg so each language words it itself, and there is no Learn more link. English is
 *  rendered from the same Msgs rather than written a second time. */
export function buildLimitTimeRow(rows: readonly LimitTimeRow[]): SyntheticOpportunity | null {
  const over = rows.filter((r) => r.totalMs >= LIMIT_TIME_FINDING_MS);
  if (!over.length) return null;
  const top = [...over].sort(limitTimeOrder)[0];
  const minutes = (ms: number) => Math.floor(Math.max(ms, 0) / 60_000);
  const titleMsg: Msg = { key: "finding.limit-time.title", vars: {}, count: over.length };
  const detailMsg: Msg = {
    key: "finding.limit-time.detail",
    vars: {
      total: durationMsg(minutes(top.totalMs)),
      times: { key: "unit.times", vars: {}, count: top.times },
      longest: durationMsg(minutes(top.longestMs)),
    },
    count: null,
  };
  return {
    id: "limit-time",
    kind: "learn",
    title: render("en", titleMsg),
    detail: render("en", detailMsg),
    titleMsg,
    detailMsg,
    learnUrl: null,
  };
}

// ---------------------------------------------------------------------------
// Budgets: the finding
// ---------------------------------------------------------------------------

/** One saved budget against its agent's spend for the calendar month: `BudgetRow` in
 *  crates/core/src/agent_watch.rs, as `get_agent_watch` lists it. */
export type BudgetRow = { agent: string; monthToDate: number; monthlyBudget: number };

/** The over-budget finding, as `agent_watch::opportunities` builds its budget half from
 *  `currently_over`: absent unless some budget is at or over its figure (at it counts, like a
 *  client budget). The title is a count of those budgets and nothing else; the detail names them in
 *  the order they were saved, each as "name ($spent / $budget)" to the cent, and there is no Learn
 *  more link. Built from the budgets as they are now, so one the visitor removes or adds changes it.
 *  The cents differ from Rust's `{:.2}` on one kind of figure: Rust rounds a figure exactly halfway
 *  between two cents to the even cent, so x.125 and x.625 print `.12` and `.62` where `toFixed(2)`
 *  gives `.13` and `.63`. None is reached here: the Agents view sets a budget in whole dollars, and
 *  the fixture's spends carry four decimals, none of them such a figure. */
export function buildOverBudgetRow(rows: readonly BudgetRow[]): SyntheticOpportunity | null {
  const over = rows.filter((r) => r.monthToDate >= r.monthlyBudget);
  if (!over.length) return null;
  const usd = (x: number) => `$${x.toFixed(2)}`;
  const names = over.map((r) => `${r.agent} (${usd(r.monthToDate)} / ${usd(r.monthlyBudget)})`).join(", ");
  const titleMsg: Msg = { key: "finding.agent-over-budget.title", vars: {}, count: over.length };
  const detailMsg: Msg = { key: "finding.agent-over-budget.detail", vars: { names }, count: null };
  return {
    id: "agent-over-budget",
    kind: "learn",
    title: render("en", titleMsg),
    detail: render("en", detailMsg),
    titleMsg,
    detailMsg,
    learnUrl: null,
  };
}

// ---------------------------------------------------------------------------
// The chart, and the audit
// ---------------------------------------------------------------------------

/** The most points the app hands the chart for one limit: `MAX_POINTS` in crates/core/src/history.rs. */
export const HISTORY_MAX_POINTS = 1_500;

/** A long range cut to the chart's cap the way `history::thin` does it: every `step`-th reading
 *  with `step = ceil(len / cap)`, and always the newest. A range at or under the cap is returned
 *  whole. A new array; the one passed in is not touched. */
export function thin<T>(points: readonly T[]): T[] {
  if (points.length <= HISTORY_MAX_POINTS) return [...points];
  const step = Math.ceil(points.length / HISTORY_MAX_POINTS);
  const last = points.length - 1;
  return points.filter((_, i) => i % step === 0 || i === last);
}

/** The findings the demo works out from its own state each time it is asked, as the app does on
 *  every load, instead of lifting the engine's from the fixture: the engine's describe the machine
 *  as it was generated, so the over-budget one would outlive a budget the visitor removes, and the
 *  fictional machine has no limit history for the engine to read for the other. */
export const STATE_FINDING_IDS: readonly string[] = ["agent-over-budget", "limit-time"];

/** The checks crates/core/src/audit.rs lists first in the Usage section, in its order: the pricing
 *  check, then the findings it adds only when they are present, ahead of the checks that depend on
 *  spend. */
const USAGE_HEAD = ["pricing-cache-ttl", "pricing-drift", "agent-over-budget", "agent-runaway", "limit-time"];

/** A copy of the audit report whose Usage section carries the findings the demo built from its own
 *  state (`STATE_FINDING_IDS`) in place of the fixture's, "consider" and so unscored, in the shape the
 *  audit's own checks have and where the engine puts them: among the checks that lead that section,
 *  in its order. A finding that is null is left out, so a budget the visitor removed leaves no check
 *  behind. The report passed in is never touched. */
export function auditWithFindings<R extends { sections: AuditSection[] }>(report: R, rows: readonly (SyntheticOpportunity | null)[]): R {
  const copy = structuredClone(report);
  const usage = copy.sections.find((s) => s.nameKey === "section.usage");
  if (!usage) return copy;
  const kept = usage.checks.filter((c) => !STATE_FINDING_IDS.includes(c.id));
  const added: AuditCheck[] = rows
    .filter((r): r is SyntheticOpportunity => r !== null)
    .map((r) => ({ id: r.id, status: "consider", title: r.title, detail: r.detail, titleMsg: r.titleMsg, detailMsg: r.detailMsg }));
  const rank = (id: string) => USAGE_HEAD.indexOf(id);
  let lead = 0;
  while (lead < kept.length && rank(kept[lead].id) >= 0) lead++;
  usage.checks = [...[...kept.slice(0, lead), ...added].sort((a, b) => rank(a.id) - rank(b.id)), ...kept.slice(lead)];
  return copy;
}
