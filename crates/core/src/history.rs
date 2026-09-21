//! Local history of limit readings, so the detail view can chart a quota
//! over time instead of showing only the live number.
//!
//! One SQLite file in the app's config dir. Nothing here leaves the machine.
//! Only what the cards already show is stored: provider id, metric label, the
//! used percentage and the reset time. No tokens, prompts, keys or paths.

use crate::providers::Snapshot;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::sync::{Mutex, OnceLock};

/// Readings older than this are dropped.
const RETENTION_MS: i64 = 90 * 24 * 3_600_000;
/// A reading is skipped when the previous one for the same metric is newer
/// than this *and* the value has barely moved: a one-minute refresh would
/// otherwise store 1,440 near-identical rows per metric per day.
const MIN_GAP_MS: i64 = 4 * 60_000;
const MIN_DELTA: f64 = 0.5;
/// A flat line still needs a point now and then to be drawn as flat.
const MAX_GAP_MS: i64 = 60 * 60_000;
/// Upper bound on points handed to the UI for one metric.
const MAX_POINTS: usize = 1_500;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Point {
    /// Epoch milliseconds.
    pub at: i64,
    pub used: f64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Series {
    pub metric: String,
    pub points: Vec<Point>,
}

pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS samples (
             provider  TEXT    NOT NULL,
             metric    TEXT    NOT NULL,
             at        INTEGER NOT NULL,
             used      REAL    NOT NULL,
             resets_at INTEGER
         );
         CREATE INDEX IF NOT EXISTS samples_lookup ON samples (provider, metric, at);",
    )
}

/// Stores the live progress readings in `snapshots`. Returns how many rows
/// were written. Stale or failed snapshots are last-known values, not
/// observations, and are skipped.
pub fn record_at(conn: &Connection, snapshots: &[Snapshot], now: i64) -> rusqlite::Result<usize> {
    let mut written = 0;
    for snapshot in snapshots.iter().filter(|s| s.status == "ok" && !s.stale) {
        for metric in snapshot.metrics.iter().filter(|m| m.kind == "progress") {
            let Some(used) = metric.used_percent.filter(|u| u.is_finite()) else {
                continue;
            };
            let used = used.clamp(0.0, 100.0);
            let last: Option<(i64, f64)> = conn
                .query_row(
                    "SELECT at, used FROM samples WHERE provider = ?1 AND metric = ?2
                     ORDER BY at DESC LIMIT 1",
                    params![snapshot.id, metric.label],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();
            if let Some((at, prev)) = last {
                let gap = now - at;
                let moved = (used - prev).abs() >= MIN_DELTA;
                if gap < 0 || (gap < MAX_GAP_MS && (gap < MIN_GAP_MS || !moved)) {
                    continue;
                }
            }
            conn.execute(
                "INSERT INTO samples (provider, metric, at, used, resets_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![snapshot.id, metric.label, now, used, metric.resets_at],
            )?;
            written += 1;
        }
    }
    Ok(written)
}

pub fn prune_at(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM samples WHERE at < ?1", params![now - RETENTION_MS])
}

/// Every metric of `provider` with readings at or after `since`, oldest
/// first. Long ranges are thinned evenly to `MAX_POINTS`, always keeping the
/// newest reading.
pub fn series_at(conn: &Connection, provider: &str, since: i64) -> rusqlite::Result<Vec<Series>> {
    let mut out = raw_series(conn, provider, since)?;
    for series in &mut out {
        thin(&mut series.points);
    }
    Ok(out)
}

/// Every reading, unthinned: the burn profile works on consecutive pairs.
fn raw_series(conn: &Connection, provider: &str, since: i64) -> rusqlite::Result<Vec<Series>> {
    let mut stmt = conn.prepare(
        "SELECT metric, at, used FROM samples WHERE provider = ?1 AND at >= ?2 ORDER BY metric, at",
    )?;
    let rows = stmt.query_map(params![provider, since], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, f64>(2)?))
    })?;
    let mut out: Vec<Series> = Vec::new();
    for row in rows {
        let (metric, at, used) = row?;
        match out.last_mut() {
            Some(series) if series.metric == metric => series.points.push(Point { at, used }),
            _ => out.push(Series { metric, points: vec![Point { at, used }] }),
        }
    }
    Ok(out)
}

/// When in the week a limit gets used: points of the limit burned in each
/// (weekday, hour) cell, Monday first, in the user's local time.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BurnProfile {
    pub metric: String,
    /// `cells[weekday][hour]`, weekday 0 = Monday.
    pub cells: Vec<Vec<f64>>,
    /// Distinct local days with at least one usable reading pair.
    pub days_observed: usize,
}

