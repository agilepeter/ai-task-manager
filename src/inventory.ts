// Inventory tab: what AI tooling is wired up on this machine, and what that
// setup suggests learning or tightening next. The Rust side (`inventory.rs`)
// hands over names, shapes and counts only, never a config value.

import { invoke } from "@tauri-apps/api/core";
import { showLedger } from "./ledger";
import { localeTag, plural, t } from "./i18n";

const T = (k: string, v?: Record<string, string | number>) => t(`inventory.${k}`, v);

interface McpServer {
  name: string;
  client: string;
  scope: "user" | "project";
  project: string | null;
  transport: string;
  target: string;
  package: string | null;
  envCount: number;
  /** What a one-click pin would write, when the package is unpinned and a version is cached locally. */
  pinTo: string | null;
}

/** One MCP server as it exists in memory right now. Never a command line. */
interface RunningServer {
  name: string;
  configured: boolean;
  client: string | null;
  package: string | null;
  instances: number;
  rssBytes: number;
  elapsedSecs: number;
  pids: number[];
}

interface Probe {
  kind: "file" | "keychain" | "folder";
  location: string;
  found: boolean | null;
}

interface Diagnosis {
  id: string;
  name: string;
  probes: Probe[];
  verifiedHere: boolean;
  hint: string;
}

interface PinPlan {
  file: string;
  package: string;
  installedVersion: string;
  from: string;
  to: string;
  occurrences: number;
  fileLen: number;
  fileMtimeMs: number;
}

interface Definition {
  name: string;
  scope: "user" | "project";
  project: string | null;
  model: string | null;
}

interface Opportunity {
  id: string;
  kind: "tighten" | "learn";
  title: string;
  detail: string;
  learnUrl: string | null;
}

interface Inventory {
  mcpServers: McpServer[];
  agents: Definition[];
  skills: Definition[];
  hooks: { event: string; count: number }[];
  permissions: { defaultMode: string | null; allow: number; ask: number; deny: number };
  model: string | null;
  projects: number;
  tools: { name: string; kind: "app" | "cli"; mcpServers: number }[];
  opportunities: Opportunity[];
}

interface Rating {
  package: string;
  listedAs: string | null;
  tier: string | null;
  score: number | null;
}

interface TrustView {
  enabled: boolean;
  fetchedAt: number | null;
  listed: number;
  ratings: Rating[];
  error: string | null;
}

/// Config lives in main.ts; the Inventory tab only needs this one switch.
export interface InventoryHost {
  trustLookup(): boolean;
  setTrustLookup(on: boolean): Promise<void>;
}

type View = "usage" | "inventory" | "ledger";
type KindFilter = "all" | "tighten" | "learn";

const ALL_SCOPES = "__all__";
const USER_SCOPE = "__user__";

let inventory: Inventory | null = null;
let loadError = "";
let running: RunningServer[] = [];
let runningError = "";
let signIns: Diagnosis[] = [];
let ending = "";
let scopeFilter = ALL_SCOPES;
let kindFilter: KindFilter = "all";
const ALL_APPS = "__all__";
let appFilter = ALL_APPS;
let host: InventoryHost | null = null;
let trust: TrustView | null = null;
/** The pin being previewed, keyed by "client/name", and what happened to it. */
let pin: { key: string; plan: PinPlan | null; note: string; done: boolean } | null = null;
const openSections = new Set<string>(["opportunities", "running", "mcp"]);

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

