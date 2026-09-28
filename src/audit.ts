// The audit panel: a scored, sectioned read of this machine's AI setup. It
// opens by itself on the first run and from the Inventory tab after that.
// Everything shown is computed in Rust (crates/core/src/audit.rs); this file
// only draws it. Statuses are always a word as well as a colour.

import { invoke } from "@tauri-apps/api/core";
import { plural, t, tm, type Msg } from "./i18n";

const T = (k: string, v?: Record<string, string | number>) => t(`audit.${k}`, v);

interface Check {
  id: string;
  status: "pass" | "attention" | "consider" | "info";
  title: string;
  detail: string;
  /** See inventory.ts's Opportunity: same optional/nullable Msg pair. */
  titleMsg?: Msg | null;
  detailMsg?: Msg | null;
}

interface AuditReport {
  generatedAt: number;
  passed: number;
  attention: number;
  score: number | null;
  sections: { name: string; nameKey?: string; checks: Check[] }[];
}

export interface AuditHost {
  seen(): boolean;
  markSeen(): void;
  /** Where a check sends the user: a tab of the main view, or the Agents
   *  view -- a slide-in panel, not a tab, but reached through this same
   *  goTo() so a check never needs to know the difference. */
  goTo(view: "inventory" | "ledger" | "usage" | "agents"): void;
}

let host: AuditHost | null = null;
let report: AuditReport | null = null;
let note = "";
let firstRun = false;
/** The button that opened this view (see openAudit()), restored on close. */
let opener: HTMLElement | null = null;

/// Same rule as src/agents.ts's own RELOAD_FRESHNESS_MS/shouldReload() --
/// duplicated rather than imported, the same convention this file already
/// follows for esc() and closeFocusTarget() (each view keeps its own tiny
/// copy rather than share one across independent views). See that file's
/// comment for why the window is 60s and why a backwards clock reloads.
export const RELOAD_FRESHNESS_MS = 60_000;
export function shouldReload(lastLoadedAt: number, now: number, open: boolean): boolean {
  if (!open) return false;
  if (now < lastLoadedAt) return true;
  return now - lastLoadedAt >= RELOAD_FRESHNESS_MS;
}
/** Stamped every time get_audit resolves, by whichever path asked for it. */
let lastLoadedAt = 0;

function isOpen(): boolean {
  return document.body.classList.contains("audit-open");
}

/** Which tab (or the Agents view) fixes a check, when one does. The three
 *  agent guardrail checks (agent-tools, agent-model, deny-shell) and
 *  agents-none used to point at Inventory, back when Inventory itself
 *  showed agent rows; now that those rows live only in the Agents view,
 *  these four point there instead. */
const WHERE: Record<string, "inventory" | "ledger" | "usage" | "agents"> = {
  "mcp-unpinned": "inventory", "mcp-env-secrets": "inventory", "mcp-remote": "inventory",
  "perm-none": "inventory", "perm-deny": "inventory", "perm-deny-only": "inventory", "hooks-none": "inventory",
  "agents-none": "agents", "agent-tools": "agents", "deny-shell": "agents", "agent-model": "agents",
  ledger: "ledger", "ledger-idle": "ledger", "ledger-dates": "ledger",
  clients: "usage", "areas-unsorted": "usage", "mix-top-heavy": "usage", "session-long-lived": "usage",
  "cache-read-share": "usage", "subagent-share": "usage",
  "mcp-context-heavy": "inventory", "guardrail-removed": "inventory", "setup-changed": "inventory",
};

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

/// Rust's Section carries a name_key ("section.setup" etc, fully-qualified --
/// not under the "audit." prefix, because it is shared with any other future
/// reader of the same sections) alongside the English name. An older,
/// un-regenerated demo fixture has no name_key, so this falls back to
/// guessing the same key from the fixed English vocabulary Rust has always
/// sent ("Setup" / "Guardrails" / "Usage" / "Money", crates/core/src/audit.rs)
/// -- both paths resolve through the same top-level keys, and an unrecognised
/// name (a future section neither path maps) falls back to itself.
function sectionLabel(name: string, nameKey?: string): string {
  const key =
    nameKey ??
    (name === "Setup"
      ? "section.setup"
      : name === "Guardrails"
        ? "section.guardrails"
        : name === "Usage"
          ? "section.usage"
          : name === "Money"
            ? "section.money"
            : null);
  return key ? t(key) : name;
}

