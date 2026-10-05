//! How long each limit sat at 100 percent over the last 30 days, worked out
//! from the readings the history store already keeps. Nothing new is read.
//!
//! The history store writes a reading when the value moves, and at least once
//! an hour while the app runs and the value is flat. So while a limit sits at
//! 100 and the app is running there is a reading at least hourly; a longer
//! gap means the app was not watching, and that stretch is skipped rather
//! than guessed. The counting is pure: no clock, no database.

use crate::i18n::{self, Msg};
use crate::inventory::Opportunity;
use serde::Serialize;

/// The test-side key registry (see inventory::FINDING_IDS).
#[cfg(test)]
pub(crate) const FINDING_IDS: &[&str] = &["limit-time"];

/// A reading at or above this is a limit that has been reached.
pub const AT_LIMIT: f64 = 100.0;
/// Two readings further apart than this say nothing about the time between
/// them. The same rule the burn profile uses for "when was it used".
pub const MAX_GAP_MS: i64 = 90 * 60_000;
/// The span the page and the finding look back over.
pub const WINDOW_MS: i64 = 30 * 24 * 3_600_000;
/// A limit's total at 100 percent that is worth a finding.
pub const FINDING_AT_MS: i64 = 2 * 3_600_000;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LimitTime {
    pub provider: String,
    pub metric: String,
    /// Separate stretches at the limit, counted even when no time could be
    /// measured for one (a single reading with nothing around it).
    pub times: u32,
    pub total_ms: i64,
    /// The most measured time inside one stretch.
    pub longest_ms: i64,
}

/// The stretches at the limit for one metric. `points` are
/// `(at, used, resets_at)`, oldest first; `None` when it was never reached.
///
/// Walking consecutive pairs A then B, a pair counts only when A is at the
/// limit and the two are within `MAX_GAP_MS`:
/// - A's own reset time falls strictly between them: the limit held until the
///   reset, so that much counts, and the stretch ends there;
/// - otherwise, B at the limit too: the whole gap counts;
/// - otherwise B is lower with no reset between: nobody knows when it dropped,
///   so nothing counts and the stretch ends.
///
/// A pair further apart than the gap ends the stretch. Out-of-order readings
/// count nothing and end it; a reset time already past is simply not between.
pub fn from_points(provider: &str, metric: &str, points: &[(i64, f64, Option<i64>)]) -> Option<LimitTime> {
    let (mut times, mut total, mut longest) = (0u32, 0i64, 0i64);
    let (mut open, mut run) = (false, 0i64);
    for (i, &(at, used, resets_at)) in points.iter().enumerate() {
        if used < AT_LIMIT || used.is_nan() {
            longest = longest.max(run);
            open = false;
            continue;
        }
        if !open {
            times = times.saturating_add(1);
            open = true;
            run = 0;
        }
        let Some(&(next_at, next_used, _)) = points.get(i + 1) else { break };
        let gap = next_at.saturating_sub(at);
        let mut continues = false;
        if (0..=MAX_GAP_MS).contains(&gap) {
            let counted = match resets_at {
                Some(r) if r > at && r < next_at => Some(r - at),
                _ if next_used >= AT_LIMIT => {
                    continues = true;
                    Some(gap)
                }
                _ => None,
            };
            if let Some(ms) = counted {
                total = total.saturating_add(ms);
                run = run.saturating_add(ms);
            }
        }
        if !continues {
            longest = longest.max(run);
            open = false;
        }
    }
    longest = longest.max(run);
    (times > 0).then(|| LimitTime { provider: provider.to_string(), metric: metric.to_string(), times, total_ms: total, longest_ms: longest })
}

