// About: who made this, where to learn more, and whose work it stands on.
// The mascot is an original drawing in the HalperBot family. He has a few
// things to say if you keep poking him.

import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { BRAND, CREDITS } from "./brand";
import { focusOrFallback } from "./focus";
import { t } from "./i18n";

const T = (k: string, v?: Record<string, string | number>) => t(`about.${k}`, v);

let pokes = 0;
/** The version render() last drew, so rerender() can redraw with it. */
let lastVersion = "";
/** The button that opened this panel (#about-btn in the auto-hiding rail, or
 *  #build-info in the footer -- see CLAUDE.md's About section), restored on
 *  close. Same reasoning as src/agents.ts's own `opener`: WebKit does not
 *  focus a button on a mouse click, so the click handler has to hand this in
 *  explicitly rather than close() reading `document.activeElement` back. */
let opener: HTMLElement | null = null;

/// The bubble text for a given poke count, cycling through 8 lines. Each case
/// is a literal T("egg....") call, not a computed key, so i18n.test.mjs's
/// KEYED_SOURCES coverage check (which greps for quoted literals) actually
/// sees every egg.* reference. Short, upbeat, emoji-forward: he is the most
/// cheerful character there is. Each locale gets its own real joke, not a
/// word-for-word one, with the emoji kept.
function eggLine(n: number): string {
  switch ((n - 1) % 8) {
    case 0: return T("egg.hi");
    case 1: return T("egg.tokens");
    case 2: return T("egg.pinned");
    case 3: return T("egg.beep");
    case 4: return T("egg.minidoge");
    case 5: return T("egg.fetch");
    case 6: return T("egg.freshSession");
    default: return T("egg.tickles");
  }
}

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

/// A function, not a module-level constant: the aria-label must re-read
/// through T() on every render so a locale switch actually changes it.
function botSvg(): string {
  return `
<svg class="ab-bot" viewBox="0 0 240 240" role="img" aria-label="${esc(T("bot.ariaLabel"))}">
  <g class="ab-bot-body">
    <circle class="ab-antenna-glow" cx="120" cy="34" r="20"/>
    <rect x="116" y="40" width="8" height="26" rx="4" fill="#c2c2cc"/>
    <circle class="ab-antenna" cx="120" cy="34" r="10"/>
    <rect x="30" y="112" width="16" height="44" rx="7" fill="#2a2a35"/>
    <rect x="194" y="112" width="16" height="44" rx="7" fill="#2a2a35"/>
    <rect x="42" y="62" width="156" height="140" rx="42" fill="#d0d0d8"/>
    <rect x="60" y="84" width="120" height="96" rx="24" fill="#1a2030"/>
    <g class="ab-eyes" fill="none" stroke="#5cc8ff" stroke-width="8" stroke-linecap="round"><path d="M80 130v-7a11 11 0 0 1 22 0v7"/><path d="M138 130v-7a11 11 0 0 1 22 0v7"/></g>
    <g fill="#5cc8ff"><rect x="82" y="146" width="9" height="9" rx="2"/><rect x="94" y="152" width="9" height="9" rx="2"/><rect x="106" y="155" width="9" height="9" rx="2"/><rect x="118" y="156" width="9" height="9" rx="2"/><rect x="130" y="155" width="9" height="9" rx="2"/><rect x="142" y="152" width="9" height="9" rx="2"/><rect x="154" y="146" width="9" height="9" rx="2"/></g>
  </g>
</svg>`;
}

function render(version: string): void {
  lastVersion = version;
  const el = document.querySelector<HTMLElement>("#about-body");
  if (!el) return;
  // CREDITS is brand.ts data (MIT attribution, stays fixed in any fork per
  // CLAUDE.md); the connecting "by" stays with it rather than being the one
  // translated word inside an otherwise-English attribution line.
  const credits = CREDITS.map(
    (c) => `<p class="ab-credit"><button class="lg-link" data-link="${esc(c.url)}">${esc(c.name)}</button> by ${esc(c.by)}: ${esc(c.note)}.</p>`,
  ).join("");
  el.innerHTML = `
    <section class="dt-section ab-hero">
      <button class="ab-bot-btn" id="ab-bot" aria-label="${esc(T("bot.pokeAriaLabel"))}">${botSvg()}</button>
      <p class="ab-bubble" id="ab-bubble" aria-live="polite" hidden></p>
      <h2>${esc(BRAND.product)}</h2>
      <p class="ab-tagline">${esc(BRAND.tagline)}</p>
      <p class="ab-maker">${esc(T("madeBy"))} <button class="lg-link" data-link="${esc(BRAND.makerUrl)}">${esc(BRAND.maker)}</button> · ${esc(BRAND.by)}</p>
      <p class="dt-caption">${esc(T("version", { version }))}</p>
    </section>
    <section class="dt-section">
      <h3>${esc(T("learnMore"))}</h3>
      ${BRAND.links
        .map(
          (l) => `
        <button class="ab-link" data-link="${esc(l.url)}">
          <span class="ab-link-main"><b>${esc(l.label)}</b><span>${esc(l.hint)}</span></span><span aria-hidden="true">↗</span>
        </button>`,
        )
        .join("")}
    </section>
    <section class="dt-section">
      <h3>${esc(T("standingOn"))}</h3>
      ${credits}
      <p class="dt-caption">${esc(T("license"))}</p>
    </section>`;
}