/// "/Users/me/work/acme" → "acme": the folder is what people recognise.
function projectLabel(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function inScope(item: { scope: string; project: string | null }): boolean {
  if (scopeFilter === ALL_SCOPES) return true;
  if (scopeFilter === USER_SCOPE) return item.scope === "user";
  return item.project === scopeFilter;
}

function scopeChip(item: { scope: string; project: string | null }): string {
  const label = item.scope === "user" ? T("scope.userChip") : projectLabel(item.project ?? "project");
  const tip = item.scope === "user" ? T("scope.userTip") : (item.project ?? "");
  return `<span class="inv-chip" title="${esc(tip)}">${esc(label)}</span>`;
}

/// `lead` always renders above the rows (a filter, say). `hint` replaces the
/// rows when there are none, unless `keepBody` says the rows stand on their
/// own (Guardrails lists its settings even when every count is zero).
function section(
  id: string,
  title: string,
  count: number,
  body: string,
  hint: string,
  opts: { lead?: string; keepBody?: boolean } = {},
): string {
  const open = openSections.has(id);
  const rows = count === 0 && !opts.keepBody ? `<p class="inv-empty">${esc(hint)}</p>` : body;
  return `
    <article class="provider inv-section" data-section="${id}">
      <button class="inv-head" data-toggle="${id}" aria-expanded="${open}">
        <span class="provider-name">${esc(title)}</span>
        <span class="plan">${count}</span>
        <span class="spacer"></span>
        <span class="inv-caret">${open ? "▾" : "▸"}</span>
      </button>
      ${open ? `<div class="card-panel">${opts.lead ?? ""}${rows}</div>` : ""}
    </article>`;
}

function renderOpportunities(list: Opportunity[]): string {
  const shown = list.filter((o) => kindFilter === "all" || o.kind === kindFilter);
  const body = shown
    .map(
      (o) => `
      <div class="inv-opp inv-opp-${o.kind}">
        <div class="inv-opp-title"><span class="inv-dot"></span>${esc(o.title)}</div>
        <p class="inv-opp-detail">${esc(o.detail)}</p>
        ${o.learnUrl ? `<button class="inv-learn" data-link="${esc(o.learnUrl)}">${esc(T("learnMore"))}</button>` : ""}
      </div>`,
    )
    .join("");
  const filter = `
    <label class="inv-filter">${esc(T("filter.show"))}
      <select id="inv-kind">
        <option value="all"${kindFilter === "all" ? " selected" : ""}>${esc(T("filter.everything", { n: list.length }))}</option>
        <option value="tighten"${kindFilter === "tighten" ? " selected" : ""}>${esc(T("filter.tighten", { n: list.filter((o) => o.kind === "tighten").length }))}</option>
        <option value="learn"${kindFilter === "learn" ? " selected" : ""}>${esc(T("filter.learn", { n: list.filter((o) => o.kind === "learn").length }))}</option>
      </select>
    </label>`;
  return section(
    "opportunities",
    T("section.opportunities"),
    shown.length,
    body,
    list.length === 0 ? T("empty.opportunitiesNone") : T("empty.opportunitiesFiltered"),
    { lead: list.length ? filter : "" },
  );
}

/// The tier vocabulary the MCP Trust Index publishes. Falls back to the raw
/// tier string for a value this build does not know yet.
function tierLabel(tier: string): string {
  if (tier === "enterprise-verified") return T("trust.enterpriseVerified");
  if (tier === "recommended") return T("trust.recommended");
  if (tier === "emerging") return T("trust.emerging");
  return tier;
}

function bareName(spec: string): string {
  const at = spec.startsWith("@") ? spec.indexOf("@", 1) : spec.indexOf("@");
  return (at === -1 ? spec : spec.slice(0, at)).toLowerCase();
}

/// The rating chip for a server, when ratings are on. Only packages can be
/// matched: a remote server or a local binary has no package to look up.
function trustChip(s: McpServer): string {
  if (!trust?.enabled || !trust.ratings.length || !s.package) return "";
  const r = trust.ratings.find((x) => bareName(x.package) === bareName(s.package!));
  if (!r) return "";
  if (!r.tier) return `<span class="inv-chip inv-trust-none" title="${esc(T("trust.notRatedTip"))}">${esc(T("trust.notRated"))}</span>`;
  const tier = tierLabel(r.tier);
  return `<span class="inv-chip inv-trust" title="${esc(T("trust.tooltip", { listedAs: r.listedAs ?? "", tier, score: r.score ?? "?" }))}">${esc(tier)} ${r.score ?? ""}</span>`;
}

async function loadTrust(): Promise<void> {
  if (!inventory || !host?.trustLookup()) {
    trust = null;
    return;
  }
  const packages = inventory.mcpServers.map((s) => s.package).filter((p): p is string => !!p);
  try {
    trust = await invoke<TrustView>("get_trust", { packages });
  } catch {
    trust = null;
  }
  render();
}

function pinKey(s: McpServer): string {
  return `${s.client}/${s.name}`;
}

function pinPanel(s: McpServer): string {
  if (pin?.key !== pinKey(s)) return "";
  if (pin.done) return `<div class="inv-pin"><p class="inv-pin-ok">${esc(pin.note)}</p></div>`;
  if (!pin.plan) return `<div class="inv-pin"><p class="dt-caption">${esc(pin.note || T("pin.workingOut"))}</p></div>`;
  const p = pin.plan;
  return `<div class="inv-pin">
      <p class="dt-caption">${esc(plural("inventory.pin.inFile", p.occurrences, { file: p.file }))}</p>
      <pre class="inv-diff"><span class="inv-del">- "${esc(p.from)}"</span>\n<span class="inv-add">+ "${esc(p.to)}"</span></pre>
      <p class="dt-caption">${esc(T("pin.explain", { version: p.installedVersion }))}</p>
      ${pin.note ? `<p class="lg-error" role="alert">${esc(pin.note)}</p>` : ""}
      <div class="dt-rule-actions"><span class="spacer"></span>
        <button class="inv-learn" data-pin-cancel>${esc(t("dialog.cancel"))}</button>
        <button class="lg-save" data-pin-apply>${esc(T("pin.apply"))}</button>
      </div>
    </div>`;
}

function renderMcp(list: McpServer[]): string {
  const rows = list
    .map((s) => {
      const what = s.package ?? s.target;
      const facts = [
        s.transport === "stdio" ? T("mcp.runsLocally") : T("mcp.remote", { transport: s.transport }),
        s.envCount > 0 ? plural("inventory.mcp.credentials", s.envCount) : "",
      ].filter(Boolean);
      return `
        <div class="inv-row">
          <div class="inv-row-main">
            <span class="inv-name">${esc(s.name)}</span>
            <span class="inv-sub" title="${esc(what)}">${esc(what)}</span>
          </div>
          <div class="inv-row-meta">${facts.map((f) => `<span class="inv-fact">${esc(f)}</span>`).join("")}${s.pinTo && pin?.key !== pinKey(s) ? `<button class="inv-chip inv-pin-btn" data-pin="${esc(pinKey(s))}" title="${esc(T("pin.buttonTip", { version: s.pinTo }))}">${esc(T("pin.button"))}</button>` : ""}${trustChip(s)}${s.client === "Claude Code" ? scopeChip(s) : `<span class="inv-chip" title="${esc(T("mcp.loadedBy", { client: s.client }))}">${esc(s.client)}</span>`}</div>
        </div>${pinPanel(s)}`;
    })
    .join("");
  const apps = [...new Set((inventory?.mcpServers ?? []).map((s) => s.client))];
  const on = host?.trustLookup() === true;
  const status = !on
    ? T("trust.offNote")
    : trust?.error
      ? T("trust.fetchError", { error: trust.error })
      : trust?.fetchedAt
        ? T("trust.status", {
            listed: trust.listed,
            date: new Date(trust.fetchedAt).toLocaleDateString(localeTag(), { month: "short", day: "numeric" }),
          })
        : t("detail.loading");
  const trustLead = `<label class="inv-filter">${esc(T("trust.label"))}
      <select id="inv-trust">
        <option value="off"${on ? "" : " selected"}>${esc(t("settings.alertOff"))}</option>
        <option value="on"${on ? " selected" : ""}>${esc(T("trust.on"))}</option>
      </select>
      <button class="lg-link" data-link="https://staas.fund/mcp/">${esc(T("trust.aboutIndex"))}</button>
    </label><p class="dt-caption inv-trust-note">${esc(status)}</p>`;
  const lead =
    trustLead +
    (apps.length > 1
      ? `<label class="inv-filter">${esc(T("mcp.appLabel"))}
          <select id="inv-app">
            <option value="${ALL_APPS}"${appFilter === ALL_APPS ? " selected" : ""}>${esc(T("mcp.allApps"))}</option>
            ${apps.map((a) => `<option value="${esc(a)}"${appFilter === a ? " selected" : ""}>${esc(a)}</option>`).join("")}
          </select>
        </label>`
      : "");
  return section("mcp", T("section.mcp"), list.length, rows, T("empty.mcp"), { lead });
}

/// MB/GB stay English: format tokens, not prose, same as detail.ts's fileSize()
/// MB/KB and money()'s $.
function mbLabel(bytes: number): string {
  const mb = bytes / 1048576;
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;
}

/** "3d 4h", "2h 10m", "6m". Uptime, not time spent working. */
function upLabel(secs: number): string {
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d) return t("time.daysHours", { d, h });
  if (h) return t("time.hoursMins", { h, m });
  return t("time.mins", { m: Math.max(1, m) });
}