/// At most one finding, present only when some limit spent `FINDING_AT_MS` or
/// more at 100 percent. The title is a count of such limits and nothing else:
/// titles leave the machine in the seat report, and a provider, a card or a
/// metric name can hold an account. The detail carries the figures of the
/// limit that spent longest, with every duration and count a nested `Msg` so
/// each language words it itself; which limit it was is on the provider's page.
pub fn opportunities(rows: &[LimitTime]) -> Vec<Opportunity> {
    let over: Vec<&LimitTime> = rows.iter().filter(|r| r.total_ms >= FINDING_AT_MS).collect();
    let Some(top) = over.iter().copied().max_by_key(|r| r.total_ms) else {
        return Vec::new();
    };
    let (title, detail) = messages(top, over.len());
    vec![Opportunity::from_msgs("limit-time", "learn", title, Some(detail), None)]
}

/// The finding's two messages, split out so the wording can be exercised at
/// sizes below the threshold.
fn messages(top: &LimitTime, limits: usize) -> (Msg, Msg) {
    (
        Msg::new("finding.limit-time.title").count(limits as i64),
        Msg::new("finding.limit-time.detail")
            .sub("total", duration(top.total_ms))
            .sub("times", Msg::new("unit.times").count(i64::from(top.times)))
            .sub("longest", duration(top.longest_ms)),
    )
}