/// The "Open …" link under a check that names which tab (or the Agents view)
/// fixes it.
function gotoLabel(where: "inventory" | "ledger" | "usage" | "agents"): string {
  if (where === "ledger") return T("goto.ledger");
  if (where === "usage") return T("goto.usage");
  if (where === "agents") return T("goto.agents");
  return T("goto.inventory");
}

/// Guards against a get_audit() call that was in flight finishing after the
/// panel has since closed -- without this, a slow response landing late
/// would still overwrite #audit-body's content (and reset its scroll)
/// underneath a view the user is no longer looking at (same fix as
/// src/agents.ts's own render()).
function render(): void {
  if (!isOpen()) return;
  const el = document.querySelector<HTMLElement>("#audit-body");
  if (!el) return;
  if (!report) {
    el.innerHTML = `<p class="dt-empty">${esc(T("loading"))}</p>`;
    return;
  }
  const r = report;
  const scored = r.passed + r.attention;
  // Rebuilt on every render (not a module-level const) so a locale switch is
  // reflected immediately, same reasoning as inventory.ts's tierLabel().
  const WORD: Record<Check["status"], string> = {
    pass: T("status.pass"), attention: T("status.attention"), consider: T("status.consider"), info: T("status.info"),
  };
  const head = `
    <section class="dt-section au-head">
      ${firstRun ? `<p class="au-welcome">${esc(T("welcome"))}</p>` : ""}
      <div class="au-score">
        <b>${r.score === null ? "–" : r.score}</b>
        <span>${r.score === null ? esc(T("score.none")) : esc(T("score.line", { passed: r.passed, scored }))}</span>
      </div>
      <div class="au-meter" role="img" aria-label="${esc(T("score.meterAria", { passed: r.passed, scored }))}"><span style="width:${scored ? (r.passed / scored) * 100 : 0}%"></span></div>
      <p class="dt-caption">${esc(T("score.explain", { consider: WORD.consider }))} ${r.attention ? esc(plural("audit.score.attention", r.attention, { status: WORD.attention })) : esc(T("score.nothingNeeded"))}</p>
      <div class="dt-rule-actions"><span class="spacer"></span><button class="inv-learn" id="au-export" title="${esc(T("export.tip"))}">${esc(T("export.button"))}</button></div>
      ${note ? `<p class="dt-caption">${esc(note)}</p>` : ""}
    </section>`;
  const sections = r.sections
    .map((s) => {
      // What needs attention first, then what passes, then notes.
      const order = { attention: 0, consider: 1, pass: 2, info: 3 } as const;
      const checks = [...s.checks].sort((a, b) => order[a.status] - order[b.status]);
      return `<section class="dt-section"><h3>${esc(sectionLabel(s.name, s.nameKey))}</h3>${checks
        .map((c) => {
          const title = c.titleMsg ? tm(c.titleMsg) : c.title;
          const detail = c.detailMsg ? tm(c.detailMsg) : c.detail;
          return `
        <div class="au-check au-${c.status}">
          <div class="au-check-head"><span class="au-status">${esc(WORD[c.status])}</span><span class="au-title">${esc(title)}</span></div>
          ${detail ? `<p class="au-detail">${esc(detail)}</p>` : ""}
          ${(c.status === "attention" || c.status === "consider") && WHERE[c.id] ? `<button class="lg-link" data-goto="${WHERE[c.id]}">${esc(gotoLabel(WHERE[c.id]))}</button>` : ""}
        </div>`;
        })
        .join("")}</section>`;
    })
    .join("");
  el.innerHTML = head + sections;
}

/// Same decision agents.ts's closeFocusTarget() makes, kept as this view's
/// own copy rather than shared (see that file's comment on why it takes
/// `openerStillInDocument` as a parameter instead of reading the DOM itself:
/// it is what makes this testable without a real one). Every view here keeps
/// its own tiny esc(); this is the same convention for focus-restore.
export function closeFocusTarget(
  opener: HTMLElement | null,
  openerStillInDocument: boolean,
  fallback: HTMLElement | null,
): HTMLElement | null {
  if (opener && openerStillInDocument) return opener;
  return fallback;
}

