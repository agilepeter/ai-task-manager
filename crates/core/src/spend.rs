//! Local spend computation — the "Total Spend" dashboard, per-provider
//! Today / Yesterday / Last 30 Days rows, per-model breakdowns, and the
//! 30-day Usage Trend series. Mirrors the macOS app: costs are derived
//! from the session logs each CLI already writes on this machine, so
//! nothing is sent anywhere.
//!
//! Large logs are handled with a per-file cache keyed by (mtime, size):
//! only files that changed since the last refresh are re-parsed.

use chrono::{DateTime, Datelike, Local, NaiveDate, Utc};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::pricing;
use crate::providers;

pub const TREND_DAYS: usize = 30;

/// Parses `AITM_TODAY`'s raw value as an ISO `YYYY-MM-DD` date -- the one
/// shape scripts/make-demo-fixture.py ever writes. Kept separate from the
/// env read so the parsing rule has one home and can be unit-tested without
/// touching process environment state.
fn parse_today_override(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()
}

/// Which local calendar day counts as "today" for every day-bucketing
/// cutoff in this module -- the last-30-days window, the trend window, a
/// session's own recency filter. `Local::now().date_naive()` normally;
/// `AITM_TODAY` (an ISO `YYYY-MM-DD` date), when set and parseable, stands
/// in for it instead. Real users never set this, so nothing about a live
/// install changes. It exists for scripts/make-demo-fixture.py, which
/// writes a synthetic month of session logs dated relative to one fixed
/// instant: without a matching override here, this function kept reading
/// the real date while the log content stayed frozen, so the 30-day window
/// would walk past the fixture's own data a little more every day real time
/// moved on, and every unrelated regeneration reshuffled the fixture's
/// weekday-dependent random draws along the way.
pub fn today_naive_date() -> NaiveDate {
    if let Ok(raw) = std::env::var("AITM_TODAY") {
        if let Some(d) = parse_today_override(&raw) {
            return d;
        }
        // Set but not an ISO date -- almost certainly a typo in a shell
        // export while poking at the fixture script, so name the value
        // that got rejected rather than silently reading the real clock
        // and leaving the mismatch to be found later. Debug-only: never
        // worth a release build's stderr, and either way the real clock
        // beneath this keeps a live install running normally.
        #[cfg(debug_assertions)]
        eprintln!("AITM_TODAY={raw:?} is not an ISO YYYY-MM-DD date; using the real date instead");
    }
    Local::now().date_naive()
}

/// The CE-ordinal form of `today_naive_date()` -- what every day-bucketing
/// cutoff in this file actually compares against.
pub fn today_days_from_ce() -> i32 {
    today_naive_date().num_days_from_ce()
}

#[derive(Serialize, Clone, serde::Deserialize)]
pub struct ModelSpend {
    pub model: String,
    pub cost: f64,
    pub tokens: f64,
}

#[derive(Serialize, serde::Deserialize, Clone, Default)]
pub struct Window {
    pub cost: f64,
    pub tokens: f64,
    pub models: Vec<ModelSpend>,
}

/// Two seven-day windows side by side, so a number can say which way it is
/// going. `change_percent` is absent when last week was zero: "up from
/// nothing" is not a percentage, and printing one would invent a trend.
#[derive(Serialize, serde::Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WeekDelta {
    pub this_week: f64,
    pub last_week: f64,
    pub change_percent: Option<f64>,
}

/// The last 7 days against the 7 before them, from a `daily_cost` series that
/// ends today. Needs a full fortnight of readings; with less, there is nothing
/// honest to compare and the answer is None.
pub fn week_over_week(daily_cost: &[f64]) -> Option<WeekDelta> {
    if daily_cost.len() < 14 {
        return None;
    }
    let n = daily_cost.len();
    let this_week: f64 = daily_cost[n - 7..].iter().sum::<f64>() + 0.0;
    let last_week: f64 = daily_cost[n - 14..n - 7].iter().sum::<f64>() + 0.0;
    let change_percent = (last_week > 0.0).then(|| (this_week - last_week) / last_week * 100.0);
    Some(WeekDelta { this_week, last_week, change_percent })
}

#[derive(Serialize, Clone)]
pub struct ProviderSpend {
    pub id: String,
    pub name: String,
    pub today: Window,
    pub yesterday: Window,
    pub last30: Window,
    /// Tokens per day, oldest first — trend[29] is today.
    pub trend: Vec<f64>,
    /// Events whose model no catalog prices. Their measured tokens still
    /// count in token totals/trend, but no dollars are guessed for them
    /// (a deliberate softening of the Mac's exclude-everything semantics:
    /// tokens are facts, only prices are unknown), so dollar figures
    /// under-report and the ⚠ says so.
    pub unpriced: u64,
    pub unpriced_models: Vec<String>,
    /// Dollars per day, oldest first, indexed exactly like `trend`.
    pub daily_cost: Vec<f64>,
    /// Claude Code only: the same windows split by project folder, largest
    /// 30-day cost first. Empty for every other provider.
    pub projects: Vec<ProjectSpend>,
    /// Last 7 days against the 7 before. Derived from `daily_cost` after the
    /// scan, so it is never part of the persisted cache.
    pub week: Option<WeekDelta>,
}

/// One project's share of a provider's spend.
#[derive(Serialize, Clone)]
pub struct ProjectSpend {
    /// The project's path when Claude Code still knows it, else the folder
    /// name it logs under.
    pub project: String,
    pub today: Window,
    pub yesterday: Window,
    pub last30: Window,
    /// Where inside the project the spend went, largest 30-day cost first.
    pub areas: Vec<AreaSpend>,
}

/// One work area's share of a project: a top-level folder, or `(unsorted)`
/// for spend before the session had been anywhere.
#[derive(Serialize, serde::Deserialize, Clone)]
pub struct AreaSpend {
    pub area: String,
    pub today: Window,
    pub yesterday: Window,
    pub last30: Window,
    /// Dollars per day, oldest first with today last, like `daily_cost` on a
    /// card. Lets a calendar month be cut out of the rolling window.
    pub daily_cost: Vec<f64>,
    /// Last 7 days against the 7 before, like the card's. Recomputed after
    /// every scan, so a cache written before this field existed still loads.
    #[serde(default)]
    pub week: Option<WeekDelta>,
}

impl ProviderSpend {
    fn has_data(&self) -> bool {
        self.last30.cost > 0.004 || self.last30.tokens > 0.0 || self.unpriced > 0
    }
}

pub fn provider_spend_has_data(sp: &ProviderSpend) -> bool {
    sp.has_data()
}

/// (local calendar day, model) → (cost, tokens). Day = days since CE.
type DayMap = HashMap<(i32, String), (f64, f64)>;

/// Longest model string admitted as a days/unpriced key. Same bound as
/// catalog canonicals (MAX_PROBE_KEY), which every real model fits;
/// longer names fold into OVERFLOW_MODEL_KEY.
const MAX_MODEL_KEY: usize = MAX_PROBE_KEY;

/// Distinct model keys one file may admit before extras fold into
/// OVERFLOW_MODEL_KEY. Real session logs name a handful of models — the
/// cap stops a hostile log from inflating the maps (and spend_cache.json).
const MAX_MODELS_PER_FILE: usize = 4096;

/// Work-area names come from paths in the logs, so they are capped the same
/// way model names are: a bounded length and a bounded count per file.
const MAX_AREA_KEY: usize = 96;
const MAX_AREAS_PER_FILE: usize = 96;
const OTHER_AREA: &str = "(other)";
/// Spend before the session has been anywhere but its starting folder.
const UNSORTED_AREA: &str = "(unsorted)";

/// Fixed bucket for model names refused by the two caps above. Spend and
/// token totals stay exact — only the per-model attribution merges.
const OVERFLOW_MODEL_KEY: &str = "[over-limit model name]";

/// Everything one file contributes: priced per-day totals plus the tally of
/// unpriced (excluded) events per model name. Cached as a unit so exclusion
/// counts survive the per-file cache.
#[derive(Default, Clone)]
struct FileData {
    days: DayMap,
    /// Claude Code only: the same dollars keyed by (day, work area). A second
    /// view of `days`, never extra spend. See `claude_area`.
    areas: DayMap,
    unpriced: HashMap<String, u64>,
    /// Distinct model keys admitted by `model_key` during this file's
    /// parse — the state behind MAX_MODELS_PER_FILE. Consulted only while
    /// parsing; split helpers don't keep it in step.
    models: HashSet<String>,
    /// Claude Code only, per-file identity rather than spend: `Some(<uuid>)`
    /// when this file is a subagent transcript (`<uuid>/subagents/<name>.jsonl`),
    /// set from the path by `claude_file`/`claude_line`. `merge_data`
    /// deliberately never touches this or `agent` below — combining many
    /// files into `all` / a project's totals leaves no single file's
    /// identity left to report.
    parent_session: Option<String>,
    /// This file's `attributionAgent`, the first one an assistant line
    /// carries (constant for the whole file in practice). `None` either
    /// because this is not a subagent transcript or because none of its
    /// lines have named one yet; `agent_spend` is what turns the latter
    /// into an empty, unattributed display name, never this field directly.
    agent: Option<String>,
}

impl FileData {
    /// Bounded key for a log-supplied model string: within both caps the
    /// name passes through, otherwise OVERFLOW_MODEL_KEY. Model strings
    /// come straight from the logs, so without this a hostile line could
    /// key these maps (and spend_cache.json) with unbounded names.
    fn model_key(&mut self, model: &str) -> String {
        if model.len() <= MAX_MODEL_KEY
            && (self.models.contains(model) || self.models.len() < MAX_MODELS_PER_FILE)
        {
            self.models.insert(model.to_string());
            return model.to_string();
        }
        overflow_key(model)
    }
}

/// The overflow bucket keeps Pi's routing prefix: take_tagged can only
/// claim keys that still start with `{card}\u{1}`, so folding a tagged
/// name into the bare overflow key would strand that usage between cards.
/// Pi's card set is fixed (`claude`, `codex` — see pi_line), so this adds
/// at most one bounded key per card.
fn overflow_key(model: &str) -> String {
    for card in ["claude", "codex"] {
        let prefix = format!("{card}{PI_SEP}");
        if model.starts_with(&prefix) {
            return format!("{prefix}{OVERFLOW_MODEL_KEY}");
        }
    }
    OVERFLOW_MODEL_KEY.to_string()
}

struct FileEntry {
    mtime: SystemTime,
    size: u64,
    /// Pricing-catalog generation the file was priced under. A catalog
    /// refresh bumps the generation; the entry is then kept only if its
    /// recorded price probes still replay identically (see `PriceProbe`).
    gen: u64,
    probes: Vec<PriceProbe>,
    data: FileData,
    /// First / last bytes of the cached prefix — a larger rewrite that
    /// is not an append fails this check and full-parses. Empty means
    /// an older cache entry; those still tail so a busy machine does
    /// not regress. The tail alone is often a stable JSON suffix.
    prefix_head: Vec<u8>,
    prefix_tail: Vec<u8>,
    /// Compact Grok pid→model checkpoint. Restored before a tail parse
    /// so a model-change older than the 1 MB warmup still attributes.
    grok_models: HashMap<i64, String>,
    /// Compact Codex totals/model/gate. Same idea — no 200 MB re-read.
    codex: Option<CodexFileState>,
    /// Claude `{mid}:{rid}` / sidechain checkpoints. A replay older
    /// than the 1 MB warmup still dedups.
    claude: Option<ClaudeFileState>,
    /// Pi message-id checkpoint. Same replay problem as Claude.
    pi_seen: HashSet<String>,
}

/// One pricing question a file's parse asked, together with the answer it
/// got. Replaying the questions under a newer catalog proves whether the
/// file's cached dollars are still exact — if every probe answers the same,
/// re-parsing the file would reproduce the same numbers, so the cached
/// summary stays valid without re-reading a byte. This is what keeps the
/// daily catalog refresh from discarding the whole cache and re-reading
/// hundreds of MB of session logs whose prices didn't actually change.
/// Unique pricing questions stored per file. A log that named thousands
/// of distinct models must not grow the persist file without bound —
/// overflowing this cap forces a re-parse on the next catalog change.
const MAX_PROBES_PER_FILE: usize = 64;
/// Same bound as catalog canonicals. A log with a huge model string must
/// not inflate spend_cache.json; overflow forces a re-parse instead.
const MAX_PROBE_KEY: usize = 128;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum PriceProbe {
    /// `pricing::lookup(key)` returned `price`.
    Lookup { key: String, price: Option<pricing::Price> },
    /// `pricing::fast_multiplier(key)` returned `mult`.
    FastMult { key: String, mult: f64 },
    /// The file asked more unique questions than `MAX_PROBES_PER_FILE`,
    /// or a model string longer than `MAX_PROBE_KEY`. A truncated list
    /// cannot vouch for prices we didn't record.
    Overflow,
}

impl PriceProbe {
    fn still_valid(&self) -> bool {
        match self {
            PriceProbe::Lookup { key, price } => pricing::lookup(key) == *price,
            PriceProbe::FastMult { key, mult } => pricing::fast_multiplier(key) == *mult,
            PriceProbe::Overflow => false,
        }
    }
}

/// Whether a cached file's dollars are still exact under the live catalog.
/// An empty probe list means the parse never asked the catalog (carried
/// costUSD) — keep it only if it actually produced events. Empty probes
/// plus empty data is the failed-open artifact, which must not survive
/// a catalog refresh or it hides that session forever.
fn probes_still_vouch(probes: &[PriceProbe], data: &FileData) -> bool {
    if probes.is_empty() {
        return !data.days.is_empty() || !data.unpriced.is_empty();
    }
    probes.iter().all(PriceProbe::still_valid)
}

thread_local! {
    /// Probe recorder, active only while `file_days` runs a parse. Parsers
    /// route pricing calls through `probe_lookup`/`probe_fast_multiplier`
    /// so each file's entry remembers exactly which prices it depended on.
    static PROBES: std::cell::RefCell<Option<Vec<PriceProbe>>> =
        const { std::cell::RefCell::new(None) };
}

fn record_probe(probe: PriceProbe) {
    PROBES.with(|p| {
        if let Some(list) = p.borrow_mut().as_mut() {
            if list.iter().any(|q| matches!(q, PriceProbe::Overflow) || q == &probe) {
                return;
            }
            if list.len() >= MAX_PROBES_PER_FILE {
                list.push(PriceProbe::Overflow);
                return;
            }
            list.push(probe);
        }
    });
}

/// `pricing::lookup` with the question/answer recorded for cache
/// revalidation. Every parser that runs under `file_days` must use this
/// (and `probe_fast_multiplier`) instead of calling pricing directly —
/// an unrecorded call would make the cached entry look valid after that
/// price changed.
fn probe_lookup(model: &str) -> Option<pricing::Price> {
    let price = pricing::lookup(model);
    record_model_probe(model, |key| PriceProbe::Lookup { key, price });
    price
}

/// `pricing::fast_multiplier`, recorded — see `probe_lookup`.
fn probe_fast_multiplier(model: &str) -> f64 {
    let mult = pricing::fast_multiplier(model);
    record_model_probe(model, |key| PriceProbe::FastMult { key, mult });
    mult
}

fn record_model_probe(model: &str, make: impl FnOnce(String) -> PriceProbe) {
    if model.len() > MAX_PROBE_KEY {
        record_probe(PriceProbe::Overflow);
    } else {
        record_probe(make(model.to_string()));
    }
}

fn cache() -> &'static Mutex<HashMap<PathBuf, FileEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, FileEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// Persistent parse cache. The per-file summaries above are tiny (a few
// day/model totals per file) but rebuilding them means re-reading every
// session log ever written — thousands of files, growing forever. Saving
// the summaries to disk makes a fresh launch re-parse only files that
// changed since the last run.
//
// Trust rules: a PERSIST_VERSION mismatch (cache format *or* a parser
// change — `claude_line` / `codex_line` / `pi_line` / …) or a
// corrections-revision mismatch (the pricing *code* changed) discards
// the cache wholesale. A catalog *file* change (pricing::catalog_stamp
// moved — this happens on every daily/hourly refresh) instead replays
// each entry's recorded price probes: entries whose prices still answer
// the same stay, only files whose prices actually moved re-parse. A
// stale-price cache is worse than a slow first scan — but re-reading
// gigabytes because a catalog mtime ticked is what froze the app on
// "Scanning session logs…" every day.
// ---------------------------------------------------------------------------

/// Bump on any cache-format change or parser-logic change (`claude_line` /
/// `codex_line` / `pi_line` / `sidechain_parent` / …) — the trust rules
/// above explain why a mismatch discards the cache wholesale rather than
/// trying to salvage it. 10: `sidechain_parent` now recognizes a workflow
/// transcript nested under `subagents/workflows/<workflow-id>/`, not just
/// directly under `subagents/`. Without this bump, a cache an older build
/// already wrote would go on treating every one of those as its own
/// phantom session forever, since `cache_unchanged` never re-parses a file
/// whose mtime and size have not moved.
const PERSIST_VERSION: u32 = 10;

/// Set when any file was (re)parsed this run — nothing changed, nothing saved.
static CACHE_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Paths seen by file_days() this collect() run. Entries for paths nobody
/// scanned anymore (deleted logs, disabled providers) are dropped on save,
/// so the cache can't grow without bound.
fn touched() -> &'static Mutex<HashSet<PathBuf>> {
    static TOUCHED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    TOUCHED.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct PersistEntry {
    path: PathBuf,
    /// mtime at full filesystem precision (NTFS is 100ns) — millisecond
    /// rounding would break the equality check and re-parse everything.
    mtime_secs: u64,
    mtime_nanos: u32,
    size: u64,
    days: Vec<(i32, String, f64, f64)>,
    /// Claude work areas, same shape as `days`. Absent in older caches,
    /// which the PERSIST_VERSION bump discards anyway.
    #[serde(default)]
    areas: Vec<(i32, String, f64, f64)>,
    unpriced: Vec<(String, u64)>,
    /// Pricing questions this file's parse asked (see `PriceProbe`).
    /// Older caches without the field deserialize as empty — safe, because
    /// they can only load through the exact-stamp fast path.
    #[serde(default)]
    probes: Vec<PriceProbe>,
    #[serde(default)]
    prefix_head: Vec<u8>,
    #[serde(default)]
    prefix_tail: Vec<u8>,
    #[serde(default)]
    grok_models: Vec<(i64, String)>,
    #[serde(default)]
    codex: Option<CodexFileState>,
    #[serde(default)]
    claude: Option<ClaudeFileState>,
    #[serde(default)]
    pi_seen: Vec<String>,
    /// `FileData::parent_session` / `::agent` — see those fields. Absent in
    /// older caches, which the PERSIST_VERSION bump discards anyway.
    #[serde(default)]
    parent_session: Option<String>,
    #[serde(default)]
    agent: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistFile {
    version: u32,
    pricing_stamp: String,
    /// Baked-pricing revision the entries were priced under. Probes only
    /// witness catalog lookups — a changed corrections revision means the
    /// pricing *code* changed, which probes can't vouch for.
    #[serde(default)]
    corrections: u32,
    entries: Vec<PersistEntry>,
}

fn persist_path() -> PathBuf {
    providers::config_dir().join("spend_cache.json")
}

/// Loads the persisted cache into the in-memory map, once per app run.
/// Runs after pricing::ensure_fresh() so the stamp reflects any catalog
/// download that just happened. An exact stamp match loads everything; a
/// stamp mismatch (a catalog file changed) replays each entry's price
/// probes and keeps the entries whose prices didn't move — only a version
/// or corrections-revision change discards the cache wholesale.
fn load_persisted_cache() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        load_persisted_cache_from(&persist_path());
    });
}

/// True when a persisted doc's format version and pricing-corrections
/// revision both still match what this build writes. Those are the only
/// two mismatches that discard a cache wholesale — a pricing-stamp
/// mismatch alone instead replays price probes, in `load_persisted_cache_from`.
fn persisted_is_current(doc: &PersistFile) -> bool {
    doc.version == PERSIST_VERSION && doc.corrections == pricing::corrections_rev()
}

/// `load_persisted_cache`'s real work, parameterized by path so a test can
/// exercise the keep/discard/replay decision against a throwaway file
/// instead of this machine's own `spend_cache.json`. `load_persisted_cache`
/// itself stays the once-per-run singleton that reads the real path.
fn load_persisted_cache_from(path: &Path) {
    let Ok(raw) = fs::read_to_string(path) else { return };
    let Ok(doc) = serde_json::from_str::<PersistFile>(&raw) else { return };
    if !persisted_is_current(&doc) {
        return;
    }
    let stamp_matches = doc.pricing_stamp == pricing::catalog_stamp();
    if !stamp_matches {
        // Kept entries get re-persisted under the fresh stamp even if
        // no file re-parses this run.
        CACHE_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let gen = pricing::generation();
    let Ok(mut map) = cache().lock() else { return };
    for e in doc.entries {
        let mut data = FileData::default();
        for (day, model, cost, tokens) in e.days {
            data.days.insert((day, model), (cost, tokens));
        }
        for (day, area, cost, tokens) in e.areas {
            data.areas.insert((day, area), (cost, tokens));
        }
        data.unpriced = e.unpriced.into_iter().collect();
        data.parent_session = e.parent_session;
        data.agent = e.agent;
        if !stamp_matches && !probes_still_vouch(&e.probes, &data) {
            continue; // a price this file used changed — re-parse it
        }
        let mtime = SystemTime::UNIX_EPOCH
            + std::time::Duration::new(e.mtime_secs, e.mtime_nanos);
        map.insert(
            e.path,
            FileEntry {
                mtime,
                size: e.size,
                gen,
                probes: e.probes,
                data,
                prefix_head: clip_fingerprint(e.prefix_head, PREFIX_HEAD),
                prefix_tail: clip_fingerprint(e.prefix_tail, PREFIX_TAIL),
                grok_models: clip_grok_models(e.grok_models.into_iter().collect()),
                codex: e.codex,
                claude: clip_claude_ckpt(e.claude),
                pi_seen: clip_pi_seen(e.pi_seen.into_iter().collect()),
            },
        );
    }
}

/// Writes the cache back to disk (atomically, via temp + rename) when this
/// run parsed anything new. Only entries that are current — touched this
/// run and priced under the live catalog generation — are persisted.
fn save_persisted_cache() {
    let dirty = CACHE_DIRTY.swap(false, std::sync::atomic::Ordering::Relaxed);
    let path = persist_path();
    if !dirty && path.exists() {
        return;
    }
    let gen = pricing::generation();
    let Ok(touched_set) = touched().lock() else { return };
    let Ok(map) = cache().lock() else { return };
    let entries: Vec<PersistEntry> = map
        .iter()
        .filter(|(p, e)| e.gen == gen && touched_set.contains(*p))
        .map(|(p, e)| {
            let d = e
                .mtime
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default();
            PersistEntry {
                path: p.clone(),
                mtime_secs: d.as_secs(),
                mtime_nanos: d.subsec_nanos(),
                size: e.size,
                days: e
                    .data
                    .days
                    .iter()
                    .map(|((day, model), (cost, tokens))| (*day, model.clone(), *cost, *tokens))
                    .collect(),
                areas: e
                    .data
                    .areas
                    .iter()
                    .map(|((day, area), (cost, tokens))| (*day, area.clone(), *cost, *tokens))
                    .collect(),
                unpriced: e.data.unpriced.iter().map(|(m, c)| (m.clone(), *c)).collect(),
                probes: e.probes.clone(),
                prefix_head: e.prefix_head.clone(),
                prefix_tail: e.prefix_tail.clone(),
                grok_models: e.grok_models.iter().map(|(k, v)| (*k, v.clone())).collect(),
                codex: e.codex.clone(),
                claude: e.claude.clone(),
                pi_seen: e.pi_seen.iter().cloned().collect(),
                parent_session: e.data.parent_session.clone(),
                agent: e.data.agent.clone(),
            }
        })
        .collect();
    let doc = PersistFile {
        version: PERSIST_VERSION,
        pricing_stamp: pricing::catalog_stamp(),
        corrections: pricing::corrections_rev(),
        entries,
    };
    let Ok(json) = serde_json::to_string(&doc) else { return };
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, json).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

fn day_of_utc(ts: DateTime<Utc>) -> i32 {
    ts.with_timezone(&Local).date_naive().num_days_from_ce()
}

fn add_event(data: &mut FileData, ts: DateTime<Utc>, model: &str, cost: f64, tokens: f64) {
    let key = data.model_key(model);
    let entry = data.days.entry((day_of_utc(ts), key)).or_insert((0.0, 0.0));
    entry.0 += cost;
    entry.1 += tokens;
}

/// Tally an event no catalog can price: its tokens still count (they're
/// measured, not guessed) at zero cost, so only the dollars under-report.
fn note_unpriced(data: &mut FileData, ts: DateTime<Utc>, model: &str, tokens: f64) {
    let key = data.model_key(model);
    *data.unpriced.entry(key).or_insert(0) += 1;
    if tokens > 0.0 {
        // add_event re-derives the same bounded key.
        add_event(data, ts, model, 0.0, tokens);
    }
}

/// Combines two files' totals. `parent_session` and `agent` are deliberately
/// left out: they identify one physical file, and a target that has already
/// absorbed several files has no single file's identity left to report.
fn merge_data(target: &mut FileData, source: FileData) {
    for (key, (cost, tokens)) in source.days {
        let entry = target.days.entry(key).or_insert((0.0, 0.0));
        entry.0 += cost;
        entry.1 += tokens;
    }
    for (key, (cost, tokens)) in source.areas {
        let entry = target.areas.entry(key).or_insert((0.0, 0.0));
        entry.0 += cost;
        entry.1 += tokens;
    }
    for (model, count) in source.unpriced {
        *target.unpriced.entry(model).or_insert(0) += count;
    }
    target.models.extend(source.models);
}

/// Ranked model list for one window: top models by cost, anything past the
/// fifth name or under a 5% share folds into "Other".
fn finalize_models(raw: HashMap<String, (f64, f64)>, window_cost: f64) -> Vec<ModelSpend> {
    let mut list: Vec<ModelSpend> = raw
        .into_iter()
        .map(|(model, (cost, tokens))| ModelSpend { model, cost, tokens })
        .collect();
    list.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap_or(std::cmp::Ordering::Equal));

    let mut named = Vec::new();
    let mut other = ModelSpend { model: "Other".into(), cost: 0.0, tokens: 0.0 };
    for (i, m) in list.into_iter().enumerate() {
        let share = if window_cost > 0.0 { m.cost / window_cost } else { 0.0 };
        if i < 5 && (share >= 0.05 || i == 0) {
            named.push(m);
        } else {
            other.cost += m.cost;
            other.tokens += m.tokens;
        }
    }
    if other.cost > 0.001 || other.tokens > 0.0 {
        named.push(other);
    }
    named
}

fn build_spend(id: impl Into<String>, name: impl Into<String>, data: FileData) -> ProviderSpend {
    let today = today_days_from_ce();
    let mut unpriced_models: Vec<String> = data.unpriced.keys().cloned().collect();
    unpriced_models.sort();
    unpriced_models.truncate(5);
    let days = data.days;
    let mut sp = ProviderSpend {
        week: None,
        id: id.into(),
        name: name.into(),
        today: Window::default(),
        yesterday: Window::default(),
        last30: Window::default(),
        trend: vec![0.0; TREND_DAYS],
        unpriced: data.unpriced.values().sum(),
        unpriced_models,
        daily_cost: vec![0.0; TREND_DAYS],
        projects: Vec::new(),
    };
    let mut models: [HashMap<String, (f64, f64)>; 3] =
        [HashMap::new(), HashMap::new(), HashMap::new()];

    for ((day, model), (cost, tokens)) in days {
        let mut bump = |idx: usize, w: &mut Window| {
            w.cost += cost;
            w.tokens += tokens;
            let entry = models[idx].entry(model.clone()).or_insert((0.0, 0.0));
            entry.0 += cost;
            entry.1 += tokens;
        };
        if day == today {
            bump(0, &mut sp.today);
        }
        if day == today - 1 {
            bump(1, &mut sp.yesterday);
        }
        if day > today - TREND_DAYS as i32 {
            bump(2, &mut sp.last30);
            let idx = (day - (today - TREND_DAYS as i32 + 1)) as usize;
            if idx < TREND_DAYS {
                sp.trend[idx] += tokens;
                sp.daily_cost[idx] += cost;
            }
        }
    }

    let [m0, m1, m2] = models;
    sp.today.models = finalize_models(m0, sp.today.cost);
    sp.yesterday.models = finalize_models(m1, sp.yesterday.cost);
    sp.last30.models = finalize_models(m2, sp.last30.cost);
    sp
}

/// How deep below a scan root directories are visited. Session logs nest a
/// handful of levels at most; the cap keeps a pathological tree from turning
/// the walk into an unbounded crawl.
const MAX_SCAN_DEPTH: usize = 16;

/// Upper bound on directories inspected per scan root, so a link into a huge
/// tree (or `/`) can't stall the refresh thread.
const MAX_SCAN_DIRS: usize = 20_000;

/// Session logs larger than this are skipped whole, with a diagnostic.
/// Upstream drew the line at 512 MiB on the view that a bigger log must be
/// corrupt or hostile, but long-lived agent sessions really do get there (a
/// genuine 723 MiB Claude Code session was being left out of spend). The
/// reader streams, no line is kept past MAX_LINE_BYTES, and after the first
/// pass only appended bytes are re-read, so size costs one slow scan, once.
/// 2 GiB still stops a runaway file from holding the refresh thread forever.
const MAX_LOG_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Stored bytes per JSONL line: a longer physical line is skipped and its
/// remainder read-and-discarded, never kept. Legit Claude/Codex lines
/// reach ~1 MB, so 4 MiB loses nothing real.
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// First / last bytes of a cached prefix — a larger rewrite that is
/// not an append fails this check. Empty (older cache) still tails.
/// The tail alone is often a stable JSON suffix (`"usageScope":…`);
/// 64 bytes at the start usually includes the timestamp.
const PREFIX_HEAD: usize = 64;
const PREFIX_TAIL: usize = 32;
/// Re-parse this much of the cached prefix into a discard `FileData`
/// so Codex/Claude/Grok/Pi closures keep their per-file state. 1 MB
/// is tiny next to a 200 MB session.
const TAIL_WARMUP: u64 = 1024 * 1024;

/// Report a log skipped for exceeding MAX_LOG_FILE_BYTES.
fn oversized_log(path: &Path, size: u64) {
    eprintln!(
        "[aitm] spend: skipping {} — {} MiB exceeds the {} MiB log-file cap",
        path.display(),
        size / (1024 * 1024),
        MAX_LOG_FILE_BYTES / (1024 * 1024),
    );
}

