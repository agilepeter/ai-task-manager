// The About panel (src/about.ts): behavioural tests in the same style as
// scripts/agents-view.test.mjs -- real functions from the real source, run
// against a fake `document` minimal enough for openAbout() to execute end to
// end, no real DOM required. src/about.ts's own imports (invoke, getVersion,
// BRAND/CREDITS, t) are inlined/stripped the same way
// scripts/agents-view.test.mjs does it for src/agents.ts's imports: src/i18n.ts
// first (with its locale JSON imports inlined), src/brand.ts next (no imports
// of its own, so it is appended as is), then src/about.ts's source with its
// four import lines dropped -- t and BRAND/CREDITS are already in scope from
// the sources above it; invoke and getVersion (Tauri APIs this suite never
// calls for real) become local stand-ins instead.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

let cachedModule = null;
async function loadAboutModule() {
  if (!cachedModule) cachedModule = buildAboutModule();
  return cachedModule;
}

async function buildAboutModule() {
  const i18nSource = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const localesDir = new URL("../src/locales/", import.meta.url);
  const inlinedI18n = await inlineLocaleImports(i18nSource, localesDir);

  const brandSource = await readFile(new URL("../src/brand.ts", import.meta.url), "utf8");

  const aboutSource = await readFile(new URL("../src/about.ts", import.meta.url), "utf8");
  const strippedAbout = aboutSource
    .replace(
      'import { invoke } from "@tauri-apps/api/core";',
      'const invoke = async () => { throw new Error("invoke() is not stubbed in this test"); };',
    )
    .replace('import { getVersion } from "@tauri-apps/api/app";', 'const getVersion = async () => "0.0.0-test";')
    .replace('import { BRAND, CREDITS } from "./brand";', "")
    .replace('import { t } from "./i18n";', "")
    // src/i18n.ts exports its own top-level `render(locale, msg)`; about.ts's
    // own private `render(version)` would otherwise collide with it once the
    // two sources are concatenated -- the same rename
    // scripts/agents-view.test.mjs applies to src/agents.ts's own `render`,
    // a plain word-boundary rename of the declaration and every call site,
    // never a single substitution.
    .replace(/\brender\b/g, "__aboutRender");
  if (strippedAbout === aboutSource) throw new Error("no substitution matched -- src/about.ts's source shape moved under this test");

  const code = ts.transpileModule(`${inlinedI18n}\n${brandSource}\n${strippedAbout}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

// A minimal stand-in for `document`, the same shape
// scripts/agents-view.test.mjs and scripts/audit-reload.test.mjs use for the
// same reason: one persistent fake element per selector (so a write in one
// call and a read in a later one see the same object), a real toggleable
// body classList, and the handful of members this module's code actually
// calls on one -- `.innerHTML`, `.focus()`, `.classList`,
// `.addEventListener()`, `.setAttribute()`/`.removeAttribute()` (the `inert`
// toggle) as no-ops.
function makeFakeDocument() {
  const bodyClasses = new Set();
  const elements = new Map();
  const attrs = new Map();
  function elementFor(selector) {
    if (!elements.has(selector)) {
      elements.set(selector, {
        innerHTML: "",
        focused: false,
        focus() { this.focused = true; },
        addEventListener() {},
        setAttribute(name) { attrs.set(`${selector}\u0000${name}`, true); },
        removeAttribute(name) { attrs.delete(`${selector}\u0000${name}`); },
        hasAttribute(name) { return attrs.has(`${selector}\u0000${name}`); },
        classList: { contains: () => false, add() {}, remove() {} },
      });
    }
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
    querySelectorAll: () => [],
    contains: (el) => [...elements.values()].includes(el),
    addEventListener: () => {},
  };
}

function flushMicrotasks() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

// closeFocusTarget() (src/about.ts) is the pure decision close() makes about
// where to send focus -- the same convention every view here follows (see
// audit.ts's own comment: each view keeps a private copy rather than share
// one across independent views), exercised the same way
// scripts/agents-view.test.mjs exercises its own copy: plain placeholder
// objects standing in for elements, no real DOM required.
test("closeFocusTarget: returns to the opener when it is still in the document, else the fallback, else nothing", async () => {
  const { closeFocusTarget } = await loadAboutModule();
  const opener = { tag: "opener" };
  const fallback = { tag: "fallback" };
  assert.equal(closeFocusTarget(opener, true, fallback), opener, "a live opener should win over the fallback");
  assert.equal(closeFocusTarget(opener, false, fallback), fallback, "a detached opener should fall back");
  assert.equal(closeFocusTarget(null, false, fallback), fallback, "no opener at all should fall back");
  assert.equal(closeFocusTarget(null, false, null), null, "no opener and no fallback should return null, not throw");
});

// openAbout() end to end: opens the panel (body class + inert removed) and
// moves focus into it (the heading), the same on-open contract every panel
// in this app now follows (src/agents.ts's own openAgents() is the original).
test("openAbout(): opens the panel, clears its inert attribute, renders content, and focuses the heading", async () => {
  const { openAbout } = await loadAboutModule();
  const fakeDocument = makeFakeDocument();
  globalThis.document = fakeDocument;
  try {
    fakeDocument.querySelector("#about").setAttribute("inert", "");
    const opener = fakeDocument.querySelector("#about-btn");

    openAbout(opener);
    await flushMicrotasks();

    assert.ok(fakeDocument.body.classList.contains("about-open"), "openAbout() must add the about-open body class");
    assert.ok(!fakeDocument.elements.get("#about").hasAttribute("inert"), "openAbout() must clear the panel's inert attribute");
    assert.ok(fakeDocument.elements.get("#about-heading").focused, "openAbout() must focus the panel heading");
    assert.match(fakeDocument.elements.get("#about-body").innerHTML, /AI Task Manager/, "openAbout() must have rendered the panel's content");
  } finally {
    delete globalThis.document;
  }
});

// rerender() is a no-op while the panel is closed (the same freshness
// contract every other panel's own rerender() follows), and redraws once it
// is open -- e.g. after a locale switch.
test("rerender(): a no-op while closed, redraws once the panel is open", async () => {
  const { rerender, openAbout } = await loadAboutModule();
  const fakeDocument = makeFakeDocument();
  globalThis.document = fakeDocument;
  try {
    rerender();
    assert.equal(fakeDocument.querySelector("#about-body").innerHTML, "", "rerender() must not draw into a closed panel");

    openAbout(null);
    await flushMicrotasks();
    fakeDocument.querySelector("#about-body").innerHTML = ""; // simulate something else having cleared it
    rerender();
    assert.match(fakeDocument.querySelector("#about-body").innerHTML, /AI Task Manager/, "rerender() must redraw once the panel is open");
  } finally {
    delete globalThis.document;
  }
});
