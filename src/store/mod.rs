mod export;
mod query;
mod sql_summary;

pub use query::{ContextTransitionQuery, FlagTransitionQuery};

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;

use crate::error::Error;
use crate::error::Result;
use crate::model::{
    CaptureMarker, ContextSeries, ContextSeriesPoint, ContextSnapshot, ContextSourceStatus,
    ContextTransition, ContextValue, ContextValueKind, ContextValueRow, FlagSeries,
    FlagSeriesPoint, FlagTransition, JournalEvent, MetricSample, MetricSeries, MetricSeriesPoint,
    SystemInfo, TimeBounds, WindowSummary,
};

/// Maximum export window (7 days).
pub const MAX_EXPORT_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Maximum raw samples per export request.
pub const MAX_EXPORT_SAMPLES: i64 = 500_000;
/// Maximum throttle/context transitions loaded into an export bundle.
pub const EXPORT_TRANSITION_LIMIT: i64 = 100_000;
/// SQLite bind-parameter budget for `IN (...)` journal lookups (stay well under 999).
const JOURNAL_IN_BATCH: usize = 500;
use crate::throttle::flag_active;

fn context_str_to_num(s: &str) -> f64 {
    match s {
        "true" | "1" | "charging" | "performance" | "wake" | "awake" => 1.0,
        "false" | "0" | "discharging" | "power-saver" | "sleep" => 0.0,
        "balanced" => 0.5,
        "full" | "not-charging" => 0.75,
        _ => 0.0,
    }
}

fn downsample_by_stride<T>(points: Vec<T>, max_points: usize) -> Vec<T> {
    if max_points == 0 || points.len() <= max_points {
        return points;
    }
    let stride = points.len().div_ceil(max_points);
    points
        .into_iter()
        .enumerate()
        .filter_map(|(i, p)| (i % stride == 0).then_some(p))
        .collect()
}

fn context_key_kind(key: &str) -> ContextValueKind {
    if key.ends_with(".percent")
        || key.ends_with(".watts")
        || key.ends_with("_watts")
        || key.ends_with("_mw")
        || key.ends_with("_ms")
        || key.ends_with("_c")
        || key.ends_with("_mhz")
        || key.ends_with(".online_cpus")
        || key.ends_with(".ac_connected")
        || key.ends_with(".external_connected")
        || key.ends_with(".external_count")
        || key.ends_with(".input_watts")
        || key.ends_with(".adapter_watts_reported")
        || key.ends_with(".dgpu_runtime_suspended")
        || key.ends_with(".dgpu_d3cold_allowed")
    {
        ContextValueKind::Number
    } else {
        ContextValueKind::Enum
    }
}

pub struct Store {
    pub(crate) pool: SqlitePool,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;

        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_unix_ms INTEGER NOT NULL,
                device_pci TEXT NOT NULL,
                device_name TEXT NOT NULL,
                throttle_status_raw INTEGER,
                indep_throttle_status INTEGER NOT NULL,
                active_flags TEXT NOT NULL,
                apu_power_mw INTEGER,
                stapm_limit_mw INTEGER,
                current_stapm_limit_mw INTEGER,
                temperature_core_max REAL,
                extra_json TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_samples_ts ON samples(ts_unix_ms);
            CREATE INDEX IF NOT EXISTS idx_samples_pci_ts ON samples(device_pci, ts_unix_ms);