/// All .jsonl files under `root` modified in the last 31 days.
/// Symlinks and junctions are followed throughout: directories are resolved
/// through links when recursing, and the recency check below reads the
/// *target* file's mtime — a link's own (usually ancient) timestamp must not
/// hide logs a user relocated to another drive.
///
/// Because links are followed, the walk is iterative and bounded: it stops at
/// `MAX_SCAN_DEPTH` levels, visits at most `MAX_SCAN_DIRS` directories, and
/// skips canonical paths already seen, so a link cycle can't spin forever.
fn recent_jsonl_files(root: &Path, out: &mut Vec<PathBuf>) {
    let cutoff = SystemTime::now() - Duration::from_secs(31 * 86_400);
    // Canonical paths of link targets already entered. Cycles and aliases can
    // only form through links, so plain directories skip the canonicalize —
    // on Windows it opens a real handle per directory (plus an antivirus
    // round-trip), which made every refresh crawl on big log trees.
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> = Vec::new();
    let mut dirs_visited = 0usize;
    // Set when any link is traversed: a link can alias a subtree that is
    // also reached directly, so only then do the collected files need a
    // canonical-identity dedup (below). Link-free trees pay nothing.
    let mut followed_link = false;
    let first_new = out.len();

    seen.insert(fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));
    stack.push((root.to_path_buf(), 0));

    while let Some((dir, depth)) = stack.pop() {
        dirs_visited += 1;
        if dirs_visited > MAX_SCAN_DIRS {
            eprintln!(
                "[aitm] spend: scan of {} stopped after {MAX_SCAN_DIRS} directories — results may be partial",
                root.display()
            );
            return;
        }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            // The listing itself carries the entry type (free on Windows —
            // no extra stat). Symlinks/junctions report as symlink here, not
            // as their target type.
            let Ok(ftype) = entry.file_type() else { continue };
            if ftype.is_dir() {
                if depth + 1 > MAX_SCAN_DEPTH {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if ftype.is_symlink() {
                // Links still resolve (relocated logs must be found), but
                // only they pay for canonicalize and the seen-set gate.
                let Ok(meta) = fs::metadata(&path) else { continue };
                if meta.is_dir() {
                    if depth + 1 > MAX_SCAN_DEPTH {
                        continue;
                    }
                    let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                    if seen.insert(canonical) {
                        followed_link = true;
                        stack.push((path, depth + 1));
                    }
                } else if path.extension().is_some_and(|e| e == "jsonl")
                    && meta.modified().map(|m| m >= cutoff).unwrap_or(true)
                {
                    // Target metadata, so a link's own ancient mtime can't
                    // hide a recently written log.
                    if meta.len() > MAX_LOG_FILE_BYTES {
                        oversized_log(&path, meta.len());
                        continue;
                    }
                    followed_link = true;
                    out.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                let Ok(meta) = entry.metadata() else { continue };
                if meta.len() > MAX_LOG_FILE_BYTES {
                    oversized_log(&path, meta.len());
                    continue;
                }
                if meta.modified().map(|m| m >= cutoff).unwrap_or(true) {
                    out.push(path);
                }
            }
        }
    }

    // A traversed link may alias a subtree that was also walked directly
    // (either order), which would list — and count — the same log twice
    // under two spellings. Dedup by canonical identity, keeping the first
    // spelling so codex's sessions/-relative dedup keeps working.
    if followed_link {
        let tail: Vec<PathBuf> = out.split_off(first_new);
        let mut identities: HashSet<PathBuf> = HashSet::new();
        for path in tail {
            let id = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if identities.insert(id) {
                out.push(path);
            }
        }
    }
}

/// A later write that only appended bytes. Session logs are JSONL; a
/// rewrite (shrink or same-size mtime bump) must full-parse. A larger
/// rewrite that keeps growing past the cached size is caught by the
/// prefix fingerprint — empty fingerprints (older cache) still tail.
fn cached_prefix(
    map: &HashMap<PathBuf, FileEntry>,
    path: &Path,
    size: u64,
    gen: u64,
) -> Option<(FileData, Vec<PriceProbe>, u64)> {
    let entry = map.get(path)?;
    if size <= entry.size {
        return None;
    }
    if !(entry.gen == gen || probes_still_vouch(&entry.probes, &entry.data)) {
        return None;
    }
    if !prefix_still_matches(path, entry.size, &entry.prefix_head, &entry.prefix_tail) {
        return None;
    }
    Some((entry.data.clone(), entry.probes.clone(), entry.size))
}

/// Hostile/corrupt persist blobs must not sit in RAM. Oversized marks
/// are dropped so the next scan still tails (speed) instead of holding
/// the payload or forcing a full re-read.
const MAX_GROK_PIDS: usize = 256;

fn clip_grok_models(map: HashMap<i64, String>) -> HashMap<i64, String> {
    if map.len() <= MAX_GROK_PIDS {
        map
    } else {
        HashMap::new()
    }
}

/// Hostile/corrupt persist blobs must not sit in RAM. A 20 MB Claude
/// session is hundreds of ids, not tens of thousands — over the cap
/// we drop the checkpoint so the next tail warms 1 MB instead.
const MAX_DEDUP_IDS: usize = 8192;

fn clip_claude_ckpt(st: Option<ClaudeFileState>) -> Option<ClaudeFileState> {
    let st = st?;
    if st.seen.len() > MAX_DEDUP_IDS || st.seen_mids.len() > MAX_DEDUP_IDS {
        None
    } else {
        Some(st)
    }
}

fn clip_pi_seen(seen: HashSet<String>) -> HashSet<String> {
    if seen.len() <= MAX_DEDUP_IDS {
        seen
    } else {
        HashSet::new()
    }
}

fn cache_unchanged(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let size = meta.len();
    let gen = pricing::generation();
    cache()
        .lock()
        .ok()
        .and_then(|map| {
            map.get(path).map(|e| {
                e.mtime == mtime
                    && e.size == size
                    && (e.gen == gen || probes_still_vouch(&e.probes, &e.data))
            })
        })
        .unwrap_or(false)
}

fn will_resume_tail(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let gen = pricing::generation();
    cache()
        .lock()
        .ok()
        .and_then(|map| cached_prefix(&map, path, meta.len(), gen))
        .is_some()
}

fn load_grok_models(path: &Path) -> HashMap<i64, String> {
    cache()
        .lock()
        .ok()
        .and_then(|map| map.get(path).map(|e| e.grok_models.clone()))
        .unwrap_or_default()
}

fn store_grok_models(path: &Path, models: HashMap<i64, String>) {
    if let Ok(mut map) = cache().lock() {
        if let Some(e) = map.get_mut(path) {
            e.grok_models = clip_grok_models(models);
        }
    }
}

fn load_codex_ckpt(path: &Path) -> Option<CodexFileState> {
    cache()
        .lock()
        .ok()
        .and_then(|map| map.get(path).and_then(|e| e.codex.clone()))
}

fn store_codex_ckpt(path: &Path, st: CodexFileState) {
    if let Ok(mut map) = cache().lock() {
        if let Some(e) = map.get_mut(path) {
            e.codex = Some(st);
        }
    }
}

fn load_claude_ckpt(path: &Path) -> Option<ClaudeFileState> {
    cache()
        .lock()
        .ok()
        .and_then(|map| map.get(path).and_then(|e| e.claude.clone()))
}

fn store_claude_ckpt(path: &Path, st: ClaudeFileState) {
    if let Ok(mut map) = cache().lock() {
        if let Some(e) = map.get_mut(path) {
            e.claude = clip_claude_ckpt(Some(st));
        }
    }
}

fn load_pi_seen(path: &Path) -> HashSet<String> {
    cache()
        .lock()
        .ok()
        .and_then(|map| map.get(path).map(|e| e.pi_seen.clone()))
        .unwrap_or_default()
}

fn store_pi_seen(path: &Path, seen: HashSet<String>) {
    if let Ok(mut map) = cache().lock() {
        if let Some(e) = map.get_mut(path) {
            e.pi_seen = clip_pi_seen(seen);
        }
    }
}

fn clip_fingerprint(bytes: Vec<u8>, cap: usize) -> Vec<u8> {
    if bytes.len() <= cap {
        bytes
    } else {
        Vec::new()
    }
}

/// One open, two tiny reads. Four opens on a grown log would add up
/// across hundreds of Codex files.
fn read_prefix_marks(path: &Path, end: u64) -> (Vec<u8>, Vec<u8>) {
    if end == 0 {
        return (Vec::new(), Vec::new());
    }
    let Ok(mut f) = fs::File::open(path) else {
        return (Vec::new(), Vec::new());
    };
    let head_n = PREFIX_HEAD.min(end as usize);
    let mut head = vec![0u8; head_n];
    if f.read_exact(&mut head).is_err() {
        return (Vec::new(), Vec::new());
    }
    let tail_n = PREFIX_TAIL.min(end as usize);
    if f.seek(SeekFrom::Start(end - tail_n as u64)).is_err() {
        return (Vec::new(), Vec::new());
    }
    let mut tail = vec![0u8; tail_n];
    if f.read_exact(&mut tail).is_err() {
        return (Vec::new(), Vec::new());
    }
    (head, tail)
}

fn prefix_still_matches(path: &Path, end: u64, head: &[u8], tail: &[u8]) -> bool {
    // Older cache: both empty — still tail so a busy machine does not
    // regress to a full re-read. Unreadable marks also tail: the later
    // File::open will fail the same way, and a full re-read would be worse.
    if head.is_empty() && tail.is_empty() {
        return true;
    }
    let (got_head, got_tail) = read_prefix_marks(path, end);
    if !head.is_empty() && got_head != head {
        return false;
    }
    if !tail.is_empty() && got_tail != tail {
        return false;
    }
    true
}

fn remember_file(
    path: &Path,
    mtime: SystemTime,
    cached_size: u64,
    gen: u64,
    probes: Vec<PriceProbe>,
    data: FileData,
) {
    let (prefix_head, prefix_tail) = read_prefix_marks(path, cached_size);
    if let Ok(mut map) = cache().lock() {
        let (grok_models, codex, claude, pi_seen) = map
            .get(path)
            .map(|e| {
                (
                    e.grok_models.clone(),
                    e.codex.clone(),
                    e.claude.clone(),
                    e.pi_seen.clone(),
                )
            })
            .unwrap_or_default();
        map.insert(
            path.to_path_buf(),
            FileEntry {
                mtime,
                size: cached_size,
                gen,
                probes,
                data,
                prefix_head,
                prefix_tail,
                grok_models,
                codex,
                claude,
                pi_seen,
            },
        );
    }
    CACHE_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Parses one file into per-day totals, via the cache when unchanged.
/// Growing logs only read the new tail. Stateless parsers (Kimi/Qwen)
/// skip the 1 MB warmup — that I/O is only for Codex/Claude/Grok/Pi.
fn file_days(path: &Path, parse: &mut dyn FnMut(&str, &mut FileData)) -> FileData {
    file_days_inner(path, parse, false)
}

/// Same as `file_days`, but replays the last 1 MB of the cached prefix
/// into a discard `FileData` so the caller's closure keeps Codex totals,
/// Claude mids, Grok pid→model, and Pi seen ids.
fn file_days_stateful(path: &Path, parse: &mut dyn FnMut(&str, &mut FileData)) -> FileData {
    file_days_inner(path, parse, true)
}

fn file_days_inner(
    path: &Path,
    parse: &mut dyn FnMut(&str, &mut FileData),
    warm: bool,
) -> FileData {
    let Ok(meta) = fs::metadata(path) else { return FileData::default() };
    if meta.len() > MAX_LOG_FILE_BYTES {
        // Also gated in recent_jsonl_files; this catches direct-path
        // callers (grok's unified.jsonl). Not touched, so a stale cache
        // entry for it is pruned on the next save.
        oversized_log(path, meta.len());
        return FileData::default();
    }
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let size = meta.len();

    if let Ok(mut t) = touched().lock() {
        t.insert(path.to_path_buf());
    }
    let gen = pricing::generation();
    if let Ok(mut map) = cache().lock() {
        if let Some(entry) = map.get_mut(path) {
            if entry.mtime == mtime && entry.size == size {
                if entry.gen == gen {
                    return entry.data.clone();
                }
                // A catalog refreshed mid-run (generation bump). The file
                // itself is unchanged — keep its summary if every price it
                // used still answers the same, instead of re-reading it.
                if probes_still_vouch(&entry.probes, &entry.data) {
                    entry.gen = gen;
                    CACHE_DIRTY.store(true, std::sync::atomic::Ordering::Relaxed);
                    return entry.data.clone();
                }
            }
        }
    }

    let resume = cache()
        .lock()
        .ok()
        .and_then(|map| cached_prefix(&map, path, size, gen));

    let mut data = resume
        .as_ref()
        .map(|(d, _, _)| d.clone())
        .unwrap_or_default();
    let start_probes = resume
        .as_ref()
        .map(|(_, p, _)| p.clone())
        .unwrap_or_default();
    let mut from = resume.as_ref().map(|(_, _, from)| *from).unwrap_or(0);
    PROBES.with(|p| *p.borrow_mut() = Some(start_probes));
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            // Exists (metadata succeeded) but unreadable this pass — antivirus
            // lock, permissions blip. Do not cache an empty result: empty
            // probes would vouch on the next catalog refresh and the session
            // would vanish for good.
            PROBES.with(|p| {
                p.borrow_mut().take();
            });
            return FileData::default();
        }
    };
    let mut reader = BufReader::new(file);
    // Legacy v4 entries cached the raw EOF, which can sit inside a
    // half-written line. Back up to the previous newline (64 KB) instead
    // of discarding the whole persist file. Already-aligned offsets
    // cost one byte on this open handle.
    if from > 0 {
        from = align_to_line_start(&mut reader, from);
    }
    if from > 0 && warm {
        // Replay a bounded window of the cached prefix into a discard
        // FileData so the caller's closure (Codex totals, Claude mids,
        // Grok pid→model, Pi seen ids) is warm before the real tail.
        // A failed warmup mutates that closure — do not full-rescan
        // with it or prefix events look like duplicates and vanish.
        match seek_warmup(&mut reader, from) {
            Some(warm_pos) if warm_pos < from => {
                let mut discard = FileData::default();
                let (warm_ok, _) =
                    parse_jsonl_reader(&mut reader, parse, &mut discard, warm_pos, Some(from));
                if (!warm_ok || reader.seek(SeekFrom::Start(from)).is_err())
                    && reader.seek(SeekFrom::Start(from)).is_err()
                {
                    PROBES.with(|p| {
                        p.borrow_mut().take();
                    });
                    return data;
                }
            }
            Some(_) => {}
            None => {
                if reader.seek(SeekFrom::Start(from)).is_err() {
                    PROBES.with(|p| {
                        p.borrow_mut().take();
                    });
                    return data;
                }
            }
        }
    } else if from > 0 && reader.seek(SeekFrom::Start(from)).is_err() {
        // No warmup ran, but the closure may still hold a restored
        // checkpoint (Codex/Grok/Claude/Pi). Keep the prefix.
        PROBES.with(|p| {
            p.borrow_mut().take();
        });
        return data;
    }
    let (read_ok, last_complete) = parse_jsonl_reader(&mut reader, parse, &mut data, from, None);
    let probes = PROBES.with(|p| p.borrow_mut().take()).unwrap_or_default();
    if !read_ok {
        // Prefix `data` is already correct. A full retry would reuse
        // warmed/restored parser state and drop those events.
        return data;
    }

    remember_file(path, mtime, last_complete, gen, probes, data.clone());
    data
}

#[allow(dead_code)]
fn file_days_full(
    path: &Path,
    parse: &mut dyn FnMut(&str, &mut FileData),
    mtime: SystemTime,
    _size: u64,
    gen: u64,
) -> FileData {
    let mut data = FileData::default();
    PROBES.with(|p| *p.borrow_mut() = Some(Vec::new()));
    let Ok(file) = fs::File::open(path) else {
        PROBES.with(|p| {
            p.borrow_mut().take();
        });
        return FileData::default();
    };
    let mut reader = BufReader::new(file);
    let (read_ok, last_complete) = parse_jsonl_reader(&mut reader, parse, &mut data, 0, None);
    let probes = PROBES.with(|p| p.borrow_mut().take()).unwrap_or_default();
    if !read_ok {
        return data;
    }
    remember_file(path, mtime, last_complete, gen, probes, data.clone());
    data
}

/// If `from` is mid-line, walk back at most 64 KB to the previous
/// newline. Already-aligned offsets cost one byte. A cache that ended
/// on a complete no-newline JSON value stays put when growth starts
/// with `\n` — backing up would re-parse that record and double it.
/// No newline in the window keeps `from` so we do not re-parse cached
/// complete lines.
fn align_to_line_start(reader: &mut BufReader<fs::File>, from: u64) -> u64 {
    if from == 0 {
        return 0;
    }
    if reader.seek(SeekFrom::Start(from - 1)).is_ok() {
        let mut b = [0u8; 1];
        if reader.read_exact(&mut b).is_ok() && b[0] == b'\n' {
            return from;
        }
    }
    // Cached through a complete record that had no trailing newline.
    // The next scan's new bytes start here; a leading `\n` is a new
    // line, not a resume inside the old object.
    if reader.seek(SeekFrom::Start(from)).is_ok() {
        let mut b = [0u8; 1];
        if reader.read_exact(&mut b).is_ok() && b[0] == b'\n' {
            return from;
        }
    }
    const ALIGN_BACK: u64 = 64 * 1024;
    let start = from.saturating_sub(ALIGN_BACK);
    if reader.seek(SeekFrom::Start(start)).is_err() {
        return from;
    }
    let mut buf = vec![0u8; (from - start) as usize];
    let n = match reader.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return from,
    };
    match buf[..n].iter().rposition(|&c| c == b'\n') {
        Some(i) => start + i as u64 + 1,
        None => from,
    }
}

/// Seek to the 1 MB warmup window just before `from` and skip a
/// partial first line. Returns the byte position the next parse starts
/// at, or `from` if a giant line overshot the cached boundary.
fn seek_warmup(reader: &mut BufReader<fs::File>, from: u64) -> Option<u64> {
    let warm_at = from.saturating_sub(TAIL_WARMUP);
    reader.seek(SeekFrom::Start(warm_at)).ok()?;
    if warm_at == 0 {
        return Some(0);
    }
    let (skipped, _) = skip_line_rest(reader).ok()?;
    let pos = warm_at + skipped;
    if pos > from {
        reader.seek(SeekFrom::Start(from)).ok()?;
        return Some(from);
    }
    Some(pos)
}

/// Returns `(ok, last_complete_pos)`. `last_complete_pos` is the offset
/// after the last record we consumed. A finished last line, or a
/// leftover that is already valid JSON (closed file, no trailing
/// newline), advances the cache. A partial write does not.
fn parse_jsonl_reader(
    reader: &mut impl BufRead,
    parse: &mut dyn FnMut(&str, &mut FileData),
    data: &mut FileData,
    mut pos: u64,
    stop_before: Option<u64>,
) -> (bool, u64) {
    let mut last_complete = pos;
    loop {
        if let Some(limit) = stop_before {
            if pos >= limit {
                return (true, last_complete.min(limit));
            }
        }
        // One physical line, storing at most MAX_LINE_BYTES (+1 byte to
        // detect overflow) — a hostile log must not make a single line
        // allocate without bound.
        let mut buf: Vec<u8> = Vec::new();
        let read = reader
            .by_ref()
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut buf);
        match read {
            Ok(0) => return (true, last_complete),
            Ok(n) if buf.len() > MAX_LINE_BYTES && !buf.ends_with(b"\n") => {
                pos += n as u64;
                // Overlong line: discard the rest of it without storing.
                match skip_line_rest(reader) {
                    Ok((skipped, found_nl)) => {
                        pos += skipped;
                        if found_nl {
                            last_complete = pos;
                        }
                    }
                    Err(_) => return (false, last_complete),
                }
            }
            Ok(n) => {
                pos += n as u64;
                let had_nl = buf.ends_with(b"\n");
                if had_nl {
                    buf.pop();
                    if buf.ends_with(b"\r") {
                        buf.pop();
                    }
                } else {
                    // No newline. A closed log may still end on a
                    // complete JSON value — count it and cache through
                    // EOF. A partial write fails serde and stays
                    // uncached so the next scan retries.
                    match String::from_utf8(buf) {
                        Ok(line)
                            if !line.is_empty()
                                && serde_json::from_str::<Value>(&line).is_ok() =>
                        {
                            parse(&line, data);
                            return (true, pos);
                        }
                        Ok(_) => return (true, last_complete),
                        Err(_) => return (true, last_complete),
                    }
                }
                match String::from_utf8(buf) {
                    Ok(line) => {
                        parse(&line, data);
                        last_complete = pos;
                    }
                    Err(_) => {
                        // lines() treated invalid UTF-8 as a read error;
                        // keep the file out of the cache the same way.
                        return (false, last_complete);
                    }
                }
            }
            Err(_) => return (false, last_complete),
        }
    }
}

/// Consume through the next '\n' (or EOF) using only the reader's own
/// buffer. Returns `(bytes_skipped, found_newline)`.
fn skip_line_rest(reader: &mut impl BufRead) -> std::io::Result<(u64, bool)> {
    let mut n = 0u64;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok((n, false));
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                reader.consume(i + 1);
                return Ok((n + i as u64 + 1, true));
            }
            None => {
                let k = buf.len();
                reader.consume(k);
                n += k as u64;
            }
        }
    }
}

fn parse_ts(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    if let Some(s) = value.as_str() {
        return DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc));
    }
    if let Some(n) = value.as_i64() {
        // Heuristic: values past ~2001-09 in ms are millisecond stamps.
        let ms = if n > 1_000_000_000_000 { n } else { n * 1000 };
        return DateTime::from_timestamp_millis(ms);
    }
    None
}

// ---------------------------------------------------------------------------
// Pricing (dollars per million tokens: input, output, cache read, cache write)
// ---------------------------------------------------------------------------

fn claude_price(model: &str) -> Option<(f64, f64, f64, f64)> {
    let m = model.to_lowercase();
    if m.contains("opus") {
        Some((15.0, 75.0, 1.5, 18.75))
    } else if m.contains("sonnet") {
        Some((3.0, 15.0, 0.3, 3.75))
    } else if m.contains("haiku") {
        Some((1.0, 5.0, 0.1, 1.25))
    } else {
        // Unknown model (or a new family): rely on the log's own costUSD;
        // counting tokens at a guessed price would fabricate dollars.
        None
    }
}

fn codex_price(model: &str) -> (f64, f64, f64) {
    let m = model.to_lowercase();
    if m.contains("mini") || m.contains("spark") {
        (0.25, 2.0, 0.025)
    } else {
        // gpt-5 family / codex defaults
        (1.25, 10.0, 0.125)
    }
}

fn grok_price(model: &str) -> (f64, f64) {
    let m = model.to_lowercase();
    // Grok 4.6's "fast" is a 2x PREMIUM speed tier (launch post: "twice
    // the price"), the opposite of the older grok-4-fast/grok-code-fast
    // line where "fast" meant a smaller, cheaper model — 4.6 slugs must
    // never fall into that cheap branch. (Normally unreachable: the
    // baked-in catalog entry resolves 4.6 before this backstop.)
    if m.contains("4.6") || m.contains("4-6") {
        if m.contains("fast") {
            (4.0, 12.0)
        } else {
            (2.0, 6.0)
        }
    } else if m.contains("code") || m.contains("fast") {
        (0.2, 1.5)
    } else {
        (3.0, 15.0)
    }
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

/// Token buckets of one Claude usage object — a message's `usage` or one
/// advisor iteration inside `usage.iterations` (same field names). `None`
/// when the required input/output counts are missing, or when `speed`
/// carries a value outside the known set (an unrecognized log shape — the
/// Mac skips those lines too).
struct ClaudeTokens {
    input: f64,
    output: f64,
    cache_read: f64,
    w5m: f64,
    w1h: f64,
    fast: bool,
}

impl ClaudeTokens {
    fn total(&self) -> f64 {
        self.input + self.output + self.cache_read + self.w5m + self.w1h
    }
}

fn claude_tokens(u: &Value) -> Option<ClaudeTokens> {
    let input = u.get("input_tokens").and_then(Value::as_f64)?;
    let output = u.get("output_tokens").and_then(Value::as_f64)?;
    let speed = u.get("speed").and_then(Value::as_str);
    if let Some(s) = speed {
        if s != "fast" && s != "standard" {
            return None;
        }
    }
    let num = |k: &str| u.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let cache_write = num("cache_creation_input_tokens");
    // Cache writes split by lifetime when the breakdown is present —
    // 1-hour writes bill at twice the input rate.
    let (w5m, w1h) = match u.get("cache_creation") {
        Some(cc) => {
            let g = |k: &str| cc.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            let (a, b) = (g("ephemeral_5m_input_tokens"), g("ephemeral_1h_input_tokens"));
            if a + b > 0.0 { (a, b) } else { (cache_write, 0.0) }
        }
        None => (cache_write, 0.0),
    };
    Some(ClaudeTokens {
        input,
        output,
        cache_read: num("cache_read_input_tokens"),
        w5m,
        w1h,
        fast: speed == Some("fast"),
    })
}

/// Every computed-cost path funnels here: effective-dated cards (DeepSeek
/// V4.1 Flash's 2026-09-10 changeover bills earlier events at the flat
/// launch card) and the weekday peak windows. Carried costs — dollars the
/// vendor already billed, recorded in the logs — never pass through here,
/// so they're never re-multiplied.
fn cost_for(
    model: &str,
    p: &pricing::Price,
    u: &pricing::Usage,
    long_context_threshold: f64,
    ts: DateTime<Utc>,
) -> f64 {
    let legacy = pricing::v41_flash_legacy_card(model, ts.timestamp_millis());
    let p = legacy.as_ref().unwrap_or(p);
    pricing::request_cost_at(p, u, long_context_threshold)
        * pricing::peak_multiplier(model, ts.timestamp_millis())
}

/// Price one Claude entry: live catalog → static family fallback → None
/// (excluded, never a guessed $0). Fast-flagged requests scale by the
/// supplement's multiplier.
fn claude_cost(model: &str, t: &ClaudeTokens, ts: DateTime<Utc>) -> Option<f64> {
    let price = probe_lookup(model).or_else(|| {
        claude_price(model).map(|(i, o, cr, cw)| pricing::Price::flat(i, o, cr, cw))
    })?;
    let u = pricing::Usage {
        input: t.input,
        output: t.output,
        cache_read: t.cache_read,
        cache_write_5m: t.w5m,
        cache_write_1h: t.w1h,
    };
    let mult = if t.fast { probe_fast_multiplier(model) } else { 1.0 };
    Some(cost_for(model, &price, &u, 200_000.0, ts) * mult)
}

/// Per-file dedup state for the Claude scanner. Persisted so a tail
/// can drop a replay whose original sits older than the 1 MB warmup.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct ClaudeFileState {
    /// (message id, request id) pairs already counted.
    seen: HashSet<String>,
    /// message id → whether its first occurrence was a sidechain line.
    seen_mids: HashMap<String, bool>,
    /// The first working directory the session logged: its project root.
    #[serde(default)]
    root: Option<String>,
    /// The work area in force. Sticky: it changes when the session moves.
    #[serde(default)]
    area: Option<String>,
    /// First and last counted message, epoch milliseconds: the session's span.
    #[serde(default)]
    first_ms: Option<i64>,
    #[serde(default)]
    last_ms: Option<i64>,
    /// This file's sidechain parent, recomputed from the path by
    /// `claude_file` before every parse — never loaded from a checkpoint,
    /// so a subagent transcript's attribution survives even when
    /// `clip_claude_ckpt` wipes the rest of this state for being oversized.
    #[serde(skip)]
    parent_session: Option<String>,
}

/// Parse one Claude Code session-log line into spend events. Persisted
/// `claude -p` runs write the same assistant records (entrypoint "sdk-cli"),
/// so they count like interactive usage; `--no-session-persistence` runs
/// write no log at all.
/// How many folder levels an area keeps. Two, so a rule can tell
/// `site/client-a` from `site/client-b`; views roll up to one by default.
const AREA_DEPTH: usize = 2;

/// The folders of `path` beneath `root`, up to `AREA_DEPTH` levels, joined
/// with '/': the work area it belongs to. `is_dir` says the path is itself a
/// folder (a working directory); otherwise its last component is a file name
/// and is left out, so a lone file sitting in the root names no area.
fn area_under(path: &str, root: &str, is_dir: bool) -> Option<String> {
    let path = path.replace('\\', "/");
    let root = root.replace('\\', "/");
    let root = root.trim_end_matches('/');
    let rel = path.strip_prefix(root)?.strip_prefix('/')?.trim_end_matches('/');
    let mut folders: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
    if !is_dir {
        folders.pop();
    }
    folders.truncate(AREA_DEPTH);
    (!folders.is_empty()).then(|| folders.join("/"))
}

/// The top-level folder of an area: "site/client-a" → "site".
pub fn area_top(area: &str) -> &str {
    area.split('/').next().unwrap_or(area)
}

/// Folders named like scratch or tooling space (`_screenshots`, `.cache`).
/// A tool call that merely reads from one does not move the work there;
/// only a working directory inside it does.
fn is_scratch_area(area: &str) -> bool {
    let top = area_top(area);
    top.starts_with('_') || top.starts_with('.')
}

/// The first path in a shell command that falls under `root`.
fn area_in_command(command: &str, root: &str, home: Option<&str>) -> Option<String> {
    static PATHS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let paths = PATHS.get_or_init(|| regex::Regex::new(r"(?:~|/)[A-Za-z0-9_.~/\-]+").expect("path regex"));
    paths.find_iter(command).take(64).find_map(|m| {
        let raw = m.as_str();
        let expanded = match (raw.strip_prefix('~'), home) {
            (Some(rest), Some(home)) => format!("{home}{rest}"),
            _ => raw.to_string(),
        };
        // A command names folders as readily as files (`cd …/acme`); a last
        // component with an extension is taken to be a file.
        let leaf = expanded.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        let is_file = leaf.rfind('.').is_some_and(|dot| dot > 0 && dot < leaf.len() - 1);
        area_under(&expanded, root, !is_file).filter(|a| !is_scratch_area(a))
    })
}

/// Tracks which part of the project a session is working in. Claude Code is
/// often started once at the top of a workspace and then roams, so the
/// project folder alone says little. Two signals, in order: the line's
/// working directory relative to where the session began, then the first
/// path under that root in the message's tool calls (`file_path`-style
/// inputs, or paths inside a Bash command). With neither, the last area
/// stands: work tends to stay where it was.
fn claude_area(st: &mut ClaudeFileState, v: &Value, data: &FileData) {
    let Some(cwd) = v.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) else { return };
    let root = st.root.get_or_insert_with(|| cwd.to_string()).clone();
    let mut found = area_under(cwd, &root, true);
    if found.is_none() {
        let home = dirs::home_dir();
        let home = home.as_deref().and_then(Path::to_str);
        found = tool_use_area(v, &root, home);
    }
    let Some(found) = found else { return };
    // Names come from the log: bound their length and their number.
    let distinct: HashSet<&String> = data.areas.keys().map(|(_, a)| a).collect();
    let over = found.len() > MAX_AREA_KEY
        || (!distinct.contains(&found) && distinct.len() >= MAX_AREAS_PER_FILE);
    st.area = Some(if over { OTHER_AREA.to_string() } else { found });
}

/// Books a priced event to the area in force. Only models billed to the
/// Claude card: rows that `split_models` later moves to another card must
/// not stay behind in this card's areas.
fn add_area(st: &ClaudeFileState, data: &mut FileData, ts: DateTime<Utc>, model: &str, cost: f64, tokens: f64) {
    if !model.starts_with("claude") && model != "unattributed" {
        return;
    }
    let area = st.area.clone().unwrap_or_else(|| UNSORTED_AREA.to_string());
    let entry = data.areas.entry((day_of_utc(ts), area)).or_insert((0.0, 0.0));
    entry.0 += cost;
    entry.1 += tokens;
}

/// Longest `attributionAgent` name admitted onto a `FileData`. Same bound
/// as catalog canonicals (MAX_PROBE_KEY) — every built-in or custom agent
/// name fits; a hostile or corrupt line just never sets the field.
const MAX_AGENT_KEY: usize = MAX_PROBE_KEY;

/// How many times a sidechain line's own `sessionId` has disagreed with the
/// directory it was found under. `claude_line` trusts the directory either
/// way (a rewritten or relocated log could disagree), so this is purely a
/// debug signal — counted rather than logged, since a busy subagent fan-out
/// would otherwise spam the log once per line.
static SIDECHAIN_SESSION_MISMATCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn note_sidechain_session_mismatch() {
    SIDECHAIN_SESSION_MISMATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// `Some(<session-uuid>)` when `path` sits anywhere underneath a directory
/// literally named `subagents` — a subagent transcript, never a session of
/// its own. Claude Code writes most of these one level down
/// (`<uuid>/subagents/<file>.jsonl`), but a workflow run nests its own
/// transcripts a folder deeper still
/// (`<uuid>/subagents/workflows/<workflow-id>/<file>.jsonl`), so this walks
/// up `path`'s ancestors looking for the nearest one named `subagents` and
/// returns *that* component's own parent's name — the enclosing session,
/// whatever the transcript's own nesting depth. Pure path check: no
/// filesystem access, and no validation that the returned name actually
/// looks like a uuid.
fn sidechain_parent(path: &Path) -> Option<String> {
    for ancestor in path.ancestors() {
        if ancestor.file_name().and_then(|n| n.to_str()) == Some("subagents") {
            let session_dir = ancestor.parent()?;
            return session_dir.file_name()?.to_str().map(str::to_string);
        }
    }
    None
}

fn claude_line(st: &mut ClaudeFileState, line: &str, data: &mut FileData) {
    if !line.contains("\"type\":\"assistant\"") {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    // A subagent transcript's own directory names its parent session (see
    // `sidechain_parent`, set by `claude_file` before the parse starts).
    // `merge_data` never combines this across files, so it survives exactly
    // as one file's own identity.
    data.parent_session = st.parent_session.clone();
    if let Some(parent) = &st.parent_session {
        if let Some(sid) = v.get("sessionId").and_then(Value::as_str) {
            if sid != parent {
                note_sidechain_session_mismatch();
            }
        }
        // The name is constant for the whole file in practice, so the first
        // assistant line to carry one settles it; a transcript that never
        // carries one stays `None` here and becomes an empty, unattributed
        // display name only in `agent_spend`, never guessed this early.
        if data.agent.is_none() {
            if let Some(name) = v.get("attributionAgent").and_then(Value::as_str) {
                if !name.is_empty() && name.len() <= MAX_AGENT_KEY {
                    data.agent = Some(name.to_string());
                }
            }
        }
    }
    // Before the duplicate check on purpose: Claude Code logs one content
    // block per line, so a message's tool call arrives on a later line than
    // the one that carried its cost, and that later line is a "duplicate".
    claude_area(st, &v, data);
    let Some(ts) = parse_ts(v.get("timestamp")) else { return };
    let ms = ts.timestamp_millis();
    st.first_ms = Some(st.first_ms.map_or(ms, |f| f.min(ms)));
    st.last_ms = Some(st.last_ms.map_or(ms, |l| l.max(ms)));
    let usage = v.pointer("/message/usage").cloned().unwrap_or(Value::Null);
    let Some(t) = claude_tokens(&usage) else { return };

    // Resumed sessions repeat messages under the same request id, and
    // sidechain logs replay the parent's message under a *fresh* request id
    // — dedupe on both. Keep-first: the parent line precedes its sidechain
    // replay in the log. (The Mac also re-prefers a parent that arrives
    // after its sidechain copy; a streaming pass can't retract an event, so
    // that rarer order keeps the sidechain copy — still counted once.)
    let sidechain = v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false);
    if let Some(mid) = v.pointer("/message/id").and_then(Value::as_str) {
        let rid = v.get("requestId").and_then(Value::as_str).unwrap_or("");
        if !st.seen.insert(format!("{mid}:{rid}")) {
            return;
        }
        if let Some(&first_was_sidechain) = st.seen_mids.get(mid) {
            if sidechain || first_was_sidechain {
                return;
            }
            // Same message id under distinct request ids with no sidechain
            // involved is a genuine retry — both count (Mac parity).
        }
        st.seen_mids.entry(mid.to_string()).or_insert(sidechain);
    }

    // `<synthetic>` is Claude Code's placeholder for tool-generated turns:
    // there is no real model to price or warn about, so only a carried
    // costUSD makes the line count (as unattributed usage).
    let model_raw = v.pointer("/message/model").and_then(Value::as_str);
    let synthetic = model_raw == Some("<synthetic>");
    let model = model_raw.unwrap_or("unknown").to_string();

    // Cost preference: the log's own costUSD → live catalog price
    // → static family fallback → excluded (never a guessed $0).
    match v.get("costUSD").and_then(Value::as_f64) {
        Some(c) => {
            let name = if synthetic { "unattributed" } else { model.as_str() };
            if t.total() > 0.0 || c > 0.0 {
                add_event(data, ts, name, c, t.total());
                add_area(st, data, ts, name, c, t.total());
            }
        }
        None if synthetic => {}
        None => match claude_cost(&model, &t, ts) {
            Some(c) => {
                if t.total() > 0.0 || c > 0.0 {
                    add_event(data, ts, &model, c, t.total());
                    add_area(st, data, ts, &model, c, t.total());
                }
            }
            None => {
                if t.total() > 0.0 {
                    note_unpriced(data, ts, &model, t.total());
                }
            }
        },
    }

    // Fable-era logs nest advisor work in `usage.iterations`. Only
    // advisor-message iterations become extra entries, under the advisor's
    // own model — ordinary message iterations are already inside the
    // parent's usage totals, and counting them again would double-count.
    let Some(iters) = usage.get("iterations").and_then(Value::as_array) else { return };
    for it in iters {
        if it.get("type").and_then(Value::as_str) != Some("advisor_message") {
            continue;
        }
        let Some(advisor) = it
            .get("model")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty() && *m != "<synthetic>")
        else {
            continue;
        };
        let Some(at) = claude_tokens(it) else { continue };
        if at.total() <= 0.0 {
            continue;
        }
        match claude_cost(advisor, &at, ts) {
            Some(c) => {
                add_event(data, ts, advisor, c, at.total());
                add_area(st, data, ts, advisor, c, at.total());
            }
            None => note_unpriced(data, ts, advisor, at.total()),
        }
    }
}

/// Move every event whose model starts with `prefix` out of `data` into a
/// new FileData (unpriced tallies included). Used to re-route usage that a
/// CLI logged on another vendor's behalf.
/// Prefix match is case-insensitive: gateways spell the same family both
/// ways ("qwen3.8-max", "Qwen/Qwen3-235B") and a case miss would leave
/// rows on the wrong card.
fn split_models(data: &mut FileData, prefix: &str) -> FileData {
    let prefix = prefix.to_ascii_lowercase();
    let matches = |m: &str| m.to_ascii_lowercase().starts_with(&prefix);
    let mut out = FileData::default();
    data.days.retain(|(day, model), v| {
        if matches(model) {
            out.days.insert((*day, model.clone()), *v);
            false
        } else {
            true
        }
    });
    let moved: Vec<String> =
        data.unpriced.keys().filter(|m| matches(m)).cloned().collect();
    for m in moved {
        if let Some(c) = data.unpriced.remove(&m) {
            out.unpriced.insert(m, c);
        }
    }
    out
}

/// Peel the vendor prefixes Kimi usage arrives under, so rows routed from
/// other CLIs merge with the Kimi CLI's own spellings ("kimi-oauth/k3" and
/// "k3" are the same model on the same bill).
fn strip_kimi_prefix(model: &str) -> String {
    let lower = model.to_ascii_lowercase();
    ["moonshot-ai/", "moonshotai/", "kimi-code/", "kimi-oauth/", "moonshot/"]
        .iter()
        .find(|p| lower.starts_with(**p))
        .map(|p| model[p.len()..].to_string())
        .unwrap_or_else(|| model.to_string())
}

/// Kimi/Moonshot models logged by another CLI — Codex driven through a
/// router against the Kimi OAuth plan ("kimi-oauth/k3"), or a session
/// pointed at Moonshot's API ("moonshot-ai/kimi-k3"). Moonshot bills those
/// turns, not the CLI's own subscription, so the rows move to the
/// Kimi/Moonshot card with their vendor prefixes peeled.
fn split_kimi_routed(all: &mut FileData) -> FileData {
    let mut moved = FileData::default();
    for prefix in ["kimi", "moonshot"] {
        merge_data(&mut moved, split_models(all, prefix));
    }
    let mut out = FileData::default();
    for ((day, model), (cost, tokens)) in moved.days {
        let entry = out.days.entry((day, strip_kimi_prefix(&model))).or_insert((0.0, 0.0));
        entry.0 += cost;
        entry.1 += tokens;
    }
    for (model, count) in moved.unpriced {
        *out.unpriced.entry(strip_kimi_prefix(&model)).or_insert(0) += count;
    }
    out
}

/// Claude Code writes one JSONL per session under ~/.claude/projects. Each
/// assistant line carries usage token counts and usually a precomputed
/// costUSD, which we prefer over our own pricing table.
///
/// Claude Code can also run against MiniMax's Anthropic-compatible endpoint
/// (ANTHROPIC_BASE_URL); those sessions log MiniMax models into the same
/// files. That usage is split out and returned separately — it belongs on
/// the MiniMax card, not Claude's.
/// Claude Code names a project's log folder after its path with every
/// non-alphanumeric character replaced by '-'. That is lossy, so a folder
/// name cannot be decoded, only matched against paths that are still known.
fn encode_project_path(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn resolve_project(dir_name: &str, known_paths: &[String]) -> String {
    known_paths
        .iter()
        .find(|p| encode_project_path(p) == dir_name)
        .cloned()
        .unwrap_or_else(|| dir_name.to_string())
}

/// The project folder a log file belongs to: the first component under the
/// `projects` root. None for a file sitting directly in the root.
fn project_of(root: &Path, file: &Path) -> Option<String> {
    let mut parts = file.strip_prefix(root).ok()?.components();
    let first = parts.next()?;
    parts.next()?; // the file itself, or deeper: either way `first` is a folder
    Some(first.as_os_str().to_string_lossy().into_owned())
}

/// Per-project windows, cut exactly like a card's. Projects with no spend in
/// the last 30 days are left out; the rest sort by 30-day cost.
/// One Claude Code session (one log file): when it ran, what it cost, where
/// it worked. Metadata only. The conversation titles Claude Code keeps in
/// the same logs are derived from prompts and are deliberately not read.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionSpend {
    /// The log file's stem: Claude Code's session id.
    pub id: String,
    pub project: String,
    pub started_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    /// Cost and tokens inside the 30-day window.
    pub cost: f64,
    pub tokens: f64,
    /// Size of the log file on disk. A session that is weeks old and still
    /// being appended to is usually also the largest; the size says so.
    pub bytes: u64,
    pub top_model: Option<String>,
    /// (area, cost) inside the window, largest first.
    pub areas: Vec<(String, f64)>,
    /// When a single day was asked for: this session's cost on that day.
    pub day_cost: Option<f64>,
    /// This session's own subagent runs' share of `cost` (already included
    /// in it: the session really spent that) — broken out only for display.
    pub subagent_cost: f64,
    /// How many subagent transcript files fed `subagent_cost`, inside the
    /// same window `cost` is cut to.
    pub subagent_runs: usize,
}

fn session_from(id: &str, project: &str, data: &FileData, st: Option<&ClaudeFileState>, today: i32, bytes: u64) -> Option<SessionSpend> {
    let in_window = |day: i32| day > today - TREND_DAYS as i32 && day <= today;
    let mut models: HashMap<&str, f64> = HashMap::new();
    let (mut cost, mut tokens) = (0.0, 0.0);
    for ((day, model), (c, t)) in &data.days {
        if in_window(*day) {
            cost += c;
            tokens += t;
            *models.entry(model.as_str()).or_insert(0.0) += c;
        }
    }
    if cost <= 0.004 && tokens <= 0.0 {
        return None;
    }
    let mut areas: HashMap<&str, f64> = HashMap::new();
    for ((day, area), (c, _)) in &data.areas {
        if in_window(*day) {
            *areas.entry(area.as_str()).or_insert(0.0) += c;
        }
    }
    let mut areas: Vec<(String, f64)> = areas.into_iter().map(|(a, c)| (a.to_string(), c)).collect();
    areas.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Some(SessionSpend {
        id: id.to_string(),
        project: project.to_string(),
        started_ms: st.and_then(|s| s.first_ms),
        ended_ms: st.and_then(|s| s.last_ms),
        cost,
        tokens,
        bytes,
        top_model: models
            .into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(a.0)))
            .map(|(m, _)| m.to_string()),
        areas,
        day_cost: None,
        subagent_cost: 0.0,
        subagent_runs: 0,
    })
}

