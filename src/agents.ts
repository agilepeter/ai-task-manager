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
import { money, wholeMoney } from "./format";
import { focusOrFallback } from "./focus";
import { plural, t } from "./i18n";
import { focusAfterClose, isTopPanel, syncPanels } from "./panels";
import {
  agentRows,
  builtInAgentCount,
  builtInAgentRows,
  renderAgents,
  renderOpportunityRows,
  type AgentSpend,
  type Inventory,
  type RescanResult,
  type RunningAgent,
} from "./inventory";

const T = (k: string, v?: Record<string, string | number>) => t(`agents.${k}`, v);

/// The finding ids this view's "Worth a look" section shows -- exactly the
/// ones about agents, decided from the FINDING_IDS registries in the Rust
/// core: "agents-none", "agents-model-unset" and "agent-unused" come from
/// crates/core/src/inventory.rs's FINDING_IDS (the custom-agent findings);
/// "subagent-share" comes from crates/core/src/coaching.rs's FINDING_IDS
/// (the one usage-coaching finding that is about agents); "agent-over-budget"
/// and "agent-runaway" come from crates/core/src/agent_watch.rs. Every other id in
/// either registry is about MCP servers, permissions or usage patterns that
/// have nothing to do with agents, so it stays out of this list. Exported so
/// scripts/agent-id-registries.test.mjs can check this set against the Rust
/// source directly, rather than trusting this comment to stay accurate.
export const AGENT_FINDING_IDS = new Set([
  "agents-none",
  "agents-model-unset",
  "agent-unused",
  "subagent-share",
  "agent-over-budget",
  "agent-runaway",
]);

/// The Audit's own agent guardrail checks (crates/core/src/audit.rs's
/// agent_checks()) that can actually fail: "agent-tools" and "deny-shell".
/// These are never shown as rows here -- they stay scored inside the Audit
/// -- but a failing one (status "attention", the only status that counts
/// against the Audit's score) gets counted into the guardrail line below,
/// with a button that opens the Audit itself. "agent-model" is deliberately
/// NOT in this set: crates/core/src/audit.rs's agent_checks() only ever
/// gives it status "consider" (a model left to inherit is worth a look, not
/// a failing) -- it has no "attention" branch at all, so counting it here
/// would count something that can never happen. Exported for the same
/// registry test as AGENT_FINDING_IDS above.
export const AGENT_GUARDRAIL_CHECK_IDS = new Set(["agent-tools", "deny-shell"]);

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
let agentSpendError = "";
let failingGuardrails = 0;
/** The button that was clicked to open this view (the toolbar button, the
 *  door row, or null when opened some other way -- see openAgents()),
 *  restored on close. */
let opener: HTMLElement | null = null;

// ---------------------------------------------------------------------------
// Agent budgets and the live rule: one shared copy of what the backend last
// said, used by the Budgets section below and by the two Settings dropdowns
// (src/main.ts). It is the only place that calls get_agent_watch and
// set_agent_watch.
// ---------------------------------------------------------------------------

export interface AgentBudgetSetting {
  agent: string;
  monthlyBudget: number;
}
export interface LiveRuleSetting {
  hourlyPaceUsd: number | null;
  maxMinutes: number | null;
}
/** What set_agent_watch takes, and what the backend echoes back as `watch`. */
export interface AgentWatchPayload {
  budgets: AgentBudgetSetting[];
  live: LiveRuleSetting;
}
export interface AgentBudgetRow {
  agent: string;
  monthToDate: number;
  monthlyBudget: number;
}
export interface AgentWatchView {
  watch: AgentWatchPayload;
  budgets: AgentBudgetRow[];
  /** Not painted here: the findings carry the runaways. */
  runaways: unknown[];
  liveHint: number | null;
  known: string[];
}
/** What the add row holds between redraws: the picked name and the typed text. */
export interface BudgetDraft {
  agent: string;
  amount: string;
}

/** The backend's own limits, so the add row can refuse before it calls. */
export const MAX_AGENT_BUDGETS = 50;
export const MAX_BUDGET_USD = 1_000_000;

let watchView: AgentWatchView | null = null;
let watchError = "";
/** The note shown under the add row, cleared by the next success. Either which of this view's own messages it is (translated when painted, so
 *  a language switch changes it) or a refusal the backend already translated
 *  (shown as it came, and dropped on a language switch: it cannot be redone). */
let budgetNote: { key: string } | { text: string } | null = null;
/** True from the moment a save is sent to the moment its answer is painted.
 *  Every control that could start another save is disabled meanwhile, so two
 *  saves are never built from the same view and no change is overwritten. */
let saving = false;
let budgetDraft: BudgetDraft = { agent: "", amount: "" };
/** True while an input method is composing text in the amount field. A redraw
 *  replaces that field and would cut the composition off, so it waits. */
let composing = false;
let renderPending = false;
/** True while keyboard focus is parked on the panel heading because the
 *  control that had it was disabled by a save in flight. */
let parkedFocus = false;
/** Counts every get_agent_watch / set_agent_watch call. A load's answer paints
 *  only when it belongs to the latest call: one that answers after a later
 *  load, or after a save, describes a file that has since changed. (Saves
 *  never overlap and loads are skipped during one, so a save has no check.) */
let watchSeq = 0;
function noteText(): string {
  return !budgetNote ? "" : "key" in budgetNote ? t(budgetNote.key) : budgetNote.text;
}
const watchListeners = new Set<() => void>();

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