/// A rise between two readings is booked to the hour of the later one, but
/// only when they are this close: across a longer gap (the app was off, the
/// machine asleep) nobody knows which hour the usage happened in.
const MAX_BURN_GAP_MS: i64 = 90 * 60_000;

/// `offset_ms` is local time minus UTC, passed in so the hour bucketing is
/// testable anywhere.
pub fn burn_profile_from(metric: &str, points: &[Point], offset_ms: i64) -> BurnProfile {
    let mut cells = vec![vec![0.0; 24]; 7];
    let mut days = std::collections::HashSet::new();
    for pair in points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let rise = b.used - a.used;
        let gap = b.at - a.at;
        // Only rises count: a fall is the window rolling over, not negative use.
        if gap <= 0 || gap > MAX_BURN_GAP_MS || rise <= 0.0 {
            continue;
        }
        let local = b.at + offset_ms;
        let day_index = local.div_euclid(86_400_000);
        // 1970-01-01 was a Thursday: shift so that Monday is 0.
        let weekday = (day_index + 3).rem_euclid(7) as usize;
        let hour = (local.rem_euclid(86_400_000) / 3_600_000) as usize;
        cells[weekday][hour] += rise;
        days.insert(day_index);
    }
    BurnProfile { metric: metric.to_string(), cells, days_observed: days.len() }
}

pub fn burn_profiles(provider: &str, since: i64, offset_ms: i64) -> Vec<BurnProfile> {
    store()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|conn| raw_series(conn, provider, since).ok()))
        .unwrap_or_default()
        .iter()
        .map(|s| burn_profile_from(&s.metric, &s.points, offset_ms))
        .collect()
}

fn thin(points: &mut Vec<Point>) {
    if points.len() <= MAX_POINTS {
        return;
    }
    let step = points.len().div_ceil(MAX_POINTS);
    let last = points.len() - 1;
    let mut index = 0;
    points.retain(|_| {
        let keep = index % step == 0 || index == last;
        index += 1;
        keep
    });
}

// ---------------------------------------------------------------------------
// Process-wide store
// ---------------------------------------------------------------------------

fn store() -> &'static Mutex<Option<Connection>> {
    static STORE: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();
    STORE.get_or_init(|| {
        let path = crate::providers::config_dir().join("history.sqlite");
        let conn = std::fs::create_dir_all(crate::providers::config_dir())
            .ok()
            .and_then(|_| Connection::open(path).ok())
            .filter(|conn| init(conn).is_ok());
        if let Some(conn) = &conn {
            let _ = prune_at(conn, chrono::Utc::now().timestamp_millis());
        }
        Mutex::new(conn)
    })
}

/// Records a refresh. History is a convenience: a store that cannot be
/// opened or written never fails a refresh.
pub fn record(snapshots: &[Snapshot]) {
    if let Ok(guard) = store().lock() {
        if let Some(conn) = guard.as_ref() {
            let _ = record_at(conn, snapshots, chrono::Utc::now().timestamp_millis());
        }
    }
}

