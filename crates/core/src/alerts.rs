//! Pace-based notification rules, mirroring the Mac app:
//! - "Almost Out" — a metric drops under 10% remaining.
//! - "Cutting It Close" — projected to finish the period with <10% spare.
//! - "Will Run Out" — projected to hit the limit before the reset.
//! - "Limit Reset" — a weekly-or-longer reset window rolled over and the
//!   quota is full (short windows reset too often to be worth a toast).
//! - "Burning Fast" — a weekly-or-longer quota rose by N points inside 30
//!   minutes. The projection rules above are straight lines from the period
//!   start, so an agent fan-out early in the week looks fine to them until
//!   much later; this one watches the rate itself.
//! - "Daily Spend" — today's local spend crossed the amount the user set.
//!
//! Anti-spam: an alert fires only when a quota *worsens while the app is
//! running* (the first reading after launch is a silent baseline), fires
//! once per state, re-arms if the metric recovers, and the slate is wiped
//! when a new reset period begins. State is in-memory by design — matching
//! the Mac's "already-bad at launch won't alert" behavior.

use crate::digest::n0;
use crate::i18n::Msg;
use crate::providers::Snapshot;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Default, Clone)]
struct MetricState {
    resets_at: Option<i64>,
    prev_used: Option<f64>,
    seen: bool,
    almost_out: bool,
    close: bool,
    run_out: bool,
    /// Recent (time, used%) readings, oldest first, for the burn-rate rule.
    history: Vec<(i64, f64)>,
    burning: bool,
}

fn states() -> &'static Mutex<HashMap<String, MetricState>> {
    static STATES: OnceLock<Mutex<HashMap<String, MetricState>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Ephemeral: evaluated and fired in the same beat, never painted by the
/// popover and never stored, so unlike `Opportunity`/`Diagnosis` there is no
/// English rendering carried alongside -- `title`/`body` are rendered once,
/// at the `tauri_plugin_notification` call in `lib.rs`, in the locale that
/// call already has in scope.
pub struct Alert {
    pub title: Msg,
    pub body: Msg,
}

/// The test-side key registry: every `alert.<case>.title`/`alert.<case>…`
/// key this module can emit. Only `i18n.rs`'s test module reads this, so it
/// does not exist in a release build at all.
#[cfg(test)]
pub(crate) const ALERT_KEYS: &[&str] = &[
    "alert.reset.title",
    "alert.reset.backTo100",
    "alert.reset.available",
    "alert.runOut.title",
    "alert.runOut.body",
    "alert.close.title",
    "alert.close.body",
    "alert.almostOut.title",
    "alert.almostOut.body",
    "alert.burningFast.title",
    "alert.burningFast.body",
    "alert.dailySpend.title",
    "alert.dailySpend.body",
];

#[derive(PartialEq, Clone, Copy)]
enum Verdict {
    Ok,
    Close,
    RunOut,
}

/// Same straight-line projection the UI uses for bar colors.
fn verdict(used: f64, resets_at: Option<i64>, period_ms: Option<i64>) -> (Verdict, f64) {
    let used = used.clamp(0.0, 100.0);
    let left = 100.0 - used;
    if left < 0.5 {
        return (Verdict::RunOut, 0.0);
    }
    let (Some(resets_at), Some(period_ms)) = (resets_at, period_ms) else {
        return (Verdict::Ok, left);
    };
    if period_ms <= 0 {
        return (Verdict::Ok, left);
    }
    let now = chrono::Utc::now().timestamp_millis();
    let remain = (resets_at - now).max(0);
    let elapsed = period_ms - remain;
    let frac = elapsed as f64 / period_ms as f64;
    if frac < 0.05 || elapsed < 5 * 60_000 {
        return (Verdict::Ok, left);
    }
    let projected = used / frac;
    let spare = (100.0 - projected).max(0.0);
    if projected >= 100.0 {
        (Verdict::RunOut, 0.0)
    } else if projected >= 90.0 {
        (Verdict::Close, spare.max(1.0))
    } else {
        (Verdict::Ok, spare)
    }
}

/// A reset time that moved by more than ten minutes means a new period
/// (small drifts happen because some providers report "seconds from now").
fn period_changed(old: Option<i64>, new: Option<i64>) -> bool {
    match (old, new) {
        (Some(a), Some(b)) => (a - b).abs() > 10 * 60_000,
        _ => false,
    }
}

/// Reset toasts only cover weekly-and-longer windows — 5-hour and daily
/// windows roll too often to be worth a notification.
const LONG_WINDOW_MS: i64 = 6 * 24 * 3_600_000;

/// Is this metric's window long enough to toast about? The declared
/// `period_ms` wins; when a provider reports no period, the jump between
/// consecutive reset times IS the period.
fn long_window(
    period_ms: Option<i64>,
    old_reset: Option<i64>,
    new_reset: Option<i64>,
) -> bool {
    period_ms.is_some_and(|p| p >= LONG_WINDOW_MS)
        || (period_ms.is_none()
            && matches!((old_reset, new_reset), (Some(o), Some(n)) if n - o >= LONG_WINDOW_MS))
}

