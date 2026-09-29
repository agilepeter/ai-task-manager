// src/panels.ts: the module that decides which slide-in panel is on top,
// tested two ways. First, its pure core (openIdsFromClasses(), nextStack(),
// plan()) -- no DOM, exhaustive, exactly the sequences its own header
// describes. Second, one behavioural walk with a fake `document`, the same
// shape scripts/agents-view.test.mjs and its siblings use, driving the real
// syncPanels()/isTopPanel()/focusAfterClose()/beginModal()/endModal() end to
// end through a whole session: open, cover, close the top one, a modal over
// two panels, and the wide-mode Detail exception -- one continuous scenario
// rather than several independent tests, since the module's own stack and
// modal depth are private, module-level state with no reset export (by
// design: nothing outside this module is meant to poke at them directly).
//
// src/panels.ts has no imports of its own, so it needs none of the
// combined-module inlining this suite's i18n-dependent tests use -- a
// straight transpile of its own source, the same as scripts/tabbable.test.mjs
// and scripts/focus.test.mjs.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

async function loadPanelsModule() {
  const source = await readFile(new URL("../src/panels.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

// ---------------------------------------------------------------------------
// Pure core: openIdsFromClasses(), nextStack(), plan(). No DOM.
// ---------------------------------------------------------------------------

test("openIdsFromClasses: reads only the classes it recognises, ignoring anything else on body", async () => {
  const { openIdsFromClasses, PANEL_IDS } = await loadPanelsModule();

  const present = new Set(["agents-open", "no-glass", "wide"]);
  const has = (c) => present.has(c);

  assert.deepEqual(openIdsFromClasses(has), ["agents"], "an unknown class (no-glass, wide) must never be read as a panel id");
  assert.deepEqual(openIdsFromClasses(() => false), [], "nothing open reads as no ids at all");
  assert.deepEqual(
    openIdsFromClasses((c) => ["drawer-open", "settings-open", "about-open", "audit-open", "agents-open", "detail-open"].includes(c)),
    PANEL_IDS,
    "every known class present reads back as every known id, in PANEL_IDS's own order",
  );
  // A narrowed `ids` list (the applier's own wide-mode exclusion of "detail")
  // is respected even when detail-open is itself present.
  assert.deepEqual(
    openIdsFromClasses((c) => c === "detail-open" || c === "audit-open", PANEL_IDS.filter((id) => id !== "detail")),
    ["audit"],
    "an excluded id must never be read back as open, even with its class present",
  );
});

test("nextStack: open A", async () => {
  const { nextStack } = await loadPanelsModule();
  assert.deepEqual(nextStack([], ["a"]), ["a"]);
});

test("nextStack: open A then B", async () => {
  const { nextStack } = await loadPanelsModule();
  assert.deepEqual(nextStack(["a"], ["a", "b"]), ["a", "b"], "B opens on top of A, appended at the end");
});

test("nextStack: close A while B is on top", async () => {
  const { nextStack } = await loadPanelsModule();
  assert.deepEqual(nextStack(["a", "b"], ["b"]), ["b"], "A is removed from wherever it sat; B keeps its place");
});

test("nextStack: close B", async () => {
  const { nextStack } = await loadPanelsModule();
  assert.deepEqual(nextStack(["b"], []), [], "the stack empties out completely");
});

test("nextStack: open A, B, C and close B", async () => {
  const { nextStack } = await loadPanelsModule();
  const withThree = nextStack(nextStack(nextStack([], ["a"]), ["a", "b"]), ["a", "b", "c"]);
  assert.deepEqual(withThree, ["a", "b", "c"]);
  assert.deepEqual(nextStack(withThree, ["a", "c"]), ["a", "c"], "B is removed from the middle; A and C keep their relative order");
});

test("nextStack: classes added in one mutation batch resolve deterministically, in the order the caller lists them", async () => {
  const { nextStack } = await loadPanelsModule();
  assert.deepEqual(nextStack([], ["b", "a"]), ["b", "a"], "both new in one call: pushed in the given order");
  assert.deepEqual(nextStack([], ["a", "b"]), ["a", "b"], "the reverse order pushes in the reverse order -- the caller's list is what decides, not alphabetical or any other implicit order");
});

test("nextStack: an id already open keeps its place rather than jumping to the end", async () => {
  const { nextStack } = await loadPanelsModule();
  // "a" was already open (bottom of the stack); this call reports "a" and
  // "c" open (b closed, c newly opened) -- a must stay at the bottom, not
  // jump above c just because it was named again.
  assert.deepEqual(nextStack(["a", "b"], ["a", "c"]), ["a", "c"]);
});

test("plan: nothing open -- every id is inert, no top, background not inert", async () => {
  const { plan } = await loadPanelsModule();
  const result = plan([], ["a", "b", "c"]);
  assert.deepEqual([...result.inertIds].sort(), ["a", "b", "c"]);
  assert.equal(result.topId, null);
  assert.equal(result.backgroundInert, false);
});

test("plan: one open -- it alone is not inert, it is top, background is inert", async () => {
  const { plan } = await loadPanelsModule();
  const result = plan(["a"], ["a", "b", "c"]);
  assert.deepEqual([...result.inertIds].sort(), ["b", "c"]);
  assert.equal(result.topId, "a");
  assert.equal(result.backgroundInert, true);
});

test("plan: two open -- only the top is not inert; the covered one is inert same as a closed one", async () => {
  const { plan } = await loadPanelsModule();
  const result = plan(["a", "b"], ["a", "b", "c"]);
  assert.deepEqual([...result.inertIds].sort(), ["a", "c"], "a is covered, c is closed -- both inert, only b (top) is not");
  assert.equal(result.topId, "b");
  assert.equal(result.backgroundInert, true);
});

// ---------------------------------------------------------------------------
// Behavioural: one continuous session against a fake document, driving the
// real DOM-touching functions.
// ---------------------------------------------------------------------------

/** A fake element carrying only what panels.ts reads or writes on one:
 *  `setAttribute`/`removeAttribute`/`hasAttribute` (real tracking, the same
 *  convention scripts/about-view.test.mjs's own fake elements use, since
 *  this suite asserts on `inert` and `data-top-panel` directly) and
 *  `contains()` -- a plain, explicit parent/child registry
 *  (`adopt()`/`isAdopted`) rather than a real DOM tree, since these fakes
 *  have no parents of their own; `contains(self)` is also true, matching a
 *  real Node's own reflexive contains(). */
function fakeElement(id) {
  const attrs = new Set();
  const children = new Set();
  return {
    id,
    setAttribute(name) { attrs.add(name); },
    removeAttribute(name) { attrs.delete(name); },
    hasAttribute(name) { return attrs.has(name); },
    adopt(child) { children.add(child); },
    contains(other) { return other === this || children.has(other); },
  };
}

function makeFakeDocument() {
  const bodyClasses = new Set();
  const elements = new Map();
  function elementFor(selector) {
    if (!elements.has(selector)) elements.set(selector, fakeElement(selector));
    return elements.get(selector);
  }
  return {
    elements,
    body: {
      classList: {
        contains: (c) => bodyClasses.has(c),
        add: (c) => bodyClasses.add(c),
        remove: (c) => bodyClasses.delete(c),
      },
    },
    querySelector: (selector) => elementFor(selector),
  };
}

test("panels.ts behavioural walk: open, cover, close the top one, a modal over two panels, and the wide-mode Detail exception", async () => {
  const { syncPanels, isTopPanel, focusAfterClose, beginModal, endModal } = await loadPanelsModule();
  const fakeDocument = makeFakeDocument();
  globalThis.document = fakeDocument;

  const mainCol = () => fakeDocument.elements.get(".main-col");
  const sideZone = () => fakeDocument.elements.get("#side-zone");
  const agents = () => fakeDocument.elements.get("#agents");
  const settings = () => fakeDocument.elements.get("#settings");
  const detail = () => fakeDocument.elements.get("#detail");
  const audit = () => fakeDocument.elements.get("#audit");

  try {
    // Nothing open: background and rail both reachable, nothing is "top".
    syncPanels();
    assert.equal(mainCol().hasAttribute("inert"), false, "background must not be inert with nothing open");
    assert.equal(sideZone().hasAttribute("inert"), false, "the rail must never be inert outside a modal");
    assert.equal(agents().hasAttribute("data-top-panel"), false);

    // Open Agents (from a main-col control): it is top, background goes inert.
    fakeDocument.body.classList.add("agents-open");
    syncPanels();
    assert.equal(agents().hasAttribute("inert"), false, "the only open panel must not be inert");
    assert.equal(agents().hasAttribute("data-top-panel"), true, "the only open panel is the top panel");
    assert.equal(mainCol().hasAttribute("inert"), true, "background must go inert once a panel is open");
    assert.equal(sideZone().hasAttribute("inert"), false, "the rail stays reachable over an open panel");
    assert.equal(isTopPanel("agents"), true);

    // Cover it with Settings (opened from the rail): Settings becomes top,
    // Agents becomes inert (covered, not closed), background stays inert.
    fakeDocument.body.classList.add("settings-open");
    syncPanels();
    assert.equal(settings().hasAttribute("inert"), false, "the newly-opened top panel must not be inert");
    assert.equal(settings().hasAttribute("data-top-panel"), true);
    assert.equal(agents().hasAttribute("inert"), true, "a covered panel must be inert, same as a closed one");
    assert.equal(agents().hasAttribute("data-top-panel"), false, "only one panel may carry data-top-panel at a time");
    assert.equal(mainCol().hasAttribute("inert"), true);
    assert.equal(isTopPanel("settings"), true, "Settings is on top");
    assert.equal(isTopPanel("agents"), false, "Agents is covered, so it is not top even though it is still open");

    // A modal (the confirm dialog) opens over both: everything goes inert,
    // including the rail and the panel that was on top a moment ago.
    beginModal();
    assert.equal(settings().hasAttribute("inert"), true, "the modal sits above every panel, including the one that was on top");
    assert.equal(settings().hasAttribute("data-top-panel"), false, "no panel is 'top' while a modal is up");
    assert.equal(agents().hasAttribute("inert"), true);
    assert.equal(mainCol().hasAttribute("inert"), true);
    assert.equal(sideZone().hasAttribute("inert"), true, "the rail is not reachable under the modal's own full-viewport backdrop, so it must not be reachable by keyboard either");
    assert.equal(isTopPanel("settings"), false, "isTopPanel must answer false for everything while a modal is shown");

    // The modal closes: the exact prior state comes back.
    endModal();
    assert.equal(settings().hasAttribute("inert"), false, "Settings must be exactly as it was before the modal");
    assert.equal(settings().hasAttribute("data-top-panel"), true);
    assert.equal(agents().hasAttribute("inert"), true, "Agents must still be covered, not suddenly exposed");
    assert.equal(sideZone().hasAttribute("inert"), false, "the rail is reachable again");
    assert.equal(isTopPanel("settings"), true);

    // One Escape closes the top panel (Settings). Its own opener is the
    // rail's gear button -- inside the rail -- so focusAfterClose() must
    // return it unchanged, not redirect to Agents' own heading.
    const settingsBtn = fakeDocument.querySelector("#settings-btn");
    sideZone().adopt(settingsBtn);
    fakeDocument.body.classList.remove("settings-open");
    syncPanels();
    assert.equal(agents().hasAttribute("inert"), false, "Agents, now exposed again, must no longer be inert");
    assert.equal(agents().hasAttribute("data-top-panel"), true, "Agents becomes the top panel again");
    assert.equal(mainCol().hasAttribute("inert"), true, "the background stays inert -- Agents is still open");
    assert.equal(focusAfterClose(settingsBtn), settingsBtn, "an opener inside the rail is used as is");

    // The opposite case: an opener that lives in neither the rail nor the
    // now-top panel (e.g. a stale main-col control) must redirect to the
    // now-top panel's own heading instead of silently focusing nothing.
    const staleMainColButton = fakeDocument.querySelector("#some-main-col-button");
    const agentsHeading = fakeDocument.querySelector("#agents-heading");
    assert.equal(focusAfterClose(staleMainColButton), agentsHeading, "a candidate outside the rail and the now-top panel must redirect to that panel's heading");
    assert.equal(focusAfterClose(null), agentsHeading, "no candidate at all must also redirect to the now-top panel's heading");

    // Closing the last panel empties the stack: focusAfterClose() stops
    // redirecting -- the plain candidate stands, exactly as before two
    // panels could ever be open at once.
    fakeDocument.body.classList.remove("agents-open");
    syncPanels();
    assert.equal(mainCol().hasAttribute("inert"), false, "background must not be inert once the stack is empty");
    assert.equal(focusAfterClose(staleMainColButton), staleMainColButton, "with the stack empty, the candidate is returned unchanged");
    assert.equal(focusAfterClose(null), null, "with the stack empty, null stays null rather than being redirected");

    // Wide mode: Detail docks as a second, permanent column. Open alone, it
    // must never make the background inert -- both columns are meant to be
    // used together.
    fakeDocument.body.classList.add("wide");
    fakeDocument.body.classList.add("detail-open");
    syncPanels();
    assert.equal(detail().hasAttribute("inert"), false, "an open, docked Detail must not be inert");
    assert.equal(detail().hasAttribute("data-top-panel"), false, "docked Detail never competes for 'top' -- nothing overlaps it");
    assert.equal(mainCol().hasAttribute("inert"), false, "the list column must stay usable beside a docked Detail");

    // Audit opens from the rail over the LEFT column only (per
    // src/styles.css's `body.wide #audit` rule): the background (the left
    // column) goes inert, but the docked Detail column, spatially
    // untouched, must not.
    fakeDocument.body.classList.add("audit-open");
    syncPanels();
    assert.equal(audit().hasAttribute("inert"), false);
    assert.equal(audit().hasAttribute("data-top-panel"), true);
    assert.equal(detail().hasAttribute("inert"), false, "docked Detail stays interactive even while another panel covers the left column");
    assert.equal(mainCol().hasAttribute("inert"), true, "the left column is genuinely covered by Audit now");
    assert.equal(isTopPanel("detail"), false, "Detail is excluded from the stacking contest entirely while wide");

    // Audit closes; the left column is usable again, Detail untouched.
    fakeDocument.body.classList.remove("audit-open");
    syncPanels();
    assert.equal(mainCol().hasAttribute("inert"), false);
    assert.equal(detail().hasAttribute("inert"), false);

    // Leaving wide mode with Detail still open: it re-enters the ordinary
    // overlay stack as a fresh push (nothing else is open to contend with),
    // becomes top, and now covers the background like the full-screen
    // slide-in it becomes in narrow mode.
    fakeDocument.body.classList.remove("wide");
    syncPanels();
    assert.equal(detail().hasAttribute("inert"), false);
    assert.equal(detail().hasAttribute("data-top-panel"), true, "narrow-mode Detail is a real overlay and becomes top");
    assert.equal(mainCol().hasAttribute("inert"), true, "narrow-mode Detail covers the background, unlike its docked wide-mode self");
  } finally {
    delete globalThis.document;
  }
});