function poke(): void {
  pokes += 1;
  const bot = document.querySelector<HTMLElement>("#ab-bot");
  const bubble = document.querySelector<HTMLElement>("#ab-bubble");
  if (!bot || !bubble) return;
  bubble.textContent = eggLine(pokes);
  bubble.hidden = false;
  // Restart the little hop; every tenth poke earns the full dance.
  bot.classList.remove("ab-hop", "ab-dance");
  void bot.offsetWidth;
  bot.classList.add(pokes % 10 === 0 ? "ab-dance" : "ab-hop");
  if (pokes % 10 === 0) bubble.textContent = T("egg.dance");
}

/// Redraws the About panel in place, e.g. after a locale switch (task 7
/// wires this into the locale-change handler). A no-op while the panel is
/// closed. render() takes the version string (fetched async, so it isn't
/// known up front the way audit's report or ledger's data are), so this
/// redraws with the last version render() actually drew.
export function rerender(): void {
  if (document.body.classList.contains("about-open")) render(lastVersion);
}

/// Same decision src/agents.ts's closeFocusTarget() makes, kept as this
/// view's own copy -- the convention every view here follows (see
/// audit.ts's own comment on why: testable with plain placeholder objects,
/// no real DOM required, and independent views don't share this).
export function closeFocusTarget(
  opener: HTMLElement | null,
  openerStillInDocument: boolean,
  fallback: HTMLElement | null,
): HTMLElement | null {
  if (opener && openerStillInDocument) return opener;
  return fallback;
}

function close(): void {
  document.body.classList.remove("about-open");
  // Off screen again -- inert takes this whole panel (and everything in it)
  // out of the Tab order while it is closed, same as every other slide-in
  // panel; see index.html's own comment on why a closed panel otherwise stays
  // reachable (it is moved by `transform`, never unrendered).
  document.querySelector("#about")?.setAttribute("inert", "");
  const fallback = document.querySelector<HTMLElement>("#about-btn");
  const stillThere = opener != null && document.contains(opener);
  focusOrFallback(closeFocusTarget(opener, stillThere, fallback));
  opener = null;
}

/// Opens the panel -- `opener` is the button close() returns focus to when
/// it is still in the document once the panel closes again (see
/// closeFocusTarget() above and src/agents.ts's own openAgents() for why this
/// has to be handed in explicitly rather than read back from
/// document.activeElement). Exported so a test can drive it directly, the
/// same way scripts/agents-view.test.mjs drives openAgents().
export function openAbout(opener_: HTMLElement | null = null): void {
  opener = opener_;
  document.body.classList.add("about-open");
  document.querySelector("#about")?.removeAttribute("inert");
  render("");
  void getVersion().then((v) => render(v), () => render(""));
  // The heading lives in the static panel head (index.html), not in
  // anything render() paints, so it is already there to receive focus --
  // same as src/agents.ts's own #agents-heading.
  document.querySelector<HTMLElement>("#about-heading")?.focus();
}

export function setupAbout(): void {
  document.querySelector("#about-btn")?.addEventListener("click", (e) => openAbout(e.currentTarget as HTMLElement));
  document.querySelector("#build-info")?.addEventListener("click", (e) => openAbout(e.currentTarget as HTMLElement));
  document.querySelector("#about-close")?.addEventListener("click", close);
  document.querySelector("#about-body")?.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    if (target.closest("#ab-bot")) return poke();
    const url = target.closest<HTMLElement>("[data-link]")?.dataset.link;
    if (url) void invoke("open_link", { url }).catch(() => {});
  });
  document.addEventListener(
    "keydown",
    (e) => {
      if (e.key === "Escape" && document.body.classList.contains("about-open")) {
        e.stopImmediatePropagation();
        e.preventDefault();
        close();
      }
    },
    true,
  );
}
