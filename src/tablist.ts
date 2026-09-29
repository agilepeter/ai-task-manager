// The tabs pattern for a role="tablist" group. Today that is one group,
// #view-tabs (Usage / Inventory / Subscriptions), wired from src/main.ts.
// Total Spend's Today / Yesterday / 30 Days switch and the segmented
// controls in Detail and Settings are plain buttons with no role="tab":
// each is its own Tab stop, and none of them calls into this file.
//
// Two halves:
//
// 1. The list is ONE Tab stop. The selected tab carries tabindex="0" and
//    the others tabindex="-1". src/tabbable.ts leaves any element alone
//    that carries a tabindex of its own, -1 included.
//
// 2. Left, Right, Home and End move focus AND select. The target tab is
//    focused and clicked, so the one place that decides what selecting a
//    tab means (the #view-tabs click listener in src/inventory.ts) stays
//    the only place. This file only decides WHICH tab is next.

/** Pure: which tab index `key` moves the roving-tabindex cursor to, given
 *  the currently-focused tab's index and how many tabs there are. Null for
 *  any key outside this pattern (every other key is left alone -- Tab in
 *  particular, which must keep leaving the list rather than being caught
 *  here). Right-to-left is not a concern: this app ships no RTL locale, so
 *  ArrowRight always means "next" and ArrowLeft always means "previous." */
export function nextTabIndex(key: string, current: number, count: number): number | null {
  if (count <= 0) return null;
  switch (key) {
    case "ArrowRight":
      return (current + 1) % count;
    case "ArrowLeft":
      return (current - 1 + count) % count;
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

/** Sets the roving tabindex from the DOM's own aria-selected: 0 on the
 *  selected tab, -1 on every other. */
export function syncTabList(list: ParentNode, selector = '[role="tab"]'): void {
  list.querySelectorAll<HTMLElement>(selector).forEach((tab) => {
    tab.tabIndex = tab.getAttribute("aria-selected") === "true" ? 0 : -1;
  });
}

/** Wires Left, Right, Home and End on `list`, and keeps the roving tabindex
 *  in step with aria-selected however the selection changes (a click, an
 *  arrow key, or the app switching view from code). Acts on a key only
 *  while focus is on one of the tabs. */
export function wireTabList(list: HTMLElement, selector = '[role="tab"]'): void {
  syncTabList(list, selector);
  new MutationObserver(() => syncTabList(list, selector)).observe(list, {
    attributes: true,
    attributeFilter: ["aria-selected"],
    subtree: true,
  });
  list.addEventListener("keydown", (e) => {
    const key = (e as KeyboardEvent).key;
    const tabs = Array.from(list.querySelectorAll<HTMLElement>(selector));
    const current = tabs.indexOf(document.activeElement as HTMLElement);
    if (current === -1) return;
    const next = nextTabIndex(key, current, tabs.length);
    if (next === null) return;
    e.preventDefault();
    tabs[next].focus();
    tabs[next].click();
  });
}