            CREATE TABLE IF NOT EXISTS transitions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_unix_ms INTEGER NOT NULL,
                sample_id INTEGER NOT NULL,
                device_pci TEXT NOT NULL,
                flag_name TEXT NOT NULL,
                old_value INTEGER NOT NULL,
                new_value INTEGER NOT NULL,
                FOREIGN KEY (sample_id) REFERENCES samples(id)
            );
            CREATE INDEX IF NOT EXISTS idx_transitions_ts ON transitions(ts_unix_ms);

            CREATE TABLE IF NOT EXISTS journal_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                transition_id INTEGER NOT NULL,
                ts_unix_ms INTEGER NOT NULL,
                unit TEXT,
                priority INTEGER,
                message TEXT NOT NULL,
                cursor TEXT,
                FOREIGN KEY (transition_id) REFERENCES transitions(id)
            );

            CREATE TABLE IF NOT EXISTS context_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_unix_ms INTEGER NOT NULL,
                source_id TEXT NOT NULL,
                health TEXT NOT NULL,
                raw_json TEXT NOT NULL,
                error_message TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_context_snapshots_ts ON context_snapshots(ts_unix_ms);
            CREATE INDEX IF NOT EXISTS idx_context_snapshots_source_ts ON context_snapshots(source_id, ts_unix_ms);

            CREATE TABLE IF NOT EXISTS context_values (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                snapshot_id INTEGER NOT NULL,
                ts_unix_ms INTEGER NOT NULL,
                source_id TEXT NOT NULL,
                key TEXT NOT NULL,
                value_num REAL,
                value_str TEXT,
                FOREIGN KEY (snapshot_id) REFERENCES context_snapshots(id)
            );
            CREATE INDEX IF NOT EXISTS idx_context_values_key_ts ON context_values(key, ts_unix_ms);

            CREATE TABLE IF NOT EXISTS context_transitions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_unix_ms INTEGER NOT NULL,
                snapshot_id INTEGER,
                source_id TEXT NOT NULL,
                key TEXT NOT NULL,
                old_value TEXT NOT NULL,
                new_value TEXT NOT NULL,
                FOREIGN KEY (snapshot_id) REFERENCES context_snapshots(id)
            );
            CREATE INDEX IF NOT EXISTS idx_context_transitions_ts ON context_transitions(ts_unix_ms);

            CREATE TABLE IF NOT EXISTS context_journal_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                context_transition_id INTEGER NOT NULL,
                ts_unix_ms INTEGER NOT NULL,
                unit TEXT,
                priority INTEGER,
                message TEXT NOT NULL,
                FOREIGN KEY (context_transition_id) REFERENCES context_transitions(id)
            );

            CREATE TABLE IF NOT EXISTS system_info_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                collected_at_ms INTEGER NOT NULL,
                schema_version INTEGER NOT NULL,
                health TEXT NOT NULL,
                info_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_system_info_collected_at ON system_info_snapshots(collected_at_ms);

            CREATE TABLE IF NOT EXISTS capture_markers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts_unix_ms INTEGER NOT NULL,
                label TEXT NOT NULL,
                detail TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_capture_markers_ts ON capture_markers(ts_unix_ms);
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn insert_capture_marker(
        &self,
        ts_unix_ms: i64,
        label: &str,
        detail: Option<&str>,
    ) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO capture_markers (ts_unix_ms, label, detail)
            VALUES (?, ?, ?)
            "#,
        )
        .bind(ts_unix_ms)
        .bind(label)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn list_capture_markers(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<CaptureMarker>> {
        let rows = sqlx::query(
            r#"
            SELECT id, ts_unix_ms, label, detail
            FROM capture_markers
            WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
            ORDER BY ts_unix_ms ASC
            "#,
        )
        .bind(from_ms)
        .bind(to_ms)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| CaptureMarker {
                id: r.get("id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                label: r.get("label"),
                detail: r.get("detail"),
            })
            .collect())
    }

    pub async fn insert_sample(&self, sample: &MetricSample) -> Result<i64> {
        let active_flags = serde_json::to_string(&sample.active_flags)?;
        let result = sqlx::query(
            r#"
            INSERT INTO samples (
                ts_unix_ms, device_pci, device_name, throttle_status_raw,
                indep_throttle_status, active_flags, apu_power_mw,
                stapm_limit_mw, current_stapm_limit_mw, temperature_core_max, extra_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(sample.ts_unix_ms)
        .bind(&sample.device_pci)
        .bind(&sample.device_name)
        .bind(sample.throttle_status_raw.map(|v| v as i64))
        .bind(sample.indep_throttle_status as i64)
        .bind(active_flags)
        .bind(sample.apu_power_mw.map(|v| v as i64))
        .bind(sample.stapm_limit_mw.map(|v| v as i64))
        .bind(sample.current_stapm_limit_mw.map(|v| v as i64))
        .bind(sample.temperature_core_max)
        .bind(&sample.extra_json)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn insert_transition(
        &self,
        ts_unix_ms: i64,
        sample_id: i64,
        device_pci: &str,
        flag_name: &str,
        old_value: bool,
        new_value: bool,
    ) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO transitions (ts_unix_ms, sample_id, device_pci, flag_name, old_value, new_value)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(ts_unix_ms)
        .bind(sample_id)
        .bind(device_pci)
        .bind(flag_name)
        .bind(old_value as i64)
        .bind(new_value as i64)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn insert_journal_event(
        &self,
        transition_id: i64,
        ts_unix_ms: i64,
        unit: Option<&str>,
        priority: Option<i32>,
        message: &str,
        cursor: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO journal_events (transition_id, ts_unix_ms, unit, priority, message, cursor)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(transition_id)
        .bind(ts_unix_ms)
        .bind(unit)
        .bind(priority)
        .bind(message)
        .bind(cursor)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_flag_transitions(
        &self,
        query: FlagTransitionQuery<'_>,
    ) -> Result<Vec<FlagTransition>> {
        let FlagTransitionQuery {
            from_ms,
            to_ms,
            limit,
            device_pci,
            flag_name,
            direction,
            newest_first,
        } = query;

        let mut sql = String::from(
            r#"
            SELECT id, ts_unix_ms, sample_id, device_pci, flag_name, old_value, new_value
            FROM transitions
            WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
            "#,
        );
        if device_pci.is_some() {
            sql.push_str(" AND device_pci = ?");
        }
        if flag_name.is_some() {
            sql.push_str(" AND flag_name = ?");
        }
        if let Some(dir) = direction {
            match dir {
                "asserted" => sql.push_str(" AND new_value = 1"),
                "cleared" => sql.push_str(" AND new_value = 0"),
                _ => {
                    return Err(Error::BadRequest(format!(
                        "unknown transition direction: {dir}"
                    )))
                }
            }
        }
        if newest_first {
            sql.push_str(" ORDER BY ts_unix_ms DESC LIMIT ?");
        } else {
            sql.push_str(" ORDER BY ts_unix_ms ASC LIMIT ?");
        }

        let mut q = sqlx::query(&sql).bind(from_ms).bind(to_ms);
        if let Some(pci) = device_pci {
            q = q.bind(pci);
        }
        if let Some(flag) = flag_name {
            q = q.bind(flag);
        }
        q = q.bind(limit);
        let rows = q.fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|r| FlagTransition {
                id: r.get("id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                sample_id: r.get("sample_id"),
                device_pci: r.get("device_pci"),
                flag_name: r.get("flag_name"),
                old_value: r.get::<i64, _>("old_value") != 0,
                new_value: r.get::<i64, _>("new_value") != 0,
            })
            .collect())
    }

    pub async fn journal_for_transition(&self, transition_id: i64) -> Result<Vec<JournalEvent>> {
        let rows = sqlx::query(
            r#"
            SELECT id, transition_id, ts_unix_ms, unit, priority, message
            FROM journal_events
            WHERE transition_id = ?
            ORDER BY ts_unix_ms ASC
            "#,
        )
        .bind(transition_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| JournalEvent {
                id: r.get("id"),
                transition_id: r.get("transition_id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                unit: r.get("unit"),
                priority: r.get("priority"),
                message: r.get("message"),
            })
            .collect())
    }

    pub async fn flag_series(
        &self,
        device_pci: Option<&str>,
        flag: &str,
        from_ms: i64,
        to_ms: i64,
        max_points: Option<usize>,
    ) -> Result<FlagSeries> {
        let rows = if let Some(pci) = device_pci {
            sqlx::query(
                r#"
                SELECT ts_unix_ms, indep_throttle_status
                FROM samples
                WHERE device_pci = ? AND ts_unix_ms >= ? AND ts_unix_ms <= ?
                ORDER BY ts_unix_ms ASC
                "#,
            )
            .bind(pci)
            .bind(from_ms)
            .bind(to_ms)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT ts_unix_ms, indep_throttle_status
                FROM samples
                WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
                ORDER BY ts_unix_ms ASC
                "#,
            )
            .bind(from_ms)
            .bind(to_ms)
            .fetch_all(&self.pool)
            .await?
        };

        let points = rows
            .into_iter()
            .map(|r| {
                let ts: i64 = r.get("ts_unix_ms");
                let mask: i64 = r.get("indep_throttle_status");
                FlagSeriesPoint {
                    ts_unix_ms: ts,
                    value: flag_active(mask as u64, flag) as u8,
                }
            })
            .collect();

        let points = match max_points {
            Some(max) if max > 0 => downsample_by_stride(points, max),
            _ => points,
        };

        Ok(FlagSeries {
            flag: flag.to_string(),
            points,
        })
    }

    pub async fn metric_series(
        &self,
        metric: &str,
        device_pci: Option<&str>,
        from_ms: i64,
        to_ms: i64,
        max_points: Option<usize>,
    ) -> Result<MetricSeries> {
        let (column, unit) = metric_column(metric)?;
        let sql = format!(
            "SELECT ts_unix_ms, CAST({column} AS REAL) AS value FROM samples WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?{} ORDER BY ts_unix_ms ASC",
            if device_pci.is_some() {
                " AND device_pci = ?"
            } else {
                ""
            }
        );
        let mut q = sqlx::query(&sql).bind(from_ms).bind(to_ms);
        if let Some(pci) = device_pci {
            q = q.bind(pci);
        }
        let rows = q.fetch_all(&self.pool).await?;

        let mut points: Vec<MetricSeriesPoint> = rows
            .into_iter()
            .filter_map(|r| {
                let ts: i64 = r.get("ts_unix_ms");
                let v: Option<f64> = r.get("value");
                v.map(|n| MetricSeriesPoint {
                    ts_unix_ms: ts,
                    value: n,
                })
            })
            .collect();

        if let Some(max) = max_points {
            if max > 0 {
                points = downsample_by_stride(points, max);
            }
        }

        Ok(MetricSeries {
            metric: metric.to_string(),
            unit: unit.to_string(),
            points,
        })
    }

    pub async fn window_summary(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<WindowSummary> {
        self.window_summary_sql(from_ms, to_ms, device_pci).await
    }

    pub async fn count_flag_transitions(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<i64> {
        let mut sql = String::from(
            "SELECT COUNT(*) AS c FROM transitions WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?",
        );
        if device_pci.is_some() {
            sql.push_str(" AND device_pci = ?");
        }
        let mut q = sqlx::query(&sql).bind(from_ms).bind(to_ms);
        if let Some(pci) = device_pci {
            q = q.bind(pci);
        }
        let row = q.fetch_one(&self.pool).await?;
        Ok(row.get("c"))
    }

    pub async fn count_context_transitions(&self, from_ms: i64, to_ms: i64) -> Result<i64> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS c FROM context_transitions WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?",
        )
        .bind(from_ms)
        .bind(to_ms)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("c"))
    }

    pub async fn distinct_flags(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT flag_name FROM transitions ORDER BY flag_name ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.get("flag_name")).collect())
    }

    pub async fn insert_context_snapshot(&self, snap: &ContextSnapshot) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO context_snapshots (ts_unix_ms, source_id, health, raw_json, error_message)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(snap.ts_unix_ms)
        .bind(&snap.source_id)
        .bind(&snap.health)
        .bind(&snap.raw_json)
        .bind(&snap.error_message)
        .execute(&self.pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    pub async fn insert_context_value(
        &self,
        snapshot_id: i64,
        ts_unix_ms: i64,
        source_id: &str,
        value: &ContextValue,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO context_values (snapshot_id, ts_unix_ms, source_id, key, value_num, value_str)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(snapshot_id)
        .bind(ts_unix_ms)
        .bind(source_id)
        .bind(&value.key)
        .bind(value.value_num)
        .bind(&value.value_str)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_context_transition(&self, t: &ContextTransition) -> Result<i64> {
        let old = serde_json::to_string(&t.old_value)?;
        let new = serde_json::to_string(&t.new_value)?;
        let result = sqlx::query(
            r#"
            INSERT INTO context_transitions (ts_unix_ms, snapshot_id, source_id, key, old_value, new_value)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(t.ts_unix_ms)
        .bind(t.snapshot_id)
        .bind(&t.source_id)
        .bind(&t.key)
        .bind(old)
        .bind(new)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn insert_context_journal_event(
        &self,
        context_transition_id: i64,
        ts_unix_ms: i64,
        unit: Option<&str>,
        priority: Option<i32>,
        message: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO context_journal_events (context_transition_id, ts_unix_ms, unit, priority, message)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(context_transition_id)
        .bind(ts_unix_ms)
        .bind(unit)
        .bind(priority)
        .bind(message)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_context_transitions_query(
        &self,
        query: ContextTransitionQuery<'_>,
    ) -> Result<Vec<ContextTransition>> {
        let ContextTransitionQuery {
            from_ms,
            to_ms,
            limit,
            source_id,
            key,
            newest_first,
        } = query;

        let mut sql = String::from(
            r#"
            SELECT id, ts_unix_ms, snapshot_id, source_id, key, old_value, new_value
            FROM context_transitions
            WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
            "#,
        );
        if source_id.is_some() {
            sql.push_str(" AND source_id = ?");
        }
        if key.is_some() {
            sql.push_str(" AND key = ?");
        }
        if newest_first {
            sql.push_str(" ORDER BY ts_unix_ms DESC LIMIT ?");
        } else {
            sql.push_str(" ORDER BY ts_unix_ms ASC LIMIT ?");
        }

        let mut q = sqlx::query(&sql).bind(from_ms).bind(to_ms);
        if let Some(s) = source_id {
            q = q.bind(s);
        }
        if let Some(k) = key {
            q = q.bind(k);
        }
        q = q.bind(limit);
        let rows = q.fetch_all(&self.pool).await?;

        rows.into_iter()
            .map(|r| {
                let old_s: String = r.get("old_value");
                let new_s: String = r.get("new_value");
                Ok(ContextTransition {
                    id: r.get("id"),
                    ts_unix_ms: r.get("ts_unix_ms"),
                    snapshot_id: r.get("snapshot_id"),
                    source_id: r.get("source_id"),
                    key: r.get("key"),
                    old_value: serde_json::from_str(&old_s)?,
                    new_value: serde_json::from_str(&new_s)?,
                })
            })
            .collect()
    }

    pub async fn journal_for_context_transition(
        &self,
        transition_id: i64,
    ) -> Result<Vec<JournalEvent>> {
        let rows = sqlx::query(
            r#"
            SELECT id, context_transition_id, ts_unix_ms, unit, priority, message
            FROM context_journal_events
            WHERE context_transition_id = ?
            ORDER BY ts_unix_ms ASC
            "#,
        )
        .bind(transition_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| JournalEvent {
                id: r.get("id"),
                transition_id: r.get("context_transition_id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                unit: r.get("unit"),
                priority: r.get("priority"),
                message: r.get("message"),
            })
            .collect())
    }

    pub async fn context_series(
        &self,
        key: &str,
        from_ms: i64,
        to_ms: i64,
        max_points: Option<usize>,
    ) -> Result<ContextSeries> {
        let value_kind = context_key_kind(key);
        let rows = sqlx::query(
            r#"
            SELECT ts_unix_ms, value_num, value_str
            FROM context_values
            WHERE key = ? AND ts_unix_ms >= ? AND ts_unix_ms <= ?
            ORDER BY ts_unix_ms ASC
            "#,
        )
        .bind(key)
        .bind(from_ms)
        .bind(to_ms)
        .fetch_all(&self.pool)
        .await?;

        let mut enum_labels = std::collections::HashMap::new();
        let points = rows
            .into_iter()
            .filter_map(|r| {
                let ts: i64 = r.get("ts_unix_ms");
                let num: Option<f64> = r.get("value_num");
                let s: Option<String> = r.get("value_str");
                let value = if value_kind == ContextValueKind::Number {
                    num.or_else(|| s.as_ref().map(|x| context_str_to_num(x)))?
                } else {
                    let label = s
                        .clone()
                        .unwrap_or_else(|| num.map(|n| n.to_string()).unwrap_or_default());
                    let mapped = context_str_to_num(&label);
                    enum_labels.insert(label, mapped);
                    mapped
                };
                Some(ContextSeriesPoint {
                    ts_unix_ms: ts,
                    value,
                })
            })
            .collect::<Vec<_>>();

        let points = match max_points {
            Some(max) if max > 0 => downsample_by_stride(points, max),
            _ => points,
        };

        Ok(ContextSeries {
            key: key.to_string(),
            value_kind,
            enum_labels: if enum_labels.is_empty() {
                None
            } else {
                Some(enum_labels)
            },
            points,
        })
    }

    pub async fn context_keys(&self) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT key FROM context_values ORDER BY key ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.get("key")).collect())
    }

    pub async fn latest_context_health(&self) -> Result<Vec<ContextSourceStatus>> {
        let rows = sqlx::query(
            r#"
            SELECT cs.source_id, cs.ts_unix_ms, cs.health, cs.error_message
            FROM context_snapshots cs
            INNER JOIN (
                SELECT source_id, MAX(ts_unix_ms) AS max_ts
                FROM context_snapshots
                GROUP BY source_id
            ) latest ON cs.source_id = latest.source_id AND cs.ts_unix_ms = latest.max_ts
            ORDER BY cs.source_id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| ContextSourceStatus {
                source_id: r.get("source_id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                health: r.get("health"),
                error_message: r.get("error_message"),
            })
            .collect())
    }

    pub async fn time_bounds(&self) -> Result<TimeBounds> {
        let row = sqlx::query(
            r#"
            SELECT
                (SELECT MIN(ts_unix_ms) FROM samples) AS min_sample_ts,
                (SELECT MAX(ts_unix_ms) FROM samples) AS max_sample_ts,
                (SELECT COUNT(*) FROM samples) AS sample_count,
                (SELECT COUNT(*) FROM context_snapshots) AS context_snapshot_count
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        let min_sample: Option<i64> = row.get("min_sample_ts");
        let max_sample: Option<i64> = row.get("max_sample_ts");
        let min_ctx =
            sqlx::query_scalar::<_, Option<i64>>("SELECT MIN(ts_unix_ms) FROM context_snapshots")
                .fetch_one(&self.pool)
                .await?;
        let max_ctx =
            sqlx::query_scalar::<_, Option<i64>>("SELECT MAX(ts_unix_ms) FROM context_snapshots")
                .fetch_one(&self.pool)
                .await?;

        let min_ts_ms = match (min_sample, min_ctx) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let max_ts_ms = match (max_sample, max_ctx) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };

        Ok(TimeBounds {
            min_ts_ms,
            max_ts_ms,
            sample_count: row.get("sample_count"),
            context_snapshot_count: row.get("context_snapshot_count"),
        })
    }

    /// Returns an error when samples were written within `quiet_ms` of now.
    pub async fn assert_not_collecting(&self, quiet_ms: i64) -> Result<()> {
        let bounds = self.time_bounds().await?;
        let Some(max_ts) = bounds.max_ts_ms else {
            return Ok(());
        };
        let now = crate::util::current_ts_ms();
        if now.saturating_sub(max_ts) < quiet_ms {
            return Err(Error::BadRequest(format!(
                "database still receiving samples (last sample {max_ts} ms, now {now} ms); stop framelog.service or any other collector using this DB first"
            )));
        }
        Ok(())
    }

    pub async fn reset_data_collection(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for table in [
            "journal_events",
            "context_journal_events",
            "transitions",
            "context_values",
            "context_transitions",
            "context_snapshots",
            "samples",
            "system_info_snapshots",
            "capture_markers",
        ] {
            sqlx::query(&format!("DELETE FROM {table}"))
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            r#"
            DELETE FROM sqlite_sequence
            WHERE name IN (
                'journal_events',
                'context_journal_events',
                'transitions',
                'context_values',
                'context_transitions',
                'context_snapshots',
                'samples',
                'system_info_snapshots',
                'capture_markers'
            )
            "#,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn insert_system_info_snapshot(&self, info: &SystemInfo) -> Result<i64> {
        let info_json = serde_json::to_string(info)?;
        let result = sqlx::query(
            r#"
            INSERT INTO system_info_snapshots (collected_at_ms, schema_version, health, info_json)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(info.collected_at_ms)
        .bind(info.schema_version as i64)
        .bind(&info.health)
        .bind(info_json)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn latest_system_info(&self) -> Result<Option<SystemInfo>> {
        let row = sqlx::query(
            r#"
            SELECT info_json
            FROM system_info_snapshots
            ORDER BY collected_at_ms DESC, id DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| serde_json::from_str(r.get::<&str, _>("info_json")))
            .transpose()
            .map_err(Error::from)
    }

    pub async fn count_samples_in_range(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<i64> {
        let count: i64 = if let Some(pci) = device_pci {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM samples WHERE device_pci = ? AND ts_unix_ms >= ? AND ts_unix_ms <= ?",
            )
            .bind(pci)
            .bind(from_ms)
            .bind(to_ms)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM samples WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?",
            )
            .bind(from_ms)
            .bind(to_ms)
            .fetch_one(&self.pool)
            .await?
        };
        Ok(count)
    }

    pub async fn list_samples(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<Vec<MetricSample>> {
        let rows = if let Some(pci) = device_pci {
            sqlx::query(
                r#"
                SELECT ts_unix_ms, device_pci, device_name, throttle_status_raw,
                       indep_throttle_status, active_flags, apu_power_mw,
                       stapm_limit_mw, current_stapm_limit_mw, temperature_core_max, extra_json
                FROM samples
                WHERE device_pci = ? AND ts_unix_ms >= ? AND ts_unix_ms <= ?
                ORDER BY ts_unix_ms ASC
                "#,
            )
            .bind(pci)
            .bind(from_ms)
            .bind(to_ms)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                SELECT ts_unix_ms, device_pci, device_name, throttle_status_raw,
                       indep_throttle_status, active_flags, apu_power_mw,
                       stapm_limit_mw, current_stapm_limit_mw, temperature_core_max, extra_json
                FROM samples
                WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
                ORDER BY ts_unix_ms ASC
                "#,
            )
            .bind(from_ms)
            .bind(to_ms)
            .fetch_all(&self.pool)
            .await?
        };

        rows.into_iter().map(row_to_metric_sample).collect()
    }

    pub async fn list_context_snapshots(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ContextSnapshot>> {
        let rows = sqlx::query(
            r#"
            SELECT source_id, ts_unix_ms, health, raw_json, error_message
            FROM context_snapshots
            WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
            ORDER BY ts_unix_ms ASC
            "#,
        )
        .bind(from_ms)
        .bind(to_ms)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| ContextSnapshot {
                source_id: r.get("source_id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                health: r.get("health"),
                values: Vec::new(),
                raw_json: r.get("raw_json"),
                error_message: r.get("error_message"),
            })
            .collect())
    }

    pub async fn list_context_values(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<ContextValueRow>> {
        let rows = sqlx::query(
            r#"
            SELECT snapshot_id, ts_unix_ms, source_id, key, value_num, value_str
            FROM context_values
            WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
            ORDER BY ts_unix_ms ASC
            "#,
        )
        .bind(from_ms)
        .bind(to_ms)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| ContextValueRow {
                snapshot_id: r.get("snapshot_id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                source_id: r.get("source_id"),
                key: r.get("key"),
                value_num: r.get("value_num"),
                value_str: r.get("value_str"),
            })
            .collect())
    }

    pub async fn journal_for_transitions(
        &self,
        transition_ids: &[i64],
    ) -> Result<Vec<JournalEvent>> {
        if transition_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        for chunk in transition_ids.chunks(JOURNAL_IN_BATCH) {
            events.extend(self.journal_for_transitions_batch(chunk).await?);
        }
        events.sort_by_key(|e| e.ts_unix_ms);
        Ok(events)
    }

    async fn journal_for_transitions_batch(
        &self,
        transition_ids: &[i64],
    ) -> Result<Vec<JournalEvent>> {
        let placeholders = transition_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, transition_id, ts_unix_ms, unit, priority, message FROM journal_events WHERE transition_id IN ({placeholders}) ORDER BY ts_unix_ms ASC"
        );
        let mut q = sqlx::query(&sql);
        for id in transition_ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| JournalEvent {
                id: r.get("id"),
                transition_id: r.get("transition_id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                unit: r.get("unit"),
                priority: r.get("priority"),
                message: r.get("message"),
            })
            .collect())
    }

    pub async fn journal_for_context_transitions(
        &self,
        transition_ids: &[i64],
    ) -> Result<Vec<JournalEvent>> {
        if transition_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        for chunk in transition_ids.chunks(JOURNAL_IN_BATCH) {
            events.extend(self.journal_for_context_transitions_batch(chunk).await?);
        }
        events.sort_by_key(|e| e.ts_unix_ms);
        Ok(events)
    }

    async fn journal_for_context_transitions_batch(
        &self,
        transition_ids: &[i64],
    ) -> Result<Vec<JournalEvent>> {
        let placeholders = transition_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, context_transition_id, ts_unix_ms, unit, priority, message FROM context_journal_events WHERE context_transition_id IN ({placeholders}) ORDER BY ts_unix_ms ASC"
        );
        let mut q = sqlx::query(&sql);
        for id in transition_ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| JournalEvent {
                id: r.get("id"),
                transition_id: r.get("context_transition_id"),
                ts_unix_ms: r.get("ts_unix_ms"),
                unit: r.get("unit"),
                priority: r.get("priority"),
                message: r.get("message"),
            })
            .collect())
    }

    pub async fn devices(&self) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query(
            r#"
            SELECT DISTINCT device_pci, device_name FROM samples ORDER BY device_pci ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| (r.get("device_pci"), r.get("device_name")))
            .collect())
    }
}