/// Sessions from the scan cache. `area` keeps those that spent anything in
/// that area (a top-level folder matches what is under it); `day` (a local
/// `YYYY-MM-DD`) keeps those that spent on that day and ranks by that day's
/// cost, which answers "what was that expensive day?". Otherwise ranked by
/// 30-day cost. Reads what the last `collect` cached; it does not rescan.
/// Where Claude Code keeps its session logs: `<config dir>/projects`.
fn claude_projects_root() -> PathBuf {
    std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".claude"))
        .join("projects")
}

/// The log file behind a session id, if the scan cache knows it. Only a
/// `.jsonl` one folder under the projects root can match, so an id can name
/// a Claude Code session and nothing else the cache has seen (codex logs,
/// grok's unified file). The id is a file stem, so it cannot carry a path.
fn session_path_among<'a>(root: &Path, paths: impl Iterator<Item = &'a PathBuf>, id: &str) -> Option<PathBuf> {
    paths
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .filter(|p| project_of(root, p).is_some())
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(id))
        .cloned()
}

/// `session_path_among` over the real cache. Reads only; the caller shows
/// the file, it never touches it.
pub fn session_path(id: &str) -> Option<PathBuf> {
    load_persisted_cache();
    let root = claude_projects_root();
    let map = cache().lock().ok()?;
    session_path_among(&root, map.keys(), id)
}

pub fn claude_sessions(area: Option<&str>, day: Option<&str>, limit: usize) -> Vec<SessionSpend> {
    let day = day
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .map(|d| d.num_days_from_ce());
    let root = claude_projects_root();
    load_persisted_cache();
    let today = today_days_from_ce();
    let in_window = |d: i32| d > today - TREND_DAYS as i32 && d <= today;
    let known = crate::inventory::known_project_paths();
    let Ok(map) = cache().lock() else { return Vec::new() };

    // A subagent transcript is a sidechain of the session that spawned it,
    // never a session of its own (see `sidechain_parent`): fold each one's
    // in-window cost and tokens into its parent id before the list below is
    // built. A parent id nothing here matches (its own file sits outside
    // the window, or is gone) simply never has this map consulted — its
    // sidechains are not listed under anything, though the project/day
    // totals upstream still count them.
    let mut subagent: HashMap<&str, (f64, f64, usize)> = HashMap::new();
    for entry in map.values() {
        let Some(parent) = entry.data.parent_session.as_deref() else { continue };
        let (mut cost, mut tokens) = (0.0, 0.0);
        for ((d, _), (c, t)) in &entry.data.days {
            if in_window(*d) {
                cost += c;
                tokens += t;
            }
        }
        if cost <= 0.004 && tokens <= 0.0 {
            continue;
        }
        let slot = subagent.entry(parent).or_insert((0.0, 0.0, 0));
        slot.0 += cost;
        slot.1 += tokens;
        slot.2 += 1;
    }

    let mut out: Vec<SessionSpend> = map
        .iter()
        .filter(|(_, entry)| entry.data.parent_session.is_none())
        .filter_map(|(path, entry)| {
            let project = project_of(&root, path)?;
            let id = path.file_stem()?.to_str()?;
            let mut session =
                session_from(id, &resolve_project(&project, &known), &entry.data, entry.claude.as_ref(), today, entry.size)?;
            // The session really spent this: cost/tokens include the
            // subagent share, which is broken out separately for display.
            if let Some(&(sub_cost, sub_tokens, sub_runs)) = subagent.get(id) {
                session.cost += sub_cost;
                session.tokens += sub_tokens;
                session.subagent_cost = sub_cost;
                session.subagent_runs = sub_runs;
            }
            if let Some(day) = day {
                let mut on_day: f64 =
                    entry.data.days.iter().filter(|((d, _), _)| *d == day).map(|(_, (c, _))| c).sum();
                on_day += map
                    .values()
                    .filter(|e| e.data.parent_session.as_deref() == Some(id))
                    .flat_map(|e| e.data.days.iter())
                    .filter(|((d, _), _)| *d == day)
                    .map(|(_, (c, _))| c)
                    .sum::<f64>();
                if on_day <= 0.004 {
                    return None;
                }
                session.day_cost = Some(on_day);
            }
            Some(session)
        })
        .filter(|s| match area {
            Some(want) => s.areas.iter().any(|(a, _)| a == want || area_top(a) == want),
            None => true,
        })
        .collect();
    out.sort_by(|a, b| {
        let key = |s: &SessionSpend| s.day_cost.unwrap_or(s.cost);
        key(b).total_cmp(&key(a)).then_with(|| a.id.cmp(&b.id))
    });
    out.truncate(limit);
    out
}

/// Subagent transcripts grouped by who ran them: a built-in agent
/// (`general-purpose`, `Explore`, `Plan`, …), a custom definition's own
/// name, or an empty `name` for a transcript whose lines never carried one.
/// Empty rather than a placeholder word like `"unknown"` because a `.md`
/// definition's own stem can never be empty, so the unattributed bucket can
/// never collide with a real agent's name. Metadata only, same contract as
/// `SessionSpend`: no field here can hold a prompt or a title, because none
/// of its inputs (`FileData::agent`, day/model totals, a checkpoint's
/// timestamps) can either.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpend {
    pub name: String,
    /// How many subagent transcript files this agent name covers.
    pub runs: usize,
    pub cost: f64,
    pub tokens: u64,
    /// The newest assistant line across this agent's transcripts, epoch
    /// milliseconds, falling back to a file's own mtime for an older
    /// checkpoint that predates the span fields.
    pub last_used_ms: i64,
    pub top_model: Option<String>,
}

/// The `PersistEntry` shape `agent_spend_from` groups over, built straight
/// from a scan result rather than read back off the persisted cache. Only
/// the fields the grouping actually reads are filled in — everything else
/// (`path`, `probes`, `prefix_head`/`tail`, …) stays its `Default`, because
/// grouping never looks at them. `agent_spend`'s own wrapper below, and any
/// test that wants the real parser's output without touching `cache()`'s
/// pre-existing contents, both build their input through this.
fn to_agent_entry(data: &FileData, claude: Option<ClaudeFileState>, mtime: SystemTime) -> PersistEntry {
    let since_epoch = mtime.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    PersistEntry {
        mtime_secs: since_epoch.as_secs(),
        mtime_nanos: since_epoch.subsec_nanos(),
        days: data
            .days
            .iter()
            .map(|((day, model), (cost, tokens))| (*day, model.clone(), *cost, *tokens))
            .collect(),
        claude,
        parent_session: data.parent_session.clone(),
        agent: data.agent.clone(),
        ..Default::default()
    }
}

/// `agent_spend`'s own grouping, pure: no cache lock, no filesystem, no
/// clock read. `today` is the caller's own `today_days_from_ce()`, so a
/// fixture test can pick any day it likes and still exercise the real
/// window arithmetic. An entry with no `parent_session` is not a subagent
/// transcript at all and is skipped, same as `agent_spend` always did.
fn agent_spend_from<'a>(entries: impl Iterator<Item = &'a PersistEntry>, days: u32, today: i32) -> Vec<AgentSpend> {
    let cutoff = today - days as i32;
    let in_window = |d: i32| d > cutoff && d <= today;

    struct Group {
        runs: usize,
        cost: f64,
        tokens: f64,
        last_used_ms: i64,
        by_model: HashMap<String, f64>,
    }
    let mut groups: HashMap<String, Group> = HashMap::new();
    for entry in entries {
        if entry.parent_session.is_none() {
            continue; // not a subagent transcript at all
        }
        let (mut cost, mut tokens) = (0.0, 0.0);
        let mut by_model: HashMap<String, f64> = HashMap::new();
        for (d, model, c, t) in &entry.days {
            if !in_window(*d) {
                continue;
            }
            cost += c;
            tokens += t;
            *by_model.entry(model.clone()).or_insert(0.0) += c;
        }
        if cost <= 0.0 && tokens <= 0.0 {
            continue; // no activity in the requested window
        }
        let last_used_ms = entry
            .claude
            .as_ref()
            .and_then(|c| c.last_ms)
            .unwrap_or_else(|| entry.mtime_secs as i64 * 1000 + entry.mtime_nanos as i64 / 1_000_000);
        // Empty, never "unknown": a `.md` definition's own stem is never
        // empty, so this can't collide with a real agent that happens to be
        // named it.
        let name = entry.agent.clone().unwrap_or_default();
        let g = groups.entry(name).or_insert_with(|| Group {
            runs: 0,
            cost: 0.0,
            tokens: 0.0,
            last_used_ms: 0,
            by_model: HashMap::new(),
        });
        g.runs += 1;
        g.cost += cost;
        g.tokens += tokens;
        g.last_used_ms = g.last_used_ms.max(last_used_ms);
        for (model, c) in by_model {
            *g.by_model.entry(model).or_insert(0.0) += c;
        }
    }

    let mut out: Vec<AgentSpend> = groups
        .into_iter()
        .map(|(name, g)| {
            let top_model = g
                .by_model
                .into_iter()
                .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                .map(|(m, _)| m);
            AgentSpend {
                name,
                runs: g.runs,
                cost: g.cost,
                tokens: g.tokens.round() as u64,
                last_used_ms: g.last_used_ms,
                top_model,
            }
        })
        .collect();
    // The empty, unattributed name (if any) sorts last regardless of its
    // cost -- named agents are always the more actionable rows, and an
    // empty name would otherwise sort first on a cost tie or even ahead of
    // a named row that spent less.
    out.sort_by(|a, b| a.name.is_empty().cmp(&b.name.is_empty()).then_with(|| b.cost.total_cmp(&a.cost)).then_with(|| a.name.cmp(&b.name)));
    out
}

/// `agent_spend` groups this many days of subagent activity, read from the
/// same persisted scan cache `claude_sessions` reads — no second scan.
/// Sorted by cost, highest first. A thin wrapper: locks the cache, converts
/// each subagent-transcript entry to the shape `agent_spend_from` groups
/// over, and hands it off.
pub fn agent_spend(days: u32) -> Vec<AgentSpend> {
    load_persisted_cache();
    let today = today_days_from_ce();
    // Scoped so the cache lock is held only long enough to copy entries out
    // of it -- `agent_spend_from` below does its own (unrelated) work and
    // has no business running while the cache stays locked.
    let entries: Vec<PersistEntry> = {
        let Ok(map) = cache().lock() else { return Vec::new() };
        map.values()
            .filter(|e| e.data.parent_session.is_some())
            .map(|e| to_agent_entry(&e.data, e.claude.clone(), e.mtime))
            .collect()
    };
    agent_spend_from(entries.iter(), days, today)
}

// ---------------------------------------------------------------------------
// Live pace: what a running Claude Code session is doing right now
// ---------------------------------------------------------------------------

/// Width of the "right now" window: tokens, cost, model and area all come
/// from assistant lines inside the last 10 minutes only, so a session that
/// has gone quiet reports nothing instead of a stale number.
const LIVE_WINDOW_MS: i64 = 10 * 60 * 1000;
/// A session file counts as live only when it was written inside the last
/// 5 minutes -- half the pace window, so "live" always means the file is
/// still being appended to, never a session that has plainly ended.
const LIVE_FRESH_MS: i64 = 5 * 60 * 1000;
/// How much of a live session's tail gets read. There is no persisted
/// cursor here (unlike the day scanner's cache): every poll re-opens the
/// file fresh, so this stays small -- comfortably a few minutes of a busy
/// session without ever re-reading a multi-gigabyte log.
const LIVE_TAIL_BYTES: usize = 256 * 1024;

/// A running agent's own session, read fresh: recent pace, not history.
/// Structurally incapable of carrying a prompt, a title or any other text
/// block -- only counts, a price, a model name and a work area derived
/// from tool-call *paths*, the same way the history scanner's areas are.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LivePace {
    /// File stem of the newest live session file: Claude Code's session id.
    pub session_id: String,
    /// Input + output + cache tokens of deduplicated assistant lines inside
    /// the last 10 minutes.
    pub tokens_10m: u64,
    /// Those same lines, priced like the scanner: the live catalog first,
    /// then the static family fallback. A line whose model prices as
    /// neither adds 0 here and turns `priced` off -- never a guessed
    /// dollar figure. Always priced from the catalog, never a carried
    /// `costUSD`: when the log later has one for the same line, the history
    /// scanner's own figure can end up differing from what this reported live.
    pub cost_10m: f64,
    pub priced: bool,
    /// Seconds since the newest assistant line's own timestamp.
    pub idle_secs: u64,
    /// The newest assistant line's model.
    pub model: Option<String>,
    /// The most recent work area the window's tool calls touched: the same
    /// `area_under` / `area_in_command` cascade `claude_area` runs for its
    /// own tool-call signal, never the process's bare cwd, which (being the
    /// session's own root) names no area relative to itself.
    pub area: Option<String>,
    /// The `AGENT_HOSTS` display name this pace was read for ("Claude
    /// Code" / "Codex" / "Gemini CLI"). Set by whichever arm of
    /// `live_session_for_cwd` produced it, never guessed from the session
    /// file's own shape.
    pub tool: String,
}

/// Reads whichever running agent's own session format matches `tool`, at
/// the folder `cwd` names. Each arm resolves its tool's home directory
/// itself (`CODEX_HOME` / the Gemini tmp root / Claude's projects root) so
/// the lookup stays a single call for every caller; a tool with no live
/// reader here (most of `AGENT_HOSTS`) is `None`, never a guess at a format
/// nobody has taught this function yet.
pub fn live_session_for_cwd(tool: &str, cwd: &str, now_ms: i64) -> Option<LivePace> {
    match tool {
        "Claude Code" => {
            let dir = claude_projects_root().join(encode_project_path(cwd));
            live_session_in(&dir, now_ms)
        }
        "Codex" => codex_live_session_in(&codex_home(), cwd, now_ms),
        "Gemini CLI" => gemini_live_session_in(&gemini_tmp_root(), cwd, now_ms),
        _ => None,
    }
}

/// How many directory levels `jsonl_files_under` will descend beneath a
/// session's `subagents/` folder. Claude Code itself only nests one level
/// further, for a workflow run (`subagents/workflows/<workflow-id>/`), so 6
/// is generous headroom against a future nesting change, never a depth
/// anything real is expected to reach -- it exists so a pathological or
/// cyclic tree can't turn a live-pace poll into an unbounded walk.
const SUBAGENT_WALK_DEPTH: u32 = 6;

/// The newest write across a project's top-level `<uuid>.jsonl` session
/// files and, anywhere beneath each session's own `<uuid>/subagents/`
/// directory however deeply nested (a Task-tool fan-out writes one level
/// down; a workflow run nests its own transcripts a folder deeper still,
/// under `subagents/workflows/<workflow-id>/`) -- so a fan-out or a
/// workflow run keeps its parent uuid live even while the parent's own
/// file sits idle. The walk under `subagents/` is depth-bounded (see
/// `SUBAGENT_WALK_DEPTH`), and a transcript's write only ever promotes the
/// *enclosing* `<uuid>`: it is never itself picked as the live session.
fn live_session_in(dir: &Path, now_ms: i64) -> Option<LivePace> {
    let mut newest: Option<(String, SystemTime)> = None;
    // Each session directory's own subagent files, path plus the mtime this
    // same discovery pass already paid to stat. Keeping the list here means
    // the eventual winner's `subagents/` subtree only has to be walked and
    // stat'd once, below, instead of a second time when the tail is built.
    let mut sub_files_by_uuid: HashMap<String, Vec<(PathBuf, SystemTime)>> = HashMap::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let Ok(ftype) = entry.file_type() else { continue };
        if ftype.is_file() {
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(uuid) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else { continue };
            let better = match &newest {
                Some((_, best)) => mtime > *best,
                None => true,
            };
            if better {
                newest = Some((uuid.to_string(), mtime));
            }
        } else if ftype.is_dir() {
            let Some(uuid) = path.file_name().and_then(|s| s.to_str()) else { continue };
            let mut sub_paths = Vec::new();
            jsonl_files_under(&path.join("subagents"), SUBAGENT_WALK_DEPTH, &mut sub_paths);
            let mut sub_files = Vec::with_capacity(sub_paths.len());
            for sub_path in sub_paths {
                let Ok(mtime) = fs::metadata(&sub_path).and_then(|m| m.modified()) else { continue };
                let better = match &newest {
                    Some((_, best)) => mtime > *best,
                    None => true,
                };
                if better {
                    newest = Some((uuid.to_string(), mtime));
                }
                sub_files.push((sub_path, mtime));
            }
            sub_files_by_uuid.insert(uuid.to_string(), sub_files);
        }
    }
    let (uuid, mtime) = newest?;
    let mtime_ms = mtime.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as i64;
    if now_ms.saturating_sub(mtime_ms) > LIVE_FRESH_MS {
        return None;
    }

    // The parent's own tail, plus every one of its subagent transcripts
    // that is itself still inside the freshness window -- a Task run that
    // finished an hour ago should not be re-read on every poll. All of it
    // is attributed to the parent uuid; `pace_from_lines` sums and dedupes
    // across the combined lines exactly as it would within one file.
    let mut lines = tail_lines(&dir.join(format!("{uuid}.jsonl")));
    // Reuse the winner's own subagent list from the discovery loop above --
    // already walked and stat'd once there. A winner with no `subagents/`
    // directory of its own just never got an entry, same as an empty walk.
    for (sub_path, sub_mtime) in sub_files_by_uuid.remove(&uuid).unwrap_or_default() {
        let sub_ms = sub_mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if now_ms.saturating_sub(sub_ms) > LIVE_FRESH_MS {
            continue;
        }
        lines.extend(tail_lines(&sub_path));
    }
    pace_from_lines(lines.iter().map(String::as_str), &uuid, now_ms)
}

/// Every `.jsonl` file anywhere under `dir`, however deeply nested.
/// Depth-bounded so a runaway or cyclic tree can't turn a live-pace poll
/// into an unbounded walk; only directories are recursed into, and
/// anything unreadable (including a missing `dir` itself) is silently
/// skipped, same as every other best-effort read in this module.
///
/// Symlinks are followed, same as `recent_jsonl_files`: a `subagents/`
/// entry relocated behind a link must still be found. A link cycle can't
/// turn this into an infinite walk either way -- termination comes from
/// the depth bound alone, not from refusing to follow links.
fn jsonl_files_under(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ftype) = entry.file_type() else { continue };
        if ftype.is_file() {
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
            }
        } else if ftype.is_dir() {
            jsonl_files_under(&path, depth - 1, out);
        } else if ftype.is_symlink() {
            // `file_type()` reports the link itself, never its target, so
            // the target's own type has to come from a follow-through stat.
            let Ok(meta) = fs::metadata(&path) else { continue };
            if meta.is_dir() {
                jsonl_files_under(&path, depth - 1, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
            }
        }
    }
}

/// The last `LIVE_TAIL_BYTES` of `path` (or the whole file when it is
/// smaller), split into lines. A seek into the middle of a file almost
/// never lands on a line boundary, so whenever the read did not start at
/// byte 0 the first entry is dropped unconditionally -- simpler than the
/// day scanner's `align_to_line_start`, and fine for a live ticker that
/// re-reads from scratch on every poll rather than resuming a cursor.
fn tail_lines(path: &Path) -> Vec<String> {
    let Ok(mut file) = fs::File::open(path) else { return Vec::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(LIVE_TAIL_BYTES as u64);
    if start > 0 && file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    // Lossy: a seek can split a multi-byte character right at the start of
    // the buffer, inside the partial line that is about to be dropped
    // anyway -- a replacement character there costs nothing real.
    let mut lines: Vec<String> = String::from_utf8_lossy(&buf).split('\n').map(str::to_string).collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    lines
}

/// The work area one assistant line's `tool_use` blocks name, with `root`
/// as the folder they are measured under: `file_path` / `path` /
/// `notebook_path` first, then the first path under `root` in a Bash
/// `command`. The one implementation of that signal -- `claude_area` (the
/// history scanner) and `pace_from_lines` (the live ticker) both call this
/// rather than each walking `tool_use` blocks itself; `area_under`,
/// `is_scratch_area` and `area_in_command` do the actual path matching.
fn tool_use_area(v: &Value, root: &str, home: Option<&str>) -> Option<String> {
    let blocks = v.pointer("/message/content").and_then(Value::as_array)?;
    blocks.iter().find_map(|block| {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            return None;
        }
        let input = block.get("input")?;
        ["file_path", "path", "notebook_path"]
            .iter()
            .find_map(|k| input.get(*k).and_then(Value::as_str))
            .and_then(|p| area_under(p, root, false))
            .filter(|a| !is_scratch_area(a))
            .or_else(|| input.get("command").and_then(Value::as_str).and_then(|c| area_in_command(c, root, home)))
    })
}

/// One live session's tail, already read: the last 10 minutes of assistant
/// activity, deduplicated exactly like `claude_line`, priced exactly like
/// the scanner. Pure -- no I/O, no clock reads beyond the `now_ms` given --
/// so a fixture tail exercises the real logic end to end. `None` when
/// nothing in `lines` is an in-window assistant line.
fn pace_from_lines<'a>(lines: impl Iterator<Item = &'a str>, session_id: &str, now_ms: i64) -> Option<LivePace> {
    let cutoff = now_ms - LIVE_WINDOW_MS;
    let home = dirs::home_dir();
    let home = home.as_deref().and_then(Path::to_str);

    let mut seen: HashSet<String> = HashSet::new();
    let mut seen_mids: HashMap<String, bool> = HashMap::new();
    let mut tokens_10m = 0.0f64;
    let mut cost_10m = 0.0f64;
    let mut priced = true;
    let mut newest_ms: Option<i64> = None;
    let mut model: Option<String> = None;
    let mut area: Option<String> = None;

    for line in lines {
        // Same fast path as `claude_line`: rule out anything that plainly
        // is not an assistant line before paying for a JSON parse.
        if !line.contains("\"type\":\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(ts) = parse_ts(v.get("timestamp")) else { continue };
        let ms = ts.timestamp_millis();
        if ms < cutoff {
            continue;
        }

        // Area and the "newest line" markers are read from every
        // qualifying line, dedupe or not -- exactly like `claude_area` runs
        // ahead of `claude_line`'s duplicate check, because a message's
        // tool_use block routinely arrives on what dedupe would call a
        // repeat of an earlier line (Claude Code logs one content block per
        // JSONL line).
        if let Some(cwd) = v.get("cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
            if let Some(found) = tool_use_area(&v, cwd, home) {
                area = Some(found);
            }
        }
        if newest_ms.is_none_or(|n| ms >= n) {
            newest_ms = Some(ms);
            model = v.pointer("/message/model").and_then(Value::as_str).map(str::to_string);
        }

        // Dedupe on (message id, request id), like `claude_line`: a
        // resumed session can repeat the same line verbatim. Same guard as
        // `claude_line`'s own `seen_mids`: a sidechain log replays the
        // parent's message under a fresh request id, which slips past the
        // check above on its own, so a message id whose first sighting (this
        // line or an earlier one) was a sidechain counts once, not twice.
        let sidechain = v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false);
        if let Some(mid) = v.pointer("/message/id").and_then(Value::as_str) {
            let rid = v.get("requestId").and_then(Value::as_str).unwrap_or("");
            if !seen.insert(format!("{mid}:{rid}")) {
                continue;
            }
            if let Some(&first_was_sidechain) = seen_mids.get(mid) {
                if sidechain || first_was_sidechain {
                    continue;
                }
            }
            seen_mids.entry(mid.to_string()).or_insert(sidechain);
        }

        let usage = v.pointer("/message/usage").cloned().unwrap_or(Value::Null);
        let Some(t) = claude_tokens(&usage) else { continue };
        tokens_10m += t.total();

        let line_model = v.pointer("/message/model").and_then(Value::as_str).unwrap_or("unknown");
        match claude_cost(line_model, &t, ts) {
            Some(c) => cost_10m += c,
            None => priced = false,
        }
    }

    let newest_ms = newest_ms?;
    Some(LivePace {
        session_id: session_id.to_string(),
        tokens_10m: tokens_10m.round() as u64,
        cost_10m,
        priced,
        idle_secs: now_ms.saturating_sub(newest_ms).max(0) as u64 / 1000,
        model,
        area,
        tool: "Claude Code".to_string(),
    })
}

/// Today / yesterday / last-30-days totals for each key of a day map.
fn windows_by_key(days: DayMap, today: i32) -> HashMap<String, ([Window; 3], Vec<f64>)> {
    let mut out: HashMap<String, ([Window; 3], Vec<f64>)> = HashMap::new();
    for ((day, key), (cost, tokens)) in days {
        let (windows, daily) = out.entry(key).or_insert_with(|| (Default::default(), vec![0.0; TREND_DAYS]));
        let idx = day - (today - TREND_DAYS as i32 + 1);
        if (0..TREND_DAYS as i32).contains(&idx) {
            daily[idx as usize] += cost;
        }
        let bump = |w: &mut Window| {
            w.cost += cost;
            w.tokens += tokens;
        };
        if day == today {
            bump(&mut windows[0]);
        }
        if day == today - 1 {
            bump(&mut windows[1]);
        }
        if day > today - TREND_DAYS as i32 && day <= today {
            bump(&mut windows[2]);
        }
    }
    out
}

fn project_spends(per_project: Vec<(String, FileData)>, today: i32) -> Vec<ProjectSpend> {
    let mut out: Vec<ProjectSpend> = per_project
        .into_iter()
        .filter_map(|(project, data)| {
            // Every model folds into one key: the project's own totals.
            let folded = data.days.into_iter().map(|((day, _), v)| ((day, String::new()), v));
            let mut totals: DayMap = HashMap::new();
            for (key, (cost, tokens)) in folded {
                let entry = totals.entry(key).or_insert((0.0, 0.0));
                entry.0 += cost;
                entry.1 += tokens;
            }
            let [today_w, yesterday, last30] =
                windows_by_key(totals, today).remove("").map(|(w, _)| w).unwrap_or_default();
            let mut areas: Vec<AreaSpend> = windows_by_key(data.areas, today)
                .into_iter()
                .filter(|(_, (w, _))| w[2].cost > 0.004 || w[2].tokens > 0.0)
                .map(|(area, ([today, yesterday, last30], daily_cost))| AreaSpend {
                    area,
                    today,
                    yesterday,
                    last30,
                    daily_cost,
                    week: None,
                })
                .collect();
            areas.sort_by(|a, b| b.last30.cost.total_cmp(&a.last30.cost).then_with(|| a.area.cmp(&b.area)));
            (last30.cost > 0.004 || last30.tokens > 0.0).then_some(ProjectSpend {
                project,
                today: today_w,
                yesterday,
                last30,
                areas,
            })
        })
        .collect();
    out.sort_by(|a, b| b.last30.cost.total_cmp(&a.last30.cost).then_with(|| a.project.cmp(&b.project)));
    out
}

fn claude(extra: FileData) -> (ProviderSpend, FileData, FileData, FileData) {
    let root = std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".claude"))
        .join("projects");

    let mut files = Vec::new();
    recent_jsonl_files(&root, &mut files);
    let mut all = FileData::default();
    let mut by_project: HashMap<String, FileData> = HashMap::new();
    for file in files {
        let data = claude_file(&file);
        if let Some(project) = project_of(&root, &file) {
            merge_data(by_project.entry(project).or_default(), data.clone());
        }
        merge_data(&mut all, data);
    }
    // Usage from other scanners that belongs on this card (pi sessions)
    // driving a Claude account) joins before the splits below, so it gets
    // the same model-based routing as natively-logged rows.
    merge_data(&mut all, extra);
    let minimax = split_models(&mut all, "MiniMax");
    // Qwen-family models in Claude Code logs mean the session ran against
    // AihubMix's Anthropic-compatible endpoint (the only way qwen slugs
    // appear there) — those dollars belong on the AihubMix card.
    let qwen_via_aihubmix = split_models(&mut all, "qwen");
    // Kimi slugs likewise mean Moonshot billed the session (Anthropic-
    // compatible endpoint or a router) — Kimi's card owns those dollars.
    let kimi_routed = split_kimi_routed(&mut all);
    let mut spend = build_spend("claude", "Claude", all);
    // Projects get the same splits as the card, so their totals add up to
    // it: rows routed to another card's backend are not Claude spend.
    let known = crate::inventory::known_project_paths();
    let per_project = by_project
        .into_iter()
        .map(|(dir, mut data)| {
            let _ = split_models(&mut data, "MiniMax");
            let _ = split_models(&mut data, "qwen");
            let _ = split_kimi_routed(&mut data);
            (resolve_project(&dir, &known), data)
        })
        .collect();
    spend.projects = project_spends(per_project, today_days_from_ce());
    (spend, minimax, qwen_via_aihubmix, kimi_routed)
}

/// Spend for each discovered extra Claude account, scanned from that
/// account's own config dir (each dir keeps its own projects/ logs, so the
/// scopes never mix). MiniMax/qwen-routed rows split out the same way the
/// default account's do and are handed back for the caller to merge into
/// those cards.
fn claude_extra_accounts() -> (Vec<ProviderSpend>, FileData, FileData, FileData) {
    let mut spends = Vec::new();
    let mut minimax_extra = FileData::default();
    let mut qwen_extra = FileData::default();
    let mut kimi_extra = FileData::default();
    for acct in providers::claude::discover_extra_accounts() {
        let root = acct.dir.join("projects");
        let mut files = Vec::new();
        recent_jsonl_files(&root, &mut files);
        let mut all = FileData::default();
        for file in files {
            merge_data(&mut all, claude_file(&file));
        }
        merge_data(&mut minimax_extra, split_models(&mut all, "MiniMax"));
        merge_data(&mut qwen_extra, split_models(&mut all, "qwen"));
        merge_data(&mut kimi_extra, split_kimi_routed(&mut all));
        spends.push(build_spend(acct.id, acct.name, all));
    }
    (spends, minimax_extra, qwen_extra, kimi_extra)
}

/// One Claude session file. A restored checkpoint skips the 1 MB
/// warmup; an older cache without one still warms. Exact hits must
/// not `store_*` a Default and wipe a good checkpoint.
fn claude_file(file: &Path) -> FileData {
    if cache_unchanged(file) {
        return file_days(file, &mut |_, _| {});
    }
    let tail = will_resume_tail(file);
    let ckpt = if tail { load_claude_ckpt(file) } else { None };
    let mut state = ckpt.clone().unwrap_or_default();
    // Always fresh from the current path, whatever the checkpoint carried:
    // cheap, and a `clip_claude_ckpt` reset must not cost a subagent
    // transcript its attribution.
    state.parent_session = sidechain_parent(file);
    let data = if tail && ckpt.as_ref().is_some_and(|s| !s.seen.is_empty()) {
        file_days(file, &mut |line, data| claude_line(&mut state, line, data))
    } else if tail {
        file_days_stateful(file, &mut |line, data| claude_line(&mut state, line, data))
    } else {
        file_days(file, &mut |line, data| claude_line(&mut state, line, data))
    };
    store_claude_ckpt(file, state);
    data
}

/// MiniMax spend: the Agent CLI's local token_usage store (its own cost_usd
/// preferred, catalog-priced otherwise) plus whatever Claude Code logged
/// while pointed at MiniMax's endpoint (passed in from the Claude scan).
fn minimax(extra: FileData) -> ProviderSpend {
    let mut data = extra;
    for ev in providers::minimax::collect_usage_events() {
        let Some(ts) = DateTime::from_timestamp_millis(ev.ts_ms) else { continue };
        let tokens = ev.input + ev.output + ev.reasoning + ev.cache_read + ev.cache_write;
        if tokens <= 0.0 && ev.cost_usd <= 0.0 {
            continue;
        }
        // Rows carry provider-prefixed slugs ("minimax/MiniMax-M3") — the
        // bare model is what the card, catalogs, and the Claude-side split
        // all use.
        let model = ev.model.strip_prefix("minimax/").unwrap_or(&ev.model).to_string();
        if ev.cost_usd > 0.0 {
            add_event(&mut data, ts, &model, ev.cost_usd, tokens);
            continue;
        }
        match pricing::lookup(&model) {
            Some(p) => {
                let u = pricing::Usage {
                    input: ev.input,
                    output: ev.output + ev.reasoning,
                    cache_read: ev.cache_read,
                    cache_write_5m: ev.cache_write,
                    cache_write_1h: 0.0,
                };
                add_event(&mut data, ts, &model, cost_for(&model, &p, &u, 200_000.0, ts), tokens);
            }
            None => note_unpriced(&mut data, ts, &model, tokens),
        }
    }
    build_spend("minimax", "MiniMax", data)
}

/// Which spend slice a Hermes row belongs to. MiniMax- and OpenRouter-routed
/// sessions join those providers' slices (they already have cards), including
/// a custom URL pointed at those hosts. Every other route — AihubMix, a
/// custom OpenAI-compatible URL, Nous API — stays on the Hermes card.
fn hermes_bucket(billing_provider: &str, billing_base_url: &str) -> (&'static str, &'static str) {
    providers::hermes::spend_slice(billing_provider, billing_base_url)
}

/// Hermes spend, grouped per target slice. Rows are cumulative per
/// (session, model, route) — the app updates them in place while a session
/// runs — so every refresh rebuilds from the full table instead of
/// accumulating deltas. The whole session lands on its last-active day.
fn hermes() -> Vec<(&'static str, &'static str, FileData)> {
    let mut buckets: Vec<(&'static str, &'static str, FileData)> = Vec::new();
    for ev in providers::hermes::collect_usage_events() {
        let Some(ts) = DateTime::from_timestamp_millis(ev.ts_ms) else { continue };
        let tokens = ev.input + ev.output + ev.reasoning + ev.cache_read + ev.cache_write;
        if tokens <= 0.0 && ev.cost_usd <= 0.0 {
            continue;
        }
        let (id, name) = hermes_bucket(&ev.billing_provider, &ev.billing_base_url);
        let data = match buckets.iter_mut().find(|(bid, _, _)| *bid == id) {
            Some((_, _, data)) => data,
            None => {
                buckets.push((id, name, FileData::default()));
                &mut buckets.last_mut().unwrap().2
            }
        };
        if ev.cost_usd > 0.0 {
            add_event(data, ts, &ev.model, ev.cost_usd, tokens);
            continue;
        }
        match pricing::lookup(&providers::hermes::price_lookup_slug(
            &ev.model,
            &ev.billing_provider,
            &ev.billing_base_url,
        )) {
            Some(p) => {
                let u = pricing::Usage {
                    input: ev.input,
                    output: ev.output + ev.reasoning,
                    cache_read: ev.cache_read,
                    cache_write_5m: ev.cache_write,
                    cache_write_1h: 0.0,
                };
                // Rows aggregate a whole session's requests — long-context
                // stays base (same reasoning as the Cursor CSV scanner).
                // A session can straddle the card changeover AND a peak
                // boundary: price each duration share at its own card and
                // window (legacy 1×, new off-peak 1×, new peak 2×). One
                // event keeps the day attribution on last_seen.
                let total_ms = (ev.ts_ms - ev.start_ms).max(0);
                let peak_ms = pricing::peak_overlap_ms(ev.start_ms, ev.ts_ms);
                let legacy_ms = pricing::V41_FLASH_CHANGEOVER_MS
                    .min(ev.ts_ms)
                    .saturating_sub(ev.start_ms)
                    .clamp(0, total_ms);
                let cost = if pricing::peak_windowed(&ev.model)
                    && total_ms > 0
                    && (peak_ms > 0 || legacy_ms > 0)
                {
                    let new_base = pricing::request_cost(&p, &u, false);
                    let legacy_base = pricing::v41_flash_legacy_card(&ev.model, ev.start_ms)
                        .map(|lp| pricing::request_cost(&lp, &u, false))
                        .unwrap_or(new_base);
                    let off_ms = total_ms - legacy_ms - peak_ms;
                    (legacy_base * legacy_ms as f64
                        + new_base * off_ms as f64
                        + new_base * 2.0 * peak_ms as f64)
                        / total_ms as f64
                } else {
                    cost_for(&ev.model, &p, &u, f64::INFINITY, ts)
                };
                add_event(data, ts, &ev.model, cost, tokens);
            }
            None => note_unpriced(data, ts, &ev.model, tokens),
        }
    }
    buckets
}

/// One `token_count` usage object, tolerating the older field spellings
/// (`prompt_tokens`, `cache_read_input_tokens`, …) the Mac scanner accepts.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct CodexRaw {
    input: f64,
    cached: f64,
    output: f64,
    reasoning: f64,
    total: f64,
}

fn codex_raw(v: &Value) -> CodexRaw {
    let num = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| v.get(*k).and_then(Value::as_f64))
            .unwrap_or(0.0)
    };
    let input = num(&["input_tokens", "prompt_tokens", "input"]);
    let cached = num(&["cached_input_tokens", "cache_read_input_tokens", "cached_tokens"]);
    let output = num(&["output_tokens", "completion_tokens", "output"]);
    let reasoning = num(&["reasoning_output_tokens", "reasoning_tokens"]);
    let reported = num(&["total_tokens"]);
    let recomputed = input + output + reasoning;
    let total = if reported > 0.0 || recomputed == 0.0 { reported } else { recomputed };
    CodexRaw { input, cached, output, reasoning, total }
}

