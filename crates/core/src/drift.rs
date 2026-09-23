//! Is our rate card still right?
//!
//! Claude Code writes a `cost-state` line into its session logs holding, per
//! model, the tokens it used and **the vendor's own `costUSD` for them**. That
//! is a price reference already sitting on the machine, so the app can check
//! its catalogue against the vendor instead of assuming. No network, no new
//! permission: the same files the spend scan already reads.
//!
//! What a disagreement means is deliberately left open in the copy. Either the
//! catalogue is stale, or the vendor counts tokens differently than we do.
//! Both are worth knowing and neither is worth asserting, so the finding shows
//! both numbers and says so.
//!
//! Samples that used web search are skipped: those requests are billed per
//! search, which no token rate can reproduce, and including them would invent
//! a drift that is not there.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use serde::Serialize;

use crate::i18n::Msg;
use crate::inventory::Opportunity;
use crate::pricing::{self, Price};

/// Below this the two numbers agree for our purposes: rounding, a token the
/// vendor counts in a different bucket, a cache write billed at a 1h rate.
const AGREE_PERCENT: f64 = 8.0;
/// Under this many dollars the percentage is noise on a rounding error.
const MIN_DOLLARS: f64 = 0.50;
/// One odd session proves nothing; a rate that is wrong is wrong every time.
const MIN_SAMPLES: usize = 2;
/// Session files to read, newest first. A stale rate shows up in recent work.
const MAX_FILES: usize = 250;
/// Stop reading a single file after this much: the line we want is small and
/// a session log can be gigabytes.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

const PRICING_LEARN: &str = "https://staas.fund/classroom/";

/// One model's slice of one `cost-state` line.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelCost {
    pub model: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub web_searches: u64,
    /// What the vendor says those tokens cost.
    pub vendor_cost: f64,
}

/// Our catalogue against the vendor, for one model.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub model: String,
    /// What this app's rate card charges for the same tokens.
    pub ours: f64,
    /// What the vendor charged.
    pub vendor: f64,
    /// Ours against theirs. Positive = we over-report the spend.
    pub diff_percent: f64,
    pub samples: usize,
    /// False when no catalogue prices the model at all.
    pub priced: bool,
    /// Set when the whole gap is explained by cache writes having been made
    /// with the 1-hour TTL, which bills at twice input rather than the
    /// 5-minute rate the app assumes. The logs do not record which TTL was
    /// used, so this is inferred from the money and only claimed when the
    /// remainder is under a cent on the dollar.
    pub one_hour_cache: bool,
    /// Cache-creation tokens behind the comparison, kept for that test.
    #[serde(skip)]
    pub cache_write_tokens: f64,
}

impl Check {
    /// Far enough apart, and on enough money, to be worth saying.
    pub fn disagrees(&self) -> bool {
        self.priced && self.vendor >= MIN_DOLLARS && self.samples >= MIN_SAMPLES && self.diff_percent.abs() >= AGREE_PERCENT
    }
}

/// Pull every model's figures out of one log line. A line that is not a
/// `cost-state`, or whose own totals the vendor could not price, yields none.
pub fn parse_cost_state(line: &str) -> Vec<ModelCost> {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    if doc.get("type").and_then(|t| t.as_str()) != Some("cost-state") {
        return Vec::new();
    }
    // The vendor met a model it could not price, so its own total is partial.
    if doc.get("hasUnknownModelCost").and_then(|v| v.as_bool()) == Some(true) {
        return Vec::new();
    }
    let Some(usage) = doc.get("modelUsage").and_then(|m| m.as_object()) else {
        return Vec::new();
    };
    let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    usage
        .iter()
        .filter_map(|(model, u)| {
            let vendor_cost = u.get("costUSD").and_then(|c| c.as_f64())?;
            Some(ModelCost {
                model: model.clone(),
                input: num(u, "inputTokens"),
                output: num(u, "outputTokens"),
                cache_read: num(u, "cacheReadInputTokens"),
                cache_write: num(u, "cacheCreationInputTokens"),
                web_searches: num(u, "webSearchRequests") as u64,
                vendor_cost,
            })
        })
        .collect()
}

