//! UI locale for tray + Windows toasts. The popover translates in TypeScript
//! from the same dictionaries (`src/i18n.ts` + `src/locales/*.json`); these
//! functions paint the strings that only Rust ever renders (the tray
//! tooltip, the Quit menu item) from the identical files, so there is one
//! dictionary per language for both halves instead of a parallel Rust table
//! that could drift from the JSON one.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The one list of locales on the Rust side. A later task adds a language by
/// appending here, adding a row to `WINDOWS_LANGIDS` and/or `ENV_PREFIXES` if
/// it needs one, and adding a match arm for it in `locale_source` below
/// (`include_str!` needs a literal path, so it cannot be driven from this
/// list directly). `locale_source` has no catch-all: forgetting that arm
/// fails `every_locale_file_parses` loudly instead of silently degrading
/// that locale to English.
pub const LOCALES: &[&str] = &["en", "zh", "ru"];

/// Primary Windows UI language id (`langid & 0x03FF`) → locale. Only the
/// locales that need a non-English match have a row; anything else falls
/// back to "en". Used on Windows and, via `locale_for_langid`, in tests.
#[cfg(any(windows, test))]
const WINDOWS_LANGIDS: &[(u16, &str)] = &[(0x04, "zh"), (0x19, "ru")];

/// `LC_ALL` / `LC_MESSAGES` / `LANG` tag prefix (lowercased) → locale.
const ENV_PREFIXES: &[(&str, &str)] = &[("zh", "zh"), ("ru", "ru")];

/// `include_str!` needs a literal path per file, so this is the one place
/// that lists them; `LOCALES` above stays the only list of which locales
/// exist. Deliberately no catch-all: a `LOCALES` entry with no arm here
/// returns `None` rather than silently reading the English file, so an
/// omitted arm is a loud test failure (`every_locale_file_parses`) instead
/// of that locale quietly always showing English. `dict()` below is the one
/// place that decides what an unmatched locale falls back to at runtime.
fn locale_source(locale: &str) -> Option<&'static str> {
    Some(match locale {
        "en" => include_str!("../../../src/locales/en.json"),
        "zh" => include_str!("../../../src/locales/zh.json"),
        "ru" => include_str!("../../../src/locales/ru.json"),
        _ => return None,
    })
}

/// Every locale's dictionary, parsed once. A `LOCALES` entry with no arm in
/// `locale_source` parses the English file instead, so an unexpected locale
/// still resolves at runtime as designed (the loud failure for that case is
/// `every_locale_file_parses`, not a panic here); a file that fails to parse
/// yields an empty map rather than panicking, which that same test also
/// catches.
fn dict(locale: &str) -> &'static HashMap<String, String> {
    static CACHE: OnceLock<HashMap<&'static str, HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| {
        let en_source = locale_source("en").expect("en.json always has an arm in locale_source");
        LOCALES
            .iter()
            .map(|&l| {
                let raw = locale_source(l).unwrap_or(en_source);
                (l, serde_json::from_str(raw).unwrap_or_default())
            })
            .collect()
    });
    cache
        .get(locale)
        .or_else(|| cache.get("en"))
        .expect("the en locale is always in LOCALES")
}

/// `dict[key]`, empty string treated as absent (a translator's blank cell
/// falls through rather than painting nothing).
fn non_empty<'a>(d: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    d.get(key).map(String::as_str).filter(|s| !s.is_empty())
}

/// `dict(locale)[key]`, falling back to English, then to the key itself —
/// same order as `t()` in `src/i18n.ts`. An empty string counts as missing,
/// so a translator's blank cell paints English rather than nothing.
fn lookup(locale: &str, key: &str) -> String {
    non_empty(dict(locale), key)
        .or_else(|| non_empty(dict("en"), key))
        .unwrap_or(key)
        .to_string()
}