/// Compact duration for toast text: `6d 23h`, `4h 58m`, `12m`. Negative
/// durations clamp to `0m`.
fn compact_duration(ms: i64) -> String {
    let ms = ms.max(0);
    const MIN: i64 = 60_000;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    if ms >= DAY {
        format!("{}d {}h", ms / DAY, (ms % DAY) / HOUR)
    } else if ms >= HOUR {
        format!("{}h {}m", ms / HOUR, (ms % HOUR) / MIN)
    } else {
        format!("{}m", ms / MIN)
    }
}

/// The burn-rate rule looks this far back.
const BURN_WINDOW_MS: i64 = 30 * 60_000;
/// Readings closer together than this are not a rate yet.
const BURN_MIN_SPAN_MS: i64 = 2 * 60_000;

/// How far `used` has risen above the oldest reading still inside the burn
/// window: `(points, minutes)`. None without a reading old enough to compare.
fn burn_rise(history: &[(i64, f64)], now: i64, used: f64) -> Option<(f64, i64)> {
    let (at, then) = *history.iter().find(|(at, _)| now - at <= BURN_WINDOW_MS)?;
    let span = now - at;
    (span >= BURN_MIN_SPAN_MS).then_some((used - then, span / 60_000))
}

pub fn evaluate(snapshots: &[Snapshot], cfg: &Value) -> Vec<Alert> {
    evaluate_at(snapshots, cfg, chrono::Utc::now().timestamp_millis())
}

/// `evaluate` with the clock passed in, so the burn window is testable.
pub fn evaluate_at(snapshots: &[Snapshot], cfg: &Value, now: i64) -> Vec<Alert> {
    let want = |key: &str| cfg.get(key).and_then(Value::as_bool).unwrap_or(false);
    let burn_points = cfg.get("burnAlertPoints").and_then(Value::as_f64).unwrap_or(0.0);
    let want_burn = burn_points > 0.0;
    let want_almost = want("notifyAlmostOut");
    let want_close = want("notifyCuttingClose");
    let want_runout = want("notifyWillRunOut");
    let want_reset = want("notifyReset");
    if !(want_almost || want_close || want_runout || want_reset || want_burn) {
        return Vec::new();
    }

    let mut alerts = Vec::new();
    let Ok(mut map) = states().lock() else { return alerts };

    for snapshot in snapshots.iter().filter(|s| s.status == "ok") {
        // Restored snapshots past the UI grace window are last-good, not
        // live. Skipping them means stale data can neither fire a new
        // worsening alert nor reset the armed state the live data left.
        // The 3-minute grace (`stale` still false, `attempt_failed` true)
        // still alerts — same as today.
        if snapshot.stale {
            continue;
        }
        if snapshot.id.starts_with("sub2api@") {
            let disabled = cfg.get("disabled").and_then(Value::as_array);
            if disabled.is_some_and(|ids| ids.iter().any(|id| {
                id.as_str().is_some_and(|id| id == "sub2api" || id == snapshot.id)
            })) {
                continue;
            }
        }
        for metric in snapshot.metrics.iter().filter(|m| m.kind == "progress") {
            // Restored Kimi API rows are last-known, not live — don't
            // fire Almost Out off a wallet timeout.
            if snapshot.id == "kimi"
                && snapshot.warning.is_some()
                && matches!(metric.label.as_str(), "API" | "Credits used")
            {
                continue;
            }
            let Some(used) = metric.used_percent else { continue };
            if !used.is_finite() || used < 0.0 {
                continue;
            }
            let key = format!("{}:{}", snapshot.id, metric.label);
            let entry = map.entry(key).or_default();

            // A reset time that jumped forward means the quota window
            // rolled over. The usage guard blocks sliding-window providers
            // whose resets_at creeps forward every refresh while usage
            // stays put: only a near-empty (or sharply dropped) reading
            // counts as a real reset.
            let advanced = matches!(
                (entry.resets_at, metric.resets_at),
                (Some(old), Some(new)) if new - old > 10 * 60_000
            );
            let rolled_over = entry.seen
                && advanced
                && long_window(metric.period_ms, entry.resets_at, metric.resets_at)
                && (used < 2.0 || entry.prev_used.is_some_and(|p| used + 10.0 < p));

            if period_changed(entry.resets_at, metric.resets_at) {
                *entry = MetricState::default();
            }
            entry.resets_at = metric.resets_at;

            let left = (100.0 - used.clamp(0.0, 100.0)).max(0.0);
            let (v, spare) = verdict(used, metric.resets_at, metric.period_ms);
            let almost_now = left < 10.0;
            let close_now = v == Verdict::Close;
            let run_out_now = v == Verdict::RunOut;
            let baseline = !entry.seen;
            entry.seen = true;

            if want_reset && rolled_over {
                let shown = crate::i18n::metric_label(cfg, &metric.label);
                let name = format!("{} {}", snapshot.name, shown);
                // `advanced` (required for `rolled_over`) only holds when
                // both the old and new reset times are `Some`, so
                // `metric.resets_at` is always `Some` here -- `next` is
                // never absent in this branch.
                let next = metric
                    .resets_at
                    .map(|resets| compact_duration(resets - chrono::Utc::now().timestamp_millis()))
                    .expect("resets_at is Some whenever rolled_over is true");
                let body_key = if used < 2.0 { "alert.reset.backTo100" } else { "alert.reset.available" };
                alerts.push(Alert {
                    title: Msg::new("alert.reset.title"),
                    body: Msg::new(body_key).var("name", &name).var("left", n0(left)).var("next", next),
                });
            }

            if !baseline {
                let shown = crate::i18n::metric_label(cfg, &metric.label);
                let name = format!("{} {}", snapshot.name, shown);
                if want_runout && run_out_now && !entry.run_out {
                    alerts.push(Alert {
                        title: Msg::new("alert.runOut.title"),
                        body: Msg::new("alert.runOut.body").var("name", &name),
                    });
                } else if want_close && close_now && !entry.close {
                    alerts.push(Alert {
                        title: Msg::new("alert.close.title"),
                        body: Msg::new("alert.close.body").var("name", &name).var("spare", n0(spare)),
                    });
                }
                if want_almost && almost_now && !entry.almost_out {
                    alerts.push(Alert {
                        title: Msg::new("alert.almostOut.title"),
                        body: Msg::new("alert.almostOut.body").var("name", &name).var("left", n0(left)),
                    });
                }
            }

            // Burn rate, weekly-and-longer windows only: a 5-hour window
            // climbing 15 points in half an hour is just a working session.
            if want_burn && metric.period_ms.is_some_and(|p| p >= LONG_WINDOW_MS) {
                let rise = burn_rise(&entry.history, now, used);
                let hot = rise.is_some_and(|(points, _)| points >= burn_points);
                if let (true, false, Some((points, minutes))) = (hot, entry.burning, rise) {
                    let shown = crate::i18n::metric_label(cfg, &metric.label);
                    let name = format!("{} {}", snapshot.name, shown);
                    alerts.push(Alert {
                        title: Msg::new("alert.burningFast.title"),
                        body: Msg::new("alert.burningFast.body")
                            .var("name", &name)
                            .var("points", n0(points))
                            .var("minutes", minutes)
                            .var("left", n0(left)),
                    });
                }
                // Re-arm only once the rate has clearly fallen, so a spike
                // hovering at the threshold does not toast on every refresh.
                entry.burning = if entry.burning {
                    rise.is_some_and(|(points, _)| points >= burn_points / 2.0)
                } else {
                    hot
                };
                entry.history.retain(|(at, _)| now - at <= BURN_WINDOW_MS);
                entry.history.push((now, used));
            }

            entry.almost_out = almost_now;
            entry.close = close_now;
            entry.run_out = run_out_now;
            // After the wipe, so a rollover restarts the usage history.
            entry.prev_used = Some(used);
        }
    }
    alerts
}