/// What this app's rate card charges for one sample's tokens.
pub fn our_cost(price: &Price, m: &ModelCost) -> f64 {
    (m.input * price.input
        + m.output * price.output
        + m.cache_read * price.cache_read
        + m.cache_write * price.cache_write)
        / 1_000_000.0
}

/// Compare samples against a price source, one row per model.
pub fn compare(samples: &[ModelCost], price_of: impl Fn(&str) -> Option<Price>) -> Vec<Check> {
    let mut rows: Vec<Check> = Vec::new();
    for s in samples {
        // Per-search billing is not a token rate; including it invents drift.
        if s.web_searches > 0 {
            continue;
        }
        let price = price_of(&s.model);
        let ours = price.as_ref().map(|p| our_cost(p, s)).unwrap_or(0.0);
        match rows.iter_mut().find(|r| r.model == s.model) {
            Some(r) => {
                r.ours += ours;
                r.vendor += s.vendor_cost;
                r.samples += 1;
            }
            None => rows.push(Check {
                model: s.model.clone(),
                ours,
                vendor: s.vendor_cost,
                diff_percent: 0.0,
                samples: 1,
                priced: price.is_some(),
                one_hour_cache: false,
                cache_write_tokens: s.cache_write,
            }),
        }
        if let Some(r) = rows.iter_mut().find(|r| r.model == s.model) {
            if r.samples > 1 {
                r.cache_write_tokens += s.cache_write;
            }
        }
    }
    for r in rows.iter_mut() {
        r.diff_percent = if r.vendor > 0.0 { (r.ours - r.vendor) / r.vendor * 100.0 } else { 0.0 };
        // Does charging the 1-hour cache rate close the gap exactly?
        if let Some(p) = price_of(&r.model) {
            let one_hour = p.cache_write_1h.unwrap_or(p.input * 2.0);
            let extra = r.cache_write_tokens * (one_hour - p.cache_write) / 1_000_000.0;
            if extra > 0.0 && r.vendor > 0.0 && ((r.ours + extra) - r.vendor).abs() / r.vendor < 0.01 {
                r.one_hour_cache = true;
            }
        }
        r.ours += 0.0;
        r.vendor += 0.0;
    }
    rows.sort_by(|a, b| b.vendor.total_cmp(&a.vendor));
    rows
}

/// The test-side key registry (see inventory::FINDING_IDS): every finding id
/// this module can emit. Only `i18n.rs`'s test module reads this, so it does
/// not exist in a release build at all.
#[cfg(test)]
pub(crate) const FINDING_IDS: &[&str] = &["pricing-cache-ttl", "pricing-drift"];

/// What the comparison is worth telling the user. Agreement says nothing.
pub fn opportunities(checks: &[Check]) -> Vec<Opportunity> {
    let off: Vec<&Check> = checks.iter().filter(|c| c.disagrees()).collect();
    if off.is_empty() {
        return Vec::new();
    }
    let worst = off.iter().max_by(|a, b| a.diff_percent.abs().total_cmp(&b.diff_percent.abs())).expect("non-empty");
    let names = off.iter().map(|c| c.model.as_str()).collect::<Vec<_>>().join(", ");
    // A tiny reusable Msg rather than a plain English var: "more"/"less" is
    // meaningful prose, not a format token, so it needs to translate too.
    let direction = Msg::new(if worst.diff_percent > 0.0 { "unit.more" } else { "unit.less" });
    // When the arithmetic lands on the 1-hour cache rate to within a cent on
    // the dollar, the cause is known and worth naming instead of hedging.
    if off.iter().all(|c| c.one_hour_cache) {
        let total_ours: f64 = off.iter().map(|c| c.ours).sum::<f64>() + 0.0;
        let total_vendor: f64 = off.iter().map(|c| c.vendor).sum::<f64>() + 0.0;
        return vec![Opportunity::from_msgs(
            "pricing-cache-ttl",
            "tighten",
            Msg::new("finding.pricing-cache-ttl.title")
                .var("pct", format!("{:.0}", (total_vendor - total_ours) / total_vendor * 100.0)),
            Some(
                Msg::new("finding.pricing-cache-ttl.detail")
                    .var("totalVendor", format!("{total_vendor:.2}"))
                    .var("totalOurs", format!("{total_ours:.2}"))
                    .var("names", &names),
            ),
            Some(PRICING_LEARN),
        )];
    }
    vec![Opportunity::from_msgs(
        "pricing-drift",
        "tighten",
        Msg::new("finding.pricing-drift.title").count(off.len() as i64),
        Some(
            Msg::new("finding.pricing-drift.detail")
                .var("ours", format!("{:.2}", worst.ours))
                .var("vendor", format!("{:.2}", worst.vendor))
                .var("model", &worst.model)
                .var("pct", format!("{:.0}", worst.diff_percent.abs()))
                .sub("direction", direction)
                .var("names", &names),
        ),
        Some(PRICING_LEARN),
    )]
}

