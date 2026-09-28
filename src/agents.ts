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
import { money } from "./format";
import { plural, t } from "./i18n";
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
/// (the one usage-coaching finding that is about agents). Every other id in
/// either registry is about MCP servers, permissions or usage patterns that
/// have nothing to do with agents, so it stays out of this list. Exported so
/// scripts/agent-id-registries.test.mjs can check this set against the Rust
/// source directly, rather than trusting this comment to stay accurate.
export const AGENT_FINDING_IDS = new Set(["agents-none", "agents-model-unset", "agent-unused", "subagent-share"]);

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
  const statsRow = `
    <div class="ag-stats">
      <div class="ag-stat"><span class="ag-stat-n">${stats.yours}</span><span class="ag-stat-label">${esc(T("stat.yours"))}</span></div>
      <div class="ag-stat"><span class="ag-stat-n">${stats.runningNow}</span><span class="ag-stat-label">${esc(T("stat.running"))}</span></div>
      <div class="ag-stat"><span class="ag-stat-n">${spendError ? "?" : esc(money(stats.spend30))}</span><span class="ag-stat-label">${esc(T("stat.spend"))}</span></div>
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
  if (loadError) {
    el.innerHTML = renderLoadError(loadError);
    return;
  }
  if (!inventory) {
    el.innerHTML = `<p class="dt-empty">${esc(t("detail.loading"))}</p>`;
    return;
  }
  el.innerHTML = renderAgentsView(inventory, runningAgents, runningAgentsError, agentSpend, agentSpendError, failingGuardrails, Date.now());
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
}

function close(): void {
  document.body.classList.remove("agents-open");
  const fallback = document.querySelector<HTMLElement>("#agents-open-btn");
  const stillThere = opener != null && document.contains(opener);
  closeFocusTarget(opener, stillThere, fallback)?.focus();
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
  document.body.classList.add("agents-open");
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
