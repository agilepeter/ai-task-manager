//! Agent budgets and the runaway rule.
//!
//! Two things the user can set, both off until they set them:
//!
//! - **A monthly budget per agent name**, on the calendar month, alerting once
//!   a month per agent by the same rule the client budgets use
//!   (`clients::over_budget`). Month to date comes from the per-day agent
//!   spend the scan already keeps, so nothing about the scan changes.
//! - **One live rule on running agents**: a session whose ten-minute pace,
//!   carried over an hour, reaches a dollar figure; or that has been open
//!   longer than a number of minutes and is still working.
//!
//! The app tells; it never ends, pauses or changes an agent. Nothing here acts
//! on a process, and a figure the user has not set is never guessed.
//!
//! The file, `agent_watch.json`, holds agent names the user may have chosen to
//! match a client, so it stays on this machine: `seat.rs` and
//! `httpapi::agents_feed` never read it, and a finding's *title* carries a
//! count only (titles reach the seat report; details do not).

use crate::alerts::Alert;
use crate::i18n::Msg;
use crate::inventory::Opportunity;
use crate::procs::RunningAgent;
use crate::spend::{AgentSpend, LivePace};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Every finding id this module can emit (see `inventory::FINDING_IDS`).
#[cfg(test)]
pub(crate) const FINDING_IDS: &[&str] = &["agent-over-budget", "agent-runaway"];

/// The translatable errors `save_to` can return.
#[cfg(test)]
pub(crate) const ERROR_KEYS: &[&str] = &[
    "error.agentWatch.figure",
    "error.agentWatch.tooMany",
    "error.agentWatch.pick",
    "error.agentWatch.duplicate",
    "error.agentWatch.write",
];

/// The same watch as the webview sends it. The one figure that must be whole,
/// `maxMinutes`, arrives as a plain number so that a fraction is refused by
/// `into_watch` in the user's language rather than by serde in English.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WatchInput {
    #[serde(default)]
    pub budgets: Vec<AgentBudget>,
    #[serde(default)]
    pub live: LiveInput,
}

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveInput {
    pub hourly_pace_usd: Option<f64>,
    pub max_minutes: Option<f64>,
}

impl WatchInput {
    /// A whole number of minutes above zero, or `error.agentWatch.figure`.
    pub fn into_watch(self) -> Result<Watch, Msg> {
        let max_minutes = match self.live.max_minutes {
            None => None,
            Some(m) if m.is_finite() && m.fract() == 0.0 && (1.0..=1e9).contains(&m) => Some(m as u64),
            Some(_) => return Err(Msg::new("error.agentWatch.figure")),
        };
        Ok(Watch { budgets: self.budgets, live: LiveRule { hourly_pace_usd: self.live.hourly_pace_usd, max_minutes } })
    }
}

/// The notification keys this module's alerts and the budget notification in
/// `src-tauri/src/lib.rs` use.
#[cfg(test)]
pub(crate) const NOTIFY_KEYS: &[&str] = &[
    "notify.agentBudget.title",
    "notify.agentBudget.body",
    "notify.agentRunaway.title",
    "notify.agentRunaway.pace",
    "notify.agentRunaway.duration",
];

pub const MAX_BUDGETS: usize = 50;
/// A name longer than this is not an agent name; a hand-edited file may hold one.
const MAX_NAME_CHARS: usize = 80;
/// Keys of the remembered "already said" marks in `alert_marks.json`.
const BUDGET_MARKS: &str = "agentBudgetFired";
const RUNAWAY_MARKS: &str = "runawayFired";
/// A pace is cost in the last ten minutes carried over an hour.
const PACE_FACTOR: f64 = 6.0;
/// Seconds without a new line, with nothing in ten minutes, after which a
/// session is idle. The Running now row in `src/inventory.ts` says the same.
const IDLE_SECS: u64 = 60;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentBudget {
    pub agent: String,
    /// Dollars for a calendar month.
    pub monthly_budget: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveRule {
    /// Dollars an hour, at the pace of the last ten minutes.
    pub hourly_pace_usd: Option<f64>,
    /// Minutes a host may be open while still working.
    pub max_minutes: Option<u64>,
}

/// Everything the user has set. The default is nothing set, and nothing set
/// means nothing is computed, nothing fires and no finding appears.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Watch {
    pub budgets: Vec<AgentBudget>,
    pub live: LiveRule,
}

impl Watch {
    pub fn live_is_set(&self) -> bool {
        self.live.hourly_pace_usd.is_some() || self.live.max_minutes.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.budgets.is_empty() && !self.live_is_set()
    }
}

fn good_figure(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

pub fn path() -> PathBuf {
    crate::providers::config_dir().join("agent_watch.json")
}

/// A missing or broken file is "nothing set", never an error. The file is read
/// on every refresh and may have been edited by hand, so it is bounded: each
/// entry is parsed on its own (one malformed entry does not cost the rest), an
/// empty or over-long name is dropped, a repeated name keeps its first row, at
/// most `MAX_BUDGETS` rows are kept, and a figure that could not have been
/// saved is dropped rather than acted on.
pub fn load_from(path: &Path) -> Watch {
    let Some(root) = std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str::<Value>(&raw).ok()) else {
        return Watch::default();
    };
    let mut w = Watch::default();
    if let Some(list) = root.get("budgets").and_then(Value::as_array) {
        for entry in list {
            let Ok(b) = serde_json::from_value::<AgentBudget>(entry.clone()) else { continue };
            let ok = !b.agent.is_empty()
                && b.agent.chars().count() <= MAX_NAME_CHARS
                && good_figure(b.monthly_budget)
                && !w.budgets.iter().any(|seen| seen.agent == b.agent);
            if ok {
                w.budgets.push(b);
            }
            if w.budgets.len() >= MAX_BUDGETS {
                break;
            }
        }
    }
    let live = root.get("live");
    w.live.hourly_pace_usd = live.and_then(|l| l.get("hourlyPaceUsd")).and_then(Value::as_f64).filter(|x| good_figure(*x));
    w.live.max_minutes = live.and_then(|l| l.get("maxMinutes")).and_then(Value::as_u64).filter(|m| *m > 0);
    w
}

