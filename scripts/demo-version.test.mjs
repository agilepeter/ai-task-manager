// The browser demo answers "which version is this?" itself, because there is no
// app behind it to ask. It used to answer with a number typed into the mock, so
// the demo on the product page went on saying v0.1.0 while the page beside it
// and the release it was built from said 0.1.2.
//
// The version has one source for a browser build, package.json, read by
// scripts/app-version.mjs and handed to the build by vite.config.ts. These
// tests hold that line, and hold the three manifests that state the app's
// version to the same number, since a release that bumps two of them ships an
// app, an installer and a demo that disagree.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import { appVersion } from "./app-version.mjs";

const read = (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

test("the version for a browser build is the one in package.json", async () => {
  const pkg = JSON.parse(await read("package.json"));
  assert.equal(appVersion(), pkg.version);
  assert.match(appVersion(), /^\d+\.\d+\.\d+$/);
});

test("the app, its installer and its package all state the same version", async () => {
  const pkg = JSON.parse(await read("package.json")).version;
  const tauri = JSON.parse(await read("src-tauri/tauri.conf.json")).version;
  const cargo = (await read("src-tauri/Cargo.toml")).match(/^\[package\][^[]*?^version = "([^"]+)"/ms)?.[1];
  assert.equal(tauri, pkg, "src-tauri/tauri.conf.json and package.json disagree");
  assert.equal(cargo, pkg, "src-tauri/Cargo.toml and package.json disagree");
});

test("the build is handed the version, and the demo uses what it is handed", async () => {
  const vite = await read("vite.config.ts");
  assert.match(vite, /__APP_VERSION__: JSON\.stringify\(appVersion\(\)\)/);

  const mock = await read("src/demo/mock.ts");
  assert.match(mock, /case "plugin:app\|version": return __APP_VERSION__;/);
  assert.match(mock, /lastSeenVersion: __APP_VERSION__,/);
});

test("no version of the app is typed into the demo's stand-in for it", async () => {
  const mock = await read("src/demo/mock.ts");
  const typed = mock
    .split("\n")
    .map((line, i) => ({ line, n: i + 1 }))
    // A version in a comment explains; one in code is an answer the demo gives.
    .filter(({ line }) => !line.trim().startsWith("//"))
    // The demo's made-up MCP packages have versions of their own, and should.
    .filter(({ line }) => /version/i.test(line))
    .filter(({ line }) => /["'`]v?\d+\.\d+\.\d+["'`]/.test(line));
  assert.deepEqual(typed.map(({ n, line }) => `${n}: ${line.trim()}`), []);
});