fn metric_column(metric: &str) -> Result<(&'static str, &'static str)> {
    match metric {
        "apu_power_mw" => Ok(("apu_power_mw", "mW")),
        "stapm_limit_mw" => Ok(("stapm_limit_mw", "mW")),
        "current_stapm_limit_mw" => Ok(("current_stapm_limit_mw", "mW")),
        "temperature_core_max" => Ok(("temperature_core_max", "C")),
        _ => Err(Error::BadRequest(format!("unknown metric: {metric}"))),
    }
}

fn row_to_metric_sample(r: sqlx::sqlite::SqliteRow) -> Result<MetricSample> {
    let active_flags_json: String = r.get("active_flags");
    let active_flags: Vec<String> = serde_json::from_str(&active_flags_json)?;
    let throttle_raw: Option<i64> = r.get("throttle_status_raw");
    Ok(MetricSample {
        ts_unix_ms: r.get("ts_unix_ms"),
        device_pci: r.get("device_pci"),
        device_name: r.get("device_name"),
        throttle_status_raw: throttle_raw.map(|v| v as u32),
        indep_throttle_status: r.get::<i64, _>("indep_throttle_status") as u64,
        active_flags,
        apu_power_mw: r.get::<Option<i64>, _>("apu_power_mw").map(|v| v as u32),
        stapm_limit_mw: r.get::<Option<i64>, _>("stapm_limit_mw").map(|v| v as u16),
        current_stapm_limit_mw: r
            .get::<Option<i64>, _>("current_stapm_limit_mw")
            .map(|v| v as u16),
        temperature_core_max: r.get("temperature_core_max"),
        extra_json: r.get("extra_json"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MetricSample;

    #[tokio::test]
    async fn roundtrip_sample_and_transition() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let store = Store::open(&path).await.unwrap();

        let sample = MetricSample {
            ts_unix_ms: 1_700_000_000_000,
            device_pci: "0000:c1:00.0".into(),
            device_name: "Test GPU".into(),
            throttle_status_raw: Some(0),
            indep_throttle_status: 1 << 4,
            active_flags: vec!["SPL".into()],
            apu_power_mw: Some(35_000),
            stapm_limit_mw: Some(35_000),
            current_stapm_limit_mw: Some(35_000),
            temperature_core_max: Some(74.0),
            extra_json: None,
        };

        let sample_id = store.insert_sample(&sample).await.unwrap();
        let transition_id = store
            .insert_transition(
                sample.ts_unix_ms,
                sample_id,
                &sample.device_pci,
                "SPL",
                false,
                true,
            )
            .await
            .unwrap();

        store
            .insert_journal_event(
                transition_id,
                sample.ts_unix_ms,
                Some("kernel.service"),
                Some(4),
                "test message",
                None,
            )
            .await
            .unwrap();

        let transitions = store
            .list_flag_transitions(FlagTransitionQuery {
                from_ms: sample.ts_unix_ms - 1,
                to_ms: sample.ts_unix_ms + 1,
                limit: 10,
                device_pci: None,
                flag_name: None,
                direction: None,
                newest_first: true,
            })
            .await
            .unwrap();
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].flag_name, "SPL");

        let journal = store.journal_for_transition(transition_id).await.unwrap();
        assert_eq!(journal.len(), 1);
        assert_eq!(journal[0].message, "test message");

        let stapm = store
            .metric_series(
                "current_stapm_limit_mw",
                None,
                sample.ts_unix_ms - 1,
                sample.ts_unix_ms + 1,
                None,
            )
            .await
            .unwrap();
        assert_eq!(stapm.points.len(), 1);
        assert_eq!(stapm.points[0].value, 35_000.0);
    }

    #[tokio::test]
    async fn roundtrip_context_snapshot_and_transition() {
        use crate::model::{ContextSnapshot, ContextTransition, ContextValue};
        use serde_json::json;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("ctx.db")).await.unwrap();

        let snap = ContextSnapshot {
            source_id: "power".into(),
            ts_unix_ms: 1_700_000_000_000,
            health: "ok".into(),
            values: vec![
                ContextValue::num("ac_connected", 1.0),
                ContextValue::num("input_watts", 65.0),
            ],
            raw_json: "{}".into(),
            error_message: None,
        };
        let snapshot_id = store.insert_context_snapshot(&snap).await.unwrap();
        store
            .insert_context_value(
                snapshot_id,
                snap.ts_unix_ms,
                "power",
                &ContextValue {
                    key: "power.ac_connected".into(),
                    value_num: Some(1.0),
                    value_str: None,
                },
            )
            .await
            .unwrap();

        let transition = ContextTransition {
            id: 0,
            ts_unix_ms: snap.ts_unix_ms,
            snapshot_id,
            source_id: "power".into(),
            key: "power.ac_connected".into(),
            old_value: json!(false),
            new_value: json!(true),
        };
        let transition_id = store.insert_context_transition(&transition).await.unwrap();
        store
            .insert_context_journal_event(
                transition_id,
                snap.ts_unix_ms,
                Some("systemd-logind"),
                Some(6),
                "Power key pressed",
            )
            .await
            .unwrap();

        let listed = store
            .list_context_transitions_query(ContextTransitionQuery {
                from_ms: snap.ts_unix_ms - 1,
                to_ms: snap.ts_unix_ms + 1,
                limit: 10,
                source_id: None,
                key: None,
                newest_first: true,
            })
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, "power.ac_connected");

        let journal = store
            .journal_for_context_transition(transition_id)
            .await
            .unwrap();
        assert_eq!(journal.len(), 1);

        let series = store
            .context_series(
                "power.ac_connected",
                snap.ts_unix_ms - 1,
                snap.ts_unix_ms + 1,
                None,
            )
            .await
            .unwrap();
        assert_eq!(series.points.len(), 1);
        assert_eq!(series.points[0].value, 1.0);

        let keys = store.context_keys().await.unwrap();
        assert!(keys.contains(&"power.ac_connected".to_string()));
    }

    #[tokio::test]
    async fn time_bounds_and_export_bundle() {
        use crate::model::{
            BiosInfo, ContextSnapshot, ContextValue, CpuInfo, MemoryInfo, OsInfo, SystemInfo,
        };

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("exp.db")).await.unwrap();
        let ts = 1_700_000_000_000_i64;

        let sample = MetricSample {
            ts_unix_ms: ts,
            device_pci: "0000:c1:00.0".into(),
            device_name: "GPU".into(),
            throttle_status_raw: None,
            indep_throttle_status: 0,
            active_flags: vec!["FPPT".into()],
            apu_power_mw: Some(30_000),
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: Some(70.0),
            extra_json: None,
        };
        store.insert_sample(&sample).await.unwrap();

        let snap = ContextSnapshot {
            source_id: "power".into(),
            ts_unix_ms: ts,
            health: "ok".into(),
            values: vec![ContextValue::num("ac_connected", 1.0)],
            raw_json: "{}".into(),
            error_message: None,
        };
        let sid = store.insert_context_snapshot(&snap).await.unwrap();
        store
            .insert_context_value(
                sid,
                ts,
                "power",
                &ContextValue {
                    key: "power.ac_connected".into(),
                    value_num: Some(1.0),
                    value_str: None,
                },
            )
            .await
            .unwrap();

        let system = SystemInfo {
            schema_version: 1,
            collected_at_ms: ts + 10,
            health: "ok".into(),
            warnings: Vec::new(),
            cpu: CpuInfo {
                vendor: Some("AuthenticAMD".into()),
                model_name: Some("Test CPU".into()),
                logical_cpus: Some(16),
                physical_cores: Some(8),
                max_frequency_mhz: Some(5000),
            },
            gpus: Vec::new(),
            memory: MemoryInfo::default(),
            storage: Vec::new(),
            bios: BiosInfo::default(),
            os: OsInfo {
                kernel_release: Some("7.0.9-test".into()),
                ..Default::default()
            },
        };
        store.insert_system_info_snapshot(&system).await.unwrap();
        let latest_system = store.latest_system_info().await.unwrap().unwrap();
        assert_eq!(latest_system.cpu.model_name.as_deref(), Some("Test CPU"));

        let bounds = store.time_bounds().await.unwrap();
        assert_eq!(bounds.sample_count, 1);
        assert_eq!(bounds.min_ts_ms, Some(ts));

        let listed = store.list_samples(ts - 1, ts + 1, None).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].apu_power_mw, Some(30_000));

        let bundle = store.export_bundle(ts - 1, ts + 1, None).await.unwrap();
        assert_eq!(bundle.schema_version, 1);
        assert_eq!(bundle.format, crate::model::EXPORT_FORMAT);
        assert_eq!(bundle.samples.len(), 1);
        assert_eq!(bundle.summary.as_ref().map(|s| s.sample_count), Some(1));
        let json = serde_json::to_string(&bundle).unwrap();
        let parsed: crate::model::ExportBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.format, crate::model::EXPORT_FORMAT);

        assert_eq!(bundle.context_values.len(), 1);
        assert_eq!(
            bundle
                .system
                .as_ref()
                .and_then(|s| s.os.kernel_release.as_deref()),
            Some("7.0.9-test")
        );

        let err = store
            .export_bundle(ts, ts + MAX_EXPORT_WINDOW_MS + 1, None)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)));

        store.reset_data_collection().await.unwrap();
        let bounds = store.time_bounds().await.unwrap();
        assert_eq!(bounds.sample_count, 0);
        assert_eq!(bounds.context_snapshot_count, 0);
        assert_eq!(bounds.min_ts_ms, None);
        assert!(store.latest_system_info().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn capture_markers_roundtrip_and_reset() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("markers.db")).await.unwrap();
        let ts = 1_700_000_000_000_i64;

        store
            .insert_capture_marker(ts, "capture_start", Some("framework-146"))
            .await
            .unwrap();
        store
            .insert_capture_marker(ts + 10, "capture_end", None)
            .await
            .unwrap();

        let markers = store.list_capture_markers(ts - 1, ts + 11).await.unwrap();
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].label, "capture_start");
        assert_eq!(markers[0].detail.as_deref(), Some("framework-146"));
        assert_eq!(markers[1].label, "capture_end");

        store.reset_data_collection().await.unwrap();
        let markers = store.list_capture_markers(ts - 1, ts + 11).await.unwrap();
        assert!(markers.is_empty());
    }

    #[tokio::test]
    async fn sql_window_summary_matches_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("sql-summary.db"))
            .await
            .unwrap();
        let ts = 1_700_000_000_000_i64;
        let sample = MetricSample {
            ts_unix_ms: ts,
            device_pci: "0000:c1:00.0".into(),
            device_name: "GPU".into(),
            throttle_status_raw: None,
            indep_throttle_status: 1 << 4,
            active_flags: vec!["SPL".into(), "FPPT".into()],
            apu_power_mw: Some(40_000),
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: Some(80.0),
            extra_json: None,
        };
        let sid = store.insert_sample(&sample).await.unwrap();
        store
            .insert_transition(ts, sid, "0000:c1:00.0", "SPL", false, true)
            .await
            .unwrap();

        let sql = store
            .window_summary_sql(ts - 1, ts + 1, None)
            .await
            .unwrap();
        let mem = crate::summary::summarize_window(
            ts - 1,
            ts + 1,
            &[sample],
            &store
                .list_flag_transitions(FlagTransitionQuery {
                    from_ms: ts - 1,
                    to_ms: ts + 1,
                    limit: 100,
                    device_pci: None,
                    flag_name: None,
                    direction: None,
                    newest_first: false,
                })
                .await
                .unwrap(),
            &[],
        );
        assert_eq!(sql.sample_count, mem.sample_count);
        assert_eq!(sql.max_apu_power_mw, mem.max_apu_power_mw);
        assert_eq!(sql.throttle_transition_count, mem.throttle_transition_count);
        let sql_spl = sql
            .flag_activity
            .iter()
            .find(|f| f.flag_name == "SPL")
            .unwrap();
        let mem_spl = mem
            .flag_activity
            .iter()
            .find(|f| f.flag_name == "SPL")
            .unwrap();
        assert_eq!(sql_spl.assert_count, mem_spl.assert_count);
        assert!((sql_spl.active_sample_pct - mem_spl.active_sample_pct).abs() < 0.01);
    }

    #[tokio::test]
    async fn export_bundle_has_no_analysis_until_attached() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("export-raw.db"))
            .await
            .unwrap();
        let ts = 1_700_000_000_000_i64;
        let sample = MetricSample {
            ts_unix_ms: ts,
            device_pci: "0000:c1:00.0".into(),
            device_name: "GPU".into(),
            throttle_status_raw: None,
            indep_throttle_status: 0,
            active_flags: vec![],
            apu_power_mw: Some(30_000),
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: None,
            extra_json: None,
        };
        store.insert_sample(&sample).await.unwrap();
        let bundle = store
            .build_export_bundle(super::export::ExportOptions {
                from_ms: ts - 1,
                to_ms: ts + 1,
                device_pci: None,
                enforce_web_limits: false,
            })
            .await
            .unwrap();
        assert!(bundle.analysis.is_none());
        let with_analysis = store.export_bundle(ts - 1, ts + 1, None).await.unwrap();
        assert!(with_analysis.analysis.is_some());
    }

    #[tokio::test]
    async fn analysis_bundle_omits_export_only_payload() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("analysis-bundle.db"))
            .await
            .unwrap();
        let ts = 1_700_000_000_000_i64;
        let sample = MetricSample {
            ts_unix_ms: ts,
            device_pci: "0000:c1:00.0".into(),
            device_name: "GPU".into(),
            throttle_status_raw: None,
            indep_throttle_status: 0,
            active_flags: vec![],
            apu_power_mw: Some(30_000),
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: None,
            extra_json: None,
        };
        store.insert_sample(&sample).await.unwrap();

        let bundle = store
            .build_analysis_bundle(ts - 1, ts + 1, Some("0000:c1:00.0"))
            .await
            .unwrap();
        assert_eq!(bundle.samples.len(), 1);
        assert!(bundle.devices.is_empty());
        assert!(bundle.context_snapshots.is_empty());
        assert!(bundle.analysis.is_none());
    }

    #[tokio::test]
    async fn journal_for_transitions_batches_large_id_sets() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("journal-batch.db"))
            .await
            .unwrap();
        let ts = 1_700_000_000_000_i64;
        let sample = MetricSample {
            ts_unix_ms: ts,
            device_pci: "0000:c1:00.0".into(),
            device_name: "GPU".into(),
            throttle_status_raw: None,
            indep_throttle_status: 0,
            active_flags: vec![],
            apu_power_mw: None,
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: None,
            extra_json: None,
        };
        let sid = store.insert_sample(&sample).await.unwrap();

        let mut ids = Vec::new();
        for i in 0..600 {
            let id = store
                .insert_transition(ts + i, sid, "0000:c1:00.0", "SPL", false, true)
                .await
                .unwrap();
            store
                .insert_journal_event(id, ts + i, None, None, &format!("event-{i}"), None)
                .await
                .unwrap();
            ids.push(id);
        }

        let events = store.journal_for_transitions(&ids).await.unwrap();
        assert_eq!(events.len(), 600);
        assert_eq!(events.first().unwrap().message, "event-0");
        assert_eq!(events.last().unwrap().message, "event-599");
    }

    #[tokio::test]
    async fn filtered_transitions_and_metric_series() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("filt.db")).await.unwrap();
        let ts = 1_700_000_000_000_i64;

        let sample = MetricSample {
            ts_unix_ms: ts,
            device_pci: "0000:c1:00.0".into(),
            device_name: "GPU".into(),
            throttle_status_raw: None,
            indep_throttle_status: 0,
            active_flags: vec!["FPPT".into()],
            apu_power_mw: Some(35_000),
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: Some(72.0),
            extra_json: None,
        };
        let sid = store.insert_sample(&sample).await.unwrap();
        store
            .insert_transition(ts, sid, "0000:c1:00.0", "SPL", false, true)
            .await
            .unwrap();
        store
            .insert_transition(ts + 1, sid, "0000:c1:00.0", "SPL", true, false)
            .await
            .unwrap();

        let asserted = store
            .list_flag_transitions(FlagTransitionQuery {
                from_ms: ts - 1,
                to_ms: ts + 2,
                limit: 10,
                device_pci: Some("0000:c1:00.0"),
                flag_name: Some("SPL"),
                direction: Some("asserted"),
                newest_first: true,
            })
            .await
            .unwrap();
        assert_eq!(asserted.len(), 1);
        assert!(asserted[0].new_value);
        let bad_direction = store
            .list_flag_transitions(FlagTransitionQuery {
                from_ms: ts - 1,
                to_ms: ts + 2,
                limit: 10,
                device_pci: Some("0000:c1:00.0"),
                flag_name: Some("SPL"),
                direction: Some("maybe"),
                newest_first: true,
            })
            .await
            .unwrap_err();
        assert!(matches!(bad_direction, Error::BadRequest(_)));

        let power = store
            .metric_series("apu_power_mw", Some("0000:c1:00.0"), ts - 1, ts + 1, None)
            .await
            .unwrap();
        assert_eq!(power.points.len(), 1);
        assert_eq!(power.points[0].value, 35_000.0);

        let summary = store
            .window_summary(ts - 1, ts + 2, Some("0000:c1:00.0"))
            .await
            .unwrap();
        assert_eq!(summary.sample_count, 1);
        assert_eq!(summary.max_apu_power_mw, Some(35_000));
        let fppt = summary
            .flag_activity
            .iter()
            .find(|f| f.flag_name == "FPPT")
            .expect("active flags without transitions are included");
        assert_eq!(fppt.active_sample_pct, 100.0);
        assert_eq!(fppt.assert_count, 0);
        assert_eq!(fppt.clear_count, 0);
    }

    #[tokio::test]
    async fn stores_temperature_as_celsius_real() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("temp.db")).await.unwrap();
        let ts = 1_700_000_000_000_i64;
        let sample = MetricSample {
            ts_unix_ms: ts,
            device_pci: "0000:c1:00.0".into(),
            device_name: "GPU".into(),
            throttle_status_raw: None,
            indep_throttle_status: 0,
            active_flags: vec![],
            apu_power_mw: None,
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: Some(72.5),
            extra_json: None,
        };
        store.insert_sample(&sample).await.unwrap();

        let samples = store.list_samples(ts - 1, ts + 1, None).await.unwrap();
        assert_eq!(samples[0].temperature_core_max, Some(72.5));

        let series = store
            .metric_series("temperature_core_max", None, ts - 1, ts + 1, None)
            .await
            .unwrap();
        assert_eq!(series.points[0].value, 72.5);

        let summary = store.window_summary(ts - 1, ts + 1, None).await.unwrap();
        assert_eq!(summary.max_temperature_core, Some(72.5));
    }

    #[tokio::test]
    async fn context_series_marks_external_connected_as_numeric() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("ctx-kind.db")).await.unwrap();
        let ts = 1_700_000_000_000_i64;
        let snap = ContextSnapshot {
            source_id: "display".into(),
            ts_unix_ms: ts,
            health: "ok".into(),
            values: vec![ContextValue::num("external_connected", 1.0)],
            raw_json: "{}".into(),
            error_message: None,
        };
        let sid = store.insert_context_snapshot(&snap).await.unwrap();
        store
            .insert_context_value(
                sid,
                ts,
                "display",
                &ContextValue::num("display.external_connected", 1.0),
            )
            .await
            .unwrap();

        let series = store
            .context_series("display.external_connected", ts - 1, ts + 1, None)
            .await
            .unwrap();
        assert_eq!(series.value_kind, ContextValueKind::Number);
        assert_eq!(series.points.len(), 1);
        assert_eq!(series.points[0].value, 1.0);
    }
}
