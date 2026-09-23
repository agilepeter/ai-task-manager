//! UI locale for tray + Windows toasts. The popover translates in TypeScript
//! from the same dictionaries (`src/i18n.ts` + `src/locales/*.json`); these
//! functions paint the strings that only Rust ever renders (the tray
//! tooltip, the Quit menu item) from the identical files, so there is one
//! dictionary per language for both halves instead of a parallel Rust table
//! that could drift from the JSON one.

use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
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
/// left untouched. Mirrors the split/join substitution in `t()`. Takes
/// `template` by value: every caller already owns a fresh `String` (from
/// `lookup()` or `render_core`'s candidate search), so taking `&str` here
/// would only force a clone this function immediately throws away.
fn substitute(template: String, vars: &[(&str, &str)]) -> String {
    let mut out = template;
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
        return substitute(lookup(locale, "label.weeklySuffix"), &[("model", model)]);
    }
    label.to_string()
}

pub fn pct_left(cfg: &Value, name: &str, label: &str, left: f64) -> String {
    let locale = resolved_locale(cfg);
    let shown = metric_label(cfg, label);
    let left = format!("{left:.0}");
    substitute(lookup(locale, "tray.pctLeft"), &[("name", name), ("label", &shown), ("left", &left)])
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

/// A `Msg` var: ordinarily literal text, but a var may itself be another
/// `Msg` (a nested sentence rendered in the same locale before it is spliced
/// in). `#[serde(untagged)]` tries `Text` first, so a `Text` var still
/// serialises as a bare JSON string -- the wire shape for every existing,
/// non-nested var is unchanged; only a `.sub()` var gains the extra
/// `{key,vars,count}` shape on the wire.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Var {
    Text(String),
    Msg(Box<Msg>),
}

/// The wire shape the popover's `tm()` (`src/i18n.ts`) decodes: a message
/// key plus its substitution vars and an optional plural count. Rust hands
/// over structured data instead of an already-formatted sentence, so the
/// popover can pick the active locale's own word order and plural form at
/// paint time.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Msg {
    pub key: &'static str,
    pub vars: BTreeMap<&'static str, Var>,
    pub count: Option<i64>,
}

impl Msg {
    pub fn new(key: &'static str) -> Self {
        Self { key, vars: BTreeMap::new(), count: None }
    }

    /// Builder: `Msg::new("a.b").var("x", 1).var("y", "z")`.
    pub fn var(mut self, k: &'static str, v: impl std::fmt::Display) -> Self {
        self.vars.insert(k, Var::Text(v.to_string()));
        self
    }

    /// A var whose value is itself another `Msg`, rendered in the same
    /// locale before substitution. For a sentence that carries two
    /// independent counts (a server count and, inside it, a process count,
    /// say): the outer `Msg`'s own `count` can only select one plural form,
    /// so the second count travels as a nested `Msg` under its own key
    /// (typically a shared `unit.*` one) and picks its own form. A nested
    /// `Msg` is owned (`Box<Msg>`), never a reference back into anything, so
    /// the tree it forms is always finite and `render_core`'s recursion into
    /// it always terminates.
    pub fn sub(mut self, k: &'static str, v: Msg) -> Self {
        self.vars.insert(k, Var::Msg(Box::new(v)));
        self
    }

    /// The English `.one` value spells the number out ("1 MCP server
    /// runs…") rather than using `{count}`, matching the sentences the
    /// existing, English-pinning tests were already written against; a
    /// non-English locale's `.one` is free to use `{count}` instead.
    pub fn count(mut self, n: i64) -> Self {
        self.count = Some(n);
        self
    }
}

