// The demo's hand-authored findings (src/demo/synthetic.ts) build their own Msgs by hand,
// keyed and varred to match the Rust modules' real sentences (crates/core/src/procs.rs for the
// duplicate-process row, coaching.rs for the usage rows it lifts out of the audit, limit_time.rs
// and agent_watch.rs for the two it builds from the demo's own state) -- nothing on either side
// checks that the two agree. A var-name typo here leaves a literal "{worstName}" (or whichever
// var drifted) sitting in the rendered sentence, in every locale, silently: this renders the
// duplicate-process row and the lifted usage rows in every registered locale and fails if any
// title or detail still has an unfilled {var}, or came back as its own raw key (t()'s fallback
// for a key that resolves nowhere). The two built from state are rendered the same way in
// scripts/demo-limit-time.test.mjs and scripts/demo-agent-watch.test.mjs, beside what they hold
// them to. The last two tests here hold the list of findings that carry no Learn more link to
// the Rust sources.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { loadDemoBackend } from "./demo-backend.mjs";

// One instance of the demo's modules for the whole file: loading joins i18n.ts's nine dictionaries
// and the two demo sources into one module (scripts/demo-backend.mjs). The tests here pick the
// active language themselves.
let demo = null;
const loadDemo = () => (demo ??= loadDemoBackend());

// Derived from i18n.ts's own export (not hand-copied), so a newly added
// locale is covered here automatically instead of silently getting no
// synthetic-row coverage.
const { LOCALES } = await loadDemo();

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
    const { buildUsageRows, render, setActiveLocale } = await loadDemo();
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
    const { buildDuplicateProcessesRow, render, setActiveLocale } = await loadDemo();
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
  const { buildDuplicateProcessesRow } = await loadDemo();
  assert.equal(buildDuplicateProcessesRow([{ name: "solo", instances: 1, rssBytes: 1048576 }]), null);
});

// mcp-remote's audit check and Inventory opportunity share one id, so plain
// id equality already keeps it from being lifted twice -- that case is
// covered by "already-listed" above. agent-model and agents-model-unset
// are the pair that does NOT share an id (see AUDIT_ID_ALIASES's own
// comment for why), so it needs its own case: without the alias, this
// fixture would wrongly lift a second "inherits the session model" card.
test("buildUsageRows() skips a check aliased to an existing Inventory opportunity under a different id", async () => {
  const { buildUsageRows, AUDIT_ID_ALIASES } = await loadDemo();
  assert.equal(
    AUDIT_ID_ALIASES["agent-model"],
    "agents-model-unset",
    "this test's fixture below assumes agent-model's alias is agents-model-unset",
  );
  const sections = [
    {
      checks: [
        {
          id: "agent-model",
          status: "consider",
          title: "1 agent inherits whatever model runs it",
          detail: "An agent with no model line runs on the caller's model, often the most expensive one. Pin a cheaper model where the task allows it.",
          titleMsg: { key: "check.agent-model.consider.title", vars: {}, count: 1 },
          detailMsg: { key: "check.agent-model.consider.detail", vars: {}, count: null },
        },
        {
          id: "mix-top-heavy",
          status: "consider",
          title: "80% of spend is on the largest models",
          detail: "$1063 of $1329 in 30 days went to the top tier.",
          titleMsg: { key: "finding.mix-top-heavy.title", vars: { pct: "80" }, count: null },
          detailMsg: { key: "finding.mix-top-heavy.detail", vars: { top: "$1063", known: "$1329" }, count: null },
        },
      ],
    },
  ];
  // opportunities already carry agents-model-unset (the Inventory-tab
  // finding), never agent-model itself -- the two ids only line up
  // through AUDIT_ID_ALIASES.
  const rows = buildUsageRows(sections, new Set(["agents-model-unset"]));
  assert.deepEqual(
    rows.map((r) => r.id).sort(),
    ["mix-top-heavy"],
    "agent-model must be skipped via its agents-model-unset alias; mix-top-heavy must still be lifted",
  );
});

// An audit check carries no link of its own, so buildUsageRows() has to say which of the
// findings it lifts into the Inventory tab the engine builds without a Learn more link.
// NO_LEARN_LINK is that list; the Rust sources are what it is held to.
test("buildUsageRows() gives a finding the engine builds without a link none, and a coaching one the classroom's", async () => {
  const { buildUsageRows, NO_LEARN_LINK } = await loadDemo();
  const check = (id) => ({
    id,
    status: "consider",
    title: "t",
    detail: "d",
    titleMsg: { key: "finding.mix-top-heavy.title", vars: { pct: "1" }, count: null },
    detailMsg: null,
  });
  const ids = [...NO_LEARN_LINK, "mix-top-heavy", "session-long-lived"];
  const rows = buildUsageRows([{ checks: ids.map(check) }], new Set());
  const expected = Object.fromEntries(ids.map((id) => [id, NO_LEARN_LINK.has(id) ? null : "https://staas.fund/classroom/"]));
  assert.deepEqual(Object.fromEntries(rows.map((r) => [r.id, r.learnUrl])), expected);
  assert.equal(expected["mix-top-heavy"], "https://staas.fund/classroom/", "a coaching finding still has its link");
});

