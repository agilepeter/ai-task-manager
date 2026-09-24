// ts.transpileModule transpiles one file at a time, so it never bundles
// src/i18n.ts's `import xx from "./locales/xx.json"` dictionary imports --
// and Node's native ESM loader can neither resolve a relative specifier
// against a data: URL nor take a bare JSON import without an attribute
// (which Vite's bundler build doesn't need). Every test that loads a
// transpiled copy of src/i18n.ts therefore has to turn those imports into
// plain object literals first, and three of them did (scripts/i18n.test.mjs,
// scripts/demo-synthetic.test.mjs, scripts/sub2api-display.test.mjs), each
// with its own byte-identical copy of the same loop. One copy here instead,
// so the three loaders keep only their own module-assembly logic.
import { readFile, readdir } from "node:fs/promises";

/// Returns `i18nSource` with every `import <name> from "./locales/<file>";`
/// line -- one per `.json` file in `localesDir` -- replaced by an inlined
/// `const <name> = <json>;`. The import identifier is read off the real
/// import line rather than assumed from the file's own basename: a locale
/// code with a hyphen ("pt-BR") is not a legal JS identifier, so i18n.ts
/// spells its import with the hyphen stripped ("ptBR"), and only reading
/// the source's own line can ever agree with whatever it actually calls a
/// given locale. `localesDir` is a `URL` (e.g.
/// `new URL("../src/locales/", import.meta.url)`), the same shape every
/// caller already builds to pass to `readdir`/`readFile`.
///
/// A locale file with no matching import line fails loudly, naming the
/// file, rather than being left un-inlined: a dangling `import` statement
/// in the transpiled output does not fail here -- it fails later, as a
/// generic module-resolution error with no hint of which file caused it.
export async function inlineLocaleImports(i18nSource, localesDir) {
  const files = (await readdir(localesDir)).filter((f) => f.endsWith(".json"));
  let inlined = i18nSource;
  for (const file of files) {
    const escapedFile = file.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const importLine = i18nSource.match(new RegExp(`import\\s+([A-Za-z_$][\\w$]*)\\s+from\\s+"\\./locales/${escapedFile}";`));
    if (!importLine) throw new Error(`no import line for ${file} in src/i18n.ts`);
    const name = importLine[1];
    const json = await readFile(new URL(file, localesDir), "utf8");
    inlined = inlined.replace(`import ${name} from "./locales/${file}";`, `const ${name} = ${json};`);
  }
  return inlined;
}