/// The names a budget may take: every built-in agent, every agent defined on
/// this machine, every named agent with spend, and every name already saved.
/// The last keeps a budget whose agent has since been deleted both listed and
/// editable, so a stale row can never block saving the others. Sorted, no
/// duplicates, and never the empty unattributed row.
pub fn known_names(defined: &[String], spend: &[AgentSpend], saved: &Watch) -> Vec<String> {
    let mut names: Vec<String> = crate::seat::BUILTIN_AGENTS.iter().map(|s| s.to_string()).collect();
    names.extend(defined.iter().cloned());
    names.extend(saved.budgets.iter().map(|b| b.agent.clone()));
    names.extend(spend.iter().map(|a| a.name.clone()));
    names.retain(|n| !n.is_empty());
    names.sort();
    names.dedup();
    names
}

fn validate(w: Watch, known: &[String]) -> Result<Watch, Msg> {
    if w.budgets.len() > MAX_BUDGETS {
        return Err(Msg::new("error.agentWatch.tooMany").var("max", MAX_BUDGETS));
    }
    let mut seen: Vec<&str> = Vec::new();
    for b in &w.budgets {
        if b.agent.is_empty() || !known.contains(&b.agent) {
            return Err(Msg::new("error.agentWatch.pick"));
        }
        if seen.contains(&b.agent.as_str()) {
            return Err(Msg::new("error.agentWatch.duplicate"));
        }
        seen.push(&b.agent);
        if !good_figure(b.monthly_budget) {
            return Err(Msg::new("error.agentWatch.figure"));
        }
    }
    if w.live.hourly_pace_usd.is_some_and(|x| !good_figure(x)) || w.live.max_minutes == Some(0) {
        return Err(Msg::new("error.agentWatch.figure"));
    }
    Ok(w)
}

/// Validates, then writes owner-only through a temp file and a rename.
pub fn save_to(path: &Path, w: Watch, known: &[String]) -> Result<Watch, Msg> {
    let w = validate(w, known)?;
    // The shared writer's own messages are English and name a credential file,
    // so the user gets a fixed translated line and the detail goes to the log.
    let failed = |detail: String| {
        eprintln!("[aitm] agent watch: could not save agent_watch.json: {detail}");
        Msg::new("error.agentWatch.write")
    };
    let body = serde_json::to_string_pretty(&w).map_err(|e| failed(e.to_string()))?;
    crate::providers::onenewapi::store::atomic_write(path, &body).map_err(failed)?;
    Ok(w)
}

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BudgetRow {
    pub agent: String,
    pub month_to_date: f64,
    pub monthly_budget: f64,
}

impl BudgetRow {
    fn is_over(&self) -> bool {
        self.month_to_date >= self.monthly_budget
    }
}

/// One row per budget, in the order the budgets were saved. An agent with no
/// spend this month still has a row, at zero: a budget never disappears.
pub fn budget_rows(month_rows: &[AgentSpend], w: &Watch) -> Vec<BudgetRow> {
    w.budgets
        .iter()
        .map(|b| BudgetRow {
            agent: b.agent.clone(),
            month_to_date: month_rows.iter().find(|r| r.name == b.agent).map_or(0.0, |r| r.cost),
            monthly_budget: b.monthly_budget,
        })
        .collect()
}

/// The rows at or over their budget right now: a state, shown for as long as
/// it holds.
pub fn currently_over(rows: &[BudgetRow]) -> Vec<BudgetRow> {
    rows.iter().filter(|r| r.is_over()).cloned().collect()
}

