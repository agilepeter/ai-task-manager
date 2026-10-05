// The idle rule is stated twice: Rust's runaway rule (IDLE_SECS in
// crates/core/src/agent_watch.rs) and the Running now row (IDLE_SECS in
// src/inventory.ts). Read both sources as text and fail when the numbers differ.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const read = (rel) => readFile(new URL(`../${rel}`, import.meta.url), "utf8");

test("the row and the rule agree on idle", async () => {
  const rust = (await read("crates/core/src/agent_watch.rs")).match(/const IDLE_SECS:\s*u64\s*=\s*(\d+);/);
  const ts = await read("src/inventory.ts");
  const web = ts.match(/export const IDLE_SECS\s*=\s*(\d+);/);
  assert.ok(rust, "could not find IDLE_SECS in agent_watch.rs");
  assert.ok(web, "could not find IDLE_SECS in src/inventory.ts");
  assert.equal(Number(web[1]), Number(rust[1]), "the Running now row and the runaway rule disagree on how long is idle");
  assert.match(ts, /p\.idleSecs >= IDLE_SECS && p\.tokens10m === 0/, "the row must use the named constant");
});
