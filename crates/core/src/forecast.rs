//! When will this limit run out? A forecast from the recent rate of use.
//!
//! The pace rules in `alerts.rs` draw one straight line from the start of the
//! period. That answers "am I ahead of an even burn?" but not "at the rate I
//! have been going lately, when do I hit the wall?", which is the question
//! people ask on a heavy afternoon. This uses the local history of readings
//! for a recent rate and falls back to the period average without enough of it.

use serde::Serialize;

const HOUR_MS: i64 = 3_600_000;
/// A fall of at least this many points between readings is a reset, not use.
const RESET_DROP: f64 = 10.0;
/// The recent window must cover at least this long to be a rate.
const MIN_RECENT_SPAN_MS: i64 = 20 * 60_000;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Forecast {
    pub metric: String,
    /// "recent": from the last hours of history. "period": the average since
    /// the period began, used when recent history is too thin.
    pub basis: String,
    /// Hours of history the recent rate was measured over.
    pub window_hours: f64,
    /// Points of the limit used per hour at that rate.
    pub rate_per_hour: f64,
    /// When the limit reaches 100% at that rate, if that is before the reset.
    pub hits_limit_at: Option<i64>,
    /// Where usage stands at the reset if the rate holds, capped at 100.
    pub projected_at_reset: Option<f64>,
}

/// How far back "recent" looks: a seventh of the period, between one hour
/// and a day. A weekly limit looks at the last day; a 5-hour one at the last hour.
fn lookback_ms(period_ms: Option<i64>) -> i64 {
    period_ms.map_or(24 * HOUR_MS, |p| (p / 7).clamp(HOUR_MS, 24 * HOUR_MS))
}

/// Readings since the last reset, oldest first.
fn since_reset(points: &[(i64, f64)]) -> &[(i64, f64)] {
    let start = points
        .windows(2)
        .rposition(|w| w[1].1 + RESET_DROP <= w[0].1)
        .map_or(0, |i| i + 1);
    &points[start..]
}