/** The arguments of every `Opportunity::from_msgs(id, kind, title, detail, learn_url)` call in `source`,
 *  split at the top level of the call: strings, parentheses, brackets and braces are skipped over. */
function fromMsgsCalls(source) {
  const needle = "Opportunity::from_msgs(";
  const calls = [];
  for (let at = source.indexOf(needle); at !== -1; at = source.indexOf(needle, at + needle.length)) {
    const args = [];
    let depth = 0;
    let start = at + needle.length;
    let i = start;
    for (; i < source.length; i++) {
      const c = source[i];
      if (c === '"') {
        for (i++; source[i] !== '"'; i++) if (source[i] === "\\") i++;
      } else if (c === "(" || c === "[" || c === "{") depth++;
      else if (c === "]" || c === "}") depth--;
      else if (c === ")") {
        if (depth === 0) break;
        depth--;
      } else if (c === "," && depth === 0) {
        args.push(source.slice(start, i).trim());
        start = i + 1;
      }
    }
    const last = source.slice(start, i).trim();
    if (last) args.push(last);
    calls.push(args);
  }
  return calls;
}

test("the scanner reads a from_msgs call the way the Rust is written", () => {
  const source = `
    out.push(Opportunity::from_msgs(
        "agent-over-budget",
        "learn",
        Msg::new("finding.x.title").count(n),
        Some(Msg::new("finding.x.detail").var("names", format!("{} ({})", a, b))),
        None,
    ));
    vec![Opportunity::from_msgs("limit-time", "learn", title, Some(detail), None)]
    Opportunity::from_msgs(id, kind, title_msg, Some(detail_msg), Some(CLASSROOM))`;
  assert.deepEqual(fromMsgsCalls(source).map((args) => [args.length, args[0], args.at(-1)]), [
    [5, '"agent-over-budget"', "None"],
    [5, '"limit-time"', "None"],
    [5, "id", "Some(CLASSROOM)"],
  ]);
});

test("NO_LEARN_LINK is every finding the two Rust modules build with no link, and nothing else", async () => {
  const { NO_LEARN_LINK } = await loadDemo();
  const source = (file) => readFile(new URL(`../crates/core/src/${file}`, import.meta.url), "utf8");
  // The modules whose findings reach the demo through the audit alone, without their test module
  // (a `#[cfg(test)]` on a constant higher up is not where the tests begin).
  const noLink = new Set();
  for (const file of ["agent_watch.rs", "changes.rs"]) {
    const [production, ...rest] = (await source(file)).split(/#\[cfg\(test\)\]\s*mod tests\b/);
    assert.equal(rest.length, 1, `${file}: expected one test module to cut at`);
    const calls = fromMsgsCalls(production);
    assert.ok(calls.length > 0, `${file}: no from_msgs call found, so the scanner or the file moved`);
    for (const args of calls) {
      assert.equal(args.length, 5, `${file}: a from_msgs call with ${args.length} arguments`);
      const id = args[0].match(/^"([a-z0-9-]+)"$/);
      assert.ok(id, `${file}: a finding built with an id that is not a string (${args[0]}): this test cannot say what link it has`);
      if (args.at(-1) === "None") noLink.add(id[1]);
    }
  }
  assert.deepEqual([...noLink].sort(), [...NO_LEARN_LINK].sort(), "a finding built with no link must be listed, and a listed one must still be built so");

  // Two more findings have no link and are not listed, because neither passes through buildUsageRows:
  // agent-unused arrives in the fixture's own opportunities, with its null link, and limit-time is
  // built by buildLimitTimeRow (its link is held in scripts/demo-limit-time.test.mjs).
  const fixture = JSON.parse(await readFile(new URL("../src/demo-fixture.json", import.meta.url), "utf8"));
  const unused = fixture.inventory.opportunities.find((o) => o.id === "agent-unused");
  assert.ok(unused, "the fixture's own opportunities carry agent-unused, or this note about it no longer holds");
  assert.equal(unused.learnUrl, null);
  // What every other lifted check is made of: coaching findings, which carry the classroom link.
  const coaching = await source("coaching.rs");
  assert.match(coaching, /const CLASSROOM: &str = "https:\/\/staas\.fund\/classroom\/";/);
  assert.match(coaching, /Some\(CLASSROOM\)/);
});
