// The agent watch's limits are stated in Rust (crates/core/src/agent_watch.rs) and again where the
// browser needs them: the add row (src/agents.ts), which refuses a figure before it calls the
// backend, and the demo's stand-in for the command (src/demo/mock.ts), which refuses what the real
// one refuses and works the live hint out the way the engine does. Read each source as text, like
// scripts/agent-idle.test.mjs does for the idle rule, and fail when a copy differs from the Rust,
// so a limit changed in one place cannot leave another refusing, or accepting, something else.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const read = (rel) => readFile(new URL(`../${rel}`, import.meta.url), "utf8");

/** A number literal as the sources spell it: digit groups with underscores, a trailing `.0` in Rust. */
const toNumber = (literal) => Number(literal.replace(/_/g, ""));

/** `const NAME: type = <number>;` in Rust, `pub` or not. */
function rustConstant(source, name) {
  const found = source.match(new RegExp(`const ${name}:\\s*[a-z0-9]+\\s*=\\s*([0-9_.]+);`));
  assert.ok(found, `could not find ${name} in crates/core/src/agent_watch.rs`);
  return toNumber(found[1]);
}

/** `const NAME = <number>;` in TypeScript, exported or not. */
function tsConstant(source, name, file) {
  const found = source.match(new RegExp(`const ${name}\\s*=\\s*([0-9_.]+);`));
  assert.ok(found, `could not find ${name} in ${file}`);
  return toNumber(found[1]);
}

test("the add row and the demo refuse what the engine refuses", async () => {
  const rust = await read("crates/core/src/agent_watch.rs");
  const agents = await read("src/agents.ts");
  const mock = await read("src/demo/mock.ts");
  const budgets = rustConstant(rust, "MAX_BUDGETS");
  const usd = rustConstant(rust, "MAX_USD");
  const minutes = rustConstant(rust, "MAX_MINUTES");

  assert.equal(tsConstant(agents, "MAX_AGENT_BUDGETS", "src/agents.ts"), budgets, "the most budgets the add row lets a user keep");
  assert.equal(tsConstant(agents, "MAX_BUDGET_USD", "src/agents.ts"), usd, "the largest budget the add row accepts");
  assert.equal(tsConstant(mock, "MAX_WATCH_BUDGETS", "src/demo/mock.ts"), budgets, "the most budgets the demo keeps");
  assert.equal(tsConstant(mock, "MAX_WATCH_USD", "src/demo/mock.ts"), usd, "the largest figure the demo accepts");
  assert.equal(tsConstant(mock, "MAX_WATCH_MINUTES", "src/demo/mock.ts"), minutes, "the largest open time the demo accepts");

  // Named where they are used, so a number typed beside them cannot drift from the constant.
  assert.match(mock, /budgets\.length > MAX_WATCH_BUDGETS/, "the demo must refuse by the named budget count");
  assert.match(mock, /x <= MAX_WATCH_USD/, "the demo must refuse by the named dollar limit");
  assert.match(mock, /m > MAX_WATCH_MINUTES/, "the demo must refuse by the named minutes limit");
  assert.match(agents, /amount > MAX_BUDGET_USD/, "the add row must refuse by the named dollar limit");
  assert.match(agents, /budgets\.length >= MAX_AGENT_BUDGETS/, "the add row must close by the named budget count");
});

test("the demo works the live hint out with the engine's idle rule and pace factor", async () => {
  const rust = await read("crates/core/src/agent_watch.rs");
  const mock = await read("src/demo/mock.ts");
  assert.equal(tsConstant(mock, "IDLE_SECS", "src/demo/mock.ts"), rustConstant(rust, "IDLE_SECS"), "seconds without a new line after which a session is idle");
  assert.equal(tsConstant(mock, "PACE_FACTOR", "src/demo/mock.ts"), rustConstant(rust, "PACE_FACTOR"), "the last ten minutes carried over an hour");
  assert.match(mock, /p\.idleSecs >= IDLE_SECS && p\.tokens10m === 0/, "the idle test must use the named constant");
  assert.match(mock, /p!\.cost10m \* PACE_FACTOR/, "the pace must use the named factor");
});
