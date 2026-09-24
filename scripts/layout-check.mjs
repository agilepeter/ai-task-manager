// Layout verification at the app's real window width (380px) across every
// locale and every view. This is the mechanical half of the nine-language
// layout sweep; a human still has to LOOK at the screenshots this writes.
// Two blind spots are deliberate -- ellipsis truncation and an awkward but
// technically non-overflowing wrap are excluded on purpose; see the
// ALLOWLIST and the ellipsis carve-out in scanOverflow(). A third is
// structural, not a choice: a native <select>'s own clipped, selected-option
// text never moves its scrollWidth or clientWidth in WebKit -- forcing one
// down to 70px and reading its own numbers back (69/68) stays clean even
// though the label is visibly cut. Every <select> in a view is checked only
// by the LOOK pass below; a green run is not proof for one.
//
// What it checks per (view, locale) cell:
//   1. document.documentElement.scrollWidth <= 380 -- the popover itself
//      never gets pushed wider than its fixed, non-resizable window.
//   2. No element's scrollWidth exceeds its clientWidth by more than 1px,
//      except elements on the ALLOWLIST below (each with a one-line reason)
//      and elements that use `text-overflow: ellipsis` on purpose -- that is
//      a *feature* (a label truncates and a hover tooltip shows the full
//      text; see setupTooltips() in src/main.ts), not a bug, and it trips
//      scrollWidth > clientWidth by design.
//
// How to run it:
//   npm run build:demo
//   (cd dist-demo && python3 -m http.server 8731 &)
//   node scripts/layout-check.mjs
//   # optional: node scripts/layout-check.mjs http://127.0.0.1:8731 /some/out/dir
//
// Env overrides for faster iteration while chasing a single fix (the default
// with no env vars is the full 63-cell matrix the gates expect):
//   ONLY_LOCALES=de,ru node scripts/layout-check.mjs
//   ONLY_VIEWS=settings,audit node scripts/layout-check.mjs
//
// Playwright is not a dependency of this repo (same as make-product-shots.mjs);
// run it from an install that has it, e.g. `npx playwright install webkit`.
// WebKit on purpose: the macOS webview is WebKit (see CLAUDE.md).
//
// Deterministic and offline: the demo's mock backend (src/demo/mock.ts) is
// the only data source, no real network calls happen once dist-demo is
// built, and nothing here touches cargo. Not a `*.test.mjs`, so `npm test`
// does not run it -- a 9-locale x 7-view WebKit sweep with screenshots does
// not belong on the hot path of every `npm test`.

import { mkdirSync, readFileSync, existsSync } from 'fs';
import { homedir } from 'os';
import path from 'path';
import { fileURLToPath, pathToFileURL } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.join(__dirname, '..');

// Playwright is not a dependency of this repo (same reasoning as
// make-product-shots.mjs). Node's ESM resolver only looks for bare
// specifiers in node_modules directories that are actual ancestors of this
// file, so a plain `import 'playwright'` cannot see an npx-cache install
// that lives under a different tree (e.g. ~/.npm/_npx/<hash>). Try the
// normal resolution first (works if the repo, or a parent directory, ever
// gets its own install), then fall back to the documented npx cache
// location by importing its package.json "exports"."import" entry directly.
async function loadPlaywright() {
  try {
    return await import('playwright');
  } catch {
    const fallbackDir = path.join(homedir(), '.npm/_npx/e41f203b7505f1fb/node_modules/playwright');
    if (!existsSync(fallbackDir)) {
      throw new Error(
        "Playwright not found. Either `npm install playwright` somewhere above this repo, or " +
          "`npx playwright install webkit` (installs into ~/.npm/_npx/<hash>) and adjust " +
          'fallbackDir in scripts/layout-check.mjs to match.',
      );
    }
    return import(pathToFileURL(path.join(fallbackDir, 'index.mjs')).href);
  }
}
const { webkit } = await loadPlaywright();

const BASE = process.argv[2] || 'http://127.0.0.1:8731';
const OUT_DIR = process.argv[3] || path.join(homedir(), 'saarvis', '_screenshots', 'i18n');
mkdirSync(OUT_DIR, { recursive: true });

