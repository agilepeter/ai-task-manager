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
