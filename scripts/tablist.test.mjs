// src/tablist.ts: which tab an arrow key goes to, and which tab is the one
// Tab stop of its list.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

async function load() {
  const source = await readFile(new URL("../src/tablist.ts", import.meta.url), "utf8");
  const code = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext } }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}

test("arrows move one tab and wrap at both ends", async () => {
  const { nextTabIndex } = await load();
  assert.equal(nextTabIndex("ArrowRight", 0, 3), 1);
  assert.equal(nextTabIndex("ArrowRight", 2, 3), 0);
  assert.equal(nextTabIndex("ArrowLeft", 0, 3), 2);
  assert.equal(nextTabIndex("ArrowLeft", 2, 3), 1);
});

test("Home and End go to the first and the last", async () => {
  const { nextTabIndex } = await load();
  assert.equal(nextTabIndex("Home", 2, 3), 0);
  assert.equal(nextTabIndex("End", 0, 3), 2);
});

test("every other key is left for whoever wants it, Tab above all", async () => {
  const { nextTabIndex } = await load();
  for (const key of ["Tab", "Enter", " ", "Escape", "ArrowUp", "ArrowDown", "a"]) assert.equal(nextTabIndex(key, 1, 3), null, key);
  assert.equal(nextTabIndex("ArrowRight", 0, 0), null, "an empty list has nowhere to go");
});

test("a list of one stays on its one tab", async () => {
  const { nextTabIndex } = await load();
  assert.equal(nextTabIndex("ArrowRight", 0, 1), 0);
  assert.equal(nextTabIndex("ArrowLeft", 0, 1), 0);
});

test("the selected tab is the list's one Tab stop", async () => {
  const { syncTabList } = await load();
  const tab = (selected) => ({ tabIndex: 0, getAttribute: (k) => (k === "aria-selected" ? String(selected) : null) });
  const tabs = [tab(false), tab(true), tab(false)];
  syncTabList({ querySelectorAll: () => tabs });
  assert.deepEqual(tabs.map((t) => t.tabIndex), [-1, 0, -1]);
});