/// Replaces each `{name}` with its value; a brace with no matching var is
/// left untouched. Mirrors the split/join substitution in `t()`.
fn substitute(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in vars {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

pub fn resolved_locale(cfg: &Value) -> &'static str {
    let requested = cfg.get("locale").and_then(Value::as_str);
    match requested.and_then(|l| LOCALES.iter().copied().find(|&loc| loc == l)) {
        Some(locale) => locale,
        None => system_ui_locale(),
    }
}

pub fn quit_label(cfg: &Value) -> String {
    lookup(resolved_locale(cfg), "tray.quit")
}

pub fn metric_label(cfg: &Value, label: &str) -> String {
    let locale = resolved_locale(cfg);
    let key = format!("label.{label}");
    let translated = lookup(locale, &key);
    if translated != key {
        return translated;
    }
    if let Some(model) = label.strip_suffix(" weekly") {
        return substitute(&lookup(locale, "label.weeklySuffix"), &[("model", model)]);
    }
    label.to_string()
}

pub fn pct_left(cfg: &Value, name: &str, label: &str, left: f64) -> String {
    let locale = resolved_locale(cfg);
    let shown = metric_label(cfg, label);
    let left = format!("{left:.0}");
    substitute(&lookup(locale, "tray.pctLeft"), &[("name", name), ("label", &shown), ("left", &left)])
}

/// Windows langid → locale via `WINDOWS_LANGIDS`, matched on the primary
/// language id (`langid & 0x03FF`). Unknown id → "en".
#[cfg(any(windows, test))]
fn locale_for_langid(langid: u16) -> &'static str {
    let primary = langid & 0x03FF;
    WINDOWS_LANGIDS
        .iter()
        .find(|&&(id, _)| id == primary)
        .map(|&(_, locale)| locale)
        .unwrap_or("en")
}

#[cfg(any(windows, test))]
fn langid_is_zh(langid: u16) -> bool {
    locale_for_langid(langid) == "zh"
}

#[cfg(any(windows, test))]
fn langid_is_ru(langid: u16) -> bool {
    locale_for_langid(langid) == "ru"
}

/// Windows *display* language, not the regional-format locale.
/// Same source the popover asks for via `system_ui_locale`.
#[cfg(windows)]
pub fn system_ui_locale() -> &'static str {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    let langid = unsafe { GetUserDefaultUILanguage() };
    locale_for_langid(langid)
}

/// Env tag prefix (already lowercased internally) → locale via
/// `ENV_PREFIXES`. Factored out as a pure function so it is testable without
/// touching the process environment. Unmatched (including empty) → "en".
#[cfg(any(not(windows), test))]
fn locale_for_env_tag(tag: &str) -> &'static str {
    let tag = tag.to_ascii_lowercase();
    ENV_PREFIXES
        .iter()
        .find(|&&(prefix, _)| tag.starts_with(prefix))
        .map(|&(_, locale)| locale)
        .unwrap_or("en")
}

