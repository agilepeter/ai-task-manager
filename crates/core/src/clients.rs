//! Clients: a name over one or more work areas, so spend rolls up to the
//! thing it gets billed or justified against.
//!
//! Rules are the user's own, kept in `clients.json` beside the ledger. They
//! never leave the machine, and neither do the area names they match.

use crate::spend::{AreaSpend, Window, TREND_DAYS};
use chrono::{Datelike, Days, NaiveDate};
use serde::{Deserialize, Serialize};
use std::path::Path;

const MAX_RULES: usize = 100;
const MAX_PATTERNS: usize = 20;
const MAX_TEXT: usize = 96;
pub const UNASSIGNED: &str = "Unassigned";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClientRule {
    pub client: String,
    /// Area patterns. `*` matches any run of characters; a pattern with no
    /// `*` matches that folder and everything beneath it.
    pub patterns: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct ClientsFile {
    rules: Vec<ClientRule>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClientSpend {
    pub client: String,
    pub today: Window,
    pub yesterday: Window,
    pub last30: Window,
    /// This calendar month so far, from the daily series.
    pub month_to_date: f64,
    /// The areas that rolled up here, largest first.
    pub areas: Vec<String>,
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// `*` wildcard match, ASCII case-insensitive.
fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let t: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

pub fn pattern_matches(pattern: &str, area: &str) -> bool {
    let pattern = pattern.trim().trim_matches('/');
    if pattern.is_empty() {
        return false;
    }
    if pattern.contains('*') {
        return glob(pattern, area);
    }
    // A plain folder owns itself and whatever is under it.
    glob(pattern, area) || glob(&format!("{pattern}/*"), area)
}

/// The first rule with a matching pattern wins, so order is priority.
pub fn client_of<'a>(area: &str, rules: &'a [ClientRule]) -> Option<&'a str> {
    rules
        .iter()
        .find(|r| r.patterns.iter().any(|p| pattern_matches(p, area)))
        .map(|r| r.client.as_str())
}

// ---------------------------------------------------------------------------
// Rollup
// ---------------------------------------------------------------------------

fn add(into: &mut Window, from: &Window) {
    into.cost += from.cost;
    into.tokens += from.tokens;
}

/// Sum of the daily series over the days that fall in `today`'s month.
/// `daily` is oldest first with today last, like every trend in the app.
fn month_to_date(daily: &[f64], today: NaiveDate) -> f64 {
    let n = daily.len();
    daily
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            today
                .checked_sub_days(Days::new((n - 1 - i) as u64))
                .is_some_and(|d| d.month() == today.month() && d.year() == today.year())
        })
        .map(|(_, c)| c)
        .sum()
}