/// The day the spend alert last fired for. One toast per day: spend only
/// grows through a day, so a crossing happens once.
fn spend_alert_day() -> &'static Mutex<Option<String>> {
    static DAY: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    DAY.get_or_init(|| Mutex::new(None))
}

/// "Daily Spend": today's total crossed `dailySpendAlert` dollars (0 = off).
/// `today` is the local date the total belongs to, e.g. "2026-09-21".
pub fn evaluate_spend(today_cost: f64, today: &str, cfg: &Value) -> Option<Alert> {
    let limit = cfg.get("dailySpendAlert").and_then(Value::as_f64).unwrap_or(0.0);
    if limit <= 0.0 || !today_cost.is_finite() || today_cost < limit {
        return None;
    }
    let mut fired = spend_alert_day().lock().ok()?;
    if fired.as_deref() == Some(today) {
        return None;
    }
    *fired = Some(today.to_string());
    Some(Alert {
        title: Msg::new("alert.dailySpend.title"),
        body: Msg::new("alert.dailySpend.body").var("spent", n0(today_cost)).var("limit", n0(limit)),
    })
}

#[cfg(any(test, feature = "test-support"))]
pub fn reset_spend_alert_for_test() {
    if let Ok(mut fired) = spend_alert_day().lock() {
        *fired = None;
    }
}

/// Drop every metric keyed as `{snapshot_id}:…`. Prefix is `id + ':'` so
/// `onenewapi@abc` does not also wipe `onenewapi@abcd`.
pub fn forget_snapshot(id: &str) {
    let prefix = format!("{id}:");
    let Ok(mut map) = states().lock() else {
        return;
    };
    map.retain(|k, _| !k.starts_with(&prefix));
}

#[cfg(any(test, feature = "test-support"))]
pub fn insert_state_for_test(key: &str) {
    let Ok(mut map) = states().lock() else {
        return;
    };
    map.insert(key.to_string(), MetricState::default());
}

