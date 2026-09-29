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
// document instead, registered with only `childList`/`subtree` -- never
// `attributes` -- so the attributes this callback itself sets can never
// re-trigger it. It looks only at what a mutation ADDED, and never asks the
// browser for layout, so a large render costs one pass over its own nodes.
//
// Two consequences of giving a button a tabindex, both handled here:
//
// 1. WebKit focuses a clicked element only when it carries an explicit
//    tabindex. Before this module a clicked button never took focus; with
//    it, every one would, and the next Space or Enter would activate it
//    again, with no ring to show where focus sat (a ring is drawn for
//    keyboard focus only). So after a POINTER click, focus does not stay
//    on a control this module made tabbable. Keyboard activation keeps it.
//    A right-click and a middle click count too: WebKit focuses on the
//    press, and no click follows to undo it.
//
// 2. An element that is a button by role only (a provider card's name, a
//    figure that flips when clicked) is reachable now, so Enter and Space
//    have to activate it as they would a real button.
//
// And one guard for everyone who uses the keyboard: a held-down Enter
// repeats, and each repeat would press the control in focus again. A
// control that opens a confirmation would then answer it. Only the first
// press of a held key activates anything.

const TABBABLE_SELECTOR = 'button:not([tabindex]), a[href]:not([tabindex]), summary:not([tabindex]), [role="button"]:not([tabindex]), [role="tab"]:not([tabindex])';

/** Set on every element this module gave its tabindex, so the two listeners
 *  below can tell it from a tabindex a template wrote on purpose. */
export const ADDED = "data-tab-added";

/** Gives every matching, enabled control in `root`, and `root` itself when
 *  it matches, an explicit tabindex="0". A control that is not displayed
 *  gets one too: it is not a Tab stop while hidden, and needs no second
 *  visit when a class change shows it. A closed panel's controls are kept
 *  out of the Tab order by `inert` on the panel (src/panels.ts), not here. */
export function makeTabbable(root: ParentNode = document): void {
  const mark = (el: HTMLElement) => {
    if ((el as HTMLButtonElement).disabled) return;
    el.tabIndex = 0;
    el.setAttribute(ADDED, "");
  };
  const self = root as Partial<HTMLElement>;
  if (typeof self.matches === "function" && self.matches(TABBABLE_SELECTOR)) mark(root as HTMLElement);
  root.querySelectorAll<HTMLElement>(TABBABLE_SELECTOR).forEach(mark);
}

/** Pure: whether focus is given up after a click. Only when the pointer
 *  made the click and focus now rests on a control this module made
 *  tabbable. That is the control clicked, or another one the click's
 *  handlers sent focus to: closing a panel returns focus to the button that
 *  opened it, and Space would open the panel again. Focus a handler put
 *  anywhere else (a panel's heading, a form's first field) stays. */
export function releasesFocus(pointerClick: boolean, focusIsOnAControlThisModuleMarked: boolean): boolean {
  return pointerClick && focusIsOnAControlThisModuleMarked;
}

/** Pure: whether a key press is the automatic repeat of a held key on a
 *  control, which must not press it again. */
export function blocksRepeat(key: string, repeat: boolean, onControl: boolean): boolean {
  return repeat && onControl && (key === "Enter" || key === " ");
}

/** Pure: whether a key press activates an element that is a button by role
 *  only. Native controls answer Enter and Space themselves. */
export function activatesByKey(key: string, roleButton: boolean, native: boolean, alreadyHandled: boolean): boolean {
  return roleButton && !native && !alreadyHandled && (key === "Enter" || key === " ");
}

const NATIVE = "button, a[href], summary, input, select, textarea";
const CONTROL = 'button, a[href], summary, [role="button"], [role="tab"]';

/** Runs makeTabbable() once for what is already in the DOM, then for every
 *  subtree a later change adds, and installs the two listeners described at
 *  the top of this file. Call once, at boot. */
export function watchTabbable(): void {
  makeTabbable(document);
  new MutationObserver((records) => {
    for (const record of records) {
      record.addedNodes.forEach((node) => {
        if (node instanceof HTMLElement) makeTabbable(node);
      });
    }
  }).observe(document.body, { childList: true, subtree: true });

  // Which kind of input came last. A click that Enter, Space or an arrow key
  // made follows a keydown; one the pointer made follows a pointerdown.
  let pointer = false;
  document.addEventListener("pointerdown", () => (pointer = true), true);
  document.addEventListener("keydown", () => (pointer = false), true);

  // On the document, in the bubble phase: every handler of the click itself
  // has run by now and has put focus where it wants it.
  const release = () => {
    const focused = document.activeElement as HTMLElement | null;
    if (!releasesFocus(pointer, focused?.hasAttribute?.(ADDED) === true)) return;
    // To <body>, which carries tabindex="-1" for this (src/focus.ts): focus
    // sent nowhere leaves Tab dead in this WebKit.
    document.body.focus({ preventScroll: true });
  };
  for (const type of ["click", "auxclick", "contextmenu"]) document.addEventListener(type, release);

  document.addEventListener("keydown", (e) => {
    const el = e.target as HTMLElement | null;
    if (!el?.matches) return;
    if (blocksRepeat(e.key, e.repeat, el.matches(CONTROL))) {
      e.preventDefault();
      return;
    }
    if (!activatesByKey(e.key, el.matches('[role="button"]'), el.matches(NATIVE), e.defaultPrevented)) return;
    e.preventDefault(); // Space would scroll the view
    el.click();
  });
}