/// Areas grouped by client, largest 30-day cost first, `Unassigned` last.
/// Every dollar lands in exactly one row.
pub fn rollup(areas: &[AreaSpend], rules: &[ClientRule], today: NaiveDate) -> Vec<ClientSpend> {
    let mut out: Vec<ClientSpend> = Vec::new();
    let mut sorted: Vec<&AreaSpend> = areas.iter().collect();
    sorted.sort_by(|a, b| b.last30.cost.total_cmp(&a.last30.cost));
    for area in sorted {
        let name = client_of(&area.area, rules).unwrap_or(UNASSIGNED);
        let row = match out.iter_mut().find(|c| c.client == name) {
            Some(row) => row,
            None => {
                out.push(ClientSpend {
                    client: name.to_string(),
                    today: Window::default(),
                    yesterday: Window::default(),
                    last30: Window::default(),
                    month_to_date: 0.0,
                    areas: Vec::new(),
                });
                out.last_mut().expect("just pushed")
            }
        };
        add(&mut row.today, &area.today);
        add(&mut row.yesterday, &area.yesterday);
        add(&mut row.last30, &area.last30);
        row.month_to_date += month_to_date(&area.daily_cost, today);
        row.areas.push(area.area.clone());
    }
    out.sort_by(|a, b| {
        (a.client == UNASSIGNED)
            .cmp(&(b.client == UNASSIGNED))
            .then(b.last30.cost.total_cmp(&a.last30.cost))
    });
    out
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// One CSV cell. Quoted when it must be, and a leading `= + - @` (or tab /
/// CR) is defused with an apostrophe: area and client names are free text,
/// and a spreadsheet would otherwise run them as formulas.
fn cell(text: &str) -> String {
    let risky = text.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let text = if risky { format!("'{text}") } else { text.to_string() };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

pub fn csv(rows: &[ClientSpend], today: NaiveDate) -> String {
    let mut out = String::from(
        "Client,Month to date (USD),Last 30 days (USD),Last 30 days (tokens),Today (USD),Areas\r\n",
    );
    for r in rows {
        out.push_str(&format!(
            "{},{:.2},{:.2},{:.0},{:.2},{}\r\n",
            cell(&r.client),
            r.month_to_date,
            r.last30.cost,
            r.last30.tokens,
            r.today.cost,
            cell(&r.areas.join("; ")),
        ));
    }
    out.push_str(&format!(
        "\r\n{}\r\n",
        cell(&format!(
            "Generated {today}. From local Claude Code logs, priced at API rates: on a flat-rate plan this is equivalent value, not a charge. The daily series covers {TREND_DAYS} days, so month to date is partial once a month runs longer than that."
        ))
    ));
    out
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

pub fn validate(rules: Vec<ClientRule>) -> Result<Vec<ClientRule>, String> {
    if rules.len() > MAX_RULES {
        return Err("Too many clients.".into());
    }
    let mut out = Vec::new();
    for rule in rules {
        let client: String = rule.client.trim().chars().take(MAX_TEXT).collect();
        let patterns: Vec<String> = rule
            .patterns
            .iter()
            .map(|p| p.trim().trim_matches('/').chars().take(MAX_TEXT).collect::<String>())
            .filter(|p| !p.is_empty())
            .take(MAX_PATTERNS)
            .collect();
        if client.is_empty() && patterns.is_empty() {
            continue; // an untouched blank row
        }
        if client.is_empty() {
            return Err("Every rule needs a client name.".into());
        }
        if client.eq_ignore_ascii_case(UNASSIGNED) {
            return Err(format!("\"{UNASSIGNED}\" is reserved for spend no rule matches."));
        }
        if patterns.is_empty() {
            return Err(format!("{client} needs at least one folder pattern."));
        }
        out.push(ClientRule { client, patterns });
    }
    Ok(out)
}

pub fn load_from(path: &Path) -> Vec<ClientRule> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ClientsFile>(&raw).ok())
        .map(|f| f.rules)
        .unwrap_or_default()
}

pub fn save_to(path: &Path, rules: Vec<ClientRule>) -> Result<Vec<ClientRule>, String> {
    let rules = validate(rules)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create settings folder: {e}"))?;
    }
    let body = serde_json::to_string_pretty(&ClientsFile { rules: rules.clone() })
        .map_err(|e| format!("encode clients: {e}"))?;
    crate::providers::onenewapi::store::atomic_write(path, &body)?;
    Ok(rules)
}

