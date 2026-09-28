// The Audit panel's own reload-freshness rule (src/audit.ts's shouldReload(),
// mirroring src/agents.ts's identical function -- each view keeps its own
// tiny copy rather than share one, same convention as esc() and
// closeFocusTarget() elsewhere in both files). Reopening the popover happens
// far more often than the underlying setup actually changes, so
// reloadAudit(false) (the popover-shown path) skips its own get_audit() call
// within this window; reloadAudit(true) (the rescan path) always applies,
// since a rescan is a real change, not a "maybe".
//
// src/i18n.ts's locale JSON is inlined (same trick scripts/agents-view.test.mjs
// uses) rather than dropped: the shouldReload() tests below never touch t()/
// tm()/plural(), but the behavioural tests further down call openAudit() and
// reloadAudit() for real, which paint through render() -- and render() calls
// T() (this file's own `t("audit.…")` alias) throughout. Dropping i18n
// entirely, the way this file used to, left `t`/`tm`/`plural` as free
// identifiers that would throw the moment anything actually rendered.
// audit.ts's own top-level `function render(): void` (the DOM orchestrator)
// is renamed the same way src/inventory.ts's and src/agents.ts's are in
// scripts/agents-view.test.mjs, and for the identical reason: left in place,
// it would collide with i18n's own exported `render(locale, msg)` once both
// sources are concatenated, and whichever one loses would either break every
// t()/tm() call in this file or, if only the declaration moved without its
// call sites, leave `render()`'s own bare calls (inside openAudit(),
// reloadAudit(), rerender(), etc.) calling the wrong function with the wrong
// arity. One `\brender\(\)` rename fixes the declaration and every call site
// in a single pass -- it can never touch `rerender(` (no word boundary before
// "render" there) or any other `renderXxx(` identifier (not followed
// immediately by "()").
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

let cachedModule = null;
async function loadAuditModule() {
  if (!cachedModule) cachedModule = buildAuditModule();
  return cachedModule;
}

async function buildAuditModule() {
  const i18nSource = await readFile(new URL("../src/i18n.ts", import.meta.url), "utf8");
  const localesDir = new URL("../src/locales/", import.meta.url);
  const inlinedI18n = await inlineLocaleImports(i18nSource, localesDir);

  const auditSource = await readFile(new URL("../src/audit.ts", import.meta.url), "utf8");
  const stripped = auditSource
    .replace('import { invoke } from "@tauri-apps/api/core";', "")
    .replace('import { plural, t, tm, type Msg } from "./i18n";', "")
    // A whole-word rename (declaration and every call site, bare `render()`
    // or a bare callback reference alike -- audit.ts has none of the latter
    // today, but the pattern costs nothing and stays correct either way).
    .replace(/\brender\b/g, "__unusedAuditRender");
  if (stripped === auditSource) throw new Error("no substitution matched -- src/audit.ts's source shape moved under this test");
  const code = ts.transpileModule(`${inlinedI18n}\n${stripped}`, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

// A minimal stand-in for `document`, the same shape scripts/agents-view.test.mjs
// uses for the same reason: just enough for openAudit() / reloadAudit() /
// render() to run end to end without a real DOM. One persistent fake element
// per selector (so a write in one call and a read in a later one see the same
// object), a real toggleable body classList (isOpen() reads "audit-open" back
// off it), and the handful of members audit.ts's code actually calls on an
// element (`.innerHTML`, `.focus()`, `.addEventListener()`, and, since
// openAudit()/close() now toggle the panel's `inert` attribute alongside the
// body class -- see src/audit.ts's own comment -- `.setAttribute()` /
// `.removeAttribute()` as no-ops too).
function makeFakeDocument() {
  const bodyClasses = new Set();
  const elements = new Map();
  function elementFor(selector) {
    if (!elements.has(selector)) {
      elements.set(selector, {
        innerHTML: "",
        focus() {},
        addEventListener() {},
        setAttribute() {},
        removeAttribute() {},
        classList: { contains: () => false, add() {}, remove() {} },
      });
    }
    return elements.get(selector);
  }
  return {
    elements,
    body: {
      classList: {
        contains: (c) => bodyClasses.has(c),
        add: (c) => bodyClasses.add(c),
        remove: (c) => bodyClasses.delete(c),
      },
    },
    querySelector: (selector) => elementFor(selector),
    querySelectorAll: () => [],
    contains: () => false,
    addEventListener: () => {},
  };
}

// A recording `invoke()` stand-in, same as scripts/agents-view.test.mjs's own
// copy: every call is pushed to `calls` before it resolves (or rejects) from
// `fixtures`; `delays[cmd]`, when given, is awaited first, so a test can prove
// something really did wait for a call to settle rather than merely running
// after it by coincidence.
function makeRecordingInvoke(fixtures, delays = {}) {
  const calls = [];
  const invoke = async (cmd, args) => {
    calls.push(cmd);
    if (delays[cmd]) await delays[cmd]();
    if (!(cmd in fixtures)) throw new Error(`makeRecordingInvoke: no fixture registered for "${cmd}"`);
    const v = fixtures[cmd];
    if (v instanceof Error) throw v;
    return typeof v === "function" ? v(args) : v;
  };
  return { calls, invoke };
}

// Drains the microtask queue completely -- see scripts/agents-view.test.mjs's
// own copy for why a setTimeout callback, not a fixed `.then()` chain, is the
// reliable way to say "let everything already in flight finish".
function flushMicrotasks() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

function emptyReport(overrides = {}) {
  return { generatedAt: 0, passed: 0, attention: 0, score: null, sections: [], ...overrides };
}

test("audit.ts shouldReload(): closed never reloads, open+fresh does not, open+stale does, a backwards clock reloads, loading blocks a second load, a failure retries at once", async () => {
  const { shouldReload, RELOAD_FRESHNESS_MS } = await loadAuditModule();
  const now = 1_790_000_000_000;

  assert.equal(shouldReload(now - 1, now, false, false, false), false, "a closed panel must never reload, no matter how stale");
  assert.equal(shouldReload(now - 1, now, false, false, true), false, "closed must never reload even with a failed last load");
  assert.equal(shouldReload(now, now, true, false, false), false, "just succeeded (age 0) must not reload");
  assert.equal(shouldReload(now - 10_000, now, true, false, false), false, "succeeded 10s ago must not reload");
  assert.equal(shouldReload(now - (RELOAD_FRESHNESS_MS - 1), now, true, false, false), false, "one millisecond inside the freshness window must not reload");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS, now, true, false, false), true, "exactly at the freshness window must reload");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS - 1, now, true, false, false), true, "past the freshness window must reload");
  assert.equal(shouldReload(now - 61_000, now, true, false, false), true, "succeeded 61s ago must reload");
  assert.equal(shouldReload(now + 1, now, true, false, false), true, "a clock that moved backwards must reload rather than trust the (negative) age");
  assert.equal(shouldReload(now, now, true, true, false), false, "a load already in flight must never be joined by a second one");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS - 1, now, true, true, false), false, "loading must block a reload even when the last success is stale");
  assert.equal(shouldReload(now, now, true, false, true), true, "a failed load must retry at once, even though its own timestamp reads as fresh");
  assert.equal(shouldReload(now - RELOAD_FRESHNESS_MS - 1, now, true, false, true), true, "a failed, stale load must also reload");
});