/// Session logs, newest first, capped.
fn recent_session_files() -> Vec<PathBuf> {
    let root = std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().join(".claude"))
        .join("projects");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let Ok(projects) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    for project in projects.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "jsonl") {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.len() > MAX_FILE_BYTES {
                continue;
            }
            files.push((meta.modified().unwrap_or(std::time::UNIX_EPOCH), path));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(MAX_FILES);
    files.into_iter().map(|(_, p)| p).collect()
}

/// Read the recent logs for cost references and compare them with the
/// catalogue. Cheap: a substring test rejects almost every line before it is
/// ever parsed as JSON.
pub fn scan() -> Vec<Check> {
    let mut samples = Vec::new();
    for path in recent_session_files() {
        let Ok(file) = std::fs::File::open(&path) else { continue };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("\"cost-state\"") {
                continue;
            }
            samples.extend(parse_cost_state(&line));
        }
    }
    compare(&samples, |m| pricing::lookup(m))
}

/// Is the gap a stale rate, or a token this comparison is not counting?
/// Solves the vendor's own figure against the catalogue both ways.
#[test]
#[ignore]
fn live_pricing_diagnose() {
    pricing::ensure_fresh();
    let mut samples = Vec::new();
    for path in recent_session_files() {
        let Ok(file) = std::fs::File::open(&path) else { continue };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if line.contains("\"cost-state\"") {
                for raw in parse_cost_state(&line) {
                    let thinking = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|d| {
                            d.get("modelUsage")?.get(&raw.model)?.get("thinkingTokens")?.as_f64()
                        })
                        .unwrap_or(0.0);
                    samples.push((raw, thinking));
                }
            }
        }
    }
    for (m, thinking) in &samples {
        let Some(p) = pricing::lookup(&m.model) else { continue };
        let plain = our_cost(&p, m);
        let with_thinking = our_cost(&p, &ModelCost { output: m.output + thinking, ..m.clone() });
        println!(
            "{:<28} in={:>9} out={:>7} think={:>6} cr={:>9} cw={:>8}\n  rates in={} out={} cr={} cw={}\n  vendor=${:.5} ours=${:.5} (+think ${:.5})",
            m.model, m.input, m.output, thinking, m.cache_read, m.cache_write,
            p.input, p.output, p.cache_read, p.cache_write,
            m.vendor_cost, plain, with_thinking
        );
        // What output rate would the vendor's number imply, holding the rest?
        let rest = (m.input * p.input + m.cache_read * p.cache_read + m.cache_write * p.cache_write) / 1e6;
        if m.output > 0.0 {
            println!("  implied output rate if the rest is right: ${:.2}/M", (m.vendor_cost - rest) * 1e6 / m.output);
        }
    }
}

