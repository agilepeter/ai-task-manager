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
type AuditSection = { checks: AuditCheck[] };

/** The app adds the usage findings to the setup ones; the audit carries
 *  them, Msg and all, so they translate exactly as they do on that panel. */
export function buildUsageRows(sections: readonly AuditSection[], existingIds: ReadonlySet<string>): SyntheticOpportunity[] {
  return sections
    .flatMap((sec) => sec.checks)
    .filter((c) => c.status === "consider" && !existingIds.has(c.id))
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
