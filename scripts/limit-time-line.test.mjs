// The Detail page's "time at the limit" lines (src/detail.ts's
// limitTimeSection()): a pure function from the rows get_limit_time returns to
// markup, tested the way scripts/detail-focus.test.mjs tests the rest of that
// module, through the same combined-module build.
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
  const inlinedI18n = await inlineLocaleImports(i18nSource, new URL("../src/locales/", import.meta.url));
  const formatSource = await readFile(new URL("../src/format.ts", import.meta.url), "utf8");
  const strippedFormat = formatSource.replace('import { localeTag, plural, t } from "./i18n";', "");
  if (strippedFormat === formatSource) throw new Error("src/format.ts's import line moved under this test");
  const focusSource = await readFile(new URL("../src/focus.ts", import.meta.url), "utf8");
  const panelsSource = await readFile(new URL("../src/panels.ts", import.meta.url), "utf8");
  const detailSource = await readFile(new URL("../src/detail.ts", import.meta.url), "utf8");
  const strippedDetail = detailSource
    .replace('import { invoke } from "@tauri-apps/api/core";', "const invoke = async () => { throw new Error(\"no invoke in this test\"); };")
    .replace('import { focusOrFallback } from "./focus";', "")
    .replace('import { focusAfterClose, isTopPanel, syncPanels } from "./panels";', "")
    .replace('import { displayMetricDetail, displayMetricLabel, localeTag, plural, t } from "./i18n";', "")
    .replace('import { money, relativeActivity, tokens } from "./format";', "")
    .replace(/\brender\b/g, "__detailRender");
  if (strippedDetail === detailSource) throw new Error("src/detail.ts's source shape moved under this test");
  const code = ts.transpileModule(`${inlinedI18n}\n${strippedFormat}\n${focusSource}\n${panelsSource}\n${strippedDetail}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

const MIN = 60_000;
const row = (metric, times, totalMin) => ({ provider: "claude", metric, times, totalMs: totalMin * MIN, longestMs: 0 });

test("the limit time line shows only when a limit was reached", async () => {
  const { limitTimeSection, setActiveLocale } = await loadDetailModule();
  setActiveLocale("en");
  assert.equal(limitTimeSection([]), "", "nothing reached: no heading, no 'never' line, no empty box");
  assert.equal(limitTimeSection([row("Weekly", 0, 0)]), "", "a row that never reached the limit is not a line");

  const html = limitTimeSection([row("Session", 1, 40), row("Weekly", 3, 130), row("Opus weekly", 2, 1620)]);
  const lines = [...html.matchAll(/<p>(.*?)<\/p>/g)].map((m) => m[1]);
  assert.deepEqual(lines, [
    "Opus weekly: at 100% 2 times in the last 30 days, 1d 3h in all",
    "Weekly: at 100% 3 times in the last 30 days, 2h 10m in all",
    "Session: at 100% 1 time in the last 30 days, 40m in all",
  ], "most time first, the count through the plural machinery, the length through the time.* keys");

  // Another language words and orders it itself; no English unit survives.
  setActiveLocale("ru");
  try {
    const ru = limitTimeSection([row("Weekly", 2, 130)]);
    assert.match(ru, /2 раза/);
    assert.match(ru, /2ч 10м/);
    assert.doesNotMatch(ru, /times|\{/);
  } finally {
    setActiveLocale("en");
  }
});

test("a hostile metric label in the limit time line renders as text", async () => {
  const { limitTimeSection, setActiveLocale } = await loadDetailModule();
  setActiveLocale("en");
  const hostile = `<img src=x onerror="alert(1)">'&`;
  const html = limitTimeSection([row(hostile, 1, 30)]);
  assert.ok(!html.includes("<img"), html);
  assert.ok(html.includes("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;&#39;&amp;"), html);
  assert.equal((html.match(/</g) ?? []).length, 4, "only the wrapper div and the one paragraph, opened and closed");
});
