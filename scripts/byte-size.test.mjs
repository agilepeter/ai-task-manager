// byteSize() (src/format.ts) turns a raw byte count -- the MCP usage chip's
// 30-day result size -- into "N KB/MB/GB". Two edges the plain "KB under 1
// MB" arithmetic got wrong on its own: zero used to read "1 KB" because the
// KB branch's own floor (`Math.max(1, …)`) exists to keep a handful of real
// bytes from reading "0 KB", and applied to an actual zero it lied the same
// way; and anything that cannot be a byte count at all (not finite, or
// negative) used to still print a KB/MB/GB string instead of admitting it
// has nothing honest to show. This checks both edges plus the ordinary
// KB/MB/GB boundaries around them.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

// Same trick as scripts/mcp-usage-note.test.mjs: strip src/format.ts's own
// `./i18n` import (byteSize() never touches localeTag/plural/t) and
// transpile the rest to plain JS for a data-URI import.
let cachedModule = null;
async function loadFormatModule() {
  if (!cachedModule) cachedModule = buildFormatModule();
  return cachedModule;
}

async function buildFormatModule() {
  const formatSource = await readFile(new URL("../src/format.ts", import.meta.url), "utf8");
  const stripped = formatSource.replace('import { localeTag, plural, t } from "./i18n";', "");
  if (stripped === formatSource) throw new Error("no substitution matched -- src/format.ts's source shape moved under this test");
  const code = ts.transpileModule(stripped, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

test("byteSize", async (t) => {
  const { byteSize } = await loadFormatModule();

  await t.test("0 bytes reads 0 KB, not 1 KB from the KB floor", () => {
    assert.equal(byteSize(0), "0 KB");
  });
  await t.test("1 byte still rounds up to 1 KB", () => {
    assert.equal(byteSize(1), "1 KB");
  });
  await t.test("1023 bytes rounds to 1 KB", () => {
    assert.equal(byteSize(1023), "1 KB");
  });
  await t.test("1024 bytes (1 KB) stays under the MB threshold", () => {
    assert.equal(byteSize(1024), "1 KB");
  });
  await t.test("1536 bytes rounds to 2 KB", () => {
    assert.equal(byteSize(1536), "2 KB");
  });
  await t.test("1048576 bytes (1 MB) crosses into MB", () => {
    assert.equal(byteSize(1048576), "1.0 MB");
  });
  await t.test("5 GB crosses into GB", () => {
    assert.equal(byteSize(5 * 1024 ** 3), "5.0 GB");
  });
  await t.test("NaN is not a byte count -- empty string, no chip", () => {
    assert.equal(byteSize(NaN), "");
  });
  await t.test("a negative count is not a byte count -- empty string, no chip", () => {
    assert.equal(byteSize(-1), "");
  });
});
