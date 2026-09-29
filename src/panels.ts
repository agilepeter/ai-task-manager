// One module that owns which slide-in panel is "on top" -- no panel module
// (about.ts, audit.ts, agents.ts, detail.ts) or main.ts (Settings, Customize,
// the confirm dialog) decides this for itself any more. Two problems this
// fixes, now that every control is reachable by plain Tab (src/tabbable.ts):
//
// A. Content behind an open panel stayed in the Tab order -- with Settings
//    open, Tab walked through its own controls and then straight into the
//    tab bar and cards underneath it.
// B. Two panels could be open at once (e.g. Settings opened from the rail
//    while Agents was open), with no agreement on which one is actually on
//    top: each panel used to set its own `inert` and answer its own Escape
//    unconditionally, so a mouse click could land on a covered close button
//    and Escape closed whichever handler happened to be registered first,
//    never necessarily the panel a person could actually see.
//
// The stack: which panels are open, in the order they were opened, derived
// every time from the six body classes that already mean "this panel is
// open" (drawer-open, about-open, audit-open, agents-open, detail-open,
// settings-open) -- never written here, only read. A MutationObserver on
// document.body's own `class` attribute (never subtree/children: nothing
// else on the page is this module's business) keeps the stack in step with
// every future change; syncPanels() below is also called directly, and
// synchronously, from each panel's own open()/close() right after it
// toggles its class, because the very next line in most of them moves focus
// into the panel -- a MutationObserver callback is always a microtask late
// for that, so relying on it alone would leave the newly-opened panel
// `inert` at the exact moment its own heading tries to focus itself.
// Calling syncPanels() twice for one change is harmless: it always
// recomputes from the DOM's actual current classes, not from what changed,
// so the observer's own call just reconfirms what the direct one already
// applied.

/** Body class each panel's open state lives on -- the one thing this module
 *  reads and never writes. Adding a panel to the app is: give its `<aside>`
 *  the same open-class convention, add one line here, and remove whatever
 *  code used to set/clear `inert` on it directly (see each panel's own
 *  open()/close() on this branch for the shape that removal takes). */
const CLASS_FOR: Record<string, string> = {
  drawer: "drawer-open",
  about: "about-open",
  audit: "audit-open",
  agents: "agents-open",
  detail: "detail-open",
  settings: "settings-open",
};

/** Every panel this module knows about, in a fixed order -- also the
 *  deterministic tie-break nextStack() uses when two classes appear in the
 *  very same batch (see that function's own comment). */
export const PANEL_IDS: string[] = Object.keys(CLASS_FOR);

/** The element each panel's close() should send focus to when it is forced
 *  to give up its own opener/fallback because another panel is still open
 *  underneath (see focusAfterClose() below) -- every panel's own static
 *  heading, except Customize, whose panel has no heading of its own (see
 *  index.html's #drawer and setDrawer()'s own comment): its Done button is
 *  the drawer's first control, the same role a heading plays everywhere
 *  else. */
const HEADING_FOR: Record<string, string> = {
  drawer: "[data-customize-close]",
  about: "#about-heading",
  audit: "#audit-heading",
  agents: "#agents-heading",
  detail: "#detail-title",
  settings: "#settings-heading",
};

// ---------------------------------------------------------------------------
// Pure core -- no DOM. Exhaustively tested in scripts/panels.test.mjs.
// ---------------------------------------------------------------------------

/** Which of `ids` (default every known panel) has its open-class present,
 *  per `hasClass` -- a plain predicate rather than a live DOMTokenList, so
 *  this asks the same single question (`document.body.classList.contains`)
 *  every other piece of this app already asks, and any DOM stand-in that
 *  implements that one method works here (real `classList`s are not
 *  iterable in every fake used across this suite's other tests, only
 *  queryable one class at a time). A class this module does not recognise
 *  (`wide`, `no-glass`, anything else that ever lands on body) is silently
 *  ignored: it names no panel, so it can never become one. */
export function openIdsFromClasses(hasClass: (cls: string) => boolean, ids: string[] = PANEL_IDS): string[] {
  return ids.filter((id) => hasClass(CLASS_FOR[id]));
}

