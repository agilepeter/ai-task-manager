// The Agents view: everything about agents gathered into one place -- what
// used to take six looks (Inventory > Running now, Inventory > Setup's
// custom and built-in agent lists, Inventory > Opportunities, the Audit's
// agent guardrail checks, and a line in a session's Detail view) is one
// slide-in panel, opened from the Inventory toolbar or its own summary door
// row. Built the same way the Audit view is built (src/audit.ts): same
// slide-in panel, same back button, same focus handling, same keyboard
// behaviour, same way of reaching a tab through the host.
//
// Nothing here starts, stops, ends or edits an agent -- there is no action
// control on an agent row, only the same read-only rows Inventory already
// draws. Every row renderer is reused from src/inventory.ts, never
// reimplemented here.

import { invoke } from "@tauri-apps/api/core";
import { plural, t } from "./i18n";
import {
  agentRows,
  agentsSummaryLine,
  builtInAgentRows,
  renderAgents,
  renderOpportunityRows,
  type AgentSpend,
  type Inventory,
  type RunningAgent,
} from "./inventory";

const T = (k: string, v?: Record<string, string | number>) => t(`agents.${k}`, v);

/// The finding ids this view's "Worth a look" section shows -- exactly the
/// ones about agents, decided from the FINDING_IDS registries in the Rust
/// core: "agents-none", "agents-model-unset" and "agent-unused" come from
/// crates/core/src/inventory.rs's FINDING_IDS (the custom-agent findings);
/// "subagent-share" comes from crates/core/src/coaching.rs's FINDING_IDS
/// (the one usage-coaching finding that is about agents). Every other id in
/// either registry is about MCP servers, permissions or usage patterns that
/// have nothing to do with agents, so it stays out of this list.
const AGENT_FINDING_IDS = new Set(["agents-none", "agents-model-unset", "agent-unused", "subagent-share"]);

/// The Audit's own agent guardrail checks (crates/core/src/audit.rs's
/// agent_checks()): "agent-tools", "deny-shell" and "agent-model". These are
/// never shown as rows here -- they stay scored inside the Audit -- but a
/// failing one (status "attention", the only status that counts against the
/// Audit's score) gets counted into the one-line summary below, with a
/// button that opens the Audit itself.
const AGENT_GUARDRAIL_CHECK_IDS = new Set(["agent-tools", "deny-shell", "agent-model"]);

interface AuditCheckLite {
  id: string;
  status: string;
}
interface AuditReportLite {
  sections: { checks: AuditCheckLite[] }[];
}

export interface AgentsHost {
  /** Opens the Audit view -- reached from the failing-guardrail-check line. */
  openAudit(): void;
}

let host: AgentsHost | null = null;
let inventory: Inventory | null = null;
let loadError = "";
let runningAgents: RunningAgent[] = [];
let runningAgentsError = "";
let agentSpend: AgentSpend[] = [];
let failingGuardrails = 0;
/** Whatever had focus right before openAgents() was called, restored on close. */
let lastFocused: HTMLElement | null = null;

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

/// The whole view's content, top to bottom, for a given snapshot of data --
/// pure (no DOM, no invoke), so it can be exercised in isolation the same
/// way src/inventory.ts's renderChanges() is: takes the data as parameters
/// rather than reading this module's own state directly.
export function renderAgentsView(
  inv: Inventory,
  running: RunningAgent[],
  runningError: string,
  spend: AgentSpend[],
  failing: number,
  nowMs: number,
): string {
  // 1. Summary line.
  const cost30 = spend.reduce((sum, s) => sum + s.cost, 0);
  const summary = agentsSummaryLine(inv.agents.length, running.length, cost30);

  // 2. Running now -- the exact rows Inventory used to show, folded above
  // the (now absent) MCP server rows. renderAgents() itself stays silent at
  // zero unless an error was set on ITS OWN module (src/inventory.ts's own
  // runningAgentsError, which this view never touches), so the empty and
  // error cases are handled here instead, from this view's own fetch.
  const runningBody = runningError
    ? `<p class="inv-empty">${esc(t("inventory.empty.runningAgentsError", { error: runningError }))}</p>`
    : running.length
      ? renderAgents(running)
      : `<p class="inv-empty">${esc(t("inventory.empty.runningAgents"))}</p>`;

  // 3. Your agents -- the same custom agent rows Setup used to show, in
  // every scope (this view has no scope filter of its own: it gathers
  // everything into one place, not a second copy of Inventory's filter UI).
  const spendByName = new Map(spend.map((s) => [s.name, s]));
  const yourAgentsBody = inv.agents.length
    ? agentRows(inv.agents, spendByName, nowMs)
    : `<p class="inv-empty">${esc(t("inventory.empty.agents"))}</p>`;

  // 4. Built-in agents -- same rows Setup used to show. builtInAgentRows()
  // stays silent when there is nothing to attribute, which reads fine when
  // SOME subagent spend exists but none of it happens to be a built-in's;
  // when there has been NO subagent spend at all in the window, that silence
  // would look like a gap instead of an answer, so this view names it.
  const totalRuns = spend.reduce((sum, s) => sum + s.runs, 0);
  const builtInsBody = builtInAgentRows(inv.agents, spend, nowMs);
  const builtInSection = builtInsBody
    ? `<section class="dt-section">${builtInsBody}</section>`
    : totalRuns === 0
      ? `<section class="dt-section"><h3>${esc(t("inventory.agents.builtIn"))}</h3><p class="inv-empty">${esc(T("empty.noSubagentRuns"))}</p></section>`
      : "";

  // 5. Worth a look -- the findings that are about agents, and only those
  // (see AGENT_FINDING_IDS above), painted with the exact renderer
  // Inventory's own Opportunities section uses for each one, so the text
  // never drifts from what that section shows for the same finding. A
  // failing agent guardrail check is a one-line count with a button to the
  // Audit, not a second Opportunity card -- the Audit is still where that
  // check is scored.
  const findings = inv.opportunities.filter((o) => AGENT_FINDING_IDS.has(o.id));
  const findingsBody = findings.length
    ? renderOpportunityRows(findings)
    : `<p class="inv-empty">${esc(T("empty.nothingToFlag"))}</p>`;
  const guardrailLine =
    failing > 0
      ? `<p class="dt-caption">${esc(plural("agents.guardrailFailing", failing))} <button class="inv-learn" id="agents-open-audit">${esc(T("openAudit"))}</button></p>`
      : "";

  return `
    <p class="dt-caption ag-summary">${esc(summary)}</p>
    <section class="dt-section">
      <h3>${esc(T("section.running"))}</h3>
      ${runningBody}
    </section>
    <section class="dt-section">
      <h3>${esc(T("section.yours"))}</h3>
      ${yourAgentsBody}
    </section>
    ${builtInSection}
    <section class="dt-section">
      <h3>${esc(T("section.worthALook"))}</h3>
      ${findingsBody}
      ${guardrailLine}
    </section>`;
}