/// What close() should hand focus to: the element that opened this view, if
/// it is still attached to the document, else the fallback button --
/// deliberately a plain decision with no DOM reads of its own, so a test can
/// exercise every branch with plain objects standing in for elements. WebKit
/// does not focus a button on a mouse click (document.activeElement stays
/// <body>), which is why close() cannot simply restore "whatever had focus
/// before" the way it used to: the opener has to be the button the click
/// handler actually saw, passed in explicitly. `openerStillInDocument` is a
/// parameter rather than a `document.contains()` call inside this function
/// on purpose -- it is what makes this testable without a real DOM, and it
/// is also the reason a soon-to-close Audit's own "Open" button never wins
/// here: main.ts's goTo() passes no opener at all for that path (see the
/// comment on openAgents() below), so this function never even sees it.
export function closeFocusTarget(
  opener: HTMLElement | null,
  openerStillInDocument: boolean,
  fallback: HTMLElement | null,
): HTMLElement | null {
  if (opener && openerStillInDocument) return opener;
  return fallback;
}

/// The three headline numbers, computed once so the stats row (below) and
/// the door row's matching line in src/inventory.ts (agentsDoorLine()) can
/// never disagree about what they count: `yours` is the user's own agent
/// definitions, `runningNow` is agent HOSTS running right now, and `spend30`
/// is 30 days of ALL subagent spend -- overwhelmingly the built-in agents',
/// not the user's own few definitions, which is exactly why this view never
/// joins the three into one sentence any more (see the review this fixed:
/// "2 agents, 2 running, $17" read as if the two defined agents had spent
/// the $17). `spend30` sums the RAW per-row costs before money() rounds the
/// total once, so it can never drift from the sum of what "Your agents" and
/// "Built-in agents" show on their own rows -- rounding each of those rows
/// individually first and adding the rounded strings would not, in general,
/// equal this total.
export function agentStats(inv: Inventory, running: RunningAgent[], spend: AgentSpend[]): { yours: number; runningNow: number; spend30: number } {
  return { yours: inv.agents.length, runningNow: running.length, spend30: spend.reduce((sum, s) => sum + s.cost, 0) };
}

/// Why an add was refused before any call: the translated message's key.
export type BudgetInputProblem = "error.agentWatch.pick" | "error.agentWatch.figure";

/// The agents a new budget may still be set for: every known name that has no
/// budget yet. The picker offers exactly these, and Add accepts only these.
export function freeAgentNames(view: AgentWatchView): string[] {
  const taken = new Set(view.budgets.map((b) => b.agent));
  return view.known.filter((n) => !taken.has(n));
}

/// The add row's own check, the same bounds the backend enforces, so a typo
/// never costs a round trip: the agent must be one the picker offers (`free`),
/// and the amount must be digits only, a whole number from 1 to 1,000,000. The
/// text is normalised first so full-width digits typed through an input method
/// count; `Number()` is deliberately not used, since it also reads "1e3",
/// "0x10" and "1.0". Pure.
export function checkBudgetInput(agent: string, amountText: string, free: readonly string[]): { ok: true; amount: number } | { ok: false; key: BudgetInputProblem } {
  if (!agent || !free.includes(agent)) return { ok: false, key: "error.agentWatch.pick" };
  const text = amountText.normalize("NFKC").trim();
  if (!/^\d+$/.test(text)) return { ok: false, key: "error.agentWatch.figure" };
  const amount = Number(text);
  if (amount < 1 || amount > MAX_BUDGET_USD) return { ok: false, key: "error.agentWatch.figure" };
  return { ok: true, amount };
}

/// The whole watch to send for an add or a remove, built from the last view
/// the backend returned: its saved budgets plus or minus the one row, its live
/// rule untouched. Pure.
export function budgetPayload(last: AgentWatchView, change: { add: AgentBudgetSetting } | { remove: string }): AgentWatchPayload {
  const budgets = "add" in change ? [...last.watch.budgets, change.add] : last.watch.budgets.filter((b) => b.agent !== change.remove);
  return { budgets, live: last.watch.live };
}

/// The whole watch to send when one live-rule figure changes: the saved
/// budgets go back as they were, and only the named figure differs. Pure.
export function livePayload(last: AgentWatchView, change: Partial<LiveRuleSetting>): AgentWatchPayload {
  return { budgets: last.watch.budgets, live: { ...last.watch.live, ...change } };
}

/// The presets of the two live-rule dropdowns in Settings, as the numbers
/// their options carry: dollars an hour, and minutes open. Kept beside the
/// payload builders so the markup in index.html and these cannot drift (a test
/// reads both).
export const LIVE_PACE_PRESETS: readonly number[] = [5, 10, 25, 50, 100, 250];
export const LIVE_OPEN_PRESETS: readonly number[] = [60, 120, 240, 480, 720, 1440];

/// What a live-rule dropdown shows for the saved figure: "" is the Off option,
/// a preset selects its own option, and anything else (a hand-edited file) is
/// null, which the caller shows as a blank select. The figure keeps working
/// either way; nothing here rewrites it. Pure.
export function liveSelectValue(saved: number | null, presets: readonly number[]): string | null {
  if (saved === null) return "";
  return presets.includes(saved) ? String(saved) : null;
}

/// A control a keyboard step can hand focus to. A row is named by its agent,
/// never by a selector built from that name: a file name may hold any
/// character, and a selector made of one can throw.
export type BudgetFocusTarget = { remove: string } | { picker: true };

/// Where keyboard focus goes after a Remove, first match wins: the row that
/// moved up into its place, else the row before it, else the agent picker.
/// Nothing for a pointer user, who is left with nothing armed
/// (src/tabbable.ts). `names` are the rows as they were before the removal.
export function budgetRemoveFocus(names: string[], removed: string, byKeyboard: boolean): BudgetFocusTarget[] {
  if (!byKeyboard) return [];
  const own = (n: string | undefined): BudgetFocusTarget[] => (n === undefined ? [] : [{ remove: n }]);
  const at = names.indexOf(removed);
  if (at < 0) return [{ picker: true }];
  return [...own(names[at + 1]), ...(at > 0 ? own(names[at - 1]) : []), { picker: true }];
}