pub fn path() -> std::path::PathBuf {
    crate::providers::config_dir().join("clients.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(cost: f64) -> Window {
        Window { cost, tokens: cost * 10.0, models: Vec::new() }
    }

    fn area(name: &str, last30: f64, today: f64, daily_tail: &[f64]) -> AreaSpend {
        let mut daily = vec![0.0; TREND_DAYS];
        let start = TREND_DAYS - daily_tail.len();
        daily[start..].copy_from_slice(daily_tail);
        AreaSpend { area: name.into(), today: w(today), yesterday: w(0.0), last30: w(last30), daily_cost: daily }
    }

    fn rule(client: &str, patterns: &[&str]) -> ClientRule {
        ClientRule { client: client.into(), patterns: patterns.iter().map(|p| p.to_string()).collect() }
    }

    #[test]
    fn a_plain_folder_owns_what_is_under_it_and_a_star_is_a_wildcard() {
        assert!(pattern_matches("acme", "acme"));
        assert!(pattern_matches("acme", "acme/web"));
        assert!(!pattern_matches("acme", "acme-labs"), "a folder is not a prefix");
        assert!(pattern_matches("site/acme*", "site/acme-portal"));
        assert!(!pattern_matches("site/acme*", "site/beta"));
        assert!(pattern_matches("*/reports", "site/reports"));
        assert!(pattern_matches("/Site/ACME/", "site/acme"), "slashes and case are forgiven");
        assert!(!pattern_matches("", "anything"));
        assert!(!pattern_matches("   ", "anything"));
    }

    #[test]
    fn the_first_matching_rule_wins() {
        let rules = [rule("Acme", &["site/acme*"]), rule("House", &["site", "tools"])];
        assert_eq!(client_of("site/acme-portal", &rules), Some("Acme"));
        assert_eq!(client_of("site/blog", &rules), Some("House"));
        assert_eq!(client_of("tools", &rules), Some("House"));
        assert_eq!(client_of("elsewhere", &rules), None);
    }

    #[test]
    fn every_dollar_lands_in_exactly_one_client_row() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 3).unwrap();
        let areas = [
            // daily tail: ..., Aug 31, Sep 1, Sep 2, Sep 3 (today)
            area("site/acme-portal", 100.0, 5.0, &[40.0, 10.0, 20.0, 5.0]),
            area("site/acme-api", 50.0, 0.0, &[0.0, 0.0, 7.0, 0.0]),
            area("tools", 30.0, 1.0, &[0.0, 0.0, 0.0, 1.0]),
            area("(unsorted)", 20.0, 0.0, &[]),
        ];
        let rows = rollup(&areas, &[rule("Acme", &["site/acme*"])], today);
        assert_eq!(rows.iter().map(|r| r.client.as_str()).collect::<Vec<_>>(), ["Acme", UNASSIGNED]);
        assert_eq!(rows[0].last30.cost, 150.0);
        assert_eq!(rows[0].today.cost, 5.0);
        assert_eq!(rows[0].month_to_date, 42.0, "Sep 1-3 only: Aug 31's 40 is last month");
        assert_eq!(rows[0].areas, ["site/acme-portal", "site/acme-api"]);
        assert_eq!(rows[1].last30.cost, 50.0);
        let total: f64 = rows.iter().map(|r| r.last30.cost).sum();
        assert_eq!(total, areas.iter().map(|a| a.last30.cost).sum::<f64>());
        // Unassigned sorts last even when it is the biggest.
        let big = [area("misc", 999.0, 0.0, &[]), area("site/acme-x", 1.0, 0.0, &[])];
        let rows = rollup(&big, &[rule("Acme", &["site/acme*"])], today);
        assert_eq!(rows.last().unwrap().client, UNASSIGNED);
    }

    #[test]
    fn csv_cells_are_quoted_and_cannot_run_as_formulas() {
        assert_eq!(cell("Acme"), "Acme");
        assert_eq!(cell("Acme, Inc"), "\"Acme, Inc\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("=HYPERLINK(\"http://x\")"), "\"'=HYPERLINK(\"\"http://x\"\")\"");
        assert_eq!(cell("-2+3"), "'-2+3");
        assert_eq!(cell("@cmd"), "'@cmd");

        let today = NaiveDate::from_ymd_opt(2026, 9, 3).unwrap();
        let rows = rollup(&[area("=evil", 12.5, 1.0, &[1.0])], &[], today);
        let out = csv(&rows, today);
        assert!(out.starts_with("Client,Month to date (USD)"));
        assert!(out.contains("Unassigned,1.00,12.50,125,1.00,'=evil\r\n"), "{out}");
        assert!(out.contains("equivalent value, not a charge"));
    }

    #[test]
    fn rules_are_cleaned_blank_rows_dropped_and_bad_ones_refused() {
        let ok = validate(vec![
            rule("  Acme  ", &[" /site/acme*/ ", "  "]),
            rule("", &[]), // an untouched blank row in the editor
        ])
        .unwrap();
        assert_eq!(ok, [rule("Acme", &["site/acme*"])]);
        assert!(validate(vec![rule("", &["x"])]).is_err());
        assert!(validate(vec![rule("Acme", &[])]).is_err());
        assert!(validate(vec![rule("unassigned", &["x"])]).is_err(), "the reserved name");
    }

    #[test]
    fn rules_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("aitm-clients-{}", crate::providers::unique_stamp()));
        let path = dir.join("clients.json");
        assert!(load_from(&path).is_empty());
        save_to(&path, vec![rule("Acme", &["site/acme*"])]).unwrap();
        assert_eq!(load_from(&path), [rule("Acme", &["site/acme*"])]);
        assert!(save_to(&path, vec![rule("", &["x"])]).is_err());
        assert_eq!(load_from(&path).len(), 1, "a refused save leaves the file alone");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
