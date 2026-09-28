// README.md names the eight agent hosts the Agents view can show running
// (crates/core/src/procs.rs's AGENT_HOSTS). The README used to say "Copilot
// CLI" where the app itself shows "GitHub Copilot CLI" (AGENT_HOSTS' own
// display name) -- this reads AGENT_HOSTS directly, rather than hardcoding a
// second copy of the list here, so a future host added to one side fails
// this test instead of quietly drifting out of step with the other.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

async function agentHostNames() {
  const source = await readFile(new URL("../crates/core/src/procs.rs", import.meta.url), "utf8");
  const table = source.match(/const AGENT_HOSTS: &\[\(&str, &\[&str\], &\[&str\]\)\] = &\[([\s\S]*?)\n\];/);
  assert.ok(table, "AGENT_HOSTS not found, or its source shape moved under this test");
  const names = [...table[1].matchAll(/\(\s*"([^"]+)"/g)].map((m) => m[1]);
  assert.ok(names.length >= 2, "found no display names inside AGENT_HOSTS -- the regex above no longer matches its shape");
  return names;
}

test("README's Agents paragraph names exactly the AGENT_HOSTS display names, all of them", async () => {
  const names = await agentHostNames();
  const readme = await readFile(new URL("../README.md", import.meta.url), "utf8");
  const para = readme.match(/The eight agent hosts it knows are ([^.]+)\./);
  assert.ok(para, "README no longer has the 'The eight agent hosts it knows are ...' sentence -- update this test's pattern");
  // README wraps its prose at a fixed column, so a host's own name can fall
  // across a line break ("Cursor\nAgent") without that being a real gap --
  // collapse whitespace before comparing.
  const sentence = para[1].replace(/\s+/g, " ");
  for (const name of names) {
    assert.ok(sentence.includes(name), `README's agent-host sentence is missing "${name}" (AGENT_HOSTS in crates/core/src/procs.rs)`);
  }
  assert.equal(names.length, 8, "AGENT_HOSTS no longer has eight entries -- update the README sentence and this test's expectation together");
});
