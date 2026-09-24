//! Local spend computation — the "Total Spend" dashboard, per-provider
//! Today / Yesterday / Last 30 Days rows, per-model breakdowns, and the
//! 30-day Usage Trend series. Mirrors the macOS app: costs are derived
//! from the session logs each CLI already writes on this machine, so
//! nothing is sent anywhere.
//!
//! Large logs are handled with a per-file cache keyed by (mtime, size):
//! only files that changed since the last refresh are re-parsed.

use chrono::{DateTime, Datelike, Local, Utc};
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

const PERSIST_VERSION: u32 = 8; // bump on cache format *or* parser-logic changes

/// Set when any file was (re)parsed this run — nothing changed, nothing saved.
static CACHE_DIRTY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Paths seen by file_days() this collect() run. Entries for paths nobody
/// scanned anymore (deleted logs, disabled providers) are dropped on save,
/// so the cache can't grow without bound.
fn touched() -> &'static Mutex<HashSet<PathBuf>> {
    static TOUCHED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    TOUCHED.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(serde::Serialize, serde::Deserialize)]
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
        let Ok(raw) = fs::read_to_string(persist_path()) else { return };
        let Ok(doc) = serde_json::from_str::<PersistFile>(&raw) else { return };
        if doc.version != PERSIST_VERSION || doc.corrections != pricing::corrections_rev() {
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
    });
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
    let today = Local::now().date_naive().num_days_from_ce();
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
                if !warm_ok || reader.seek(SeekFrom::Start(from)).is_err() {
                    if reader.seek(SeekFrom::Start(from)).is_err() {
                        PROBES.with(|p| {
                            p.borrow_mut().take();
                        });
                        return data;
                    }
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
        let blocks = v.pointer("/message/content").and_then(Value::as_array);
        found = blocks.into_iter().flatten().find_map(|block| {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                return None;
            }
            let input = block.get("input")?;
            ["file_path", "path", "notebook_path"]
                .iter()
                .find_map(|k| input.get(*k).and_then(Value::as_str))
                .and_then(|p| area_under(p, &root, false))
                .filter(|a| !is_scratch_area(a))
                .or_else(|| {
                    input
                        .get("command")
                        .and_then(Value::as_str)
                        .and_then(|c| area_in_command(c, &root, home))
                })
        });
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

