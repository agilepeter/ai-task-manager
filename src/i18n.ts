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

export function setActiveLocale(locale: Locale): void {
  active = locale;
}

export function getLocale(): Locale {
  return active;
}

export function localeTag(): string {
  return LOCALE_TAGS[active];
}

export function t(key: string, vars?: Record<string, string | number>): string {
  let s = DICTS[active][key] ?? DICTS.en[key] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      s = s.split(`{${k}}`).join(String(v));
    }
  }
  return s;
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