/// macOS / Linux: the first language tag in the usual locale env vars
/// ("zh_CN.UTF-8" → "zh"). GUI launches often carry none of them, which
/// lands on "en"; an explicit `locale` in config always wins over this.
#[cfg(not(windows))]
pub fn system_ui_locale() -> &'static str {
    let tag = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    locale_for_env_tag(&tag)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_locale_wins() {
        assert_eq!(resolved_locale(&json!({"locale": "zh"})), "zh");
        assert_eq!(resolved_locale(&json!({"locale": "en"})), "en");
        assert_eq!(resolved_locale(&json!({"locale": "ru"})), "ru");
    }

    #[test]
    fn zh_metric_labels() {
        let zh = json!({"locale": "zh"});
        assert_eq!(metric_label(&zh, "Session"), "会话");
        assert_eq!(metric_label(&zh, "Sonnet weekly"), "Sonnet 每周");
        assert_eq!(metric_label(&zh, "Rate Limit Resets"), "速率限制重置");
        assert_eq!(metric_label(&json!({"locale": "en"}), "Session"), "Session");
    }

    #[test]
    fn ru_metric_labels() {
        let ru = json!({"locale": "ru"});
        assert_eq!(metric_label(&ru, "Session"), "Сессия");
        assert_eq!(metric_label(&ru, "Sonnet weekly"), "Sonnet за неделю");
        assert_eq!(metric_label(&ru, "Rate Limit Resets"), "Сбросы лимитов");
        assert_eq!(metric_label(&ru, "Recent models"), "Недавние модели");
        assert_eq!(quit_label(&ru), "Выйти из AI Task Manager");
    }

    #[test]
    fn chinese_langids_match() {
        assert!(langid_is_zh(0x0804)); // zh-CN
        assert!(langid_is_zh(0x0404)); // zh-TW
        assert!(langid_is_zh(0x0C04)); // zh-HK
        assert!(!langid_is_zh(0x0409)); // en-US
        assert!(!langid_is_zh(0x0411)); // ja
        assert!(!langid_is_zh(0x0419)); // ru-RU
    }

    #[test]
    fn russian_langids_match() {
        assert!(langid_is_ru(0x0419)); // ru-RU
        assert!(!langid_is_ru(0x0409)); // en-US
        assert!(!langid_is_ru(0x0804)); // zh-CN
    }

    #[test]
    fn every_locale_file_parses() {
        for &locale in LOCALES {
            assert!(
                locale_source(locale).is_some(),
                "{locale} has no arm in locale_source — it would fall back to English silently"
            );
            assert!(!dict(locale).is_empty(), "{locale}.json parsed to an empty dict");
        }
    }

    #[test]
    fn every_locale_has_the_tray_keys() {
        for &locale in LOCALES {
            for key in ["tray.quit", "tray.pctLeft"] {
                let value = dict(locale).get(key);
                assert!(
                    matches!(value, Some(s) if !s.is_empty()),
                    "{locale}.json is missing a non-empty \"{key}\""
                );
            }
        }
    }

    #[test]
    fn every_locale_translates_every_label_any_other_locale_does() {
        fn label_keys(d: &HashMap<String, String>) -> std::collections::BTreeSet<&str> {
            d.keys().filter(|k| k.starts_with("label.")).map(String::as_str).collect()
        }
        let en_keys = label_keys(dict("en"));
        for &locale in LOCALES {
            let d = dict(locale);
            assert_eq!(label_keys(d), en_keys, "{locale}.json's label.* keys differ from en.json");
            for &key in &en_keys {
                assert!(non_empty(d, key).is_some(), "{locale}.json has a blank value for \"{key}\"");
            }
        }
    }

    #[test]
    fn windows_langids_map_to_locales() {
        assert_eq!(locale_for_langid(0x0804), "zh"); // zh-CN
        assert_eq!(locale_for_langid(0x0404), "zh"); // zh-TW
        assert_eq!(locale_for_langid(0x0419), "ru"); // ru-RU
        assert_eq!(locale_for_langid(0x0819), "ru"); // ru-MD
        assert_eq!(locale_for_langid(0x0409), "en"); // en-US
        assert_eq!(locale_for_langid(0xFFFF), "en"); // unknown
    }

    #[test]
    fn env_prefixes_map_to_locales() {
        assert_eq!(locale_for_env_tag("zh_CN.UTF-8"), "zh");
        assert_eq!(locale_for_env_tag("ru_RU"), "ru");
        assert_eq!(locale_for_env_tag("en_US.UTF-8"), "en");
        assert_eq!(locale_for_env_tag(""), "en");
    }

    #[test]
    fn an_unknown_config_locale_falls_back_to_the_system() {
        let resolved = resolved_locale(&json!({"locale": "xx"}));
        assert_eq!(resolved, system_ui_locale());
        assert!(LOCALES.contains(&resolved), "{resolved} is not one of LOCALES");
        assert_ne!(resolved, "xx");
    }
}
