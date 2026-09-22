// Screenshots for the staas.fund product page, taken from the browser demo.
//
// These were taken by hand once and then went stale without anyone noticing:
// thirteen commits landed on top of them, including the one that stopped
// shipping upstream's logo, so the page was advertising another project's
// mark. A script makes them reproducible, which is the only thing that keeps
// an image honest as the UI moves.
//
// The demo is the source on purpose. Its data is fictional by construction
// (src/demo/mock.ts), so a product shot can never leak a real limit, a real
// dollar figure, a real folder name or a real client.
//
// WebKit on purpose too: the macOS webview is WebKit, and Chromium paints the
// SVG lens effect that WebKit does not (`body.no-svg-lens`). A Chromium shot
// would advertise a UI no Mac user sees.
//
// Usage:
//   npm run build:demo
//   (cd dist-demo && python3 -m http.server 8731 &)
//   node scripts/make-product-shots.mjs http://127.0.0.1:8731 ./out
//   # then: magick <name>.png -quality 82 -define webp:method=6 <name>.webp
//
// Playwright is not a dependency of this repo; run it from an install that
// has it (`npx playwright install webkit` once).

import { webkit } from 'playwright';
import { mkdirSync } from 'fs';

const BASE = process.argv[2] || 'http://127.0.0.1:8731';
const OUT = process.argv[3] || './product-shots';
mkdirSync(OUT, { recursive: true });

// The popover is a fixed 380x600 window; 640 leaves room for the footer.
// deviceScaleFactor 2 gives the 760x1280 the page's <img> attributes expect.
const browser = await webkit.launch();
const page = await browser.newPage({ viewport: { width: 380, height: 640 }, deviceScaleFactor: 2 });

const boot = async () => {
  await page.goto(`${BASE}/demo.html`, { waitUntil: 'networkidle' });
  await page.waitForTimeout(1800);
};
const shot = async (name) => {
  await page.waitForTimeout(500);
  await page.screenshot({ path: `${OUT}/${name}.png` });
  console.log('  ✓', name);
};
const tab = async (label) => {
  await page.locator(`.tab:has-text("${label}")`).first().click();
  await page.waitForTimeout(900);
};

/** Scroll the app's inner scroller so `text` sits `pad` pixels below the top. */
async function scrollTo(text, pad = 12) {
  const found = await page.evaluate(([t, p]) => {
    const el = [...document.querySelectorAll('*')].find(
      (e) => e.children.length === 0 && (e.textContent || '').trim().startsWith(t) && e.offsetParent !== null,
    );
    if (!el) return false;
    let scroller = el.parentElement;
    while (scroller && scroller.scrollHeight <= scroller.clientHeight) scroller = scroller.parentElement;
    if (!scroller) return false;
    scroller.scrollTop =
      el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop - p;
    return true;
  }, [text, pad]);
  await page.waitForTimeout(700);
  if (!found) console.log(`  ! could not find "${text}"`);
  return found;
}

const openDetail = async () => {
  await page.locator('.provider:not(.total-spend) .provider-name').first().click();
  await page.waitForTimeout(1500);
};

await boot();
await shot('usage');

await openDetail();
await scrollTo('Limits over time');
await shot('limits');
await scrollTo('Your week', 56); // 56 keeps the section title above the heatmap
await shot('week');
await page.selectOption('#dt-group', 'client');
await page.waitForTimeout(1400);
await scrollTo('Spend');
await shot('clients');

await boot();
await tab('Inventory');
await shot('inventory');
await scrollTo('Running now');
await shot('running');

// The pin chip only exists on an unpinned server row, and the preview it
// opens frames itself; scrolling afterwards lands on the word "playwright"
// in an opportunity instead.
const pin = page.locator('.inv-pin-btn').first();
if (await pin.count()) {
  await pin.click();
  await page.waitForTimeout(1200);
  await shot('pin');
} else {
  console.log('  ! no unpinned MCP server in the demo, skipping pin');
}

await boot();
await tab('Inventory');
await page.locator('button:has-text("Audit")').first().click();
await page.waitForTimeout(1800);
await shot('audit');

await boot();
await tab('Subscriptions');
await shot('subscriptions');

await boot();
// The sidebar rail auto-hides. Hover to reach the button, then move away so
// the rail retracts before the shot, or it pushes the panel right.
await page.locator('#side-zone').hover();
await page.waitForTimeout(700);
await page.locator('#about-btn').click({ force: true });
await page.waitForTimeout(900);
await page.mouse.move(340, 320);
await page.waitForTimeout(900);
await shot('about');

await browser.close();
console.log('done ->', OUT);
