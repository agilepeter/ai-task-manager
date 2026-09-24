// The demo's two hand-authored findings (src/demo/synthetic.ts) build their
// own Msgs by hand, keyed and varred to match crates/core/src/procs.rs's and
// crates/core/src/coaching.rs's real sentences -- nothing on either side
// checks that the two agree. A var-name typo here leaves a literal
// "{worstName}" (or whichever var drifted) sitting in the rendered sentence,
// in every locale, silently: this renders every synthetic row in every
// registered locale and fails if any title or detail still has an unfilled
// {var}, or came back as its own raw key (t()'s fallback for a key that
// resolves nowhere).
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

// inlineLocaleImports() (scripts/inline-locales.mjs) inlines src/i18n.ts's
// dictionary imports as object literals, since ts.transpileModule handles
// one file at a time and Node's loader cannot resolve a relative specifier
// off a data: URL; this extends that to also load src/demo/synthetic.ts:
// its own `import { render, type Msg } from "../i18n"` is stripped and the
// file is appended after i18n's already-inlined source, so the combined
// text transpiles as a single self-contained module with both files'
// exports.
async function loadSyntheticModule() {
  const i18nSource = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const localesDir = new URL("../src/locales/", import.meta.url);
  const inlined = await inlineLocaleImports(i18nSource, localesDir);
  const syntheticSource = await readFile(new URL("../src/demo/synthetic.ts", import.meta.url), "utf8");
  const withoutI18nImport = syntheticSource.replace(/^import\s*\{[^}]*\}\s*from\s*["']\.\.\/i18n["'];\s*$/m, "");
  const code = ts.transpileModule(`${inlined}\n${withoutI18nImport}`, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

// Derived from i18n.ts's own export (not hand-copied), so a newly added
// locale is covered here automatically instead of silently getting no
// synthetic-row coverage.
const { LOCALES } = await loadSyntheticModule();

// A fixed, self-contained fixture rather than src/demo-fixture.json, so this
// test always exercises both builders' var wiring regardless of whether a
// given regeneration happens to produce any "consider" checks or any
// duplicate process. Keys and vars are copied from coaching.rs/procs.rs's
// real construction, not invented, so a real rendering is what gets checked.
const RUNNING = [
  { name: "chrome-devtools", instances: 3, rssBytes: 812 * 1048576 },
  { name: "notes", instances: 2, rssBytes: 276 * 1048576 },
  { name: "playwright", instances: 1, rssBytes: 188 * 1048576 },
];
const SECTIONS = [
  {
    checks: [
      {
        id: "mix-top-heavy",
        status: "consider",
        title: "80% of spend is on the largest models",
        detail: "$1063 of $1329 in 30 days went to the top tier.",
        titleMsg: { key: "finding.mix-top-heavy.title", vars: { pct: "80" }, count: null },
        detailMsg: { key: "finding.mix-top-heavy.detail", vars: { top: "$1063", known: "$1329" }, count: null },
      },
      {
        id: "session-long-lived",
        status: "consider",
        title: "One session has been open 18 days",
        detail: "It cost $42 in the last 30 days.",
        titleMsg: { key: "finding.session-long-lived.title", vars: { days: "18" }, count: null },
        detailMsg: { key: "finding.session-long-lived.detail", vars: { cost: "$42" }, count: null },
      },
      // Excluded by status: "attention" checks are shown elsewhere already.
      {
        id: "mcp-unpinned",
        status: "attention",
        title: "irrelevant",
        detail: "irrelevant",
        titleMsg: { key: "check.mcp-unpinned.pass.title", vars: {}, count: null },
        detailMsg: null,
      },
      // Excluded by id: already present among the fixture's own opportunities.
      {
        id: "already-listed",
        status: "consider",
        title: "irrelevant",
        detail: "irrelevant",
        titleMsg: { key: "finding.mix-top-heavy.title", vars: { pct: "1" }, count: null },
        detailMsg: null,
      },
    ],
  },
];

function hasLeftoverBraces(s) {
  return /[{}]/.test(s);
}

function assertRendersCleanly(locale, label, msg, render) {
  const rendered = render(locale, msg);
  assert.ok(!hasLeftoverBraces(rendered), `${locale} ${label} left a {var} unfilled: "${rendered}"`);
  assert.notEqual(rendered, msg.key, `${locale} ${label} rendered as its own raw key`);
}

for (const locale of LOCALES) {
  test(`buildUsageRows() renders cleanly in ${locale}`, async () => {
    const { buildUsageRows, render, setActiveLocale } = await loadSyntheticModule();
    setActiveLocale(locale);
    const rows = buildUsageRows(SECTIONS, new Set(["already-listed"]));
    assert.deepEqual(
      rows.map((r) => r.id).sort(),
      ["mix-top-heavy", "session-long-lived"],
      "wrong-status and already-listed checks must both be excluded",
    );
    for (const row of rows) {
      assertRendersCleanly(locale, `${row.id}.titleMsg`, row.titleMsg, render);
      if (row.detailMsg) assertRendersCleanly(locale, `${row.id}.detailMsg`, row.detailMsg, render);
    }
  });

  test(`buildDuplicateProcessesRow() renders cleanly in ${locale}`, async () => {
    const { buildDuplicateProcessesRow, render, setActiveLocale } = await loadSyntheticModule();
    setActiveLocale(locale);
    const row = buildDuplicateProcessesRow(RUNNING);
    assert.ok(row, "two RUNNING entries have instances > 1, so a row must be built");
    assertRendersCleanly(locale, "titleMsg", row.titleMsg, render);
    assertRendersCleanly(locale, "detailMsg", row.detailMsg, render);
    // The nested unit.times nested Msg lives inside detailMsg.vars.times --
    // covered by assertRendersCleanly above via render()'s own recursion,
    // but checked explicitly too since it is the one var a flat pass would
    // miss entirely (JSON.stringify would show it, a plain render would not
    // catch a drift confined to just that nested key).
    assertRendersCleanly(locale, "detailMsg.vars.times", row.detailMsg.vars.times, render);
  });
}

test("buildDuplicateProcessesRow() returns null when nothing is running more than once", async () => {
  const { buildDuplicateProcessesRow } = await loadSyntheticModule();
  assert.equal(buildDuplicateProcessesRow([{ name: "solo", instances: 1, rssBytes: 1048576 }]), null);
});