/// The Task Manager view: what is in memory right now, heaviest first. The
/// backend matches processes to configured servers and never hands over a
/// command line, so there is nothing here to redact.
function renderRunning(): string {
  const total = running.reduce((sum, r) => sum + r.rssBytes, 0);
  const procCount = running.reduce((sum, r) => sum + r.pids.length, 0);
  const bar = total > 0 ? running.map((r) => r.rssBytes / total) : [];
  const rows = running
    .map((r, i) => {
      const copies = r.instances > 1 ? `<span class="inv-chip run-dupe" title="${esc(T("running.copiesTip"))}">&times;${r.instances}</span>` : "";
      const end = ending === r.name
        ? `<span class="run-confirm">${esc(T("running.endConfirm", { name: r.name }))} <button class="mini-btn run-yes" data-end-yes="${esc(r.name)}">${esc(T("running.endTask"))}</button><button class="mini-btn" data-end-no="1">${esc(t("dialog.cancel"))}</button></span>`
        : `<button class="mini-btn run-end" data-end="${esc(r.name)}" title="${esc(T("running.endTip"))}">${esc(T("running.endTask"))}</button>`;
      const where = r.configured
        ? esc(r.client ?? "")
        : `<span class="inv-chip run-unknown" title="${esc(T("running.notInConfigTip"))}">${esc(T("running.notInConfig"))}</span>`;
      const processes = plural("inventory.running.processCount", r.pids.length);
      // `where` may itself carry a <span> chip's markup (the "not in a config" case),
      // so this line is built from already-escaped pieces and not esc()-wrapped again.
      const rowSub = T("running.rowSub", { where, time: upLabel(r.elapsedSecs), processes });
      return `
      <div class="inv-row run-row">
        <div class="inv-row-main">
          <span class="inv-name">${esc(r.name)}</span>
          ${copies}
          <span class="spacer"></span>
          <span class="run-mem">${mbLabel(r.rssBytes)}</span>
        </div>
        <div class="run-meter"><i style="--w:${(bar[i] * 100).toFixed(1)}%"></i></div>
        <div class="inv-row-sub">${rowSub}</div>
        <div class="run-actions">${end}</div>
      </div>`;
    })
    .join("");
  const lead = running.length
    ? `<p class="inv-note run-lead">${esc(T("running.summary", { mem: mbLabel(total), processes: plural("inventory.running.processCount", procCount) }))}</p>`
    : "";
  return section(
    "running",
    T("section.running"),
    running.length,
    rows,
    runningError ? T("empty.runningError", { error: runningError }) : T("empty.running"),
    { lead },
  );
}