/// Rows that have newly passed their budget and have not alerted this month:
/// an edge, for the notification. `fired` holds "agent|YYYY-MM" marks and is
/// updated in place, so each agent alerts once a calendar month; marks from
/// earlier months are dropped, as `clients::over_budget` does.
pub fn over_budget(rows: &[BudgetRow], today: NaiveDate, fired: &mut Vec<String>) -> Vec<BudgetRow> {
    let month = today.format("%Y-%m").to_string();
    fired.retain(|m| m.ends_with(&format!("|{month}")));
    let mut out = Vec::new();
    for row in rows {
        let mark = format!("{}|{month}", row.agent);
        if row.is_over() && !fired.contains(&mark) {
            fired.push(mark);
            out.push(row.clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The live rule
// ---------------------------------------------------------------------------

/// Sixty seconds or more since the last line and nothing in ten minutes: what
/// the Running now row calls idle.
pub fn is_idle(pace: &LivePace) -> bool {
    pace.idle_secs >= IDLE_SECS && pace.tokens_10m == 0
}

fn pace_per_hour(pace: &LivePace) -> Option<f64> {
    pace.priced.then_some(pace.cost_10m * PACE_FACTOR)
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Runaway {
    pub tool: String,
    /// The host process, used only to key the once-a-day mark. Never sent to
    /// the webview or anywhere else.
    #[serde(skip)]
    pub pid: u32,
    pub area: Option<String>,
    /// "pace" | "duration"
    pub reason: &'static str,
    pub pace_per_hour: Option<f64>,
    pub minutes: u64,
}

/// Running agents the live rule fires on. An agent with no live pace never
/// fires, and neither does an idle one, however long it has been open. When
/// both figures are passed, "pace" is the reason given.
pub fn runaways(running: &[RunningAgent], rule: &LiveRule) -> Vec<Runaway> {
    let mut out = Vec::new();
    for agent in running {
        let Some(pace) = agent.pace.as_ref() else { continue };
        if is_idle(pace) {
            continue;
        }
        let per_hour = pace_per_hour(pace);
        let fast = matches!((rule.hourly_pace_usd, per_hour), (Some(limit), Some(now)) if now >= limit);
        let long = rule.max_minutes.is_some_and(|m| agent.elapsed_secs >= m.saturating_mul(60));
        let reason = if fast {
            "pace"
        } else if long {
            "duration"
        } else {
            continue;
        };
        out.push(Runaway {
            tool: agent.tool.clone(),
            pid: agent.pid,
            area: agent.area.clone(),
            reason,
            pace_per_hour: per_hour,
            minutes: agent.elapsed_secs / 60,
        });
    }
    out
}

/// The fastest pace per hour among running sessions that are priced and not
/// idle: shown beside the empty field so the user can see what their own
/// sessions run at. `None` when there is no such session.
pub fn live_hint(running: &[RunningAgent]) -> Option<f64> {
    running
        .iter()
        .filter_map(|a| a.pace.as_ref())
        .filter(|p| !is_idle(p))
        .filter_map(pace_per_hour)
        .max_by(|a, b| a.total_cmp(b))
}

fn who(r: &Runaway) -> String {
    match &r.area {
        Some(area) => format!("{} ({area})", r.tool),
        None => r.tool.clone(),
    }
}

/// Hours:minutes, language-neutral, for the finding's rows.
fn hours_minutes(minutes: u64) -> String {
    format!("{}:{:02}", minutes / 60, minutes % 60)
}

/// The open time as the locale's own short duration, chosen by size the way
/// the Running now row does (days and hours, hours and minutes, minutes).
fn duration_msg(minutes: u64) -> Msg {
    let (d, h, m) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    if d > 0 {
        Msg::new("time.daysHours").var("d", d).var("h", h)
    } else if h > 0 {
        Msg::new("time.hoursMins").var("h", h).var("m", m)
    } else {
        Msg::new("time.mins").var("m", m.max(1))
    }
}

/// One alert per running agent per day, and the process is the agent: the mark
/// is "tool|area|pid|YYYY-MM-DD", because the folder is often unknown and two
/// sessions of one tool must not silence each other. It is one per process
/// whatever the reason, so a duration alert also uses up that process's pace
/// alert for the day. `today` is "YYYY-MM-DD"; `fired` is updated in place and
/// marks from other days are dropped.
///
/// A name or area holding "|" is harmless: marks are only compared whole and
/// the day is read from the end, so the separator is never parsed back apart.
pub fn alerts_for(runaways: &[Runaway], today: &str, fired: &mut Vec<String>) -> Vec<Alert> {
    fired.retain(|m| m.ends_with(&format!("|{today}")));
    let mut out = Vec::new();
    for r in runaways {
        let mark = format!("{}|{}|{}|{today}", r.tool, r.area.as_deref().unwrap_or(""), r.pid);
        if fired.contains(&mark) {
            continue;
        }
        fired.push(mark);
        let body = match (r.reason, r.pace_per_hour) {
            ("pace", Some(pace)) => Msg::new("notify.agentRunaway.pace").var("who", who(r)).var("pace", format!("{pace:.2}")),
            _ => Msg::new("notify.agentRunaway.duration").var("who", who(r)).sub("open", duration_msg(r.minutes)),
        };
        out.push(Alert { title: Msg::new("notify.agentRunaway.title"), body });
    }
    out
}

fn read_marks(marks: &mut Value, key: &str) -> Vec<String> {
    if !marks.is_object() {
        *marks = json!({});
    }
    marks.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// The loop's budget step: reads its own marks out of `alert_marks.json`'s
/// value, writes them back, and returns the rows to notify about. Marks that
/// are not an object are replaced by an empty one.
pub fn newly_over_budget(marks: &mut Value, rows: &[BudgetRow], today: NaiveDate) -> Vec<BudgetRow> {
    let mut fired = read_marks(marks, BUDGET_MARKS);
    let out = over_budget(rows, today, &mut fired);
    marks[BUDGET_MARKS] = json!(fired);
    out
}

/// The loop's live-rule step; same contract as `newly_over_budget`.
pub fn due_runaway_alerts(marks: &mut Value, runaways: &[Runaway], today: &str) -> Vec<Alert> {
    let mut fired = read_marks(marks, RUNAWAY_MARKS);
    let out = alerts_for(runaways, today, &mut fired);
    marks[RUNAWAY_MARKS] = json!(fired);
    out
}

/// Marks for a rule that is no longer set are dropped. True when something
/// was removed, so the caller knows the file changed.
pub fn drop_unused_marks(marks: &mut Value, w: &Watch) -> bool {
    let Some(map) = marks.as_object_mut() else { return false };
    let mut removed = false;
    if w.budgets.is_empty() {
        removed |= map.remove(BUDGET_MARKS).is_some();
    }
    if !w.live_is_set() {
        removed |= map.remove(RUNAWAY_MARKS).is_some();
    }
    removed
}

// ---------------------------------------------------------------------------
// Findings
// ---------------------------------------------------------------------------

fn usd(x: f64) -> String {
    format!("${x:.2}")
}

/// Titles carry a count only, because titles leave the machine in the seat
/// report; the agent's name, the work area and the figures are in the detail.
/// `over` is the rows currently at or over budget (see `currently_over`).
pub fn opportunities(over: &[BudgetRow], runaways: &[Runaway]) -> Vec<Opportunity> {
    let mut out = Vec::new();
    if !over.is_empty() {
        let n = over.len() as i64;
        let names = over
            .iter()
            .map(|r| format!("{} ({} / {})", r.agent, usd(r.month_to_date), usd(r.monthly_budget)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Opportunity::from_msgs(
            "agent-over-budget",
            "learn",
            Msg::new("finding.agent-over-budget.title").count(n),
            Some(Msg::new("finding.agent-over-budget.detail").var("names", names)),
            None,
        ));
    }
    if !runaways.is_empty() {
        let n = runaways.len() as i64;
        let names = runaways
            .iter()
            .map(|r| match (r.reason, r.pace_per_hour) {
                ("pace", Some(p)) => format!("{}: {}", who(r), usd(p)),
                _ => format!("{}: {}", who(r), hours_minutes(r.minutes)),
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Opportunity::from_msgs(
            "agent-runaway",
            "learn",
            Msg::new("finding.agent-runaway.title").count(n),
            Some(Msg::new("finding.agent-runaway.detail").var("names", names)),
            None,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn spend_row(name: &str, cost: f64) -> AgentSpend {
        AgentSpend { name: name.into(), runs: 1, cost, tokens: 0, last_used_ms: 0, top_model: None, by_client: vec![] }
    }

    fn pace(cost_10m: f64, tokens_10m: u64, idle_secs: u64, priced: bool) -> LivePace {
        LivePace {
            session_id: "sess-fixture".into(),
            tokens_10m,
            cost_10m,
            priced,
            idle_secs,
            model: None,
            area: None,
            tool: "Claude Code".into(),
        }
    }

    fn running(tool: &str, area: Option<&str>, elapsed_secs: u64, p: Option<LivePace>) -> RunningAgent {
        RunningAgent {
            tool: tool.into(),
            pid: 1,
            elapsed_secs,
            rss_bytes: 0,
            cpu_percent: None,
            cwd: None,
            area: area.map(str::to_string),
            client: None,
            pace: p,
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("aitm-agent-watch-{}", crate::providers::unique_stamp())).join(name)
    }

    fn budget(agent: &str, usd: f64) -> AgentBudget {
        AgentBudget { agent: agent.into(), monthly_budget: usd }
    }

    #[test]
    fn nothing_set_means_nothing_fires() {
        let w = Watch::default();
        // Spend far over any plausible budget, and an agent far over any
        // plausible pace and open for days: with nothing set, nothing happens.
        let rows = budget_rows(&[spend_row("Explore", 1_000_000.0)], &w);
        assert!(rows.is_empty());
        let mut fired = Vec::new();
        assert!(over_budget(&rows, day("2026-10-03"), &mut fired).is_empty());
        let fast = running("Claude Code", Some("site"), 9 * 24 * 3600, Some(pace(5_000.0, 9_000, 0, true)));
        let found = runaways(&[fast], &w.live);
        assert!(found.is_empty());
        assert!(opportunities(&currently_over(&rows), &found).is_empty());
        assert!(alerts_for(&found, "2026-10-03", &mut fired).is_empty());
        let mut marks = json!({});
        assert!(newly_over_budget(&mut marks, &rows, day("2026-10-03")).is_empty());
        assert!(due_runaway_alerts(&mut marks, &found, "2026-10-03").is_empty());
    }

    #[test]
    fn a_budget_counts_the_calendar_month_not_thirty_days() {
        // Spend on the last day of the previous month and on the 1st and 3rd.
        let daily = [(day("2026-09-30"), 5.0), (day("2026-10-01"), 1.0), (day("2026-10-03"), 2.0)];
        let rows = crate::spend::month_spend_from_daily("Explore", &daily, day("2026-10-03"));
        assert_eq!(rows.len(), 1);
        assert!((rows[0].cost - 3.0).abs() < 1e-9, "only this month's dollars, got {}", rows[0].cost);

        // A 31-day month on its 31st still counts the 1st, and not the day before it.
        let daily = [(day("2026-09-30"), 9.0), (day("2026-10-01"), 4.0), (day("2026-10-31"), 0.5)];
        let rows = crate::spend::month_spend_from_daily("Explore", &daily, day("2026-10-31"));
        assert!((rows[0].cost - 4.5).abs() < 1e-9, "got {}", rows[0].cost);

        // February: the 28th counts all 28 days.
        let daily = [(day("2027-01-31"), 7.0), (day("2027-02-01"), 1.0), (day("2027-02-28"), 1.0)];
        let rows = crate::spend::month_spend_from_daily("Explore", &daily, day("2027-02-28"));
        assert!((rows[0].cost - 2.0).abs() < 1e-9, "got {}", rows[0].cost);
    }

    #[test]
    fn a_budget_alerts_once_a_month_per_agent() {
        let w = Watch { budgets: vec![budget("reviewer", 10.0), budget("Explore", 10.0)], live: LiveRule::default() };
        let rows = budget_rows(&[spend_row("reviewer", 12.0), spend_row("Explore", 3.0)], &w);
        let mut fired = vec!["gone|2026-09".to_string()];
        let first = over_budget(&rows, day("2026-10-03"), &mut fired);
        assert_eq!(first.iter().map(|r| r.agent.as_str()).collect::<Vec<_>>(), ["reviewer"]);
        assert_eq!(fired, ["reviewer|2026-10"], "last month's mark is dropped, this month's is kept");
        assert!(over_budget(&rows, day("2026-10-20"), &mut fired).is_empty(), "same month: silent");
        // The next month it is armed again.
        assert_eq!(over_budget(&rows, day("2026-11-01"), &mut fired).len(), 1);
        // Exactly at the budget counts, like a client budget.
        let at = budget_rows(&[spend_row("Explore", 10.0)], &Watch { budgets: vec![budget("Explore", 10.0)], ..Watch::default() });
        assert_eq!(over_budget(&at, day("2026-11-01"), &mut Vec::new()).len(), 1);
    }

    #[test]
    fn a_budget_may_name_a_built_in_agent() {
        let known = known_names(&[], &[], &Watch::default());
        let path = temp_path("agent_watch.json");
        let saved = save_to(&path, Watch { budgets: vec![budget("Explore", 25.0)], ..Watch::default() }, &known).expect("a built-in name is allowed");
        assert_eq!(saved.budgets[0].agent, "Explore");
        assert_eq!(load_from(&path), saved);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_budget_for_an_unknown_name_is_refused() {
        let known = known_names(&["reviewer".to_string()], &[spend_row("", 1.0), spend_row("worker", 2.0)], &Watch::default());
        assert!(known.contains(&"worker".to_string()) && known.contains(&"reviewer".to_string()));
        assert!(!known.contains(&String::new()), "the unattributed row can never take a budget");
        let path = temp_path("agent_watch.json");
        for bad in ["not-an-agent", "", "explore"] {
            let err = save_to(&path, Watch { budgets: vec![budget(bad, 5.0)], ..Watch::default() }, &known).unwrap_err();
            assert_eq!(err.key, "error.agentWatch.pick", "{bad:?}");
        }
        assert!(!path.exists(), "a refused save writes nothing");
    }

    #[test]
    fn a_save_refuses_bad_figures_repeats_and_too_many() {
        let known = known_names(&["reviewer".to_string()], &[], &Watch::default());
        let path = temp_path("agent_watch.json");
        let try_save = |w: Watch| save_to(&path, w, &known).unwrap_err().key;
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(try_save(Watch { budgets: vec![budget("reviewer", bad)], ..Watch::default() }), "error.agentWatch.figure");
            assert_eq!(
                try_save(Watch { live: LiveRule { hourly_pace_usd: Some(bad), max_minutes: None }, ..Watch::default() }),
                "error.agentWatch.figure"
            );
        }
        assert_eq!(
            try_save(Watch { live: LiveRule { hourly_pace_usd: None, max_minutes: Some(0) }, ..Watch::default() }),
            "error.agentWatch.figure"
        );
        assert_eq!(
            try_save(Watch { budgets: vec![budget("reviewer", 1.0), budget("reviewer", 2.0)], ..Watch::default() }),
            "error.agentWatch.duplicate"
        );
        let many: Vec<String> = (0..=MAX_BUDGETS).map(|i| format!("agent-{i}")).collect();
        let known_many = known_names(&many, &[], &Watch::default());
        let w = Watch { budgets: many.iter().map(|n| budget(n, 1.0)).collect(), ..Watch::default() };
        assert_eq!(save_to(&path, w, &known_many).unwrap_err().key, "error.agentWatch.tooMany");
        assert!(!path.exists());
    }

    #[test]
    fn the_file_is_lenient_to_read_and_owner_only() {
        let path = temp_path("agent_watch.json");
        assert_eq!(load_from(&path), Watch::default(), "missing file");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), Watch::default(), "broken file");
        std::fs::write(&path, r#"{"budgets":[{"agent":"Plan","monthlyBudget":-4},{"agent":"Explore","monthlyBudget":8}],"live":{"hourlyPaceUsd":0,"maxMinutes":0}}"#).unwrap();
        let w = load_from(&path);
        assert_eq!(w.budgets, vec![budget("Explore", 8.0)], "a figure that could not have been saved is dropped");
        assert_eq!(w.live, LiveRule::default());
        let w = Watch { budgets: vec![budget("Plan", 3.0)], live: LiveRule { hourly_pace_usd: Some(20.0), max_minutes: Some(90) } };
        let saved = save_to(&path, w.clone(), &known_names(&[], &[], &Watch::default())).unwrap();
        assert_eq!(saved, w);
        assert_eq!(load_from(&path), w);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_budget_with_no_spend_still_shows() {
        let w = Watch { budgets: vec![budget("worker", 10.0), budget("Plan", 4.0)], ..Watch::default() };
        let rows = budget_rows(&[spend_row("Plan", 1.5)], &w);
        assert_eq!(
            rows,
            vec![
                BudgetRow { agent: "worker".into(), month_to_date: 0.0, monthly_budget: 10.0 },
                BudgetRow { agent: "Plan".into(), month_to_date: 1.5, monthly_budget: 4.0 },
            ],
            "one row per budget, in the order saved"
        );
    }

    #[test]
    fn an_idle_or_unpriced_session_is_never_a_runaway() {
        let rule = LiveRule { hourly_pace_usd: Some(10.0), max_minutes: None };
        // $2 in ten minutes is $12 an hour: over the figure when it counts.
        let live = running("Claude Code", Some("a"), 60, Some(pace(2.0, 500, 5, true)));
        assert_eq!(runaways(&[live], &rule).len(), 1, "the control case fires");
        let idle = running("Claude Code", Some("a"), 60, Some(pace(2.0, 0, 90, true)));
        let unpriced = running("Claude Code", Some("b"), 60, Some(pace(2.0, 500, 5, false)));
        let no_pace = running("Claude Code", Some("c"), 60, None);
        assert!(runaways(&[idle, unpriced, no_pace], &rule).is_empty());
        // Below the figure is not a runaway either; exactly at it is.
        let slow = running("Claude Code", None, 60, Some(pace(1.0, 500, 5, true)));
        assert!(runaways(&[slow], &rule).is_empty());
        let at = running("Claude Code", None, 60, Some(pace(10.0 / 6.0, 500, 5, true)));
        assert_eq!(runaways(&[at], &rule).len(), 1, "at the figure counts");
    }

    #[test]
    fn a_long_open_idle_session_is_not_a_runaway() {
        let rule = LiveRule { hourly_pace_usd: None, max_minutes: Some(120) };
        let three_hours = 3 * 3600;
        let idle = running("Claude Code", Some("a"), three_hours, Some(pace(0.0, 0, 600, true)));
        let working = running("Claude Code", Some("b"), three_hours, Some(pace(0.1, 40, 3, false)));
        let short = running("Claude Code", Some("c"), 119 * 60, Some(pace(0.1, 40, 3, true)));
        let no_pace = running("Claude Code", Some("d"), three_hours, None);
        let out = runaways(&[idle, working, short, no_pace], &rule);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].area.as_deref(), out[0].reason, out[0].minutes), (Some("b"), "duration", 180));
        let at = running("Claude Code", None, 120 * 60, Some(pace(0.1, 40, 3, true)));
        assert_eq!(runaways(&[at], &rule).len(), 1, "open exactly the set time counts");
    }

    #[test]
    fn the_live_hint_is_absent_without_a_priced_active_session() {
        assert_eq!(live_hint(&[]), None);
        let idle = running("Claude Code", None, 1, Some(pace(9.0, 0, 120, true)));
        let unpriced = running("Claude Code", None, 1, Some(pace(9.0, 10, 1, false)));
        let none = running("Claude Code", None, 1, None);
        assert_eq!(live_hint(&[idle, unpriced, none]), None);
        let slow = running("Claude Code", None, 1, Some(pace(1.0, 10, 1, true)));
        let fast = running("Claude Code", None, 1, Some(pace(3.0, 10, 1, true)));
        assert_eq!(live_hint(&[slow, fast]), Some(18.0), "the fastest pace per hour");
    }

    fn runaway(area: Option<&str>, pid: u32) -> Runaway {
        Runaway { tool: "Claude Code".into(), pid, area: area.map(str::to_string), reason: "pace", pace_per_hour: Some(12.0), minutes: 5 }
    }

    #[test]
    fn a_runaway_alerts_once_a_day() {
        let r = |area: Option<&str>| runaway(area, 7);
        let mut fired = vec!["Claude Code|old|7|2026-10-02".to_string()];
        let first = alerts_for(&[r(Some("site")), r(None)], "2026-10-03", &mut fired);
        assert_eq!(first.len(), 2, "two different areas are two agents");
        assert_eq!(fired, ["Claude Code|site|7|2026-10-03", "Claude Code||7|2026-10-03"], "yesterday's mark is dropped");
        assert!(alerts_for(&[r(Some("site")), r(None)], "2026-10-03", &mut fired).is_empty(), "same day: silent");
        // One process in one pass is one alert, and it is armed again the next day.
        let mut fresh = Vec::new();
        assert_eq!(alerts_for(&[r(Some("site")), r(Some("site"))], "2026-10-04", &mut fresh).len(), 1);
        assert_eq!(alerts_for(&[r(Some("site"))], "2026-10-05", &mut fresh).len(), 1, "armed again the next day");
        // The body names the figure for a pace and the open time for a duration.
        let p = &alerts_for(&[r(Some("x"))], "2026-10-06", &mut Vec::new())[0];
        assert_eq!(p.body.key, "notify.agentRunaway.pace");
        let d = Runaway { reason: "duration", pace_per_hour: None, minutes: 130, ..r(Some("x")) };
        let body = &alerts_for(std::slice::from_ref(&d), "2026-10-06", &mut Vec::new())[0].body;
        assert_eq!(body.key, "notify.agentRunaway.duration");
        let Some(crate::i18n::Var::Msg(open)) = body.vars.get("open") else { panic!("the open time is a nested message") };
        assert_eq!((open.key, open.vars.get("h").cloned(), open.vars.get("m").cloned()), ("time.hoursMins", Some(crate::i18n::Var::Text("2".into())), Some(crate::i18n::Var::Text("10".into()))));
        // The reason does not matter: a duration alert uses up that process's day.
        let mut one = Vec::new();
        assert_eq!(alerts_for(&[d], "2026-10-06", &mut one).len(), 1);
        assert!(alerts_for(&[r(Some("x"))], "2026-10-06", &mut one).is_empty());
    }

    #[test]
    fn two_agents_in_the_same_tool_each_alert_once() {
        // The folder is unknown for both, so only the process tells them apart.
        let both = [runaway(None, 101), runaway(None, 102)];
        let mut fired = Vec::new();
        assert_eq!(alerts_for(&both, "2026-10-03", &mut fired).len(), 2);
        assert!(alerts_for(&both, "2026-10-03", &mut fired).is_empty(), "each only once that day");
        let json = serde_json::to_string(&both[0]).unwrap();
        assert!(!json.contains("101") && !json.contains("pid"), "the pid never leaves: {json}");
    }

    #[test]
    fn a_pipe_in_a_name_or_an_area_is_harmless() {
        let w = Watch { budgets: vec![budget("a|b", 1.0)], ..Watch::default() };
        let rows = budget_rows(&[spend_row("a|b", 2.0)], &w);
        let mut fired = Vec::new();
        assert_eq!(over_budget(&rows, day("2026-10-03"), &mut fired).len(), 1);
        assert!(over_budget(&rows, day("2026-10-09"), &mut fired).is_empty(), "same month: silent");
        assert_eq!(over_budget(&rows, day("2026-11-01"), &mut fired).len(), 1, "the mark is dropped by month, not parsed apart");
        let mut marks = Vec::new();
        let odd = [runaway(Some("x|y"), 5), runaway(Some("x"), 5), runaway(Some("y"), 5)];
        assert_eq!(alerts_for(&odd, "2026-10-03", &mut marks).len(), 3, "areas with and around a pipe stay distinct");
        assert!(alerts_for(&odd, "2026-10-03", &mut marks).is_empty());
        assert!(alerts_for(&odd, "2026-10-04", &mut marks).len() == 3, "and are dropped by day");
    }

    #[test]
    fn findings_count_what_their_detail_names() {
        let over = vec![
            BudgetRow { agent: "reviewer".into(), month_to_date: 12.5, monthly_budget: 10.0 },
            BudgetRow { agent: "Explore".into(), month_to_date: 30.0, monthly_budget: 30.0 },
        ];
        let run = vec![
            Runaway { tool: "Claude Code".into(), pid: 1, area: Some("site".into()), reason: "pace", pace_per_hour: Some(14.0), minutes: 4 },
            Runaway { tool: "Codex".into(), pid: 2, area: None, reason: "duration", pace_per_hour: None, minutes: 130 },
        ];
        let found = opportunities(&over, &run);
        let ids: Vec<&str> = found.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, FINDING_IDS);
        assert!(found.iter().all(|o| o.kind == "learn"));
        let names = |o: &Opportunity| match o.detail_msg.as_ref().and_then(|m| m.vars.get("names")) {
            Some(crate::i18n::Var::Text(t)) => t.clone(),
            other => panic!("names is a text var, got {other:?}"),
        };
        for (o, rows) in found.iter().zip([over.len(), run.len()]) {
            assert_eq!(o.title_msg.count, Some(rows as i64));
            assert_eq!(names(o).split(", ").count(), rows, "the count is the number of rows named");
            assert!(o.title_msg.vars.is_empty(), "a title carries a count only");
        }
        assert_eq!(names(&found[0]), "reviewer ($12.50 / $10.00), Explore ($30.00 / $30.00)");
        assert_eq!(names(&found[1]), "Claude Code (site): $14.00, Codex: 2:10", "rows hold figures only, no unit words");
        assert!(opportunities(&[], &[]).is_empty(), "absent when there is nothing to say");
        // Only what is over now: a row under budget is not a finding.
        let under = vec![BudgetRow { agent: "Plan".into(), month_to_date: 1.0, monthly_budget: 2.0 }];
        assert!(currently_over(&under).is_empty());
        let exactly = vec![BudgetRow { agent: "Plan".into(), month_to_date: 2.0, monthly_budget: 2.0 }];
        assert_eq!(currently_over(&exactly).len(), 1, "at the budget counts");
    }

    #[test]
    fn a_budget_equal_to_spend_counts_as_over_after_a_round_trip() {
        let path = temp_path("agent_watch.json");
        let saved = save_to(&path, Watch { budgets: vec![budget("Plan", 12.5)], ..Watch::default() }, &known_names(&[], &[], &Watch::default())).unwrap();
        let rows = budget_rows(&[spend_row("Plan", 12.5)], &load_from(&path));
        assert_eq!(rows[0].monthly_budget, saved.budgets[0].monthly_budget);
        assert_eq!(currently_over(&rows).len(), 1);
        assert_eq!(over_budget(&rows, day("2026-10-03"), &mut Vec::new()).len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_saved_budget_for_a_vanished_agent_does_not_block_saving() {
        let path = temp_path("agent_watch.json");
        let first = Watch { budgets: vec![budget("gone-agent", 5.0)], ..Watch::default() };
        let known = known_names(&["gone-agent".to_string()], &[], &Watch::default());
        save_to(&path, first, &known).unwrap();
        // The definition is deleted and it has no spend: only the saved file still names it.
        let saved = load_from(&path);
        let known = known_names(&[], &[], &saved);
        assert!(known.contains(&"gone-agent".to_string()), "still listed, so the row can be shown and edited");
        let next = Watch { budgets: vec![budget("gone-agent", 9.0), budget("Plan", 3.0)], live: LiveRule { hourly_pace_usd: Some(20.0), max_minutes: None } };
        assert_eq!(save_to(&path, next.clone(), &known).unwrap(), next);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_failed_write_gives_a_fixed_message_not_the_writers_detail() {
        let dir = temp_path("blocker");
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, "a file where a folder is needed").unwrap();
        let err = save_to(&dir.join("agent_watch.json"), Watch::default(), &[]).unwrap_err();
        assert_eq!(err.key, "error.agentWatch.write");
        assert!(err.vars.is_empty(), "the writer's English, credential-file wording must not reach the user");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[test]
    fn a_hand_edited_file_is_bounded_on_load() {
        let path = temp_path("agent_watch.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let long = "x".repeat(MAX_NAME_CHARS + 1);
        let mut entries: Vec<String> = vec![
            r#"{"agent":"first","monthlyBudget":1}"#.into(),
            r#"{"agent":"first","monthlyBudget":99}"#.into(),
            r#"{"agent":"broken","monthlyBudget":"lots"}"#.into(),
            r#"{"nonsense":true}"#.into(),
            r#"42"#.into(),
            format!(r#"{{"agent":"{long}","monthlyBudget":1}}"#),
            r#"{"agent":"","monthlyBudget":1}"#.into(),
            r#"{"agent":"last-ok","monthlyBudget":2}"#.into(),
        ];
        entries.extend((0..60).map(|i| format!(r#"{{"agent":"bulk-{i}","monthlyBudget":1}}"#)));
        std::fs::write(&path, format!(r#"{{"budgets":[{}],"live":{{"hourlyPaceUsd":3.5,"maxMinutes":90.5}}}}"#, entries.join(","))).unwrap();
        let w = load_from(&path);
        assert_eq!(w.budgets.len(), MAX_BUDGETS, "stops at the cap");
        assert_eq!((w.budgets[0].agent.as_str(), w.budgets[0].monthly_budget), ("first", 1.0), "a repeated name keeps its first row");
        assert_eq!(w.budgets[1].agent, "last-ok", "one malformed entry does not cost the rest");
        assert!(w.budgets.iter().all(|b| !b.agent.is_empty() && b.agent != long && b.agent != "broken"));
        assert_eq!(w.live, LiveRule { hourly_pace_usd: Some(3.5), max_minutes: None }, "a fractional minute count is dropped, the other figure kept");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_fractional_or_out_of_range_minute_count_is_refused_in_the_users_language() {
        let input = |m: Option<f64>| WatchInput { budgets: vec![], live: LiveInput { hourly_pace_usd: None, max_minutes: m } };
        for bad in [90.5, 0.0, -3.0, f64::NAN, 1e30] {
            assert_eq!(input(Some(bad)).into_watch().unwrap_err().key, "error.agentWatch.figure", "{bad}");
        }
        assert_eq!(input(Some(90.0)).into_watch().unwrap().live.max_minutes, Some(90));
        assert_eq!(input(None).into_watch().unwrap().live.max_minutes, None);
        let parsed: WatchInput = serde_json::from_str(r#"{"budgets":[],"live":{"maxMinutes":90.5}}"#).expect("a fraction parses, then is refused by into_watch");
        assert!(parsed.into_watch().is_err());
    }

    #[test]
    fn the_loop_steps_read_and_write_their_own_marks() {
        // A missing key, and marks that are valid JSON but not an object.
        let rows = [BudgetRow { agent: "Plan".into(), month_to_date: 9.0, monthly_budget: 5.0 }];
        for mut marks in [json!({}), json!([1, 2]), json!("text"), json!(null)] {
            assert_eq!(newly_over_budget(&mut marks, &rows, day("2026-10-03")).len(), 1);
            assert_eq!(marks[BUDGET_MARKS], json!(["Plan|2026-10"]));
            assert!(newly_over_budget(&mut marks, &rows, day("2026-10-20")).is_empty(), "same month: silent");
            // A stale month is dropped, and the 1st re-arms it.
            marks[BUDGET_MARKS] = json!(["Plan|2026-09", "Other|2026-09"]);
            assert_eq!(newly_over_budget(&mut marks, &rows, day("2026-10-01")).len(), 1);
            assert_eq!(marks[BUDGET_MARKS], json!(["Plan|2026-10"]));
        }
        let mut marks = json!({"budgetFired": ["keep|2026-10"]});
        let run = [runaway(Some("site"), 3)];
        assert_eq!(due_runaway_alerts(&mut marks, &run, "2026-10-03").len(), 1);
        assert!(due_runaway_alerts(&mut marks, &run, "2026-10-03").is_empty());
        assert_eq!(marks["runawayFired"], json!(["Claude Code|site|3|2026-10-03"]));
        assert_eq!(marks["budgetFired"], json!(["keep|2026-10"]), "other keys are left alone");
        assert_eq!(due_runaway_alerts(&mut json!(7), &run, "2026-10-03").len(), 1);
        let mut rolled = json!({"runawayFired": ["Claude Code|site|3|2026-10-02"]});
        assert_eq!(due_runaway_alerts(&mut rolled, &run, "2026-10-03").len(), 1, "yesterday's mark is gone");
    }

    #[test]
    fn marks_for_a_rule_that_is_no_longer_set_are_dropped() {
        let mut marks = json!({"agentBudgetFired": ["a|2026-10"], "runawayFired": ["t||1|2026-10-03"], "budgetFired": []});
        let budget_only = Watch { budgets: vec![budget("Plan", 1.0)], ..Watch::default() };
        assert!(drop_unused_marks(&mut marks, &budget_only));
        assert!(marks.get("runawayFired").is_none() && marks.get("agentBudgetFired").is_some());
        assert!(!drop_unused_marks(&mut marks, &budget_only), "nothing more to remove: not a change");
        assert!(drop_unused_marks(&mut marks, &Watch::default()));
        assert_eq!(marks, json!({"budgetFired": []}), "the client budget marks are never touched");
        assert!(!drop_unused_marks(&mut json!([1]), &Watch::default()));
    }

    #[test]
    fn a_custom_agent_name_never_reaches_the_seat_report_or_the_feed() {
        use crate::inventory::Inventory;
        const NAME: &str = "northwind-intake-reviewer";
        const AREA: &str = "northwind-portal/billing";
        // Figures no other fixture here has, so finding one in a payload means this crossed over.
        const BUDGET: f64 = 4812.5;
        const PACE: f64 = 7391.25;

        let w = Watch {
            budgets: vec![budget(NAME, BUDGET)],
            live: LiveRule { hourly_pace_usd: Some(PACE), max_minutes: Some(777) },
        };
        let rows = budget_rows(&[spend_row(NAME, BUDGET + 1.0)], &w);
        // The feed does publish a live pace by design, so the running row's own
        // cost is a different figure from the limit the user set.
        let busy = running("Claude Code", Some(AREA), 9_999, Some(pace(1_300.0, 5_000, 1, true)));
        let run = runaways(std::slice::from_ref(&busy), &w.live);
        assert_eq!(run.len(), 1, "the runaway has to exist, or this proves nothing");
        let found = opportunities(&currently_over(&rows), &run);
        assert_eq!(found.len(), 2);
        assert!(found[0].detail.contains(NAME) && found[1].detail.contains(AREA), "the markers must be in the details");

        let inv = Inventory { opportunities: found, ..Inventory::default() };
        let report = crate::seat::build_with("seat-abcdefgh", "Dana's MacBook", 1, &inv, &[], &[]);
        assert_eq!(report.findings.len(), 2, "the findings do reach the report, as titles");
        let seat_wire = serde_json::to_string(&report).unwrap();

        // The feed publishes a running row's own area by design, so it is fed
        // the row with its area unknown: what is checked is that nothing from
        // the watch (a name, a figure, the rule) enters it.
        let mut shown = busy.clone();
        shown.area = None;
        let feed = crate::httpapi::agents_feed(&[spend_row("Explore", 1.0)], &[shown]);
        let keys: Vec<&String> = feed.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["agents", "running"], "the feed has no watch field");
        let feed_wire = feed.to_string();

        for wire in [&seat_wire, &feed_wire] {
            for marker in [NAME, "northwind", AREA, "northwind-portal", "4812.5", "4813.5", "7391.25", "777"] {
                assert!(!wire.contains(marker), "{marker} leaked into {wire}");
            }
        }
    }
}
