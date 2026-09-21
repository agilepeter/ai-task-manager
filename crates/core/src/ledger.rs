//! Subscription ledger: what the user pays for AI tools, when each renews,
//! and whether the usage the app measures justifies the price.
//!
//! Entries are typed in by the user. Nothing here guesses a price: a plan
//! name detected on a card says which tier someone is on, not what they pay
//! (regional pricing, annual discounts, team seats), so detected tools are
//! offered by name only and the amount is always the user's own number.
//!
//! One JSON file in the app's config dir. It never leaves the machine.

use chrono::{Datelike, Months, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

const MAX_ITEMS: usize = 200;
const MAX_NAME: usize = 80;
const MAX_NOTES: usize = 300;
const MAX_PRICE: f64 = 1_000_000.0;
/// A linked tool with less API-equivalent usage than this in 30 days counts
/// as idle. Above zero so a stray test call does not read as "in use".
const IDLE_BELOW: f64 = 1.0;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Cycle {
    Monthly,
    Yearly,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    /// Empty on a new entry; `save` assigns one.
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// What is charged each cycle, in the user's own currency.
    pub price: f64,
    pub cycle: Cycle,
    /// Any date the subscription renewed or will renew on, `YYYY-MM-DD`.
    /// Later renewals are worked out from it.
    #[serde(default)]
    pub renews_on: Option<String>,
    /// Card id this pays for ("claude"), so usage can be set against price.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    /// The renewal date a reminder was last sent for: one reminder each.
    #[serde(default)]
    pub reminded_for: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct LedgerFile {
    items: Vec<Subscription>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    #[serde(flatten)]
    pub subscription: Subscription,
    pub monthly_cost: f64,
    pub next_renewal: Option<String>,
    pub days_left: Option<i64>,
    /// API-equivalent usage of the linked tool over the last 30 days.
    pub usage30: Option<f64>,
    /// `usage30` over the monthly cost: 9.5 means the plan did 9.5x its price.
    pub value_ratio: Option<f64>,
    /// Linked to a tool that showed almost no usage in 30 days.
    pub idle: bool,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LedgerView {
    pub items: Vec<ItemView>,
    pub monthly: f64,
    pub yearly: f64,
    /// Monthly cost of entries flagged idle: what cancelling them would save.
    pub idle_monthly: f64,
}

// ---------------------------------------------------------------------------
// Pure logic
// ---------------------------------------------------------------------------

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

/// The first renewal on or after `today`. Steps are counted from the anchor,
/// not from the previous result: Jan 31 monthly goes Feb 28, Mar 31, Apr 30,
/// where stepping from each clamped date would drift to the 28th for good.
pub fn next_renewal(anchor: NaiveDate, cycle: Cycle, today: NaiveDate) -> Option<NaiveDate> {
    if anchor >= today {
        return Some(anchor);
    }
    let per_step: u32 = match cycle {
        Cycle::Monthly => 1,
        Cycle::Yearly => 12,
    };
    let months_apart = (today.year_ce().1 as i64 - anchor.year_ce().1 as i64) * 12
        + (today.month0() as i64 - anchor.month0() as i64);
    // Start one step early: the clamp can land a step just before `today`.
    let mut k = (months_apart / per_step as i64 - 1).max(0) as u32;
    loop {
        let candidate = anchor.checked_add_months(Months::new(k.checked_mul(per_step)?))?;
        if candidate >= today {
            return Some(candidate);
        }
        k = k.checked_add(1)?;
    }
}


pub fn monthly_cost(sub: &Subscription) -> f64 {
    match sub.cycle {
        Cycle::Monthly => sub.price,
        Cycle::Yearly => sub.price / 12.0,
    }
}

/// `usage30` maps a card id to its API-equivalent spend over 30 days.
pub fn view(items: &[Subscription], today: NaiveDate, usage30: &HashMap<String, f64>) -> LedgerView {
    let mut rows: Vec<ItemView> = items
        .iter()
        .map(|sub| {
            let monthly = monthly_cost(sub);
            let next = sub
                .renews_on
                .as_deref()
                .and_then(parse_date)
                .and_then(|anchor| next_renewal(anchor, sub.cycle, today));
            // A linked tool the scan knows nothing about is "no data", not
            // "idle": only a measured low number earns the idle flag.
            let usage = sub.provider.as_ref().and_then(|p| usage30.get(p).copied());
            ItemView {
                subscription: sub.clone(),
                monthly_cost: monthly,
                next_renewal: next.map(|d| d.format("%Y-%m-%d").to_string()),
                days_left: next.map(|d| (d - today).num_days()),
                usage30: usage,
                value_ratio: usage.filter(|_| monthly > 0.0).map(|u| u / monthly),
                idle: usage.is_some_and(|u| u < IDLE_BELOW) && monthly > 0.0,
            }
        })
        .collect();
    // Soonest renewal first; undated entries after, by cost.
    rows.sort_by(|a, b| match (a.days_left, b.days_left) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.monthly_cost.total_cmp(&a.monthly_cost),
    });
    let monthly: f64 = rows.iter().map(|r| r.monthly_cost).sum();
    LedgerView {
        idle_monthly: rows.iter().filter(|r| r.idle).map(|r| r.monthly_cost).sum(),
        monthly,
        yearly: monthly * 12.0,
        items: rows,
    }
}

/// Cleans and checks an entry from the UI. Text is trimmed and bounded, the
/// price must be a real non-negative amount, the date a real date.
pub fn validate(mut sub: Subscription) -> Result<Subscription, String> {
    sub.name = sub.name.trim().chars().take(MAX_NAME).collect();
    if sub.name.is_empty() {
        return Err("Give the subscription a name.".into());
    }
    if !sub.price.is_finite() || sub.price < 0.0 || sub.price > MAX_PRICE {
        return Err("Enter the price as a number, zero or more.".into());
    }
    sub.renews_on = match sub.renews_on.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => Some(
            parse_date(d).ok_or("Use a real date, like 2026-10-05.")?.format("%Y-%m-%d").to_string(),
        ),
        None => None,
    };
    sub.provider = sub.provider.map(|p| p.trim().to_string()).filter(|p| {
        !p.is_empty()
            && p.len() <= 64
            && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b'-' | b'_' | b'.'))
    });
    sub.notes = sub
        .notes
        .map(|n| n.trim().chars().take(MAX_NOTES).collect::<String>())
        .filter(|n| !n.is_empty());
    Ok(sub)
}

