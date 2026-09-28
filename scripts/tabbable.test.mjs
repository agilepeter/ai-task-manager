// src/tabbable.ts's makeTabbable(): the pure per-element decision (skip
// disabled, skip genuinely unrendered, else tabIndex = 0), exercised against
// a fake root whose querySelectorAll() ignores the selector string and just
// returns a fixed list of fake elements -- the CSS selector itself (which
// tags/roles actually match) is a real-DOM question, checked directly
// against the live app in Playwright WebKit (see the report), not here.
// src/tabbable.ts has no imports of its own, so it needs none of the
// combined-module inlining this suite's other tests use for i18n-dependent
// modules -- straight import from the compiled source.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

async function loadTabbableModule() {
  const source = await readFile(new URL("../src/tabbable.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

/** A fake element carrying only what makeTabbable() reads or writes:
 *  `.disabled`, `.getClientRects()` and `.tabIndex`. `rects` defaults to one
 *  non-empty rect (a normally-rendered control); pass `[]` for one that is
 *  not actually rendered (display:none, zero-size, or -- the real case this
 *  matters for -- simply not present in this fake's own list). */
function fakeEl({ disabled = false, rects = [{}] } = {}) {
  return { disabled, getClientRects: () => rects, tabIndex: -1 };
}

function fakeRoot(elements) {
  return { querySelectorAll: () => elements };
}

test("makeTabbable: sets tabIndex=0 on a normal, enabled, rendered element", async () => {
  const { makeTabbable } = await loadTabbableModule();
  const el = fakeEl();
  makeTabbable(fakeRoot([el]));
  assert.equal(el.tabIndex, 0);
});

test("makeTabbable: skips a disabled element, leaving its tabIndex untouched", async () => {
  const { makeTabbable } = await loadTabbableModule();
  const el = fakeEl({ disabled: true });
  makeTabbable(fakeRoot([el]));
  assert.equal(el.tabIndex, -1, "a disabled control must not become tabbable");
});

test("makeTabbable: skips an element with no client rects (not actually rendered)", async () => {
  const { makeTabbable } = await loadTabbableModule();
  const el = fakeEl({ rects: [] });
  makeTabbable(fakeRoot([el]));
  assert.equal(el.tabIndex, -1, "an unrendered control (display:none, zero-size) must not become tabbable");
});

test("makeTabbable: a fixed-position, off-screen-via-transform element (an open OR closed slide-in panel) still gets tabIndex=0 -- inert, not this function, is what keeps a closed one out of the Tab order", async () => {
  const { makeTabbable } = await loadTabbableModule();
  // getClientRects() stays non-empty for a `position: fixed` element pushed
  // off-screen by `transform` (confirmed against the real CSS): only
  // display:none/zero-size empties it, which is exactly why this function
  // cannot be the mechanism that hides a closed panel's controls.
  const el = fakeEl({ rects: [{ x: -9999, y: -9999, width: 380, height: 600 }] });
  makeTabbable(fakeRoot([el]));
  assert.equal(el.tabIndex, 0);
});

test("makeTabbable: processes every element the selector returns, independently", async () => {
  const { makeTabbable } = await loadTabbableModule();
  const ok1 = fakeEl();
  const disabled = fakeEl({ disabled: true });
  const hidden = fakeEl({ rects: [] });
  const ok2 = fakeEl();
  makeTabbable(fakeRoot([ok1, disabled, hidden, ok2]));
  assert.equal(ok1.tabIndex, 0);
  assert.equal(disabled.tabIndex, -1);
  assert.equal(hidden.tabIndex, -1);
  assert.equal(ok2.tabIndex, 0);
});
