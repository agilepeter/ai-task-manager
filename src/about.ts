// About: who made this, where to learn more, and whose work it stands on.
// The mascot is an original drawing in the HalperBot family. He has a few
// things to say if you keep poking him.

import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { BRAND, CREDITS } from "./brand";

// Short, upbeat, emoji-forward: he is the most cheerful character there is.
const LINES = [
  "Hi! 👋",
  "I counted your tokens! 🔢✨",
  "Pinned versions are my love language 📌💙",
  "Beep! Still under the limit! 🎉",
  "MiniDoge says hi 🐕",
  "I fetched that for you 🦴… wait, wrong job 😅",
  "Fresh session? Fresh start! 🌱",
  "Okay okay, that tickles 🤖💫",
];
let pokes = 0;

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!,
  );
}

const BOT = `
<svg class="ab-bot" viewBox="0 0 240 240" role="img" aria-label="A small friendly robot">
  <g class="ab-bot-body">
    <circle class="ab-antenna-glow" cx="120" cy="34" r="20"/>
    <rect x="116" y="40" width="8" height="26" rx="4" fill="#c2c2cc"/>
    <circle class="ab-antenna" cx="120" cy="34" r="10"/>
    <rect x="30" y="112" width="16" height="44" rx="7" fill="#2a2a35"/>
    <rect x="194" y="112" width="16" height="44" rx="7" fill="#2a2a35"/>
    <rect x="42" y="62" width="156" height="140" rx="42" fill="#d0d0d8"/>
    <rect x="60" y="84" width="120" height="96" rx="24" fill="#1a2030"/>
    <g class="ab-eyes"><rect x="80" y="104" width="22" height="26" rx="9" fill="#5cc8ff"/><rect x="138" y="104" width="22" height="26" rx="9" fill="#5cc8ff"/></g>
    <g fill="#5cc8ff"><rect x="82" y="146" width="9" height="9" rx="2"/><rect x="94" y="152" width="9" height="9" rx="2"/><rect x="106" y="155" width="9" height="9" rx="2"/><rect x="118" y="156" width="9" height="9" rx="2"/><rect x="130" y="155" width="9" height="9" rx="2"/><rect x="142" y="152" width="9" height="9" rx="2"/><rect x="154" y="146" width="9" height="9" rx="2"/></g>
  </g>
</svg>`;

function render(version: string): void {
  const el = document.querySelector<HTMLElement>("#about-body");
  if (!el) return;
  el.innerHTML = `
    <section class="dt-section ab-hero">
      <button class="ab-bot-btn" id="ab-bot" aria-label="Say hi to the robot">${BOT}</button>
      <p class="ab-bubble" id="ab-bubble" aria-live="polite" hidden></p>
      <h2>${esc(BRAND.product)}</h2>
      <p class="ab-tagline">${esc(BRAND.tagline)}</p>
      <p class="ab-maker">Made by <button class="lg-link" data-link="${esc(BRAND.makerUrl)}">${esc(BRAND.maker)}</button> · ${esc(BRAND.by)}</p>
      <p class="dt-caption">Version ${esc(version)}</p>
    </section>
    <section class="dt-section">
      <h3>Learn more</h3>
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
      <h3>Standing on</h3>
      ${CREDITS.map((c) => `<p class="ab-credit"><button class="lg-link" data-link="${esc(c.url)}">${esc(c.name)}</button> by ${esc(c.by)}: ${esc(c.note)}.</p>`).join("")}
      <p class="dt-caption">Open source under the MIT licence. Everything this app reads stays on your computer.</p>
    </section>`;
}

function poke(): void {
  pokes += 1;
  const bot = document.querySelector<HTMLElement>("#ab-bot");
  const bubble = document.querySelector<HTMLElement>("#ab-bubble");
  if (!bot || !bubble) return;
  bubble.textContent = LINES[(pokes - 1) % LINES.length];
  bubble.hidden = false;
  // Restart the little hop; every tenth poke earns the full dance.
  bot.classList.remove("ab-hop", "ab-dance");
  void bot.offsetWidth;
  bot.classList.add(pokes % 10 === 0 ? "ab-dance" : "ab-hop");
  if (pokes % 10 === 0) bubble.textContent = "DANCE BREAK! 🕺💃🤖✨";
}

export function setupAbout(): void {
  const open = () => {
    document.body.classList.add("about-open");
    render("");
    void getVersion().then((v) => render(v), () => render(""));
  };
  const close = () => document.body.classList.remove("about-open");
  document.querySelector("#about-btn")?.addEventListener("click", open);
  document.querySelector("#build-info")?.addEventListener("click", open);
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