/// Entries whose next renewal is within `within_days` and that have not been
/// reminded about for that renewal. Marks them as reminded.
pub fn due_reminders(items: &mut [Subscription], today: NaiveDate, within_days: i64) -> Vec<ItemView> {
    if within_days <= 0 {
        return Vec::new();
    }
    let rows = view(items, today, &HashMap::new()).items;
    let mut due = Vec::new();
    for row in rows {
        let (Some(days), Some(next)) = (row.days_left, row.next_renewal.clone()) else { continue };
        if days > within_days || row.subscription.reminded_for.as_deref() == Some(next.as_str()) {
            continue;
        }
        if let Some(item) = items.iter_mut().find(|i| i.id == row.subscription.id) {
            item.reminded_for = Some(next);
            due.push(row);
        }
    }
    due
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

pub fn load_from(path: &Path) -> Vec<Subscription> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<LedgerFile>(&raw).ok())
        .map(|f| f.items)
        .unwrap_or_default()
}

fn save_to(path: &Path, items: &[Subscription]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create settings folder: {e}"))?;
    }
    let body = serde_json::to_string_pretty(&LedgerFile { items: items.to_vec() })
        .map_err(|e| format!("encode ledger: {e}"))?;
    // Same writer as saved API keys: temp file, owner-only, atomic replace.
    crate::providers::onenewapi::store::atomic_write(path, &body)
}

/// Adds the entry, or replaces the one with the same id.
pub fn upsert_in(path: &Path, sub: Subscription) -> Result<Subscription, String> {
    let mut sub = validate(sub)?;
    let mut items = load_from(path);
    match items.iter_mut().find(|i| !sub.id.is_empty() && i.id == sub.id) {
        Some(existing) => {
            // A moved renewal date is a new renewal: remind again.
            if existing.renews_on != sub.renews_on || existing.cycle != sub.cycle {
                sub.reminded_for = None;
            } else {
                sub.reminded_for = existing.reminded_for.clone();
            }
            *existing = sub.clone();
        }
        None => {
            if items.len() >= MAX_ITEMS {
                return Err("The ledger is full.".into());
            }
            sub.id = format!("sub-{}", crate::providers::unique_stamp());
            sub.reminded_for = None;
            items.push(sub.clone());
        }
    }
    save_to(path, &items)?;
    Ok(sub)
}