/// Why a card is empty. Says where the app looked, so "not signed in" and
/// "we looked in the wrong place" stop looking the same.
function renderSignIns(): string {
  const rows = signIns
    .map((d) => {
      const probes = d.probes
        .map((p) => {
          const mark = p.found === null ? T("signins.notOpened") : p.found ? T("signins.found") : T("signins.missing");
          const cls = p.found === null ? "sig-unknown" : p.found ? "sig-found" : "sig-missing";
          return `<div class="sig-probe"><span class="inv-chip ${cls}">${esc(mark)}</span><code>${esc(p.location)}</code></div>`;
        })
        .join("");
      const unverified = d.verifiedHere
        ? ""
        : `<span class="inv-chip sig-unverified" title="${esc(T("signins.unverifiedTip"))}">${esc(T("signins.unverified"))}</span>`;
      const hint = d.probes.length && d.probes.every((p) => p.found === false) ? `<p class="sig-hint">${esc(d.hint)}</p>` : "";
      return `
      <div class="inv-row sig-row">
        <div class="inv-row-main">
          <span class="inv-name">${esc(d.name)}</span>
          ${unverified}
        </div>
        ${probes}
        ${hint}
      </div>`;
    })
    .join("");
  return section(
    "signins",
    T("section.signins"),
    signIns.length,
    rows,
    T("empty.signins"),
    { lead: `<p class="inv-note">${esc(T("signins.note"))}</p>` },
  );
}

