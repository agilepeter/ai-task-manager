// Pure builders for the Inventory findings the demo authors by hand, rather
// than reading one the engine already computed onto the fixture. Kept free
// of window/document (mock.ts's own boot flag and command handlers) so a
// plain Node test can import this file, build these rows and check them in
// every locale without touching a browser.
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

/** Findings the engine builds with no Learn more link: the `None` last argument of
 *  `Opportunity::from_msgs` in crates/core/src/agent_watch.rs and crates/core/src/changes.rs. The
 *  other checks the audit carries that the Inventory tab does not are usage-coaching ones, which
 *  crates/core/src/coaching.rs gives the classroom link. Held to the Rust sources by
 *  scripts/demo-synthetic.test.mjs. */
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
 *  does (crates/core/src/i18n.rs): days and hours, hours and minutes, or minutes. Whole minutes, and
 *  anything under a minute reads as one. A Msg and not text, so that each language words it itself
 *  wherever the sentence it sits in is painted: the choosers in detail.ts, inventory.ts and main.ts
 *  return finished text, which a Msg cannot carry. */
export function durationMsg(minutes: number): Msg {
  const d = Math.floor(minutes / 1440);
  const h = Math.floor((minutes % 1440) / 60);
  const m = minutes % 60;
  if (d > 0) return { key: "time.daysHours", vars: { d: String(d), h: String(h) }, count: null };
  if (h > 0) return { key: "time.hoursMins", vars: { h: String(h), m: String(m) }, count: null };
  return { key: "time.mins", vars: { m: String(Math.max(m, 1)) }, count: null };
}

/** One limit's time at 100 percent, as `get_limit_time` answers it. */
export type LimitTimeRow = { provider: string; metric: string; times: number; totalMs: number; longestMs: number };

/** A limit's total at 100 percent that is worth a finding: `FINDING_AT_MS` in
 *  crates/core/src/limit_time.rs, which scripts/demo-limit-time.test.mjs reads to hold the two together. */
export const LIMIT_TIME_FINDING_MS = 2 * 3_600_000;

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

/** The checks crates/core/src/audit.rs lists first in the Usage section, in its order: the pricing
 *  check, then the findings it adds only when they are present, ahead of the checks that depend on
 *  spend. The time-at-the-limit check is the last of those present-only ones. */
const USAGE_HEAD = new Set(["pricing-cache-ttl", "pricing-drift", "agent-over-budget", "agent-runaway"]);

/** A copy of the audit report with the time-at-the-limit finding as a check in its Usage section,
 *  "consider" and so unscored, in the shape the audit's own checks have and where the engine puts
 *  it: right after the checks that lead that section. The report passed in is never touched. With
 *  no finding, or one the report already carries, the copy is the report as it was. */
export function auditWithLimitTime<R extends { sections: AuditSection[] }>(report: R, row: SyntheticOpportunity | null): R {
  const copy = structuredClone(report);
  const usage = copy.sections.find((s) => s.nameKey === "section.usage");
  if (!row || !usage || usage.checks.some((c) => c.id === row.id)) return copy;
  let at = 0;
  while (at < usage.checks.length && USAGE_HEAD.has(usage.checks[at].id)) at++;
  usage.checks.splice(at, 0, {
    id: row.id,
    status: "consider",
    title: row.title,
    detail: row.detail,
    titleMsg: row.titleMsg,
    detailMsg: row.detailMsg,
  });
  return copy;
}
