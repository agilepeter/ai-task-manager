//! The weekly digest: one notification that says what the week cost, where
//! it went, and what renews next. Built from numbers the app already has.
//!
//! Sent once per ISO week, on the weekday the user picked, not before 9 in
//! the morning local time. A week with nothing to report sends nothing.

use crate::i18n::Msg;
use crate::ledger::LedgerView;
use crate::spend::ProviderSpend;
use chrono::{Datelike, NaiveDate, Weekday};

/// One Msg per sentence, joined with " " at the `tauri_plugin_notification`
/// call in `lib.rs` (today's `parts.join(" ")`), same as `Alert` -- ephemeral
/// and never painted by the popover, so there is no English rendering
/// carried alongside.
pub struct Digest {
    pub title: Msg,
    pub body: Vec<Msg>,
}

/// The test-side key registry: every `digest.*` key this module can emit.
/// Only `i18n.rs`'s test module reads this, so it does not exist in a
/// release build at all.
#[cfg(test)]
pub(crate) const DIGEST_KEYS: &[&str] = &[
    "digest.title",
    "digest.total",
    "digest.totalFlat",
    "digest.totalUp",
    "digest.totalDown",
    "digest.mostWentTo",
    "digest.renewal.today",
    "digest.renewal.tomorrow",
    "digest.renewal.inDays",
    "digest.renewal.many",
    "digest.setupChanged",
];

fn money(n: f64) -> String {
    if n >= 10.0 { format!("${:.0}", n) } else { format!("${:.2}", n) }
}

/// `{:.0}` -- the shared whole-number formatting for a percentage or a bare
/// dollar amount whose `%`/`$` sign is already literal in the surrounding
/// template (unlike `money()` above, which puts its own `$` on).
pub fn n0(x: f64) -> String {
    format!("{x:.0}")
}

/// "mon" … "sun". Anything else, including "off", is no digest.
pub fn weekday_of(setting: &str) -> Option<Weekday> {
    match setting.to_ascii_lowercase().as_str() {
        "mon" => Some(Weekday::Mon),
        "tue" => Some(Weekday::Tue),
        "wed" => Some(Weekday::Wed),
        "thu" => Some(Weekday::Thu),
        "fri" => Some(Weekday::Fri),
        "sat" => Some(Weekday::Sat),
        "sun" => Some(Weekday::Sun),
        _ => None,
    }
}

/// "2026-W39": the mark for "this week's digest has gone out".
pub fn week_mark(day: NaiveDate) -> String {
    let w = day.iso_week();
    format!("{}-W{:02}", w.year(), w.week())
}

/// Is a digest due now? On or after the chosen weekday (so a Mac that was
/// asleep on Monday still gets it on Tuesday), from 9:00, once per ISO week.
pub fn due(setting: &str, today: NaiveDate, hour: u32, last_sent: Option<&str>) -> bool {
    let Some(day) = weekday_of(setting) else { return false };
    let reached = today.weekday().num_days_from_monday() >= day.num_days_from_monday();
    let late_enough = today.weekday() != day || hour >= 9;
    reached && late_enough && last_sent != Some(week_mark(today).as_str())
}

fn last7(daily: &[f64]) -> f64 {
    daily.iter().rev().take(7).sum::<f64>() + 0.0
}

fn prior7(daily: &[f64]) -> f64 {
    daily.iter().rev().skip(7).take(7).sum::<f64>() + 0.0
}