function renderTools(inv: Inventory): string {
  const rows = inv.tools
    .map(
      (tool) => `
      <div class="inv-row">
        <div class="inv-row-main"><span class="inv-name">${esc(tool.name)}</span></div>
        <div class="inv-row-meta">
          ${tool.mcpServers ? `<span class="inv-fact">${esc(plural("inventory.tools.mcpServerCount", tool.mcpServers))}</span>` : ""}
          <span class="inv-chip" title="${esc(tool.kind === "app" ? T("tools.viaApp") : T("tools.viaCommand"))}">${esc(tool.kind === "app" ? T("tools.chipApp") : T("tools.chipCommand"))}</span>
        </div>
      </div>`,
    )
    .join("");
  return section("tools", T("section.tools"), inv.tools.length, rows, T("empty.tools"));
}

function defRows(list: Definition[]): string {
  return list
    .map(
      (d) => `
      <div class="inv-row">
        <div class="inv-row-main"><span class="inv-name">${esc(d.name)}</span></div>
        <div class="inv-row-meta">${d.model ? `<span class="inv-fact">${esc(d.model)}</span>` : ""}${scopeChip(d)}</div>
      </div>`,
    )
    .join("");
}

/// Agents, skills and guardrails are all "how this machine is configured",
/// they rarely change, and each was its own accordion. Eight collapsible
/// sections is a wall; these three are one, with sub-headings inside.
function renderSetup(inv: Inventory, agents: Definition[], skills: Definition[]): string {
  const p = inv.permissions;
  const guardrails = [
    [T("setup.defaultModel"), inv.model ?? T("setup.modelNotPinned")],
    [T("setup.permissionMode"), p.defaultMode ?? T("setup.modeDefault")],
    [T("setup.allowRules"), String(p.allow)],
    [T("setup.askRules"), String(p.ask)],
    [T("setup.denyRules"), String(p.deny)],
    ...inv.hooks.map((h) => [T("setup.hookLabel", { event: h.event }), String(h.count)]),
  ]
    .map(
      ([k, v]) => `
      <div class="inv-row">
        <div class="inv-row-main"><span class="inv-name">${esc(k)}</span></div>
        <div class="inv-row-meta"><span class="inv-fact">${esc(v)}</span></div>
      </div>`,
    )
    .join("");
  const defs = (title: string, list: Definition[], hint: string) =>
    `<div class="inv-grouphead">${esc(title)} <span class="inv-grouphead-n">${list.length}</span></div>` +
    (list.length ? defRows(list) : `<p class="inv-empty">${esc(hint)}</p>`);
  const body =
    defs(T("setup.agents"), agents, T("empty.agents")) +
    defs(T("setup.skills"), skills, T("empty.skills")) +
    `<div class="inv-grouphead">${esc(T("setup.guardrails"))}</div>${guardrails}`;
  const count = agents.length + skills.length + p.allow + p.ask + p.deny + inv.hooks.length;
  return section("setup", T("section.setup"), count, body, "", { keepBody: true });
}

