// Loads the browser demo's stand-in backend (src/demo/mock.ts) into a plain Node test, with the
// modules it really imports: src/i18n.ts with its nine dictionaries inlined, and
// src/demo/synthetic.ts. `ts.transpileModule` handles one file at a time and Node cannot resolve a
// relative import from a data: URL, so the three sources are joined into one module whose
// top-level names do not clash (the loader fails loudly if an import line moved). The module
// exports everything the three do: `handle`, the findings' builders, `render`, `LOCALES`.
//
// `fixture` is what the demo reads as its generated fixture (src/demo-fixture.json); by default
// the committed one, so a test runs against what the demo ships. Each call is a new instance of
// the module, with the in-memory state and active language of a fresh page load.
import { readFile } from "node:fs/promises";
import ts from "typescript";
import { inlineLocaleImports } from "./inline-locales.mjs";

let instances = 0;

const read = (rel) => readFile(new URL(`../src/${rel}`, import.meta.url), "utf8");

/** `source` without the one import line matching `pattern`, or the replacement given. */
function without(source, pattern, what, replacement = "") {
  if (!pattern.test(source)) throw new Error(`no ${what} import in the source -- it moved under scripts/demo-backend.mjs`);
  return source.replace(pattern, replacement);
}

export async function loadDemoBackend(fixture) {
  fixture ??= JSON.parse(await read("demo-fixture.json"));
  const i18n = await inlineLocaleImports(await read("i18n.ts"), new URL("../src/locales/", import.meta.url));
  const synthetic = without(await read("demo/synthetic.ts"), /^import\s*\{[^}]*\}\s*from\s*["']\.\.\/i18n["'];[ \t]*$/m, "i18n");
  let mock = await read("demo/mock.ts");
  mock = without(mock, /^import fixture from "\.\.\/demo-fixture\.json";[ \t]*$/m, "fixture", `const fixture = ${JSON.stringify(fixture)};`);
  mock = without(mock, /^import\s*\{[^}]*\}\s*from\s*["']\.\.\/i18n["'];[ \t]*$/m, "i18n");
  mock = without(mock, /^import\s*\{[^}]*\}\s*from\s*["']\.\/synthetic["'];[ \t]*$/m, "synthetic");
  // The build hands the demo its version through a `declare const`, which transpiling erases.
  // The target is tsconfig.json's own: left to its ES5 default, transpiling turns spreading a Set
  // into an empty array, which the build the demo ships in never does.
  const code = ts.transpileModule(`const __APP_VERSION__ = "test";\n${i18n}\n${synthetic}\n${mock}\n// instance ${instances++}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}