/// CLDR cardinal rule for exactly the nine locales this app plans to ship.
/// Same rules as `pluralForm` in `src/i18n.ts`; `plural_forms_match_the_typescript_table`
/// keeps the two identical instead of letting them drift apart by hand.
fn plural_form(locale: &str, n: i64) -> &'static str {
    match locale {
        "ru" => {
            let mod10 = n % 10;
            let mod100 = n % 100;
            if mod10 == 1 && mod100 != 11 {
                "one"
            } else if (2..=4).contains(&mod10) && !(12..=14).contains(&mod100) {
                "few"
            } else {
                "many"
            }
        }
        "zh" | "ja" | "ko" => "other",
        "fr" => {
            if n == 0 || n == 1 {
                "one"
            } else {
                "other"
            }
        }
        // en, es, de, pt-BR, and the default for anything else.
        _ => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

/// Shared candidate search behind `render` and (test-only) `render_with`:
/// try `key.<form>` -> `key.other` -> `key`, each through a `(locale, key)
/// -> Option<String>` lookup the caller supplies -- production hits the
/// real `dict()` cache, tests inject a scratch table. Falls back to the
/// literal key when nothing resolves anywhere, then substitutes {vars} and
/// {count}.
/// `lookup_in` is a trait object, not `impl Fn`: a nested `Var::Msg` recurses
/// into this same function (see the `Var::Msg(m)` arm below), and a generic
/// `impl Fn` recursing into itself makes the compiler try to monomorphize a
/// new `&`-wrapped closure type at every call depth -- an infinite family of
/// types from the type checker's point of view, even though the actual data
/// only ever nests one level deep. A fixed `&dyn Fn` sidesteps that.
fn render_core(lookup_in: &dyn Fn(&str, &str) -> Option<String>, locale: &str, msg: &Msg) -> String {
    let candidates: Vec<String> = match msg.count {
        Some(n) => {
            let form = plural_form(locale, n);
            vec![format!("{}.{form}", msg.key), format!("{}.other", msg.key), msg.key.to_string()]
        }
        None => vec![msg.key.to_string()],
    };

    let template = candidates
        .iter()
        .find_map(|cand| lookup_in(locale, cand).or_else(|| lookup_in("en", cand)))
        .unwrap_or_else(|| msg.key.to_string());

    // Resolve every var to an owned string before substituting: a Text var
    // is used as-is, a Msg var renders itself first -- in the same locale,
    // through the same lookup_in this call is already using, recursively --
    // so a nested Msg picks its own plural form independently of the outer
    // one (see Msg::sub).
    let mut pairs: Vec<(&str, String)> = msg
        .vars
        .iter()
        .map(|(&k, v)| {
            let s = match v {
                Var::Text(s) => s.clone(),
                Var::Msg(m) => render_core(lookup_in, locale, m),
            };
            (k, s)
        })
        .collect();
    // An explicit vars["count"] (rare) wins over the derived count below:
    // vars are listed first and substitute() fully replaces each {name}
    // before moving to the next pair, so the vars entry consumes every
    // {count} occurrence and the later derived one is a no-op. TS's t()
    // must agree on this precedence -- it does, by a different mechanism
    // (see the comment there).
    if let Some(n) = msg.count {
        pairs.push(("count", n.to_string()));
    }
    let refs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
    substitute(template, &refs)
}

/// `key.<plural_form(locale, count)>` -> `key.other` -> `key`; substitutes
/// {vars} and {count}; each candidate falls back en -> key like `lookup`.
pub fn render(locale: &str, msg: &Msg) -> String {
    render_core(&|loc, key| non_empty(dict(loc), key).map(str::to_string), locale, msg)
}

/// `render`, but against an explicit locale -> key -> value table instead of
/// the static, `include_str!`-backed `dict()` cache. Lets tests exercise
/// locales (fr, ja) this build doesn't ship yet, and missing-key fallback,
/// without editing a real locale file.
#[cfg(test)]
fn render_with(dicts: &HashMap<&str, HashMap<&str, &str>>, locale: &str, msg: &Msg) -> String {
    render_core(
        &|loc, key| dicts.get(loc).and_then(|d| d.get(key)).copied().filter(|s| !s.is_empty()).map(str::to_string),
        locale,
        msg,
    )
}

pub fn t(cfg: &Value, msg: &Msg) -> String {
    render(resolved_locale(cfg), msg)
}

/// The test-side key registry for the three hardcoded notifications built
/// in `src-tauri/src/lib.rs` (renewal, client budget, long session). They
/// live in the Tauri crate, which this crate cannot depend on, so their key
/// list is registered here instead -- next to the completeness tests that
/// read it -- rather than beside the code that renders them. Only this
/// module's own test module reads it, so it does not exist in a release
/// build at all.
#[cfg(test)]
pub(crate) const NOTIFY_KEYS: &[&str] = &[
    "notify.renewal.title",
    "notify.renewal.today",
    "notify.renewal.tomorrow",
    "notify.renewal.inDays",
    "notify.renewal.cycleMonthly",
    "notify.renewal.cycleYearly",
    "notify.clientBudget.title",
    "notify.clientBudget.body",
    "notify.longSession.title",
    "notify.longSession.body",
    "notify.longSession.others",
];

/// Same reasoning as `NOTIFY_KEYS`: `export_table` / `export_audit` /
/// `export_clients_csv` are Tauri commands in `src-tauri/src/lib.rs`, so
/// their `error.export.*` keys are registered here instead of beside them.
#[cfg(test)]
pub(crate) const EXPORT_ERROR_KEYS: &[&str] =
    &["error.export.unsupported", "error.export.downloadsDir", "error.export.write"];

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

    #[test]
    fn msg_builder_serialises_camel_case() {
        let msg = Msg::new("a.b").var("x", 1).count(2);
        let value = serde_json::to_value(&msg).unwrap();
        assert_eq!(value, json!({"key": "a.b", "vars": {"x": "1"}, "count": 2}));
    }

    /// A scratch locale -> key -> value table for `render_with`, covering
    /// locales (fr, ja) this build doesn't ship in LOCALES yet, plus an
    /// English-only key for the fallback test below.
    fn scratch_dict() -> HashMap<&'static str, HashMap<&'static str, &'static str>> {
        HashMap::from([
            (
                "en",
                HashMap::from([
                    ("greet.one", "{count} thing"),
                    ("greet.other", "{count} things"),
                    ("enOnly.other", "only in english"),
                ]),
            ),
            (
                "ru",
                HashMap::from([
                    ("greet.one", "{count} штука"),
                    ("greet.few", "{count} штуки"),
                    ("greet.many", "{count} штук"),
                ]),
            ),
            ("fr", HashMap::from([("greet.one", "{count} chose"), ("greet.other", "{count} choses")])),
            ("ja", HashMap::from([("greet.other", "{count} 個")])),
        ])
    }

    #[test]
    fn render_picks_the_locale_s_plural_form() {
        let dicts = scratch_dict();
        let msg = |n: i64| Msg::new("greet").count(n);
        assert_eq!(render_with(&dicts, "ru", &msg(1)), "1 штука");
        assert_eq!(render_with(&dicts, "ru", &msg(3)), "3 штуки");
        assert_eq!(render_with(&dicts, "ru", &msg(5)), "5 штук");
        assert_eq!(render_with(&dicts, "fr", &msg(0)), "0 chose");
        assert_eq!(render_with(&dicts, "ja", &msg(2)), "2 個");
        assert_eq!(render_with(&dicts, "en", &msg(1)), "1 thing");
        assert_eq!(render_with(&dicts, "en", &msg(2)), "2 things");
    }

    #[test]
    fn render_substitutes_a_nested_message_in_the_same_locale() {
        // A `.sub()` var renders its own Msg first, in the SAME locale as
        // the outer one, through the same fallback rules -- so the nested
        // Msg picks its own plural form independently of whatever count (if
        // any) the outer Msg carries. Needed because one sentence can carry
        // two independent counts -- a running-servers count and, inside it,
        // a processes count, say -- and a Msg's own `count` field can only
        // ever select one plural form.
        let dicts: HashMap<&str, HashMap<&str, &str>> = HashMap::from([
            (
                "en",
                HashMap::from([
                    ("outer", "see {inner}"),
                    ("proc.one", "{count} process"),
                    ("proc.other", "{count} processes"),
                ]),
            ),
            (
                "ru",
                HashMap::from([
                    ("outer", "видно {inner}"),
                    ("proc.one", "{count} процесс"),
                    ("proc.few", "{count} процесса"),
                    ("proc.many", "{count} процессов"),
                ]),
            ),
        ]);
        let five = Msg::new("outer").sub("inner", Msg::new("proc").count(5));
        assert_eq!(render_with(&dicts, "ru", &five), "видно 5 процессов");
        let one = Msg::new("outer").sub("inner", Msg::new("proc").count(1));
        assert_eq!(render_with(&dicts, "en", &one), "see 1 process");
    }

    /// Every finding id each emitting module registers, and every check
    /// prefix `audit.rs` registers: `en.json` has a `.title` (bare, or the
    /// full plural-form set) and a `.detail` (same) for each -- unless the
    /// id is in `NO_DETAIL`, where NEITHER branch that produces this prefix
    /// ever carries a detail Msg (pure data, or an empty pass detail; see
    /// audit.rs's check_data). "check.tools.info" is deliberately NOT here:
    /// its empty-list branch has a real detail sentence even though its
    /// names branch does not, so the prefix as a whole still needs the key
    /// this test would otherwise let it skip. This is the loud half of the
    /// pair with `no_orphan_finding_or_check_keys` below: this one catches a
    /// registered id nobody wrote a key for.
    #[test]
    fn every_finding_and_check_id_has_title_and_detail_keys() {
        const NO_DETAIL: &[&str] = &["check.mcp.configured", "check.perm-none.pass"];
        let en = dict("en");
        let has_key_or_forms = |base: &str| -> bool {
            non_empty(en, base).is_some() || ["one", "other"].iter().all(|f| non_empty(en, &format!("{base}.{f}")).is_some())
        };
        let mut prefixes: Vec<String> = crate::inventory::FINDING_IDS
            .iter()
            .chain(crate::coaching::FINDING_IDS)
            .chain(crate::procs::FINDING_IDS)
            .chain(crate::drift::FINDING_IDS)
            .map(|id| format!("finding.{id}"))
            .collect();
        prefixes.extend(crate::audit::CHECK_KEYS.iter().map(|k| k.to_string()));

        for prefix in &prefixes {
            assert!(has_key_or_forms(&format!("{prefix}.title")), "{prefix}.title is missing from en.json");
            if !NO_DETAIL.contains(&prefix.as_str()) {
                assert!(has_key_or_forms(&format!("{prefix}.detail")), "{prefix}.detail is missing from en.json");
            }
        }
    }

    /// Same shape as the test above, for the five newer prefixes:
    /// `alert.*` / `digest.*` / `notify.*` / `hint.*` / `error.*`. Unlike
    /// `finding.*`/`check.*`, none of these have a uniform "one Msg per id,
    /// with a `.title`/`.detail` pair" shape -- a `hint.<id>` is one bare
    /// sentence, `digest.renewal.inDays` is a count-bearing sentence with no
    /// `.title` at all, `alert.reset` has two different body keys for one
    /// title. So each registry lists exact base keys (bare, or the stem of
    /// a count-bearing key) rather than id prefixes needing a suffix
    /// appended, and `has_key_or_forms` (bare-or-complete-forms) is applied
    /// to each one directly.
    #[test]
    fn every_alert_and_notification_key_exists() {
        let en = dict("en");
        let has_key_or_forms = |base: &str| -> bool {
            non_empty(en, base).is_some() || ["one", "other"].iter().all(|f| non_empty(en, &format!("{base}.{f}")).is_some())
        };
        let keys: Vec<&str> = crate::alerts::ALERT_KEYS
            .iter()
            .chain(crate::digest::DIGEST_KEYS)
            .chain(crate::diagnose::HINT_KEYS)
            .chain(NOTIFY_KEYS)
            .chain(crate::pin::ERROR_KEYS)
            .chain(crate::procs::ERROR_KEYS)
            .chain(crate::ledger::ERROR_KEYS)
            .chain(crate::trust::ERROR_KEYS)
            .chain(EXPORT_ERROR_KEYS)
            .copied()
            .collect();
        for key in keys {
            assert!(has_key_or_forms(key), "{key} is missing from en.json (bare or complete plural forms)");
        }
    }

    /// The reverse of the test above: every `finding.*` / `check.*` /
    /// `section.*` / `unit.*` key actually sitting in `en.json` maps back to
    /// something a module registered. Catches a typo'd or orphaned key --
    /// one nothing ever asks `render()` for, which `t()`'s fallback-to-the-
    /// key behaviour would otherwise hide (it would just never be reached,
    /// not fail).
    #[test]
    fn no_orphan_finding_or_check_keys() {
        const UNIT_KEYS: &[&str] = &["unit.times", "unit.process", "unit.more", "unit.less"];
        let finding_prefixes: Vec<String> = crate::inventory::FINDING_IDS
            .iter()
            .chain(crate::coaching::FINDING_IDS)
            .chain(crate::procs::FINDING_IDS)
            .chain(crate::drift::FINDING_IDS)
            .map(|id| format!("finding.{id}."))
            .collect();
        let check_prefixes: Vec<String> = crate::audit::CHECK_KEYS.iter().map(|k| format!("{k}.")).collect();
        // Every registered alert/digest/notify/hint/error base, as an exact
        // key or a "base." prefix (for a count-bearing key's forms).
        let exact_or_prefix = |registered: &[&str], key: &str| -> bool {
            registered.iter().any(|&r| key == r || key.starts_with(&format!("{r}.")))
        };
        let alert_keys = crate::alerts::ALERT_KEYS;
        let digest_keys = crate::digest::DIGEST_KEYS;
        let hint_keys = crate::diagnose::HINT_KEYS;
        let notify_keys = NOTIFY_KEYS;
        let error_keys: Vec<&str> = crate::pin::ERROR_KEYS
            .iter()
            .chain(crate::procs::ERROR_KEYS)
            .chain(crate::ledger::ERROR_KEYS)
            .chain(crate::trust::ERROR_KEYS)
            .chain(EXPORT_ERROR_KEYS)
            .copied()
            .collect();

        for key in dict("en").keys() {
            if key.starts_with("finding.") {
                assert!(finding_prefixes.iter().any(|p| key.starts_with(p.as_str())), "orphan finding key: {key}");
            } else if key.starts_with("check.") {
                assert!(check_prefixes.iter().any(|p| key.starts_with(p.as_str())), "orphan check key: {key}");
            } else if key.starts_with("section.") {
                assert!(crate::audit::SECTION_KEYS.contains(&key.as_str()), "orphan section key: {key}");
            } else if key.starts_with("unit.") {
                assert!(
                    UNIT_KEYS.iter().any(|&u| key == u || key.starts_with(&format!("{u}."))),
                    "orphan unit key: {key}"
                );
            } else if key.starts_with("alert.") {
                assert!(exact_or_prefix(alert_keys, key), "orphan alert key: {key}");
            } else if key.starts_with("digest.") {
                assert!(exact_or_prefix(digest_keys, key), "orphan digest key: {key}");
            } else if key.starts_with("notify.") {
                assert!(exact_or_prefix(notify_keys, key), "orphan notify key: {key}");
            } else if key.starts_with("hint.") {
                assert!(exact_or_prefix(hint_keys, key), "orphan hint key: {key}");
            } else if key.starts_with("error.") {
                assert!(exact_or_prefix(&error_keys, key), "orphan error key: {key}");
            }
        }
    }

    #[test]
    fn render_falls_back_to_english_then_the_key() {
        let dicts = scratch_dict();
        // "enOnly" has no ru forms at all: candidates enOnly.few and
        // enOnly.other both miss in ru, then enOnly.other hits in en.
        assert_eq!(render_with(&dicts, "ru", &Msg::new("enOnly").count(3)), "only in english");
        // Present nowhere -> the literal key comes back unresolved.
        assert_eq!(render_with(&dicts, "ru", &Msg::new("nowhere.atAll")), "nowhere.atAll");
    }

    /// Parses the `PLURAL_FORMS` array literal (not its conditional-logic
    /// twin, `pluralForm`, which would need a much less mechanical parse)
    /// out of the real `src/i18n.ts`, so the file this test reads is the
    /// same one a developer edits -- not a hand-copied fixture that could
    /// silently drift from it.
    fn parse_plural_forms_table(ts_source: &str) -> HashMap<String, Vec<String>> {
        // \b...\b, not a bare substring match: an earlier decoy identifier
        // that merely starts with "PLURAL_FORMS" (e.g. a hypothetical
        // PLURAL_FORMS_BY_ROOT) would otherwise match first and hand back
        // ITS object body instead of the real table's -- see
        // parse_plural_forms_table_skips_an_earlier_by_root_decoy below.
        let block_re = regex::Regex::new(r"(?s)\bPLURAL_FORMS\b[^=]*=\s*\{(.*?)\n\};").expect("valid regex");
        let block = block_re
            .captures(ts_source)
            .unwrap_or_else(|| panic!("could not find a `PLURAL_FORMS = {{ ... }};` block in src/i18n.ts"))
            .get(1)
            .unwrap()
            .as_str();
        let row_re = regex::Regex::new(r#"([\w-]+):\s*\[([^\]]*)\]"#).expect("valid regex");
        let form_re = regex::Regex::new(r#""([a-z]+)""#).expect("valid regex");
        let table: HashMap<String, Vec<String>> = row_re
            .captures_iter(block)
            .map(|cap| {
                let locale = cap[1].to_string();
                let forms = form_re.captures_iter(&cap[2]).map(|m| m[1].to_string()).collect();
                (locale, forms)
            })
            .collect();
        assert!(!table.is_empty(), "parsed zero rows out of PLURAL_FORMS in src/i18n.ts");
        table
    }

    #[test]
    fn parse_plural_forms_table_skips_an_earlier_by_root_decoy() {
        let decoy_source = concat!(
            "const PLURAL_FORMS_BY_ROOT = {\n",
            "  en: [\"decoy-should-not-be-parsed\"],\n",
            "};\n",
            "\n",
            "export const PLURAL_FORMS: Record<Locale, readonly string[]> = {\n",
            "  en: [\"one\", \"other\"],\n",
            "  ru: [\"one\", \"few\", \"many\"],\n",
            "};\n",
        );
        let mut expected: HashMap<String, Vec<String>> = HashMap::new();
        expected.insert("en".to_string(), vec!["one".to_string(), "other".to_string()]);
        expected.insert("ru".to_string(), vec!["one".to_string(), "few".to_string(), "many".to_string()]);
        assert_eq!(parse_plural_forms_table(decoy_source), expected);
    }

    #[test]
    fn plural_forms_match_the_typescript_table() {
        // For each locale PLURAL_FORMS lists in src/i18n.ts, the SET of
        // forms Rust's plural_form produces over a fixed n sample must
        // equal that locale's listed forms exactly: every listed form is
        // reachable (e.g. ru really does hit few and many in this sample,
        // not just one/many), and Rust never invents a form TypeScript
        // doesn't know about. A mismatch here means the two files' rules
        // have drifted apart.
        let ts_source = include_str!("../../../src/i18n.ts");
        let table = parse_plural_forms_table(ts_source);
        // A locale dropped from either side (a deleted LOCALES entry, or a
        // deleted/renamed PLURAL_FORMS row) must fail here loudly -- the
        // loop below only ever iterates what it parsed, so on its own a
        // missing row would just silently check nothing for that locale
        // instead of failing.
        let parsed_locales: std::collections::BTreeSet<&str> = table.keys().map(String::as_str).collect();
        let known_locales: std::collections::BTreeSet<&str> = LOCALES.iter().copied().collect();
        assert_eq!(
            parsed_locales, known_locales,
            "PLURAL_FORMS in src/i18n.ts and Rust's LOCALES list different locales"
        );
        let samples: [i64; 12] = [0, 1, 2, 3, 5, 11, 12, 21, 22, 25, 100, 101];
        for (locale, expected_forms) in &table {
            let expected: std::collections::BTreeSet<&str> = expected_forms.iter().map(String::as_str).collect();
            let produced: std::collections::BTreeSet<&str> =
                samples.iter().map(|&n| plural_form(locale, n)).collect();
            assert_eq!(
                produced, expected,
                "{locale}: plural_form over n={samples:?} produced {produced:?}, but src/i18n.ts's PLURAL_FORMS says {expected:?}"
            );
        }
    }
}