function scopeOptions(inv: Inventory): string {
  const projects = new Set<string>();
  for (const item of [...inv.mcpServers, ...inv.agents, ...inv.skills]) {
    if (item.project) projects.add(item.project);
  }
  const opt = (value: string, label: string) =>
    `<option value="${esc(value)}"${scopeFilter === value ? " selected" : ""}>${esc(label)}</option>`;
  return (
    opt(ALL_SCOPES, T("scope.all")) +
    opt(USER_SCOPE, T("scope.user")) +
    [...projects].sort().map((p) => opt(p, T("scope.project", { project: projectLabel(p) }))).join("")
  );
}

function render(): void {
  const el = document.querySelector<HTMLElement>("#inventory");
  if (!el) return;
  if (loadError) {
    el.innerHTML = `<article class="provider"><div class="card-panel"><p class="inv-empty">${esc(T("loadError", { error: loadError }))}</p></div></article>`;
    return;
  }
  if (!inventory) {
    el.innerHTML = `<div class="skeleton-card"><div class="skeleton-line title"></div><div class="skeleton-line bar"></div></div>`;
    return;
  }
  const inv = inventory;
  if (appFilter !== ALL_APPS && !inv.mcpServers.some((s) => s.client === appFilter)) appFilter = ALL_APPS;
  const mcp = inv.mcpServers.filter(inScope).filter((s) => appFilter === ALL_APPS || s.client === appFilter);
  const agents = inv.agents.filter(inScope);
  const skills = inv.skills.filter(inScope);
  const scanned = plural("inventory.projectsScanned", inv.projects);
  el.innerHTML = `
    <div class="inv-toolbar">
      <label class="inv-filter">${esc(T("scope.label"))}
        <select id="inv-scope">${scopeOptions(inv)}</select>
      </label>
      <span class="lg-toolbar">
        <button class="inv-rescan" id="audit-open-btn" title="${esc(T("audit.tip"))}">${esc(T("audit.button"))}</button>
        <button class="inv-rescan" id="inv-rescan" title="${esc(T("rescan.tip"))}">${esc(T("rescan.button"))}</button>
      </span>
    </div>
    <p class="inv-note">${esc(T("note", { scanned }))}</p>
    ${renderOpportunities(inv.opportunities)}
    ${renderRunning()}
    ${renderSignIns()}
    ${renderTools(inv)}
    ${renderMcp(mcp)}
    ${renderSetup(inv, agents, skills)}`;
}

/// Cheap next to a full scan, so it refreshes on its own whenever the view is
/// open: a memory figure that is ten minutes old is worse than none.
async function loadRunning(): Promise<void> {
  try {
    running = await invoke<RunningServer[]>("get_running");
    runningError = "";
  } catch (err) {
    running = [];
    runningError = String(err);
  }
  try {
    signIns = await invoke<Diagnosis[]>("get_diagnosis");
  } catch {
    signIns = [];
  }
}

async function load(): Promise<void> {
  try {
    inventory = await invoke<Inventory>("get_inventory");
    loadError = "";
    // A project that disappeared since the last scan cannot stay selected.
    const known = new Set(
      [...inventory.mcpServers, ...inventory.agents, ...inventory.skills].map((i) => i.project),
    );
    if (scopeFilter !== ALL_SCOPES && scopeFilter !== USER_SCOPE && !known.has(scopeFilter)) {
      scopeFilter = ALL_SCOPES;
    }
  } catch (err) {
    loadError = String(err);
  }
  render();
  void loadTrust();
  void loadRunning().then(render);
}

function show(view: View): void {
  const sections: [View, string][] = [["usage", "#providers"], ["inventory", "#inventory"], ["ledger", "#ledger"]];
  for (const [name, selector] of sections) {
    const el = document.querySelector<HTMLElement>(selector);
    if (el) el.hidden = view !== name;
  }
  document.querySelectorAll<HTMLElement>("#view-tabs .tab").forEach((b) => {
    const active = b.dataset.view === view;
    b.classList.toggle("active", active);
    b.setAttribute("aria-selected", String(active));
  });
  if (view === "inventory") {
    render();
    void load();
  }
  if (view === "ledger") showLedger();
}