impl CodexRaw {
    fn any_tokens(&self) -> bool {
        self.input > 0.0 || self.cached > 0.0 || self.output > 0.0 || self.reasoning > 0.0
    }

    /// Recover a turn delta from cumulative totals (when `last_token_usage`
    /// is absent).
    fn minus(&self, prev: Option<&CodexRaw>) -> CodexRaw {
        let p = |f: fn(&CodexRaw) -> f64| prev.map(f).unwrap_or(0.0);
        CodexRaw {
            input: (self.input - p(|r| r.input)).max(0.0),
            cached: (self.cached - p(|r| r.cached)).max(0.0),
            output: (self.output - p(|r| r.output)).max(0.0),
            reasoning: (self.reasoning - p(|r| r.reasoning)).max(0.0),
            total: (self.total - p(|r| r.total)).max(0.0),
        }
    }
}

/// A session_meta payload marking the file as a child session (subagent
/// spawn or fork) whose leading `token_count` lines replay the parent's
/// history. JSON `null` and blank strings count as absent — a root session
/// declaring `forked_from_id: null` must not be misclassified as a child.
fn codex_child_meta(payload: &Value) -> bool {
    let set = |k: &str| {
        payload.get(k).is_some_and(|v| match v {
            Value::Null => false,
            Value::String(s) => !s.trim().is_empty(),
            _ => true,
        })
    };
    set("forked_from_id")
        || set("parent_thread_id")
        || payload.get("thread_source").and_then(Value::as_str) == Some("subagent")
        || payload.pointer("/source/subagent").is_some_and(|v| !v.is_null())
}

/// How a child session's replayed parent history is gated until its first
/// live turn.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
enum CodexReplayGate {
    /// Clear when `task_started.started_at` is at/after the child's creation
    /// epoch (replayed task_started lines carry the parent's older one).
    UntilStartedAt(f64),
    /// The child's session_meta had no parseable creation timestamp: clear
    /// when `started_at` is at/after that task_started line's own wall-clock
    /// second.
    SelfTimed,
}

/// Per-file parse state for one Codex rollout.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct CodexFileState {
    model: String,
    saw_meta: bool,
    gate: Option<CodexReplayGate>,
    fast_tier: bool,
    prev_totals: Option<CodexRaw>,
}

/// Date-stamped snapshots ("gpt-5.6-sol-2026-06-01" / "-20260601") map to
/// their base slug for the provider tables below.
fn codex_dated_base(model: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"-(\d{4}-\d{2}-\d{2}|\d{8})$").unwrap());
    re.replace(model, "").into_owned()
}

/// Codex priority/fast service-tier multipliers are provider-specific and
/// intentionally not Cursor's `-fast` supplement multipliers. Unknown models
/// use the supplement's multiplier when one exists, else 2x. Kimi/Moonshot
/// slugs billed through a Codex router are not OpenAI-tiered — leave them
/// at 1× so they merge with the Kimi CLI's own (unmultiplied) rows.
fn codex_priority_multiplier(dated: &str, rate_model: &str) -> f64 {
    let lower = rate_model.to_ascii_lowercase();
    if lower.contains("kimi") || lower.contains("moonshot") {
        return 1.0;
    }
    match dated {
        "gpt-5.5" | "gpt-5.5-pro" => 2.5,
        "gpt-5.4" | "gpt-5.4-pro" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna"
        | "gpt-6-astra" => 2.0,
        _ => {
            let m = probe_fast_multiplier(rate_model);
            if m == 1.0 { 2.0 } else { m }
        }
    }
}

/// OpenAI's published long-context rates (input, output, cache read $/MTok)
/// for Codex models — the whole request switches tiers above 272k prompt
/// tokens, not the 200k Anthropic uses.
fn codex_long_context(dated: &str) -> Option<(f64, f64, f64)> {
    match dated {
        "gpt-5.4" => Some((5.0, 22.5, 0.5)),
        "gpt-5.4-pro" | "gpt-5.5-pro" => Some((60.0, 270.0, 60.0)),
        "gpt-5.5" | "gpt-5.6-sol" => Some((10.0, 45.0, 1.0)),
        // 2026-07-31 price cut: terra/luna long-context dropped with the
        // base rates (terra used to share gpt-5.4's row).
        "gpt-5.6-terra" => Some((4.0, 18.0, 0.4)),
        "gpt-5.6-luna" => Some((0.4, 1.8, 0.04)),
        "gpt-6-astra" => Some((20.0, 75.0, 2.0)),
        _ => None,
    }
}

/// OpenAI publishes no cached-input discount for these Pro models: cached
/// input bills at the full input rate.
fn codex_no_cache_discount(dated: &str) -> bool {
    matches!(dated, "gpt-5.4-pro" | "gpt-5.5-pro")
}

/// One rollout line's effect on `st`, applied identically for the day
/// scanner (`codex_line`) and the live ticker (`codex_pace_from_lines`) so
/// the two can never read the same turn differently. Every line updates
/// state only (model, replay gate, fast tier) and returns `None`, except a
/// live, non-replayed, non-stale token_count turn, which additionally
/// returns its own timestamp, resolved model and delta usage.
fn codex_turn(st: &mut CodexFileState, v: &Value) -> Option<(DateTime<Utc>, String, CodexRaw)> {
    // Only the file's own (first) session_meta counts — a child file replays
    // the parent's session_meta lines right after its own.
    if v.get("type").and_then(Value::as_str) == Some("session_meta") {
        if !st.saw_meta {
            st.saw_meta = true;
            if let Some(p) = v.get("payload") {
                if codex_child_meta(p) {
                    st.gate = Some(match parse_ts(v.get("timestamp")) {
                        Some(ts) => CodexReplayGate::UntilStartedAt(ts.timestamp() as f64),
                        None => CodexReplayGate::SelfTimed,
                    });
                }
                if let Some(m) = p.get("model").and_then(Value::as_str) {
                    st.model = m.to_string();
                }
            }
        }
        return None;
    }

    match v.pointer("/payload/type").and_then(Value::as_str) {
        Some("thread_settings_applied") => {
            let tier = v
                .pointer("/payload/thread_settings/service_tier")
                .or_else(|| v.pointer("/payload/service_tier"))
                .and_then(Value::as_str);
            if let Some(t) = tier {
                st.fast_tier = t == "fast" || t == "priority";
            }
            None
        }
        Some("task_started") => {
            // The first live task_started ends a child's replayed history —
            // replayed ones carry the parent's original, older started_at.
            if let Some(gate) = &st.gate {
                if let Some(started) = v.pointer("/payload/started_at").and_then(Value::as_f64) {
                    let cleared = match gate {
                        CodexReplayGate::UntilStartedAt(t) => started >= *t,
                        CodexReplayGate::SelfTimed => parse_ts(v.get("timestamp"))
                            .is_some_and(|ts| started >= ts.timestamp() as f64),
                    };
                    if cleared {
                        st.gate = None;
                    }
                }
            }
            None
        }
        Some("token_count") => {
            // A model on the line itself wins.
            if let Some(m) = v
                .pointer("/payload/model")
                .and_then(Value::as_str)
                .or_else(|| v.pointer("/payload/info/model").and_then(Value::as_str))
            {
                st.model = m.to_string();
            }
            let ts = parse_ts(v.get("timestamp"))?;
            let totals = v.pointer("/payload/info/total_token_usage").map(codex_raw);

            // Replayed parent history: seed the delta baseline, never count
            // it — a large parent history takes several seconds to replay,
            // which is why this is a log marker and not a time window (the
            // Mac's old one-second window leaked replays and inflated
            // spend ~20x).
            if st.gate.is_some() {
                if let Some(t) = totals {
                    st.prev_totals = Some(t);
                }
                return None;
            }
            // Unchanged cumulative totals mean a re-emitted stale snapshot,
            // not new usage — even when the line repeats a last_token_usage.
            if let (Some(t), Some(p)) = (&totals, &st.prev_totals) {
                if t == p {
                    return None;
                }
            }
            let usage = match v.pointer("/payload/info/last_token_usage") {
                Some(l) => codex_raw(l),
                // `.as_ref()` only borrows `totals` -- it is still needed,
                // whole, to seed `prev_totals` right below.
                None => totals.as_ref()?.minus(st.prev_totals.as_ref()),
            };
            if let Some(t) = totals {
                st.prev_totals = Some(t);
            }
            if !usage.any_tokens() {
                return None;
            }
            let model = if st.model.is_empty() { "gpt-5".to_string() } else { st.model.clone() };
            Some((ts, model, usage))
        }
        _ => {
            // turn_context (or older shapes): update the session's model.
            if let Some(m) = v.pointer("/payload/model").and_then(Value::as_str) {
                st.model = m.to_string();
            }
            None
        }
    }
}

/// Prices one Codex turn's delta usage against `model`: live catalog (with
/// the dated-snapshot fallback) first, then the static gpt-5-family table,
/// else `None` (excluded, never a guessed dollar figure) — the same
/// three-tier shape `claude_cost` uses, with Codex's own fast-tier
/// multiplier and long-context threshold. Shared by the day scanner
/// (`codex_line`) and the live ticker (`codex_pace_from_lines`) so the two
/// can never quote a different dollar figure for the same turn.
fn codex_turn_cost(model: &str, usage: &CodexRaw, fast_tier: bool, ts: DateTime<Utc>) -> Option<f64> {
    // Codex speed is a provider tier, not Cursor's `-fast` price variant: a
    // `-fast` slug resolves through its unscaled base rates and the Codex
    // multiplier applies exactly once. A fast-only third-party slug with no
    // base entry keeps its already-scaled rate, no second multiplier.
    // Auto-review keeps its own name in the breakdown; only the dollar math
    // uses the dated GPT fallback (Mac parity with OpenUsage #1085).
    let rate_source = if model.eq_ignore_ascii_case("codex-auto-review") {
        auto_review_fallback(ts)
    } else {
        model.to_string()
    };
    let (rate_model, alias_fast) = match rate_source.strip_suffix("-fast") {
        Some(base) if !base.is_empty() => (base.to_string(), true),
        _ => (rate_source.clone(), false),
    };
    let lower = rate_source.to_lowercase();
    // Date-stamped Codex names ("gpt-6-astra-2026-09-01") miss the exact
    // builtin. Strip the stamp before the price lookup so Astra (and any
    // later dated GPT) uses the baked card, not generic GPT-5 rates.
    // The breakdown still records the original `model` name.
    let dated = codex_dated_base(&rate_model.to_lowercase());
    let base_price = probe_lookup(&rate_model).or_else(|| {
        if !rate_model.eq_ignore_ascii_case(&dated) {
            probe_lookup(&dated)
        } else {
            None
        }
    });
    let price = base_price
        .or_else(|| if alias_fast { probe_lookup(&rate_source) } else { None })
        .or_else(|| {
            // The static gpt-5 table only for recognizably Codex-family
            // models; anything else is excluded.
            if lower.contains("gpt") || lower.contains("codex") {
                let (i, o, cr) = codex_price(&rate_model);
                Some(pricing::Price::flat(i, o, cr, i))
            } else {
                None
            }
        });
    let mut p = price?;
    let mut threshold = 200_000.0;
    if let Some((i, o, cr)) = codex_long_context(&dated) {
        p.input_200k = Some(i);
        p.output_200k = Some(o);
        p.cache_read_200k = Some(cr);
        threshold = 272_000.0;
    }
    if codex_no_cache_discount(&dated) {
        p.cache_read = p.input;
        p.cache_read_200k = p.input_200k;
    }
    let is_fast = if alias_fast { base_price.is_some() } else { fast_tier };
    let mult = if is_fast { codex_priority_multiplier(&dated, &rate_model) } else { 1.0 };

    let cached = usage.cached.min(usage.input);
    let u = pricing::Usage {
        input: usage.input - cached,
        output: usage.output,
        cache_read: cached,
        cache_write_5m: 0.0,
        cache_write_1h: 0.0,
    };
    Some(cost_for(model, &p, &u, threshold, ts) * mult)
}

/// Parse one Codex rollout line. Tracks the current model (turn_context),
/// the fast/priority service tier (thread_settings_applied — config.toml is
/// deliberately not consulted, toggling it must not reprice history), and a
/// child session's replay gate; normalizes each token_count into a delta
/// event.
fn codex_line(st: &mut CodexFileState, line: &str, data: &mut FileData) {
    if !(line.contains("token_count")
        || line.contains("turn_context")
        || line.contains("session_meta")
        || line.contains("task_started")
        || line.contains("thread_settings_applied"))
    {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let Some((ts, model, usage)) = codex_turn(st, &v) else { return };
    let tokens = usage.total;
    match codex_turn_cost(&model, &usage, st.fast_tier, ts) {
        Some(cost) => add_event(data, ts, &model, cost, tokens),
        None => note_unpriced(data, ts, &model, tokens),
    }
}

/// `codex-auto-review` release timeline (newest first), from ccusage's
/// embedded snapshot: a line dated on/after a release prices as that model.
fn auto_review_fallback_date(date: &str) -> &'static str {
    if date.len() != 10
        || !date.as_bytes().iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
    {
        return "gpt-5";
    }
    const FALLBACKS: &[(&str, &str)] = &[
        ("2026-04-23", "gpt-5.5"),
        ("2026-03-05", "gpt-5.4"),
        ("2026-02-05", "gpt-5.3-codex"),
        ("2025-12-11", "gpt-5.2-codex"),
        ("2025-11-13", "gpt-5.1-codex"),
        ("2025-09-15", "gpt-5-codex"),
        ("2025-08-07", "gpt-5"),
    ];
    FALLBACKS
        .iter()
        .find(|(released, _)| date >= *released)
        .map(|(_, model)| *model)
        .unwrap_or("gpt-5")
}

fn auto_review_fallback(ts: DateTime<Utc>) -> String {
    auto_review_fallback_date(&ts.format("%Y-%m-%d").to_string()).to_string()
}

/// Codex rollout files log a token_count event per turn; the model rides in
/// the surrounding turn_context/session_meta lines. Child sessions (subagent
/// spawns and forks) replay the parent's entire history at spawn — those
/// lines are skipped via a replay gate (see `codex_line`).
/// Session logs of one Codex home. An archived session is often a
/// byte-for-byte copy of one still in sessions/ — count each relative path
/// once, sessions/ winning.
fn codex_session_files(home: &Path) -> Vec<PathBuf> {
    let sessions_root = home.join("sessions");
    let archived_root = home.join("archived_sessions");
    let mut files = Vec::new();
    recent_jsonl_files(&sessions_root, &mut files);
    let live_rel: HashSet<PathBuf> = files
        .iter()
        .filter_map(|f| f.strip_prefix(&sessions_root).ok().map(Path::to_path_buf))
        .collect();
    let mut archived = Vec::new();
    recent_jsonl_files(&archived_root, &mut archived);
    files.extend(archived.into_iter().filter(|f| {
        f.strip_prefix(&archived_root)
            .map(|rel| !live_rel.contains(rel))
            .unwrap_or(true)
    }));
    files
}

fn codex_scan(home: &Path) -> FileData {
    let mut all = FileData::default();
    for file in codex_session_files(home) {
        if cache_unchanged(&file) {
            merge_data(&mut all, file_days(&file, &mut |_, _| {}));
            continue;
        }
        let tail = will_resume_tail(&file);
        let ckpt = if tail { load_codex_ckpt(&file) } else { None };
        let mut state = ckpt.clone().unwrap_or_default();
        let data = if tail && ckpt.is_some() {
            file_days(&file, &mut |line, data| codex_line(&mut state, line, data))
        } else if tail {
            file_days_stateful(&file, &mut |line, data| codex_line(&mut state, line, data))
        } else {
            file_days(&file, &mut |line, data| codex_line(&mut state, line, data))
        };
        store_codex_ckpt(&file, state);
        merge_data(&mut all, data);
    }
    all
}

/// `CODEX_HOME`, or `~/.codex` when unset. The one place both the day
/// scanner and the live ticker resolve it, so the two can never disagree
/// about which account's logs they're reading.
fn codex_home() -> PathBuf {
    std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".codex"))
}

fn codex(extra: FileData) -> (ProviderSpend, FileData) {
    let mut all = codex_scan(&codex_home());
    // Pi sessions that drove a Codex account (passed in from the pi scan).
    merge_data(&mut all, extra);
    // Kimi OAuth / Moonshot turns routed through Codex (codex-router logs
    // them as "kimi-oauth/k3" etc.) bill the Kimi plan, not the ChatGPT
    // subscription — hand them to the Kimi card.
    let kimi_routed = split_kimi_routed(&mut all);
    (build_spend("codex", "Codex", all), kimi_routed)
}

/// Spend for each discovered extra Codex account, scanned from that
/// account's own home (each keeps its own sessions/ logs). Kimi-routed
/// rows split out the same way the default account's do.
fn codex_extra_accounts() -> (Vec<ProviderSpend>, FileData) {
    let mut spends = Vec::new();
    let mut kimi_extra = FileData::default();
    for acct in providers::codex::discover_extra_accounts() {
        let mut data = codex_scan(&acct.dir);
        merge_data(&mut kimi_extra, split_kimi_routed(&mut data));
        spends.push(build_spend(acct.id, acct.name, data));
    }
    (spends, kimi_extra)
}

// ---------------------------------------------------------------------------
// Live pace: Codex
// ---------------------------------------------------------------------------

/// A rollout's own `session_meta.cwd`, read from just its opening lines —
/// confirmed against the real `codex-rs` source (`SessionMeta.cwd:
/// PathBuf`, flattened onto the `session_meta` line's `payload` exactly
/// like the `forked_from_id` / `model` fields `codex_child_meta` and
/// `codex_turn` already read there); this machine has no Codex CLI
/// installed to sample a live rollout against. The record is always a
/// rollout's first line, so this never pays for a read of what can be a
/// very large file. `None` when nothing within a generous opening-line
/// bound parses as one (a foreign or truncated file), which simply drops
/// the file from cwd matching rather than guessing.
fn codex_rollout_cwd(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(20).map_while(Result::ok) {
        if !line.contains("session_meta") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        return v.pointer("/payload/cwd").and_then(Value::as_str).map(str::to_string);
    }
    None
}

/// `live_session_in` for Codex. Unlike Claude Code, which keys one folder
/// per project under `claude_projects_root()`, Codex logs every session
/// into one shared `sessions/` (plus `archived_sessions/`) tree with no
/// per-project split — so finding "this folder's" rollout means checking
/// each fresh candidate's own `session_meta.cwd` (`codex_rollout_cwd`)
/// rather than jumping straight to a directory. Freshness is checked
/// before cwd is ever read, so a month of quiet history costs nothing
/// beyond an mtime stat per file; `codex_session_files` is the same
/// enumeration (`sessions/` plus `archived_sessions/`, archived duplicates
/// of a live file counted once) the day scanner uses.
fn codex_live_session_in(sessions_root: &Path, cwd: &str, now_ms: i64) -> Option<LivePace> {
    let mut best: Option<(PathBuf, SystemTime)> = None;
    for file in codex_session_files(sessions_root) {
        let Ok(mtime) = fs::metadata(&file).and_then(|m| m.modified()) else { continue };
        let mtime_ms = mtime.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as i64;
        if now_ms.saturating_sub(mtime_ms) > LIVE_FRESH_MS {
            continue;
        }
        let newer_than_best = best.as_ref().is_none_or(|(_, best_mtime)| mtime > *best_mtime);
        if !newer_than_best {
            continue;
        }
        if codex_rollout_cwd(&file).as_deref() != Some(cwd) {
            continue;
        }
        best = Some((file, mtime));
    }
    let (path, _) = best?;
    let session_id = path.file_stem().and_then(|s| s.to_str())?.to_string();
    let lines = tail_lines(&path);
    codex_pace_from_lines(lines.iter().map(String::as_str), &session_id, now_ms)
}

/// One live Codex rollout's tail, already read: recent turns priced and
/// summed exactly like the day scanner (`codex_turn` / `codex_turn_cost`
/// are the very same calls `codex_line` makes), windowed to the last 10
/// minutes, and starting from a fresh, unpersisted `CodexFileState` — a
/// live poll never resumes the on-disk checkpoint. A token_count line just
/// before the tail's own start (if any) still seeds `prev_totals`
/// correctly; being before the cutoff, it is never itself added to the
/// window.
///
/// `area` needs a folder to be relative to, which only `session_meta`
/// carries — and a busy rollout's 256 KB tail rarely reaches back that
/// far. So it is entirely best-effort: the earliest and latest `cwd` this
/// call happens to see (session_meta's own, or any later turn_context)
/// stand in for "session root" and "now", and when the tail shows only one
/// (or none), `area` is `None` rather than a guess.
fn codex_pace_from_lines<'a>(lines: impl Iterator<Item = &'a str>, session_id: &str, now_ms: i64) -> Option<LivePace> {
    let cutoff = now_ms - LIVE_WINDOW_MS;
    let mut st = CodexFileState::default();
    let mut tokens_10m = 0.0f64;
    let mut cost_10m = 0.0f64;
    let mut priced = true;
    let mut newest_ms: Option<i64> = None;
    let mut model: Option<String> = None;
    let mut root_cwd: Option<String> = None;
    let mut latest_cwd: Option<String> = None;

    for line in lines {
        if !(line.contains("token_count")
            || line.contains("turn_context")
            || line.contains("session_meta")
            || line.contains("task_started")
            || line.contains("thread_settings_applied"))
        {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };

        // Every session_meta/turn_context cwd this tail happens to carry,
        // in file order — independent of the token-delta state machine
        // below, and never gated by the window: `area` reports where the
        // session is scoped now, same as `st.model` tracks the current
        // model regardless of which turn eventually prices against it.
        if let Some(c) = v.pointer("/payload/cwd").and_then(Value::as_str).filter(|c| !c.is_empty()) {
            root_cwd.get_or_insert_with(|| c.to_string());
            latest_cwd = Some(c.to_string());
        }

        let Some((ts, turn_model, usage)) = codex_turn(&mut st, &v) else { continue };
        let ms = ts.timestamp_millis();
        if ms < cutoff {
            continue;
        }
        if newest_ms.is_none_or(|n| ms >= n) {
            newest_ms = Some(ms);
            model = Some(turn_model.clone());
        }
        tokens_10m += usage.total;
        match codex_turn_cost(&turn_model, &usage, st.fast_tier, ts) {
            Some(cost) => cost_10m += cost,
            None => priced = false,
        }
    }

    let newest_ms = newest_ms?;
    let area = match (&root_cwd, &latest_cwd) {
        (Some(root), Some(latest)) => area_under(latest, root, true),
        _ => None,
    };
    Some(LivePace {
        session_id: session_id.to_string(),
        tokens_10m: tokens_10m.round() as u64,
        cost_10m,
        priced,
        idle_secs: now_ms.saturating_sub(newest_ms).max(0) as u64 / 1000,
        model,
        area,
        tool: "Codex".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Live pace: Gemini CLI
//
// No day-scan card exists for Gemini here (nothing in this module scans
// its history), only this live ticker. Confirmed against a real 0.61.0
// session on this Mac (2026-09-25 — `npm i -g @google/gemini-cli`, one
// trivial non-interactive prompt, then a resumed second turn to check
// whether usage was cumulative): `~/.gemini/tmp/<project>/.project_root`
// is a plain text file holding the project's literal absolute path, and
// `~/.gemini/tmp/<project>/chats/*.jsonl` holds, among `"$set"` patches
// and plain "user" turns this never reads, one self-contained
// `"type":"gemini"` line per model turn carrying its own `tokens`
// (input/cached/output/thoughts/tool/total), `model` and `timestamp` — the
// second turn's own numbers did not need the first subtracted out, so
// (unlike Codex) no running-total delta state is kept here at all.
// ---------------------------------------------------------------------------

/// Where Gemini CLI keeps its per-project chat logs. No env override
/// exists for this (unlike Codex's `CODEX_HOME`), matching how
/// `inventory.rs` already resolves the same tool's config folder.
fn gemini_tmp_root() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".gemini").join("tmp")
}

/// `live_session_in` for Gemini CLI. Gemini keys its tmp folder by
/// project, not by session, so — unlike Claude's uuid-per-session files or
/// Codex's session_meta — the newest fresh `chats/*.jsonl` under whichever
/// project directory's `.project_root` names `cwd` is the one to read;
/// nothing inside a chat log's own lines carries a folder at all.
fn gemini_live_session_in(tmp_root: &Path, cwd: &str, now_ms: i64) -> Option<LivePace> {
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    let entries = fs::read_dir(tmp_root).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(root) = fs::read_to_string(dir.join(".project_root")) else { continue };
        if root.trim() != cwd {
            continue;
        }
        let Ok(chats) = fs::read_dir(dir.join("chats")) else { continue };
        for chat in chats.flatten() {
            let path = chat.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(mtime) = chat.metadata().and_then(|m| m.modified()) else { continue };
            if newest.as_ref().is_none_or(|(_, best)| mtime > *best) {
                newest = Some((path, mtime));
            }
        }
    }
    let (path, mtime) = newest?;
    let mtime_ms = mtime.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as i64;
    if now_ms.saturating_sub(mtime_ms) > LIVE_FRESH_MS {
        return None;
    }
    let session_id = path.file_stem().and_then(|s| s.to_str())?.to_string();
    let lines = tail_lines(&path);
    gemini_pace_from_lines(lines.iter().map(String::as_str), &session_id, now_ms)
}