function close(): void {
  document.body.classList.remove("audit-open");
  const fallback = document.querySelector<HTMLElement>("#audit-open-btn");
  const stillThere = opener != null && document.contains(opener);
  closeFocusTarget(opener, stillThere, fallback)?.focus();
  opener = null;
  if (firstRun) {
    firstRun = false;
    host?.markSeen();
  }
}

/// `opener` is the button close() returns focus to, when it is still in the
/// document -- same reasoning as agents.ts's openAgents(): WebKit does not
/// focus a button on a mouse click, so the click handler has to hand this in
/// explicitly rather than close() reading `document.activeElement` back.
/// Left out for the two callers with no real button to return to: the
/// first-run auto-open (maybeFirstRunAudit(), no click happened at all) and
/// the Agents view's own "Open Audit" line (src/agents.ts's
/// #agents-open-audit, which closes the Agents panel first -- its own button
/// is still technically attached but off-screen, not where focus belongs) --
/// close() then falls back to #audit-open-btn on its own.
export function openAudit(opener_: HTMLElement | null = null): void {
  opener = opener_;
  document.body.classList.add("audit-open");
  note = "";
  render();
  lastLoadedAt = Date.now();
  void invoke<AuditReport>("get_audit").then(
    (r) => { report = r; render(); },
    (err) => { note = String(err); report = report ?? { generatedAt: 0, passed: 0, attention: 0, score: null, sections: [] }; render(); },
  );
}

/// Opens the audit once, on the first run. Call after the config has loaded.
export function maybeFirstRunAudit(): void {
  if (host && !host.seen()) {
    firstRun = true;
    openAudit();
  }
}

/// Redraws the audit panel in place, e.g. after a locale switch (task 7 wires
/// this into the locale-change handler). A no-op while the panel is closed.
export function rerender(): void {
  if (document.body.classList.contains("audit-open")) render();
}

/// Refreshes the audit's own data in place, without opening the panel,
/// capturing a new opener, or moving focus -- called from two different
/// events in main.ts, which is what `force` distinguishes:
///
/// - `force: false` (the default), on "popover-shown" -- reopening the
///   popover happens far more often than the setup actually changes, so this
///   is gated by shouldReload() the same way src/agents.ts's reloadAgents()
///   gates its own popover-shown refresh: skip while the last get_audit is
///   still within RELOAD_FRESHNESS_MS.
/// - `force: true`, after a rescan -- the setup genuinely just changed, so
///   this always re-fetches. There is nothing of a rescan's own data to
///   reuse here the way src/agents.ts's applyRescan() reuses three of
///   src/inventory.ts's four rescan calls: the Audit's report is computed
///   entirely by its own `get_audit` command from the just-rescanned setup,
///   which a rescan never calls on its own, so get_audit is the one call a
///   rescan-driven reload has no way to avoid.
///
/// Either way, a no-op while the panel is closed. Deliberately does not
/// clear `report` before the call resolves, so the previous render stays up
/// until fresh data actually lands -- #audit-body's content only changes
/// once, in place, the same reasoning as reloadAgents(). #audit itself (not
/// #audit-body) is the scrollable element, so replacing a child's innerHTML
/// does not reset scrollTop: the panel's scroll position survives a reload
/// on its own.
export function reloadAudit(force = false): void {
  const open = isOpen();
  if (!force && !shouldReload(lastLoadedAt, Date.now(), open)) return;
  if (!open) return;
  lastLoadedAt = Date.now();
  void invoke<AuditReport>("get_audit").then(
    (r) => { report = r; note = ""; render(); },
    (err) => { note = String(err); render(); },
  );
}

export function setupAudit(h: AuditHost): void {
  host = h;
  document.querySelector("#audit-close")?.addEventListener("click", close);
  document.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>("#audit-open-btn");
    if (btn) openAudit(btn);
  });
  document.querySelector("#audit-body")?.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const go = target.closest<HTMLElement>("[data-goto]")?.dataset.goto as "inventory" | "ledger" | "usage" | "agents" | undefined;
    if (go) {
      close();
      host?.goTo(go);
      return;
    }
    if (target.closest("#au-export")) {
      void invoke<string>("export_audit").then(
        (path) => { note = t("detail.csv.saved", { path }); render(); },
        (err) => { note = String(err); render(); },
      );
    }
  });
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key === "Escape" && document.body.classList.contains("audit-open")) {
        e.stopImmediatePropagation();
        e.preventDefault();
        close();
      }
    },
    true,
  );
}
