// Inventory tab: what AI tooling is wired up on this machine, and what that
// setup suggests learning or tightening next. The Rust side (`inventory.rs`)
// hands over names, shapes and counts only, never a config value.

import { invoke } from "@tauri-apps/api/core";
import { showLedger } from "./ledger";

interface McpServer {
  name: string;
  scope: "user" | "project";
  project: string | null;
  transport: string;
  target: string;
  package: string | null;
  envCount: number;
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
  opportunities: Opportunity[];
}

type View = "usage" | "inventory" | "ledger";
type KindFilter = "all" | "tighten" | "learn";

const ALL_SCOPES = "__all__";
const USER_SCOPE = "__user__";

let inventory: Inventory | null = null;
let loadError = "";
let scopeFilter = ALL_SCOPES;
let kindFilter: KindFilter = "all";
const openSections = new Set<string>(["opportunities", "mcp"]);

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
          <div class="inv-row-meta">${facts.map((f) => `<span class="inv-fact">${esc(f)}</span>`).join("")}${scopeChip(s)}</div>
        </div>`;
    })
    .join("");
  return section("mcp", "MCP servers", list.length, rows, "No MCP servers configured for this scope.");
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
  const mcp = inv.mcpServers.filter(inScope);
  const agents = inv.agents.filter(inScope);
  const skills = inv.skills.filter(inScope);
  el.innerHTML = `
    <div class="inv-toolbar">
      <label class="inv-filter">Scope
        <select id="inv-scope">${scopeOptions(inv)}</select>
      </label>
      <button class="inv-rescan" id="inv-rescan" title="Read the local setup again">Rescan</button>
    </div>
    <p class="inv-note">Read from this machine only. Names and counts, never keys or prompts. ${inv.projects} project${inv.projects === 1 ? "" : "s"} scanned.</p>
    ${renderOpportunities(inv.opportunities)}
    ${renderMcp(mcp)}
    ${renderDefinitions("agents", "Agents", agents, "No custom agents in this scope.")}
    ${renderDefinitions("skills", "Skills", skills, "No skills in this scope.")}
    ${renderGuardrails(inv)}`;
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
export function setupViews(): void {
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
    if (target.id === "inv-scope") scopeFilter = target.value;
    else if (target.id === "inv-kind") kindFilter = target.value as KindFilter;
    else return;
    render();
  });
}