/// The Budgets section: pure, takes the last view (null until the first
/// answer), the load error (with a view it is shown beside the controls), the add row's draft and
/// the rejection to show. With no view there are no controls at all, so no
/// payload can ever be built from an empty default and written over the
/// user's budgets.
export function renderBudgetsSection(view: AgentWatchView | null, error: string, draft: BudgetDraft, note = "", busy = false): string {
  const off = busy ? " disabled" : "";
  const head = (count: number | null) =>
    `<h3>${esc(T("budgets.title"))}${count === null ? "" : ` <span class="plan">${count}</span>`}</h3><p class="inv-note">${esc(T("budgets.note"))}</p>`;
  const open = `<section class="dt-section" data-section="agent-budgets">`;
  if (!view) {
    const body = error ? T("budgets.loadError", { error }) : t("detail.loading");
    return `${open}${head(null)}<p class="inv-empty">${esc(body)}</p></section>`;
  }
  // A failed reload keeps the last good view and its controls; the error sits beside them.
  const reloadError = error ? `<p class="lg-error" role="alert">${esc(T("budgets.loadError", { error }))}</p>` : "";
  const rows = view.budgets
    .map((b) => {
      const over = b.monthToDate >= b.monthlyBudget;
      const line = b.monthToDate === 0
        ? T("budgets.noSpend", { budget: wholeMoney(b.monthlyBudget) })
        : T("budgets.spent", { spent: money(b.monthToDate), budget: wholeMoney(b.monthlyBudget) });
      return `
      <div class="inv-row ag-budget-row">
        <div class="inv-row-main">
          <span class="inv-name" title="${esc(b.agent)}">${esc(b.agent)}</span>
          <span class="inv-sub inv-sub-wrap${over ? " dt-forecast-hit" : ""}">${esc(line)}</span>
        </div>
        <div class="inv-row-meta"><button class="inv-learn" data-budget-remove="${esc(b.agent)}"${off} aria-label="${esc(T("budgets.removeAria", { agent: b.agent }))}">${esc(T("budgets.remove"))}</button></div>
      </div>`;
    })
    .join("");
  const free = freeAgentNames(view);
  const options = free
    .map((n) => `<option value="${esc(n)}"${n === draft.agent ? " selected" : ""}>${esc(n)}</option>`)
    .join("");
  const addRow =
    view.budgets.length >= MAX_AGENT_BUDGETS
      ? `<p class="dt-caption">${esc(T("budgets.full", { max: MAX_AGENT_BUDGETS }))}</p>`
      : free.length === 0
        ? `<p class="dt-caption">${esc(T("budgets.allSet"))}</p>`
        : `<div class="ag-budget-add">
          <select id="agent-budget-pick"${off} aria-label="${esc(T("budgets.pickAria"))}">
            <option value="" disabled${free.includes(draft.agent) ? "" : " selected"}>${esc(T("budgets.pick"))}</option>${options}
          </select>
          <input id="agent-budget-amount"${off} type="text" inputmode="numeric" autocomplete="off" placeholder="${esc(t("detail.client.budgetPh"))}" value="${esc(draft.amount)}" aria-label="${esc(T("budgets.amountAria"))}" />
          <button class="inv-learn" id="agent-budget-add"${off}>${esc(t("ledger.add"))}</button>
        </div>`;
  const empty = view.budgets.length ? "" : `<p class="inv-empty">${esc(T("budgets.empty"))}</p>`;
  const noteLine = note ? `<p class="lg-error" role="alert">${esc(note)}</p>` : "";
  return `${open}${head(view.budgets.length)}${reloadError}${rows}${empty}${addRow}${noteLine}</section>`;
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
  spendError: string,
  failing: number,
  nowMs: number,
  watch: AgentWatchView | null = null,
  watchLoadError = "",
  draft: BudgetDraft = { agent: "", amount: "" },
  budgetRejection = "",
  budgetBusy = false,
): string {
  // 1. Three labelled facts, not one sentence that joins a count of the
  // user's own agents with a cost that is mostly built-ins' (see agentStats()
  // above). Wraps at 380px via flex-wrap, same as every other toolbar/filter
  // row in this app; no card-inside-card, just the app's own caption size
  // and colour for the label under a slightly larger number.
  // The spend figure falls back to "?" (this app's existing convention for a
  // figure it could not read -- see src/inventory.ts's trustChip()) when
  // spendError is set: `spend` is `[]` on a failed read the same as it would
  // be on a genuinely quiet 30 days, and stats.spend30 sums to 0 either way,
  // so "$0.00" would show as a real, confident zero instead of the unknown it
  // actually is.
  const stats = agentStats(inv, running, spend);
  // The third figure's label used to carry its own period ("Subagent spend,
  // 30 days"), which wraps to two lines in Russian, French, Portuguese and
  // German -- the label alone never does. Split: the noun stays the label,
  // and the period becomes its own, smaller caption line under this figure
  // only, reusing "detail.range.last30d" (the same "Last 30 days" every
  // locale already ships for the history range picker) rather than minting
  // a second string for the same fact.
  const stats3 = `<span class="ag-stat-label">${esc(T("stat.spendLabel"))}</span><span class="ag-stat-period">${esc(t("detail.range.last30d"))}</span>`;
  const statsRow = `
    <div class="ag-stats">
      <div class="ag-stat"><span class="ag-stat-n">${stats.yours}</span><span class="ag-stat-label">${esc(T("stat.yours"))}</span></div>
      <div class="ag-stat"><span class="ag-stat-n">${stats.runningNow}</span><span class="ag-stat-label">${esc(T("stat.running"))}</span></div>
      <div class="ag-stat"><span class="ag-stat-n">${spendError ? "?" : esc(money(stats.spend30))}</span>${stats3}</div>
    </div>`;

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
  // A null spend map (spendError set) makes agentRows() drop every row's
  // spend sub-line entirely, rather than showing each of the user's agents
  // as "Never run" -- a claim about the agent -- when the truth is this view
  // simply could not read spend at all -- a claim about the request.
  const spendByName = spendError ? null : new Map(spend.map((s) => [s.name, s]));
  const yourAgentsBody = inv.agents.length
    ? agentRows(inv.agents, spendByName, nowMs)
    : `<p class="inv-empty">${esc(t("inventory.empty.agents"))}</p>`;

  // 4. Built-in agents -- same rows Setup used to show. builtInAgentRows()
  // stays silent when there is nothing to attribute, which reads fine when
  // SOME subagent spend exists but none of it happens to be a built-in's;
  // when there has been NO subagent spend at all in the window, that silence
  // would look like a gap instead of an answer, so this view names it. Either
  // way the section's own heading carries the count and the hint stays under
  // the title, the same shape every other section in this view uses.
  // A spend read failure pre-empts both of those readings: there is no "0
  // built-ins" or "no runs" to report, only that the 30-day read itself
  // failed, so the section shows that error in place of its rows or its
  // empty state.
  const totalRuns = spend.reduce((sum, s) => sum + s.runs, 0);
  const builtInCount = builtInAgentCount(inv.agents, spend);
  const builtInsBody = builtInAgentRows(inv.agents, spend, nowMs);
  // No count beside the title when the read failed: how many ran is exactly
  // what is not known, and a 0 there would be one more figure stated as fact.
  const builtInHead = (count: number | null) => `<h3>${esc(t("inventory.agents.builtIn"))}${count === null ? "" : ` <span class="plan">${count}</span>`}</h3><p class="inv-note">${esc(t("inventory.agents.builtInHint"))}</p>`;
  const builtInSection = spendError
    ? `<section class="dt-section">${builtInHead(null)}<p class="inv-empty">${esc(T("spendError", { error: spendError }))}</p></section>`
    : builtInsBody
      ? `<section class="dt-section">${builtInHead(builtInCount)}${builtInsBody}</section>`
      : totalRuns === 0
        ? `<section class="dt-section">${builtInHead(0)}<p class="inv-empty">${esc(T("empty.noSubagentRuns"))}</p></section>`
        : "";

  // 5. Worth a look -- the findings that are about agents, and only those
  // (see AGENT_FINDING_IDS above), painted with the exact renderer
  // Inventory's own Opportunities section uses for each one, so the text
  // never drifts from what that section shows for the same finding. A
  // failing agent guardrail check is a one-line count with a button to the
  // Audit, not a second Opportunity card -- the Audit is still where that
  // check is scored, so it is not part of this heading's own count either.
  const findings = inv.opportunities.filter((o) => AGENT_FINDING_IDS.has(o.id));
  const findingsBody = findings.length
    ? renderOpportunityRows(findings)
    : `<p class="inv-empty">${esc(T("empty.nothingToFlag"))}</p>`;
  // A block, not a button inside a sentence: the sentence sits on its own
  // line(s) and the button sits under it, left aligned, same gap as the
  // "Learn more" buttons above have under their own text (.ag-guardrail's
  // rule in styles.css). A button inside a <p> wrapped differently in every
  // language -- beside the text in English, under it and indented in German
  // and Russian, crowded against the last word in Portuguese -- which is
  // exactly what this fixed.
  const guardrailLine =
    failing > 0
      ? `<div class="ag-guardrail"><p class="dt-caption">${esc(plural("agents.guardrailFailing", failing))}</p><button class="inv-learn" id="agents-open-audit">${esc(T("openAudit"))}</button></div>`
      : "";

  // One shape for all four section titles -- a name with its count beside
  // it, same as Inventory's own sections ("Running now 2", "Your agents 2")
  // -- rather than the three different shapes this view used to mix: a bare
  // h3 with no count (Your agents, Worth a look), an h3 with a nested
  // sub-heading duplicating its own count underneath (Running now used to
  // carry an "AGENTS 2" grouphead plus a "2 agents running" line, both
  // dead now that the section title itself carries the number), and no h3 at
  // all, just a grouphead from inside the row renderer (Built-in agents).
  return `
    ${statsRow}
    <section class="dt-section">
      <h3>${esc(T("section.running"))} <span class="plan">${running.length}</span></h3>
      ${runningBody}
    </section>
    <section class="dt-section">
      <h3>${esc(T("section.yours"))} <span class="plan">${inv.agents.length}</span></h3>
      ${yourAgentsBody}
    </section>
    ${builtInSection}
    ${renderBudgetsSection(watch, watchLoadError, draft, budgetRejection, budgetBusy)}
    <section class="dt-section">
      <h3>${esc(T("section.worthALook"))} <span class="plan">${findings.length}</span></h3>
      ${findingsBody}
      ${guardrailLine}
    </section>`;
}