// LOCALES comes from the app's own source so this list can never drift from
// what actually ships (the file header in src/i18n.ts says a new language
// adds one entry here; this script needs no matching edit).
const i18nSrc = readFileSync(path.join(REPO_ROOT, 'src/i18n.ts'), 'utf8');
const localesLiteral = i18nSrc.match(/export const LOCALES = \[([^\]]+)\]/);
if (!localesLiteral) throw new Error('could not find `export const LOCALES = [...]` in src/i18n.ts');
const ALL_LOCALES = [...localesLiteral[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
if (ALL_LOCALES.length === 0) throw new Error('parsed zero locales out of src/i18n.ts -- regex is stale');

// One row per required view. `scroller` is that view's own root element --
// the same one the CSS keys visibility off of -- and doubles as the element
// whose scrollHeight determines how tall the viewport must grow to render
// the view without any internal scrolling, so one screenshot shows the
// whole thing (see tallEnoughFor()).
// Both `#settings`/`#audit`/`#detail`/`#about` (full-window slide-in panels)
// and `#providers`/`#inventory`/`#ledger` (the three tab bodies) are their
// own `overflow-y: auto` scrollers -- confirmed by reading src/styles.css,
// not assumed.
const VIEWS = [
  { name: 'usage', scroller: '#providers' },
  { name: 'settings', scroller: '#settings' },
  { name: 'detail', scroller: '#detail' },
  { name: 'inventory', scroller: '#inventory' },
  { name: 'audit', scroller: '#audit' },
  { name: 'about', scroller: '#about' },
  { name: 'subscriptions', scroller: '#ledger' },
];

const ONLY_LOCALES = process.env.ONLY_LOCALES?.split(',').map((s) => s.trim()).filter(Boolean);
const ONLY_VIEWS = process.env.ONLY_VIEWS?.split(',').map((s) => s.trim()).filter(Boolean);
const LOCALES = ONLY_LOCALES?.length ? ALL_LOCALES.filter((l) => ONLY_LOCALES.includes(l)) : ALL_LOCALES;
const VIEWS_TO_RUN = ONLY_VIEWS?.length ? VIEWS.filter((v) => ONLY_VIEWS.includes(v.name)) : VIEWS;

// Elements confirmed, by reading their CSS, to legitimately render wider
// than their own box. Each entry is a CSS selector plus the one-line reason
// it is allowed to trip the raw scrollWidth > clientWidth check below.
const ALLOWLIST = [
  {
    selector: '.total-spend .legend',
    reason:
      "Its .legend-row children carry `padding: 1px 5px; margin: 0 -5px` (src/styles.css, " +
      "the Total Spend card's hover-highlight bleed) so a hovered row's background reads " +
      'slightly past the text column on purpose. The bleed is absorbed by the card\'s own ' +
      'padding one level up -- document.documentElement.scrollWidth is never affected -- and ' +
      'it reproduces in English with no translation involved, so it is not an i18n defect.',
  },
  {
    selector: '.total-spend .donut-wrap',
    reason: 'Contains .legend (above); inherits the same few px for the same reason.',
  },
  {
    selector: '#side-zone',
    reason:
      'The auto-hiding sidebar rail: collapsed it is a 10px sliver, but its child .sidebar ' +
      'is an absolutely-positioned, always-42px-wide element parked mostly off-screen via ' +
      '`transform`, revealed on hover. #side-zone has no overflow:hidden, so scrollWidth ' +
      'reads 42 against a 10px clientWidth any time nothing is actively hovering it -- by ' +
      'construction, not a locale-dependent bug (no text is involved).',
  },
];

function check(label, cond) {
  console.log(`  ${cond ? 'PASS' : 'FAIL'}: ${label}`);
  return cond;
}

// Every one of Settings/Detail/Audit/About's root elements is `position:
// fixed; inset: 0` and stays fully laid out (real scrollWidth/clientWidth,
// non-empty getClientRects()) even while "closed" -- closed just means
// `transform: translateX(103%)`, which moves it, it does not unrender it.
// Two consequences a scan across the whole document would get wrong: (1) a
// genuine bug inside a currently-closed panel would get attributed to
// whatever view happens to be active when the scan runs, and (2) a panel
// that was rendered under an earlier locale and never reopened since (so
// never re-rendered by rerender()) keeps that STALE locale's text, which
// would then get scanned as if it belonged to the current locale. Both are
// real failure modes seen while developing this script (a French "Detail"
// render leaking into a later German "Settings" cell's offender list).
// Scoping the scan to the current view's own root, plus whatever sits
// outside all seven roots (the tab strip, sidebar, footer -- always live),
// avoids both.
const VIEW_ROOTS = VIEWS.map((v) => v.scroller);

/** Full page.evaluate for one cell: overflow scan + the doc-width assertion.
 * `currentRoot` is this cell's own view.scroller (e.g. '#settings'); content
 * under any *other* VIEW_ROOTS entry is skipped (see the comment above). */
async function scanOverflow(page, allowlistSelectors, currentRoot, allRoots) {
  return page.evaluate(
    ({ selectors, currentRoot, allRoots }) => {
      const allowed = new Set();
      for (const sel of selectors) document.querySelectorAll(sel).forEach((el) => allowed.add(el));
      const foreignRoots = allRoots.filter((r) => r !== currentRoot);
      const offenders = [];
      document.body.querySelectorAll('*').forEach((el) => {
        if (allowed.has(el)) return;
        if (foreignRoots.some((r) => el.closest(r))) return;
        if (el.getClientRects().length === 0) return; // not rendered (display:none, or hidden)
        const cs = getComputedStyle(el);
        if (cs.textOverflow === 'ellipsis') return; // deliberate truncation; judged visually, not here
        if (el.scrollWidth > el.clientWidth + 1) {
          offenders.push({
            tag: el.tagName.toLowerCase(),
            id: el.id || null,
            cls: typeof el.className === 'string' ? el.className.slice(0, 100) : null,
            scrollWidth: el.scrollWidth,
            clientWidth: el.clientWidth,
            text: (el.textContent || '').trim().replace(/\s+/g, ' ').slice(0, 70),
          });
        }
      });
      return { docScrollWidth: document.documentElement.scrollWidth, offenders };
    },
    { selectors: allowlistSelectors, currentRoot, allRoots },
  );
}

/** Grow the viewport (width fixed at 380) so `scroller`'s full content
 * renders without internal scrolling, so a single screenshot shows all of
 * it. Confirmed independent of viewport height: the panels are `inset: 0`
 * fixed elements or `flex: 1` tab bodies sized off .popover's `100vh`, and
 * scrollWidth/clientWidth (what scanOverflow checks) do not change when only
 * the height changes -- verified directly before writing this script. */
async function tallEnoughFor(page, scrollerSelector) {
  const needed = await page.evaluate((sel) => {
    const el = document.querySelector(sel);
    return el ? el.scrollHeight : 600;
  }, scrollerSelector);
  await page.setViewportSize({ width: 380, height: Math.max(600, needed + 40) });
}

async function resetScroll(page) {
  await page.evaluate(() => document.querySelectorAll('*').forEach((el) => { if (el.scrollTop) el.scrollTop = 0; }));
}

// A fixed-ms wait after setViewportSize() is not the same thing as "the
// renderer has actually finished a layout pass at the new size" -- a real,
// intermittent flakiness while developing this script (identical offenders
// on some runs, absent on others, with no CSS or content difference between
// them) traced to exactly that gap. Two rAFs guarantee at least one full
// style/layout/paint cycle has completed, which a timeout can only ever
// approximate.
async function settle(page) {
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  );
}

// #settings-btn *toggles* the panel, so calling this while it is already
// open (e.g. two views in a row that both need it, or ONLY_VIEWS narrowed to
// just "settings") would close it instead. Check the real state first, the
// same way gotoUsageTab does.
async function openSettings(page) {
  const open = await page.evaluate(() => document.body.classList.contains('settings-open'));
  if (open) return;
  await page.locator('#side-zone').hover();
  await page.waitForTimeout(250); // the rail's 0.24s slide-out, so #settings-btn is actually clickable
  await page.locator('#settings-btn').click({ force: true });
  await page.waitForTimeout(500); // #settings' 0.28s slide-in transition, plus its first render
}

async function closeSettings(page) {
  const open = await page.evaluate(() => document.body.classList.contains('settings-open'));
  if (!open) return;
  await page.locator('#settings-close').click();
  await page.waitForTimeout(300); // #settings' 0.28s slide-out transition
}

async function setLocale(page, locale) {
  await openSettings(page);
  await page.selectOption('#locale', locale);
  await page.waitForTimeout(500); // applyLocale() is synchronous; this covers the transition CSS
  await closeSettings(page);
}

/** Expand every visible accordion group in Settings (the hidden #api-keys-group
 * stays closed on purpose -- it is revealed only by its own button and is not
 * part of the normal settings surface this sweep covers). */
async function expandAllAccordions(page) {
  const groups = await page.locator('#settings .acc-group:not([hidden])').all();
  for (const g of groups) {
    const isOpen = await g.evaluate((el) => el.classList.contains('open'));
    if (!isOpen) await g.locator('.acc-head').click();
  }
  await page.waitForTimeout(450); // > the .acc-body 0.26s grid-template-rows transition
}

// Which panel is open is a body class (src/styles.css keys visibility off of
// it, sliding the panel on/off screen via `transform`), not something
// Playwright's generic isVisible() can see -- a closed panel is still
// "visible" by CSS visibility/display/size, just transformed off past the
// right edge. Reading the same class the app itself toggles is the only
// reliable way to know what is actually open.
const PANEL_CLOSE = {
  'detail-open': '#detail-close',
  'audit-open': '#audit-close',
  'about-open': '#about-close',
  'settings-open': '#settings-close',
};

async function gotoUsageTab(page) {
  // Detail/Audit/Settings/About cover #view-tabs while open at this width,
  // so whichever panel is actually open has to close first.
  for (const [bodyClass, closeBtn] of Object.entries(PANEL_CLOSE)) {
    const open = await page.evaluate((c) => document.body.classList.contains(c), bodyClass);
    if (open) {
      await page.locator(closeBtn).click();
      await page.waitForTimeout(250); // that panel's own slide-out transition
    }
  }
  await page.locator('[data-view="usage"]').click();
  await page.waitForTimeout(500); // the Usage tab's provider cards to render
}

/** One (view, locale) cell: prepare the view, assert, screenshot. Returns
 * { pass, docScrollWidth, offenders, file }. */
async function runCell(page, view, locale) {
  await gotoUsageTab(page);

  switch (view.name) {
    case 'usage':
      break; // already there
    case 'settings':
      await openSettings(page);
      await expandAllAccordions(page);
      break;
    case 'detail':
      await page.locator('#providers .provider:not(.total-spend) .provider-name').first().click();
      await page.waitForSelector('#detail .dt-section', { timeout: 5000 });
      await page.waitForTimeout(1500); // history + forecast calls settle
      break;
    case 'inventory':
      await page.locator('[data-view="inventory"]').click();
      await page.waitForTimeout(800); // Inventory's async load + render
      break;
    case 'audit':
      await page.locator('[data-view="inventory"]').click();
      await page.waitForTimeout(500); // Inventory rendered, so #audit-open-btn exists to click
      await page.locator('#audit-open-btn').click();
      await page.waitForSelector('#audit-body .au-head', { timeout: 5000 });
      await page.waitForTimeout(500); // safety margin after the score/sections paint
      break;
    case 'about':
      await page.locator('#side-zone').hover();
      await page.waitForTimeout(250); // the rail's 0.24s slide-out, so #about-btn is actually clickable
      await page.locator('#about-btn').click({ force: true });
      await page.waitForSelector('#about-body .ab-hero', { timeout: 5000 });
      await page.waitForTimeout(300); // the panel's slide-in transition
      await page.mouse.move(340, 320); // let the auto-hide rail retract before the shot
      await page.waitForTimeout(300); // the rail's own 0.24s retract transition
      break;
    case 'subscriptions':
      await page.locator('[data-view="ledger"]').click();
      await page.waitForTimeout(600); // the Subscriptions tab's async load + render
      break;
    default:
      throw new Error(`unknown view ${view.name}`);
  }

  await tallEnoughFor(page, view.scroller);
  await settle(page);
  await resetScroll(page);
  // Settings/About open via a hover on #side-zone; leaving the pointer there
  // keeps the auto-hide rail expanded over the shot. It is a paint-only
  // overlay (confirmed not to move docScrollWidth), but it is not what a
  // real user sees on this screen, so move away before every screenshot.
  await page.mouse.move(340, 40);
  await page.waitForTimeout(200); // most of the rail's own 0.24s retract transition
  await settle(page);

  const { docScrollWidth, offenders } = await scanOverflow(
    page,
    ALLOWLIST.map((a) => a.selector),
    view.scroller,
    VIEW_ROOTS,
  );

  const file = path.join(OUT_DIR, `${view.name}-${locale}.png`);
  await page.screenshot({ path: file });

  await page.setViewportSize({ width: 380, height: 600 });

  const pass = docScrollWidth <= 380 && offenders.length === 0;
  return { pass, docScrollWidth, offenders, file };
}

async function main() {
  const browser = await webkit.launch();
  const page = await browser.newPage({ viewport: { width: 380, height: 600 }, deviceScaleFactor: 2 });
  const pageErrors = [];
  page.on('pageerror', (e) => pageErrors.push(String(e)));

  const matrix = []; // { view, locale, pass, docScrollWidth, offenders, file }
  const seenAllowlistHit = new Set();

  try {
    await page.goto(`${BASE}/demo.html`, { waitUntil: 'networkidle' });
    await page.waitForTimeout(1800); // the demo to boot and its fixture data to render
    await page.evaluate(() => document.fonts.ready); // text metrics settled before anything is measured

    for (const locale of LOCALES) {
      await setLocale(page, locale);
      for (const view of VIEWS_TO_RUN) {
        const result = await runCell(page, view, locale);
        matrix.push({ view: view.name, locale, ...result });
        const label = `${view.name}-${locale}`;
        check(label, result.pass);
        if (!result.pass) {
          console.log(`    docScrollWidth=${result.docScrollWidth} offenders=${result.offenders.length}`);
          result.offenders.forEach((o) => console.log('     ', JSON.stringify(o)));
        }
        for (const a of ALLOWLIST) {
          const hit = await page.locator(a.selector).count();
          if (hit > 0) seenAllowlistHit.add(a.selector);
        }
      }
    }
  } finally {
    await browser.close();
  }

  console.log('\n=== 63-cell matrix (view x locale) ===');
  const header = ['view'.padEnd(14), ...LOCALES.map((l) => l.padEnd(6))].join(' ');
  console.log(header);
  for (const view of VIEWS_TO_RUN) {
    const row = [view.name.padEnd(14)];
    for (const locale of LOCALES) {
      const cell = matrix.find((m) => m.view === view.name && m.locale === locale);
      row.push((cell ? (cell.pass ? 'PASS' : 'FAIL') : '--').padEnd(6));
    }
    console.log(row.join(' '));
  }

  const failed = matrix.filter((m) => !m.pass);
  const staleAllowlist = ALLOWLIST.filter((a) => !seenAllowlistHit.has(a.selector));
  console.log(`\n${matrix.length} cells run, ${failed.length} failed.`);
  if (staleAllowlist.length) {
    console.log('WARNING: allowlist entries that matched nothing this run (stale?):');
    staleAllowlist.forEach((a) => console.log(`  - ${a.selector}`));
  }
  if (pageErrors.length) {
    console.log(`WARNING: ${pageErrors.length} page error(s) during the run:`);
    pageErrors.forEach((e) => console.log(`  - ${e}`));
  }
  console.log(`Screenshots written to ${OUT_DIR}`);

  if (failed.length > 0) {
    console.log('\nRESULT: FAIL');
    process.exitCode = 1;
  } else {
    console.log('\nRESULT: PASS');
  }
}

await main();