/// `points` are `(epoch ms, used %)`, oldest first. `used` is the live reading.
pub fn forecast(
    metric: &str,
    points: &[(i64, f64)],
    used: f64,
    resets_at: Option<i64>,
    period_ms: Option<i64>,
    now: i64,
) -> Option<Forecast> {
    if !used.is_finite() {
        return None;
    }
    let used = used.clamp(0.0, 100.0);
    let window = lookback_ms(period_ms);
    let recent: Vec<(i64, f64)> = since_reset(points)
        .iter()
        .copied()
        .filter(|(at, _)| *at <= now && now - at <= window)
        .collect();

    // Recent rate: from the oldest reading in the window to the live value.
    let recent_rate = recent.first().and_then(|(at, then)| {
        let span = now - at;
        (span >= MIN_RECENT_SPAN_MS).then(|| ((used - then) / (span as f64 / HOUR_MS as f64), span))
    });
    // Period average: needs to know when the period began.
    let period_rate = match (resets_at, period_ms) {
        (Some(resets), Some(period)) if period > 0 => {
            let elapsed = period - (resets - now).max(0);
            (elapsed >= MIN_RECENT_SPAN_MS).then(|| used / (elapsed as f64 / HOUR_MS as f64))
        }
        _ => None,
    };

    let (basis, rate, span) = match (recent_rate, period_rate) {
        (Some((r, span)), _) if r > 0.0 => ("recent", r, span),
        // Idle lately: the honest forecast is the flat line, not the average.
        (Some((_, span)), _) => ("recent", 0.0, span),
        (None, Some(r)) => ("period", r, 0),
        (None, None) => return None,
    };

    let hours_to_reset = resets_at.map(|r| (r - now).max(0) as f64 / HOUR_MS as f64);
    let hits = (rate > 0.0 && used < 100.0)
        .then(|| now + (((100.0 - used) / rate) * HOUR_MS as f64) as i64)
        .filter(|at| resets_at.is_none_or(|r| *at < r));
    Some(Forecast {
        metric: metric.to_string(),
        basis: basis.to_string(),
        window_hours: span as f64 / HOUR_MS as f64,
        rate_per_hour: rate,
        hits_limit_at: hits,
        projected_at_reset: hours_to_reset.map(|h| (used + rate * h).min(100.0)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEEK: i64 = 7 * 24 * HOUR_MS;
    const NOW: i64 = 1_000 * 24 * HOUR_MS;

    #[test]
    fn a_heavy_day_forecasts_the_wall_that_the_period_average_misses() {
        // Day 4 of 7, 40% used: an even burn says ~70% at reset, fine.
        // But the last 24 hours alone used 24 points: 1 point an hour.
        let resets = NOW + 3 * 24 * HOUR_MS;
        let points = [(NOW - 30 * HOUR_MS, 10.0), (NOW - 24 * HOUR_MS, 16.0), (NOW - 6 * HOUR_MS, 34.0)];
        let f = forecast("Weekly", &points, 40.0, Some(resets), Some(WEEK), NOW).unwrap();
        assert_eq!(f.basis, "recent");
        assert!((f.rate_per_hour - 1.0).abs() < 1e-9, "{}", f.rate_per_hour);
        assert_eq!(f.hits_limit_at, Some(NOW + 60 * HOUR_MS), "60 points left at 1 an hour");
        assert_eq!(f.projected_at_reset, Some(100.0));
        assert!((f.window_hours - 24.0).abs() < 1e-9, "the 30-hour-old reading is outside a weekly lookback");
    }

    #[test]
    fn a_limit_that_outlasts_the_reset_has_no_wall_only_a_landing_point() {
        let resets = NOW + 10 * HOUR_MS;
        let points = [(NOW - 10 * HOUR_MS, 20.0)];
        let f = forecast("Weekly", &points, 30.0, Some(resets), Some(WEEK), NOW).unwrap();
        assert_eq!(f.hits_limit_at, None, "70 hours to the wall, 10 to the reset");
        assert_eq!(f.projected_at_reset, Some(40.0));
    }

    #[test]
    fn a_reset_in_the_history_is_not_negative_use() {
        // 90% -> 2% is the window rolling over; only the climb after it counts.
        let points = [(NOW - 5 * HOUR_MS, 90.0), (NOW - 4 * HOUR_MS, 2.0), (NOW - 2 * HOUR_MS, 6.0)];
        let f = forecast("Weekly", &points, 10.0, Some(NOW + 6 * 24 * HOUR_MS), Some(WEEK), NOW).unwrap();
        assert!((f.rate_per_hour - 2.0).abs() < 1e-9, "8 points in the 4 hours since the reset");
    }

    #[test]
    fn a_quiet_stretch_forecasts_flat_rather_than_the_old_average() {
        let points = [(NOW - 20 * HOUR_MS, 50.0), (NOW - 10 * HOUR_MS, 50.0)];
        let f = forecast("Weekly", &points, 50.0, Some(NOW + 2 * 24 * HOUR_MS), Some(WEEK), NOW).unwrap();
        assert_eq!(f.rate_per_hour, 0.0);
        assert_eq!(f.hits_limit_at, None);
        assert_eq!(f.projected_at_reset, Some(50.0));
    }

    #[test]
    fn thin_history_falls_back_to_the_period_average_and_says_so() {
        // One reading five minutes old is not a rate.
        let points = [(NOW - 5 * 60_000, 49.0)];
        let resets = NOW + 2 * 24 * HOUR_MS; // 5 of 7 days elapsed
        let f = forecast("Weekly", &points, 50.0, Some(resets), Some(WEEK), NOW).unwrap();
        assert_eq!(f.basis, "period");
        assert!((f.rate_per_hour - 50.0 / 120.0).abs() < 1e-9);
        assert_eq!(f.projected_at_reset, Some(70.0));
        // No history and no period: nothing honest to say.
        assert_eq!(forecast("Credits", &[], 50.0, None, None, NOW), None);
        assert_eq!(forecast("Weekly", &points, f64::NAN, Some(resets), Some(WEEK), NOW), None);
    }

    #[test]
    fn a_short_window_looks_back_an_hour_not_a_day() {
        let session = 5 * HOUR_MS;
        // Three hours ago was a different burst; the last hour is the rate.
        let points = [(NOW - 3 * HOUR_MS, 5.0), (NOW - 50 * 60_000, 40.0)];
        let f = forecast("Session", &points, 60.0, Some(NOW + HOUR_MS), Some(session), NOW).unwrap();
        assert!((f.rate_per_hour - 24.0).abs() < 1e-9, "20 points in 50 minutes");
        assert!(f.hits_limit_at.is_none(), "100 minutes to the wall, 60 to the reset");
        assert_eq!(f.projected_at_reset, Some(84.0));
    }
}
