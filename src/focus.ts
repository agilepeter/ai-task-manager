// A safe .focus() for every panel's own close()/cancel(): the resolved
// opener or fallback is not always something the browser will actually
// accept focus on. `.focus()` on an element with no tabindex and no native
// focusability (a bare <span> or <article>, e.g. a Detail page's own opener
// when it was opened by clicking a card's plain-text name rather than its
// "Details" button) is a silent no-op -- it does not throw, and it does not
// move focus, so whatever had focus before (already forced elsewhere by an
// `inert` toggle or a DOM removal, see below) stays exactly where it was.
//
// That matters because leaving focus at nowhere at all is not a neutral
// outcome in this WebKit: found live in Playwright that once something
// forces the previously-focused element to blur with no explicit next
// target -- an ancestor becoming `inert`, or the focused element itself
// being destroyed by an innerHTML rewrite -- plain Tab stops responding for
// the rest of the app until an unrelated mouse click happens to reset it. A
// keyboard-only user has no such click to fall back on.
//
// document.body carries tabindex="-1" (index.html) specifically so it is
// always a valid target here: never a Tab stop of its own, but always
// focusable programmatically, which is what actually recovers Tab
// navigation from that state.
export function focusOrFallback(target: HTMLElement | null, fallback: HTMLElement = document.body): void {
  target?.focus();
  if (document.activeElement !== target) fallback.focus();
}

// ---------------------------------------------------------------------------
// Keeping a keyboard user's place across a redraw.
//
// Every view here redraws by replacing innerHTML: the Usage cards on a timer
// and on every refresh, Inventory after a rescan, a figure when it is
// flipped. The focused control is destroyed with the rest and focus falls to
// <body>. For a pointer user that is nothing. For a keyboard user it is
// their place in the page, lost every few seconds.
//
// So the last control the keyboard focused is remembered by where it sits,
// not by what it is: the nearest ancestor that can be found again (an id, a
// provider's card, a section), what kind of control it is inside that, and
// which one of those it is in order. After a redraw the control in the same
// place takes focus back, without scrolling.

export interface Place {
  /** Selector of the nearest ancestor-or-self that can be found again. */
  anchor: string;
  /** Selector of the control inside the anchor; "" when it IS the anchor. */
  inside: string;
  /** Which match of `inside` within the anchor, in document order. */
  index: number;
}

const quote = (v: string) => `"${v.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;

/** Pure: the selector that finds an anchor again, from its id and the two
 *  data attributes that name a card or a section. Null when it has none. */
export function anchorSelector(id: string, provider: string | undefined, section: string | undefined): string | null {
  if (id) return `[id=${quote(id)}]`;
  if (provider !== undefined) return `[data-provider=${quote(provider)}]`;
  if (section !== undefined) return `[data-section=${quote(section)}]`;
  return null;
}

/** Pure: the selector for a control inside its anchor: its tag, its first
 *  class, and every data attribute it carries except this app's own marker
 *  (values included, so "End task" for one server is not another's). */
export function insideSelector(tag: string, firstClass: string, data: Record<string, string | undefined>): string {
  const attrs = Object.keys(data)
    .filter((k) => k !== "tabAdded")
    .sort()
    .map((k) => `[data-${k.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}=${quote(data[k] ?? "")}]`)
    .join("");
  return `${tag.toLowerCase()}${firstClass ? `.${firstClass}` : ""}${attrs}`;
}

export function placeOf(el: HTMLElement): Place | null {
  let node: HTMLElement | null = el;
  while (node && node !== document.body) {
    const anchor = anchorSelector(node.id, node.dataset.provider, node.dataset.section);
    if (anchor) {
      if (node === el) return { anchor, inside: "", index: 0 };
      const inside = insideSelector(el.tagName, el.classList[0] ?? "", { ...el.dataset });
      const index = Array.from(node.querySelectorAll(inside)).indexOf(el);
      return index < 0 ? null : { anchor, inside, index };
    }
    node = node.parentElement;
  }
  return null;
}

export function findPlace(place: Place): HTMLElement | null {
  const anchor = document.querySelector<HTMLElement>(place.anchor);
  if (!anchor) return null;
  if (!place.inside) return anchor;
  return anchor.querySelectorAll<HTMLElement>(place.inside)[place.index] ?? null;
}

/** Pure: whether a redraw should hand focus back. Only for a keyboard user,
 *  only when the control they were on is gone, and only when focus fell to
 *  nothing: focus that a handler moved somewhere on purpose is left alone. */
export function handsFocusBack(keyboard: boolean, lastIsGone: boolean, focusFellToNothing: boolean): boolean {
  return keyboard && lastIsGone && focusFellToNothing;
}

/** Call once, at boot. */
export function keepFocusAcrossRedraws(): void {
  let keyboard = false;
  let last: HTMLElement | null = null;
  let place: Place | null = null;
  document.addEventListener("keydown", () => (keyboard = true), true);
  document.addEventListener("pointerdown", () => (keyboard = false), true);
  document.addEventListener("focusin", (e) => {
    const el = e.target as HTMLElement;
    if (el === document.body) return;
    last = el;
    place = placeOf(el);
  });
  new MutationObserver(() => {
    const active = document.activeElement;
    const nothing = !active || active === document.body;
    if (!place || !handsFocusBack(keyboard, last !== null && !last.isConnected, nothing)) return;
    findPlace(place)?.focus({ preventScroll: true });
  }).observe(document.body, { childList: true, subtree: true });
}
