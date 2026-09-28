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
