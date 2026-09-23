// Frontend locale: Settings stores auto / en / zh / ru. Metric row labels from
// Rust stay English in config.layout (stars, pins, Customize keys); only
// the painted text switches.
//
// Dictionaries live in src/locales/*.json — one flat file per language, the
// single source both this popover and the Rust tray (task 3) read from.
// This file is logic only.

import en from "./locales/en.json";
import zh from "./locales/zh.json";
import ru from "./locales/ru.json";

export const LOCALES = ["en", "zh", "ru"] as const; // later tasks append
export type Locale = (typeof LOCALES)[number];
export type LocalePref = "auto" | Locale;

type Dict = Record<string, string>;

const DICTS: Record<Locale, Dict> = { en, zh, ru };

const LOCALE_TAGS: Record<Locale, string> = {
  en: "en-US",
  zh: "zh-CN",
  ru: "ru-RU",
};

let active: Locale = "en";
/// Filled from Rust `system_ui_locale` so Auto matches tray/toasts.
let systemLocale: Locale | null = null;

export function detectSystemLocale(): Locale {
  const lang = (navigator.language || "").toLowerCase();
  return LOCALES.find((l) => lang.startsWith(l)) ?? "en";
}

export function setSystemLocale(locale: Locale): void {
  systemLocale = locale;
}

export function resolveLocale(pref: string | undefined): Locale {
  if (pref && (LOCALES as readonly string[]).includes(pref)) return pref as Locale;
  return systemLocale ?? detectSystemLocale();
}

export function normalizeLocalePref(raw: unknown): LocalePref {
  if (raw === "auto") return "auto";
  return typeof raw === "string" && (LOCALES as readonly string[]).includes(raw) ? (raw as Locale) : "auto";
}

/// Same idea as normalizeLocalePref, but for callers (setSystemLocale) that need a
/// bare Locale and cannot take "auto" — normalizeLocalePref's fallback is "auto",
/// this one's is "en", so neither is expressed in terms of the other. A future
/// LOCALES entry alone is enough for every caller that goes through here; main.ts
/// never hardcodes a locale list.
export function asLocale(raw: unknown): Locale {
  return typeof raw === "string" && (LOCALES as readonly string[]).includes(raw) ? (raw as Locale) : "en";
}

export function setActiveLocale(locale: Locale): void {
  active = locale;
}

export function getLocale(): Locale {
  return active;
}

export function localeTag(): string {
  return LOCALE_TAGS[active];
}

/// The wire shape Rust's `Msg` (crates/core/src/i18n.rs) serialises to —
/// serde's `camelCase` rename leaves these three field names unchanged, so
/// the JSON Rust sends decodes directly into this shape. `vars` values are
/// always strings on the wire (Rust's builder stringifies them); `t()`
/// itself is more permissive (numbers too) for TS-side callers.
export type Msg = { key: string; vars: Record<string, string>; count?: number | null };

/// CLDR cardinal-plural forms for the nine languages this app plans to ship.
/// `Locale` is `LOCALES[number]`, so this `Record<Locale, ...>` can only be
/// keyed by locales that already exist — only en/zh/ru are keyed below.
/// Tasks 12-17 add a row each as they extend LOCALES, not a new rule; the
/// full nine-language table (mirrored by `pluralForm` below and by Rust's
/// `plural_form`, which a test keeps identical to this one) is:
///   en, es, de, pt-BR: ["one", "other"]
///   fr:                ["one", "other"]  (0 and 1 both resolve to "one")
///   ru:                ["one", "few", "many"]
///   zh, ja, ko:        ["other"]
export const PLURAL_FORMS: Record<Locale, readonly string[]> = {
  en: ["one", "other"],
  zh: ["other"],
  ru: ["one", "few", "many"],
};

// Rule-family membership for pluralForm, written for all nine locales this
// app plans to ship (not just the three keyed in PLURAL_FORMS above) so a
// future language task adds a locale code to the right family instead of
// inventing new branch logic. Plain string[], not Locale[]: these families
// intentionally list locale codes LOCALES doesn't carry yet. ru's %10/%100
// split doesn't fit a "which family" shape and is handled directly below;
// en/es/de/pt-BR are the default (n === 1 -> "one"), so they need no row.
const PLURAL_ALWAYS_OTHER: readonly string[] = ["zh", "ja", "ko"];
const PLURAL_ONE_IF_0_OR_1: readonly string[] = ["fr"];

