// Subscriptions tab: what the AI tools cost, when each renews, and whether
// the usage this app measures justifies the price. Entries are the user's own
// numbers; a detected plan is offered by name only, never with a guessed price.

import { invoke } from "@tauri-apps/api/core";
import { localeTag, plural, t } from "./i18n";

const T = (k: string, v?: Record<string, string | number>) => t(`ledger.${k}`, v);

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
  whatIf: { kind: "plan-wins" | "plan-loses"; apiCost: number; planCost: number; difference: number } | null;
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
let exportNote = "";

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

function money(n: number): string {
  return n >= 10 ? `$${Math.round(n).toLocaleString(localeTag())}` : `$${n.toFixed(2)}`;
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/// The short "/mo" or "/yr" suffix after a price. These are abbreviations of
/// real words ("a month"/"a year"), and this same view already spells those
/// out in the total line and the value-line sentences — leaving this one
/// untranslated would show a user both forms on one screen. Two literal T()
/// calls (not T(cond ? "a" : "b")) so i18n.test.mjs's KEYED_SOURCES check,
/// which greps for a quote immediately after T(, can see both keys.
function cycleSuffix(cycle: Cycle): string {
  return cycle === "monthly" ? T("perMonthShort") : T("perYearShort");
}

function renewalText(item: ItemView): string {
  if (item.daysLeft === null || !item.nextRenewal) return T("renewal.none");
  const date = new Date(`${item.nextRenewal}T00:00:00`).toLocaleDateString(localeTag(), { month: "short", day: "numeric" });
  if (item.daysLeft === 0) return T("renewal.today", { date });
  if (item.daysLeft === 1) return T("renewal.tomorrow", { date });
  // daysLeft is always >= 2 here: 0 and 1 are handled above, so plural()'s
  // .one form (n === 1) is unreachable through this branch.
  return plural("ledger.renewal.inDays", item.daysLeft, { date });
}

function valueLine(item: ItemView): string {
  if (item.idle) {
    return `<p class="lg-flag lg-flag-idle">${esc(T("value.idle", { amount: money(item.monthlyCost) }))}</p>`;
  }
  if (item.whatIf?.kind === "plan-wins") {
    const w = item.whatIf;
    return `<p class="lg-flag">${esc(T("value.planWins", { apiCost: money(w.apiCost), planCost: money(w.planCost), difference: money(w.difference) }))}</p>`;
  }
  if (item.whatIf?.kind === "plan-loses") {
    const w = item.whatIf;
    return `<p class="lg-flag lg-flag-idle">${esc(T("value.planLoses", { apiCost: money(w.apiCost), planCost: money(w.planCost), difference: money(w.difference) }))}</p>`;
  }
  if (item.usage30 === null || item.valueRatio === null) return "";
  const times = item.valueRatio >= 10 ? item.valueRatio.toFixed(0) : item.valueRatio.toFixed(1);
  // verdict is itself translated text (no markup), passed as a var into the
  // outer T() call below — same nesting inventory.ts uses for running.summary.
  const verdict =
    item.valueRatio >= 1
      ? T("value.timesPrice", { times })
      : T("value.pctPrice", { pct: Math.round(item.valueRatio * 100) });
  return `<p class="lg-flag">${esc(T("value.summary", { value: money(item.usage30), verdict }))}</p>`;
}

function blank(): Subscription {
  return { id: "", name: "", price: 0, cycle: "monthly", renewsOn: null, provider: null, notes: null };
}

function form(sub: Subscription): string {
  const tools = source?.tools() ?? [];
  // Named `tool`, not `t`: this module imports the translator as `t`, and a
  // same-named callback param here would shadow it silently (same fix as
  // inventory.ts's renderTools).
  const toolOptions =
    `<option value="">${esc(T("form.nothingTracked"))}</option>` +
    tools
      .map((tool) => `<option value="${esc(tool.id)}"${sub.provider === tool.id ? " selected" : ""}>${esc(tool.name)}</option>`)
      .join("");
  // The Name placeholder ("Claude Max") is an example plan name, not prose:
  // it stays as-is in every locale, same as a detected plan name would.
  return `
    <form class="card-panel lg-form" id="lg-form">
      <label>${esc(T("form.name"))} <input id="lg-name" type="text" maxlength="80" value="${esc(sub.name)}" placeholder="Claude Max" required /></label>
      <div class="lg-form-row">
        <label>${esc(T("form.price"))} <input id="lg-price" type="number" min="0" step="0.01" value="${sub.price > 0 ? sub.price : ""}" placeholder="0.00" required /></label>
        <label>${esc(T("form.billed"))}
          <select id="lg-cycle">
            <option value="monthly"${sub.cycle === "monthly" ? " selected" : ""}>${esc(T("form.monthly"))}</option>
            <option value="yearly"${sub.cycle === "yearly" ? " selected" : ""}>${esc(T("form.yearly"))}</option>
          </select>
        </label>
      </div>
      <div class="lg-form-row">
        <label>${esc(T("form.renewsOn"))} <input id="lg-date" type="date" value="${esc(sub.renewsOn ?? "")}" /></label>
        <label>${esc(T("form.paysFor"))} <select id="lg-provider">${toolOptions}</select></label>
      </div>
      <label>${esc(T("form.notes"))} <input id="lg-notes" type="text" maxlength="300" value="${esc(sub.notes ?? "")}" placeholder="${esc(T("form.optional"))}" /></label>
      ${formError ? `<p class="lg-error" role="alert">${esc(formError)}</p>` : ""}
      <div class="lg-form-actions">
        <button type="button" class="inv-learn" id="lg-cancel">${esc(t("dialog.cancel"))}</button>
        <button type="submit" class="lg-save">${sub.id ? esc(T("form.saveChanges")) : esc(T("form.addSubscription"))}</button>
      </div>
    </form>`;
}

function render(): void {
  const el = document.querySelector<HTMLElement>("#ledger");
  if (!el) return;
  if (loadError) {
    el.innerHTML = `<article class="provider"><div class="card-panel"><p class="inv-empty">${esc(T("loadError", { error: loadError }))}</p></div></article>`;
    return;
  }
  if (!ledger) {
    el.innerHTML = `<div class="skeleton-card"><div class="skeleton-line title"></div><div class="skeleton-line bar"></div></div>`;
    return;
  }
  const lg = ledger;
  const linked = new Set(lg.items.map((i) => i.provider).filter(Boolean));
  // `tool`, not `t`: see the comment in form() above.
  const suggestions = (source?.tools() ?? []).filter((tool) => !linked.has(tool.id));

  const headline = lg.items.length
    ? `<article class="provider"><div class="card-panel lg-total">
        <div class="dt-headline"><b>${money(lg.monthly)}</b><span>${esc(T("total.line", { yearly: money(lg.yearly) }))}</span></div>
        ${lg.idleMonthly > 0 ? `<p class="lg-flag lg-flag-idle">${esc(T("idleTotal", { amount: money(lg.idleMonthly) }))}</p>` : ""}
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
          <span class="lg-price">${money(item.price)}<small> ${esc(cycleSuffix(item.cycle))}</small></span>
        </div>
        <div class="lg-item-sub">
          <span>${esc(renewalText(item))}</span>
          <span class="lg-actions">
            <button class="lg-link" data-edit="${esc(item.id)}">${esc(T("item.edit"))}</button>
            <button class="lg-link${confirmDelete === item.id ? " lg-danger" : ""}" data-delete="${esc(item.id)}">${confirmDelete === item.id ? esc(T("item.reallyDelete")) : esc(T("item.delete"))}</button>
          </span>
        </div>
        ${valueLine(item)}
        ${item.notes ? `<p class="lg-notes">${esc(item.notes)}</p>` : ""}
      </div>`,
    )
    .join("");

  const empty =
    lg.items.length === 0 && !editing
      ? `<article class="provider"><div class="card-panel"><p class="inv-empty">${esc(T("empty"))}</p></div></article>`
      : "";

  const suggest =
    suggestions.length && !editing
      ? `<div class="lg-suggest"><span>${esc(T("suggest.found"))}</span>${suggestions
          .map((tool) => `<button class="inv-chip lg-chip" data-suggest="${esc(tool.id)}">+ ${esc(tool.name)}${tool.plan ? ` ${esc(cap(tool.plan))}` : ""}</button>`)
          .join("")}</div>`
      : "";

  el.innerHTML = `
    <div class="inv-toolbar">
      <p class="inv-note">${esc(T("note"))}</p>
      <span class="lg-toolbar">
        ${lg.items.length && !editing ? `<button class="inv-rescan" id="lg-export" title="${esc(T("export.tip"))}">${esc(T("export.button"))}</button>` : ""}
        ${editing ? "" : `<button class="inv-rescan" id="lg-add">${esc(T("add"))}</button>`}
      </span>
    </div>
    ${exportNote ? `<p class="inv-note">${esc(exportNote)}</p>` : ""}
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

/// Redraws the Subscriptions tab in place, e.g. after a locale switch (task 7
/// wires this into the locale-change handler). A no-op while another view is
/// showing, or before the first load has produced anything to redraw.
export function rerender(): void {
  const el = document.querySelector<HTMLElement>("#ledger");
  if (el && !el.hidden) render();
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
    if (target.closest("#lg-export")) {
      if (!ledger) return;
      void invoke<string>("export_table", {
        name: "ai subscriptions",
        headers: [
          t("ledger.form.name"), t("ledger.form.price"), t("ledger.form.billed"), t("ledger.csv.monthlyCost"),
          t("ledger.csv.nextRenewal"), t("ledger.form.paysFor"), t("ledger.csv.usage30"), t("ledger.form.notes"),
        ],
        rows: ledger.items.map((i) => [
          i.name, i.price.toFixed(2), i.cycle, i.monthlyCost.toFixed(2), i.nextRenewal ?? "", i.provider ?? "",
          i.usage30 === null ? "" : i.usage30.toFixed(2), i.notes ?? "",
        ]),
      }).then(
        (path) => { exportNote = t("detail.csv.saved", { path }); render(); },
        (err) => { exportNote = String(err); render(); },
      );
      return;
    }
    exportNote = "";
    if (target.closest("#lg-add")) {
      editing = blank();
    } else if (target.closest("#lg-cancel")) {
      editing = null;
    } else if (suggestId) {
      // Not `t` (shadows the translator import) and not `tool` either: the
      // very next line declares its own `tool` const, which this would shadow too.
      const tool = source?.tools().find((entry) => entry.id === suggestId);
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