function render(): void {
  const el = document.querySelector<HTMLElement>("#agents-body");
  if (!el) return;
  if (loadError) {
    el.innerHTML = `<p class="dt-empty">${esc(T("loadError", { error: loadError }))}</p>`;
    return;
  }
  if (!inventory) {
    el.innerHTML = `<p class="dt-empty">${esc(t("detail.loading"))}</p>`;
    return;
  }
  el.innerHTML = renderAgentsView(inventory, runningAgents, runningAgentsError, agentSpend, failingGuardrails, Date.now());
}

async function loadFailingGuardrails(): Promise<number> {
  try {
    const report = await invoke<AuditReportLite>("get_audit");
    let failing = 0;
    for (const section of report.sections) {
      for (const c of section.checks) {
        if (AGENT_GUARDRAIL_CHECK_IDS.has(c.id) && c.status === "attention") failing += 1;
      }
    }
    return failing;
  } catch {
    return 0;
  }
}

function close(): void {
  document.body.classList.remove("agents-open");
  lastFocused?.focus();
  lastFocused = null;
}

/// Opens the view and loads its own data fresh -- the same way openAudit()
/// (src/audit.ts) owns its own get_audit() call rather than reading
/// whatever Inventory happens to have cached, so this works whether or not
/// the Inventory tab was ever visited this session.
export function openAgents(): void {
  lastFocused = document.activeElement as HTMLElement | null;
  document.body.classList.add("agents-open");
  loadError = "";
  render();
  void invoke<Inventory>("get_inventory").then(
    (inv) => { inventory = inv; render(); },
    (err) => { loadError = String(err); render(); },
  );
  void invoke<RunningAgent[]>("get_running_agents").then(
    (rows) => { runningAgents = rows; runningAgentsError = ""; render(); },
    (err) => { runningAgents = []; runningAgentsError = String(err); render(); },
  );
  void invoke<AgentSpend[]>("get_agent_spend").then(
    (rows) => { agentSpend = rows; render(); },
    () => { agentSpend = []; render(); },
  );
  void loadFailingGuardrails().then((n) => { failingGuardrails = n; render(); });
  // The heading lives in the static panel head (index.html), not in
  // anything render() paints, so it is already there to receive focus.
  document.querySelector<HTMLElement>("#agents-heading")?.focus();
}

/// Redraws the Agents panel in place, e.g. after a locale switch. A no-op
/// while the panel is closed.
export function rerender(): void {
  if (document.body.classList.contains("agents-open")) render();
}

export function setupAgents(h: AgentsHost): void {
  host = h;
  document.querySelector("#agents-close")?.addEventListener("click", close);
  // Global click delegation for both places Inventory opens this view from
  // (its toolbar button and its own summary door row) -- the same mechanism
  // audit.ts uses for #audit-open-btn, so it works no matter which module
  // rendered the button.
  document.addEventListener("click", (e) => {
    if ((e.target as HTMLElement).closest("#agents-open-btn, #agents-door-btn")) openAgents();
  });
  document.querySelector("#agents-body")?.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const link = target.closest<HTMLElement>("[data-link]");
    if (link) {
      void invoke("open_link", { url: link.dataset.link }).catch(() => {});
      return;
    }
    if (target.closest("#agents-open-audit")) {
      close();
      host?.openAudit();
    }
  });
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key === "Escape" && document.body.classList.contains("agents-open")) {
        e.stopImmediatePropagation();
        e.preventDefault();
        close();
      }
    },
    true,
  );
}