/** Folds one round of "these ids are open now" into the ordered stack. An id
 *  that is open now and was not in `stack` is newly opened -- appended at
 *  the end, since the thing that just opened is the topmost thing on
 *  screen; one that was in `stack` but is not open now is gone -- removed
 *  from wherever it sat, never just marked; every id still open keeps its
 *  existing place rather than jumping to the end again. Two ids that both
 *  appear for the first time in the same call are pushed in the order
 *  `openIds` lists them -- the caller below always builds that list by
 *  walking PANEL_IDS's own fixed order, so a MutationObserver batch that
 *  changed several classes at once still resolves deterministically. */
export function nextStack(stack: string[], openIds: string[]): string[] {
  const open = new Set(openIds);
  const kept = stack.filter((id) => open.has(id));
  const added = openIds.filter((id) => !stack.includes(id));
  return [...kept, ...added];
}

export interface PanelPlan {
  /** Every id in `allIds` that must carry `inert` -- every open-but-covered
   *  panel and every closed one alike; only the top is left out. One rule
   *  covers both cases, because "not currently reachable" is the same fact
   *  either way. */
  inertIds: Set<string>;
  /** The id at the top of the stack, or null when nothing is open. */
  topId: string | null;
  /** Whether the background (the tab bar and the views behind every panel)
   *  must be inert -- true whenever anything at all is open. */
  backgroundInert: boolean;
}

/** The whole decision, pure and DOM-free. `allIds` is the id universe this
 *  round considers -- normally PANEL_IDS, but syncPanels() below narrows it
 *  to exclude "detail" while wide mode has it docked as a second, permanent
 *  column rather than an overlay (see that function's own comment). */
export function plan(stack: string[], allIds: string[] = PANEL_IDS): PanelPlan {
  const topId = stack.length > 0 ? stack[stack.length - 1] : null;
  const inertIds = new Set(allIds.filter((id) => id !== topId));
  return { inertIds, topId, backgroundInert: stack.length > 0 };
}

// ---------------------------------------------------------------------------
// The DOM applier -- everything below actually touches the page. Kept thin
// on purpose: every decision above is already fully tested without it.
// ---------------------------------------------------------------------------

let stack: string[] = [];

/** >0 while a modal (today, only the confirm dialog -- appConfirm() in
 *  src/main.ts) is shown. A modal sits above every panel, the background,
 *  AND the rail at once (see applyPlan()'s own comment on the rail); the
 *  stack itself is left completely untouched while a modal is up, so the
 *  exact prior state comes back the moment it ends. A counter, not a
 *  boolean, so a second modal opened over the first -- there is none today,
 *  but nothing here assumes there never will be -- could not let the
 *  first's own close reopen everything early. */
let modalDepth = 0;

function panelEl(id: string): HTMLElement | null {
  return document.querySelector<HTMLElement>(`#${id}`);
}

function setInert(el: Element | null, on: boolean): void {
  if (!el) return;
  if (on) el.setAttribute("inert", "");
  else el.removeAttribute("inert");
}

function setTop(el: Element | null, on: boolean): void {
  if (!el) return;
  if (on) el.setAttribute("data-top-panel", "");
  else el.removeAttribute("data-top-panel");
}

function applyPlan(): void {
  const mainCol = document.querySelector<HTMLElement>(".main-col");
  const sideZone = document.querySelector<HTMLElement>("#side-zone");
  if (modalDepth > 0) {
    // The modal is the top of everything: every panel, the background AND
    // the rail go inert. The rail is reachable by mouse over an open PANEL
    // (#side-zone's z-index sits above every panel's own -- see
    // styles.css's comment on #side-zone) but not over this modal:
    // #confirm-overlay's z-index sits above the rail's too, and its
    // backdrop covers the whole viewport, so a mouse click there cannot
    // reach the rail either -- a keyboard user must not be able to where a
    // mouse user cannot.
    for (const id of PANEL_IDS) {
      const el = panelEl(id);
      setInert(el, true);
      setTop(el, false);
    }
    setInert(mainCol, true);
    setInert(sideZone, true);
    return;
  }
  const wide = document.body.classList.contains("wide");
  // Wide mode docks Detail as a permanent second column beside the list
  // (CLAUDE.md's "Wide mode" section; src/styles.css's `body.wide #detail`
  // rules put it at `left: 380px`, full height, while the other five panels
  // shrink to `width: 380px` and only ever cover the LEFT column there).
  // Detail is therefore never covered by another panel in wide mode, and it
  // never makes the background inert on its own -- both columns are meant
  // to be usable together, which is the entire point of wide mode -- so it
  // is left out of the stacking contest entirely while wide is active.
  const allIds = wide ? PANEL_IDS.filter((id) => id !== "detail") : PANEL_IDS;
  const { inertIds, topId, backgroundInert } = plan(stack, allIds);
  for (const id of allIds) {
    const el = panelEl(id);
    setInert(el, inertIds.has(id));
    setTop(el, id === topId);
  }
  if (wide) {
    // Excluded from `allIds` above, so the loop never touched it: apply its
    // own, simpler rule directly -- interactive exactly while it is open,
    // never covered, never competing for "top" (nothing overlaps it).
    const detailEl = panelEl("detail");
    setInert(detailEl, !document.body.classList.contains("detail-open"));
    setTop(detailEl, false);
  }
  setInert(mainCol, backgroundInert);
  // The rail is reachable by mouse over an open panel today (its z-index
  // already sits above every panel's), so it must stay reachable by
  // keyboard too: never part of the inert background outside a modal. This
  // line is what keeps that true even if some future change to `plan()`'s
  // inputs ever tried to include it.
  setInert(sideZone, false);
}

