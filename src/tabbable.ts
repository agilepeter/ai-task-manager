// Plain Tab, in the macOS webview (WebKit), skips every <button> and <a>
// that has no explicit tabindex -- confirmed directly against the real demo
// in Playwright WebKit before writing this: 40 Tab presses from the top of
// the Usage tab reached #build-info (the one element that already carried
// tabindex="0") and then cycled through Settings' own <input>/<select>
// controls (native form controls need no tabindex at all), never a single
// <button>. Full keyboard access (WebKit's system "keyboard navigation"
// setting turned on) reaches everything, but this app cannot depend on a
// setting Peter does not control on every machine it runs on -- macOS ships
// it OFF by default and most users never turn it on.
//
// Also confirmed directly: adding tabindex="0" to a plain <button>, <a> or
// even a bare <span> DOES make it reachable by plain Tab in this WebKit, in
// DOM order, and the standard `inert` attribute IS supported and correctly
// removes a whole subtree from the Tab order regardless of any tabindex its
// descendants carry.
//
// One mechanism, one place, rather than hand-adding tabindex="0" to every
// template in this app (there are hundreds of buttons, rendered by eight
// different modules): makeTabbable() walks the live DOM after a render and
// gives every otherwise-untabbable control an explicit tabindex="0" --
// skipping anything disabled, not actually rendered, or that already has a
// tabindex of its own (an explicit tabindex="-1" some template set on
// purpose stays exactly that; a control this function already touched is
// naturally excluded from its own selector on the next pass, since it now
// has the attribute).
//
// Renders happen in many separate modules (main.ts, inventory.ts, agents.ts,
// audit.ts, about.ts, detail.ts, ledger.ts), each with its own internal call
// sites -- calling this by hand after every one of them would be one more
// thing every future render forgets. A MutationObserver watches the whole
// document instead: the browser already coalesces every synchronous DOM
// change within one microtask checkpoint into a single callback invocation
// (confirmed: a render() that replaces several containers' innerHTML in a
// row still fires this callback once, not once per container), and the
// observer is registered with only `childList`/`subtree` -- never
// `attributes` -- so the tabindex attributes this callback itself sets can
// never re-trigger it. No extra debouncing needed, and no loop is possible.

const TABBABLE_SELECTOR = 'button:not([tabindex]), a[href]:not([tabindex]), summary:not([tabindex]), [role="button"]:not([tabindex]), [role="tab"]:not([tabindex])';

/** Gives every matching, live, enabled control in `root` an explicit
 *  tabindex="0". Exported so a test (and the timing measurement in
 *  scripts/layout-check.mjs's own family) can call it directly without
 *  waiting on the MutationObserver. */
export function makeTabbable(root: ParentNode = document): void {
  root.querySelectorAll<HTMLElement>(TABBABLE_SELECTOR).forEach((el) => {
    if ((el as HTMLButtonElement).disabled) return;
    // Not `offsetParent === null`: every slide-in panel here is
    // `position: fixed`, which reads offsetParent as null even while OPEN --
    // that would skip every button inside an open panel, the opposite of
    // what this needs. getClientRects() (the same check
    // scripts/layout-check.mjs's own scanOverflow() already uses for "is
    // this actually rendered") stays non-empty for a fixed panel regardless
    // of its transform, and empty for a real `display:none`/zero-size
    // control -- an off-screen CLOSED panel's controls still get tabindex="0"
    // here, which is fine: `inert` on the panel itself (see index.html and
    // each panel's own open()/close()) is what actually keeps them out of
    // the Tab order while closed, not this function.
    if (el.getClientRects().length === 0) return;
    el.tabIndex = 0;
  });
}

/** Runs makeTabbable() once immediately (for whatever is already in the DOM
 *  when this is called) and then on every future DOM change. Call once, at
 *  boot. */
export function watchTabbable(): void {
  makeTabbable(document);
  new MutationObserver(() => makeTabbable(document)).observe(document.body, { childList: true, subtree: true });
}