/// Wires the Usage / Inventory switch. Usage is the default view every time
/// the popover opens; Inventory loads on first visit and on Rescan.
/// Switches the main view from elsewhere (the audit's "Open …" links).
export function showView(view: "usage" | "inventory" | "ledger"): void {
  show(view);
}

/// Redraws the Inventory tab in place, e.g. after a locale switch (task 7
/// wires this into the locale-change handler). A no-op while another view is
/// showing, or before the first load has produced anything to redraw.
export function rerender(): void {
  const el = document.querySelector<HTMLElement>("#inventory");
  if (el && !el.hidden) render();
}

export function setupViews(h: InventoryHost): void {
  host = h;
  document.querySelector("#view-tabs")?.addEventListener("click", (e) => {
    const tab = (e.target as HTMLElement).closest<HTMLElement>("[data-view]");
    if (tab) show(tab.dataset.view as View);
  });

  const el = document.querySelector<HTMLElement>("#inventory");
  if (!el) return;
  el.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const link = target.closest<HTMLElement>("[data-link]");
    if (link) {
      void invoke("open_link", { url: link.dataset.link }).catch(() => {});
      return;
    }
    // End task: two steps, always. The first click only asks.
    const endBtn = target.closest<HTMLElement>("[data-end]");
    if (endBtn) {
      ending = endBtn.dataset.end ?? "";
      render();
      return;
    }
    if (target.closest("[data-end-no]")) {
      ending = "";
      render();
      return;
    }
    const endYes = target.closest<HTMLElement>("[data-end-yes]");
    if (endYes) {
      const name = endYes.dataset.endYes ?? "";
      ending = "";
      runningError = "";
      void invoke<number>("end_task", { name })
        .catch((err) => {
          runningError = String(err);
        })
        .then(() => loadRunning())
        .then(render);
      return;
    }
    const pinBtn = target.closest<HTMLElement>("[data-pin]");
    if (pinBtn) {
      const server = inventory?.mcpServers.find((s) => pinKey(s) === pinBtn.dataset.pin);
      if (!server) return;
      const key = pinKey(server);
      pin = { key, plan: null, note: "", done: false };
      render();
      void invoke<PinPlan>("pin_preview", { name: server.name, client: server.client }).then(
        (plan) => { if (pin?.key === key) { pin.plan = plan; render(); } },
        (err) => { if (pin?.key === key) { pin.note = String(err); render(); } },
      );
      return;
    }
    if (target.closest("[data-pin-cancel]")) {
      pin = null;
      render();
      return;
    }
    if (target.closest("[data-pin-apply]") && pin?.plan) {
      const current = pin;
      const server = inventory?.mcpServers.find((s) => pinKey(s) === current.key);
      if (!server) return;
      const p = current.plan!;
      void invoke<string>("pin_apply", {
        name: server.name,
        client: server.client,
        seen: { file: p.file, to: p.to, fileLen: p.fileLen, fileMtimeMs: p.fileMtimeMs },
      }).then(
        (backup) => {
          current.done = true;
          current.note = T("pin.doneNote", { to: p.to, backup });
          render();
          void load(); // the finding should now be gone
        },
        (err) => { current.note = String(err); render(); },
      );
      return;
    }
    const toggle = target.closest<HTMLElement>("[data-toggle]");
    if (toggle) {
      const id = toggle.dataset.toggle!;
      if (!openSections.delete(id)) openSections.add(id);
      render();
      return;
    }
    if (target.closest("#inv-rescan")) void load();
  });
  el.addEventListener("change", (e) => {
    const target = e.target as HTMLSelectElement;
    if (target.id === "inv-trust") {
      const on = target.value === "on";
      void host?.setTrustLookup(on).then(() => {
        if (!on) trust = null;
        render();
        return loadTrust();
      });
      return;
    }
    if (target.id === "inv-scope") scopeFilter = target.value;
    else if (target.id === "inv-app") appFilter = target.value;
    else if (target.id === "inv-kind") kindFilter = target.value as KindFilter;
    else return;
    render();
  });
}