fn claude_line(st: &mut ClaudeFileState, line: &str, data: &mut FileData) {
    if !line.contains("\"type\":\"assistant\"") {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
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
    let today = Local::now().date_naive().num_days_from_ce();
    let known = crate::inventory::known_project_paths();
    let Ok(map) = cache().lock() else { return Vec::new() };
    let mut out: Vec<SessionSpend> = map
        .iter()
        .filter_map(|(path, entry)| {
            let project = project_of(&root, path)?;
            let id = path.file_stem()?.to_str()?;
            let mut session =
                session_from(id, &resolve_project(&project, &known), &entry.data, entry.claude.as_ref(), today, entry.size)?;
            if let Some(day) = day {
                let on_day: f64 =
                    entry.data.days.iter().filter(|((d, _), _)| *d == day).map(|(_, (c, _))| c).sum();
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
    /// dollar figure.
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
}

/// `live_session_in` at the project folder a cwd's own session logs live
/// under.
pub fn live_session_for_cwd(cwd: &str, now_ms: i64) -> Option<LivePace> {
    let dir = claude_projects_root().join(encode_project_path(cwd));
    live_session_in(&dir, now_ms)
}

/// The newest top-level `*.jsonl` directly under `dir`, read only when it
/// was written inside `LIVE_FRESH_MS`. Never recurses, so a subagent
/// transcript -- one or more folders deeper, under `<session-uuid>/subagents/`
/// -- can never be picked: it is never even listed.
fn live_session_in(dir: &Path, now_ms: i64) -> Option<LivePace> {
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(ftype) = entry.file_type() else { continue };
        if !ftype.is_file() {
            continue;
        }
        let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else { continue };
        let better = match &newest {
            Some((_, best)) => mtime > *best,
            None => true,
        };
        if better {
            newest = Some((path, mtime));
        }
    }
    let (path, mtime) = newest?;
    let mtime_ms = mtime.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_millis() as i64;
    if now_ms.saturating_sub(mtime_ms) > LIVE_FRESH_MS {
        return None;
    }
    let session_id = path.file_stem()?.to_str()?.to_string();
    let lines = tail_lines(&path);
    pace_from_lines(lines.iter().map(String::as_str), &session_id, now_ms)
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
/// as the folder they are measured under. Exactly `claude_area`'s own
/// tool-call signal (`file_path` / `path` / `notebook_path` first, then the
/// first path under `root` in a Bash `command`), reused rather than
/// reimplemented: `area_under` and `area_in_command` are the same private
/// helpers, scratch folders (`is_scratch_area`) are excluded the same way.
fn live_tool_use_area(v: &Value, root: &str, home: Option<&str>) -> Option<String> {
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
            if let Some(found) = live_tool_use_area(&v, cwd, home) {
                area = Some(found);
            }
        }
        if newest_ms.is_none_or(|n| ms >= n) {
            newest_ms = Some(ms);
            model = v.pointer("/message/model").and_then(Value::as_str).map(str::to_string);
        }

        // Dedupe on (message id, request id), like `claude_line`: a
        // resumed session can repeat the same line verbatim.
        if let Some(mid) = v.pointer("/message/id").and_then(Value::as_str) {
            let rid = v.get("requestId").and_then(Value::as_str).unwrap_or("");
            if !seen.insert(format!("{mid}:{rid}")) {
                continue;
            }
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
    spend.projects = project_spends(per_project, Local::now().date_naive().num_days_from_ce());
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
        return;
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
            return;
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
            return;
        }
        Some("token_count") => {}
        _ => {
            // turn_context (or older shapes): update the session's model.
            if let Some(m) = v.pointer("/payload/model").and_then(Value::as_str) {
                st.model = m.to_string();
            }
            return;
        }
    }

    // token_count from here on. A model on the line itself wins.
    if let Some(m) = v
        .pointer("/payload/model")
        .and_then(Value::as_str)
        .or_else(|| v.pointer("/payload/info/model").and_then(Value::as_str))
    {
        st.model = m.to_string();
    }
    let Some(ts) = parse_ts(v.get("timestamp")) else { return };
    let totals = v.pointer("/payload/info/total_token_usage").map(codex_raw);

    // Replayed parent history: seed the delta baseline, never count it —
    // a large parent history takes several seconds to replay, which is why
    // this is a log marker and not a time window (the Mac's old one-second
    // window leaked replays and inflated spend ~20x).
    if st.gate.is_some() {
        if let Some(t) = totals {
            st.prev_totals = Some(t);
        }
        return;
    }
    // Unchanged cumulative totals mean a re-emitted stale snapshot, not new
    // usage — even when the line repeats a last_token_usage.
    if let (Some(t), Some(p)) = (&totals, &st.prev_totals) {
        if t == p {
            return;
        }
    }
    let usage = match v.pointer("/payload/info/last_token_usage") {
        Some(l) => codex_raw(l),
        None => match &totals {
            Some(t) => t.minus(st.prev_totals.as_ref()),
            None => return,
        },
    };
    if let Some(t) = totals {
        st.prev_totals = Some(t);
    }
    if !usage.any_tokens() {
        return;
    }

    let model = if st.model.is_empty() { "gpt-5".to_string() } else { st.model.clone() };
    let tokens = usage.total;

    // Codex speed is a provider tier, not Cursor's `-fast` price variant: a
    // `-fast` slug resolves through its unscaled base rates and the Codex
    // multiplier applies exactly once. A fast-only third-party slug with no
    // base entry keeps its already-scaled rate, no second multiplier.
    // Auto-review keeps its own name in the breakdown; only the dollar math
    // uses the dated GPT fallback (Mac parity with OpenUsage #1085).
    let rate_source = if model.eq_ignore_ascii_case("codex-auto-review") {
        auto_review_fallback(ts)
    } else {
        model.clone()
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
    let Some(mut p) = price else {
        note_unpriced(data, ts, &model, tokens);
        return;
    };
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
    let is_fast = if alias_fast { base_price.is_some() } else { st.fast_tier };
    let mult = if is_fast { codex_priority_multiplier(&dated, &rate_model) } else { 1.0 };

    let cached = usage.cached.min(usage.input);
    let u = pricing::Usage {
        input: usage.input - cached,
        output: usage.output,
        cache_read: cached,
        cache_write_5m: 0.0,
        cache_write_1h: 0.0,
    };
    add_event(data, ts, &model, cost_for(&model, &p, &u, threshold, ts) * mult, tokens);
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

fn codex(extra: FileData) -> (ProviderSpend, FileData) {
    let home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".codex"));
    let mut all = codex_scan(&home);
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
        println!("{}", serde_json::to_string(&collect(None)).unwrap());
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
        let today = Local::now().date_naive().num_days_from_ce();
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
                }),
                pi_seen: vec!["pi-msg-1".into()],
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
        write!(f, "{}\n", line(4_000.0)).unwrap();
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
        let head = vec![
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
        write!(f, "{}\n", &second_line[12..]).unwrap();
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
        write!(f, "{}\n", line("msg_1", "req_1", 100.0)).unwrap();
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
        write!(f, "{}\n", &second_line[12..]).unwrap();
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
        write!(f, "{}\n", line(4_000.0)).unwrap();
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
        let paths = vec![
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
        let outside = vec![PathBuf::from("/h/.codex/sessions/def.jsonl")];
        assert_eq!(session_path_among(&root, outside.iter(), "def"), None);
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

    #[test]
    fn live_pace_skips_subagent_transcripts() {
        let dir = std::env::temp_dir().join(format!("pane-live-pace-subagents-{}", std::process::id()));
        let sub = dir.join("11111111-1111-1111-1111-111111111111").join("subagents");
        let _ = fs::create_dir_all(&sub);
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64;
        fs::write(
            dir.join("real-session.jsonl"),
            format!("{}\n", live_line("m1", "/w", LIVE_PACE_MODEL, now - 5_000, None)),
        )
        .unwrap();
        // A subagent transcript sitting a folder deeper, with its own
        // in-window line: if the walk ever recursed, this file would be
        // found (and would win on content) instead of the real session.
        fs::write(
            sub.join("subagent.jsonl"),
            format!("{}\n", live_line("m2", "/w", LIVE_PACE_MODEL, now - 1_000, None)),
        )
        .unwrap();
        let got = live_session_in(&dir, now).expect("the top-level session is live");
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(got.session_id, "real-session");
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