/// `changes_this_week` is how many things the setup-history comparison
/// found (`changes::SetupChanges::changes.len()`, computed by the caller,
/// which already has the inventory scan this module has no business
/// running itself). A week with nothing else to say but a changed setup
/// still sends the digest -- the line below is not merely appended to an
/// otherwise-empty message, it can be the entire reason this week's digest
/// exists.
///
/// `changes_since` is that same comparison's start date
/// (`changes::SetupChanges::since`), plain ISO (`YYYY-MM-DD`): the digest has
/// no other date on its face to match, and this module has no locale-aware
/// date renderer of its own to reach for, so the line just says the day
/// plainly rather than pretending it was always "this week" -- true only by
/// coincidence, since the comparison can span anywhere from a couple of days
/// on a fresh install to the better part of a month if the app went unopened.
/// `None` only when `changes_this_week` is also 0 (no earlier snapshot means
/// no changes to report either), so the line is skipped rather than dated
/// with nothing to date it against.
pub fn build(spend: &[ProviderSpend], ledger: &LedgerView, changes_this_week: usize, changes_since: Option<&str>) -> Option<Digest> {
    let week: f64 = spend.iter().map(|p| last7(&p.daily_cost)).sum();
    let before: f64 = spend.iter().map(|p| prior7(&p.daily_cost)).sum();
    let renewing: Vec<&crate::ledger::ItemView> =
        ledger.items.iter().filter(|i| i.days_left.is_some_and(|d| (0..=7).contains(&d))).collect();
    // A count of changes with no day to measure them from is a line that cannot be written,
    // so it is no reason to send a digest with nothing in it.
    let changes_this_week = if changes_since.is_some() { changes_this_week } else { 0 };
    if week < 0.005 && renewing.is_empty() && changes_this_week == 0 {
        return None;
    }

    let mut body: Vec<Msg> = Vec::new();
    if week >= 0.005 {
        // The up/down clause used to be appended to this sentence as a
        // half-formed fragment (", up 12% on the week before" -- no capital,
        // no terminal period, not a sentence a translator could judge on
        // its own). Each shape below is instead a complete, independently
        // translatable sentence, chosen by which comparison applies; the
        // "no prior week" case (`before` too small to compare against) is
        // its own key rather than folded into `totalFlat`, since "about the
        // same" and "nothing to compare against" are different claims.
        let total = if before >= 1.0 {
            let pct = (week - before) / before * 100.0;
            if pct.abs() < 5.0 {
                Msg::new("digest.totalFlat").var("money", money(week))
            } else if pct > 0.0 {
                Msg::new("digest.totalUp").var("money", money(week)).var("pct", format!("{:.0}", pct.abs()))
            } else {
                Msg::new("digest.totalDown").var("money", money(week)).var("pct", format!("{:.0}", pct.abs()))
            }
        } else {
            Msg::new("digest.total").var("money", money(week))
        };
        body.push(total);

        // Where it went: the largest work area over 7 days, by top folder.
        let mut areas: std::collections::HashMap<&str, f64> = Default::default();
        for a in spend.iter().flat_map(|p| p.projects.iter()).flat_map(|pr| pr.areas.iter()) {
            *areas.entry(crate::spend::area_top(&a.area)).or_insert(0.0) += last7(&a.daily_cost);
        }
        if let Some((area, cost)) = areas
            .into_iter()
            .filter(|(a, c)| *c >= 0.005 && !a.starts_with('('))
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        {
            body.push(Msg::new("digest.mostWentTo").var("area", area).var("money", money(cost)));
        }
    }
    match renewing.as_slice() {
        [] => {}
        [one] => {
            let name = &one.subscription.name;
            let price = money(one.subscription.price);
            let msg = match one.days_left {
                Some(0) => Some(Msg::new("digest.renewal.today").var("name", name).var("money", &price)),
                Some(1) => Some(Msg::new("digest.renewal.tomorrow").var("name", name).var("money", &price)),
                // Only "one" is unreachable here (0 and 1 have their own
                // keys above); every n >= 2 the app can produce needs a
                // real plural form, so this is a genuine count.
                Some(n) => Some(Msg::new("digest.renewal.inDays").var("name", name).var("money", &price).count(n)),
                None => None,
            };
            body.extend(msg);
        }
        // `many` is only ever reached with 2+ items (`[]` and `[one]` match
        // first), so its "one" form is unreachable -- kept anyway because a
        // count-bearing key needs every form its locale's plural rule has.
        many => body.push(
            Msg::new("digest.renewal.many")
                .var("money", money(many.iter().map(|i| i.subscription.price).sum()))
                .count(many.len() as i64),
        ),
    }
    if changes_this_week > 0 {
        if let Some(date) = changes_since {
            body.push(Msg::new("digest.setupChanged").var("date", date).count(changes_this_week as i64));
        }
    }
    Some(Digest { title: Msg::new("digest.title"), body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{view, Cycle, Subscription};
    use crate::spend::{AreaSpend, ProjectSpend, Window};
    use std::collections::HashMap;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// Every test below already read as English; joining the rendered
    /// sentences with " " reproduces the single `body: String` these
    /// assertions were written against, before `Digest.body` became
    /// `Vec<Msg>`.
    fn en_body(body: &[Msg]) -> String {
        body.iter().map(|m| crate::i18n::render("en", m)).collect::<Vec<_>>().join(" ")
    }

    fn provider(daily_tail: &[f64], areas: &[(&str, &[f64])]) -> ProviderSpend {
        let pad = |tail: &[f64]| {
            let mut v = vec![0.0; 30 - tail.len()];
            v.extend_from_slice(tail);
            v
        };
        ProviderSpend {
            week: None,
            id: "claude".into(),
            name: "Claude".into(),
            today: Window::default(),
            yesterday: Window::default(),
            last30: Window::default(),
            trend: vec![],
            unpriced: 0,
            unpriced_models: vec![],
            daily_cost: pad(daily_tail),
            projects: vec![ProjectSpend {
                project: "/w".into(),
                today: Window::default(),
                yesterday: Window::default(),
                last30: Window::default(),
                areas: areas
                    .iter()
                    .map(|(a, tail)| AreaSpend {
                        week: None,
                        area: a.to_string(),
                        today: Window::default(),
                        yesterday: Window::default(),
                        last30: Window::default(),
                        daily_cost: pad(tail),
                    })
                    .collect(),
            }],
        }
    }

    fn ledger(renews_on: Option<&str>, today: &str) -> LedgerView {
        let items: Vec<Subscription> = renews_on
            .map(|r| Subscription {
                id: "s".into(),
                name: "Claude Max".into(),
                price: 200.0,
                cycle: Cycle::Monthly,
                renews_on: Some(r.into()),
                provider: None,
                notes: None,
                reminded_for: None,
            })
            .into_iter()
            .collect();
        view(&items, d(today), &HashMap::new())
    }

    /// The digest used to say "this week" for the setup-changed line no
    /// matter what the comparison actually spanned -- two days for a
    /// freshly-installed machine, or a month if the app went unopened for a
    /// while. It should instead name the actual day the comparison started
    /// from, the same date `changes::SetupChanges::since` already carries.
    #[test]
    fn the_setup_line_names_the_day_it_compares_with() {
        let quiet = provider(&[0.0; 14], &[]);
        let got = build(&[quiet], &ledger(None, "2026-09-21"), 3, Some("2026-09-14")).unwrap();
        assert_eq!(en_body(&got.body), "3 things changed in your setup since 2026-09-14.");
    }

    #[test]
    fn it_goes_out_once_a_week_from_nine_and_catches_up_after_a_missed_day() {
        let monday = d("2026-09-21");
        assert!(!due("mon", monday, 8, None), "not before nine");
        assert!(due("mon", monday, 9, None));
        assert!(!due("mon", monday, 12, Some("2026-W39")), "already sent this week");
        assert!(due("mon", d("2026-09-23"), 7, None), "asleep on Monday: Wednesday still sends, any hour");
        assert!(due("mon", d("2026-09-28"), 9, Some("2026-W39")), "a new week");
        assert!(!due("fri", monday, 12, None), "Friday has not come yet");
        assert!(!due("off", monday, 12, None));
        assert!(!due("", monday, 12, None));
        assert_eq!(week_mark(d("2027-01-01")), "2026-W53", "ISO weeks, not calendar years");
    }

    #[test]
    fn it_says_what_the_week_cost_where_it_went_and_what_renews() {
        let mut tail = vec![10.0; 7]; // the week before: 70
        tail.extend_from_slice(&[20.0; 7]); // this week: 140
        let acme: Vec<f64> = vec![15.0; 7];
        let misc: Vec<f64> = vec![5.0; 7];
        let sp = provider(&tail, &[("acme/web", &acme), ("tools", &misc), ("(unsorted)", &[99.0; 7])]);
        let got = build(&[sp], &ledger(Some("2026-09-24"), "2026-09-21"), 0, None).unwrap();
        assert_eq!(crate::i18n::render("en", &got.title), "Your AI week");
        assert_eq!(
            en_body(&got.body),
            "$140 of AI usage in 7 days, up 100% on the week before. Most went to acme ($105). Claude Max renews in 3 days ($200)."
        );
    }

    #[test]
    fn a_steady_week_says_so_and_an_empty_one_says_nothing() {
        let steady = provider(&[10.0; 14], &[]);
        let body = en_body(&build(&[steady], &ledger(None, "2026-09-21"), 0, None).unwrap().body);
        assert_eq!(body, "$70 of AI usage in 7 days, about the same as the week before.");
        assert!(build(&[provider(&[], &[])], &ledger(None, "2026-09-21"), 0, None).is_none());
        // Nothing spent, but a renewal is still worth the note.
        let only_renewal = build(&[provider(&[], &[])], &ledger(Some("2026-09-21"), "2026-09-21"), 0, None).unwrap();
        assert_eq!(en_body(&only_renewal.body), "Claude Max renews today ($200).");
    }

    #[test]
    fn a_changed_setup_gets_its_own_line_and_can_carry_the_digest_alone() {
        let quiet = provider(&[0.0; 14], &[]);
        // No spend and no renewal: on its own this week sends nothing at all
        // (covered above). Three setup changes are reason enough by themselves.
        let got = build(&[quiet.clone()], &ledger(None, "2026-09-21"), 3, Some("2026-09-14")).unwrap();
        assert_eq!(en_body(&got.body), "3 things changed in your setup since 2026-09-14.");
        assert!(build(&[quiet], &ledger(None, "2026-09-21"), 0, None).is_none(), "still nothing to say with zero changes");

        // A normal week with spend gains the line at the end, after everything else.
        let steady = provider(&[10.0; 14], &[]);
        let with_changes = build(&[steady], &ledger(None, "2026-09-21"), 1, Some("2026-09-14")).unwrap();
        assert_eq!(
            en_body(&with_changes.body),
            "$70 of AI usage in 7 days, about the same as the week before. 1 thing changed in your setup since 2026-09-14."
        );
    }

    #[test]
    fn changes_with_no_day_to_count_from_send_nothing() {
        let quiet = provider(&[0.0; 14], &[]);
        let got = build(&[quiet], &ledger(None, "2026-09-21"), 3, None);
        assert!(got.is_none(), "never a notification with a title and no body");
    }
}
