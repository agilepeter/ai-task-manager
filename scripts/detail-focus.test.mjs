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

  const detailSource = await readFile(new URL("../src/detail.ts", import.meta.url), "utf8");
  const strippedDetail = detailSource
    .replace('import { invoke } from "@tauri-apps/api/core";', 'const invoke = async () => { throw new Error("invoke() is not stubbed in this test"); };')
    .replace('import { focusOrFallback } from "./focus";', "")
    .replace('import { displayMetricDetail, displayMetricLabel, localeTag, plural, t } from "./i18n";', "")
    .replace('import { money, relativeActivity, tokens } from "./format";', "")
    // src/i18n.ts exports its own top-level `render(locale, msg)`; detail.ts's
    // own private `function render(): void` would otherwise collide with it
    // once the two sources are concatenated -- the same rename
    // scripts/agents-view.test.mjs applies to src/agents.ts's own `render`.
    .replace(/\brender\b/g, "__detailRender");
  if (strippedDetail === detailSource) throw new Error("no substitution matched -- src/detail.ts's source shape moved under this test");

  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${focusSource}\n${strippedDetail}`, {
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
