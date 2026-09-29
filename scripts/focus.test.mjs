// src/focus.ts's focusOrFallback(): the safe .focus() every panel's own
// close()/cancel() uses instead of a bare `target?.focus()`. Exercised
// against fake elements standing in for a real DOM -- .focus() on a
// "focusable" fake actually moves a tracked activeElement, the same way a
// real <button> does; .focus() on a "not focusable" fake is a no-op, the
// same way a real <span> with no tabindex is -- so this suite can prove the
// fallback fires exactly when the browser itself would have silently
// refused the first target, without needing a real browser.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

async function loadFocusModule() {
  const source = await readFile(new URL("../src/focus.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

/** A fake `document` with one tracked `activeElement`, real enough for
 *  focusOrFallback() to read back what actually "took". Starts pointed at a
 *  distinct sentinel, never `null` or either test's own target/fallback --
 *  the real DOM's `document.activeElement` is never actually `null` (it
 *  defaults to `<body>` when nothing else has focus), so a fake that started
 *  at `null` would let a `null` target trivially "match" it and skip the
 *  fallback for the wrong reason. */
function fakeDocumentWithFocus() {
  const doc = { activeElement: { tag: "whatever-had-focus-before" } };
  return doc;
}

/** A fake element: `focusable: true` moves the fake document's
 *  activeElement, the way a real <button>/<a>/[tabindex] element's .focus()
 *  does; `focusable: false` is a no-op, the way a real <span> with no
 *  tabindex's .focus() is. */
function fakeElement(doc, { focusable = true } = {}) {
  const el = {
    focus() {
      if (focusable) doc.activeElement = el;
    },
  };
  return el;
}

test("focusOrFallback: a focusable target wins, the fallback is never touched", async () => {
  const { focusOrFallback } = await loadFocusModule();
  const doc = fakeDocumentWithFocus();
  globalThis.document = doc;
  try {
    const target = fakeElement(doc, { focusable: true });
    const fallback = fakeElement(doc, { focusable: true });
    focusOrFallback(target, fallback);
    assert.equal(doc.activeElement, target, "a focusable target should win over the fallback");
  } finally {
    delete globalThis.document;
  }
});

test("focusOrFallback: a non-focusable target (the real WebKit case: a plain <span>/<article> opener) falls back", async () => {
  const { focusOrFallback } = await loadFocusModule();
  const doc = fakeDocumentWithFocus();
  globalThis.document = doc;
  try {
    const target = fakeElement(doc, { focusable: false });
    const fallback = fakeElement(doc, { focusable: true });
    focusOrFallback(target, fallback);
    assert.equal(doc.activeElement, fallback, "an unfocusable target must fall back rather than silently doing nothing");
  } finally {
    delete globalThis.document;
  }
});

test("focusOrFallback: a null target falls back", async () => {
  const { focusOrFallback } = await loadFocusModule();
  const doc = fakeDocumentWithFocus();
  globalThis.document = doc;
  try {
    const fallback = fakeElement(doc, { focusable: true });
    focusOrFallback(null, fallback);
    assert.equal(doc.activeElement, fallback);
  } finally {
    delete globalThis.document;
  }
});

test("focusOrFallback: with no fallback argument, defaults to document.body", async () => {
  const { focusOrFallback } = await loadFocusModule();
  const doc = fakeDocumentWithFocus();
  const body = fakeElement(doc, { focusable: true });
  doc.body = body;
  globalThis.document = doc;
  try {
    focusOrFallback(null);
    assert.equal(doc.activeElement, body, "with no explicit fallback, document.body (tabindex=\"-1\" in index.html) is the default");
  } finally {
    delete globalThis.document;
  }
});

// --- keeping a keyboard user's place across a redraw -------------------------

test("an anchor is found again by its id, else its provider, else its section", async () => {
  const { anchorSelector } = await loadFocusModule();
  assert.equal(anchorSelector("inv-rescan", undefined, undefined), '[id="inv-rescan"]');
  assert.equal(anchorSelector("", "claude", undefined), '[data-provider="claude"]');
  assert.equal(anchorSelector("", undefined, "running"), '[data-section="running"]');
  assert.equal(anchorSelector("x", "claude", "running"), '[id="x"]', "the id is the surest");
  assert.equal(anchorSelector("", undefined, undefined), null);
  assert.equal(anchorSelector("", "", undefined), '[data-provider=""]', "an empty value is still a value");
});

test("a value with a quote or a backslash in it cannot break out of the selector", async () => {
  const { anchorSelector, insideSelector } = await loadFocusModule();
  assert.equal(anchorSelector("", 'a"b', undefined), '[data-provider="a\\"b"]');
  assert.equal(anchorSelector("", "a\\b", undefined), '[data-provider="a\\\\b"]');
  assert.equal(insideSelector("BUTTON", "mini-btn", { end: 'x"]' }), 'button.mini-btn[data-end="x\\"]"]');
});

test("a control is described by its tag, first class and data, never by this app's own marker", async () => {
  const { insideSelector } = await loadFocusModule();
  assert.equal(insideSelector("SPAN", "provider-name", { tabAdded: "" }), "span.provider-name");
  assert.equal(insideSelector("BUTTON", "", {}), "button");
  assert.equal(insideSelector("BUTTON", "mini-btn", { end: "notes", tabAdded: "" }), 'button.mini-btn[data-end="notes"]');
  assert.equal(insideSelector("SPAN", "clickable", { flip: "reset" }), 'span.clickable[data-flip="reset"]');
  assert.equal(
    insideSelector("DIV", "row", { drillLabel: "Mon", drillDay: "2026-09-01" }),
    'div.row[data-drill-day="2026-09-01"][data-drill-label="Mon"]',
    "camelCase back to the attribute's own spelling, in a fixed order",
  );
});

test("focus is handed back only to a keyboard user whose control is gone and whose focus fell to nothing", async () => {
  const { handsFocusBack } = await loadFocusModule();
  assert.equal(handsFocusBack(true, true, true), true);
  assert.equal(handsFocusBack(false, true, true), false, "a pointer user has no place to keep");
  assert.equal(handsFocusBack(true, false, true), false, "the control is still there: focus left it on purpose");
  assert.equal(handsFocusBack(true, true, false), false, "a handler already put focus somewhere");
});

// A small DOM stand-in for placeOf() and findPlace(). Its selector matching
// covers exactly the forms focus.ts builds, with the browser's rule that an
// attribute selector matches an element carrying more attributes than it
// names.
function parseSelector(sel) {
  const m = /^([a-z]*)(?:\.([\w-]+))?((?:\[[\w-]+="(?:[^"\\]|\\.)*"\])*)$/.exec(sel);
  if (!m) throw new Error(`the stand-in cannot parse ${sel}`);
  const attrs = [...m[3].matchAll(/\[([\w-]+)="((?:[^"\\]|\\.)*)"\]/g)].map(([, k, v]) => [k, v.replace(/\\(.)/g, "$1")]);
  return { tag: m[1], cls: m[2], attrs };
}
class El {
  constructor(tag, { id = "", cls = [], data = {} } = {}, children = []) {
    Object.assign(this, { tagName: tag.toUpperCase(), id, classList: cls, dataset: { ...data }, children, parentElement: null, isConnected: true });
    for (const c of children) c.parentElement = this;
  }
  attr(name) {
    if (name === "id") return this.id || null;
    if (!name.startsWith("data-")) return null;
    const key = name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    return key in this.dataset ? this.dataset[key] : null;
  }
  matches(sel) {
    const { tag, cls, attrs } = parseSelector(sel);
    if (tag && this.tagName.toLowerCase() !== tag) return false;
    if (cls && !this.classList.includes(cls)) return false;
    return attrs.every(([k, v]) => this.attr(k) === v);
  }
  *walk() { for (const c of this.children) { yield c; yield* c.walk(); } }
  querySelectorAll(sel) { return [...this.walk()].filter((e) => e.matches(sel)); }
  querySelector(sel) { return this.querySelectorAll(sel)[0] ?? null; }
}
// Two cards and a toolbar button, built fresh each time: a redraw.
function page(extraFirst = false) {
  const card = (id) => new El("article", { cls: ["provider"], data: { provider: id } }, [
    new El("span", { cls: ["provider-name"], data: { tabAdded: "" } }),
    ...(extraFirst && id === "codex" ? [new El("button", { cls: ["quick-link"], data: { link: "x" } })] : []),
    new El("button", { cls: ["quick-link"], data: { link: "x", tabAdded: "" } }),
    new El("button", { cls: ["quick-link"], data: { link: "x", tabAdded: "" } }),
  ]);
  return new El("body", {}, [card("claude"), card("codex"), new El("button", { id: "inv-rescan" })]);
}
function useDocument(body) {
  globalThis.document = { body, querySelector: (s) => body.querySelector(s) };
}

test("a control is found again in the same place after a redraw", async () => {
  const { placeOf, findPlace } = await loadFocusModule();
  let body = page();
  useDocument(body);
  const second = body.children[1].children[2]; // codex's second quick link
  const place = placeOf(second);
  assert.deepEqual(place, { anchor: '[data-provider="codex"]', inside: 'button.quick-link[data-link="x"]', index: 1 });
  body = page();
  useDocument(body);
  assert.equal(findPlace(place), body.children[1].children[2], "the same card, the same position among its twins");
});

test("a control that is its own anchor is found by its id", async () => {
  const { placeOf, findPlace } = await loadFocusModule();
  let body = page();
  useDocument(body);
  const place = placeOf(body.children[2]);
  assert.deepEqual(place, { anchor: '[id="inv-rescan"]', inside: "", index: 0 });
  body = page();
  useDocument(body);
  assert.equal(findPlace(place), body.children[2]);
});

test("this app's own marker never decides whether a control is found again", async () => {
  const { placeOf, findPlace } = await loadFocusModule();
  let body = page();
  useDocument(body);
  const place = placeOf(body.children[0].children[1]);
  body = page();
  useDocument(body);
  for (const e of body.walk()) delete e.dataset.tabAdded; // redrawn, not marked yet
  assert.equal(findPlace(place), body.children[0].children[1]);
});

test("nothing is handed back when the place is gone, or was never one", async () => {
  const { placeOf, findPlace } = await loadFocusModule();
  const body = page();
  useDocument(body);
  assert.equal(findPlace({ anchor: '[data-provider="gone"]', inside: "button.quick-link", index: 0 }), null, "the card was hidden meanwhile");
  assert.equal(findPlace({ anchor: '[data-provider="claude"]', inside: 'button.quick-link[data-link="x"]', index: 5 }), null);
  const orphan = new El("button", { cls: ["loose"] });
  orphan.parentElement = body;
  assert.equal(placeOf(orphan), null, "a control with no anchor has no place to keep");
});
