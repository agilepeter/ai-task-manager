// Inventory tab: what AI tooling is wired up on this machine, and what that
// setup suggests learning or tightening next. The Rust side (`inventory.rs`)
// hands over names, shapes and counts only, never a config value.

import { invoke } from "@tauri-apps/api/core";
import { showLedger } from "./ledger";

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
  const label = item.scope === "user" ? "user" : projectLabel(item.project ?? "project");
  const tip = item.scope === "user" ? "Available in every project" : (item.project ?? "");
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
        ${o.learnUrl ? `<button class="inv-learn" data-link="${esc(o.learnUrl)}">Learn more ↗</button>` : ""}
      </div>`,
    )
    .join("");
  const filter = `
    <label class="inv-filter">Show
      <select id="inv-kind">
        <option value="all"${kindFilter === "all" ? " selected" : ""}>Everything (${list.length})</option>
        <option value="tighten"${kindFilter === "tighten" ? " selected" : ""}>Gaps to tighten (${list.filter((o) => o.kind === "tighten").length})</option>
        <option value="learn"${kindFilter === "learn" ? " selected" : ""}>Things to learn (${list.filter((o) => o.kind === "learn").length})</option>
      </select>
    </label>`;
  return section(
    "opportunities",
    "Opportunities",
    shown.length,
    body,
    list.length === 0
      ? "Nothing to flag. This setup is pinned, scoped and guarded."
      : "Nothing in this category.",
    { lead: list.length ? filter : "" },
  );
}

const TIER_LABEL: Record<string, string> = {
  "enterprise-verified": "Enterprise Verified",
  recommended: "Recommended",
  emerging: "Emerging",
};

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
  if (!r.tier) return `<span class="inv-chip inv-trust-none" title="Not on the MCP Trust Index. That is not a verdict: most servers have not been reviewed.">Not rated</span>`;
  return `<span class="inv-chip inv-trust" title="MCP Trust Index: ${esc(r.listedAs ?? "")}, ${esc(TIER_LABEL[r.tier] ?? r.tier)}, score ${r.score ?? "?"} of 100">${esc(TIER_LABEL[r.tier] ?? r.tier)} ${r.score ?? ""}</span>`;
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
  if (!pin.plan) return `<div class="inv-pin"><p class="dt-caption">${esc(pin.note || "Working out the change…")}</p></div>`;
  const p = pin.plan;
  return `<div class="inv-pin">
      <p class="dt-caption">In ${esc(p.file)}${p.occurrences > 1 ? `, ${p.occurrences} places` : ""}:</p>
      <pre class="inv-diff"><span class="inv-del">- "${esc(p.from)}"</span>\n<span class="inv-add">+ "${esc(p.to)}"</span></pre>
      <p class="dt-caption">${esc(p.installedVersion)} is what already runs here, read from the local package cache. Nothing else in the file changes, a backup is saved beside it, and the app that uses this server picks the pin up when it restarts.</p>
      ${pin.note ? `<p class="lg-error" role="alert">${esc(pin.note)}</p>` : ""}
      <div class="dt-rule-actions"><span class="spacer"></span>
        <button class="inv-learn" data-pin-cancel>Cancel</button>
        <button class="lg-save" data-pin-apply>Apply</button>
      </div>
    </div>`;
}

function renderMcp(list: McpServer[]): string {
  const rows = list
    .map((s) => {
      const what = s.package ?? s.target;
      const facts = [
        s.transport === "stdio" ? "runs locally" : `remote · ${s.transport}`,
        s.envCount > 0 ? `${s.envCount} credential${s.envCount === 1 ? "" : "s"}` : "",
      ].filter(Boolean);
      return `
        <div class="inv-row">
          <div class="inv-row-main">
            <span class="inv-name">${esc(s.name)}</span>
            <span class="inv-sub" title="${esc(what)}">${esc(what)}</span>
          </div>
          <div class="inv-row-meta">${facts.map((f) => `<span class="inv-fact">${esc(f)}</span>`).join("")}${s.pinTo && pin?.key !== pinKey(s) ? `<button class="inv-chip inv-pin-btn" data-pin="${esc(pinKey(s))}" title="Pin to ${esc(s.pinTo)}. Shows the exact change first.">Pin…</button>` : ""}${trustChip(s)}${s.client === "Claude Code" ? scopeChip(s) : `<span class="inv-chip" title="Loaded by ${esc(s.client)}">${esc(s.client)}</span>`}</div>
        </div>${pinPanel(s)}`;
    })
    .join("");
  const apps = [...new Set((inventory?.mcpServers ?? []).map((s) => s.client))];
  const on = host?.trustLookup() === true;
  const status = !on
    ? "Off. Turning it on downloads the public list from staas.fund once a day and matches it on this computer. Nothing about your setup is sent."
    : trust?.error
      ? `Could not fetch the list: ${trust.error}`
      : trust?.fetchedAt
        ? `${trust.listed} servers listed · updated ${new Date(trust.fetchedAt).toLocaleDateString([], { month: "short", day: "numeric" })} · matched on this computer`
        : "Loading…";
  const trustLead = `<label class="inv-filter">Trust ratings
      <select id="inv-trust">
        <option value="off"${on ? "" : " selected"}>Off</option>
        <option value="on"${on ? " selected" : ""}>On</option>
      </select>
      <button class="lg-link" data-link="https://staas.fund/mcp/">About the index</button>
    </label><p class="dt-caption inv-trust-note">${esc(status)}</p>`;
  const lead =
    trustLead +
    (apps.length > 1
      ? `<label class="inv-filter">App
          <select id="inv-app">
            <option value="${ALL_APPS}"${appFilter === ALL_APPS ? " selected" : ""}>All apps</option>
            ${apps.map((a) => `<option value="${esc(a)}"${appFilter === a ? " selected" : ""}>${esc(a)}</option>`).join("")}
          </select>
        </label>`
      : "");
  return section("mcp", "MCP servers", list.length, rows, "No MCP servers configured for this scope.", { lead });
}

function mbLabel(bytes: number): string {
  const mb = bytes / 1048576;
  return mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${Math.round(mb)} MB`;
}

