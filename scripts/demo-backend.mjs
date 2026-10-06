// Loads the browser demo's stand-in backend (src/demo/mock.ts) into a plain Node test, the way
// scripts/client-month-to-date.test.mjs does: `ts.transpileModule` handles one file at a time and
// Node cannot resolve a relative import from a data: URL, so the module's three imports become
// inert stand-ins and the transpiled source is imported as a data: URL. None of the commands the
// tests that use this call reaches the stand-ins for `t` or the two synthetic-finding builders.
//
// `fixture` is what the demo reads as its generated fixture (src/demo-fixture.json). A test hands
// in only the part of it its commands read, or the real parts, never an invented copy of them.
// Each call is a new instance of the module, with the in-memory state of a fresh page load.
import { readFile } from "node:fs/promises";
import ts from "typescript";

let instances = 0;

export async function loadDemoBackend(fixture = {}) {
  const source = await readFile(new URL("../src/demo/mock.ts", import.meta.url), "utf8");
  const stripped = source
    .replace('import fixture from "../demo-fixture.json";', `const fixture = ${JSON.stringify(fixture)};`)
    .replace('import { t } from "../i18n";', "const t = (key) => key;")
    .replace(
      'import { buildDuplicateProcessesRow, buildUsageRows } from "./synthetic";',
      "const buildDuplicateProcessesRow = () => null;\nconst buildUsageRows = () => [];",
    );
  if (stripped === source) throw new Error("no substitution matched -- src/demo/mock.ts's imports moved under this test");
  // The build hands the demo its version through a `declare const`, which transpiling erases.
  // The target is tsconfig.json's own: left to its ES5 default, transpiling turns spreading a Set
  // into an empty array, which the build the demo ships in never does.
  const code = ts.transpileModule(`const __APP_VERSION__ = "test";\n${stripped}\n// instance ${instances++}`, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2020 },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
}