/// CLDR cardinal rule for exactly those nine locales. Same rules as Rust's
/// `plural_form`; `scripts/i18n.test.mjs`/the Rust test suite keep the two
/// in step.
export function pluralForm(locale: Locale, n: number): string {
  if (locale === "ru") {
    const mod10 = n % 10;
    const mod100 = n % 100;
    if (mod10 === 1 && mod100 !== 11) return "one";
    if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return "few";
    return "many";
  }
  if (PLURAL_ALWAYS_OTHER.includes(locale)) return "other";
  if (PLURAL_ONE_IF_0_OR_1.includes(locale)) return n === 0 || n === 1 ? "one" : "other";
  return n === 1 ? "one" : "other"; // en, es, de, pt-BR, and the default for anything else
}

/// `dict[key]`, empty string treated as absent -- same convention as t()'s
/// `||` fallback below, factored out so the count-aware path in t() can try
/// several candidate keys without repeating the "" -> undefined dance.
function lookupExact(locale: Locale, key: string): string | undefined {
  return DICTS[locale][key] || undefined;
}

export function t(key: string, vars?: Record<string, string | number>, count?: number): string {
  let s: string;
  if (count === undefined) {
    // `||`-equivalent (via lookupExact): translators hand-edit these files,
    // and a blank cell (present but "") must paint English rather than
    // nothing, same as an absent key does.
    s = lookupExact(active, key) ?? lookupExact("en", key) ?? key;
  } else {
    // key.<form> -> key.other -> key, each through the active -> en -> key
    // fallback in turn, so a locale missing just the picked form still
    // prefers English's version of THAT form over jumping straight to
    // English's "other" or the bare key.
    const form = pluralForm(active, count);
    s =
      lookupExact(active, `${key}.${form}`) ??
      lookupExact("en", `${key}.${form}`) ??
      lookupExact(active, `${key}.other`) ??
      lookupExact("en", `${key}.other`) ??
      lookupExact(active, key) ??
      lookupExact("en", key) ??
      key;
  }
  // An explicit vars.count (rare) wins over the auto-substituted count:
  // spread order puts vars after count, so a vars.count key overwrites it.
  // Rust's render_core must agree on this precedence -- it does, by a
  // different mechanism (see the comment there).
  const allVars = count === undefined ? vars : { count, ...vars };
  if (allVars) {
    for (const [k, v] of Object.entries(allVars)) {
      s = s.split(`{${k}}`).join(String(v));
    }
  }
  return s;
}

export function tm(msg: Msg): string {
  return t(msg.key, msg.vars, msg.count ?? undefined);
}

// Per-locale plural forms via t(key, vars, count) below; every existing call
// site keeps working unchanged since plural()'s own signature does not
// move -- an English-active n=1 still resolves the same `.one` form as
// before, and zh/ru now resolve their own real forms instead of English's
// one/other split.
export function plural(key: string, n: number, vars: Record<string, string | number> = {}): string {
  return t(key, { n, ...vars }, n);
}

export function displayMetricLabel(label: string): string {
  const key = `label.${label}`;
  const translated = t(key);
  if (translated !== key) return translated;
  if (label.endsWith(" weekly")) {
    return t("label.weeklySuffix", { model: label.slice(0, -7) });
  }
  return label;
}

export function displayLinkLabel(label: string): string {
  const key = `link.${label}`;
  const translated = t(key);
  return translated === key ? label : translated;
}