/** "3d 4h", "2h 10m", "6m". Uptime, not time spent working. */
function upLabel(secs: number): string {
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  return `${Math.max(1, m)}m`;
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
      const copies = r.instances > 1 ? `<span class="inv-chip run-dupe" title="Each client app that has this server configured starts its own copy.">&times;${r.instances}</span>` : "";
      const where = r.configured ? esc(r.client ?? "") : `<span class="inv-chip run-unknown" title="Running, but no config file this app can read declares it.">not in a config</span>`;
      return `
      <div class="inv-row run-row">
        <div class="inv-row-main">
          <span class="inv-name">${esc(r.name)}</span>
          ${copies}
          <span class="spacer"></span>
          <span class="run-mem">${mbLabel(r.rssBytes)}</span>
        </div>
        <div class="run-meter"><i style="--w:${(bar[i] * 100).toFixed(1)}%"></i></div>
        <div class="inv-row-sub">${where} &middot; up ${upLabel(r.elapsedSecs)} &middot; ${r.pids.length} process${r.pids.length === 1 ? "" : "es"}</div>
      </div>`;
    })
    .join("");
  const lead = running.length
    ? `<p class="inv-note run-lead">${mbLabel(total)} of memory across ${procCount} process${procCount === 1 ? "" : "es"}. Servers start when a client asks for one and stay for the session.</p>`
    : "";
  return section(
    "running",
    "Running now",
    running.length,
    rows,
    runningError
      ? `Could not read the process list: ${runningError}`
      : "No MCP servers are running. They start when a client asks for one.",
    { lead },
  );
}