/// One live Gemini CLI chat log's tail, already read. Each `"type":"gemini"`
/// line's own `tokens` object is that turn's own usage (see the module
/// note above), so — unlike `codex_pace_from_lines` — there is no delta or
/// replay state to carry between lines here, only a running window sum,
/// the same shape `pace_from_lines` uses for Claude Code.
///
/// `cached` is billed as a subset of `input`, never additional to it:
/// Gemini's own `cachedContentTokenCount` is documented as counted within
/// `promptTokenCount`, the same convention `codex_turn_cost` already
/// applies to Codex's `cached_input_tokens`
/// (ai.google.dev/gemini-api/docs/usage — this machine's one real sample
/// had no cached tokens to confirm the split against directly).
/// `thoughts` and `tool` tokens count toward the reported `tokens_10m`
/// total (Gemini's own `total` already includes them) but never toward the
/// priced output bucket — the same conservative call `codex_turn_cost`
/// makes for a Codex reasoning token: real, but never a guessed dollar
/// figure. No folder signal exists inside a line itself, so `area` is
/// always `None` here; `gemini_live_session_in`'s own `.project_root`
/// match is the only place Gemini pace learns a folder.
fn gemini_pace_from_lines<'a>(lines: impl Iterator<Item = &'a str>, session_id: &str, now_ms: i64) -> Option<LivePace> {
    let cutoff = now_ms - LIVE_WINDOW_MS;
    let mut tokens_10m = 0.0f64;
    let mut cost_10m = 0.0f64;
    let mut priced = true;
    let mut newest_ms: Option<i64> = None;
    let mut model: Option<String> = None;

    for line in lines {
        if !line.contains("\"type\":\"gemini\"") || !line.contains("\"tokens\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("gemini") {
            continue;
        }
        let Some(tokens) = v.get("tokens") else { continue };
        let Some(ts) = parse_ts(v.get("timestamp")) else { continue };
        let ms = ts.timestamp_millis();
        if ms < cutoff {
            continue;
        }

        let num = |key: &str| tokens.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        let input = num("input");
        let cached = num("cached").min(input);
        let output = num("output");
        let thoughts = num("thoughts");
        let tool = num("tool");
        let reported_total = num("total");
        let total = if reported_total > 0.0 { reported_total } else { input + cached + output + thoughts + tool };
        if total <= 0.0 {
            continue;
        }
        tokens_10m += total;

        let turn_model = v.get("model").and_then(Value::as_str).unwrap_or("gemini").to_string();
        if newest_ms.is_none_or(|n| ms >= n) {
            newest_ms = Some(ms);
            model = Some(turn_model.clone());
        }

        match probe_lookup(&turn_model) {
            Some(price) => {
                let u = pricing::Usage {
                    input: input - cached,
                    output,
                    cache_read: cached,
                    cache_write_5m: 0.0,
                    cache_write_1h: 0.0,
                };
                cost_10m += cost_for(&turn_model, &price, &u, 200_000.0, ts);
            }
            None => priced = false,
        }
    }

    let newest_ms = newest_ms?;
    Some(LivePace {
        session_id: session_id.to_string(),
        tokens_10m: tokens_10m.round() as u64,
        cost_10m,
        priced,
        idle_secs: now_ms.saturating_sub(newest_ms).max(0) as u64 / 1000,
        model,
        area: None,
        tool: "Gemini CLI".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Pi coding agent — folded into the cards of the accounts it drives
// ---------------------------------------------------------------------------

/// Where pi keeps session logs, mirroring pi's own resolution: an explicit
/// `PI_CODING_AGENT_SESSION_DIR` wins, else `PI_CODING_AGENT_DIR/sessions`
/// (config-dir override), else the default `~/.pi/agent/sessions`.
fn pi_sessions_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_SESSION_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(dir) = std::env::var("PI_CODING_AGENT_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir).join("sessions");
        }
    }
    dirs::home_dir().unwrap_or_default().join(".pi").join("agent").join("sessions")
}

/// The per-file cache stores ONE FileData per path, but a pi file can hold
/// usage for several destination cards — so models are stored tagged
/// ("claude␁<model>") and untagged by take_tagged() after the scan.
const PI_SEP: char = '\u{1}';

/// One pi session-log line → spend event. Only assistant "message" lines
/// carry usage; pi's `provider` field says whose account it drove
/// (mirroring upstream OpenUsage's mapping — pi providers with no local
/// spend source here are skipped). Pi records an authoritative
/// per-message usage.cost.total like OpenCode: a carried cost > 0 wins,
/// a $0 cost (subscription usage pi doesn't impute) prices through the
/// catalog. Duplicate message ids within a file (forked-session replays)
/// keep the first occurrence.
fn pi_line(seen: &mut HashSet<String>, line: &str, data: &mut FileData) {
    if !line.contains("\"usage\"") {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v.get("type").and_then(Value::as_str) != Some("message") {
        return;
    }
    let Some(ts) = parse_ts(v.get("timestamp")) else { return };
    let Some(msg) = v.get("message") else { return };
    if msg.get("role").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let card = match msg.get("provider").and_then(Value::as_str) {
        Some("anthropic" | "claude-agent-sdk") => "claude",
        Some("openai-codex") => "codex",
        _ => return,
    };
    if let Some(id) = v.get("id").and_then(Value::as_str) {
        if !seen.insert(id.to_string()) {
            return;
        }
    }
    let Some(usage) = msg.get("usage") else { return };
    let num = |k: &str| usage.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let (input, output, cache_read) = (num("input"), num("output"), num("cacheRead"));
    let cache_write = num("cacheWrite");
    let cache_write_1h = num("cacheWrite1h").min(cache_write);
    let reported = num("totalTokens");
    let tokens =
        if reported > 0.0 { reported } else { input + output + cache_read + cache_write };
    let carried = usage.get("cost").and_then(|c| c.get("total")).and_then(Value::as_f64);
    // A row earns its place with either signal: dollars pi recorded or
    // tokens to price — same rule as the minimax/hermes scanners. Only a
    // row with neither is noise.
    if tokens <= 0.0 && carried.unwrap_or(0.0) <= 0.0 {
        return;
    }
    let model = msg
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("unknown");
    let tagged = format!("{card}{PI_SEP}{model}");
    if let Some(c) = carried.filter(|c| *c > 0.0) {
        add_event(data, ts, &tagged, c, tokens);
        return;
    }
    match probe_lookup(model) {
        Some(p) => {
            let u = pricing::Usage {
                input,
                output,
                cache_read,
                cache_write_5m: (cache_write - cache_write_1h).max(0.0),
                cache_write_1h,
            };
            add_event(data, ts, &tagged, cost_for(&tagged, &p, &u, 200_000.0, ts), tokens);
        }
        None => note_unpriced(data, ts, &tagged, tokens),
    }
}

/// Extract (and untag) every entry destined for `card` from a tagged scan.
fn take_tagged(data: &mut FileData, card: &str) -> FileData {
    let prefix = format!("{card}{PI_SEP}");
    let mut out = FileData::default();
    let day_keys: Vec<_> =
        data.days.keys().filter(|(_, m)| m.starts_with(&prefix)).cloned().collect();
    for key in day_keys {
        if let Some(v) = data.days.remove(&key) {
            out.days.insert((key.0, key.1[prefix.len()..].to_string()), v);
        }
    }
    let unpriced: Vec<String> =
        data.unpriced.keys().filter(|m| m.starts_with(&prefix)).cloned().collect();
    for m in unpriced {
        if let Some(c) = data.unpriced.remove(&m) {
            out.unpriced.insert(m[prefix.len()..].to_string(), c);
        }
    }
    out
}

/// Pi is a bring-your-own-account agent, so its usage belongs on the card
/// of the account it drove rather than a card of its own — a Claude sub
/// used inside pi lands on the Claude card, Codex likewise (the fold
/// upstream OpenUsage ships).
fn pi() -> (FileData, FileData) {
    let root = pi_sessions_dir();
    let mut files = Vec::new();
    recent_jsonl_files(&root, &mut files);
    let mut all = FileData::default();
    for file in files {
        if cache_unchanged(&file) {
            merge_data(&mut all, file_days(&file, &mut |_, _| {}));
            continue;
        }
        let tail = will_resume_tail(&file);
        let mut seen = if tail { load_pi_seen(&file) } else { HashSet::new() };
        let data = if tail && !seen.is_empty() {
            file_days(&file, &mut |line, data| pi_line(&mut seen, line, data))
        } else if tail {
            file_days_stateful(&file, &mut |line, data| pi_line(&mut seen, line, data))
        } else {
            file_days(&file, &mut |line, data| pi_line(&mut seen, line, data))
        };
        store_pi_seen(&file, seen);
        merge_data(&mut all, data);
    }
    let claude = take_tagged(&mut all, "claude");
    let codex = take_tagged(&mut all, "codex");
    (claude, codex)
}

/// Grok CLI appends one global log at ~/.grok/logs/unified.jsonl (or under
/// $GROK_HOME). Token counts ride on `shell.turn.inference_done` lines
/// (prompt/completion/reasoning/cached_prompt); those rows carry no model
/// id, so the active model is tracked per CLI process from the model-change
/// events the CLI also logs — the same scheme the Mac scanner uses.
fn grok() -> ProviderSpend {
    let root = std::env::var("GROK_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".grok"));
    let path = root.join("logs").join("unified.jsonl");
    let mut all = FileData::default();
    if path.exists() {
        if cache_unchanged(&path) {
            merge_data(&mut all, file_days(&path, &mut |_, _| {}));
        } else {
        let tail = will_resume_tail(&path);
        let mut model_by_pid = if tail { load_grok_models(&path) } else { HashMap::new() };
        let data = if tail && !model_by_pid.is_empty() {
            file_days(&path, &mut |line, data| grok_line(&mut model_by_pid, line, data))
        } else if tail {
            file_days_stateful(&path, &mut |line, data| grok_line(&mut model_by_pid, line, data))
        } else {
            file_days(&path, &mut |line, data| grok_line(&mut model_by_pid, line, data))
        };
        store_grok_models(&path, model_by_pid);
        merge_data(&mut all, data);
        }
    }
    build_spend("grok", "Grok", all)
}

fn grok_line(model_by_pid: &mut HashMap<i64, String>, line: &str, data: &mut FileData) {
            if !line.contains("inference_done") && !line.contains("model") {
                return;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else { return };
            let Some(msg) = v.get("msg").and_then(Value::as_str) else { return };
            let ctx = v.get("ctx").cloned().unwrap_or(Value::Null);
            let pid = v.get("pid").and_then(Value::as_i64);
            let model_field = match msg {
                "model changed" => ctx.get("model"),
                "model catalog: notifying clients" => ctx.get("current_model_id"),
                "backend_search: model switch" => ctx
                    .get("model")
                    .or_else(|| ctx.get("current_model_id"))
                    .or_else(|| ctx.get("model_id")),
                "subagent model resolved" => ctx.get("model_id").or_else(|| ctx.get("model")),
                _ => None,
            };
            if let Some(m) = model_field.and_then(Value::as_str) {
                let m = m.trim();
                if !m.is_empty() {
                    if let Some(pid) = pid {
                        model_by_pid.insert(pid, m.to_string());
                    }
                }
                return;
            }
            if msg != "shell.turn.inference_done" {
                return;
            }
            let num = |k: &str| ctx.get(k).and_then(Value::as_f64);
            let Some(prompt) = num("prompt_tokens") else { return };
            let Some(ts) = parse_ts(v.get("ts")) else { return };
            let output = num("completion_tokens").unwrap_or(0.0) + num("reasoning_tokens").unwrap_or(0.0);
            // cached_prompt_tokens is a subset of prompt_tokens, so the total
            // counts the prompt once.
            let cached = num("cached_prompt_tokens").unwrap_or(0.0).min(prompt);
            let tokens = prompt + output;
            if tokens <= 0.0 {
                return;
            }
            // Token rows carry no model id — attribute via the row's process;
            // rows with no attributable model are excluded, like the Mac.
            let Some(model) = pid.and_then(|p| model_by_pid.get(&p)).cloned() else { return };
            // Static backstop only for recognizably Grok-family models
            // (catalog down); it has no cache rate, so cached tokens are
            // conservatively priced as fresh input there.
            let price = probe_lookup(&model).or_else(|| {
                if model.to_lowercase().contains("grok") {
                    let (i, o) = grok_price(&model);
                    Some(pricing::Price::flat(i, o, i, i))
                } else {
                    None
                }
            });
            let Some(p) = price else {
                note_unpriced(data, ts, &model, tokens);
                return;
            };
            let u = pricing::Usage {
                input: prompt - cached,
                output,
                cache_read: cached,
                cache_write_5m: 0.0,
                cache_write_1h: 0.0,
            };
            add_event(data, ts, &model, cost_for(&model, &p, &u, 200_000.0, ts), tokens);
}

/// OpenCode stores real per-message costs in its database — no pricing
/// table needed.
/// OpenCode's local log covers every provider routed through it. Gateway
/// providers with their own card here (AihubMix) split into their own
/// spend slice — their dollars belong to that account, and the split gives
/// the card its Today/Yesterday/30d rows and Usage Trend; everything else
/// stays under OpenCode.
/// Returns OpenCode's spend plus the AihubMix rows as raw FileData — the
/// caller merges in AihubMix traffic from other CLIs (Claude Code) before
/// building the card's spend.
fn fold_opencode_data(
    events: impl IntoIterator<Item = (f64, f64, f64, String, String)>,
) -> (FileData, FileData) {
    let mut oc = FileData::default();
    let mut aihubmix = FileData::default();
    for (ts_ms, cost, tokens, model, provider) in events {
        if let Some(ts) = DateTime::from_timestamp_millis(ts_ms as i64) {
            let target = if provider == "aihubmix" { &mut aihubmix } else { &mut oc };
            add_event(target, ts, &model, cost, tokens);
        }
    }
    (oc, aihubmix)
}

/// One discovery pass, then partition. Calling extra_ledger_homes twice
/// could move a dir across the default/extra boundary if auth swapped
/// between the two reads and double-count that ledger.
fn opencode_accounts() -> (ProviderSpend, Vec<ProviderSpend>, FileData) {
    let homes = providers::opencode::extra_ledger_homes();
    let (mut oc, mut aihubmix) =
        fold_opencode_data(providers::opencode::collect_cost_events());
    let mut groups: std::collections::BTreeMap<String, (String, FileData)> =
        std::collections::BTreeMap::new();
    for (id, name, dir) in homes {
        let (data, extra_ai) = fold_opencode_data(providers::opencode::collect_cost_events_in(&dir));
        merge_data(&mut aihubmix, extra_ai);
        if id == "opencode" {
            merge_data(&mut oc, data);
        } else {
            let entry = groups.entry(id).or_insert((name, FileData::default()));
            merge_data(&mut entry.1, data);
        }
    }
    let extras = groups
        .into_iter()
        .map(|(id, (name, data))| build_spend(id, name, data))
        .collect();
    (build_spend("opencode", "OpenCode", oc), extras, aihubmix)
}

/// Devin CLI keeps per-request token metrics in its local sessions.db
/// (cloud Devin sessions bill ACUs and write no local logs, so only CLI
/// usage appears). Events carry the session's model with Windsurf-style
/// reasoning-effort suffixes stripped for pricing.
fn devin() -> ProviderSpend {
    let mut data = FileData::default();
    for ev in providers::devin::collect_usage_events() {
        let Some(ts) = DateTime::from_timestamp_millis(ev.ts_ms) else { continue };
        let tokens = ev.input + ev.output + ev.cache_read + ev.cache_write;
        if tokens <= 0.0 {
            continue;
        }
        let model = devin_model(&ev.model);
        match pricing::lookup(&model) {
            Some(p) => {
                let u = pricing::Usage {
                    input: ev.input,
                    output: ev.output,
                    cache_read: ev.cache_read,
                    cache_write_5m: ev.cache_write,
                    cache_write_1h: 0.0,
                };
                add_event(&mut data, ts, &model, cost_for(&model, &p, &u, 200_000.0, ts), tokens);
            }
            None => note_unpriced(&mut data, ts, &model, tokens),
        }
    }
    build_spend("devin", "Devin", data)
}

/// Windsurf-style slugs append a reasoning effort ("claude-opus-4-8-medium")
/// that no catalog knows; price and display the base model. Some slugs also
/// spell the model differently than the catalogs: version dots become
/// dashes ("gpt-5-6-sol-max" is GPT-5.6 Sol Max) and Fable's parts are
/// reordered.
fn devin_model(raw: &str) -> String {
    let mut base = raw;
    // Effort tiers and Max/Ultra modes bill at the base model's rates.
    // `-fast` peels only on Cognition stems, where it is a Devin mode
    // (`swe-1-6-fast`) on the base card; on every other model it is the
    // premium fast SKU and must survive so pricing applies the
    // multiplier. `-lightning` always stays so that 5× card keeps its
    // own row.
    for suffix in ["-xhigh", "-light", "-low", "-medium", "-high", "-max", "-ultra", "-fast"] {
        if let Some(b) = raw.strip_suffix(suffix) {
            if suffix == "-fast" && !cognition_stem(b) {
                break;
            }
            base = b;
            break;
        }
    }
    if base == "claude-5-fable" {
        return "claude-fable-5".into(); // LiteLLM's slug order
    }
    if let Some(rest) = base.strip_prefix("gpt-") {
        let parts: Vec<&str> = rest.splitn(3, '-').collect();
        // Version components are 1–2 digits ("5-6" is 5.6); OpenAI's
        // date-stamped snapshots ("4-0125-preview") use 4-digit segments
        // and must pass through untouched.
        let is_ver = |s: &str| {
            !s.is_empty() && s.len() <= 2 && s.chars().all(|c| c.is_ascii_digit())
        };
        if parts.len() >= 2 && is_ver(parts[0]) && is_ver(parts[1]) {
            let tail = parts.get(2).map(|t| format!("-{t}")).unwrap_or_default();
            return format!("gpt-{}.{}{}", parts[0], parts[1], tail);
        }
    }
    base.to_string()
}

/// True when a `-fast`-stripped slug bottoms out at Cognition's SWE or
/// Penguin — the same stem check pricing::resolve's `-fast` branch makes,
/// so a Devin fast-mode session and a directly priced slug agree.
fn cognition_stem(slug: &str) -> bool {
    let mut stem = slug.rsplit('/').next().unwrap_or(slug);
    for suf in ["-xhigh", "-light", "-low", "-medium", "-high", "-max", "-ultra"] {
        if let Some(next) = stem.strip_suffix(suf) {
            stem = next;
        }
    }
    matches!(stem, "swe-1.7" | "swe-1-7" | "swe-1.6" | "swe-1-6" | "penguin")
}

/// One Kimi Code CLI wire.jsonl line → spend event. usage.record rows are
/// self-contained: model, token buckets, epoch-ms time. Only the "turn"
/// scope counts — other scopes would double-report the same tokens.
fn kimi_line(line: &str, data: &mut FileData) {
    if !line.contains("\"usage.record\"") {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v.get("type").and_then(Value::as_str) != Some("usage.record") {
        return;
    }
    if v.get("usageScope").and_then(Value::as_str) != Some("turn") {
        return;
    }
    let Some(ts) = parse_ts(v.get("time")) else { return };
    let model_raw = v.get("model").and_then(Value::as_str).unwrap_or("unknown");
    // CLI plan logs `kimi-code/k3`; API logs `moonshot-ai/kimi-k3`; Codex
    // OAuth logs `kimi-oauth/k3`. Peel those vendor prefixes so one rate
    // table covers every spelling.
    let model = ["moonshot-ai/", "kimi-code/", "kimi-oauth/"]
        .iter()
        .find_map(|p| model_raw.strip_prefix(p))
        .unwrap_or(model_raw)
        .to_string();
    let u = v.get("usage").cloned().unwrap_or(Value::Null);
    let num = |k: &str| u.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let (input, output) = (num("inputOther"), num("output"));
    let (cache_read, cache_write) = (num("inputCacheRead"), num("inputCacheCreation"));
    let tokens = input + output + cache_read + cache_write;
    if tokens <= 0.0 {
        return;
    }
    // Catalogs key Kimi models as "moonshot/<slug>"; try the bare slug
    // first (alias/fuzzy chain), then the prefixed spelling.
    let price = probe_lookup(&model).or_else(|| probe_lookup(&format!("moonshot/{model}")));
    match price {
        Some(p) => {
            let usage = pricing::Usage {
                input,
                output,
                cache_read,
                cache_write_5m: cache_write,
                cache_write_1h: 0.0,
            };
            add_event(data, ts, &model, cost_for(&model, &p, &usage, 200_000.0, ts), tokens);
        }
        None => note_unpriced(data, ts, &model, tokens),
    }
}

/// One Qwen Code token-usage line → spend event. Each line is one API
/// request: ISO timestamp, model, and token buckets. `totalTokens` equals
/// input + output; `thoughtsTokens` are a subset of output (reasoning),
/// and `cachedTokens` a subset of input.
fn qwen_line(line: &str, data: &mut FileData) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let Some(ts) = parse_ts(v.get("timestamp")) else { return };
    let model = v.get("model").and_then(Value::as_str).unwrap_or("unknown").to_string();
    let num = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let cache_read = num("cachedTokens");
    let raw_input = num("inputTokens");
    let input = (raw_input - cache_read).max(0.0);
    let mut output = num("outputTokens");
    // Real ledgers show the OpenAI shape: totalTokens == input + output,
    // thoughts a subset of output. Qwen Code's gemini-cli ancestry kept
    // thoughts OUTSIDE the output count — if a future version reverts to
    // that shape, total exceeds input + output and thoughts must be added
    // so reasoning tokens aren't silently dropped.
    if num("totalTokens") > raw_input + output + 0.5 {
        output += num("thoughtsTokens");
    }
    let tokens = input + cache_read + output;
    if tokens <= 0.0 {
        return;
    }
    // Catalogs key these as bare slugs ("qwen3.8-max") or provider-prefixed.
    let price = probe_lookup(&model).or_else(|| probe_lookup(&format!("qwen/{model}")));
    match price {
        Some(p) => {
            let usage = pricing::Usage {
                input,
                output,
                cache_read,
                cache_write_5m: 0.0,
                cache_write_1h: 0.0,
            };
            add_event(data, ts, &model, cost_for(&model, &p, &usage, 200_000.0, ts), tokens);
        }
        None => note_unpriced(data, ts, &model, tokens),
    }
}

/// Qwen Code spend: the CLI's per-request ledger under ~/.qwen/usage —
/// one token-usage-YYYY-MM.jsonl per month, one line per API request.
fn qwen() -> ProviderSpend {
    let root = dirs::home_dir().unwrap_or_default().join(".qwen").join("usage");
    let mut files = Vec::new();
    recent_jsonl_files(&root, &mut files);
    // Only the per-request ledger counts — a future rollup/summary jsonl
    // in the same tree would double-report the same tokens.
    files.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("token-usage-"))
    });
    let mut all = FileData::default();
    for file in files {
        let data = file_days(&file, &mut |line, data| qwen_line(line, data));
        merge_data(&mut all, data);
    }
    build_spend("qwen", "Qwen Code", all)
}

/// Kimi Code spend: CLI sessions under ~/.kimi-code/sessions — each
/// agent's wire.jsonl logs one usage.record per turn. Files on the plan
/// card when a login or pasted plan key exists *and* the card is on;
/// otherwise they stay on Moonshot so API-only installs (and leftover
/// logs, or a Kimi card still sitting in Disabled after `kimi login`)
/// don't lose the dollars.
fn kimi(extra: FileData) -> ProviderSpend {
    let root = providers::kimi::code_home().join("sessions");
    let mut files = Vec::new();
    recent_jsonl_files(&root, &mut files);
    let mut all = FileData::default();
    for file in files {
        let data = file_days(&file, &mut |line, data| kimi_line(line, data));
        merge_data(&mut all, data);
    }
    // Kimi/Moonshot-billed turns other CLIs logged (Codex through a
    // router, Claude Code on Moonshot's endpoint) join the same card.
    merge_data(&mut all, extra);
    let (id, name) = kimi_spend_target(
        providers::kimi::has_credentials(),
        providers::provider_disabled("kimi"),
    );
    build_spend(id, name, all)
}

fn kimi_spend_target(has_login: bool, kimi_disabled: bool) -> (&'static str, &'static str) {
    if has_login && !kimi_disabled {
        ("kimi", "Kimi Code")
    } else {
        ("moonshot", "Kimi API")
    }
}

/// Cursor spend from the dashboard's usage-events CSV export (fetched by the
/// async caller — this stays a pure parser). Column layout is discovered
/// from the header row; rows with an explicit cost win, token-only rows are
/// priced via the live catalog (the supplement carries Cursor-native models).
pub fn cursor_from_csv(csv: &str) -> ProviderSpend {
    let mut data = FileData::default();
    let mut lines = csv.lines();
    let Some(header) = lines.next() else {
        return build_spend("cursor", "Cursor", data);
    };
    let cols: Vec<String> = split_csv_row(header)
        .into_iter()
        .map(|c| c.trim().to_lowercase())
        .collect();
    let find = |names: &[&str]| {
        cols.iter().position(|c| names.iter().any(|n| c.contains(n)))
    };
    let date_col = find(&["date", "time"]);
    let model_col = find(&["model"]);
    let cost_col = find(&["cost", "amount", "price"]);
    // "Input (w/ Cache Write)" is write-inclusive; the w/o column is the
    // plain input. Their difference gets the cache-write rate.
    let input_wo_col = cols.iter().position(|c| c.contains("input") && c.contains("w/o"));
    let input_with_col = cols
        .iter()
        .position(|c| c.contains("input") && !c.contains("w/o"));
    let output_col = find(&["output"]);
    let cache_read_col = find(&["cache read", "cache_read", "cacheread"]);
    let total_col = find(&["total tokens", "total_tokens"]);
    let (Some(date_col), Some(model_col)) = (date_col, model_col) else {
        return build_spend("cursor", "Cursor", data);
    };

    for line in lines {
        let row = split_csv_row(line);
        let get = |i: Option<usize>| i.and_then(|i| row.get(i)).map(|s| s.trim()).unwrap_or("");
        let Some(ts) = parse_csv_date(get(Some(date_col))) else { continue };
        let model = {
            let m = get(Some(model_col));
            if m.is_empty() { "Unattributed".to_string() } else { m.to_string() }
        };
        let num = |i: Option<usize>| {
            get(i).replace(['$', ','], "").parse::<f64>().unwrap_or(0.0)
        };
        let input_with = num(input_with_col);
        let input_wo = if input_wo_col.is_some() { num(input_wo_col) } else { input_with };
        let cache_write = (input_with - input_wo).max(0.0);
        let output = num(output_col);
        let cache_read = num(cache_read_col);
        let tokens = {
            let t = num(total_col);
            if t > 0.0 { t } else { input_with + output + cache_read }
        };
        let explicit_cost = num(cost_col);

        if explicit_cost > 0.0 {
            add_event(&mut data, ts, &model, explicit_cost, tokens);
        } else if tokens > 0.0 {
            match pricing::lookup(&model) {
                Some(p) => {
                    let u = pricing::Usage {
                        input: input_wo,
                        output,
                        cache_read,
                        cache_write_5m: cache_write,
                        cache_write_1h: 0.0,
                    };
                    // CSV rows aggregate requests, so no single-request
                    // long-context call can be proven — stay on base rates.
                    add_event(&mut data, ts, &model, cost_for(&model, &p, &u, f64::INFINITY, ts), tokens);
                }
                None => note_unpriced(&mut data, ts, &model, tokens),
            }
        }
    }
    build_spend("cursor", "Cursor", data)
}

/// Cursor CSV dates arrive in several shapes depending on export era:
/// RFC3339, "YYYY-MM-DD HH:MM:SS", bare "YYYY-MM-DD", or epoch (s/ms).
fn parse_csv_date(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() {
        return None;
    }
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%m/%d/%Y %H:%M:%S", "%b %d, %Y, %I:%M %p", "%b %d, %Y"] {
        if let Ok(d) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(d.and_utc());
        }
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
            return d.and_hms_opt(12, 0, 0).map(|dt| dt.and_utc());
        }
    }
    if let Ok(n) = s.parse::<i64>() {
        return DateTime::from_timestamp_millis(if n > 1_000_000_000_000 { n } else { n * 1000 });
    }
    None
}

/// Minimal CSV field splitter with quoted-field support.
fn split_csv_row(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => out.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    out.push(field);
    out
}

fn spend_step<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let out = f();
    eprintln!("[aitm] spend: {name} {:?}", started.elapsed());
    out
}

fn take_join<T>(
    handle: std::thread::ScopedJoinHandle<'_, T>,
    name: &str,
    fallback: T,
) -> T {
    handle.join().unwrap_or_else(|_| {
        eprintln!("[aitm] spend: {name} panicked — keeping the other providers");
        fallback
    })
}

fn collect_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn collect(cursor_csv: Option<String>) -> Vec<ProviderSpend> {
    // Two overlapping collects share `touched` and rewrite spend_cache
    // from that set. Serialize so a second refresh waits — one scan
    // stays the same speed.
    let _busy = collect_lock().lock().unwrap_or_else(|e| e.into_inner());
    providers::sweep_temp_sqlite_copies();
    pricing::ensure_fresh();
    load_persisted_cache();
    if let Ok(mut t) = touched().lock() {
        t.clear();
    }
    let (pi_claude, pi_codex) = pi();
    // Claude / Codex / OpenCode / Devin used to run one after another on
    // this machine that is minutes of IO. They touch different trees.
    let mut list = std::thread::scope(|s| {
        let claude_t = s.spawn(|| {
            spend_step("claude", || {
                let (sp, mut mm, mut qw, mut km) = claude(pi_claude);
                let (extras, mm2, qw2, km2) = claude_extra_accounts();
                merge_data(&mut mm, mm2);
                merge_data(&mut qw, qw2);
                merge_data(&mut km, km2);
                (sp, extras, mm, qw, km)
            })
        });
        let codex_t = s.spawn(|| {
            spend_step("codex", || {
                let (sp, mut km) = codex(pi_codex);
                let (extras, km2) = codex_extra_accounts();
                merge_data(&mut km, km2);
                (sp, extras, km)
            })
        });
        let oc_t = s.spawn(|| spend_step("opencode", opencode_accounts));
        let hermes_t = s.spawn(|| spend_step("hermes", hermes));
        let grok_t = s.spawn(|| spend_step("grok", grok));
        let devin_t = s.spawn(|| spend_step("devin", devin));
        let qwen_t = s.spawn(|| spend_step("qwen", qwen));

        let (claude_sp, extra_claude_spends, mut minimax_extra, qwen_via_claude, mut kimi_routed) =
            take_join(claude_t, "claude", (
                build_spend("claude", "Claude", FileData::default()),
                Vec::new(),
                FileData::default(),
                FileData::default(),
                FileData::default(),
            ));
        let (codex_sp, extra_codex_spends, kimi_via_codex) = take_join(
            codex_t,
            "codex",
            (build_spend("codex", "Codex", FileData::default()), Vec::new(), FileData::default()),
        );
        merge_data(&mut kimi_routed, kimi_via_codex);
        let (opencode_sp, extra_opencode_spends, mut aihubmix_data) = take_join(
            oc_t,
            "opencode",
            (build_spend("opencode", "OpenCode", FileData::default()), Vec::new(), FileData::default()),
        );
        merge_data(&mut aihubmix_data, qwen_via_claude);
        let mut hermes_rest = Vec::new();
        for (id, name, data) in take_join(hermes_t, "hermes", Vec::new()) {
            if id == "minimax" {
                merge_data(&mut minimax_extra, data);
            } else {
                hermes_rest.push(build_spend(id, name, data));
            }
        }
        let mut list = vec![
            claude_sp,
            codex_sp,
            take_join(grok_t, "grok", build_spend("grok", "Grok", FileData::default())),
            opencode_sp,
            build_spend("aihubmix", "AihubMix", aihubmix_data),
            take_join(devin_t, "devin", build_spend("devin", "Devin", FileData::default())),
            minimax(minimax_extra),
            kimi(kimi_routed),
            take_join(qwen_t, "qwen", build_spend("qwen", "Qwen Code", FileData::default())),
        ];
        list.extend(extra_claude_spends);
        list.extend(extra_codex_spends);
        list.extend(extra_opencode_spends);
        list.extend(hermes_rest);
        list
    });
    if let Some(csv) = cursor_csv {
        list.push(cursor_from_csv(&csv));
    }
    // Models nothing prices yet (new slugs ship often): flag the catalog
    // to look for updates hourly instead of daily.
    if list.iter().any(|sp| sp.unpriced > 0) {
        pricing::note_unpriced();
    }
    save_persisted_cache();
    // Derived, not scanned: the fortnight comparison every view shows.
    for sp in list.iter_mut() {
        sp.week = week_over_week(&sp.daily_cost);
        for proj in sp.projects.iter_mut() {
            for area in proj.areas.iter_mut() {
                area.week = week_over_week(&area.daily_cost);
            }
        }
    }
    list.into_iter().filter(ProviderSpend::has_data).collect()
}

#[cfg(test)]
mod week_tests {
    use super::*;

    #[test]
    fn needs_a_full_fortnight() {
        assert_eq!(week_over_week(&[1.0; 13]), None, "13 readings cannot make two weeks");
        assert!(week_over_week(&[1.0; 14]).is_some());
    }

    #[test]
    fn compares_the_last_seven_with_the_seven_before() {
        let mut days = vec![2.0; 7];
        days.extend([3.0; 7]);
        let d = week_over_week(&days).expect("a fortnight");
        assert_eq!(d.last_week, 14.0);
        assert_eq!(d.this_week, 21.0);
        assert_eq!(d.change_percent, Some(50.0));
    }

    #[test]
    fn only_the_last_fortnight_counts() {
        // A 30-day series: everything before the last 14 days is ignored.
        let mut days = vec![99.0; 16];
        days.extend([1.0; 7]);
        days.extend([2.0; 7]);
        let d = week_over_week(&days).expect("a fortnight");
        assert_eq!(d.last_week, 7.0);
        assert_eq!(d.this_week, 14.0);
    }

    #[test]
    fn a_week_from_nothing_has_no_percentage() {
        let mut days = vec![0.0; 7];
        days.extend([5.0; 7]);
        let d = week_over_week(&days).expect("a fortnight");
        assert_eq!(d.change_percent, None, "up from zero is not a percentage");
        assert_eq!(d.this_week, 35.0);
    }

    #[test]
    fn a_quiet_fortnight_is_zero_not_absent() {
        let d = week_over_week(&[0.0; 14]).expect("a fortnight");
        assert_eq!(d.this_week, 0.0);
        assert!(d.this_week.is_sign_positive(), "never -0.0 in JSON");
        assert_eq!(d.change_percent, None);
    }

    #[test]
    fn a_drop_is_negative() {
        let mut days = vec![10.0; 7];
        days.extend([5.0; 7]);
        assert_eq!(week_over_week(&days).unwrap().change_percent, Some(-50.0));
    }
}

#[cfg(test)]
mod tests {
    /// Sentinel lines `scripts/make-demo-fixture.py` scans for around an
    /// ignored fixture test's printed JSON, so it can find that JSON by
    /// exact bracketing lines rather than by a leading substring like
    /// `"[{"` -- which a legitimately empty array (`[]`) never starts with,
    /// so that heuristic misread "no activity yet" as "no output at all".
    const FIXTURE_BEGIN: &str = "AITM_FIXTURE_BEGIN";
    const FIXTURE_END: &str = "AITM_FIXTURE_END";

    /// `parse_today_override` only ever accepts the one shape
    /// scripts/make-demo-fixture.py writes -- a bare ISO `YYYY-MM-DD` --
    /// and rejects everything else, including a real calendar date spelled
    /// in a different field order, rather than guessing at it.
    #[test]
    fn today_override_parses_iso_and_rejects_garbage() {
        assert_eq!(super::parse_today_override("2026-09-25"), chrono::NaiveDate::from_ymd_opt(2026, 9, 25));
        assert_eq!(super::parse_today_override(""), None, "empty string");
        assert_eq!(super::parse_today_override("not-a-date"), None, "garbage");
        assert_eq!(super::parse_today_override("25-09-2026"), None, "non-ISO field order (day-month-year)");
    }

    /// Prints real sessions as JSON: `{"area": [...], "day": [...]}` for the
    /// area and local day named in AITM_AREA / AITM_DAY.
    #[test]
    #[ignore]
    fn live_sessions() {
        let _ = collect(None);
        let area = std::env::var("AITM_AREA").ok();
        let day = std::env::var("AITM_DAY").ok();
        let out = json!({
            "area": claude_sessions(area.as_deref(), None, 40),
            "day": claude_sessions(None, day.as_deref(), 40),
        });
        println!("{out}");
    }