fn duration(ms: i64) -> Msg {
    i18n::duration_msg((ms.max(0) / 60_000) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;
    const T0: i64 = 1_790_000_000_000;

    fn pt(at_min: i64, used: f64) -> (i64, f64, Option<i64>) {
        (T0 + at_min * MIN, used, None)
    }

    fn pt_reset(at_min: i64, used: f64, reset_min: i64) -> (i64, f64, Option<i64>) {
        (T0 + at_min * MIN, used, Some(T0 + reset_min * MIN))
    }

    fn lt(provider: &str, metric: &str, total_ms: i64) -> LimitTime {
        LimitTime { provider: provider.into(), metric: metric.into(), times: 2, total_ms, longest_ms: total_ms / 2 }
    }

    #[test]
    fn time_at_the_limit_skips_gaps_the_app_did_not_watch() {
        let points = [
            pt(0, 100.0),
            pt(60, 100.0),  // 60 min counted
            pt(120, 100.0), // 60 more
            pt(400, 100.0), // 280 min later: the app was not watching, nothing counted
            pt(460, 100.0), // 60 more, a new stretch
        ];
        let got = from_points("p", "Weekly", &points).unwrap();
        assert_eq!(got.total_ms, 180 * MIN);
        assert_eq!(got.times, 2, "the unwatched gap ends the stretch");
        assert_eq!(got.longest_ms, 120 * MIN);
        // Exactly at the gap counts; one millisecond over does not.
        let edge = [(0, 100.0, None), (MAX_GAP_MS, 100.0, None)];
        assert_eq!(from_points("p", "m", &edge).unwrap().total_ms, MAX_GAP_MS);
        let over = [(0, 100.0, None), (MAX_GAP_MS + 1, 100.0, None)];
        assert_eq!(from_points("p", "m", &over).unwrap().total_ms, 0);
    }

    #[test]
    fn a_span_ends_at_the_reset() {
        // At the limit at minute 0 with a reset at minute 40; the next reading,
        // at 60, is back down. The limit held for 40 minutes.
        let points = [pt_reset(0, 100.0, 40), pt(60, 3.0)];
        let got = from_points("p", "Session", &points).unwrap();
        assert_eq!((got.times, got.total_ms, got.longest_ms), (1, 40 * MIN, 40 * MIN));
        // Still at the limit after the reset: that reading starts a new stretch.
        let again = [pt_reset(0, 100.0, 40), pt(60, 100.0), pt(100, 100.0)];
        let got = from_points("p", "Session", &again).unwrap();
        assert_eq!(got.times, 2);
        assert_eq!(got.total_ms, 40 * MIN + 40 * MIN, "40 to the reset, then 40 from the new stretch");
        assert_eq!(got.longest_ms, 40 * MIN);
        // A reset at the later reading's own instant, or already past, is not between them.
        let on = [pt_reset(0, 100.0, 60), pt(60, 100.0)];
        assert_eq!(from_points("p", "s", &on).unwrap().total_ms, 60 * MIN);
        let past = [pt_reset(30, 100.0, 10), pt(60, 100.0)];
        assert_eq!(from_points("p", "s", &past).unwrap().total_ms, 30 * MIN);
    }

    #[test]
    fn a_limit_never_reached_says_nothing() {
        assert!(from_points("p", "Weekly", &[]).is_none());
        assert!(from_points("p", "Weekly", &[pt(0, 40.0), pt(30, 99.9), pt(60, 12.0)]).is_none());
        assert!(from_points("p", "Weekly", &[pt(0, f64::NAN)]).is_none());
    }

    #[test]
    fn two_runs_at_the_limit_count_as_two_times() {
        let points = [
            pt(0, 100.0),
            pt(30, 100.0),               // 30
            pt_reset(60, 100.0, 70),     // 30 more
            pt(90, 20.0),                // reset at 70: 10 more, then it is below the limit
            pt(120, 50.0),
            pt(150, 100.0), // a second run
            pt(200, 100.0), // 50
        ];
        let got = from_points("p", "Weekly", &points).unwrap();
        assert_eq!(got.times, 2);
        assert_eq!(got.total_ms, 120 * MIN);
        assert_eq!(got.longest_ms, 70 * MIN, "the first run held 30 + 30 + 10, the second only 50");
    }

    #[test]
    fn a_drop_below_the_limit_with_no_reset_counts_nothing() {
        let points = [pt(0, 100.0), pt(30, 100.0), pt(60, 80.0)];
        let got = from_points("p", "Weekly", &points).unwrap();
        assert_eq!(got.total_ms, 30 * MIN, "the last half hour is unknown, not counted");
        assert_eq!(got.times, 1);
        // A reset time that is not between the two does not rescue it.
        let late = [pt_reset(0, 100.0, 90), pt(60, 80.0)];
        assert_eq!(from_points("p", "Weekly", &late).unwrap().total_ms, 0);
    }

    #[test]
    fn a_single_reading_at_the_limit_is_one_time_and_no_time() {
        let got = from_points("p", "Weekly", &[pt(0, 100.0)]).unwrap();
        assert_eq!((got.times, got.total_ms, got.longest_ms), (1, 0, 0));
        let beside = [pt(0, 10.0), pt(30, 100.0), pt(60, 10.0)];
        let got = from_points("p", "Weekly", &beside).unwrap();
        assert_eq!((got.times, got.total_ms), (1, 0));
    }

    #[test]
    fn disordered_readings_never_panic_or_go_negative() {
        let shuffled = [pt(60, 100.0), pt(0, 100.0), pt(30, 100.0), pt(30, 100.0), pt_reset(40, 100.0, 5), pt(10, 100.0)];
        let got = from_points("p", "Weekly", &shuffled).unwrap();
        assert!(got.total_ms >= 0 && got.longest_ms >= 0 && got.longest_ms <= got.total_ms, "{got:?}");
        assert!(got.times >= 1);
        // A reset in the past, at an extreme, and the largest timestamps.
        let extreme = [(i64::MIN, 100.0, Some(i64::MAX)), (i64::MAX, 100.0, Some(i64::MIN)), (i64::MAX, 100.0, None)];
        let got = from_points("p", "Weekly", &extreme).unwrap();
        assert!(got.total_ms >= 0);
        let same = [pt(5, 100.0), pt(5, 100.0)];
        assert_eq!(from_points("p", "Weekly", &same).unwrap().total_ms, 0);
    }

    #[test]
    fn the_finding_needs_two_hours() {
        assert_eq!(FINDING_AT_MS, 2 * HOUR, "the strings say two hours in words");
        assert!(opportunities(&[]).is_empty());
        assert!(opportunities(&[lt("claude", "Weekly", FINDING_AT_MS - 1)]).is_empty(), "just under says nothing");
        let found = opportunities(&[lt("claude", "Weekly", FINDING_AT_MS)]);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].id.as_str(), found[0].kind.as_str()), ("limit-time", "learn"));
        assert_eq!(found[0].title, "1 limit spent two hours or more at 100% in the last 30 days");
        // The count is the number of limits at or over, and the detail is the longest one.
        let rows = [lt("claude", "Weekly", 3 * HOUR), lt("codex", "Session", 5 * HOUR + 10 * MIN), lt("claude", "Session", HOUR)];
        let found = opportunities(&rows);
        assert_eq!(found.len(), 1, "at most one finding");
        assert_eq!(found[0].title_msg.count, Some(2));
        assert_eq!(found[0].title, "2 limits spent two hours or more at 100% in the last 30 days");
        assert_eq!(
            found[0].detail,
            "Over the last 30 days, the limit that sat at 100% longest spent 5h 10m there, 2 times, and 2h 35m at a stretch. While a limit is at 100%, that tool cannot be used on your plan. Each provider's page shows which limit and when."
        );
    }

    #[test]
    fn the_finding_title_carries_a_count_only() {
        use crate::inventory::Inventory;
        const PROVIDER: &str = "northwind-seat-card@ab12cd34";
        const METRIC: &str = "Northwind Reviewer weekly";
        let rows = [lt(PROVIDER, METRIC, 9 * HOUR)];
        let found = opportunities(&rows);
        assert_eq!(found.len(), 1, "the finding has to exist, or this proves nothing");
        assert!(found[0].title_msg.vars.is_empty(), "a title carries a count only");
        for part in [&found[0].title, &found[0].detail] {
            assert!(!part.contains("northwind") && !part.contains("Northwind"), "{part}");
        }
        let inv = Inventory { opportunities: found, ..Inventory::default() };
        let report = crate::seat::build_with("seat-abcdefgh", "Dana's MacBook", 1, &inv, &[], &[]);
        assert_eq!(report.findings.len(), 1, "the finding does reach the report, as its title");
        let wire = serde_json::to_string(&report).unwrap();
        for marker in [PROVIDER, "northwind", "Northwind", METRIC, "ab12cd34"] {
            assert!(!wire.contains(marker), "{marker} leaked into {wire}");
        }
    }

    #[test]
    fn the_finding_reads_whole_in_every_language_and_size() {
        let durations = [("45 minutes", 45 * MIN), ("2 hours 10 minutes", 2 * HOUR + 10 * MIN), ("1 day 3 hours", 27 * HOUR)];
        for locale in i18n::LOCALES {
            for (name, total) in durations {
                for times in [1u32, 2, 5, 21] {
                    let row = LimitTime { provider: "p".into(), metric: "m".into(), times, total_ms: total, longest_ms: total / 3 };
                    let (title_msg, detail_msg) = messages(&row, times as usize);
                    let text = i18n::render(locale, &detail_msg);
                    assert!(!text.contains('{') && !text.contains('}'), "{locale} {name} {times}: {text}");
                    assert!(!text.contains("finding.") && !text.contains("unit.") && !text.contains("time."), "{locale}: {text}");
                    if *locale != "en" {
                        assert_ne!(text, i18n::render("en", &detail_msg), "{locale} still reads in English");
                    }
                    if std::env::var("SHOW_LIMIT_TIME").is_ok() {
                        eprintln!("[{locale}] {name} x{times}: {text}");
                    }
                    let title = i18n::render(locale, &title_msg);
                    assert!(!title.contains('{') && !title.contains("finding."), "{locale}: {title}");
                    if std::env::var("SHOW_LIMIT_TIME").is_ok() {
                        eprintln!("[{locale}] title x{times}: {title}");
                    }
                }
            }
        }
    }
}