pub fn series(provider: &str, since: i64) -> Vec<Series> {
    store()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|conn| series_at(conn, provider, since).ok()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::Metric;

    const MIN: i64 = 60_000;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        conn
    }

    fn snap(id: &str, weekly: f64) -> Snapshot {
        Snapshot::ok(id, "Claude", None, vec![
            Metric::progress("Weekly", weekly, None).with_reset(Some(9_000_000), None),
            Metric::text("Plan", "Max".into()),
        ])
    }

    fn values(conn: &Connection, id: &str) -> Vec<f64> {
        series_at(conn, id, 0)
            .unwrap()
            .into_iter()
            .flat_map(|s| s.points.into_iter().map(|p| p.used))
            .collect()
    }

    #[test]
    fn records_progress_metrics_only_and_reads_them_back_in_order() {
        let conn = db();
        assert_eq!(record_at(&conn, &[snap("claude", 10.0)], 0).unwrap(), 1, "text rows are not history");
        record_at(&conn, &[snap("claude", 25.0)], 10 * MIN).unwrap();
        let got = series_at(&conn, "claude", 0).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].metric, "Weekly");
        assert_eq!(got[0].points, [Point { at: 0, used: 10.0 }, Point { at: 10 * MIN, used: 25.0 }]);
        assert!(series_at(&conn, "codex", 0).unwrap().is_empty());
        assert_eq!(series_at(&conn, "claude", 5 * MIN).unwrap()[0].points.len(), 1, "since filters");
    }

    #[test]
    fn near_identical_refreshes_are_not_stored_but_a_flat_line_still_gets_points() {
        let conn = db();
        record_at(&conn, &[snap("claude", 10.0)], 0).unwrap();
        assert_eq!(record_at(&conn, &[snap("claude", 10.2)], MIN).unwrap(), 0, "too soon, no movement");
        assert_eq!(record_at(&conn, &[snap("claude", 30.0)], 2 * MIN).unwrap(), 0, "too soon even when it moved");
        assert_eq!(record_at(&conn, &[snap("claude", 10.2)], 10 * MIN).unwrap(), 0, "old enough but flat");
        assert_eq!(record_at(&conn, &[snap("claude", 12.0)], 11 * MIN).unwrap(), 1, "old enough and moved");
        assert_eq!(record_at(&conn, &[snap("claude", 12.0)], 80 * MIN).unwrap(), 1, "hourly point on a flat line");
        assert_eq!(values(&conn, "claude"), [10.0, 12.0, 12.0]);
    }

    #[test]
    fn stale_failed_and_nonfinite_readings_are_not_observations() {
        let conn = db();
        let mut stale = snap("claude", 50.0);
        stale.stale = true;
        let failed = Snapshot::error("claude", "Claude", "HTTP 500".into());
        let nan = snap("claude", f64::NAN);
        assert_eq!(record_at(&conn, &[stale, failed, nan], 0).unwrap(), 0);
        // Out-of-range values are clamped rather than stored raw.
        record_at(&conn, &[snap("claude", 140.0)], 0).unwrap();
        assert_eq!(values(&conn, "claude"), [100.0]);
    }

    #[test]
    fn accounts_are_kept_apart_and_old_rows_are_pruned() {
        let conn = db();
        record_at(&conn, &[snap("claude", 10.0), snap("claude@ab12cd34", 70.0)], 0).unwrap();
        assert_eq!(values(&conn, "claude"), [10.0]);
        assert_eq!(values(&conn, "claude@ab12cd34"), [70.0]);
        record_at(&conn, &[snap("claude", 20.0)], RETENTION_MS + 10 * MIN).unwrap();
        assert_eq!(prune_at(&conn, RETENTION_MS + 10 * MIN).unwrap(), 2);
        assert_eq!(values(&conn, "claude"), [20.0]);
    }

    #[test]
    fn burn_is_booked_to_the_local_hour_it_was_seen_in() {
        const H: i64 = 3_600_000;
        // 2026-09-21 is a Monday. 14:00 UTC.
        let mon_14 = 1_790_000_000_000 - (1_790_000_000_000 % 86_400_000) + 14 * H;
        let monday = |h: i64, m: i64| mon_14 - 14 * H + h * H + m * MIN;
        let weekday_of_base = ((mon_14.div_euclid(86_400_000)) + 3).rem_euclid(7);
        let points = [
            Point { at: monday(14, 0), used: 10.0 },
            Point { at: monday(14, 30), used: 16.0 }, // +6 in the 14:00 hour
            Point { at: monday(15, 10), used: 20.0 }, // +4 in the 15:00 hour
            Point { at: monday(20, 0), used: 50.0 },  // a 5-hour gap: timing unknown, skipped
            Point { at: monday(20, 30), used: 3.0 },  // a reset, not negative use
            Point { at: monday(21, 0), used: 9.0 },   // +6 in the 21:00 hour
        ];
        let utc = burn_profile_from("Weekly", &points, 0);
        let row = &utc.cells[weekday_of_base as usize];
        assert_eq!((row[14], row[15], row[20], row[21]), (6.0, 4.0, 0.0, 6.0));
        assert_eq!(utc.cells.iter().flatten().sum::<f64>(), 16.0, "nothing booked anywhere else");
        assert_eq!(utc.days_observed, 1);
        // Four hours west: 14:30 UTC is 10:30 local, same weekday.
        let west = burn_profile_from("Weekly", &points, -4 * H);
        assert_eq!(west.cells[weekday_of_base as usize][10], 6.0);
        // Eleven hours east pushes 21:00 into the next day's 08:00.
        let east = burn_profile_from("Weekly", &points, 11 * H);
        assert_eq!(east.cells[((weekday_of_base + 1) % 7) as usize][8], 6.0);
        assert_eq!(burn_profile_from("x", &[], 0).days_observed, 0);
    }

    #[test]
    fn long_ranges_are_thinned_and_keep_the_newest_reading() {
        let mut points: Vec<Point> =
            (0..4_000).map(|i| Point { at: i, used: i as f64 }).collect();
        thin(&mut points);
        assert!(points.len() <= MAX_POINTS + 1, "{}", points.len());
        assert_eq!(points.first().unwrap().at, 0);
        assert_eq!(points.last().unwrap().at, 3_999);
    }
}
