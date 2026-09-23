// The audit panel: a scored, sectioned read of this machine's AI setup. It
// opens by itself on the first run and from the Inventory tab after that.
// Everything shown is computed in Rust (crates/core/src/audit.rs); this file
// only draws it. Statuses are always a word as well as a colour.

import { invoke } from "@tauri-apps/api/core";
import { plural, t } from "./i18n";

const T = (k: string, v?: Record<string, string | number>) => t(`audit.${k}`, v);

interface Check {
  id: string;
  status: "pass" | "attention" | "consider" | "info";
  title: string;
  detail: string;
}

interface AuditReport {
  generatedAt: number;
  passed: number;
  attention: number;
  score: number | null;
  sections: { name: string; checks: Check[] }[];
}

export interface AuditHost {
  seen(): boolean;
  markSeen(): void;
  /** Where a check sends the user: a tab of the main view. */
  goTo(view: "inventory" | "ledger" | "usage"): void;
}

let host: AuditHost | null = null;
let report: AuditReport | null = null;
let note = "";
let firstRun = false;

/** Which tab fixes a check, when one does. */
const WHERE: Record<string, "inventory" | "ledger" | "usage"> = {
  "mcp-unpinned": "inventory", "mcp-env-secrets": "inventory", "mcp-remote": "inventory",
  "perm-none": "inventory", "perm-deny": "inventory", "hooks-none": "inventory", "agents-none": "inventory",
  ledger: "ledger", "ledger-idle": "ledger", "ledger-dates": "ledger",
  clients: "usage", "areas-unsorted": "usage", "mix-top-heavy": "usage", "session-long-lived": "usage",
};

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

/// Rust's Section.name is a fixed, small vocabulary ("Setup" / "Guardrails" /
/// "Usage" / "Money", crates/core/src/audit.rs). Mapped by the English name it
/// sends today; task 9 gives Rust a name_key so this stops guessing from text.
/// An unrecognised name (a future section not yet mapped) falls back to itself
/// rather than showing nothing.
function sectionLabel(name: string): string {
  if (name === "Setup") return T("section.setup");
  if (name === "Guardrails") return T("section.guardrails");
  if (name === "Usage") return T("section.usage");
  if (name === "Money") return T("section.money");
  return name;
}

/// The "Open …" link under a check that names which tab fixes it.
function gotoLabel(where: "inventory" | "ledger" | "usage"): string {
  if (where === "ledger") return T("goto.ledger");
  if (where === "usage") return T("goto.usage");
  return T("goto.inventory");
}

function render(): void {
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
      return `<section class="dt-section"><h3>${esc(sectionLabel(s.name))}</h3>${checks
        .map(
          (c) => `
        <div class="au-check au-${c.status}">
          <div class="au-check-head"><span class="au-status">${esc(WORD[c.status])}</span><span class="au-title">${esc(c.title)}</span></div>
          ${c.detail ? `<p class="au-detail">${esc(c.detail)}</p>` : ""}
          ${(c.status === "attention" || c.status === "consider") && WHERE[c.id] ? `<button class="lg-link" data-goto="${WHERE[c.id]}">${esc(gotoLabel(WHERE[c.id]))}</button>` : ""}
        </div>`,
        )
        .join("")}</section>`;
    })
    .join("");
  el.innerHTML = head + sections;
}

function close(): void {
  document.body.classList.remove("audit-open");
  if (firstRun) {
    firstRun = false;
    host?.markSeen();
  }
}

export function openAudit(): void {
  document.body.classList.add("audit-open");
  note = "";
  render();
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

export function setupAudit(h: AuditHost): void {
  host = h;
  document.querySelector("#audit-close")?.addEventListener("click", close);
  document.addEventListener("click", (e) => {
    if ((e.target as HTMLElement).closest("#audit-open-btn")) openAudit();
  });
  document.querySelector("#audit-body")?.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const go = target.closest<HTMLElement>("[data-goto]")?.dataset.goto as "inventory" | "ledger" | "usage" | undefined;
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
