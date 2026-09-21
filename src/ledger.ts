// Subscriptions tab: what the AI tools cost, when each renews, and whether
// the usage this app measures justifies the price. Entries are the user's own
// numbers; a detected plan is offered by name only, never with a guessed price.

import { invoke } from "@tauri-apps/api/core";

type Cycle = "monthly" | "yearly";

interface Subscription {
  id: string;
  name: string;
  price: number;
  cycle: Cycle;
  renewsOn: string | null;
  provider: string | null;
  notes: string | null;
}

interface ItemView extends Subscription {
  monthlyCost: number;
  nextRenewal: string | null;
  daysLeft: number | null;
  usage30: number | null;
  valueRatio: number | null;
  idle: boolean;
}

interface LedgerView {
  items: ItemView[];
  monthly: number;
  yearly: number;
  idleMonthly: number;
}

export interface LedgerSource {
  /** Card id → API-equivalent spend over the last 30 days. */
  usage30(): Record<string, number>;
  /** Live cards, for the "Pays for" dropdown and the suggestions. */
  tools(): { id: string; name: string; plan: string | null }[];
}

let source: LedgerSource | null = null;
let ledger: LedgerView | null = null;
let loadError = "";
/** The entry in the form: a Subscription being edited, a blank one, or none. */
let editing: Subscription | null = null;
let formError = "";
let confirmDelete = "";

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

