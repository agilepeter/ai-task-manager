// The Detail page's own focus-restore decision (src/detail.ts's
// closeFocusTarget()), exercised the same way scripts/agents-view.test.mjs
// exercises its own copy: plain placeholder objects standing in for
// elements, no real DOM required. Detail has no single fixed opener (dozens
// of cards can open it), so its own copy is tested directly against the
// module rather than re-derived here.
//
// Same combined-module technique as scripts/agents-view.test.mjs: src/i18n.ts
// (locale JSON imports inlined) first, src/format.ts next with its own
// `./i18n` import dropped, then src/detail.ts's source with its three
// imports dropped (invoke, i18n, format -- everything they name is already
// in scope) and its own top-level `function render(): void` renamed so it
// cannot collide with i18n.ts's exported `render(locale, msg)`.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

let cachedModule = null;
async function loadDetailModule() {
  if (!cachedModule) cachedModule = buildDetailModule();
  return cachedModule;
}

async function buildDetailModule() {
  const i18nSource = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const localesDir = new URL("../src/locales/", import.meta.url);
  const inlinedI18n = await inlineLocaleImports(i18nSource, localesDir);

  const formatSource = await readFile(new URL("../src/format.ts", import.meta.url), "utf8");
  const strippedFormat = formatSource.replace('import { localeTag, plural, t } from "./i18n";', "");
  if (strippedFormat === formatSource) throw new Error("no substitution matched -- src/format.ts's source shape moved under this test");

  // src/focus.ts has no imports of its own, so it is appended as is --
  // detail.ts's own `import { focusOrFallback } from "./focus";` is dropped
  // below, since this one copy already puts it in scope.
  const focusSource = await readFile(new URL("../src/focus.ts", import.meta.url), "utf8");

  // src/panels.ts has no imports of its own either -- detail.ts's own
  // `import { focusAfterClose, isTopPanel, syncPanels } from "./panels";` is
  // dropped below for the same reason.
  const panelsSource = await readFile(new URL("../src/panels.ts", import.meta.url), "utf8");

  const detailSource = await readFile(new URL("../src/detail.ts", import.meta.url), "utf8");
  const strippedDetail = detailSource
    // A plain object, not a bare stub function, so a test can redirect it
    // per call (scripts/detail-focus.test.mjs's own saveClientRules() test
    // needs `invoke("save_clients", …)` to actually resolve) without
    // rebuilding this whole combined module -- rebuilding is expensive
    // (loadDetailModule() memoizes it in `cachedModule` for exactly that
    // reason) and would also lose the pristine-module-state guarantee later
    // tests in this file rely on. Defaults to the original throwing stub.
    .replace(
      'import { invoke } from "@tauri-apps/api/core";',
      'export const __invoke = { current: async () => { throw new Error("invoke() is not stubbed in this test"); } };\nconst invoke = (...args) => __invoke.current(...args);',
    )
    .replace('import { focusOrFallback } from "./focus";', "")
    .replace('import { focusAfterClose, isTopPanel, syncPanels } from "./panels";', "")
    .replace('import { displayMetricDetail, displayMetricLabel, localeTag, plural, t } from "./i18n";', "")
    .replace('import { money, relativeActivity, tokens } from "./format";', "")
    // src/i18n.ts exports its own top-level `render(locale, msg)`; detail.ts's
    // own private `function render(): void` would otherwise collide with it
    // once the two sources are concatenated -- the same rename
    // scripts/agents-view.test.mjs applies to src/agents.ts's own `render`.
    .replace(/\brender\b/g, "__detailRender");
  if (strippedDetail === detailSource) throw new Error("no substitution matched -- src/detail.ts's source shape moved under this test");

  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${focusSource}\n${panelsSource}\n${strippedDetail}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

// closeFocusTarget() (src/detail.ts) is the pure decision close() makes about
// where to send focus -- the same convention every view here follows (see
// audit.ts's own comment: each view keeps a private copy). Detail's own
// close() passes `null` as the fallback (no single fixed button makes sense
// when dozens of cards can open this page), so the "no opener, no fallback"
// branch is the realistic path here, not just an edge case.
test("closeFocusTarget: returns to the opener when it is still in the document, else the fallback, else nothing", async () => {
  const { closeFocusTarget } = await loadDetailModule();
  const opener = { tag: "opener" };
  const fallback = { tag: "fallback" };
  assert.equal(closeFocusTarget(opener, true, fallback), opener, "a live opener should win over the fallback");
  assert.equal(closeFocusTarget(opener, false, fallback), fallback, "a detached opener should fall back");
  assert.equal(closeFocusTarget(null, false, fallback), fallback, "no opener at all should fall back");
  assert.equal(closeFocusTarget(null, false, null), null, "no opener and no fallback (Detail's own real case) should return null, not throw");
});

/** A fake element whose .focus() moves the fake document's activeElement --
 *  same shape scripts/focus.test.mjs's own fakeElement() uses, needed here
 *  too since this test exercises the real focusOrFallback() (inlined into
 *  this same combined module) rather than a mock of it. */
function fakeFocusable(doc) {
  const el = { focus: () => { doc.activeElement = el; } };
  return el;
}

// The #dt-rule-save click handler's own body (src/detail.ts's
// saveClientRules(), pulled out of the click-delegate chain precisely so it
// can be called directly here): a fixed bug had it calling
// `document.querySelector("#dt-rule-edit")?.focus()` bare, after the
// asynchronous save+reload -- silently landing focus nowhere at all
// whenever that button was not there any more, e.g. grouping left "client"
// while the save was in flight. openId stays null (the module's own default
// boot state) for both cases below, which sends loadClients() down its own
// early-return path (`if (!sp) return;`, `sp` derived from `openId`) without
// calling invoke() or render() a second time -- so the only invoke() call
// either test needs to stub is "save_clients" itself.
test("saveClientRules: falls back instead of landing focus nowhere when #dt-rule-edit is gone by the time the save settles", async () => {
  const { saveClientRules, __invoke } = await loadDetailModule();
  const doc = { activeElement: { tag: "whatever-had-focus-before" } };
  doc.body = fakeFocusable(doc);
  // #dt-rule-edit is absent from this "document" -- the exact real-world
  // case (grouping changed while the request was in flight).
  doc.querySelector = () => null;
  globalThis.document = doc;
  __invoke.current = async (cmd) => {
    assert.equal(cmd, "save_clients");
    return undefined;
  };
  try {
    await saveClientRules([{ client: "Acme", patterns: ["acme/*"] }]);
    assert.equal(doc.activeElement, doc.body, "with no #dt-rule-edit to land on, focus must fall back to document.body, never nowhere");
  } finally {
    delete globalThis.document;
    __invoke.current = async () => { throw new Error("invoke() is not stubbed in this test"); };
  }
});

test("saveClientRules: focuses #dt-rule-edit directly when it is there", async () => {
  const { saveClientRules, __invoke } = await loadDetailModule();
  const doc = { activeElement: { tag: "whatever-had-focus-before" } };
  doc.body = fakeFocusable(doc);
  const editButton = fakeFocusable(doc);
  doc.querySelector = (sel) => (sel === "#dt-rule-edit" ? editButton : null);
  globalThis.document = doc;
  __invoke.current = async () => undefined;
  try {
    await saveClientRules([]);
    assert.equal(doc.activeElement, editButton, "the real #dt-rule-edit button should win over the document.body fallback");
  } finally {
    delete globalThis.document;
    __invoke.current = async () => { throw new Error("invoke() is not stubbed in this test"); };
  }
});

test("in wide mode Escape takes focus from the page back to the list, only when nothing covers the list", async () => {
  const { escapeReturnsToList } = await loadDetailModule();
  assert.equal(escapeReturnsToList(true, true, true, false), true);
  assert.equal(escapeReturnsToList(false, true, true, false), false, "narrow: Escape closes the page instead");
  assert.equal(escapeReturnsToList(true, false, true, false), false, "no page open");
  assert.equal(escapeReturnsToList(true, true, false, false), false, "focus is already on the list: Escape does its usual job");
  assert.equal(escapeReturnsToList(true, true, true, true), false, "a panel makes the list inert, and Escape closes the panel");
});