function renderTools(inv: Inventory): string {
  const rows = inv.tools
    .map(
      (t) => `
      <div class="inv-row">
        <div class="inv-row-main"><span class="inv-name">${esc(t.name)}</span></div>
        <div class="inv-row-meta">
          ${t.mcpServers ? `<span class="inv-fact">${t.mcpServers} MCP server${t.mcpServers === 1 ? "" : "s"}</span>` : ""}
          <span class="inv-chip" title="${t.kind === "app" ? "Its settings folder exists" : "Its command is on the PATH"}">${t.kind === "app" ? "set up" : "command"}</span>
        </div>
      </div>`,
    )
    .join("");
  return section("tools", "AI tools on this computer", inv.tools.length, rows, "None of the AI tools this app knows were found.");
}

function renderDefinitions(id: string, title: string, list: Definition[], hint: string): string {
  const rows = list
    .map(
      (d) => `
      <div class="inv-row">
        <div class="inv-row-main"><span class="inv-name">${esc(d.name)}</span></div>
        <div class="inv-row-meta">${d.model ? `<span class="inv-fact">${esc(d.model)}</span>` : ""}${scopeChip(d)}</div>
      </div>`,
    )
    .join("");
  return section(id, title, list.length, rows, hint);
}

function renderGuardrails(inv: Inventory): string {
  const p = inv.permissions;
  const rows = [
    ["Default model", inv.model ?? "not pinned"],
    ["Permission mode", p.defaultMode ?? "default (asks each time)"],
    ["Allow rules", String(p.allow)],
    ["Ask rules", String(p.ask)],
    ["Deny rules", String(p.deny)],
    ...inv.hooks.map((h) => [`Hook · ${h.event}`, String(h.count)]),
  ]
    .map(
      ([k, v]) => `
      <div class="inv-row">
        <div class="inv-row-main"><span class="inv-name">${esc(k)}</span></div>
        <div class="inv-row-meta"><span class="inv-fact">${esc(v)}</span></div>
      </div>`,
    )
    .join("");
  return section("guardrails", "Guardrails", p.allow + p.ask + p.deny + inv.hooks.length, rows, "", {
    keepBody: true,
  });
}

function scopeOptions(inv: Inventory): string {
  const projects = new Set<string>();
  for (const item of [...inv.mcpServers, ...inv.agents, ...inv.skills]) {
    if (item.project) projects.add(item.project);
  }
  const opt = (value: string, label: string) =>
    `<option value="${esc(value)}"${scopeFilter === value ? " selected" : ""}>${esc(label)}</option>`;
  return (
    opt(ALL_SCOPES, "All scopes") +
    opt(USER_SCOPE, "User (every project)") +
    [...projects].sort().map((p) => opt(p, `Project · ${projectLabel(p)}`)).join("")
  );
}

function render(): void {
  const el = document.querySelector<HTMLElement>("#inventory");
  if (!el) return;
  if (loadError) {
    el.innerHTML = `<article class="provider"><div class="card-panel"><p class="inv-empty">Could not read the local setup: ${esc(loadError)}</p></div></article>`;
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
  el.innerHTML = `
    <div class="inv-toolbar">
      <label class="inv-filter">Scope
        <select id="inv-scope">${scopeOptions(inv)}</select>
      </label>
      <span class="lg-toolbar">
        <button class="inv-rescan" id="audit-open-btn" title="A scored read of this whole setup, which you can export">Audit</button>
        <button class="inv-rescan" id="inv-rescan" title="Read the local setup again">Rescan</button>
      </span>
    </div>
    <p class="inv-note">Read from this machine only. Names and counts, never keys or prompts. ${inv.projects} project${inv.projects === 1 ? "" : "s"} scanned.</p>
    ${renderOpportunities(inv.opportunities)}
    ${renderRunning()}
    ${renderTools(inv)}
    ${renderMcp(mcp)}
    ${renderDefinitions("agents", "Agents", agents, "No custom agents in this scope.")}
    ${renderDefinitions("skills", "Skills", skills, "No skills in this scope.")}
    ${renderGuardrails(inv)}`;
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
          current.note = `Pinned to ${p.to}. The original is saved as ${backup}.`;
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