function money(n: number): string {
  return n >= 10 ? `$${Math.round(n).toLocaleString()}` : `$${n.toFixed(2)}`;
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function renewalText(item: ItemView): string {
  if (item.daysLeft === null || !item.nextRenewal) return "No renewal date";
  const date = new Date(`${item.nextRenewal}T00:00:00`).toLocaleDateString([], { month: "short", day: "numeric" });
  if (item.daysLeft === 0) return `Renews today · ${date}`;
  if (item.daysLeft === 1) return `Renews tomorrow · ${date}`;
  return `Renews in ${item.daysLeft} days · ${date}`;
}

function valueLine(item: ItemView): string {
  if (item.idle) {
    return `<p class="lg-flag lg-flag-idle">No measured usage in 30 days. Cancelling saves ${money(item.monthlyCost)} a month.</p>`;
  }
  if (item.usage30 === null || item.valueRatio === null) return "";
  const times = item.valueRatio >= 10 ? item.valueRatio.toFixed(0) : item.valueRatio.toFixed(1);
  const verdict =
    item.valueRatio >= 1
      ? `${times}x its price`
      : `${Math.round(item.valueRatio * 100)}% of its price`;
  return `<p class="lg-flag">${money(item.usage30)} of API-equivalent work in 30 days: ${verdict}.</p>`;
}

function blank(): Subscription {
  return { id: "", name: "", price: 0, cycle: "monthly", renewsOn: null, provider: null, notes: null };
}

function form(sub: Subscription): string {
  const tools = source?.tools() ?? [];
  const toolOptions =
    `<option value="">Nothing tracked here</option>` +
    tools
      .map((t) => `<option value="${esc(t.id)}"${sub.provider === t.id ? " selected" : ""}>${esc(t.name)}</option>`)
      .join("");
  return `
    <form class="card-panel lg-form" id="lg-form">
      <label>Name <input id="lg-name" type="text" maxlength="80" value="${esc(sub.name)}" placeholder="Claude Max" required /></label>
      <div class="lg-form-row">
        <label>Price <input id="lg-price" type="number" min="0" step="0.01" value="${sub.price > 0 ? sub.price : ""}" placeholder="0.00" required /></label>
        <label>Billed
          <select id="lg-cycle">
            <option value="monthly"${sub.cycle === "monthly" ? " selected" : ""}>Monthly</option>
            <option value="yearly"${sub.cycle === "yearly" ? " selected" : ""}>Yearly</option>
          </select>
        </label>
      </div>
      <div class="lg-form-row">
        <label>Renews on <input id="lg-date" type="date" value="${esc(sub.renewsOn ?? "")}" /></label>
        <label>Pays for <select id="lg-provider">${toolOptions}</select></label>
      </div>
      <label>Notes <input id="lg-notes" type="text" maxlength="300" value="${esc(sub.notes ?? "")}" placeholder="Optional" /></label>
      ${formError ? `<p class="lg-error" role="alert">${esc(formError)}</p>` : ""}
      <div class="lg-form-actions">
        <button type="button" class="inv-learn" id="lg-cancel">Cancel</button>
        <button type="submit" class="lg-save">${sub.id ? "Save changes" : "Add subscription"}</button>
      </div>
    </form>`;
}

function render(): void {
  const el = document.querySelector<HTMLElement>("#ledger");
  if (!el) return;
  if (loadError) {
    el.innerHTML = `<article class="provider"><div class="card-panel"><p class="inv-empty">Could not open the ledger: ${esc(loadError)}</p></div></article>`;
    return;
  }
  if (!ledger) {
    el.innerHTML = `<div class="skeleton-card"><div class="skeleton-line title"></div><div class="skeleton-line bar"></div></div>`;
    return;
  }
  const lg = ledger;
  const linked = new Set(lg.items.map((i) => i.provider).filter(Boolean));
  const suggestions = (source?.tools() ?? []).filter((t) => !linked.has(t.id));

  const headline = lg.items.length
    ? `<article class="provider"><div class="card-panel lg-total">
        <div class="dt-headline"><b>${money(lg.monthly)}</b><span>a month · ${money(lg.yearly)} a year</span></div>
        ${lg.idleMonthly > 0 ? `<p class="lg-flag lg-flag-idle">${money(lg.idleMonthly)} a month is going to tools with no measured usage.</p>` : ""}
      </div></article>`
    : "";

  const rows = lg.items
    .map((item) =>
      editing?.id === item.id
        ? form(editing)
        : `
      <div class="card-panel lg-item${item.daysLeft !== null && item.daysLeft <= 3 ? " lg-soon" : ""}">
        <div class="lg-item-head">
          <span class="inv-name">${esc(item.name)}</span>
          <span class="lg-price">${money(item.price)}<small> / ${item.cycle === "monthly" ? "mo" : "yr"}</small></span>
        </div>
        <div class="lg-item-sub">
          <span>${esc(renewalText(item))}</span>
          <span class="lg-actions">
            <button class="lg-link" data-edit="${esc(item.id)}">Edit</button>
            <button class="lg-link${confirmDelete === item.id ? " lg-danger" : ""}" data-delete="${esc(item.id)}">${confirmDelete === item.id ? "Really delete?" : "Delete"}</button>
          </span>
        </div>
        ${valueLine(item)}
        ${item.notes ? `<p class="lg-notes">${esc(item.notes)}</p>` : ""}
      </div>`,
    )
    .join("");

  const empty =
    lg.items.length === 0 && !editing
      ? `<article class="provider"><div class="card-panel"><p class="inv-empty">Nothing here yet. Add what you pay for AI tools to see the monthly total, renewal dates, and whether each plan earns its price.</p></div></article>`
      : "";

  const suggest =
    suggestions.length && !editing
      ? `<div class="lg-suggest"><span>Found on this computer:</span>${suggestions
          .map((t) => `<button class="inv-chip lg-chip" data-suggest="${esc(t.id)}">+ ${esc(t.name)}${t.plan ? ` ${esc(cap(t.plan))}` : ""}</button>`)
          .join("")}</div>`
      : "";

  el.innerHTML = `
    <div class="inv-toolbar">
      <p class="inv-note">Your own numbers, kept on this computer. Prices are never guessed.</p>
      ${editing ? "" : `<button class="inv-rescan" id="lg-add">Add</button>`}
    </div>
    ${headline}
    ${editing && !editing.id ? form(editing) : ""}
    ${empty}
    ${rows ? `<article class="provider lg-list">${rows}</article>` : ""}
    ${suggest}`;
  if (editing) el.querySelector<HTMLInputElement>("#lg-name")?.focus();
}

async function load(): Promise<void> {
  try {
    ledger = await invoke<LedgerView>("get_ledger", { usage30: source?.usage30() ?? {} });
    loadError = "";
  } catch (err) {
    loadError = String(err);
  }
  render();
}

async function save(): Promise<void> {
  if (!editing) return;
  const get = (id: string) => (document.querySelector<HTMLInputElement | HTMLSelectElement>(id)?.value ?? "").trim();
  const subscription: Subscription = {
    ...editing,
    name: get("#lg-name"),
    price: Number(get("#lg-price")),
    cycle: get("#lg-cycle") as Cycle,
    renewsOn: get("#lg-date") || null,
    provider: get("#lg-provider") || null,
    notes: get("#lg-notes") || null,
  };
  try {
    await invoke("save_subscription", { subscription });
    editing = null;
    formError = "";
    await load();
  } catch (err) {
    // Keep what was typed: a refused save must not empty the form.
    editing = subscription;
    formError = String(err);
    render();
  }
}

/// Called when the Subscriptions tab is shown.
export function showLedger(): void {
  render();
  void load();
}

export function setupLedger(src: LedgerSource): void {
  source = src;
  const el = document.querySelector<HTMLElement>("#ledger");
  if (!el) return;
  el.addEventListener("submit", (e) => {
    e.preventDefault();
    void save();
  });
  el.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    const pick = (attr: string) => target.closest<HTMLElement>(`[data-${attr}]`)?.dataset[attr];
    const suggestId = pick("suggest");
    const editId = pick("edit");
    const deleteId = pick("delete");
    if (target.closest("#lg-add")) {
      editing = blank();
    } else if (target.closest("#lg-cancel")) {
      editing = null;
    } else if (suggestId) {
      const tool = source?.tools().find((t) => t.id === suggestId);
      editing = { ...blank(), name: tool ? `${tool.name}${tool.plan ? ` ${cap(tool.plan)}` : ""}` : "", provider: suggestId };
    } else if (editId) {
      const item = ledger?.items.find((i) => i.id === editId);
      if (item) {
        editing = {
          id: item.id, name: item.name, price: item.price, cycle: item.cycle,
          renewsOn: item.renewsOn, provider: item.provider, notes: item.notes,
        };
      }
    } else if (deleteId) {
      if (confirmDelete !== deleteId) {
        confirmDelete = deleteId; // first click arms, second deletes
        render();
        return;
      }
      confirmDelete = "";
      void invoke("delete_subscription", { id: deleteId }).then(load, load);
      return;
    } else {
      return;
    }
    formError = "";
    confirmDelete = "";
    render();
  });
}