#[cfg(any(test, feature = "test-support"))]
pub fn has_state_for_test(key: &str) -> bool {
    states()
        .lock()
        .map(|map| map.contains_key(key))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test below built its `cfg` with `"locale": "en"` before `Alert`
    /// carried a `Msg`, so rendering in English here reproduces exactly the
    /// string each assertion already pinned.
    fn en(msg: &Msg) -> String {
        crate::i18n::render("en", msg)
    }

    const MIN: i64 = 60_000;
    const WEEK: i64 = 7 * 24 * 60 * MIN;

    fn weekly(id: &str, used: f64, now: i64) -> Snapshot {
        Snapshot::ok(id, "Claude", None, vec![
            crate::providers::Metric::progress("Weekly", used, None)
                .with_reset(Some(now + 3 * 24 * 60 * MIN), Some(WEEK)),
        ])
    }

    #[test]
    fn burn_rise_measures_against_the_oldest_sample_inside_the_window() {
        let now = 10_000 * MIN;
        let history = [(now - 50 * MIN, 5.0), (now - 25 * MIN, 20.0), (now - 5 * MIN, 30.0)];
        // The 50-minute-old reading is outside the 30-minute window.
        assert_eq!(burn_rise(&history, now, 38.0), Some((18.0, 25)));
        // One sample a few seconds old is not a rate yet.
        assert_eq!(burn_rise(&[(now - 10_000, 30.0)], now, 50.0), None);
        assert_eq!(burn_rise(&[], now, 50.0), None);
    }

    #[test]
    fn fast_burn_fires_once_on_a_weekly_window_and_re_arms_after_it_cools() {
        let id = "burn-weekly";
        let cfg = serde_json::json!({"burnAlertPoints": 15, "locale": "en"});
        let t0 = 20_000 * MIN;
        forget_snapshot(id);
        assert!(evaluate_at(&[weekly(id, 20.0, t0)], &cfg, t0).is_empty(), "baseline is silent");
        assert!(evaluate_at(&[weekly(id, 26.0, t0)], &cfg, t0 + 10 * MIN).is_empty(), "6 points is normal");
        let fired = evaluate_at(&[weekly(id, 37.0, t0)], &cfg, t0 + 20 * MIN);
        assert_eq!(fired.len(), 1, "17 points in 20 minutes");
        assert_eq!(en(&fired[0].title), "Burning Fast");
        let body = en(&fired[0].body);
        assert!(body.contains("17%") && body.contains("20 min"), "{body}");
        assert!(evaluate_at(&[weekly(id, 39.0, t0)], &cfg, t0 + 25 * MIN).is_empty(), "fires once");
        // An hour of calm, then a second spike is a new event.
        assert!(evaluate_at(&[weekly(id, 40.0, t0)], &cfg, t0 + 90 * MIN).is_empty());
        assert_eq!(evaluate_at(&[weekly(id, 58.0, t0)], &cfg, t0 + 110 * MIN).len(), 1);
        forget_snapshot(id);
    }

    #[test]
    fn fast_burn_ignores_short_windows_and_the_off_setting() {
        let id = "burn-session";
        let t0 = 30_000 * MIN;
        let session = |used: f64| Snapshot::ok(id, "Claude", None, vec![
            crate::providers::Metric::progress("Session", used, None)
                .with_reset(Some(t0 + 120 * MIN), Some(300 * MIN)),
        ]);
        let on = serde_json::json!({"burnAlertPoints": 15, "locale": "en"});
        forget_snapshot(id);
        assert!(evaluate_at(&[session(10.0)], &on, t0).is_empty());
        // 40 points of a 5-hour window in 20 minutes is just a busy session.
        assert!(evaluate_at(&[session(50.0)], &on, t0 + 20 * MIN).is_empty());
        forget_snapshot(id);

        let off = serde_json::json!({"burnAlertPoints": 0, "locale": "en"});
        let id2 = "burn-off";
        forget_snapshot(id2);
        assert!(evaluate_at(&[weekly(id2, 10.0, t0)], &off, t0).is_empty());
        assert!(evaluate_at(&[weekly(id2, 60.0, t0)], &off, t0 + 20 * MIN).is_empty());
        forget_snapshot(id2);
    }

    #[test]
    fn daily_spend_alert_fires_once_per_day_when_the_threshold_is_crossed() {
        reset_spend_alert_for_test();
        let cfg = serde_json::json!({"dailySpendAlert": 50, "locale": "en"});
        assert!(evaluate_spend(49.99, "2026-09-21", &cfg).is_none());
        let alert = evaluate_spend(62.4, "2026-09-21", &cfg).expect("crossed $50");
        assert_eq!(en(&alert.title), "Daily Spend");
        let body = en(&alert.body);
        assert!(body.contains("$62") && body.contains("$50"), "{body}");
        assert!(evaluate_spend(80.0, "2026-09-21", &cfg).is_none(), "once per day");
        assert!(evaluate_spend(55.0, "2026-09-22", &cfg).is_some(), "a new day re-arms");
        // Raising the threshold past today's spend must not re-fire today.
        reset_spend_alert_for_test();
        let off = serde_json::json!({"dailySpendAlert": 0, "locale": "en"});
        assert!(evaluate_spend(500.0, "2026-09-21", &off).is_none(), "0 is off");
        assert!(evaluate_spend(f64::NAN, "2026-09-21", &cfg).is_none());
    }

    #[test]
    fn sub2api_alerts_ignore_stale_disabled_and_nonfinite_observations() {
        let id = "sub2api@alert-eligibility";
        let cfg = serde_json::json!({"notifyAlmostOut": true, "locale": "en"});
        let make = |used| Snapshot::ok(id, "Site · Key", None,
            vec![crate::providers::Metric::progress("Total quota", used, None)]);
        forget_snapshot(id);
        assert!(evaluate(&[make(50.0)], &cfg).is_empty());
        let mut stale = make(95.0);
        stale.stale = true;
        assert!(evaluate(&[stale], &cfg).is_empty());
        assert!(evaluate(&[make(f64::NAN)], &cfg).is_empty());
        for disabled in ["sub2api", id] {
            let mut off = cfg.clone();
            off["disabled"] = serde_json::json!([disabled]);
            assert!(evaluate(&[make(95.0)], &off).is_empty());
        }
        assert_eq!(evaluate(&[make(95.0)], &cfg).len(), 1);
        forget_snapshot(id);
    }

    #[test]
    fn sub2api_thresholds_are_independent_for_equal_keys_and_each_allowance() {
        let ids = ["sub2api@alert-a", "sub2api@alert-b"];
        let cfg = serde_json::json!({"notifyAlmostOut": true, "locale": "en"});
        let make = |id: &str, used| Snapshot::ok(id, id, None, vec![
            crate::providers::Metric::progress("5h", used, None),
            crate::providers::Metric::progress("1d", used, None),
        ]);
        for id in ids { forget_snapshot(id); }
        assert!(evaluate(&[make(ids[0], 40.0), make(ids[1], 40.0)], &cfg).is_empty());
        let alerts = evaluate(&[make(ids[0], 95.0), make(ids[1], 95.0)], &cfg);
        assert_eq!(alerts.len(), 4);
        assert_eq!(alerts.iter().filter(|alert| en(&alert.body).contains(ids[0])).count(), 2);
        assert!(evaluate(&[make(ids[0], 95.0), make(ids[1], 95.0)], &cfg).is_empty());
        let wallet = Snapshot::ok(ids[0], "Wallet", None,
            vec![crate::providers::Metric::text("Balance", "$-20.00".into())]);
        assert!(evaluate(&[wallet], &cfg).is_empty());
        for id in ids { forget_snapshot(id); }
    }

    #[test]
    fn stale_snapshots_of_any_provider_never_fire_or_re_arm_alerts() {
        let id = "codex";
        let cfg = serde_json::json!({"notifyAlmostOut": true, "locale": "en"});
        let make = |used, stale| {
            let mut s = Snapshot::ok(
                id,
                "Codex",
                None,
                vec![crate::providers::Metric::progress("Weekly", used, None)],
            );
            s.stale = stale;
            s
        };
        forget_snapshot(id);
        assert!(evaluate(&[make(50.0, false)], &cfg).is_empty());
        assert!(evaluate(&[make(95.0, true)], &cfg).is_empty());
        assert_eq!(evaluate(&[make(95.0, false)], &cfg).len(), 1);
        assert!(evaluate(&[make(50.0, true)], &cfg).is_empty());
        assert!(evaluate(&[make(95.0, false)], &cfg).is_empty());
        assert!(evaluate(&[make(50.0, false)], &cfg).is_empty());
        assert_eq!(evaluate(&[make(95.0, false)], &cfg).len(), 1);
        forget_snapshot(id);
    }

    #[test]
    fn reset_fires_once_when_period_advances() {
        let id = "codex@reset-advance";
        let cfg = serde_json::json!({"notifyReset": true, "locale": "en"});
        let period = 7 * 86_400_000_i64;
        let t = chrono::Utc::now().timestamp_millis() + 2 * 3_600_000;
        let make = |used, resets| Snapshot::ok(id, "Codex", None, vec![
            crate::providers::Metric::progress("Weekly", used, None)
                .with_reset(Some(resets), Some(period)),
        ]);
        forget_snapshot(id);
        assert!(evaluate(&[make(80.0, t)], &cfg).is_empty());
        assert!(evaluate(&[make(80.0, t)], &cfg).is_empty());
        let alerts = evaluate(&[make(0.0, t + period)], &cfg);
        assert_eq!(alerts.len(), 1);
        assert_eq!(en(&alerts[0].title), "Limit reset");
        let body = en(&alerts[0].body);
        assert!(body.contains("Codex Weekly is back to 100%"));
        assert!(body.contains("Next reset in"));
        assert!(evaluate(&[make(0.0, t + period)], &cfg).is_empty());
        forget_snapshot(id);
    }

    #[test]
    fn reset_after_usage_resumed_reports_remaining() {
        let id = "codex@reset-resumed";
        let cfg = serde_json::json!({"notifyReset": true, "locale": "en"});
        let period = 7 * 86_400_000_i64;
        let t = chrono::Utc::now().timestamp_millis() + 2 * 3_600_000;
        let make = |used, resets| Snapshot::ok(id, "Codex", None, vec![
            crate::providers::Metric::progress("Weekly", used, None)
                .with_reset(Some(resets), Some(period)),
        ]);
        forget_snapshot(id);
        assert!(evaluate(&[make(80.0, t)], &cfg).is_empty());
        // Usage already resumed (20% burned in the new window): report
        // what's left, not a full quota.
        let alerts = evaluate(&[make(20.0, t + period)], &cfg);
        assert_eq!(alerts.len(), 1);
        let body = en(&alerts[0].body);
        assert!(body.contains("has reset — 80% available"));
        assert!(!body.contains("100%"));
        forget_snapshot(id);
    }

    #[test]
    fn reset_fires_for_untouched_window() {
        let id = "codex@reset-untouched";
        let cfg = serde_json::json!({"notifyReset": true, "locale": "en"});
        let period = 7 * 86_400_000_i64;
        let t = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let make = |used, resets| Snapshot::ok(id, "Codex", None, vec![
            crate::providers::Metric::progress("Weekly", used, None)
                .with_reset(Some(resets), Some(period)),
        ]);
        forget_snapshot(id);
        assert!(evaluate(&[make(0.0, t)], &cfg).is_empty());
        assert_eq!(evaluate(&[make(0.0, t + period)], &cfg).len(), 1);
        forget_snapshot(id);
    }

    #[test]
    fn sliding_window_drift_does_not_fire() {
        let id = "codex@reset-drift";
        let cfg = serde_json::json!({"notifyReset": true, "locale": "en"});
        let t = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let make = |used, resets| Snapshot::ok(id, "Codex", None, vec![
            crate::providers::Metric::progress("Weekly", used, None)
                .with_reset(Some(resets), Some(7 * 86_400_000)),
        ]);
        forget_snapshot(id);
        assert!(evaluate(&[make(60.0, t)], &cfg).is_empty());
        // resets_at crept forward while usage climbed: a sliding window,
        // not a rollover.
        assert!(evaluate(&[make(62.0, t + 15 * 60_000)], &cfg).is_empty());
        forget_snapshot(id);
    }

    #[test]
    fn short_window_reset_is_silent() {
        let id = "codex@reset-short";
        let cfg = serde_json::json!({"notifyReset": true, "locale": "en"});
        let period = 5 * 3_600_000_i64;
        let t = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let make = |used, resets| Snapshot::ok(id, "Codex", None, vec![
            crate::providers::Metric::progress("5 Hours", used, None)
                .with_reset(Some(resets), Some(period)),
        ]);
        forget_snapshot(id);
        assert!(evaluate(&[make(80.0, t)], &cfg).is_empty());
        // A real rollover — but 5-hour windows don't toast.
        assert!(evaluate(&[make(0.0, t + period)], &cfg).is_empty());
        forget_snapshot(id);
    }

    #[test]
    fn long_window_inferred_from_reset_jump_when_period_missing() {
        let id = "codex@reset-noperiod";
        let cfg = serde_json::json!({"notifyReset": true, "locale": "en"});
        let t = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let make = |used, resets| Snapshot::ok(id, "Codex", None, vec![
            crate::providers::Metric::progress("Weekly", used, None)
                .with_reset(Some(resets), None),
        ]);
        forget_snapshot(id);
        assert!(evaluate(&[make(80.0, t)], &cfg).is_empty());
        // No declared period: a 7-day jump between resets IS the window.
        assert_eq!(evaluate(&[make(0.0, t + 7 * 86_400_000)], &cfg).len(), 1);
        forget_snapshot(id);
    }

    #[test]
    fn reset_needs_baseline() {
        let id = "codex@reset-baseline";
        let cfg = serde_json::json!({"notifyReset": true, "locale": "en"});
        let t = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let make = |used, resets| Snapshot::ok(id, "Codex", None, vec![
            crate::providers::Metric::progress("Weekly", used, None)
                .with_reset(Some(resets), Some(7 * 86_400_000)),
        ]);
        forget_snapshot(id);
        // The first reading after launch is a silent baseline.
        assert!(evaluate(&[make(0.0, t)], &cfg).is_empty());
        forget_snapshot(id);
    }

    #[test]
    fn compact_duration_formats() {
        assert_eq!(compact_duration(6 * 86_400_000 + 23 * 3_600_000), "6d 23h");
        assert_eq!(compact_duration(4 * 3_600_000 + 58 * 60_000), "4h 58m");
        assert_eq!(compact_duration(12 * 60_000), "12m");
        assert_eq!(compact_duration(-5_000), "0m");
    }

    #[test]
    fn forget_snapshot_drops_that_id_only() {
        insert_state_for_test("onenewapi@ticket07-abc:Usage");
        insert_state_for_test("onenewapi@ticket07-abc:Expiry");
        insert_state_for_test("onenewapi@ticket07-abcd:Usage");
        forget_snapshot("onenewapi@ticket07-abc");
        assert!(!has_state_for_test("onenewapi@ticket07-abc:Usage"));
        assert!(!has_state_for_test("onenewapi@ticket07-abc:Expiry"));
        assert!(has_state_for_test("onenewapi@ticket07-abcd:Usage"));
        forget_snapshot("onenewapi@ticket07-abcd");
    }

    /// First written against the inline `match
    /// crate::i18n::resolved_locale(cfg) { "zh" => …, "ru" => …, _ => … }`
    /// arms this file used to have, before `Alert` carried a `Msg` at all.
    /// Drives every alert case for all three shipped locales and pins the
    /// exact text as literal assertions, table-driven (fixture -> expected
    /// title/body per locale). Now that the arms live in the locale JSON
    /// files, it renders through `i18n::render(locale, &msg)` -- same
    /// literal expectations as when it was written, so a reword during
    /// that migration would have failed here first.
    ///
    /// The "Limit reset" case appends a "next reset in …" clause built
    /// from the real clock (see `evaluate_at`), so those two fixtures
    /// check the fixed lead-in and the fixed tail with `contains` on each
    /// side of the clock-derived duration instead of one `assert_eq!` on
    /// the whole string; every other case has no clock-derived content
    /// and is checked exactly.
    ///
    /// The six cases below are hand-enumerated rather than driven from
    /// `ALERT_KEYS`, so a seventh alert added later is not automatically
    /// forced into this wording pin -- `every_alert_and_notification_key_exists`
    /// in `i18n.rs` is what catches a new alert missing its en/zh/ru values.
    #[test]
    fn zh_and_ru_alert_text_is_unchanged() {
        let locales = ["en", "zh", "ru"];

        // -- Limit reset: back to (near) 100% --------------------------
        let title = ["Limit reset", "额度已重置", "Лимит сброшен"];
        let lead = [
            "Codex Weekly is back to 100%.",
            "Codex 每周 已恢复到 100%。",
            "Codex За неделю снова 100%.",
        ];
        let next_lead = ["Next reset in", "下次重置：", "Следующий сброс через"];
        for (i, locale) in locales.iter().enumerate() {
            let locale = *locale;
            let id = format!("snap-reset-back-{locale}");
            let cfg = serde_json::json!({"notifyReset": true, "locale": locale});
            let period = 7 * 86_400_000_i64;
            let t = chrono::Utc::now().timestamp_millis() + 2 * 3_600_000;
            let make = |used, resets| {
                Snapshot::ok(&id, "Codex", None, vec![
                    crate::providers::Metric::progress("Weekly", used, None).with_reset(Some(resets), Some(period)),
                ])
            };
            forget_snapshot(&id);
            assert!(evaluate_at(&[make(80.0, t)], &cfg, t).is_empty(), "{locale}: baseline must be silent");
            let alerts = evaluate_at(&[make(0.0, t + period)], &cfg, t + period);
            assert_eq!(alerts.len(), 1, "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].title), title[i], "{locale} reset title");
            let body = crate::i18n::render(locale, &alerts[0].body);
            assert!(body.starts_with(lead[i]), "{locale}: {body}");
            assert!(body.contains(next_lead[i]), "{locale}: {body}");
            forget_snapshot(&id);
        }

        // -- Limit reset: usage already resumed, 80% available ---------
        let avail_lead = [
            "Codex Weekly has reset — 80% available.",
            "Codex 每周 已重置 — 可用 80%。",
            "Codex За неделю сброшен — доступно 80%.",
        ];
        for (i, locale) in locales.iter().enumerate() {
            let locale = *locale;
            let id = format!("snap-reset-avail-{locale}");
            let cfg = serde_json::json!({"notifyReset": true, "locale": locale});
            let period = 7 * 86_400_000_i64;
            let t = chrono::Utc::now().timestamp_millis() + 2 * 3_600_000;
            let make = |used, resets| {
                Snapshot::ok(&id, "Codex", None, vec![
                    crate::providers::Metric::progress("Weekly", used, None).with_reset(Some(resets), Some(period)),
                ])
            };
            forget_snapshot(&id);
            assert!(evaluate_at(&[make(80.0, t)], &cfg, t).is_empty(), "{locale}: baseline must be silent");
            let alerts = evaluate_at(&[make(20.0, t + period)], &cfg, t + period);
            assert_eq!(alerts.len(), 1, "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].title), title[i], "{locale} reset title");
            let body = crate::i18n::render(locale, &alerts[0].body);
            assert!(body.starts_with(avail_lead[i]), "{locale}: {body}");
            assert!(body.contains(next_lead[i]), "{locale}: {body}");
            forget_snapshot(&id);
        }

        // -- Will Run Out ------------------------------------------------
        let run_out_title = ["Will Run Out", "将会用完", "Кончится до сброса"];
        let run_out_body = [
            "Codex Weekly is on pace to hit its limit before the reset.",
            "Codex 每周 按当前速度会在重置前用完。",
            "Codex За неделю при текущем темпе исчерпается до сброса.",
        ];
        for (i, locale) in locales.iter().enumerate() {
            let locale = *locale;
            let id = format!("snap-runout-{locale}");
            let cfg = serde_json::json!({"notifyWillRunOut": true, "locale": locale});
            let t = chrono::Utc::now().timestamp_millis() + 2 * 3_600_000;
            let period = 7 * 86_400_000_i64;
            let make = |used| {
                Snapshot::ok(&id, "Codex", None, vec![
                    crate::providers::Metric::progress("Weekly", used, None).with_reset(Some(t), Some(period)),
                ])
            };
            forget_snapshot(&id);
            assert!(evaluate(&[make(10.0)], &cfg).is_empty(), "{locale}: baseline must be silent");
            let alerts = evaluate(&[make(99.0)], &cfg);
            assert_eq!(alerts.len(), 1, "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].title), run_out_title[i], "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].body), run_out_body[i], "{locale}");
            forget_snapshot(&id);
        }

        // -- Cutting It Close ---------------------------------------------
        let close_title = ["Cutting It Close", "余量紧张", "Запас на исходе"];
        let close_body = [
            "Codex Weekly is on pace to finish with only ~9% spare.",
            "Codex 每周 按当前速度重置时大约只剩 9%。",
            "Codex За неделю к сбросу останется примерно 9%.",
        ];
        for (i, locale) in locales.iter().enumerate() {
            let locale = *locale;
            let id = format!("snap-close-{locale}");
            let cfg = serde_json::json!({"notifyCuttingClose": true, "locale": locale});
            let t = chrono::Utc::now().timestamp_millis() + 2 * 3_600_000;
            let period = 7 * 86_400_000_i64;
            let make = |used| {
                Snapshot::ok(&id, "Codex", None, vec![
                    crate::providers::Metric::progress("Weekly", used, None).with_reset(Some(t), Some(period)),
                ])
            };
            forget_snapshot(&id);
            assert!(evaluate(&[make(10.0)], &cfg).is_empty(), "{locale}: baseline must be silent");
            let alerts = evaluate(&[make(90.0)], &cfg);
            assert_eq!(alerts.len(), 1, "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].title), close_title[i], "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].body), close_body[i], "{locale}");
            forget_snapshot(&id);
        }

        // -- Almost Out ----------------------------------------------------
        let almost_title = ["Almost Out", "即将用完", "Почти кончилось"];
        let almost_body = [
            "Codex Weekly is under 10% remaining (5% left).",
            "Codex 每周 剩余不足 10%（还剩 5%）。",
            "Codex За неделю осталось меньше 10% (ещё 5%).",
        ];
        for (i, locale) in locales.iter().enumerate() {
            let locale = *locale;
            let id = format!("snap-almost-{locale}");
            let cfg = serde_json::json!({"notifyAlmostOut": true, "locale": locale});
            let make = |used| Snapshot::ok(&id, "Codex", None, vec![crate::providers::Metric::progress("Weekly", used, None)]);
            forget_snapshot(&id);
            assert!(evaluate(&[make(50.0)], &cfg).is_empty(), "{locale}: baseline must be silent");
            let alerts = evaluate(&[make(95.0)], &cfg);
            assert_eq!(alerts.len(), 1, "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].title), almost_title[i], "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].body), almost_body[i], "{locale}");
            forget_snapshot(&id);
        }

        // -- Burning Fast ----------------------------------------------------
        let burn_title = ["Burning Fast", "消耗过快", "Быстрый расход"];
        let burn_body = [
            "Codex Weekly used 17% in 20 min. 63% left.",
            "Codex 每周 在 20 分钟内用掉了 17%，剩余 63%。",
            "Codex За неделю: 17% за 20 мин, осталось 63%.",
        ];
        for (i, locale) in locales.iter().enumerate() {
            let locale = *locale;
            let id = format!("snap-burn-{locale}");
            let cfg = serde_json::json!({"burnAlertPoints": 15, "locale": locale});
            let t0 = 20_000 * 60_000_i64;
            let weekly = |used: f64| {
                Snapshot::ok(&id, "Codex", None, vec![
                    crate::providers::Metric::progress("Weekly", used, None)
                        .with_reset(Some(t0 + 3 * 24 * 60 * 60_000), Some(7 * 24 * 3_600_000)),
                ])
            };
            forget_snapshot(&id);
            assert!(evaluate_at(&[weekly(20.0)], &cfg, t0).is_empty(), "{locale}: baseline must be silent");
            assert!(evaluate_at(&[weekly(26.0)], &cfg, t0 + 10 * 60_000).is_empty(), "{locale}: 6 points is normal");
            let alerts = evaluate_at(&[weekly(37.0)], &cfg, t0 + 20 * 60_000);
            assert_eq!(alerts.len(), 1, "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].title), burn_title[i], "{locale}");
            assert_eq!(crate::i18n::render(locale, &alerts[0].body), burn_body[i], "{locale}");
            forget_snapshot(&id);
        }

        // -- Daily Spend -------------------------------------------------
        let spend_title = ["Daily Spend", "今日花费", "Расход за день"];
        let spend_body = [
            "$62 spent today, past your $50 mark.",
            "今天已花费 $62，超过了你设定的 $50。",
            "Сегодня потрачено $62, это больше вашего порога $50.",
        ];
        for (i, locale) in locales.iter().enumerate() {
            let locale = *locale;
            reset_spend_alert_for_test();
            let cfg = serde_json::json!({"dailySpendAlert": 50, "locale": locale});
            let alert = evaluate_spend(62.4, "2026-09-21", &cfg).expect("crossed $50");
            assert_eq!(crate::i18n::render(locale, &alert.title), spend_title[i], "{locale}");
            assert_eq!(crate::i18n::render(locale, &alert.body), spend_body[i], "{locale}");
        }
        reset_spend_alert_for_test();
    }
}
