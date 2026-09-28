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