    /// Prints this machine's real spend. `cargo test -p aitm-core live_spend -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_spend() {
        println!("{FIXTURE_BEGIN}");
        println!("{}", serde_json::to_string(&collect(None)).unwrap());
        println!("{FIXTURE_END}");
    }

    /// Prints this machine's real 30-day agent spend, same call `get_agent_spend`
    /// makes. `cargo test -p aitm-core live_agent_spend -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_agent_spend() {
        let _ = collect(None);
        println!("{FIXTURE_BEGIN}");
        println!("{}", serde_json::to_string(&agent_spend(30)).unwrap());
        println!("{FIXTURE_END}");
    }

    /// Diagnostic (ignored): how many entries the scan cache would list as
    /// sessions without the new "not a sidechain" filter, against how many
    /// `claude_sessions` actually lists now, plus the top three
    /// `agent_spend` names by cost. Names and counts only -- never a
    /// session id, a path, or anything from message content. A stale
    /// persisted cache from a build compiled before this change (`tauri
    /// dev` rebuilds the running app on any edit under `crates/`, per this
    /// repo's own build notes) can otherwise mask the fix behind its own
    /// `cache_unchanged` fast path; delete `spend_cache.json` in this app's
    /// config dir first if these numbers look unchanged from before.
    /// Run: cargo test -p aitm-core live_subagent_fold_report -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_subagent_fold_report() {
        let _ = collect(None);
        let root = claude_projects_root();
        let known = crate::inventory::known_project_paths();
        let today = today_days_from_ce();
        let before = {
            let Ok(map) = cache().lock() else { return };
            map.iter()
                .filter_map(|(path, entry)| {
                    let project = project_of(&root, path)?;
                    let id = path.file_stem()?.to_str()?;
                    session_from(
                        id,
                        &resolve_project(&project, &known),
                        &entry.data,
                        entry.claude.as_ref(),
                        today,
                        entry.size,
                    )
                })
                .count()
        };
        let after = claude_sessions(None, None, usize::MAX).len();
        println!("sessions before (every cache entry, no sidechain filter): {before}");
        println!("sessions after  (subagent transcripts folded into their parent): {after}");
        println!("top agent_spend by cost (30d):");
        for a in agent_spend(30).into_iter().take(3) {
            println!("  {:<20} runs={:<4} cost=${:.2}", a.name, a.runs, a.cost);
        }
    }

    #[test]
    fn project_dirs_resolve_to_known_paths_and_fall_back_to_the_folder_name() {
        let known = vec!["/Users/me/work/acme-site".to_string(), "/Users/me/.dotfiles".to_string()];
        // Claude Code replaces every non-alphanumeric character with '-'.
        assert_eq!(encode_project_path("/Users/me/.dotfiles"), "-Users-me--dotfiles");
        assert_eq!(resolve_project("-Users-me-work-acme-site", &known), "/Users/me/work/acme-site");
        assert_eq!(resolve_project("-Users-me--dotfiles", &known), "/Users/me/.dotfiles");
        assert_eq!(resolve_project("-Users-me-gone", &known), "-Users-me-gone");
    }

    #[test]
    fn project_of_is_the_first_folder_under_the_projects_root() {
        let root = Path::new("/home/me/.claude/projects");
        assert_eq!(
            project_of(root, &root.join("-work-acme").join("abc.jsonl")).as_deref(),
            Some("-work-acme")
        );
        assert_eq!(
            project_of(root, &root.join("-work-acme").join("sub").join("abc.jsonl")).as_deref(),
            Some("-work-acme"),
            "sidechain files in a subfolder still belong to the project"
        );
        assert_eq!(project_of(root, &root.join("loose.jsonl")), None);
        assert_eq!(project_of(root, Path::new("/elsewhere/x.jsonl")), None);
    }

    #[test]
    fn project_spend_windows_match_the_card_and_sort_by_thirty_day_cost() {
        let today = 800_000;
        let mut small = FileData::default();
        small.days.insert((today, "opus".into()), (2.0, 100.0));
        let mut big = FileData::default();
        big.days.insert((today, "opus".into()), (5.0, 10.0));
        big.days.insert((today - 1, "sonnet".into()), (7.0, 20.0));
        big.days.insert((today - 10, "opus".into()), (30.0, 30.0));
        big.days.insert((today - 45, "opus".into()), (999.0, 999.0)); // outside 30 days
        let got = project_spends(
            vec![("small".into(), small), ("big".into(), big), ("empty".into(), FileData::default())],
            today,
        );
        assert_eq!(got.len(), 2, "a project with no spend in range is left out");
        assert_eq!(got[0].project, "big");
        assert_eq!((got[0].today.cost, got[0].yesterday.cost, got[0].last30.cost), (5.0, 7.0, 42.0));
        assert_eq!(got[0].last30.tokens, 60.0);
        assert_eq!(got[1].project, "small");
        assert_eq!(got[1].last30.cost, 2.0);
    }

    #[test]
    fn daily_cost_lines_up_with_the_token_trend() {
        let today = today_days_from_ce();
        let mut data = FileData::default();
        data.days.insert((today, "opus".into()), (3.0, 10.0));
        data.days.insert((today, "sonnet".into()), (1.5, 5.0));
        data.days.insert((today - 2, "opus".into()), (8.0, 40.0));
        let sp = build_spend("claude", "Claude", data);
        assert_eq!(sp.daily_cost.len(), TREND_DAYS);
        assert_eq!(sp.daily_cost[TREND_DAYS - 1], 4.5, "last slot is today");
        assert_eq!(sp.daily_cost[TREND_DAYS - 3], 8.0);
        assert_eq!(sp.trend[TREND_DAYS - 3], 40.0, "same indexing as the token trend");
        assert!(sp.projects.is_empty(), "only the Claude scan attaches projects");
    }
    use super::*;
    use serde_json::json;

    fn tokens_sum(d: &FileData) -> f64 {
        d.days.values().map(|v| v.1).sum()
    }

    fn cost_sum(d: &FileData) -> f64 {
        d.days.values().map(|v| v.0).sum()
    }

    // ---- Log scan: bounded walk ------------------------------------------

    #[test]
    fn scan_stops_at_the_depth_cap() {
        let base = std::env::temp_dir()
            .join(format!("pane-scan-depth-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let mut deep = base.clone();
        for i in 0..MAX_SCAN_DEPTH + 4 {
            deep = deep.join(format!("d{i}"));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(base.join("top.jsonl"), "{}").unwrap();
        fs::write(deep.join("too-deep.jsonl"), "{}").unwrap();

        let mut out = Vec::new();
        recent_jsonl_files(&base, &mut out);
        let _ = fs::remove_dir_all(&base);

        assert!(out.iter().any(|p| p.ends_with("top.jsonl")));
        assert!(!out.iter().any(|p| p.ends_with("too-deep.jsonl")));
    }

    /// A junction is followed (std reports NTFS mount points as symlinks),
    /// and a subtree reachable both directly and through the junction still
    /// counts each log exactly once.
    #[test]
    #[cfg(windows)]
    fn junction_alias_counts_each_log_once() {
        let base = std::env::temp_dir()
            .join(format!("pane-scan-junction-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("session.jsonl"), "{}").unwrap();
        let link = base.join("alias");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&real)
            .status();
        if !status.map(|s| s.success()).unwrap_or(false) {
            let _ = fs::remove_dir_all(&base);
            return; // mklink unavailable in this environment — skip
        }

        let mut out = Vec::new();
        recent_jsonl_files(&base, &mut out);
        let _ = fs::remove_dir_all(&base);

        let hits = out.iter().filter(|p| p.ends_with("session.jsonl")).count();
        assert_eq!(hits, 1, "aliased log counted {hits} times: {out:?}");
    }

    // ---- Persistent cache: serialization roundtrip -----------------------

    #[test]
    fn persist_file_roundtrips_losslessly() {
        let doc = PersistFile {
            version: PERSIST_VERSION,
            pricing_stamp: "litellm:1:2|modelsdev:3:4|supplement:5:6".into(),
            corrections: pricing::corrections_rev(),
            entries: vec![PersistEntry {
                path: PathBuf::from(r"C:\logs\session.jsonl"),
                mtime_secs: 1_784_600_000,
                mtime_nanos: 123_456_700, // NTFS 100ns precision must survive
                size: 4096,
                days: vec![(739_000, "claude-fable-5".into(), 1.25, 40_000.0)],
                areas: vec![(739_000, "acme".into(), 1.25, 40_000.0)],
                unpriced: vec![("mystery-model".into(), 3)],
                probes: vec![
                    PriceProbe::Lookup {
                        key: "claude-fable-5".into(),
                        price: Some(pricing::Price::flat(3.0, 15.0, 0.3, 3.75)),
                    },
                    PriceProbe::Lookup { key: "mystery-model".into(), price: None },
                    PriceProbe::FastMult { key: "claude-fable-5".into(), mult: 2.0 },
                ],
                prefix_head: b"head".to_vec(),
                prefix_tail: b"abcd".to_vec(),
                grok_models: vec![(42, "grok-4".into())],
                codex: None,
                claude: Some(ClaudeFileState {
                    seen: ["msg_1:req_1".into()].into_iter().collect(),
                    seen_mids: [("msg_1".into(), false)].into_iter().collect(),
                    root: Some("/work".into()),
                    area: Some("acme".into()),
                    first_ms: Some(1_790_000_000_000),
                    last_ms: Some(1_790_000_900_000),
                    parent_session: None, // #[serde(skip)]: never persisted, recomputed from the path instead
                }),
                pi_seen: vec!["pi-msg-1".into()],
                parent_session: Some("11111111-1111-1111-1111-111111111111".into()),
                agent: Some("Explore".into()),
            }],
        };
        let json = serde_json::to_string(&doc).unwrap();
        let back: PersistFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, doc.version);
        assert_eq!(back.pricing_stamp, doc.pricing_stamp);
        assert_eq!(back.corrections, doc.corrections);
        let (a, b) = (&back.entries[0], &doc.entries[0]);
        assert_eq!(a.path, b.path);
        assert_eq!((a.mtime_secs, a.mtime_nanos, a.size), (b.mtime_secs, b.mtime_nanos, b.size));
        assert_eq!(a.days, b.days);
        assert_eq!(a.areas, b.areas);
        assert_eq!(a.unpriced, b.unpriced);
        assert_eq!(a.probes, b.probes);
        assert_eq!(a.prefix_head, b.prefix_head);
        assert_eq!(a.prefix_tail, b.prefix_tail);
        assert_eq!(a.grok_models, b.grok_models);
        assert_eq!(a.codex.is_none(), b.codex.is_none());
        assert_eq!(
            a.claude.as_ref().map(|s| s.seen.len()),
            b.claude.as_ref().map(|s| s.seen.len())
        );
        assert_eq!(a.pi_seen, b.pi_seen);
        assert_eq!(a.parent_session, b.parent_session);
        assert_eq!(a.agent, b.agent);
        // A tail parse resumes from this checkpoint: the area in force has
        // to survive, or the next lines would book to "(unsorted)".
        let (sa, sb) = (a.claude.as_ref().unwrap(), b.claude.as_ref().unwrap());
        assert_eq!((&sa.root, &sa.area), (&sb.root, &sb.area));
        assert_eq!((sa.first_ms, sa.last_ms), (sb.first_ms, sb.last_ms));
    }

    /// A v2 cache (no probes/corrections fields) must not load as v3 —
    /// its entries carry no probes, so a stamp mismatch could never
    /// revalidate them and stale prices would look valid forever.
    #[test]
    fn old_cache_versions_are_discarded() {
        let v2 = r#"{"version":2,"pricing_stamp":"x","entries":[]}"#;
        let doc: PersistFile = serde_json::from_str(v2).unwrap();
        assert_ne!(doc.version, PERSIST_VERSION);
    }

    /// The cache format moved from 9 to 10 when `sidechain_parent` learned
    /// to recognize a workflow transcript nested under
    /// `subagents/workflows/<workflow-id>/`, not just directly under
    /// `subagents/`: a cache an older build wrote must not be trusted, or
    /// every one of those transcripts it already parsed would go on looking
    /// like its own phantom session forever. Exercises the real decision
    /// (`load_persisted_cache_from`) rather than just comparing version
    /// numbers: a version-9 doc's own entry must never reach the live map.
    #[test]
    fn persist_version_10_discards_a_version_9_cache() {
        assert_eq!(PERSIST_VERSION, 10, "the cache-format version this fix shipped under");
        let fake_path = PathBuf::from("/pane-test-fixture/persist-version-10-discard/uuid.jsonl");
        let v9 = PersistFile {
            version: 9,
            pricing_stamp: "x".to_string(),
            // Matches the live revision on purpose, so the version mismatch
            // alone is what's under test -- not an incidental
            // corrections-revision mismatch riding along with it.
            corrections: pricing::corrections_rev(),
            entries: vec![PersistEntry {
                path: fake_path.clone(),
                days: vec![(19_000, "claude-haiku-4-5".to_string(), 1.0, 10.0)],
                ..Default::default()
            }],
        };
        let tmp = std::env::temp_dir().join(format!("pane-persist-v9-discard-{}.json", std::process::id()));
        fs::write(&tmp, serde_json::to_string(&v9).unwrap()).unwrap();
        load_persisted_cache_from(&tmp);
        let _ = fs::remove_file(&tmp);
        let map = cache().lock().unwrap_or_else(|e| e.into_inner());
        assert!(!map.contains_key(&fake_path), "a version-9 cache's entries must never reach the live map");
    }

    /// SWE/Penguin + V4.1 Flash baked rates bumped CORRECTIONS_REV. A
    /// cache written under 12 would load without probe replay and keep
    /// unpriced totals if the revision still matched.
    #[test]
    fn stale_corrections_revision_is_not_current() {
        assert!(
            pricing::corrections_rev() >= 13,
            "V4.1 Flash rates must bump CORRECTIONS_REV"
        );
        let stale = r#"{"version":3,"pricing_stamp":"x","corrections":12,"entries":[]}"#;
        let doc: PersistFile = serde_json::from_str(stale).unwrap();
        assert_ne!(
            doc.corrections,
            pricing::corrections_rev(),
            "rev 12 must not match the live corrections revision"
        );
    }

    // ---- Price probes: catalog-refresh revalidation ------------------------

    /// Probes replay against the live catalog: an unknown model recorded as
    /// None still answers None (valid); pretending it had a price fails
    /// validation (that file would re-parse).
    #[test]
    fn price_probes_replay_against_the_catalog() {
        let absent = PriceProbe::Lookup {
            key: "pane-test-model-that-cannot-exist".into(),
            price: None,
        };
        assert!(absent.still_valid());
        let phantom = PriceProbe::Lookup {
            key: "pane-test-model-that-cannot-exist".into(),
            price: Some(pricing::Price::flat(1.0, 2.0, 0.1, 1.0)),
        };
        assert!(!phantom.still_valid());
        // fast_multiplier returns 1.0 for models the supplement doesn't
        // publish a multiplier for.
        let mult = PriceProbe::FastMult {
            key: "pane-test-model-that-cannot-exist".into(),
            mult: 1.0,
        };
        assert!(mult.still_valid());
        let wrong_mult = PriceProbe::FastMult {
            key: "pane-test-model-that-cannot-exist".into(),
            mult: 3.5,
        };
        assert!(!wrong_mult.still_valid());
    }

    /// file_days records the pricing questions a parse asked, so the entry
    /// can be revalidated after a catalog refresh without re-reading it.
    #[test]
    fn file_days_records_price_probes() {
        let dir = std::env::temp_dir().join(format!("pane-probe-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("probe-test.jsonl");
        let line = json!({"type": "usage.record", "model": "kimi-code/k3",
            "usage": {"inputOther": 1000.0, "output": 1000.0},
            "usageScope": "turn", "time": 1784208630652i64})
        .to_string();
        fs::write(&path, format!("{line}\n")).unwrap();

        let data = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!(!data.days.is_empty());
        let probes = cache()
            .lock()
            .unwrap()
            .get(&path)
            .map(|e| e.probes.clone())
            .unwrap_or_default();
        let _ = fs::remove_file(&path);
        assert!(
            probes
                .iter()
                .any(|p| matches!(p, PriceProbe::Lookup { key, .. } if key == "k3")),
            "expected a k3 lookup probe, got {} probes",
            probes.len()
        );
        // Everything just recorded replays valid against the same catalog.
        assert!(probes.iter().all(PriceProbe::still_valid));
    }

    #[test]
    fn oversized_prefix_fingerprint_is_dropped() {
        assert!(clip_fingerprint(vec![1; PREFIX_HEAD + 1], PREFIX_HEAD).is_empty());
        assert_eq!(clip_fingerprint(vec![1, 2, 3], PREFIX_HEAD), vec![1, 2, 3]);
    }

    #[test]
    fn empty_probes_do_not_vouch_for_an_empty_parse() {
        // Failed-open artifact: no events, no questions — must re-parse.
        assert!(!probes_still_vouch(&[], &FileData::default()));
        // A parse that carried its own dollars never asked the catalog.
        let mut data = FileData::default();
        data.days.insert((1, "k3".into()), (1.0, 1000.0));
        assert!(probes_still_vouch(&[], &data));
    }

    #[test]
    fn cache_unchanged_rejects_stale_prices() {
        let dir = std::env::temp_dir().join(format!("pane-cache-gen-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("session.jsonl");
        let line = json!({
            "type": "usage.record",
            "model": "kimi-code/k3",
            "usage": {"inputOther": 1000.0, "output": 0.0},
            "usageScope": "turn",
            "time": 1_784_208_630_652i64
        })
        .to_string();
        fs::write(&path, format!("{line}\n")).unwrap();
        let _ = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!(cache_unchanged(&path), "fresh parse must look unchanged");

        if let Ok(mut map) = cache().lock() {
            if let Some(e) = map.get_mut(&path) {
                e.gen = e.gen.saturating_add(1);
                e.probes = vec![PriceProbe::Overflow];
            }
        }
        assert!(
            !cache_unchanged(&path),
            "stale generation with dead probes must not take the empty-parser path"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_days_does_not_cache_an_unreadable_path() {
        // A directory has metadata but cannot be read as a file — the
        // previous insert-on-open-failure path would cache empty spend.
        let dir = std::env::temp_dir().join(format!("pane-unreadable-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let data = file_days(&dir, &mut |_, _| {});
        assert!(data.days.is_empty());
        let cached = cache().lock().unwrap().contains_key(&dir);
        let _ = fs::remove_dir_all(&dir);
        assert!(!cached, "unreadable path must not become a cache entry");
    }

    #[test]
    fn file_days_reads_only_the_appended_tail() {
        let dir = std::env::temp_dir().join(format!("pane-jsonl-tail-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("grow.jsonl");
        let line = |tokens: f64| {
            json!({
                "type": "usage.record",
                "model": "kimi-code/k3",
                "usage": {"inputOther": tokens, "output": 0.0},
                "usageScope": "turn",
                "time": 1_784_208_630_652i64
            })
            .to_string()
        };
        fs::write(&path, format!("{}\n", line(1_000.0))).unwrap();
        let first = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!((tokens_sum(&first) - 1_000.0).abs() < 0.001);

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        writeln!(f, "{}", line(4_000.0)).unwrap();
        drop(f);

        let second = file_days(&path, &mut |line, data| kimi_line(line, data));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            (tokens_sum(&second) - 5_000.0).abs() < 0.001,
            "appended line must add to the cached prefix, got {}",
            tokens_sum(&second)
        );
    }

    #[test]
    fn file_days_warms_codex_state_on_the_tail() {
        let dir = std::env::temp_dir().join(format!("pane-codex-warm-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rollout.jsonl");
        let head = [
            json!({"timestamp": "2026-09-10T10:00:00Z", "type": "turn_context",
                   "payload": {"model": "gpt-5.6-terra"}})
            .to_string(),
            token_count_line("2026-09-10T10:00:01Z", Some((1_000.0, 100.0)), (1_000.0, 100.0)),
        ];
        fs::write(&path, format!("{}\n{}\n", head[0], head[1])).unwrap();
        let mut st = CodexFileState::default();
        let first = file_days_stateful(&path, &mut |line, data| codex_line(&mut st, line, data));
        assert_eq!(tokens_sum(&first), 1_100.0);

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        writeln!(
            f,
            "{}",
            token_count_line("2026-09-10T10:00:09Z", None, (1_500.0, 150.0))
        )
        .unwrap();
        drop(f);

        // Production creates a fresh closure state per scan — warmup must
        // refill prev_totals so the cumulative snapshot is a delta.
        let mut st = CodexFileState::default();
        let second = file_days_stateful(&path, &mut |line, data| codex_line(&mut st, line, data));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(
            tokens_sum(&second),
            1_650.0,
            "tail without warmup would add the full 1650 snapshot (2750)"
        );
    }

    #[test]
    fn file_days_does_not_skip_a_completed_partial_line() {
        let dir = std::env::temp_dir().join(format!("pane-jsonl-partial-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("partial.jsonl");
        let line = |tokens: f64| {
            json!({
                "type": "usage.record",
                "model": "kimi-code/k3",
                "usage": {"inputOther": tokens, "output": 0.0},
                "usageScope": "turn",
                "time": 1_784_208_630_652i64
            })
            .to_string()
        };
        let first_line = line(1_000.0);
        let second_line = line(4_000.0);
        fs::write(&path, format!("{first_line}\n{}", &second_line[..12])).unwrap();
        let first = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!((tokens_sum(&first) - 1_000.0).abs() < 0.001);
        let cached_size = cache().lock().unwrap().get(&path).map(|e| e.size).unwrap_or(0);
        assert_eq!(cached_size, first_line.len() as u64 + 1);

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        writeln!(f, "{}", &second_line[12..]).unwrap();
        drop(f);

        let second = file_days(&path, &mut |line, data| kimi_line(line, data));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            (tokens_sum(&second) - 5_000.0).abs() < 0.001,
            "completed line must be parsed on the next scan, got {}",
            tokens_sum(&second)
        );
    }

    #[test]
    fn file_days_counts_a_final_record_without_newline() {
        let dir = std::env::temp_dir().join(format!("pane-jsonl-final-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("final.jsonl");
        let line = |tokens: f64| {
            json!({
                "type": "usage.record",
                "model": "kimi-code/k3",
                "usage": {"inputOther": tokens, "output": 0.0},
                "usageScope": "turn",
                "time": 1_784_208_630_652i64
            })
            .to_string()
        };
        let first_line = line(1_000.0);
        let second_line = line(4_000.0);
        let third_line = line(2_000.0);
        // Two records, last one without a newline — align must not
        // walk back over the preceding `\n` when the file later grows.
        fs::write(&path, format!("{first_line}\n{second_line}")).unwrap();
        let first = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!(
            (tokens_sum(&first) - 5_000.0).abs() < 0.001,
            "closed file without a trailing newline must still count, got {}",
            tokens_sum(&first)
        );
        let cached_size = cache().lock().unwrap().get(&path).map(|e| e.size).unwrap_or(0);
        assert_eq!(cached_size, first_line.len() as u64 + 1 + second_line.len() as u64);

        let again = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!(
            (tokens_sum(&again) - 5_000.0).abs() < 0.001,
            "unchanged closed file must not double-count, got {}",
            tokens_sum(&again)
        );

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        write!(f, "\n{third_line}\n").unwrap();
        drop(f);
        let second = file_days(&path, &mut |line, data| kimi_line(line, data));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            (tokens_sum(&second) - 7_000.0).abs() < 0.001,
            "append after a no-newline finale must not re-count the last record, got {}",
            tokens_sum(&second)
        );
    }

    #[test]
    fn claude_checkpoint_drops_a_replay_older_than_warmup() {
        let dir = std::env::temp_dir().join(format!("pane-claude-ckpt-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("session.jsonl");
        let line = |mid: &str, rid: &str, tokens: f64| {
            json!({
                "type": "assistant",
                "timestamp": "2026-07-10T10:00:00Z",
                "requestId": rid,
                "message": {
                    "id": mid,
                    "model": "claude-haiku-4-5",
                    "usage": {"input_tokens": tokens, "output_tokens": 0.0}
                }
            })
            .to_string()
        };
        fs::write(&path, format!("{}\n", line("msg_1", "req_1", 100.0))).unwrap();
        let first = claude_file(&path);
        assert!((tokens_sum(&first) - 100.0).abs() < 0.001);
        assert!(
            cache()
                .lock()
                .unwrap()
                .get(&path)
                .and_then(|e| e.claude.as_ref())
                .is_some_and(|s| s.seen.contains("msg_1:req_1")),
            "claude checkpoint must persist the counted id"
        );

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        writeln!(f, "{}", line("msg_1", "req_1", 100.0)).unwrap();
        drop(f);
        let second = claude_file(&path);
        let _ = fs::remove_dir_all(&dir);
        assert!(
            (tokens_sum(&second) - 100.0).abs() < 0.001,
            "replay of a checkpointed id must not count twice, got {}",
            tokens_sum(&second)
        );
    }

    #[test]
    fn file_days_aligns_a_legacy_midline_offset() {
        let dir = std::env::temp_dir().join(format!("pane-jsonl-legacy-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("legacy.jsonl");
        let line = |tokens: f64| {
            json!({
                "type": "usage.record",
                "model": "kimi-code/k3",
                "usage": {"inputOther": tokens, "output": 0.0},
                "usageScope": "turn",
                "time": 1_784_208_630_652i64
            })
            .to_string()
        };
        let first_line = line(1_000.0);
        let second_line = line(4_000.0);
        fs::write(&path, format!("{first_line}\n{}", &second_line[..12])).unwrap();
        let first = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!((tokens_sum(&first) - 1_000.0).abs() < 0.001);

        // Old v4 cache stored the raw EOF (mid-line) and had no fingerprint.
        let incomplete_len = fs::metadata(&path).unwrap().len();
        if let Ok(mut map) = cache().lock() {
            if let Some(e) = map.get_mut(&path) {
                e.size = incomplete_len;
                e.prefix_head.clear();
                e.prefix_tail.clear();
            }
        }

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        writeln!(f, "{}", &second_line[12..]).unwrap();
        drop(f);

        let second = file_days(&path, &mut |line, data| kimi_line(line, data));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            (tokens_sum(&second) - 5_000.0).abs() < 0.001,
            "legacy mid-line offset must back up to the previous newline, got {}",
            tokens_sum(&second)
        );
    }

    #[test]
    fn file_days_full_parses_a_larger_rewrite() {
        let dir = std::env::temp_dir().join(format!("pane-jsonl-rewrite-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("rewrite.jsonl");
        let line = |tokens: f64, time: i64| {
            json!({
                "type": "usage.record",
                "model": "kimi-code/k3",
                "usage": {"inputOther": tokens, "output": 0.0},
                "usageScope": "turn",
                "time": time
            })
            .to_string()
        };
        fs::write(&path, format!("{}\n", line(1_000.0, 1_784_208_630_652))).unwrap();
        let first = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!((tokens_sum(&first) - 1_000.0).abs() < 0.001);
        assert!(
            cache()
                .lock()
                .unwrap()
                .get(&path)
                .is_some_and(|e| !e.prefix_head.is_empty()),
            "prefix fingerprint must be stored"
        );

        // Same-length first line, different clock — the start fingerprint
        // includes the timestamp, so this is not treated as an append.
        fs::write(
            &path,
            format!(
                "{}\n{}\n",
                line(2_000.0, 1_784_208_999_999),
                line(3_000.0, 1_784_208_999_999)
            ),
        )
        .unwrap();
        let second = file_days(&path, &mut |line, data| kimi_line(line, data));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            (tokens_sum(&second) - 5_000.0).abs() < 0.001,
            "larger rewrite must full-parse, got {}",
            tokens_sum(&second)
        );
    }

    #[test]
    fn empty_prefix_fingerprint_still_tails() {
        let dir = std::env::temp_dir().join(format!("pane-jsonl-oldfp-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("grow.jsonl");
        let line = |tokens: f64| {
            json!({
                "type": "usage.record",
                "model": "kimi-code/k3",
                "usage": {"inputOther": tokens, "output": 0.0},
                "usageScope": "turn",
                "time": 1_784_208_630_652i64
            })
            .to_string()
        };
        fs::write(&path, format!("{}\n", line(1_000.0))).unwrap();
        let first = file_days(&path, &mut |line, data| kimi_line(line, data));
        assert!((tokens_sum(&first) - 1_000.0).abs() < 0.001);
        if let Ok(mut map) = cache().lock() {
            if let Some(e) = map.get_mut(&path) {
                e.prefix_head.clear();
                e.prefix_tail.clear();
            }
        }

        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        writeln!(f, "{}", line(4_000.0)).unwrap();
        drop(f);

        let second = file_days(&path, &mut |line, data| kimi_line(line, data));
        let _ = fs::remove_dir_all(&dir);
        assert!(
            (tokens_sum(&second) - 5_000.0).abs() < 0.001,
            "older cache with empty fingerprint must still tail, got {}",
            tokens_sum(&second)
        );
    }

    // ---- Input bounds: oversize lines, huge files, hostile model names ---

    /// A line past MAX_LINE_BYTES is skipped without ever being stored;
    /// the lines around it still parse and the file still caches (a
    /// deliberate skip is not a read failure).
    #[test]
    fn oversize_lines_are_skipped_without_storing() {
        let dir = std::env::temp_dir().join(format!("pane-bigline-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("big-line.jsonl");
        let ok = json!({"type": "usage.record", "model": "kimi-code/k3",
            "usage": {"inputOther": 1000.0, "output": 1000.0},
            "usageScope": "turn", "time": 1784208630652i64})
        .to_string();
        let huge = "x".repeat(MAX_LINE_BYTES + 1024);
        fs::write(&path, format!("{ok}\n{huge}\n{ok}\n")).unwrap();

        let mut seen: Vec<usize> = Vec::new();
        let data = file_days(&path, &mut |line, data| {
            seen.push(line.len());
            kimi_line(line, data);
        });
        let cached = cache().lock().unwrap().contains_key(&path);
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(seen, vec![ok.len(), ok.len()], "overlong line reached the parser");
        assert_eq!(tokens_sum(&data), 4_000.0);
        assert!(cached, "a skipped line must not poison the cache entry");
    }

    /// Files past MAX_LOG_FILE_BYTES are skipped: the walk won't list them,
    /// and a direct-path caller gets nothing (and no cache entry).
    #[test]
    fn huge_log_files_are_skipped() {
        let dir = std::env::temp_dir().join(format!("pane-hugefile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("small.jsonl"), "{}\n").unwrap();
        let huge_path = dir.join("huge.jsonl");
        let huge = fs::File::create(&huge_path).unwrap();
        huge.set_len(MAX_LOG_FILE_BYTES + 1).unwrap();
        drop(huge);

        let mut out = Vec::new();
        recent_jsonl_files(&dir, &mut out);
        assert!(out.iter().any(|p| p.ends_with("small.jsonl")));
        assert!(!out.iter().any(|p| p.ends_with("huge.jsonl")), "oversize log must not be listed");

        let mut parsed = false;
        let data = file_days(&huge_path, &mut |_, _| parsed = true);
        let cached = cache().lock().unwrap().contains_key(&huge_path);
        let _ = fs::remove_dir_all(&dir);
        assert!(!parsed && data.days.is_empty());
        assert!(!cached, "oversize log must not become a cache entry");
    }

    /// A model string longer than MAX_MODEL_KEY can't key the maps (or the
    /// persisted cache): it folds into the fixed overflow bucket while its
    /// dollars and tokens still count.
    #[test]
    fn huge_model_names_fold_into_the_overflow_bucket() {
        let ts = DateTime::from_timestamp_millis(1_784_208_630_652).unwrap();
        let mut data = FileData::default();
        let huge_a = format!("a{}", "x".repeat(10_000));
        let huge_b = format!("b{}", "y".repeat(10_000));
        add_event(&mut data, ts, &huge_a, 1.0, 100.0);
        add_event(&mut data, ts, &huge_b, 2.0, 200.0);
        note_unpriced(&mut data, ts, &huge_a, 50.0);

        // Totals stay exact; only the attribution merged.
        assert_eq!(cost_sum(&data), 3.0);
        assert_eq!(tokens_sum(&data), 350.0);
        assert_eq!(data.days.len(), 1);
        assert!(data.days.keys().all(|(_, m)| m == OVERFLOW_MODEL_KEY));
        assert_eq!(data.unpriced.get(OVERFLOW_MODEL_KEY), Some(&1));
        assert!(data.unpriced.keys().all(|m| m.len() <= MAX_MODEL_KEY));

        // Boundary: 128 chars is admitted, 129 folds.
        let mut b = FileData::default();
        let at_cap = "m".repeat(MAX_MODEL_KEY);
        add_event(&mut b, ts, &at_cap, 1.0, 1.0);
        assert!(b.days.keys().all(|(_, m)| m == &at_cap));
        add_event(&mut b, ts, &"m".repeat(MAX_MODEL_KEY + 1), 1.0, 1.0);
        assert!(b.days.keys().any(|(_, m)| m == OVERFLOW_MODEL_KEY));
    }

    /// One file naming endless distinct models folds everything past
    /// MAX_MODELS_PER_FILE into the overflow bucket.
    #[test]
    fn unique_model_keys_cap_per_file() {
        let ts = DateTime::from_timestamp_millis(1_784_208_630_652).unwrap();
        let mut data = FileData::default();
        for i in 0..MAX_MODELS_PER_FILE + 5 {
            add_event(&mut data, ts, &format!("model-{i}"), 1.0, 1.0);
        }
        // 4096 admitted names plus the single overflow bucket.
        assert_eq!(data.days.len(), MAX_MODELS_PER_FILE + 1);
        let overflow = data
            .days
            .get(&(day_of_utc(ts), OVERFLOW_MODEL_KEY.to_string()))
            .expect("overflow bucket");
        assert_eq!(*overflow, (5.0, 5.0));
        assert_eq!(cost_sum(&data), (MAX_MODELS_PER_FILE + 5) as f64);
    }

    #[test]
    fn corrupt_persist_file_is_rejected_not_panicked() {
        assert!(serde_json::from_str::<PersistFile>("{not json").is_err());
        assert!(serde_json::from_str::<PersistFile>(r#"{"version":1}"#).is_err());
    }

    // ---- Hermes: billing-route buckets -----------------------------------

    #[test]
    fn hermes_routes_land_in_the_right_slice() {
        assert_eq!(hermes_bucket("minimax-oauth", "").0, "minimax");
        assert_eq!(hermes_bucket("MiniMax", "").0, "minimax");
        assert_eq!(hermes_bucket("openrouter", "").0, "openrouter");
        assert_eq!(hermes_bucket("nous-api", "").0, "hermes");
        assert_eq!(hermes_bucket("aihubmix", "").0, "hermes");
        assert_eq!(hermes_bucket("custom", "").0, "hermes");
        assert_eq!(hermes_bucket("custom", "https://aihubmix.com/v1").0, "hermes");
        assert_eq!(
            hermes_bucket("custom", "https://api.minimax.io/v1").0,
            "minimax"
        );
        assert_eq!(hermes_bucket("", "").0, "hermes");
    }

    // ---- Codex: child-session replay gate --------------------------------

    #[test]
    fn codex_child_meta_rules() {
        // JSON null / blank strings are absent — a root session declaring
        // `forked_from_id: null` is not a child.
        assert!(!codex_child_meta(&json!({"forked_from_id": null, "parent_thread_id": null})));
        assert!(!codex_child_meta(&json!({"forked_from_id": "  "})));
        assert!(!codex_child_meta(&json!({"session_id": "root"})));
        assert!(codex_child_meta(&json!({"forked_from_id": "abc"})));
        assert!(codex_child_meta(&json!({"parent_thread_id": "abc"})));
        assert!(codex_child_meta(&json!({"thread_source": "subagent"})));
        assert!(codex_child_meta(&json!({"source": {"subagent": {"thread_spawn": {}}}})));
        assert!(!codex_child_meta(&json!({"source": {"subagent": null}})));
    }

    fn codex_run(lines: &[String]) -> FileData {
        let mut st = CodexFileState::default();
        let mut data = FileData::default();
        for line in lines {
            codex_line(&mut st, line, &mut data);
        }
        data
    }

    fn token_count_line(ts: &str, last: Option<(f64, f64)>, total: (f64, f64)) -> String {
        let mut info = json!({
            "total_token_usage": {"input_tokens": total.0, "output_tokens": total.1,
                                  "total_tokens": total.0 + total.1}
        });
        if let Some((i, o)) = last {
            info["last_token_usage"] =
                json!({"input_tokens": i, "output_tokens": o, "total_tokens": i + o});
        }
        json!({"timestamp": ts, "type": "event_msg",
               "payload": {"type": "token_count", "info": info}})
        .to_string()
    }

    #[test]
    fn codex_replay_gate_skips_child_history() {
        let spawn_epoch = chrono::DateTime::parse_from_rfc3339("2026-07-10T10:00:00Z")
            .unwrap()
            .timestamp();
        let lines = vec![
            // The child's own session_meta, then the replayed parent history:
            // token_counts with rewritten (fresh) timestamps and a replayed
            // task_started still carrying the parent's old started_at.
            json!({"timestamp": "2026-07-10T10:00:00Z", "type": "session_meta",
                   "payload": {"parent_thread_id": "abc", "thread_source": "subagent"}})
            .to_string(),
            json!({"timestamp": "2026-07-10T10:00:00Z", "type": "turn_context",
                   "payload": {"model": "gpt-5.6-terra"}})
            .to_string(),
            token_count_line("2026-07-10T10:00:01Z", Some((50_000.0, 5_000.0)), (50_000.0, 5_000.0)),
            json!({"timestamp": "2026-07-10T10:00:02Z", "type": "event_msg",
                   "payload": {"type": "task_started", "started_at": spawn_epoch - 3600}})
            .to_string(),
            token_count_line("2026-07-10T10:00:03Z", Some((30_000.0, 3_000.0)), (80_000.0, 8_000.0)),
            // First live turn: started_at at/after the child's creation.
            json!({"timestamp": "2026-07-10T10:00:05Z", "type": "event_msg",
                   "payload": {"type": "task_started", "started_at": spawn_epoch + 5}})
            .to_string(),
            token_count_line("2026-07-10T10:00:09Z", Some((1_000.0, 100.0)), (81_000.0, 8_100.0)),
        ];
        let data = codex_run(&lines);
        // Only the live turn counts — 88k replayed tokens stay out.
        assert_eq!(tokens_sum(&data), 1_100.0);
        assert!(data.unpriced.is_empty());
    }

    #[test]
    fn codex_root_session_with_null_parent_counts_normally() {
        let lines = vec![
            json!({"timestamp": "2026-07-10T10:00:00Z", "type": "session_meta",
                   "payload": {"forked_from_id": null, "parent_thread_id": null}})
            .to_string(),
            json!({"timestamp": "2026-07-10T10:00:00Z", "type": "turn_context",
                   "payload": {"model": "gpt-5.6-terra"}})
            .to_string(),
            token_count_line("2026-07-10T10:00:01Z", Some((1_000.0, 100.0)), (1_000.0, 100.0)),
        ];
        assert_eq!(tokens_sum(&codex_run(&lines)), 1_100.0);
    }

    #[test]
    fn auto_review_fallback_follows_ccusage_timeline() {
        assert_eq!(auto_review_fallback_date("2026-08-13"), "gpt-5.5");
        assert_eq!(auto_review_fallback_date("2026-04-23"), "gpt-5.5");
        assert_eq!(auto_review_fallback_date("2026-04-22"), "gpt-5.4");
        assert_eq!(auto_review_fallback_date("2025-08-01"), "gpt-5");
        assert_eq!(auto_review_fallback_date("nope"), "gpt-5");
    }

    #[test]
    fn codex_auto_review_keeps_its_name_in_the_breakdown() {
        let lines = vec![
            json!({"timestamp": "2026-08-13T10:00:00Z", "type": "turn_context",
                   "payload": {"model": "codex-auto-review"}})
            .to_string(),
            token_count_line("2026-08-13T10:00:01Z", Some((1_000.0, 100.0)), (1_000.0, 100.0)),
        ];
        let data = codex_run(&lines);
        let models: Vec<&str> = data.days.keys().map(|(_, m)| m.as_str()).collect();
        assert_eq!(models, vec!["codex-auto-review"]);
        assert!(cost_sum(&data) > 0.0);
        assert!(data.unpriced.is_empty());
    }

    #[test]
    fn codex_stale_snapshot_reemission_skipped() {
        let lines = vec![
            json!({"timestamp": "2026-07-10T10:00:00Z", "type": "turn_context",
                   "payload": {"model": "gpt-5.6-terra"}})
            .to_string(),
            token_count_line("2026-07-10T10:00:01Z", Some((1_000.0, 100.0)), (1_000.0, 100.0)),
            // Same cumulative totals re-emitted (Codex does this) — not new
            // usage even though it repeats a last_token_usage.
            token_count_line("2026-07-10T10:00:02Z", Some((1_000.0, 100.0)), (1_000.0, 100.0)),
        ];
        assert_eq!(tokens_sum(&codex_run(&lines)), 1_100.0);
    }

    #[test]
    fn codex_totals_delta_when_last_usage_absent() {
        let lines = vec![
            json!({"timestamp": "2026-07-10T10:00:00Z", "type": "turn_context",
                   "payload": {"model": "gpt-5.6-terra"}})
            .to_string(),
            token_count_line("2026-07-10T10:00:01Z", None, (1_000.0, 100.0)),
            token_count_line("2026-07-10T10:00:02Z", None, (3_000.0, 300.0)),
        ];
        // 1100 from the first cumulative snapshot, 2200 recovered as a delta.
        assert_eq!(tokens_sum(&codex_run(&lines)), 3_300.0);
    }

    #[test]
    fn codex_fast_tier_applies_provider_multiplier() {
        let turn = json!({"timestamp": "2026-07-10T10:00:00Z", "type": "turn_context",
                          "payload": {"model": "gpt-5.6-terra"}})
        .to_string();
        let usage = token_count_line("2026-07-10T10:00:01Z", Some((1_000.0, 100.0)), (1_000.0, 100.0));
        let standard = codex_run(&[turn.clone(), usage.clone()]);
        let fast = codex_run(&[
            turn,
            json!({"timestamp": "2026-07-10T10:00:00Z", "type": "event_msg",
                   "payload": {"type": "thread_settings_applied",
                               "thread_settings": {"service_tier": "fast"}}})
            .to_string(),
            usage,
        ]);
        // gpt-5.6-terra's Codex priority multiplier is exactly 2x, whatever
        // catalog resolved the base rates.
        assert!(cost_sum(&standard) > 0.0);
        assert!((cost_sum(&fast) / cost_sum(&standard) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn codex_fast_tier_does_not_double_kimi_oauth() {
        let turn = json!({"timestamp": "2026-07-10T10:00:00Z", "type": "turn_context",
                          "payload": {"model": "kimi-oauth/k3"}})
        .to_string();
        let usage = token_count_line("2026-07-10T10:00:01Z", Some((1_000.0, 100.0)), (1_000.0, 100.0));
        let standard = codex_run(&[turn.clone(), usage.clone()]);
        let fast = codex_run(&[
            turn,
            json!({"timestamp": "2026-07-10T10:00:00Z", "type": "event_msg",
                   "payload": {"type": "thread_settings_applied",
                               "thread_settings": {"service_tier": "fast"}}})
            .to_string(),
            usage,
        ]);
        assert!(cost_sum(&standard) > 0.0);
        assert!((cost_sum(&fast) - cost_sum(&standard)).abs() < 1e-9);
    }

    #[test]
    fn codex_dated_base_strips_snapshot_stamps() {
        assert_eq!(codex_dated_base("gpt-5.6-sol-2026-06-01"), "gpt-5.6-sol");
        assert_eq!(codex_dated_base("gpt-5.6-sol-20260601"), "gpt-5.6-sol");
        assert_eq!(codex_dated_base("gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(codex_dated_base("gpt-4-0125-preview"), "gpt-4-0125-preview");
        assert_eq!(codex_dated_base("gpt-6-astra-2026-09-01"), "gpt-6-astra");
        assert_eq!(codex_long_context("gpt-6-astra"), Some((20.0, 75.0, 2.0)));
        assert_eq!(codex_priority_multiplier("gpt-6-astra", "gpt-6-astra"), 2.0);
    }

    fn dated_astra_session(model: &str, input: f64, output: f64, fast: bool) -> FileData {
        let mut lines = vec![
            json!({"timestamp": "2026-09-01T10:00:00Z", "type": "turn_context",
                   "payload": {"model": model}})
            .to_string(),
        ];
        if fast {
            lines.push(
                json!({"timestamp": "2026-09-01T10:00:00Z", "type": "event_msg",
                       "payload": {"type": "thread_settings_applied",
                                   "thread_settings": {"service_tier": "fast"}}})
                .to_string(),
            );
        }
        lines.push(token_count_line(
            "2026-09-01T10:00:01Z",
            Some((input, output)),
            (input, output),
        ));
        codex_run(&lines)
    }

    /// Date-stamped Astra must use the baked $10/$50 card (and $20/$75
    /// above 272k), not generic GPT-5 rates. Fast is 2×. The breakdown
    /// keeps the dated name.
    #[test]
    fn dated_astra_uses_builtin_rates_below_and_above_272k() {
        let low_in = 1_000.0;
        let high_in = 273_000.0;
        let out = 1_000.0;
        let expect_low = (low_in * 10.0 + out * 50.0) / 1e6;
        let expect_high = (high_in * 20.0 + out * 75.0) / 1e6;
        let generic_low = (low_in * 1.25 + out * 10.0) / 1e6;

        for model in ["gpt-6-astra-2026-09-01", "gpt-6-astra-20260901"] {
            let standard = dated_astra_session(model, low_in, out, false);
            assert!(standard.unpriced.is_empty(), "{model}: {:?}", standard.unpriced);
            assert!(
                standard.days.keys().all(|(_, m)| m == model),
                "{model} breakdown renamed: {:?}",
                standard.days.keys().collect::<Vec<_>>()
            );
            let got = cost_sum(&standard);
            assert!(
                (got - expect_low).abs() < 1e-9,
                "{model} low cost {got}, want {expect_low} (generic GPT would be {generic_low})"
            );

            let fast = dated_astra_session(model, low_in, out, true);
            assert!((cost_sum(&fast) / got - 2.0).abs() < 1e-9, "{model} fast low");

            let high = dated_astra_session(model, high_in, out, false);
            let got_high = cost_sum(&high);
            assert!(
                (got_high - expect_high).abs() < 1e-9,
                "{model} high cost {got_high}, want {expect_high}"
            );
            let fast_high = dated_astra_session(model, high_in, out, true);
            assert!(
                (cost_sum(&fast_high) / got_high - 2.0).abs() < 1e-9,
                "{model} fast high"
            );
        }
    }

    // ---- Claude: advisor iterations, sidechain dedup, synthetic ----------

    fn claude_run(lines: &[String]) -> FileData {
        let mut st = ClaudeFileState::default();
        let mut data = FileData::default();
        for line in lines {
            claude_line(&mut st, line, &mut data);
        }
        data
    }

    // ---- Claude: work areas ---------------------------------------------

    #[test]
    fn area_is_the_first_folder_under_the_session_root() {
        let root = "/Users/me/work";
        assert_eq!(area_under("/Users/me/work/acme/a.rs", root, false).as_deref(), Some("acme"));
        assert_eq!(area_under("/Users/me/work/acme/src/a.rs", root, false).as_deref(), Some("acme/src"));
        assert_eq!(
            area_under("/Users/me/work/acme/src/deep/er/a.rs", root, false).as_deref(),
            Some("acme/src"),
            "two levels, no more"
        );
        assert_eq!(area_under("/Users/me/work/acme", root, true).as_deref(), Some("acme"), "a cwd is a folder");
        assert_eq!(area_top("acme/src"), "acme");
        assert_eq!(area_under("/Users/me/work/notes.md", root, false), None, "a file in the root names no area");
        assert_eq!(area_under("/Users/me/work", root, true), None);
        assert_eq!(area_under("/Users/me/workshop/x.rs", root, false), None, "prefix must end at a separator");
        assert_eq!(area_under("/tmp/scratch/x.rs", root, false), None);
        // Windows separators are normalised; the area keeps its spelling.
        assert_eq!(area_under(r"C:\work\Acme\a.rs", r"C:\work", false).as_deref(), Some("Acme"));
        assert_eq!(area_under(r"C:\work\Acme\web\a.rs", r"C:\work", false).as_deref(), Some("Acme/web"));
    }

    #[test]
    fn bash_commands_yield_the_first_path_under_the_root() {
        let root = "/Users/me/work";
        assert_eq!(
            area_in_command("cd /Users/me/work/acme && cargo test 2>&1 | tail -3", root, None).as_deref(),
            Some("acme")
        );
        assert_eq!(
            area_in_command("ls /tmp/x; cat ~/work/beta/README.md", root, Some("/Users/me")).as_deref(),
            Some("beta"),
            "~ expands to the home directory, and README.md is a file, not a folder"
        );
        assert_eq!(
            area_in_command("ls /Users/me/work/beta/docs", root, None).as_deref(),
            Some("beta/docs")
        );
        assert_eq!(area_in_command("git status", root, None), None);
    }

    fn area_line(mid: &str, cwd: &str, cost: f64, tool: Option<(&str, &str, &str)>) -> String {
        let content = match tool {
            Some((name, key, value)) => json!([{"type": "tool_use", "name": name, "input": {key: value}}]),
            None => json!([{"type": "text", "text": "ok"}]),
        };
        json!({
            "type": "assistant", "timestamp": "2026-09-20T12:00:00Z", "cwd": cwd, "requestId": format!("r-{mid}"),
            "costUSD": cost,
            "message": {"id": mid, "model": "claude-opus-5", "content": content,
                        "usage": {"input_tokens": 10, "output_tokens": 5}}
        })
        .to_string()
    }

    fn area_costs(data: &FileData) -> Vec<(String, f64)> {
        let mut out: HashMap<String, f64> = HashMap::new();
        for ((_, area), (cost, _)) in &data.areas {
            *out.entry(area.clone()).or_insert(0.0) += cost;
        }
        let mut out: Vec<_> = out.into_iter().collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn spend_follows_the_working_directory_then_the_files_touched() {
        let root = "/Users/me/work";
        let data = claude_run(&[
            area_line("m1", root, 1.0, None),                                     // nothing known yet
            area_line("m2", "/Users/me/work/acme", 2.0, None),                // cwd moved into acme
            area_line("m3", root, 4.0, None),                                     // back at the root: sticky
            // Same message as m3 (a later content block): not counted again,
            // but the file it touches moves the session to beta.
            area_line("m3", root, 4.0, Some(("Read", "file_path", "/Users/me/work/beta/x.md"))),
            area_line("m4", root, 8.0, None),
            // Cost and tool call on the SAME line: the message that reaches
            // into gamma is gamma work, so its own dollars go there too.
            area_line("m5", root, 16.0, Some(("Bash", "command", "cd /Users/me/work/gamma && ls"))),
            area_line("m6", root, 32.0, None),
        ]);
        assert_eq!(
            area_costs(&data),
            [
                ("(unsorted)".to_string(), 1.0),
                ("acme".to_string(), 6.0), // m2 by cwd, m3 sticky (its tool call came on a later line)
                ("beta".to_string(), 8.0), // m4, after m3's later block touched beta
                ("gamma".to_string(), 48.0), // m5 and m6
            ]
        );
        // Areas are a second view of the same dollars, never extra ones.
        let total: f64 = data.days.values().map(|(c, _)| c).sum();
        let by_area: f64 = data.areas.values().map(|(c, _)| c).sum();
        assert_eq!(total, 63.0);
        assert_eq!(by_area, total);
    }

    #[test]
    fn a_session_reports_its_span_cost_top_model_and_areas() {
        let mut st = ClaudeFileState::default();
        let mut data = FileData::default();
        let line = |mid: &str, ts: &str, cwd: &str, model: &str, cost: f64| {
            json!({
                "type": "assistant", "timestamp": ts, "cwd": cwd, "requestId": format!("r-{mid}"), "costUSD": cost,
                "message": {"id": mid, "model": model, "content": [{"type": "text", "text": "ok"}],
                            "usage": {"input_tokens": 10, "output_tokens": 5}}
            })
            .to_string()
        };
        for l in [
            line("a", "2026-09-20T10:00:00Z", "/w", "claude-sonnet-5", 1.0),
            line("b", "2026-09-20T10:30:00Z", "/w/acme", "claude-opus-5", 6.0),
            line("c", "2026-09-20T12:15:00Z", "/w/beta", "claude-opus-5", 2.0),
        ] {
            claude_line(&mut st, &l, &mut data);
        }
        let today = day_of_utc(parse_ts(Some(&json!("2026-09-21T00:00:00Z"))).unwrap());
        let s = session_from("sess-1", "/w", &data, Some(&st), today, 4096).unwrap();
        assert_eq!(s.cost, 9.0);
        assert_eq!(s.bytes, 4096, "the log's size rides along so a stale, still-open session shows its weight");
        assert_eq!(s.top_model.as_deref(), Some("claude-opus-5"));
        assert_eq!(s.ended_ms.unwrap() - s.started_ms.unwrap(), 135 * 60_000, "2h15m from first to last message");
        assert_eq!(s.areas, [("acme".to_string(), 6.0), ("beta".to_string(), 2.0), ("(unsorted)".to_string(), 1.0)]);
        // Outside the window there is nothing to report.
        assert_eq!(session_from("old", "/w", &data, Some(&st), today + 60, 1), None);
        // No checkpoint (a non-Claude or pre-upgrade entry) still reports, without a span.
        assert_eq!(session_from("x", "/w", &data, None, today, 1).unwrap().started_ms, None);
    }

    #[test]
    fn a_session_id_finds_its_own_log_and_nothing_else() {
        let root = PathBuf::from("/h/.claude/projects");
        let paths = [
            root.join("-w-acme").join("abc").with_extension("jsonl"),
            root.join("-w-acme").join("def").with_extension("jsonl"),
            // Same stem, other tools' logs: the cache has these too.
            PathBuf::from("/h/.codex/sessions/def.jsonl"),
            PathBuf::from("/h/.grok/def.jsonl"),
            // A stray file under the root that is not a session log.
            root.join("-w-acme").join("def").with_extension("txt"),
        ];
        let find = |id: &str| session_path_among(&root, paths.iter(), id);
        assert_eq!(find("def"), Some(root.join("-w-acme").join("def.jsonl")));
        assert_eq!(find("abc"), Some(root.join("-w-acme").join("abc.jsonl")));
        assert_eq!(find("ghi"), None, "unknown id");
        assert_eq!(find("../def"), None, "a stem never contains a separator, so a path cannot pass as an id");
        assert_eq!(find(""), None);
        // Nothing outside the projects root can be named, whatever the stem.
        let outside = [PathBuf::from("/h/.codex/sessions/def.jsonl")];
        assert_eq!(session_path_among(&root, outside.iter(), "def"), None);
    }

    // ---- Subagent transcripts: a fixture tree shared by several tests ----

    const PLANTED_PROMPT: &str = "PLANTED_PROMPT_SUBAGENT_FIXTURE";
    const PLANTED_TITLE: &str = "PLANTED_TITLE_SUBAGENT_FIXTURE";
    const PLANTED_TEXT: &str = "PLANTED_TEXT_SUBAGENT_FIXTURE";

    /// One assistant line shaped like a real Claude Code log entry, for the
    /// shared subagent fixture below.
    fn subagent_line(
        mid: &str,
        rid: &str,
        session_id: &str,
        cost: f64,
        sidechain: bool,
        agent: Option<&str>,
        text: &str,
    ) -> String {
        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), json!("assistant"));
        obj.insert("timestamp".into(), json!((Utc::now() - chrono::Duration::hours(1)).to_rfc3339()));
        obj.insert("cwd".into(), json!("/w"));
        obj.insert("requestId".into(), json!(rid));
        obj.insert("sessionId".into(), json!(session_id));
        obj.insert("costUSD".into(), json!(cost));
        if sidechain {
            obj.insert("isSidechain".into(), json!(true));
        }
        if let Some(a) = agent {
            obj.insert("attributionAgent".into(), json!(a));
        }
        obj.insert(
            "message".into(),
            json!({
                "id": mid, "model": "claude-haiku-4-5",
                "content": [{"type": "text", "text": text}],
                "usage": {"input_tokens": 10.0, "output_tokens": 5.0},
            }),
        );
        Value::Object(obj).to_string()
    }

    /// Serializes every test that still injects a fixture into the real,
    /// shared `cache()` (via `build_subagent_fixture` / `inject_scanned`).
    /// `agent_spend`'s own grouping is now tested with in-memory
    /// `PersistEntry` fixtures and no longer touches this cache at all, but
    /// `claude_sessions` still reads it directly, so the tests that check
    /// `claude_sessions` (fold, privacy, the outside-the-window case, token
    /// totals) still build their fixture on disk and inject it. cargo
    /// test's default parallelism would otherwise let two of those construct
    /// or tear down a fixture at once and trip over each other's -- the
    /// general "tests share process-wide state" hazard, not the original
    /// same-`attributionAgent`-name collision this lock was first written
    /// to guard `agent_spend`'s grouping against.
    fn subagent_fixture_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct SubagentFixture {
        root: PathBuf,
        project: String,
        uuid: String,
        fake_paths: Vec<PathBuf>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    /// Cleans up the cache entries and temp directory a subagent fixture
    /// created, even if the test using it panics on an assertion — a failed
    /// test must not leak fixtures that later tests could trip over.
    impl Drop for SubagentFixture {
        fn drop(&mut self) {
            if let Ok(mut map) = cache().lock() {
                for p in &self.fake_paths {
                    map.remove(p);
                }
            }
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// Runs the real scanner (`claude_file`) on a real file wherever it
    /// happens to live, then moves the result into the shared cache under a
    /// synthetic path shaped like the real Claude projects root — so the
    /// public, cache-reading `claude_sessions` / `agent_spend` exercise real
    /// parser output end to end, without this process's own
    /// `~/.claude/projects` ever being written to.
    fn inject_scanned(path: &Path, fake_path: &Path) {
        let data = claude_file(path);
        let claude_ckpt = cache().lock().ok().and_then(|mut m| m.remove(path)).and_then(|e| e.claude);
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let entry = FileEntry {
            mtime: SystemTime::now(),
            size,
            gen: pricing::generation(),
            probes: Vec::new(),
            data,
            prefix_head: Vec::new(),
            prefix_tail: Vec::new(),
            grok_models: HashMap::new(),
            codex: None,
            claude: claude_ckpt,
            pi_seen: HashSet::new(),
        };
        if let Ok(mut map) = cache().lock() {
            map.insert(fake_path.to_path_buf(), entry);
        }
    }

    /// Builds `<root>/<project>/<uuid>.jsonl` (a normal parent session:
    /// 1.0 + 2.0 = 3.0 of its own) plus `<uuid>/subagents/a.jsonl`
    /// (`attributionAgent` "Explore": 0.5 + 0.3 = 0.8, with message "a1"
    /// replayed once — deduped like any other Claude log) and `b.jsonl` (no
    /// `attributionAgent`: 0.2). All three plant a prompt, a custom title
    /// and an assistant text block that must never surface downstream.
    /// Scans them for real and stashes the results in the shared cache
    /// under the real projects root's shape (see `inject_scanned`).
    fn build_subagent_fixture(name: &str) -> SubagentFixture {
        let lock = subagent_fixture_lock().lock().unwrap_or_else(|e| e.into_inner());
        let (root, project, uuid, fake_paths) = build_subagent_fixture_unlocked(name);
        SubagentFixture { root, project, uuid, fake_paths, _lock: lock }
    }

    /// The fixture-building body, without acquiring `subagent_fixture_lock`
    /// itself — for a test that must hold the lock across a "before" read
    /// too (real "Explore" / unattributed activity on this machine must not
    /// change between that read and the fixture existing).
    fn build_subagent_fixture_unlocked(name: &str) -> (PathBuf, String, String, Vec<PathBuf>) {
        let root = std::env::temp_dir().join(format!("pane-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let project = format!("proj-{name}");
        let uuid = format!("uuid-{name}");
        let sub_dir = root.join(&project).join(&uuid).join("subagents");
        fs::create_dir_all(&sub_dir).unwrap();

        let parent_path = root.join(&project).join(format!("{uuid}.jsonl"));
        fs::write(
            &parent_path,
            format!(
                "{}\n{}\n{}\n",
                json!({"type": "user", "sessionId": uuid, "message": {"role": "user", "content": PLANTED_PROMPT}}),
                subagent_line("p1", "r-p1", &uuid, 1.0, false, None, "ok"),
                subagent_line("p2", "r-p2", &uuid, 2.0, false, None, "ok"),
            ),
        )
        .unwrap();

        let a_path = sub_dir.join("a.jsonl");
        fs::write(
            &a_path,
            format!(
                "{}\n{}\n{}\n{}\n",
                json!({"type": "custom-title", "sessionId": uuid, "isSidechain": true, "customTitle": PLANTED_TITLE}),
                subagent_line("a1", "r-a1", &uuid, 0.5, true, Some("Explore"), PLANTED_TEXT),
                subagent_line("a1", "r-a1", &uuid, 0.5, true, Some("Explore"), PLANTED_TEXT), // duplicated message id
                subagent_line("a2", "r-a2", &uuid, 0.3, true, Some("Explore"), "ok"),
            ),
        )
        .unwrap();

        let b_path = sub_dir.join("b.jsonl");
        fs::write(&b_path, format!("{}\n", subagent_line("b1", "r-b1", &uuid, 0.2, true, None, "ok"))).unwrap();

        let real_root = claude_projects_root();
        let fake_parent = real_root.join(&project).join(format!("{uuid}.jsonl"));
        let fake_a = real_root.join(&project).join(&uuid).join("subagents").join("a.jsonl");
        let fake_b = real_root.join(&project).join(&uuid).join("subagents").join("b.jsonl");
        inject_scanned(&parent_path, &fake_parent);
        inject_scanned(&a_path, &fake_a);
        inject_scanned(&b_path, &fake_b);

        (root, project, uuid, vec![fake_parent, fake_a, fake_b])
    }

    #[test]
    fn sessions_list_no_sidechain_and_fold_its_cost_into_the_parent() {
        let fx = build_subagent_fixture("sessions-fold");
        let sessions = claude_sessions(None, None, 10_000);
        assert!(
            sessions.iter().all(|s| s.id != "a" && s.id != "b"),
            "subagent files must never appear as their own session"
        );
        let parent = sessions.iter().find(|s| s.id == fx.uuid).expect("the parent session is listed");
        assert!((parent.cost - 4.0).abs() < 1e-9, "3.0 of its own + 1.0 folded from its subagents, got {}", parent.cost);
        assert!((parent.subagent_cost - 1.0).abs() < 1e-9, "got {}", parent.subagent_cost);
        assert_eq!(parent.subagent_runs, 2, "a.jsonl and b.jsonl");
        assert_eq!(parent.project, fx.project);
    }

    #[test]
    fn a_sidechain_whose_parent_is_outside_the_window_is_listed_nowhere() {
        let lock = subagent_fixture_lock().lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("pane-parent-outside-window-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let project = "proj-outside-window".to_string();
        let uuid = "77777777-7777-7777-7777-777777777777".to_string();
        let sub_dir = root.join(&project).join(&uuid).join("subagents");
        fs::create_dir_all(&sub_dir).unwrap();

        // The parent's own activity is 60 days old -- outside the 30-day
        // TREND_DAYS window `session_from` requires before it lists anything.
        let parent_path = root.join(&project).join(format!("{uuid}.jsonl"));
        fs::write(
            &parent_path,
            format!(
                "{}\n",
                json!({
                    "type": "assistant",
                    "timestamp": (Utc::now() - chrono::Duration::days(60)).to_rfc3339(),
                    "cwd": "/w", "requestId": "r-p1", "costUSD": 5.0,
                    "message": {"id": "p1", "model": "claude-haiku-4-5",
                                "usage": {"input_tokens": 10.0, "output_tokens": 5.0}}
                }),
            ),
        )
        .unwrap();

        // Its sidechain, though, wrote something inside the window.
        let sub_path = sub_dir.join("a.jsonl");
        fs::write(
            &sub_path,
            format!(
                "{}\n",
                json!({
                    "type": "assistant",
                    "timestamp": (Utc::now() - chrono::Duration::hours(1)).to_rfc3339(),
                    "cwd": "/w", "requestId": "r-a1", "sessionId": uuid, "isSidechain": true, "costUSD": 3.0,
                    "message": {"id": "a1", "model": "claude-haiku-4-5",
                                "usage": {"input_tokens": 10.0, "output_tokens": 5.0}}
                }),
            ),
        )
        .unwrap();

        let real_root = claude_projects_root();
        let fake_parent = real_root.join(&project).join(format!("{uuid}.jsonl"));
        let fake_sub = real_root.join(&project).join(&uuid).join("subagents").join("a.jsonl");
        inject_scanned(&parent_path, &fake_parent);
        inject_scanned(&sub_path, &fake_sub);
        let fx = SubagentFixture { root, project, uuid: uuid.clone(), fake_paths: vec![fake_parent, fake_sub], _lock: lock };

        // Totals are out of scope here -- only presence/absence in the list.
        let sessions = claude_sessions(None, None, 10_000);
        assert!(
            sessions.iter().all(|s| s.id != fx.uuid),
            "the parent's own activity is outside the window, so it must not be listed just to host its sidechain's cost"
        );
        assert!(sessions.iter().all(|s| s.id != "a"), "a subagent transcript is never listed as its own session");
    }

    #[test]
    fn agent_spend_groups_by_attribution_agent_and_labels_the_rest_unattributed() {
        // Pure: PersistEntry fixtures built entirely in memory, fed straight
        // to agent_spend_from -- no before/after delta and no dependence on
        // this machine's real cache or spend_cache.json, because the
        // fixture below is the *only* data agent_spend_from ever sees here.
        let today = 20_000; // arbitrary day number; only relative offsets matter
        let entry = |parent: &str, agent: Option<&str>, day: i32, cost: f64| PersistEntry {
            parent_session: Some(parent.to_string()),
            agent: agent.map(str::to_string),
            days: vec![(day, "claude-haiku-4-5".to_string(), cost, 15.0)],
            ..Default::default()
        };
        let entries = [
            entry("uuid-a", Some("Explore"), today, 0.5),
            entry("uuid-b", Some("Explore"), today, 0.3),
            entry("uuid-c", None, today, 0.2),
            // Outside the 30-day window: must not count toward Explore.
            entry("uuid-d", Some("Explore"), today - 40, 99.0),
            // Not a subagent transcript at all (no parent_session): must
            // never reach any group, however large its own cost.
            PersistEntry {
                parent_session: None,
                agent: Some("Explore".to_string()),
                days: vec![(today, "claude-haiku-4-5".to_string(), 999.0, 999.0)],
                ..Default::default()
            },
        ];
        let agents = agent_spend_from(entries.iter(), 30, today);

        let explore = agents.iter().find(|a| a.name == "Explore").expect("Explore group present");
        assert_eq!(explore.runs, 2, "uuid-a and uuid-b attribute to Explore; uuid-d is outside the window");
        assert!((explore.cost - 0.8).abs() < 1e-9, "0.5 + 0.3, got {}", explore.cost);

        let unattributed = agents.iter().find(|a| a.name.is_empty()).expect("unattributed group present");
        assert_eq!(unattributed.runs, 1);
        assert!((unattributed.cost - 0.2).abs() < 1e-9, "got {}", unattributed.cost);

        assert!(
            agents.iter().all(|a| a.name != "uuid-a" && a.name != "uuid-b" && a.name != "uuid-c"),
            "agent names come only from attributionAgent, never from a path or uuid"
        );
        let total: f64 = agents.iter().map(|a| a.cost).sum();
        assert!(
            (total - 1.0).abs() < 1e-9,
            "the out-of-window and non-subagent entries (99 and 999) must never reach any group, got total {total}"
        );
    }

    #[test]
    fn agent_spend_orders_the_unattributed_row_last_even_when_it_costs_more() {
        let today = 20_000;
        let entry = |parent: &str, agent: Option<&str>, cost: f64| PersistEntry {
            parent_session: Some(parent.to_string()),
            agent: agent.map(str::to_string),
            days: vec![(today, "claude-haiku-4-5".to_string(), cost, 15.0)],
            ..Default::default()
        };
        // The unattributed entry is by far the biggest spender here -- a
        // plain cost-descending sort would put it first. It must still land
        // last, because a named row is always the more actionable one.
        let entries = [entry("uuid-a", Some("Explore"), 0.1), entry("uuid-b", None, 99.0)];
        let agents = agent_spend_from(entries.iter(), 30, today);
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].name, "Explore");
        assert_eq!(agents.last().map(|a| a.name.as_str()), Some(""), "the unattributed row must sort last regardless of cost");
    }

    #[test]
    fn agent_spend_never_carries_prompt_text_or_titles() {
        let fx = build_subagent_fixture("agent-privacy");

        // agent_spend: re-scan the fixture's own files with the real parser
        // and feed the results straight into agent_spend_from -- same idea
        // as sidechain_tokens_count_once_in_project_totals below, so this
        // still proves the real claude_line parse never leaks into
        // AgentSpend, without depending on this machine's persisted cache
        // or whatever real entries it already holds.
        let mut files = Vec::new();
        recent_jsonl_files(&fx.root, &mut files);
        let today = today_days_from_ce();
        let entries: Vec<PersistEntry> = files
            .iter()
            .map(|f| {
                let data = claude_file(f);
                let mtime = fs::metadata(f).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                to_agent_entry(&data, None, mtime)
            })
            .collect();
        let agents_json = serde_json::to_string(&agent_spend_from(entries.iter(), 30, today)).unwrap();

        // claude_sessions still reads the real, shared cache:
        // build_subagent_fixture injects under a path shaped like the real
        // projects root, which project_of/claude_sessions need to resolve a
        // project at all -- unaffected by the agent_spend purification above.
        let sessions_json = serde_json::to_string(&claude_sessions(None, None, 10_000)).unwrap();

        for planted in [PLANTED_PROMPT, PLANTED_TITLE, PLANTED_TEXT] {
            assert!(!agents_json.contains(planted), "{planted} leaked into AgentSpend: {agents_json}");
            assert!(!sessions_json.contains(planted), "{planted} leaked into SessionSpend: {sessions_json}");
        }
    }

    #[test]
    fn sidechain_tokens_count_once_in_project_totals() {
        let fx = build_subagent_fixture("token-totals");
        // Mimics claude()'s own aggregation, scoped to this fixture's own
        // temp root: recent_jsonl_files + claude_file, the same path the
        // real scanner uses, never the cache injection the other tests use.
        let mut files = Vec::new();
        recent_jsonl_files(&fx.root, &mut files);
        let mut all = FileData::default();
        for file in &files {
            merge_data(&mut all, claude_file(file));
        }
        // 4 distinct messages (p1, p2, a1, a2) plus b1 = 5 distinct assistant
        // lines x 15 tokens each; a1's duplicate must not add a sixth.
        assert_eq!(tokens_sum(&all), 75.0, "parent + both subagent files, each counted exactly once");
    }

    #[test]
    fn touching_a_scratch_folder_does_not_change_the_work_area() {
        let root = "/w";
        let data = claude_run(&[
            area_line("m0", root, 0.5, None), // the first line fixes the session root
            area_line("m1", "/w/acme", 1.0, None),
            // Reading a screenshot or a dotfile is part of the acme work.
            area_line("m2", root, 2.0, Some(("Read", "file_path", "/w/_screenshots/shot.png"))),
            area_line("m3", root, 4.0, Some(("Bash", "command", "cat /w/.cache/x && ls /w/beta/src"))),
            area_line("m4", root, 8.0, None),
        ]);
        assert_eq!(
            area_costs(&data),
            [("(unsorted)".to_string(), 0.5), ("acme".to_string(), 3.0), ("beta/src".to_string(), 12.0)],
            "m2 stays in acme; m3's command skips .cache and lands in beta/src"
        );
        // Working *inside* such a folder is still a deliberate place to be.
        let inside = claude_run(&[
            area_line("m0", root, 0.5, None),
            area_line("m1", "/w/_screenshots", 1.0, None),
        ]);
        assert_eq!(
            area_costs(&inside),
            [("(unsorted)".to_string(), 0.5), ("_screenshots".to_string(), 1.0)]
        );
    }

    #[test]
    fn area_names_from_logs_are_bounded() {
        let root = "/w";
        let mut lines = Vec::new();
        for i in 0..(MAX_AREAS_PER_FILE + 10) {
            lines.push(area_line(&format!("m{i}"), &format!("/w/area{i}"), 1.0, None));
        }
        lines.insert(0, area_line("first", root, 1.0, None));
        lines.push(area_line("long", &format!("/w/{}", "x".repeat(MAX_AREA_KEY + 5)), 1.0, None));
        let data = claude_run(&lines);
        let names: HashSet<&String> = data.areas.keys().map(|(_, a)| a).collect();
        assert!(names.len() <= MAX_AREAS_PER_FILE + 2, "{}", names.len());
        assert!(names.contains(&OTHER_AREA.to_string()));
        assert!(names.iter().all(|n| n.len() <= MAX_AREA_KEY));
    }

    #[test]
    fn areas_survive_a_merge_and_reach_the_project_rows() {
        let today = 800_000;
        let mut a = FileData::default();
        a.areas.insert((today, "acme".into()), (5.0, 50.0));
        let mut b = FileData::default();
        b.areas.insert((today, "acme".into()), (1.0, 10.0));
        b.areas.insert((today - 3, "beta".into()), (9.0, 90.0));
        b.days.insert((today, "claude-opus-5".into()), (15.0, 150.0));
        merge_data(&mut a, b);
        let rows = project_spends(vec![("/w".into(), a)], today);
        let areas = &rows[0].areas;
        assert_eq!(areas.iter().map(|x| x.area.as_str()).collect::<Vec<_>>(), ["beta", "acme"], "by 30-day cost");
        assert_eq!(areas[1].today.cost, 6.0);
        assert_eq!(areas[0].today.cost, 0.0);
        assert_eq!(areas[0].last30.cost, 9.0);
    }

    #[test]
    fn claude_advisor_iterations_expand_once() {
        // Two ordinary message iterations (already inside the parent totals)
        // and one advisor_message that must become its own entry.
        let line = json!({"type": "assistant", "timestamp": "2026-07-10T10:00:00Z",
            "requestId": "req_1",
            "message": {"id": "msg_1", "model": "claude-fable-5-20260115",
                "usage": {"input_tokens": 2.0, "output_tokens": 491.0,
                    "cache_read_input_tokens": 1000.0,
                    "iterations": [
                        {"type": "message", "input_tokens": 1.0, "output_tokens": 200.0},
                        {"type": "advisor_message", "model": "claude-haiku-4-5",
                         "input_tokens": 10.0, "output_tokens": 2.0,
                         "cache_read_input_tokens": 4.0},
                        {"type": "message", "input_tokens": 1.0, "output_tokens": 291.0}
                    ]}}})
        .to_string();
        let once = claude_run(std::slice::from_ref(&line));
        let models: HashSet<&str> = once.days.keys().map(|(_, m)| m.as_str()).collect();
        assert!(models.iter().any(|m| m.contains("fable")));
        assert!(models.iter().any(|m| m.contains("haiku")));
        // Parent 1493 + advisor 16; the plain message iterations add nothing.
        assert_eq!(tokens_sum(&once), 1_509.0);
        // A replayed copy of the same line (same message + request id) is
        // dropped, advisors included.
        let twice = claude_run(&[line.clone(), line]);
        assert_eq!(tokens_sum(&twice), 1_509.0);
    }

    #[test]
    fn claude_sidechain_replay_is_deduped() {
        let parent = json!({"type": "assistant", "timestamp": "2026-07-10T10:00:00Z",
            "requestId": "req_1",
            "message": {"id": "msg_1", "model": "claude-haiku-4-5",
                        "usage": {"input_tokens": 100.0, "output_tokens": 10.0}}})
        .to_string();
        // Sidechain log replays the same message under a fresh request id.
        let replay = json!({"type": "assistant", "timestamp": "2026-07-10T10:00:01Z",
            "requestId": "req_2", "isSidechain": true,
            "message": {"id": "msg_1", "model": "claude-haiku-4-5",
                        "usage": {"input_tokens": 100.0, "output_tokens": 10.0}}})
        .to_string();
        assert_eq!(tokens_sum(&claude_run(&[parent.clone(), replay.clone()])), 110.0);
        // Reverse arrival order still counts the message exactly once.
        assert_eq!(tokens_sum(&claude_run(&[replay, parent.clone()])), 110.0);
        // A genuine retry (no sidechain involved) keeps both.
        let retry = json!({"type": "assistant", "timestamp": "2026-07-10T10:00:02Z",
            "requestId": "req_3",
            "message": {"id": "msg_1", "model": "claude-haiku-4-5",
                        "usage": {"input_tokens": 100.0, "output_tokens": 10.0}}})
        .to_string();
        assert_eq!(tokens_sum(&claude_run(&[parent, retry])), 220.0);
    }

    #[test]
    fn a_subagent_transcript_is_a_sidechain_of_its_directory_session() {
        let root = Path::new("/h/.claude/projects/-w-acme");
        let uuid = "11111111-1111-1111-1111-111111111111";
        assert_eq!(
            sidechain_parent(&root.join(uuid).join("subagents").join("a1b2c3.jsonl")).as_deref(),
            Some(uuid)
        );
        // A top-level session file is never a sidechain of anything.
        assert_eq!(sidechain_parent(&root.join(format!("{uuid}.jsonl"))), None);
        // Two folders deep without a `subagents` directory name is not one.
        assert_eq!(sidechain_parent(&root.join(uuid).join("notes").join("a1b2c3.jsonl")), None);
        // Any directory name two levels up is accepted — no uuid-syntax check.
        assert_eq!(
            sidechain_parent(&root.join("not-a-uuid-at-all").join("subagents").join("x.jsonl")).as_deref(),
            Some("not-a-uuid-at-all")
        );

        // claude_line attributes a line to that directory-named parent,
        // trusting it over a disagreeing `sessionId` (counted, not logged).
        let mut st = ClaudeFileState { parent_session: Some(uuid.to_string()), ..Default::default() };
        let mut data = FileData::default();
        let before = SIDECHAIN_SESSION_MISMATCHES.load(std::sync::atomic::Ordering::Relaxed);
        let line = json!({
            "type": "assistant", "timestamp": "2026-09-20T12:00:00Z", "cwd": "/w",
            "requestId": "r-1", "isSidechain": true, "sessionId": "some-other-session", "costUSD": 0.1,
            "attributionAgent": "Explore",
            "message": {"id": "m1", "model": "claude-haiku-4-5", "content": [{"type": "text", "text": "ok"}],
                         "usage": {"input_tokens": 10.0, "output_tokens": 5.0}}
        })
        .to_string();
        claude_line(&mut st, &line, &mut data);
        assert_eq!(data.parent_session.as_deref(), Some(uuid), "the directory wins over a disagreeing sessionId");
        assert_eq!(data.agent.as_deref(), Some("Explore"));
        let after = SIDECHAIN_SESSION_MISMATCHES.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(after, before + 1, "the disagreement is counted, not logged");
    }

    #[test]
    fn a_workflow_transcript_two_levels_down_is_a_sidechain_of_its_session() {
        let root = Path::new("/h/.claude/projects/-w-acme");
        let uuid = "11111111-1111-1111-1111-111111111111";
        // A workflow run nests its own transcripts a folder deeper than a
        // plain Task-tool subagent: still the enclosing session's sidechain.
        assert_eq!(
            sidechain_parent(
                &root.join(uuid).join("subagents").join("workflows").join("wf-1").join("a1b2c3.jsonl")
            )
            .as_deref(),
            Some(uuid),
            "two folders under subagents/ still belongs to the enclosing session"
        );
        // The walk isn't hardcoded to exactly one extra level -- any depth
        // under the nearest `subagents` component resolves the same way.
        assert_eq!(
            sidechain_parent(
                &root
                    .join(uuid)
                    .join("subagents")
                    .join("workflows")
                    .join("wf-1")
                    .join("attempts")
                    .join("2")
                    .join("x.jsonl")
            )
            .as_deref(),
            Some(uuid)
        );
    }

    #[test]
    fn a_session_literally_named_subagents_still_resolves() {
        // The match is on the path component's name, not on some denylist
        // of names a session directory isn't allowed to have -- a session
        // directory that happens to be named `subagents` is a perfectly
        // valid parent, found the same way any other name would be.
        let root = Path::new("/h/.claude/projects/-w-acme");
        assert_eq!(
            sidechain_parent(&root.join("subagents").join("subagents").join("x.jsonl")).as_deref(),
            Some("subagents"),
            "a session directory literally named `subagents` still resolves as the parent"
        );
        // A top-level session file whose *stem* is "subagents" has no
        // `subagents` path component at all, so it is not a sidechain of
        // anything -- the file name coincidentally matching the magic
        // directory name means nothing here.
        assert_eq!(sidechain_parent(&root.join("subagents.jsonl")), None);
    }

    #[test]
    fn claude_synthetic_model_never_priced() {
        let bare = json!({"type": "assistant", "timestamp": "2026-07-10T10:00:00Z",
            "requestId": "req_1",
            "message": {"id": "msg_1", "model": "<synthetic>",
                        "usage": {"input_tokens": 5.0, "output_tokens": 5.0}}})
        .to_string();
        let data = claude_run(&[bare]);
        assert!(data.days.is_empty());
        assert!(data.unpriced.is_empty()); // a placeholder, not an unknown model

        let carried = json!({"type": "assistant", "timestamp": "2026-07-10T10:00:00Z",
            "requestId": "req_2", "costUSD": 0.5,
            "message": {"id": "msg_2", "model": "<synthetic>",
                        "usage": {"input_tokens": 5.0, "output_tokens": 5.0}}})
        .to_string();
        let data = claude_run(&[carried]);
        assert_eq!(cost_sum(&data), 0.5);
        assert!(data.days.keys().all(|(_, m)| m == "unattributed"));
    }

    // ---- Live pace: work area and cadence for a running agent ------------

    /// A model that reaches `claude_price`'s static "sonnet" fallback but
    /// matches no real catalog slug, so cost math in these tests never
    /// depends on whatever this machine's live pricing catalog happens to
    /// hold on disk.
    const LIVE_PACE_MODEL: &str = "claude-sonnet-live-pace-fixture";

    /// One assistant line shaped like a real Claude Code log entry, keyed
    /// off an explicit epoch-millisecond timestamp so the window and
    /// staleness math can be tested against real clock arithmetic. No
    /// `costUSD`: live pace always prices through `probe_lookup` /
    /// `claude_price`, never a vendor-carried figure.
    fn live_line(mid: &str, cwd: &str, model: &str, ts_ms: i64, tool: Option<(&str, &str, &str)>) -> String {
        let content = match tool {
            Some((name, key, value)) => json!([{"type": "tool_use", "name": name, "input": {key: value}}]),
            None => json!([{"type": "text", "text": "ok"}]),
        };
        json!({
            "type": "assistant",
            "timestamp": chrono::DateTime::from_timestamp_millis(ts_ms).unwrap().to_rfc3339(),
            "cwd": cwd, "requestId": format!("r-{mid}"),
            "message": {"id": mid, "model": model, "content": content,
                        "usage": {"input_tokens": 100.0, "output_tokens": 20.0}}
        })
        .to_string()
    }

    #[test]
    fn live_pace_counts_only_the_last_ten_minutes() {
        let now = 1_790_000_000_000i64;
        let lines = [
            live_line("old", "/w", LIVE_PACE_MODEL, now - LIVE_WINDOW_MS - 1_000, None),
            live_line("new", "/w", LIVE_PACE_MODEL, now - 60_000, None),
        ];
        let pace = pace_from_lines(lines.iter().map(String::as_str), "sess", now).expect("one line is in window");
        assert_eq!(pace.tokens_10m, 120, "only the in-window message's tokens count");
        assert!(pace.priced);
        let usage = json!({"input_tokens": 100.0, "output_tokens": 20.0});
        let tokens = claude_tokens(&usage).unwrap();
        let ts = chrono::DateTime::from_timestamp_millis(now - 60_000).unwrap();
        let expect_cost = claude_cost(LIVE_PACE_MODEL, &tokens, ts).unwrap();
        assert!((pace.cost_10m - expect_cost).abs() < 1e-9, "got {}, want {expect_cost}", pace.cost_10m);
    }

    #[test]
    fn live_pace_deduplicates_streamed_messages() {
        let now = 1_790_000_000_000i64;
        // Same message id AND request id twice: a resumed session
        // replaying the same line verbatim, like claude_line's own dedupe.
        let line = live_line("m1", "/w", LIVE_PACE_MODEL, now - 60_000, None);
        let pace = pace_from_lines([line.as_str(), line.as_str()].into_iter(), "sess", now).unwrap();
        assert_eq!(pace.tokens_10m, 120, "the replay must not double the count");
    }

    #[test]
    fn live_pace_does_not_double_count_a_sidechain_replay() {
        let now = 1_790_000_000_000i64;
        let ts = chrono::DateTime::from_timestamp_millis(now - 60_000).unwrap().to_rfc3339();
        // Same shape `claude_sidechain_replay_is_deduped` exercises against
        // `claude_line`: a sidechain log replays the parent's message under
        // a fresh request id, which slips past the plain mid:rid dedupe
        // above on its own.
        let parent = json!({
            "type": "assistant", "timestamp": ts, "cwd": "/w", "requestId": "r-1",
            "message": {"id": "msg_1", "model": LIVE_PACE_MODEL,
                        "usage": {"input_tokens": 100.0, "output_tokens": 20.0}}
        })
        .to_string();
        let replay = json!({
            "type": "assistant", "timestamp": ts, "cwd": "/w", "requestId": "r-2", "isSidechain": true,
            "message": {"id": "msg_1", "model": LIVE_PACE_MODEL,
                        "usage": {"input_tokens": 100.0, "output_tokens": 20.0}}
        })
        .to_string();
        let pace = pace_from_lines([parent.as_str(), replay.as_str()].into_iter(), "sess", now).unwrap();
        assert_eq!(pace.tokens_10m, 120, "the sidechain replay must not double the count");
    }

    #[test]
    fn live_pace_ignores_a_stale_file() {
        let dir = std::env::temp_dir().join(format!("pane-live-pace-stale-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        fs::write(dir.join("session-a.jsonl"), format!("{}\n", live_line("m1", "/w", LIVE_PACE_MODEL, now - 60_000, None)))
            .unwrap();
        // The file's own mtime is "now" (just written); asking as of a
        // point 6 minutes later makes it stale by the 5-minute gate.
        let got = live_session_in(&dir, now + 6 * 60_000);
        let _ = fs::remove_dir_all(&dir);
        assert!(got.is_none(), "a file untouched for 6 minutes must not report a live pace");
    }

    /// Backdates or advances a just-written file's mtime so a live-pace
    /// test's "which write is newest" comparison never depends on how fast
    /// two sequential `fs::write` calls actually ran on this machine.
    fn set_mtime(path: &Path, ms: i64) {
        let file = fs::OpenOptions::new().write(true).open(path).expect("open for mtime");
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(ms as u64)).expect("set mtime");
    }

    #[test]
    fn live_pace_skips_subagent_transcripts() {
        // A subagent transcript is no longer recursed past to find "the
        // real session" underneath it -- a fresh one now promotes its own
        // uuid instead. This asserts that new attribution rather than the
        // old exclusion (this test's name is the exclusion it used to
        // check for; keep it, since the fixture below still proves a
        // recursive walk isn't what finds the subagent file).
        let dir = std::env::temp_dir().join(format!("pane-live-pace-subagents-{}", std::process::id()));
        let uuid = "11111111-1111-1111-1111-111111111111";
        let sub = dir.join(uuid).join("subagents");
        let _ = fs::create_dir_all(&sub);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let real_path = dir.join("real-session.jsonl");
        fs::write(&real_path, format!("{}\n", live_line("m1", "/w", LIVE_PACE_MODEL, now - 5_000, None))).unwrap();
        set_mtime(&real_path, now - 10_000);
        // A subagent transcript sitting a folder deeper, with its own
        // in-window line and a write fresher than the top-level file above:
        // it must be attributed to its own uuid, never to "real-session"
        // and never under its own file stem ("subagent").
        let subagent_path = sub.join("subagent.jsonl");
        fs::write(&subagent_path, format!("{}\n", live_line("m2", "/w", LIVE_PACE_MODEL, now - 1_000, None))).unwrap();
        set_mtime(&subagent_path, now);
        let got = live_session_in(&dir, now).expect("the subagent's fresh write keeps its uuid live");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(got.session_id, uuid, "attributed to the directory uuid, never the top-level file or the subagent's own stem");
    }

    #[test]
    fn live_pace_follows_a_session_whose_only_fresh_write_is_a_subagent() {
        let dir = std::env::temp_dir().join(format!("pane-live-pace-subagent-only-{}", std::process::id()));
        let uuid = "22222222-2222-2222-2222-222222222222";
        let sub = dir.join(uuid).join("subagents");
        let _ = fs::create_dir_all(&sub);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        // The parent's own file has gone quiet well past the freshness
        // window (and its one line sits outside the 10-minute pace window
        // too).
        let parent_path = dir.join(format!("{uuid}.jsonl"));
        fs::write(&parent_path, format!("{}\n", live_line("p1", "/w", LIVE_PACE_MODEL, now - 20 * 60_000, None))).unwrap();
        set_mtime(&parent_path, now - 20 * 60_000);
        // Only its subagent transcript is still being written.
        let subagent_path = sub.join("a.jsonl");
        fs::write(&subagent_path, format!("{}\n", live_line("a1", "/w", LIVE_PACE_MODEL, now - 5_000, None))).unwrap();
        set_mtime(&subagent_path, now);
        let got = live_session_in(&dir, now).expect("the subagent's write keeps the parent live");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(got.session_id, uuid);
        assert_eq!(got.tokens_10m, 120, "only the subagent's in-window line counts; the parent's is 20 minutes old");
    }

    #[test]
    fn live_pace_follows_a_workflow_transcript_two_levels_down() {
        // Same shape as live_pace_follows_a_session_whose_only_fresh_write_is_a_subagent,
        // but the only fresh write sits two folders under subagents/ --
        // subagents/workflows/<workflow-id>/ -- the way a workflow run nests
        // its own transcripts, rather than directly under subagents/.
        let dir = std::env::temp_dir().join(format!("pane-live-pace-workflow-{}", std::process::id()));
        let uuid = "66666666-6666-6666-6666-666666666666";
        let workflow_dir = dir.join(uuid).join("subagents").join("workflows").join("wf-1");
        let _ = fs::create_dir_all(&workflow_dir);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        // The parent's own file has gone quiet well past the freshness
        // window (and its one line sits outside the 10-minute pace window
        // too) -- identical setup to the one-level-down test above.
        let parent_path = dir.join(format!("{uuid}.jsonl"));
        fs::write(&parent_path, format!("{}\n", live_line("p1", "/w", LIVE_PACE_MODEL, now - 20 * 60_000, None))).unwrap();
        set_mtime(&parent_path, now - 20 * 60_000);
        // Only its workflow transcript, two levels under subagents/, is
        // still being written.
        let workflow_path = workflow_dir.join("a.jsonl");
        fs::write(&workflow_path, format!("{}\n", live_line("a1", "/w", LIVE_PACE_MODEL, now - 5_000, None))).unwrap();
        set_mtime(&workflow_path, now);
        let got = live_session_in(&dir, now).expect("the workflow transcript's write keeps the parent live");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(got.session_id, uuid, "attributed to the enclosing session, never the workflow id or the transcript's own stem");
        assert_eq!(got.tokens_10m, 120, "only the workflow transcript's in-window line counts; the parent's is 20 minutes old");
    }

    #[test]
    fn live_pace_sums_the_parent_and_its_subagents_within_the_window() {
        let dir = std::env::temp_dir().join(format!("pane-live-pace-sum-{}", std::process::id()));
        let uuid = "33333333-3333-3333-3333-333333333333";
        let sub = dir.join(uuid).join("subagents");
        let _ = fs::create_dir_all(&sub);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let parent_path = dir.join(format!("{uuid}.jsonl"));
        fs::write(&parent_path, format!("{}\n", live_line("p1", "/w", LIVE_PACE_MODEL, now - 60_000, None))).unwrap();
        set_mtime(&parent_path, now - 30_000);
        let subagent_path = sub.join("a.jsonl");
        fs::write(&subagent_path, format!("{}\n", live_line("a1", "/w", LIVE_PACE_MODEL, now - 30_000, None))).unwrap();
        set_mtime(&subagent_path, now);
        let got = live_session_in(&dir, now).expect("both writes are fresh");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(got.session_id, uuid);
        assert_eq!(got.tokens_10m, 240, "120 from the parent's own line plus 120 from its subagent, both in window");
    }

    #[test]
    fn live_pace_still_never_picks_a_subagent_file_as_the_session() {
        let dir = std::env::temp_dir().join(format!("pane-live-pace-own-stem-{}", std::process::id()));
        let uuid = "44444444-4444-4444-4444-444444444444";
        let sub = dir.join(uuid).join("subagents");
        let _ = fs::create_dir_all(&sub);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let parent_path = dir.join(format!("{uuid}.jsonl"));
        fs::write(&parent_path, format!("{}\n", live_line("p1", "/w", LIVE_PACE_MODEL, now - 90_000, None))).unwrap();
        set_mtime(&parent_path, now - 60_000);
        // The freshest write on disk is the subagent file itself -- if
        // anything ever reported a subagent's own path instead of the
        // enclosing directory, this would surface its distinct file stem.
        let subagent_path = sub.join("quite-a-different-name.jsonl");
        fs::write(&subagent_path, format!("{}\n", live_line("a1", "/w", LIVE_PACE_MODEL, now - 2_000, None))).unwrap();
        set_mtime(&subagent_path, now);
        let got = live_session_in(&dir, now).expect("the subagent write is fresh");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(got.session_id, uuid);
        assert_ne!(got.session_id, "quite-a-different-name");
    }

    #[test]
    fn live_pace_derives_the_area_from_tool_use_paths_not_text() {
        let now = 1_790_000_000_000i64;
        let root = "/w";
        let lines = [
            live_line("m1", root, LIVE_PACE_MODEL, now - 180_000, None), // text only: no area signal
            live_line("m2", root, LIVE_PACE_MODEL, now - 120_000, Some(("Read", "file_path", "/w/acme/src/main.rs"))),
            live_line("m3", root, LIVE_PACE_MODEL, now - 60_000, None), // sticky: still acme/src
        ];
        let pace = pace_from_lines(lines.iter().map(String::as_str), "sess", now).unwrap();
        assert_eq!(pace.area.as_deref(), Some("acme/src"));
    }

    #[test]
    fn live_pace_never_carries_prompt_text_or_titles() {
        let now = 1_790_000_000_000i64;
        let root = "/w";
        let planted_prompt = "PLANTED_PROMPT_ABOUT_A_SECRET_PROJECT";
        let text_line = json!({
            "type": "assistant",
            "timestamp": chrono::DateTime::from_timestamp_millis(now - 120_000).unwrap().to_rfc3339(),
            "cwd": root, "requestId": "r-t1",
            "message": {"id": "t1", "model": LIVE_PACE_MODEL,
                        "content": [{"type": "text", "text": planted_prompt}],
                        "usage": {"input_tokens": 10.0, "output_tokens": 5.0}}
        })
        .to_string();
        // Real Claude Code shapes for the conversation titles `SessionSpend`
        // also deliberately never reads.
        let title_line = json!({"type": "custom-title", "customTitle": "PLANTED_CUSTOM_TITLE", "sessionId": "sess"}).to_string();
        let ai_title_line = json!({"type": "ai-title", "aiTitle": "PLANTED_AI_TITLE", "sessionId": "sess"}).to_string();
        let tool_line = live_line("t2", root, LIVE_PACE_MODEL, now - 60_000, Some(("Read", "file_path", "/w/acme/notes.md")));
        let lines = [text_line.as_str(), title_line.as_str(), ai_title_line.as_str(), tool_line.as_str()];
        let pace = pace_from_lines(lines.into_iter(), "sess", now).expect("the tool-use line is in window");
        let json = serde_json::to_string(&pace).unwrap();
        for planted in [planted_prompt, "PLANTED_CUSTOM_TITLE", "PLANTED_AI_TITLE"] {
            assert!(!json.contains(planted), "{planted} reached LivePace: {json}");
        }
        assert_eq!(pace.area.as_deref(), Some("acme"), "the real signal still comes through");
    }

    #[test]
    fn live_pace_tail_drops_the_partial_first_line() {
        let dir = std::env::temp_dir().join(format!("pane-live-pace-tail-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("big-session.jsonl");
        // Filler lines pad the file well past the tail window; a real
        // assistant line closes it out.
        let filler = json!({"type": "filler", "pad": "x".repeat(500)}).to_string();
        let mut body = String::new();
        for _ in 0..(LIVE_TAIL_BYTES / filler.len() + 20) {
            body.push_str(&filler);
            body.push('\n');
        }
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let real_line = live_line("real", "/w", LIVE_PACE_MODEL, now - 30_000, None);
        body.push_str(&real_line);
        body.push('\n');
        let total_lines = body.matches('\n').count();
        fs::write(&path, &body).unwrap();
        let lines = tail_lines(&path);
        let _ = fs::remove_dir_all(&dir);
        assert!(lines.len() < total_lines, "the tail must be shorter than the whole file");
        for l in &lines {
            if l.trim().is_empty() {
                continue;
            }
            assert!(
                serde_json::from_str::<Value>(l).is_ok(),
                "a kept line must be complete JSON, never a dropped partial fragment: {l:?}"
            );
        }
        assert!(lines.iter().any(|l| l == &real_line), "the real, complete final line must survive the tail");
    }

    // ---- Live pace: Codex and Gemini CLI ----------------------------------

    #[test]
    fn live_session_for_cwd_dispatches_by_tool_and_rejects_unknown() {
        let now = 1_790_000_000_000i64;
        // A real AGENT_HOSTS tool with no live-pace arm at all: dispatch
        // must fall through to None rather than guessing at a format
        // nobody has taught this function.
        assert_eq!(live_session_for_cwd("Aider", "/w", now), None);
        // Every wired-up tool still resolves cleanly (no panic, just
        // "nothing found") against a folder nothing on this machine could
        // possibly have a session for.
        let bogus = "/definitely/not/a/real/project-xyz-never-exists";
        assert_eq!(live_session_for_cwd("Claude Code", bogus, now), None);
        assert_eq!(live_session_for_cwd("Codex", bogus, now), None);
        assert_eq!(live_session_for_cwd("Gemini CLI", bogus, now), None);
    }

    #[test]
    fn codex_pace_counts_token_deltas_inside_the_window() {
        let now = 1_790_000_000_000i64;
        let at = |offset_ms: i64| chrono::DateTime::from_timestamp_millis(now - offset_ms).unwrap().to_rfc3339();
        let lines = [
            json!({"timestamp": at(20 * 60_000), "type": "turn_context",
                   "payload": {"model": "gpt-5.6-terra"}})
            .to_string(),
            // Outside the window: seeds the delta baseline, never itself counted.
            token_count_line(&at(LIVE_WINDOW_MS + 60_000), None, (5_000.0, 500.0)),
            // Inside the window: two more turns, each a fresh delta off the running total.
            token_count_line(&at(5 * 60_000), None, (6_000.0, 600.0)),
            token_count_line(&at(60_000), None, (8_000.0, 800.0)),
        ];
        let pace = codex_pace_from_lines(lines.iter().map(String::as_str), "sess", now)
            .expect("two turns fall inside the window");
        // (6000-5000)+(600-500) then (8000-6000)+(800-600): the pre-window
        // snapshot's own 5500-token total must never appear.
        assert_eq!(pace.tokens_10m, 1_100 + 2_200);
        assert!(pace.priced);
        assert_eq!(pace.model.as_deref(), Some("gpt-5.6-terra"));
        assert_eq!(pace.tool, "Codex");
        assert_eq!(pace.session_id, "sess");

        let ts1 = chrono::DateTime::from_timestamp_millis(now - 5 * 60_000).unwrap();
        let ts2 = chrono::DateTime::from_timestamp_millis(now - 60_000).unwrap();
        let usage1 = CodexRaw { input: 1_000.0, cached: 0.0, output: 100.0, reasoning: 0.0, total: 1_100.0 };
        let usage2 = CodexRaw { input: 2_000.0, cached: 0.0, output: 200.0, reasoning: 0.0, total: 2_200.0 };
        let expect_cost = codex_turn_cost("gpt-5.6-terra", &usage1, false, ts1).unwrap()
            + codex_turn_cost("gpt-5.6-terra", &usage2, false, ts2).unwrap();
        assert!((pace.cost_10m - expect_cost).abs() < 1e-9, "got {}, want {expect_cost}", pace.cost_10m);
    }

    #[test]
    fn codex_live_session_matches_on_session_meta_cwd() {
        let home = std::env::temp_dir().join(format!("pane-codex-live-cwd-{}", std::process::id()));
        let sessions = home.join("sessions");
        let _ = fs::create_dir_all(&sessions);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let recent = chrono::DateTime::from_timestamp_millis(now - 60_000).unwrap().to_rfc3339();

        let rollout = |cwd: &str, total: (f64, f64)| {
            format!(
                "{}\n{}\n{}\n",
                json!({"timestamp": &recent, "type": "session_meta", "payload": {"cwd": cwd}}),
                json!({"timestamp": &recent, "type": "turn_context", "payload": {"model": "gpt-5.6-terra"}}),
                token_count_line(&recent, None, total),
            )
        };
        // A fresh rollout for a DIFFERENT folder must never win just for
        // being newest.
        fs::write(sessions.join("other.jsonl"), rollout("/w/other", (100.0, 10.0))).unwrap();
        fs::write(sessions.join("mine.jsonl"), rollout("/w/acme", (1_000.0, 100.0))).unwrap();

        let got = codex_live_session_in(&home, "/w/acme", now);
        let _ = fs::remove_dir_all(&home);
        let pace = got.expect("the rollout naming /w/acme must be found even though 'other' is also fresh");
        assert_eq!(pace.tool, "Codex");
        assert_eq!(pace.session_id, "mine");
        assert_eq!(pace.tokens_10m, 1_100);
    }

    #[test]
    fn codex_live_session_ignores_a_stale_rollout() {
        let home = std::env::temp_dir().join(format!("pane-codex-live-stale-{}", std::process::id()));
        let sessions = home.join("sessions");
        let _ = fs::create_dir_all(&sessions);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let recent = chrono::DateTime::from_timestamp_millis(now - 60_000).unwrap().to_rfc3339();
        let body = format!(
            "{}\n{}\n",
            json!({"timestamp": &recent, "type": "session_meta", "payload": {"cwd": "/w/acme"}}),
            token_count_line(&recent, None, (1_000.0, 100.0)),
        );
        fs::write(sessions.join("mine.jsonl"), body).unwrap();
        // The file's own mtime is "now" (just written); asking as of a
        // point 6 minutes later makes it stale by the same 5-minute gate
        // `live_pace_ignores_a_stale_file` uses for Claude Code.
        let got = codex_live_session_in(&home, "/w/acme", now + 6 * 60_000);
        let _ = fs::remove_dir_all(&home);
        assert!(got.is_none(), "a rollout untouched for 6 minutes must not report a live pace");
    }

    #[test]
    fn codex_pace_derives_area_from_a_later_turn_context_cwd() {
        let now = 1_790_000_000_000i64;
        let at = |offset_ms: i64| chrono::DateTime::from_timestamp_millis(now - offset_ms).unwrap().to_rfc3339();
        let lines = [
            json!({"timestamp": at(20 * 60_000), "type": "session_meta", "payload": {"cwd": "/w/acme"}}).to_string(),
            json!({"timestamp": at(9 * 60_000), "type": "turn_context",
                   "payload": {"model": "gpt-5.6-terra", "cwd": "/w/acme/sub"}})
            .to_string(),
            token_count_line(&at(60_000), None, (1_000.0, 100.0)),
        ];
        let pace =
            codex_pace_from_lines(lines.iter().map(String::as_str), "sess", now).expect("one turn falls inside the window");
        assert_eq!(pace.area.as_deref(), Some("sub"), "a later turn_context cwd relative to session_meta's own is the area");
    }

    /// One `"type":"gemini"` chat line shaped like the real 0.61.0 log this
    /// was verified against.
    fn gemini_line(ts: &str, input: f64, cached: f64, output: f64, thoughts: f64, tool: f64) -> String {
        json!({"id": "x", "timestamp": ts, "type": "gemini", "content": "ok", "thoughts": [],
               "tokens": {"input": input, "cached": cached, "output": output, "thoughts": thoughts,
                          "tool": tool, "total": input + cached + output + thoughts + tool},
               "model": "gemini-3.8-flash"})
        .to_string()
    }

    #[test]
    fn gemini_pace_sums_tokens_and_prices_the_newest_in_window_model() {
        let now = 1_790_000_000_000i64;
        let at = |offset_ms: i64| chrono::DateTime::from_timestamp_millis(now - offset_ms).unwrap().to_rfc3339();
        let lines = [
            gemini_line(&at(LIVE_WINDOW_MS + 60_000), 6_000.0, 0.0, 500.0, 100.0, 0.0), // outside the window
            gemini_line(&at(5 * 60_000), 100.0, 0.0, 10.0, 5.0, 0.0),
            gemini_line(&at(60_000), 200.0, 20.0, 50.0, 10.0, 5.0),
        ];
        let pace = gemini_pace_from_lines(lines.iter().map(String::as_str), "sess", now)
            .expect("two turns fall inside the window");
        assert_eq!(pace.tokens_10m, 400, "115 (turn 1) + 285 (turn 2); the pre-window turn's 6,600 must never appear");
        assert!(pace.priced, "gemini-3.8-flash is a baked price entry");
        assert_eq!(pace.model.as_deref(), Some("gemini-3.8-flash"));
        assert_eq!(pace.tool, "Gemini CLI");
        assert_eq!(pace.area, None, "no folder signal exists inside a chat log's own lines");

        let price = probe_lookup("gemini-3.8-flash").expect("baked in pricing.rs::builtin_price");
        let ts1 = chrono::DateTime::from_timestamp_millis(now - 5 * 60_000).unwrap();
        let ts2 = chrono::DateTime::from_timestamp_millis(now - 60_000).unwrap();
        let u1 = pricing::Usage { input: 100.0, output: 10.0, cache_read: 0.0, cache_write_5m: 0.0, cache_write_1h: 0.0 };
        // cached(20) is billed as a subset of input(200): u.input = 180.
        let u2 = pricing::Usage { input: 180.0, output: 50.0, cache_read: 20.0, cache_write_5m: 0.0, cache_write_1h: 0.0 };
        let expect_cost =
            cost_for("gemini-3.8-flash", &price, &u1, 200_000.0, ts1) + cost_for("gemini-3.8-flash", &price, &u2, 200_000.0, ts2);
        assert!((pace.cost_10m - expect_cost).abs() < 1e-9, "got {}, want {expect_cost}", pace.cost_10m);
    }

    #[test]
    fn gemini_live_session_matches_on_project_root() {
        let root = std::env::temp_dir().join(format!("pane-gemini-live-{}", std::process::id()));
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let recent = chrono::DateTime::from_timestamp_millis(now - 60_000).unwrap().to_rfc3339();

        let make_project = |name: &str, project_root: &str, total: f64| {
            let dir = root.join(name);
            let chats = dir.join("chats");
            fs::create_dir_all(&chats).unwrap();
            fs::write(dir.join(".project_root"), project_root).unwrap();
            let line = gemini_line(&recent, total, 0.0, 0.0, 0.0, 0.0);
            fs::write(chats.join("session-a.jsonl"), format!("{line}\n")).unwrap();
        };
        // A fresh project for a DIFFERENT folder must never win just for
        // being newest.
        make_project("other", "/w/other", 10.0);
        make_project("mine", "/w/acme", 500.0);

        let got = gemini_live_session_in(&root, "/w/acme", now);
        let _ = fs::remove_dir_all(&root);
        let pace = got.expect("the project naming /w/acme must be found even though 'other' is also fresh");
        assert_eq!(pace.tool, "Gemini CLI");
        assert_eq!(pace.tokens_10m, 500);
    }

    #[test]
    fn gemini_live_session_ignores_a_stale_project() {
        let root = std::env::temp_dir().join(format!("pane-gemini-live-stale-{}", std::process::id()));
        let dir = root.join("mine");
        let chats = dir.join("chats");
        fs::create_dir_all(&chats).unwrap();
        fs::write(dir.join(".project_root"), "/w/acme").unwrap();
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        let recent = chrono::DateTime::from_timestamp_millis(now - 60_000).unwrap().to_rfc3339();
        let line = gemini_line(&recent, 10.0, 0.0, 0.0, 0.0, 0.0);
        fs::write(chats.join("session-a.jsonl"), format!("{line}\n")).unwrap();
        let got = gemini_live_session_in(&root, "/w/acme", now + 6 * 60_000);
        let _ = fs::remove_dir_all(&root);
        assert!(got.is_none(), "a chat log untouched for 6 minutes must not report a live pace");
    }

    #[test]
    fn devin_model_normalizes_fable_and_modes() {
        assert_eq!(devin_model("claude-5-fable-medium"), "claude-fable-5");
        assert_eq!(devin_model("claude-5-fable-max"), "claude-fable-5");
        assert_eq!(devin_model("claude-5-fable-high"), "claude-fable-5");
        assert_eq!(devin_model("gpt-5-6-sol-max"), "gpt-5.6-sol");
        assert_eq!(devin_model("claude-opus-4-8-medium"), "claude-opus-4-8");
        assert_eq!(devin_model("gpt-4-0125-preview"), "gpt-4-0125-preview");
        assert_eq!(devin_model("penguin-max"), "penguin");
        assert_eq!(devin_model("swe-1-6-fast"), "swe-1-6");
        assert_eq!(devin_model("swe-1-7-medium"), "swe-1-7");
        assert_eq!(devin_model("swe-1-7-lightning"), "swe-1-7-lightning");
    }

    /// Devin review regression: `devin_model` used to strip `-fast` from
    /// every slug, so a non-Cognition fast request billed at base rates.
    /// Only Cognition stems may lose the suffix; everyone else keeps the
    /// premium SKU so the lookup applies the fast multiplier.
    #[test]
    fn devin_fast_suffix_stays_priced_for_non_cognition_models() {
        // Cognition `-fast` is a Devin mode — peels to the 1× base card.
        assert_eq!(devin_model("swe-1-6-fast"), "swe-1-6");
        assert_eq!(devin_model("swe-1-7-fast"), "swe-1-7");
        assert_eq!(devin_model("penguin-fast"), "penguin");
        // Every other `-fast` is the premium SKU and survives.
        assert_eq!(devin_model("gpt-5-6-sol-fast"), "gpt-5.6-sol-fast");
        assert_eq!(devin_model("gpt-5-6-sol-max-fast"), "gpt-5.6-sol-max-fast");
        assert_eq!(devin_model("grok-4.6-fast"), "grok-4.6-fast");

        // The surviving suffix must price above the base card, not at it.
        let base = pricing::lookup("grok-4.6").expect("grok-4.6 prices");
        let fast = pricing::lookup(&devin_model("grok-4.6-fast")).expect("grok fast prices");
        assert!(
            fast.input > base.input && fast.output > base.output,
            "fast tier must bill a premium over {base:?}, got {fast:?}"
        );
    }

    #[test]
    fn kimi_counts_turn_records_only() {
        let mut data = FileData::default();
        let turn = json!({"type": "usage.record", "model": "moonshot-ai/kimi-test-model",
            "usage": {"inputOther": 400.0, "output": 200.0, "inputCacheRead": 300.0,
                      "inputCacheCreation": 100.0},
            "usageScope": "turn", "time": 1784208630652i64})
        .to_string();
        let session_scope = turn.replace("\"turn\"", "\"session\"");
        kimi_line(&turn, &mut data);
        kimi_line(&session_scope, &mut data);
        // One event; unknown model → tokens counted, dollars honest zero.
        assert_eq!(data.days.values().map(|v| v.1).sum::<f64>(), 1_000.0);
        assert_eq!(data.unpriced.get("kimi-test-model"), Some(&1));
        assert!(data.days.keys().all(|(_, m)| m == "kimi-test-model"));
    }

    #[test]
    fn kimi_plan_k3_slug_uses_published_rates() {
        let mut data = FileData::default();
        let turn = json!({"type": "usage.record", "model": "kimi-code/k3",
            "usage": {"inputOther": 1000.0, "output": 1000.0, "inputCacheRead": 0.0,
                      "inputCacheCreation": 0.0},
            "usageScope": "turn", "time": 1784208630652i64})
        .to_string();
        kimi_line(&turn, &mut data);
        assert!(data.unpriced.is_empty(), "plan k3 should price: {:?}", data.unpriced);
        assert!(data.days.keys().all(|(_, m)| m == "k3"));
        let cost: f64 = data.days.values().map(|v| v.0).sum();
        let expect = (1000.0 * 3.0 + 1000.0 * 15.0) / 1e6;
        assert!((cost - expect).abs() < 1e-9, "cost {cost} != {expect}");
    }

    #[test]
    fn kimi_k25_cache_hits_use_published_rate() {
        let mut data = FileData::default();
        let turn = json!({"type": "usage.record", "model": "moonshot-ai/kimi-k2.5",
            "usage": {"inputOther": 0.0, "output": 0.0, "inputCacheRead": 1_000_000.0,
                      "inputCacheCreation": 0.0},
            "usageScope": "turn", "time": 1784208630652i64})
        .to_string();
        kimi_line(&turn, &mut data);
        assert!(data.unpriced.is_empty(), "k2.5 should price: {:?}", data.unpriced);
        let cost: f64 = data.days.values().map(|v| v.0).sum();
        assert!((cost - 0.10).abs() < 1e-9, "cost {cost} != 0.10");
    }

    /// Codex sessions driven through a router against the Kimi plan log
    /// "kimi-oauth/k3" turns — those bill Moonshot, not the ChatGPT sub,
    /// so they move off the Codex card with the vendor prefix peeled.
    #[test]
    fn codex_kimi_oauth_rows_move_to_the_kimi_card() {
        let lines = vec![
            json!({"timestamp": "2026-08-18T10:00:00Z", "type": "turn_context",
                   "payload": {"model": "gpt-5.6-sol"}})
            .to_string(),
            token_count_line("2026-08-18T10:00:01Z", Some((1_000.0, 100.0)), (1_000.0, 100.0)),
            json!({"timestamp": "2026-08-18T10:01:00Z", "type": "turn_context",
                   "payload": {"model": "kimi-oauth/k3"}})
            .to_string(),
            token_count_line("2026-08-18T10:01:01Z", Some((2_000.0, 200.0)), (3_000.0, 300.0)),
        ];
        let mut all = codex_run(&lines);
        let moved = split_kimi_routed(&mut all);
        // The GPT turn stays on Codex; the Kimi turn moves, prefix peeled.
        assert!(all.days.keys().all(|(_, m)| m == "gpt-5.6-sol"));
        assert_eq!(tokens_sum(&all), 1_100.0);
        assert!(moved.days.keys().all(|(_, m)| m == "k3"), "{:?}", moved.days.keys());
        assert_eq!(tokens_sum(&moved), 2_200.0);
    }

    #[test]
    fn strip_kimi_prefix_covers_every_spelling() {
        assert_eq!(strip_kimi_prefix("kimi-oauth/k3"), "k3");
        assert_eq!(strip_kimi_prefix("kimi-code/k3"), "k3");
        assert_eq!(strip_kimi_prefix("moonshot-ai/kimi-k3"), "kimi-k3");
        assert_eq!(strip_kimi_prefix("moonshot/kimi-k2.5"), "kimi-k2.5");
        assert_eq!(strip_kimi_prefix("kimi-k3"), "kimi-k3");
        assert_eq!(strip_kimi_prefix("Kimi-OAuth/K3"), "K3");
        assert_eq!(strip_kimi_prefix("moonshotai/kimi-k2"), "kimi-k2");
    }

    #[test]
    fn kimi_spend_stays_on_moonshot_without_login() {
        assert_eq!(kimi_spend_target(true, false), ("kimi", "Kimi Code"));
        assert_eq!(kimi_spend_target(false, false), ("moonshot", "Kimi API"));
        assert_eq!(kimi_spend_target(true, true), ("moonshot", "Kimi API"));
        assert_eq!(kimi_spend_target(false, true), ("moonshot", "Kimi API"));
    }

    /// Live diagnostic (ignored): what each spend source produced from
    /// this machine's real logs. Run:
    ///   cargo test spend_live_dump -- --ignored --nocapture
    #[test]
    #[ignore]
    fn spend_live_dump() {
        for sp in collect(None) {
            println!(
                "{}: today ${:.2}/{:.1}M | yesterday ${:.2} | 30d ${:.2} | unpriced {} {:?}",
                sp.id,
                sp.today.cost,
                sp.today.tokens / 1e6,
                sp.yesterday.cost,
                sp.last30.cost,
                sp.unpriced,
                sp.unpriced_models,
            );
        }
    }

    #[test]
    fn pi_lines_fold_into_the_underlying_card() {
        let mut seen = HashSet::new();
        let mut data = FileData::default();
        let carried = json!({"type": "message", "id": "m1", "timestamp": "2026-08-03T10:00:00Z",
            "message": {"role": "assistant", "provider": "anthropic", "model": "pi-test-model",
                        "usage": {"input": 400.0, "output": 100.0, "cacheRead": 0.0,
                                  "cacheWrite": 0.0, "totalTokens": 500.0,
                                  "cost": {"total": 1.25}}}})
        .to_string();
        let zero_cost = json!({"type": "message", "id": "m2", "timestamp": "2026-08-03T10:01:00Z",
            "message": {"role": "assistant", "provider": "openai-codex", "model": "pi-test-model",
                        "usage": {"input": 300.0, "output": 200.0, "cacheRead": 0.0,
                                  "cacheWrite": 0.0, "totalTokens": 500.0,
                                  "cost": {"total": 0.0}}}})
        .to_string();
        let unmapped = carried.replace("\"anthropic\"", "\"nvidia-nim\"");
        // Cost recorded but no token counters: the dollars still count.
        let cost_only = json!({"type": "message", "id": "m3", "timestamp": "2026-08-03T10:02:00Z",
            "message": {"role": "assistant", "provider": "anthropic", "model": "pi-test-model",
                        "usage": {"cost": {"total": 0.75}}}})
        .to_string();
        pi_line(&mut seen, &carried, &mut data);
        pi_line(&mut seen, &carried, &mut data); // duplicate id → dropped
        pi_line(&mut seen, &zero_cost, &mut data);
        pi_line(&mut seen, &unmapped, &mut data); // no card here → dropped
        pi_line(&mut seen, &cost_only, &mut data);

        let claude = take_tagged(&mut data, "claude");
        let codex = take_tagged(&mut data, "codex");
        assert!(data.days.is_empty() && data.unpriced.is_empty());
        // Carried costs used directly, replay dropped: 1.25 + 0.75.
        assert_eq!(claude.days.values().map(|v| (v.0, v.1)).collect::<Vec<_>>(), vec![(2.0, 500.0)]);
        assert!(claude.days.keys().all(|(_, m)| m == "pi-test-model"));
        // $0 carried cost falls through to pricing; unknown model → honest ⚠.
        assert_eq!(codex.days.values().map(|v| v.1).sum::<f64>(), 500.0);
        assert_eq!(codex.unpriced.get("pi-test-model"), Some(&1));
    }

    /// Overflowed Pi keys keep their routing prefix: take_tagged must still
    /// claim them, so capped usage lands on the right card instead of
    /// vanishing with the discarded scan.
    #[test]
    fn pi_overflow_keeps_its_card_routing() {
        let mut seen = HashSet::new();
        let mut data = FileData::default();
        let huge = "m".repeat(10_000);
        for (id, provider) in [("p1", "anthropic"), ("p2", "openai-codex")] {
            let line = json!({"type": "message", "id": id, "timestamp": "2026-08-03T10:00:00Z",
                "message": {"role": "assistant", "provider": provider, "model": &huge,
                            "usage": {"input": 100.0, "output": 50.0, "cacheRead": 0.0,
                                      "cacheWrite": 0.0, "totalTokens": 150.0,
                                      "cost": {"total": 1.0}}}})
            .to_string();
            pi_line(&mut seen, &line, &mut data);
        }
        let claude = take_tagged(&mut data, "claude");
        let codex = take_tagged(&mut data, "codex");
        // Nothing stranded between cards.
        assert!(data.days.is_empty() && data.unpriced.is_empty());
        assert_eq!(cost_sum(&claude), 1.0);
        assert_eq!(cost_sum(&codex), 1.0);
        assert_eq!(tokens_sum(&claude), 150.0);
        assert!(claude.days.keys().all(|(_, m)| m == OVERFLOW_MODEL_KEY));
        assert!(codex.days.keys().all(|(_, m)| m == OVERFLOW_MODEL_KEY));
    }

    #[test]
    fn qwen_lines_count_tokens_with_cache_split() {
        let mut data = FileData::default();
        let line = json!({"schemaVersion": 1, "timestamp": "2026-08-03T15:22:19.090Z",
            "model": "qwen-test-model", "inputTokens": 700.0, "outputTokens": 200.0,
            "cachedTokens": 300.0, "thoughtsTokens": 50.0, "totalTokens": 900.0})
        .to_string();
        qwen_line(&line, &mut data);
        qwen_line("not json", &mut data);
        // input(700, of which 300 cached) + output(200); thoughts are a
        // subset of output and must not double-count.
        assert_eq!(data.days.values().map(|v| v.1).sum::<f64>(), 900.0);
        assert_eq!(data.unpriced.get("qwen-test-model"), Some(&1));

        // Gemini-cli ancestry shape: thoughts OUTSIDE output, so
        // total(950) > input(700) + output(200) — thoughts join output.
        let mut gem = FileData::default();
        let line = json!({"schemaVersion": 1, "timestamp": "2026-08-03T15:22:19.090Z",
            "model": "qwen-test-model", "inputTokens": 700.0, "outputTokens": 200.0,
            "cachedTokens": 0.0, "thoughtsTokens": 50.0, "totalTokens": 950.0})
        .to_string();
        qwen_line(&line, &mut gem);
        assert_eq!(gem.days.values().map(|v| v.1).sum::<f64>(), 950.0);
    }

    #[test]
    fn split_models_reroutes_minimax_usage() {
        let mut data = FileData::default();
        data.days.insert((1000, "claude-fable-5".into()), (5.0, 100.0));
        data.days.insert((1000, "MiniMax-M3".into()), (0.5, 50.0));
        data.days.insert((1001, "MiniMax-M2.7".into()), (0.2, 20.0));
        data.unpriced.insert("MiniMax-Unknown".into(), 3);
        data.unpriced.insert("mystery-model".into(), 1);

        let mm = split_models(&mut data, "MiniMax");
        assert_eq!(data.days.len(), 1);
        assert_eq!(data.unpriced.len(), 1);
        assert_eq!(mm.days.len(), 2);
        assert_eq!(mm.days[&(1000, "MiniMax-M3".to_string())], (0.5, 50.0));
        assert_eq!(mm.unpriced.get("MiniMax-Unknown"), Some(&3));
    }

    #[test]
    fn claude_unknown_speed_marks_foreign_log_shape() {
        let line = json!({"type": "assistant", "timestamp": "2026-07-10T10:00:00Z",
            "requestId": "req_1",
            "message": {"id": "msg_1", "model": "claude-haiku-4-5",
                        "usage": {"input_tokens": 100.0, "output_tokens": 10.0,
                                  "speed": "turbo"}}})
        .to_string();
        assert!(claude_run(&[line]).days.is_empty());
    }

    /// Live probe over this machine's real logs + Cursor export. Prints
    /// aggregates and the CSV header only. Run via
    /// `cargo test --lib spend -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_probe() {
        // Real export data is echoed here — cap every dump at 200 chars.
        let clip = |s: String| s.chars().take(200).collect::<String>();
        let csv = crate::rt::block_on(crate::providers::cursor::fetch_usage_csv());
        eprintln!("cursor csv: {} bytes", csv.as_deref().map(str::len).unwrap_or(0));
        if let Some(c) = &csv {
            eprintln!("{}", clip(format!("csv header: {}", c.lines().next().unwrap_or(""))));
            for row in c.lines().skip(1).take(3) {
                let cells = super::split_csv_row(row);
                eprintln!(
                    "{}",
                    clip(format!(
                        "row: date={:?} parsed={} model={:?} in={:?} out={:?} total={:?} cost={:?}",
                        cells.first(),
                        cells.first().map(|d| super::parse_csv_date(d).is_some()).unwrap_or(false),
                        cells.get(4),
                        cells.get(6),
                        cells.get(9),
                        cells.get(10),
                        cells.get(11),
                    ))
                );
            }
        }
        for sp in super::collect(csv) {
            eprintln!(
                "{}",
                clip(format!(
                    "{}: today=${:.2} 30d=${:.2} tokens30={:.0} unpriced={} {:?}",
                    sp.id, sp.today.cost, sp.last30.cost, sp.last30.tokens, sp.unpriced, sp.unpriced_models
                ))
            );
        }
    }
}
