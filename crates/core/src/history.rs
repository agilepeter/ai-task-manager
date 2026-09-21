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
    for series in &mut out {
        thin(&mut series.points);
    }
    Ok(out)
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
    fn long_ranges_are_thinned_and_keep_the_newest_reading() {
        let mut points: Vec<Point> =
            (0..4_000).map(|i| Point { at: i, used: i as f64 }).collect();
        thin(&mut points);
        assert!(points.len() <= MAX_POINTS + 1, "{}", points.len());
        assert_eq!(points.first().unwrap().at, 0);
        assert_eq!(points.last().unwrap().at, 3_999);
    }
}
