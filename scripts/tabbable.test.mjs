// src/tabbable.ts: which controls become Tab stops, which give focus up
// after a pointer click, and which answer Enter and Space. The selector
// itself (which tags and roles match) is a question for a real DOM and is
// checked against the running demo in WebKit, not here.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

async function load() {
  const source = await readFile(new URL("../src/tabbable.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

function fakeEl({ disabled = false } = {}) {
  const attrs = new Map();
  return { disabled, tabIndex: -1, attrs, setAttribute: (k, v) => attrs.set(k, v) };
}
const fakeRoot = (elements, self = null) => ({
  querySelectorAll: () => elements,
  ...(self ? { matches: () => true, ...self } : {}),
});

test("an enabled control becomes a Tab stop and is marked as this module's", async () => {
  const { makeTabbable, ADDED } = await load();
  const el = fakeEl();
  makeTabbable(fakeRoot([el]));
  assert.equal(el.tabIndex, 0);
  assert.ok(el.attrs.has(ADDED), "unmarked, it could not be told from a tabindex a template wrote");
});

test("a disabled control is left alone", async () => {
  const { makeTabbable, ADDED } = await load();
  const el = fakeEl({ disabled: true });
  makeTabbable(fakeRoot([el]));
  assert.equal(el.tabIndex, -1);
  assert.ok(!el.attrs.has(ADDED));
});

test("a subtree's own root is a control too", async () => {
  const { makeTabbable } = await load();
  const self = fakeEl();
  makeTabbable(fakeRoot([], self));
  // The mark lands on the object handed in, which is the root itself.
  const root = { querySelectorAll: () => [], matches: () => true, ...fakeEl() };
  makeTabbable(root);
  assert.equal(root.tabIndex, 0, "a button added on its own, not inside a wrapper, must not be missed");
});

test("every control in the list is decided on its own", async () => {
  const { makeTabbable } = await load();
  const [a, off, b] = [fakeEl(), fakeEl({ disabled: true }), fakeEl()];
  makeTabbable(fakeRoot([a, off, b]));
  assert.deepEqual([a.tabIndex, off.tabIndex, b.tabIndex], [0, -1, 0]);
});

test("the module never asks for layout", async () => {
  const { makeTabbable } = await load();
  const el = {
    ...fakeEl(),
    getClientRects() { throw new Error("asked for layout"); },
    getBoundingClientRect() { throw new Error("asked for layout"); },
    get offsetParent() { throw new Error("asked for layout"); },
  };
  makeTabbable(fakeRoot([el]));
  assert.equal(el.tabIndex, 0);
});

test("focus is given up only after a pointer click, and only from a control this module marked", async () => {
  const { releasesFocus } = await load();
  assert.equal(releasesFocus(true, true), true, "the control clicked, or the opener a closing panel returned focus to");
  assert.equal(releasesFocus(false, true), false, "Enter or Space made the click: a keyboard user keeps their place");
  assert.equal(releasesFocus(true, false), false, "a handler put focus on a heading or a field: it stays");
  assert.equal(releasesFocus(false, false), false);
});

test("Enter and Space activate a button by role, and nothing else", async () => {
  const { activatesByKey } = await load();
  assert.equal(activatesByKey("Enter", true, false, false), true);
  assert.equal(activatesByKey(" ", true, false, false), true);
  assert.equal(activatesByKey("Enter", true, true, false), false, "a real button answers the key itself: twice would be once too many");
  assert.equal(activatesByKey("Enter", false, false, false), false);
  assert.equal(activatesByKey("Enter", true, false, true), false, "something nearer already handled the key");
  for (const key of ["Tab", "Escape", "a", "ArrowRight", "Spacebar"]) assert.equal(activatesByKey(key, true, false, false), false, key);
});