pub fn delete_in(path: &Path, id: &str) -> Result<(), String> {
    let mut items = load_from(path);
    let before = items.len();
    items.retain(|i| i.id != id);
    if items.len() == before {
        return Ok(());
    }
    save_to(path, &items)
}

/// Reminders due now, persisted as sent so each renewal reminds once.
pub fn take_reminders_in(path: &Path, today: NaiveDate, within_days: i64) -> Vec<ItemView> {
    let mut items = load_from(path);
    let due = due_reminders(&mut items, today, within_days);
    if !due.is_empty() && save_to(path, &items).is_err() {
        // Unsaved means it would fire again next refresh: better silent.
        return Vec::new();
    }
    due
}

pub fn path() -> std::path::PathBuf {
    crate::providers::config_dir().join("ledger.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        parse_date(s).unwrap()
    }

    fn sub(name: &str, price: f64, cycle: Cycle, renews_on: Option<&str>, provider: Option<&str>) -> Subscription {
        Subscription {
            id: format!("id-{name}"),
            name: name.into(),
            price,
            cycle,
            renews_on: renews_on.map(str::to_string),
            provider: provider.map(str::to_string),
            notes: None,
            reminded_for: None,
        }
    }

    #[test]
    fn renewals_step_from_the_anchor_so_month_ends_do_not_drift() {
        let jan31 = d("2026-01-31");
        assert_eq!(next_renewal(jan31, Cycle::Monthly, d("2026-02-01")), Some(d("2026-02-28")));
        assert_eq!(next_renewal(jan31, Cycle::Monthly, d("2026-03-01")), Some(d("2026-03-31")), "back to the 31st");
        assert_eq!(next_renewal(jan31, Cycle::Monthly, d("2026-04-01")), Some(d("2026-04-30")));
        // The renewal day itself counts as upcoming, not as missed.
        assert_eq!(next_renewal(d("2026-05-10"), Cycle::Monthly, d("2026-09-10")), Some(d("2026-09-10")));
        assert_eq!(next_renewal(d("2026-05-10"), Cycle::Monthly, d("2026-09-11")), Some(d("2026-10-10")));
        // A future anchor is the next renewal as it stands.
        assert_eq!(next_renewal(d("2027-01-05"), Cycle::Yearly, d("2026-09-21")), Some(d("2027-01-05")));
        // Leap day, yearly.
        assert_eq!(next_renewal(d("2024-02-29"), Cycle::Yearly, d("2026-03-01")), Some(d("2027-02-28")));
        // Years of monthly steps still land exactly.
        assert_eq!(next_renewal(d("2019-03-15"), Cycle::Monthly, d("2026-09-21")), Some(d("2026-10-15")));
    }

    #[test]
    fn totals_put_yearly_plans_on_a_monthly_footing() {
        let items = [
            sub("Claude", 200.0, Cycle::Monthly, Some("2026-09-25"), Some("claude")),
            sub("Editor", 240.0, Cycle::Yearly, None, None),
        ];
        let v = view(&items, d("2026-09-21"), &HashMap::new());
        assert_eq!(v.monthly, 220.0);
        assert_eq!(v.yearly, 2640.0);
        assert_eq!(v.items[0].subscription.name, "Claude", "dated entries come first");
        assert_eq!(v.items[0].days_left, Some(4));
        assert_eq!(v.items[1].next_renewal, None);
    }

    #[test]
    fn value_is_measured_usage_over_price_and_idle_needs_a_measurement() {
        let items = [
            sub("Claude", 200.0, Cycle::Monthly, None, Some("claude")),
            sub("Copilot", 10.0, Cycle::Monthly, None, Some("copilot")),
            sub("Unknown", 20.0, Cycle::Monthly, None, Some("cursor")),
            sub("Unlinked", 30.0, Cycle::Monthly, None, None),
        ];
        let usage = HashMap::from([("claude".to_string(), 1900.0), ("copilot".to_string(), 0.0)]);
        let v = view(&items, d("2026-09-21"), &usage);
        let by = |n: &str| v.items.iter().find(|i| i.subscription.name == n).unwrap().clone();
        assert_eq!(by("Claude").value_ratio, Some(9.5));
        assert!(!by("Claude").idle);
        assert!(by("Copilot").idle, "measured, and nothing used");
        assert_eq!(by("Unknown").usage30, None);
        assert!(!by("Unknown").idle, "no measurement is not the same as no usage");
        assert!(!by("Unlinked").idle);
        assert_eq!(v.idle_monthly, 10.0);
    }

    #[test]
    fn entries_are_cleaned_and_bad_ones_refused() {
        let mut s = sub("  Claude Max  ", 200.0, Cycle::Monthly, Some(" 2026-10-05 "), Some(" claude "));
        s.notes = Some("   ".into());
        let ok = validate(s).unwrap();
        assert_eq!(ok.name, "Claude Max");
        assert_eq!(ok.renews_on.as_deref(), Some("2026-10-05"));
        assert_eq!(ok.provider.as_deref(), Some("claude"));
        assert_eq!(ok.notes, None);

        assert!(validate(sub("", 1.0, Cycle::Monthly, None, None)).is_err());
        assert!(validate(sub("x", f64::NAN, Cycle::Monthly, None, None)).is_err());
        assert!(validate(sub("x", -1.0, Cycle::Monthly, None, None)).is_err());
        assert!(validate(sub("x", 1.0, Cycle::Monthly, Some("2026-02-30"), None)).is_err());
        // A provider id with path characters is dropped, not stored.
        assert_eq!(validate(sub("x", 1.0, Cycle::Monthly, None, Some("../etc"))).unwrap().provider, None);
        assert_eq!(validate(sub(&"n".repeat(500), 1.0, Cycle::Monthly, None, None)).unwrap().name.len(), MAX_NAME);
    }

    #[test]
    fn a_renewal_reminds_once_and_the_next_one_reminds_again() {
        let mut items = vec![
            sub("Soon", 20.0, Cycle::Monthly, Some("2026-09-23"), None),
            sub("Later", 20.0, Cycle::Monthly, Some("2026-10-20"), None),
            sub("Undated", 20.0, Cycle::Monthly, None, None),
        ];
        let today = d("2026-09-21");
        let first = due_reminders(&mut items, today, 3);
        assert_eq!(first.iter().map(|r| r.subscription.name.as_str()).collect::<Vec<_>>(), ["Soon"]);
        assert!(due_reminders(&mut items, today, 3).is_empty(), "once per renewal");
        assert!(due_reminders(&mut items, d("2026-09-23"), 3).is_empty(), "still the same renewal");
        // A month on, the next renewal is a new one.
        assert_eq!(due_reminders(&mut items, d("2026-10-21"), 3).len(), 1);
        assert!(due_reminders(&mut items, today, 0).is_empty(), "0 turns reminders off");
    }

    #[test]
    fn the_file_round_trips_and_edits_keep_or_reset_the_reminder() {
        let dir = std::env::temp_dir().join(format!("aitm-ledger-{}", crate::providers::unique_stamp()));
        let path = dir.join("ledger.json");
        assert!(load_from(&path).is_empty(), "a missing file is an empty ledger");

        let mut new = sub("Claude", 200.0, Cycle::Monthly, Some("2026-09-23"), Some("claude"));
        new.id = String::new();
        let saved = upsert_in(&path, new).unwrap();
        assert!(saved.id.starts_with("sub-"));
        assert_eq!(take_reminders_in(&path, d("2026-09-21"), 3).len(), 1);
        assert!(take_reminders_in(&path, d("2026-09-21"), 3).is_empty(), "the sent mark was saved");

        // Editing the price keeps the mark; moving the date clears it.
        let mut edit = load_from(&path)[0].clone();
        edit.price = 100.0;
        upsert_in(&path, edit.clone()).unwrap();
        assert!(take_reminders_in(&path, d("2026-09-21"), 3).is_empty());
        edit.renews_on = Some("2026-09-24".into());
        upsert_in(&path, edit).unwrap();
        assert_eq!(take_reminders_in(&path, d("2026-09-21"), 3).len(), 1);
        assert_eq!(load_from(&path).len(), 1, "edits replace, never duplicate");

        delete_in(&path, &saved.id).unwrap();
        assert!(load_from(&path).is_empty());
        delete_in(&path, "missing").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