/** Recomputes the stack from the DOM's current classes and applies `inert`
 *  and `data-top-panel` to every panel and the background. See this
 *  module's own header comment for why panels call this directly and
 *  synchronously, rather than relying solely on the MutationObserver
 *  initPanels() installs. */
export function syncPanels(): void {
  const wide = document.body.classList.contains("wide");
  const allIds = wide ? PANEL_IDS.filter((id) => id !== "detail") : PANEL_IDS;
  const openIds = openIdsFromClasses((cls) => document.body.classList.contains(cls), allIds);
  stack = nextStack(stack, openIds);
  applyPlan();
}

/** Called once, at boot (main.ts), after syncPanels() has already run once
 *  for whatever index.html's static markup starts with (nothing open).
 *  Belt and braces for any future class change this module did not itself
 *  cause via a direct syncPanels() call. */
export function initPanels(): void {
  syncPanels();
  new MutationObserver(syncPanels).observe(document.body, { attributes: true, attributeFilter: ["class"] });
}

/** True while `id` is the panel a person can actually see and reach right
 *  now -- the top of the stack, and only while no modal is shown (a modal
 *  is "the top of everything", so no panel is top while one is up). Each
 *  panel's own Escape handler asks this before acting, so one Escape press
 *  closes exactly one panel -- the one actually on top -- instead of
 *  whichever handler happens to be registered first. */
export function isTopPanel(id: string): boolean {
  return modalDepth === 0 && stack.length > 0 && stack[stack.length - 1] === id;
}

/** What a panel's close() should focus, now that syncPanels() has already
 *  run for its own removal from the stack: `candidate` (its own opener if
 *  still attached, else its own fallback) when the stack is now empty --
 *  exactly the answer every panel's close() already gave before two panels
 *  could ever be open at once -- OR when `candidate` lives inside the rail
 *  or the panel now exposed on top. Otherwise `candidate` is about to be
 *  inert again (or already is: it was never reachable while it sat under
 *  the panel that just closed either), so the now-top panel's own heading
 *  is the honest answer instead of a focus call that silently does
 *  nothing. */
export function focusAfterClose(candidate: HTMLElement | null): HTMLElement | null {
  const topId = stack.length > 0 ? stack[stack.length - 1] : null;
  if (!topId) return candidate;
  if (candidate) {
    const topEl = panelEl(topId);
    const rail = document.querySelector("#side-zone");
    if ((topEl && topEl.contains(candidate)) || (rail && rail.contains(candidate))) return candidate;
  }
  return document.querySelector<HTMLElement>(HEADING_FOR[topId]);
}

/** Called by appConfirm() (src/main.ts) right before it shows the dialog.
 *  Must run AFTER that function has already captured `document.activeElement`
 *  as its own opener: applying `inert` here can blur whatever currently has
 *  focus, which would otherwise corrupt that capture. */
export function beginModal(): void {
  modalDepth++;
  applyPlan();
}

/** Called by appConfirm() right after it removes the dialog from the DOM,
 *  and BEFORE it restores focus to its own opener: that opener can be a
 *  control inside the very panel this un-inerts, so focus would silently
 *  fail to land if this ran any later. */
export function endModal(): void {
  modalDepth = Math.max(0, modalDepth - 1);
  applyPlan();
}