/// Prints the real comparison for this machine. Ignored: it reads the logs.
#[test]
#[ignore]
fn live_pricing_check() {
    pricing::ensure_fresh();
    let rows = scan();
    println!("{} model(s) with a vendor cost reference", rows.len());
    for r in &rows {
        println!(
            "  {:<32} ours ${:>8.4}  vendor ${:>8.4}  {:+7.1}%  n={}  {}",
            r.model,
            r.ours,
            r.vendor,
            r.diff_percent,
            r.samples,
            if r.priced { "" } else { "NOT IN CATALOGUE" }
        );
    }
    for o in opportunities(&rows) {
        println!("\nFINDING: {}\n{}", o.title, o.detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(input: f64, output: f64, cache_read: f64, cache_write: f64) -> Price {
        Price {
            input,
            output,
            cache_read,
            cache_write,
            input_200k: None,
            output_200k: None,
            cache_read_200k: None,
            cache_write_200k: None,
            cache_write_1h: None,
            cache_write_1h_200k: None,
        }
    }

    const LINE: &str = r#"{"type":"cost-state","hasUnknownModelCost":false,"totalCostUSD":1.0,
        "modelUsage":{"claude-haiku-4-5":{"inputTokens":26180,"outputTokens":1478,
        "cacheReadInputTokens":0,"cacheCreationInputTokens":0,"webSearchRequests":0,"costUSD":0.03357}}}"#;

    #[test]
    fn reads_a_cost_state_line() {
        let got = parse_cost_state(LINE);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model, "claude-haiku-4-5");
        assert_eq!(got[0].input, 26180.0);
        assert_eq!(got[0].vendor_cost, 0.03357);
    }

    #[test]
    fn ignores_other_lines_and_partial_totals() {
        assert!(parse_cost_state(r#"{"type":"assistant","message":{}}"#).is_empty());
        assert!(parse_cost_state("not json").is_empty());
        let unknown = LINE.replace("\"hasUnknownModelCost\":false", "\"hasUnknownModelCost\":true");
        assert!(parse_cost_state(&unknown).is_empty(), "the vendor's own total was partial");
    }

    /// The same session a thousand times over, so the dollars clear the
    /// floor. Every figure scales together, so the correct rate still agrees.
    fn big() -> String {
        LINE.replace("26180", "26180000").replace("1478", "1478000").replace("0.03357", "33.57")
    }

    #[test]
    fn agreement_is_not_a_finding() {
        // $1/M in, $5/M out reproduces the vendor's figure exactly.
        let checks = compare(&parse_cost_state(LINE), |_| Some(price(1.0, 5.0, 0.0, 0.0)));
        assert_eq!(checks.len(), 1);
        assert!(checks[0].diff_percent.abs() < 0.01, "{:?}", checks[0]);
        assert!(opportunities(&checks).is_empty());
    }

    #[test]
    fn a_stale_rate_is_caught_once_it_repeats_and_matters() {
        let one = parse_cost_state(&big());
        // Scaled up, the correct rate still agrees exactly.
        let exact = compare(&one, |_| Some(price(1.0, 5.0, 0.0, 0.0)));
        assert!(exact[0].diff_percent.abs() < 0.01, "{:?}", exact[0]);
        let samples: Vec<ModelCost> = one.iter().chain(one.iter()).cloned().collect();
        // Twice the real input rate: input dominates, so about +78%.
        let checks = compare(&samples, |_| Some(price(2.0, 5.0, 0.0, 0.0)));
        assert!(checks[0].diff_percent > 70.0, "{:?}", checks[0]);
        assert!(checks[0].disagrees());
        let found = opportunities(&checks);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "pricing-drift");
        assert!(found[0].detail.contains("claude-haiku-4-5"));
    }

    #[test]
    fn one_sample_is_never_enough() {
        let checks = compare(&parse_cost_state(&big()), |_| Some(price(2.0, 5.0, 0.0, 0.0)));
        assert_eq!(checks[0].samples, 1);
        assert!(!checks[0].disagrees(), "one odd session proves nothing");
    }

    #[test]
    fn small_change_is_never_a_finding() {
        let one = parse_cost_state(&big());
        let samples: Vec<ModelCost> = one.iter().chain(one.iter()).cloned().collect();
        // 1% over: inside the agreement band.
        let checks = compare(&samples, |_| Some(price(1.01, 5.05, 0.0, 0.0)));
        assert!(!checks[0].disagrees(), "{:?}", checks[0]);
    }

    #[test]
    fn a_gap_that_is_exactly_the_one_hour_cache_rate_is_named() {
        // 5-minute writes at $12.50/M, 1-hour at 2x input = $20/M. Build a
        // vendor figure that is our number plus exactly that difference.
        let p = price(10.0, 50.0, 0.25, 12.5);
        let m = ModelCost {
            model: "claude-x".into(),
            input: 35.0,
            output: 11950.0,
            cache_read: 1_086_452.0,
            cache_write: 67_209.0,
            web_searches: 0,
            vendor_cost: 0.0,
        };
        let ours = our_cost(&p, &m);
        let vendor = ours + m.cache_write * (20.0 - 12.5) / 1_000_000.0;
        let sample = ModelCost { vendor_cost: vendor, ..m };
        let checks = compare(&[sample.clone(), sample], |_| Some(p));
        assert!(checks[0].one_hour_cache, "{:?}", checks[0]);
        assert!(checks[0].disagrees());
        let found = opportunities(&checks);
        assert_eq!(found[0].id, "pricing-cache-ttl");
        assert!(found[0].title.contains("1-hour cache writes"), "{}", found[0].title);
        assert!(found[0].detail.contains("floor, not a ceiling"));
        assert!(!found[0].detail.contains("  "), "no collapsed line continuations");
    }

    #[test]
    fn a_gap_the_cache_rate_cannot_explain_stays_a_plain_disagreement() {
        let p = price(10.0, 50.0, 0.25, 12.5);
        let m = ModelCost {
            model: "claude-x".into(),
            input: 35.0,
            output: 11950.0,
            cache_read: 1_086_452.0,
            cache_write: 67_209.0,
            web_searches: 0,
            vendor_cost: 0.0,
        };
        // Twice as far off as the cache rate could account for.
        let vendor = our_cost(&p, &m) + m.cache_write * (20.0 - 12.5) / 1_000_000.0 * 3.0;
        let sample = ModelCost { vendor_cost: vendor, ..m };
        let checks = compare(&[sample.clone(), sample], |_| Some(p));
        assert!(!checks[0].one_hour_cache, "{:?}", checks[0]);
        assert_eq!(opportunities(&checks)[0].id, "pricing-drift");
    }

    #[test]
    fn web_search_samples_are_left_out() {
        let searched = LINE.replace("\"webSearchRequests\":0", "\"webSearchRequests\":2");
        let checks = compare(&parse_cost_state(&searched), |_| Some(price(1.0, 5.0, 0.0, 0.0)));
        assert!(checks.is_empty(), "per-search billing is not a token rate");
    }

    /// A missing translation key renders as its own literal key text instead
    /// of failing -- that takes a real fixture run to catch. Covers both
    /// finding ids: the cache-TTL explanation and a plain drift.
    #[test]
    fn opportunities_never_render_a_raw_key() {
        let p = price(10.0, 50.0, 0.25, 12.5);
        let m = ModelCost {
            model: "claude-x".into(),
            input: 35.0,
            output: 11950.0,
            cache_read: 1_086_452.0,
            cache_write: 67_209.0,
            web_searches: 0,
            vendor_cost: 0.0,
        };
        let ours = our_cost(&p, &m);
        let cache_ttl_vendor = ours + m.cache_write * (20.0 - 12.5) / 1_000_000.0;
        let cache_ttl = compare(
            &[ModelCost { vendor_cost: cache_ttl_vendor, ..m.clone() }, ModelCost { vendor_cost: cache_ttl_vendor, ..m }],
            |_| Some(p),
        );
        // Same shape as a_stale_rate_is_caught_once_it_repeats_and_matters:
        // a real vendor_cost from the fixture log line, read back at twice
        // the correct input rate.
        let samples: Vec<ModelCost> = parse_cost_state(&big()).into_iter().chain(parse_cost_state(&big())).collect();
        let plain_drift = compare(&samples, |_| Some(price(2.0, 5.0, 0.0, 0.0)));

        for checks in [cache_ttl, plain_drift] {
            let found = opportunities(&checks);
            assert!(!found.is_empty());
            for o in found {
                assert!(!o.title.starts_with("finding.") && !o.title.starts_with("unit."), "{}: raw key in title: {}", o.id, o.title);
                assert!(!o.detail.starts_with("finding.") && !o.detail.contains("unit."), "{}: raw key in detail: {}", o.id, o.detail);
            }
        }
    }

    #[test]
    fn an_unpriced_model_is_reported_but_never_as_drift() {
        let checks = compare(&parse_cost_state(LINE), |_| None);
        assert!(!checks[0].priced);
        assert_eq!(checks[0].ours, 0.0);
        assert!(!checks[0].disagrees(), "we cannot disagree with a price we do not have");
        assert!(opportunities(&checks).is_empty());
    }
}
