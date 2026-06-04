use std::collections::HashMap;

use sqlx::Row;

use crate::error::Result;
use crate::model::{FlagActivitySummary, WindowSummary};
use crate::store::Store;

impl Store {
    pub async fn window_summary_sql(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<WindowSummary> {
        let sample_count = self
            .count_samples_in_range(from_ms, to_ms, device_pci)
            .await?;
        let throttle_transition_count = self
            .count_flag_transitions(from_ms, to_ms, device_pci)
            .await?;
        let context_transition_count = self.count_context_transitions(from_ms, to_ms).await?;

        let (max_apu_power_mw, avg_apu_power_mw, max_temperature_core) =
            self.sample_power_stats(from_ms, to_ms, device_pci).await?;

        let mut flag_stats: HashMap<String, (u64, u64)> = HashMap::new();
        for row in self
            .flag_transition_stats(from_ms, to_ms, device_pci)
            .await?
        {
            flag_stats.insert(row.0, (row.1, row.2));
        }

        let active_by_flag = self
            .flag_active_sample_counts(from_ms, to_ms, device_pci)
            .await?;

        for flag in active_by_flag.keys() {
            flag_stats.entry(flag.clone()).or_insert((0, 0));
        }

        let mut flag_activity: Vec<FlagActivitySummary> = flag_stats
            .into_iter()
            .map(|(flag_name, (assert_count, clear_count))| {
                let active = active_by_flag.get(&flag_name).copied().unwrap_or(0);
                let pct = if sample_count > 0 {
                    (active as f64 / sample_count as f64) * 100.0
                } else {
                    0.0
                };
                FlagActivitySummary {
                    flag_name,
                    active_sample_pct: pct,
                    assert_count,
                    clear_count,
                }
            })
            .collect();
        flag_activity.sort_by(|a, b| a.flag_name.cmp(&b.flag_name));

        Ok(WindowSummary {
            from_ms,
            to_ms,
            sample_count,
            throttle_transition_count,
            context_transition_count,
            flag_activity,
            max_apu_power_mw,
            avg_apu_power_mw,
            max_temperature_core,
        })
    }

    async fn sample_power_stats(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<(Option<u32>, Option<u32>, Option<f64>)> {
        let mut sql = String::from(
            r#"
            SELECT
                MAX(apu_power_mw) AS max_apu,
                AVG(apu_power_mw) AS avg_apu,
                CAST(MAX(temperature_core_max) AS REAL) AS max_temp
            FROM samples
            WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
            "#,
        );
        if device_pci.is_some() {
            sql.push_str(" AND device_pci = ?");
        }
        let mut q = sqlx::query(&sql).bind(from_ms).bind(to_ms);
        if let Some(pci) = device_pci {
            q = q.bind(pci);
        }
        let row = q.fetch_one(&self.pool).await?;
        let max_apu: Option<i64> = row.get("max_apu");
        let avg_apu: Option<f64> = row.get("avg_apu");
        let max_temp: Option<f64> = row.get("max_temp");
        Ok((
            max_apu.map(|v| v as u32),
            avg_apu.map(|v| v as u32),
            max_temp,
        ))
    }

    async fn flag_transition_stats(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<Vec<(String, u64, u64)>> {
        let mut sql = String::from(
            r#"
            SELECT flag_name,
                   SUM(CASE WHEN new_value = 1 THEN 1 ELSE 0 END) AS assert_count,
                   SUM(CASE WHEN new_value = 0 THEN 1 ELSE 0 END) AS clear_count
            FROM transitions
            WHERE ts_unix_ms >= ? AND ts_unix_ms <= ?
            "#,
        );
        if device_pci.is_some() {
            sql.push_str(" AND device_pci = ?");
        }
        sql.push_str(" GROUP BY flag_name");

        let mut q = sqlx::query(&sql).bind(from_ms).bind(to_ms);
        if let Some(pci) = device_pci {
            q = q.bind(pci);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<String, _>("flag_name"),
                    r.get::<i64, _>("assert_count") as u64,
                    r.get::<i64, _>("clear_count") as u64,
                )
            })
            .collect())
    }

    async fn flag_active_sample_counts(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<HashMap<String, u64>> {
        let mut sql = String::from(
            r#"
            SELECT je.value AS flag_name, COUNT(*) AS active_count
            FROM samples AS s, json_each(s.active_flags) AS je
            WHERE s.ts_unix_ms >= ? AND s.ts_unix_ms <= ?
            "#,
        );
        if device_pci.is_some() {
            sql.push_str(" AND s.device_pci = ?");
        }
        sql.push_str(" GROUP BY je.value");

        let mut q = sqlx::query(&sql).bind(from_ms).bind(to_ms);
        if let Some(pci) = device_pci {
            q = q.bind(pci);
        }
        let rows = q.fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    r.get::<String, _>("flag_name"),
                    r.get::<i64, _>("active_count") as u64,
                )
            })
            .collect())
    }
}