/// Rust still emits English captions ("$21.80 of $79.56 left · 545 credits").
/// Translate the known shapes at paint time so layout keys stay English.
export function displayMetricDetail(text: string): string {
  if (getLocale() === "en" || !text) return text;
  const reset = text.match(/^(.*) · Resets (\d{4}-\d{2}-\d{2} \d{2}:\d{2} UTC)$/);
  if (reset) return `${displayMetricDetail(reset[1])} · ${t("card.resetsAt", { when: reset[2] })}`;
  const states: Record<string, string> = {
    Unknown: "detail.unknown", Expired: "detail.expired", "Quota exhausted": "detail.exhausted",
    Disabled: "detail.disabled", Overdue: "detail.overdue", Wallet: "detail.wallet",
    "Key quota": "detail.keyQuota", Subscription: "detail.subscription", "Unknown type": "detail.unknownType",
  };
  if (states[text]) return t(states[text]);
  const stateParts = text.split(" · ");
  if (stateParts.length > 1 && stateParts.every((part) => states[part])) {
    return stateParts.map((part) => t(states[part])).join(" · ");
  }
  const money = "\\$[\\d,]+(?:\\.\\d+)?K?";
  const num = "[\\d,]+(?:\\.\\d+)?";
  let m = text.match(new RegExp(`^(${money}) of (${money}) left(?: · (\\d+) credits)?$`, "i"));
  if (m) {
    return m[3]
      ? t("detail.moneyOfLeftCredits", { a: m[1], b: m[2], n: m[3] })
      : t("detail.moneyOfLeft", { a: m[1], b: m[2] });
  }
  m = text.match(new RegExp(`^(${money}) left of (${money})$`, "i"));
  if (m) return t("detail.moneyLeftOf", { a: m[1], b: m[2] });
  m = text.match(new RegExp(`^(${money}) of (${money}) used$`, "i"));
  if (m) return t("detail.moneyOfUsed", { a: m[1], b: m[2] });
  m = text.match(new RegExp(`^(${money}) of (${money}) limit$`, "i"));
  if (m) return t("detail.moneyOfLimit", { a: m[1], b: m[2] });
  m = text.match(new RegExp(`^(${money}) of (${money}) monthly cap$`, "i"));
  if (m) return t("detail.moneyOfMonthlyCap", { a: m[1], b: m[2] });
  m = text.match(new RegExp(`^(${money}) of (${money})$`, "i"));
  if (m) return t("detail.moneyOf", { a: m[1], b: m[2] });
  m = text.match(new RegExp(`^(${money}) · (\\d+) credits$`, "i"));
  if (m) return t("detail.moneyCredits", { a: m[1], n: m[2] });
  m = text.match(new RegExp(`^(${num}) of (${num}) credits used$`, "i"));
  if (m) return t("detail.countCreditsUsed", { a: m[1], b: m[2] });
  m = text.match(new RegExp(`^(${num}) of (${num}) used$`, "i"));
  if (m) return t("detail.countOfUsed", { a: m[1], b: m[2] });
  m = text.match(new RegExp(`^(${num}) of (${num}) left$`, "i"));
  if (m) return t("detail.countOfLeft", { a: m[1], b: m[2] });
  if (/^available$/i.test(text.trim())) return t("card.available");
  if (/^unlimited$/i.test(text.trim())) return t("detail.unlimited");
  return text;
}

export function applyStaticI18n(): void {
  document.documentElement.lang = localeTag();
  document.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    const key = el.dataset.i18n;
    if (key) el.textContent = t(key);
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-html]").forEach((el) => {
    const key = el.dataset.i18nHtml;
    if (key) el.innerHTML = t(key);
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-title]").forEach((el) => {
    const key = el.dataset.i18nTitle;
    if (key) {
      el.title = t(key);
      delete el.dataset.tip;
    }
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-placeholder]").forEach((el) => {
    const key = el.dataset.i18nPlaceholder;
    if (key && "placeholder" in el) {
      // Shortcut hints name the platform's own modifier. Both spellings are
      // accepted by the shortcut parser (tested in the app crate).
      const mac = /Mac|iPhone|iPad/.test(navigator.platform);
      (el as HTMLInputElement).placeholder = mac ? t(key).replace("Ctrl+", "Cmd+") : t(key);
    }
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-aria]").forEach((el) => {
    const key = el.dataset.i18nAria;
    if (key) el.setAttribute("aria-label", t(key));
  });
}