/// The load-error page, pulled out of render() below so a hostile error
/// string's escaping can be exercised directly, the same way renderAgentsView()
/// already lets a test feed a hostile `runningError` straight in.
export function renderLoadError(error: string): string {
  return `<p class="dt-empty">${esc(T("loadError", { error }))}</p>`;
}

/// How long a load stays "fresh" -- reloadAgents() (called every time the
/// popover is shown again) skips its own re-fetch inside this window, since
/// reopening the popover happens far more often than this view's underlying
/// data actually changes. A rescan (applyRescan() below) is a different
/// event -- the setup genuinely changed -- so it never checks this and
/// always applies what it is handed.
export const RELOAD_FRESHNESS_MS = 60_000;

/// When the popover-shown path should actually reload: never for a closed
/// view (nothing to refresh); never while a load this view itself started is
/// still in flight (`loading` -- reopening the popover mid-fetch must not
/// stack a second concurrent get_inventory); always at once when the last
/// load FAILED (`lastFailed`) -- a stale error has nothing worth waiting 60s
/// behind; never for one whose last successful load is still fresh; always
/// once it has gone stale; and always when the clock has moved backwards
/// (`now < lastLoadedAt`, e.g. a system clock change) -- a negative age is
/// not a trustworthy "fresh", so that case reloads rather than trusting it.
/// Pure and exported so it can be tested without a DOM (see its own test).
export function shouldReload(
  lastLoadedAt: number,
  now: number,
  open: boolean,
  loading: boolean,
  lastFailed: boolean,
): boolean {
  if (!open) return false;
  if (loading) return false;
  if (lastFailed) return true;
  if (now < lastLoadedAt) return true;
  return now - lastLoadedAt >= RELOAD_FRESHNESS_MS;
}