// render() (src/audit.ts) must not write into #audit-body once the panel has
// closed -- a get_audit() call still in flight when the panel closed must not
// paint over it when it lands late. Exercised as the real race: openAudit()
// fires get_audit but its promise is held back; the panel is closed before it
// settles; once it does settle, #audit-body must still read whatever it held
// at close time, never the freshly loaded report.
test("render() bails out before touching the DOM when the panel is closed", async () => {
  const { openAudit } = await loadAuditModule();
  const fakeDocument = makeFakeDocument();
  globalThis.document = fakeDocument;

  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const lateReport = emptyReport({
    attention: 1,
    sections: [{ name: "Setup", checks: [{ id: "x", status: "attention", title: "late-check", detail: "" }] }],
  });
  const { invoke } = makeRecordingInvoke({ get_audit: lateReport }, { get_audit: () => held });
  globalThis.invoke = invoke;

  try {
    openAudit();
    const bodyWhileLoading = fakeDocument.elements.get("#audit-body").innerHTML;
    assert.ok(!bodyWhileLoading.includes("late-check"), "get_audit resolved before this test released it");

    fakeDocument.body.classList.remove("audit-open"); // the user closed the panel
    release();
    await flushMicrotasks();

    const bodyAfterClose = fakeDocument.elements.get("#audit-body").innerHTML;
    assert.equal(bodyAfterClose, bodyWhileLoading, "render() must not overwrite #audit-body once the panel has closed, even for a load that was already in flight");
    assert.ok(!bodyAfterClose.includes("late-check"), "a load that settled after close must never reach the DOM");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

// reloadAudit(force) must always proceed when force is true (a rescan is a
// real change, never a "maybe") and must consult shouldReload() when it is
// not (the popover-shown path). Exercised for real: openAudit() first (its
// own initial get_audit call), then reloadAudit(false) right after -- still
// fresh, so no new call -- then reloadAudit(true), which must cause a new
// call regardless.
test("reloadAudit(): force bypasses shouldReload(), the default does not", async () => {
  const { openAudit, reloadAudit } = await loadAuditModule();
  const fakeDocument = makeFakeDocument();
  globalThis.document = fakeDocument;
  const { calls, invoke } = makeRecordingInvoke({ get_audit: emptyReport({ passed: 1, score: 100 }) });
  globalThis.invoke = invoke;

  try {
    openAudit();
    await flushMicrotasks();
    assert.deepEqual(calls, ["get_audit"], "openAudit() must make its own initial get_audit call");
    calls.length = 0;

    reloadAudit(false);
    await flushMicrotasks();
    assert.deepEqual(calls, [], "reloadAudit(false) right after a fresh load must not re-invoke get_audit");

    reloadAudit(true);
    await flushMicrotasks();
    assert.deepEqual(calls, ["get_audit"], "reloadAudit(true) must always re-invoke get_audit, even immediately after a fresh load");
  } finally {
    delete globalThis.document;
    delete globalThis.invoke;
  }
});

test("audit.ts and agents.ts agree on the freshness window", async () => {
  const { RELOAD_FRESHNESS_MS: auditMs } = await loadAuditModule();
  const agentsSource = await readFile(new URL("../src/agents.ts", import.meta.url), "utf8");
  const match = agentsSource.match(/export const RELOAD_FRESHNESS_MS = ([\d_]+);/);
  assert.ok(match, "src/agents.ts no longer exports a plain numeric RELOAD_FRESHNESS_MS -- update this test's pattern");
  assert.equal(auditMs, Number(match[1].replace(/_/g, "")), "the Audit and the Agents view must apply the same freshness window");
});
