import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { test } from "node:test";
import { inlineLocaleImports } from "./inline-locales.mjs";

test("inlineLocaleImports fails loudly, naming the file, when a locale file has no matching import line", async () => {
  // A source missing the import line for one locale file must not be left
  // silently un-inlined: that would carry a dangling `import` statement
  // into the transpiled output, which fails later as an opaque
  // module-resolution error with no hint of which locale file caused it.
  // A scratch directory, not src/locales/, so this stays correct however
  // many real locales exist.
  const dir = await mkdtemp(path.join(tmpdir(), "inline-locales-test-"));
  try {
    await writeFile(path.join(dir, "a.json"), '{"k":"a"}');
    await writeFile(path.join(dir, "b.json"), '{"k":"b"}');
    const sourceMissingB = 'import a from "./locales/a.json";\n'; // no import line for b.json
    const localesDir = pathToFileURL(`${dir}/`);
    await assert.rejects(
      () => inlineLocaleImports(sourceMissingB, localesDir),
      (err) => err instanceof Error && err.message === "no import line for b.json in src/i18n.ts",
    );
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