/// Stamped only when a load actually SUCCEEDS (get_inventory's own promise
/// resolving, in loadData() below, or a rescan that carried no loadError, in
/// applyRescan()) -- never optimistically at call time. A failed load used to
/// stamp this immediately, which made the view read as fresh for a full
/// minute despite showing an error, so reopening the popover within that
/// window silently skipped the retry a user opening it again was clearly
/// asking for.
let lastLoadedAt = 0;
/// True from the moment loadData() fires its get_inventory call to the moment
/// that call settles (either way) -- tracked separately from `lastLoadedAt`
/// so reloadAgents() can tell "a load is already in flight, don't start a
/// second one" apart from "the last load is still fresh", which used to be
/// the same signal and so could not express both figures.
let loading = false;
/// True when the last get_inventory call failed, cleared the moment one
/// succeeds. Lets shouldReload() retry immediately the next time the popover
/// is shown, instead of trusting the stale `lastLoadedAt` a failure used to
/// leave behind.
let lastFailed = false;

function isOpen(): boolean {
  return document.body.classList.contains("agents-open");
}

/// Guards against a load that was in flight finishing after the panel has
/// since closed -- without this, a slow invoke() landing late would still
/// overwrite #agents-body's content (and reset its scroll) underneath a view
/// the user is no longer looking at.
function render(): void {
  if (!isOpen()) return;
  const el = document.querySelector<HTMLElement>("#agents-body");
  if (!el) return;
  // These two replace the whole body and destroy the amount field, and a
  // destroyed field usually delivers no compositionend: the gate must not be
  // left shut for every redraw that follows.
  if (loadError) {
    endCompositionGate();
    el.innerHTML = renderLoadError(loadError);
    return;
  }
  if (!inventory) {
    endCompositionGate();
    el.innerHTML = `<p class="dt-empty">${esc(t("detail.loading"))}</p>`;
    return;
  }
  if (composing) {
    renderPending = true;
    return;
  }
  // The amount field is replaced by this redraw, and a text field's caret
  // lives in the element: remember it and put it back, or a redraw between
  // two keystrokes would turn "25" into "52".
  const active = document.activeElement as HTMLInputElement | null;
  const caret = active && active.id === "agent-budget-amount" ? { start: active.selectionStart, end: active.selectionEnd } : null;
  el.innerHTML = renderAgentsView(inventory, runningAgents, runningAgentsError, agentSpend, agentSpendError, failingGuardrails, Date.now(), watchView, watchError, budgetDraft, noteText(), saving);
  if (!caret) return;
  const field = document.querySelector<HTMLInputElement>("#agent-budget-amount");
  if (!field || field.disabled) return;
  field.focus({ preventScroll: true });
  if (caret.start !== null && caret.end !== null) field.setSelectionRange(caret.start, caret.end);
}

/// Opens the redraw gate without redrawing: used wherever the field is gone or
/// has lost focus, so a composition that will never report its end cannot hold
/// back every later redraw (a save's result, a language switch).
function endCompositionGate(): void {
  composing = false;
  renderPending = false;
}

/// An input method starts or ends composing in the amount field. A redraw
/// asked for meanwhile is held back and runs when the composition ends.
export function noteComposition(on: boolean): void {
  composing = on;
  if (!on && renderPending) {
    renderPending = false;
    render();
  }
}

/// The composition ended, or the field lost focus (which ends one too): the
/// field's own value is what was committed. Some engines deliver the final
/// text in an input event after compositionend, and that event lands on the
/// field this redraw is about to replace, so the value is taken now, before
/// the gate opens.
export function finishComposition(fieldValue: string): void {
  noteBudgetDraft("amount", fieldValue);
  noteComposition(false);
}

/// Whether a key press in the amount field means Add: Enter, not a held-key
/// repeat, and not the Enter that confirms an input method's composition
/// (keyCode 229 is how some engines mark one that isComposing missed).
export function enterAddsBudget(key: string, repeat: boolean, isComposing: boolean, keyCode: number, targetId: string): boolean {
  return key === "Enter" && !repeat && !isComposing && keyCode !== 229 && targetId === "agent-budget-amount";
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

// ---------------------------------------------------------------------------
// Loading and saving the watch
// ---------------------------------------------------------------------------

/// What the last answer was, for the Settings dropdowns.
export function agentWatchState(): { view: AgentWatchView | null; error: string; saving: boolean } {
  return { view: watchView, error: watchError, saving };
}

/// Called after every painted answer, so Settings can repaint its two selects.
export function onAgentWatchChange(listener: () => void): void {
  watchListeners.add(listener);
}

function announceWatch(): void {
  // A drafted agent that is no longer offered (it has a budget now, or it left
  // the known names) must not be sent: the picker shows the placeholder for it.
  if (watchView && !freeAgentNames(watchView).includes(budgetDraft.agent)) budgetDraft = { ...budgetDraft, agent: "" };
  render();
  // forEach, not for-of: the test harness transpiles with no target, which
  // would silently skip every iteration over a Set.
  watchListeners.forEach((listener) => listener());
}

/// Fetches the saved watch. With no view ever loaded a failure leaves none, so
/// nothing can build a payload from an empty default and only the error shows.
/// With a good view already held, a failed reload keeps it (and its controls)
/// and shows the error beside them: that view is still exactly what the
/// backend last returned.
///
/// While a save is in flight a load is skipped: that save's answer is the
/// fresh view.
export function loadAgentWatch(): Promise<void> {
  if (saving) return Promise.resolve();
  const seq = ++watchSeq;
  return invoke<AgentWatchView>("get_agent_watch").then(
    (view) => {
      if (seq !== watchSeq) return;
      watchView = view;
      watchError = "";
      announceWatch();
    },
    (err) => {
      if (seq !== watchSeq) return;
      // The last good view stays, and a later save writes the whole watch from
      // it. That is safe: this app is the only writer of the file, and both
      // places that edit it (the budgets and the Settings dropdowns) share
      // this one copy, so the kept view is never behind a change either made.
      watchError = String(err);
      announceWatch();
    },
  );
}

export type WatchSave =
  | { outcome: "saved" }
  /** The backend refused; `error` is already translated and is shown as it is. */
  | { outcome: "rejected"; error: string }
  /** Nothing has loaded yet: no call is made, because there is nothing to build from. */
  | { outcome: "notLoaded" }
  /** Another save is still in flight; nothing is sent. */
  | { outcome: "busy" };

/// Sends a whole watch built from the last view the backend returned and
/// paints the view that comes back. One save at a time: while it is in flight
/// the controls that could start another are disabled, and a second call
/// sends nothing. `beforePaint` runs only on success, just before the repaint.
export function saveAgentWatch(build: (last: AgentWatchView) => AgentWatchPayload, beforePaint?: () => void): Promise<WatchSave> {
  if (saving) return Promise.resolve({ outcome: "busy" });
  const last = watchView;
  if (!last) return Promise.resolve({ outcome: "notLoaded" });
  // Built before the flag is set: a builder that throws must not leave every
  // control disabled for good.
  const payload = build(last);
  // A load already in flight describes the file as it was before this save.
  watchSeq += 1;
  saving = true;
  // Repaint now, so every control that could start a second save is disabled.
  announceWatch();
  return invoke<AgentWatchView>("set_agent_watch", { watch: payload }).then(
    (view): WatchSave => {
      saving = false;
      watchView = view;
      watchError = "";
      budgetNote = null;
      beforePaint?.();
      announceWatch();
      return { outcome: "saved" };
    },
    (err): WatchSave => {
      saving = false;
      announceWatch();
      return { outcome: "rejected", error: String(err) };
    },
  );
}

/// One live-rule figure changed in Settings (null is Off). The saved budgets
/// travel back untouched.
export function saveAgentLive(change: Partial<LiveRuleSetting>): Promise<WatchSave> {
  return saveAgentWatch((last) => livePayload(last, change));
}

/// Whether a Settings dropdown takes focus back after its save: only if it had
/// focus before, Settings is still open, and focus is now nowhere, on the body,
/// or still on that select. Focus the user has moved elsewhere is left alone.
/// The same "lost or ours" rule focusFirstMatch follows in this view. Pure.
export function selectTakesFocusBack(hadFocus: boolean, settingsOpen: boolean, focusIsNowhere: boolean, focusIsOnSelect: boolean): boolean {
  return hadFocus && settingsOpen && (focusIsNowhere || focusIsOnSelect);
}

/// A Settings dropdown changed: which figure, and the select's value ("" is
/// Off). Returns what happened, the backend's refusal if there was one, and the
/// value the select must show now (what is saved, which after a refusal is the
/// old figure), so src/main.ts only reads the DOM and paints.
export async function changeLiveRule(figure: keyof LiveRuleSetting, value: string): Promise<{ outcome: WatchSave["outcome"]; error: string; show: string | null }> {
  // Never rejects: a payload builder that throws must still put the select
  // back to what is saved, not leave it on a value that was never saved.
  let result: WatchSave;
  try {
    result = await saveAgentLive({ [figure]: value === "" ? null : Number(value) });
  } catch (err) {
    result = { outcome: "rejected", error: String(err) };
  }
  const live = watchView?.watch?.live;
  const presets = figure === "hourlyPaceUsd" ? LIVE_PACE_PRESETS : LIVE_OPEN_PRESETS;
  return { outcome: result.outcome, error: result.outcome === "rejected" ? result.error : "", show: live ? liveSelectValue(live[figure], presets) : null };
}

/// Remembers what the add row holds, so a redraw (a background refresh, a
/// locale switch) does not lose a half-typed budget.
export function noteBudgetDraft(field: keyof BudgetDraft, value: string): void {
  budgetDraft = { ...budgetDraft, [field]: value };
}

function inBudgets(el: Element | null): boolean {
  return el?.closest?.('[data-section="agent-budgets"]') != null;
}

/// A save in flight disables the control a keyboard user pressed, and a
/// disabled control cannot keep focus; in the macOS webview focus that falls
/// to nothing can leave Tab dead. So focus waits on the panel heading, a place
/// that is always focusable, until the answer is painted.
function parkFocus(): void {
  const heading = document.querySelector<HTMLElement>("#agents-heading");
  if (!heading) return;
  heading.focus();
  parkedFocus = true;
}

/// After a keyboard step the redraw has destroyed the control that had focus.
/// Focus moves only when it fell to nothing, is parked on the heading, or is
/// still inside this section: a user who has since gone elsewhere keeps their
/// place.
function focusFirstMatch(targets: BudgetFocusTarget[]): void {
  const active = document.activeElement as HTMLElement | null;
  const lost = !active || active === document.body || inBudgets(active) || (parkedFocus && active.id === "agents-heading");
  parkedFocus = false;
  if (!lost) return;
  const find = (target: BudgetFocusTarget): HTMLElement | null =>
    "picker" in target
      ? document.querySelector<HTMLElement>("#agent-budget-pick")
      : Array.from(document.querySelectorAll<HTMLElement>("[data-budget-remove]")).find((b) => b.dataset.budgetRemove === target.remove) ?? null;
  const found = targets.map(find).find((el) => el);
  if (found) focusOrFallback(found);
}

/// The Add button, or Enter in the amount field. `byKeyboard` is false for a
/// pointer click, which leaves nothing focused.
export async function addBudget(byKeyboard: boolean): Promise<void> {
  const checked = checkBudgetInput(budgetDraft.agent, budgetDraft.amount, watchView ? freeAgentNames(watchView) : []);
  if (!checked.ok) {
    budgetNote = { key: checked.key };
    render();
    if (byKeyboard) focusFirstMatch([{ picker: true }]);
    return;
  }
  const agent = budgetDraft.agent;
  const hadFocus = byKeyboard && inBudgets(document.activeElement);
  const pending = saveAgentWatch(
    (last) => budgetPayload(last, { add: { agent, monthlyBudget: checked.amount } }),
    () => { budgetDraft = { agent: "", amount: "" }; },
  );
  if (hadFocus && saving) parkFocus();
  const result = await pending;
  if (result.outcome === "rejected") {
    budgetNote = { text: result.error };
    render();
  }
  if (byKeyboard && (result.outcome === "saved" || result.outcome === "rejected")) focusFirstMatch([{ picker: true }]);
}

/// A row's Remove button.
export async function removeBudget(agent: string, byKeyboard: boolean): Promise<void> {
  const names = (watchView?.budgets ?? []).map((b) => b.agent);
  const hadFocus = byKeyboard && inBudgets(document.activeElement);
  const pending = saveAgentWatch((last) => budgetPayload(last, { remove: agent }));
  if (hadFocus && saving) parkFocus();
  const result = await pending;
  if (result.outcome === "rejected") {
    budgetNote = { text: result.error };
    render();
  }
  if (!byKeyboard) return;
  if (result.outcome === "saved") focusFirstMatch(budgetRemoveFocus(names, agent, true));
  if (result.outcome === "rejected") focusFirstMatch([{ remove: agent }]);
}

/// The four invoke() calls this view's data comes from, fired together --
/// used by openAgents() (the first load, always fresh) and reloadAgents()
/// (every later popover-shown refresh, once shouldReload() above says the
/// current data has actually gone stale). Deliberately does not touch
/// `loadError` on the way in and does not reset any of the four pieces of
/// state before the calls resolve: this must never flash a "loading" page
/// over content the panel is already showing, so the previous render stays
/// up until fresh data actually lands, one piece at a time, same as it
/// always has. A rescan does NOT come through here any more -- see
/// applyRescan() below, which is handed three of these four pieces directly
/// instead of re-invoking them.
function loadData(): void {
  loading = true;
  void invoke<Inventory>("get_inventory").then(
    (inv) => {
      inventory = inv;
      loadError = "";
      lastLoadedAt = Date.now();
      loading = false;
      lastFailed = false;
      render();
    },
    (err) => {
      loadError = String(err);
      loading = false;
      lastFailed = true;
      render();
    },
  );
  void invoke<RunningAgent[]>("get_running_agents").then(
    (rows) => { runningAgents = rows; runningAgentsError = ""; render(); },
    (err) => { runningAgents = []; runningAgentsError = String(err); render(); },
  );
  void invoke<AgentSpend[]>("get_agent_spend").then(
    (rows) => { agentSpend = rows; agentSpendError = ""; render(); },
    (err) => { agentSpend = []; agentSpendError = String(err); render(); },
  );
  void loadFailingGuardrails().then((n) => { failingGuardrails = n; render(); });
  void loadAgentWatch();
}

function close(): void {
  endCompositionGate();
  document.body.classList.remove("agents-open");
  // src/panels.ts is the sole writer of `inert` on panels and the
  // background now -- see that module's own header for why this is called
  // synchronously rather than left to its MutationObserver alone.
  syncPanels();
  const fallback = document.querySelector<HTMLElement>("#agents-open-btn");
  const stillThere = opener != null && document.contains(opener);
  const candidate = closeFocusTarget(opener, stillThere, fallback);
  focusOrFallback(focusAfterClose(candidate));
  opener = null;
}

/// Opens the view and loads its own data fresh -- the same way openAudit()
/// (src/audit.ts) owns its own get_audit() call rather than reading
/// whatever Inventory happens to have cached, so this works whether or not
/// the Inventory tab was ever visited this session.
///
/// `opener` is the element close() returns focus to, when it is still in the
/// document -- the button the click handler actually saw (setupAgents()
/// passes #agents-open-btn or the door row's button), never
/// `document.activeElement`: WebKit does not focus a button on a mouse
/// click, so reading the active element back would just see <body>. Left
/// out (main.ts's goTo() does this for the Audit's own "Open" button) when
/// there is no real opener to return to -- the Audit has already closed by
/// the time this runs, and its button, though still technically attached
/// off-screen, is not where focus should land -- close() then falls back to
/// #agents-open-btn on its own.
export function openAgents(opener_: HTMLElement | null = null): void {
  opener = opener_;
  composing = false;
  renderPending = false;
  document.body.classList.add("agents-open");
  syncPanels();
  loadError = "";
  render();
  loadData();
  // The heading lives in the static panel head (index.html), not in
  // anything render() paints, so it is already there to receive focus.
  document.querySelector<HTMLElement>("#agents-heading")?.focus();
}

/// Refreshes this view's own data in place, without opening the panel,
/// capturing a new opener, or moving focus -- called when the popover is
/// shown again while this view happens to still be open (main.ts wires this
/// into "popover-shown"). Gated by shouldReload(): reopening the popover
/// happens far more often than the underlying data changes, so this skips
/// its own get_inventory / get_running_agents / get_agent_spend / get_audit
/// calls entirely when the last load is still within RELOAD_FRESHNESS_MS.
/// A rescan finishing is a SEPARATE event, handled by applyRescan() below,
/// not by this function -- a rescan always applies (the setup genuinely just
/// changed), where this is purely "is it worth asking again". loadData()
/// never clears the current render before the new data lands, so
/// #agents-body's content only changes once, in place -- #agents itself
/// (not #agents-body) is the scrollable element, and replacing a child's
/// innerHTML does not reset its parent's scrollTop, so the panel's scroll
/// position survives a reload on its own, with nothing here to manage.
export function reloadAgents(): void {
  if (!shouldReload(lastLoadedAt, Date.now(), isOpen(), loading, lastFailed)) return;
  loadData();
}

/// Applies a rescan's results directly -- called from the `rescanned`
/// callback src/inventory.ts's own load() fires once a rescan has settled,
/// carrying exactly the three pieces of this view's data that a rescan also
/// produces (get_inventory, get_running_agents, get_agent_spend). Unlike
/// reloadAgents() above, this never re-invokes those three commands itself:
/// that would be the same redundant second full scan this function exists to
/// remove. It still fetches the failing-guardrail count fresh
/// (loadFailingGuardrails(), i.e. get_audit) because a rescan's own data
/// carries nothing about the Audit's checks -- those are computed by a
/// separate command a rescan never touches, so there is nothing to reuse for
/// it. Always applies when the panel is open (a rescan is a real change, not
/// a "maybe" like reloadAgents()'s freshness check); a no-op while closed,
/// same as every other refresh path here -- openAgents() reloads everything
/// fresh on its own the next time this view opens regardless.
export function applyRescan(data: RescanResult): void {
  if (!isOpen()) return;
  inventory = data.inventory;
  loadError = data.loadError;
  runningAgents = data.runningAgents;
  runningAgentsError = data.runningAgentsError;
  agentSpend = data.agentSpend;
  agentSpendError = data.agentSpendError;
  // Same "stamp freshness only on success" rule loadData() follows: a rescan
  // whose own get_inventory failed (data.loadError set) must not read as
  // fresh for the next 60s of popover-shown checks either.
  if (data.loadError) {
    lastFailed = true;
  } else {
    lastLoadedAt = Date.now();
    lastFailed = false;
  }
  render();
  void loadFailingGuardrails().then((n) => { failingGuardrails = n; render(); });
}

/// Redraws the Agents panel in place, e.g. after a locale switch. A no-op
/// while the panel is closed.
export function rerender(): void {
  // A refusal from the backend arrived already translated: it cannot follow
  // the language, so it goes rather than stay in the old one.
  if (budgetNote && "text" in budgetNote) budgetNote = null;
  if (document.body.classList.contains("agents-open")) render();
}

export function setupAgents(h: AgentsHost): void {
  host = h;
  document.querySelector("#agents-close")?.addEventListener("click", close);
  // Global click delegation for both places Inventory opens this view from
  // (its toolbar button and its own summary door row) -- the same mechanism
  // audit.ts uses for #audit-open-btn, so it works no matter which module
  // rendered the button. Passes the actual button clicked, not just the
  // fact that one was, so close() has a real opener to return focus to.
  document.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>("#agents-open-btn, #agents-door-btn");
    if (btn) openAgents(btn);
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
      return;
    }
    // `detail` is 0 for a click that Enter or Space made: only then is there
    // a keyboard user whose place has to be kept through the redraw.
    const byKeyboard = (e as MouseEvent).detail === 0;
    const remove = target.closest<HTMLElement>("[data-budget-remove]");
    if (remove) {
      void removeBudget(remove.dataset.budgetRemove ?? "", byKeyboard);
      return;
    }
    if (target.closest("#agent-budget-add")) void addBudget(byKeyboard);
  });
  const body = document.querySelector("#agents-body");
  const noteDraft = (e: Event) => {
    const field = e.target as HTMLInputElement | HTMLSelectElement;
    if (field.id === "agent-budget-pick") noteBudgetDraft("agent", field.value);
    if (field.id === "agent-budget-amount") noteBudgetDraft("amount", field.value);
  };
  body?.addEventListener("input", noteDraft);
  body?.addEventListener("change", noteDraft);
  body?.addEventListener("keydown", (e) => {
    const ke = e as KeyboardEvent;
    if (!enterAddsBudget(ke.key, ke.repeat, ke.isComposing, ke.keyCode, (ke.target as HTMLElement).id)) return;
    ke.preventDefault();
    void addBudget(true);
  });
  const isAmount = (e: Event) => (e.target as HTMLElement).id === "agent-budget-amount";
  body?.addEventListener("compositionstart", (e) => { if (isAmount(e)) noteComposition(true); });
  body?.addEventListener("compositionend", (e) => { if (isAmount(e)) finishComposition((e.target as HTMLInputElement).value); });
  // Leaving the field ends a composition whether or not an end was reported.
  body?.addEventListener("focusout", (e) => { if (isAmount(e)) finishComposition((e.target as HTMLInputElement).value); });
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key === "Escape" && isTopPanel("agents")) {
        e.stopImmediatePropagation();
        e.preventDefault();
        close();
      }
    },
    true,
  );
}
