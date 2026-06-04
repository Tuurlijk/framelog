use std::fs;

use serde_json::json;

use crate::context::{snapshot_ok, snapshot_unavailable, sysfs, ContextDiffPolicy, ContextSource};
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

const ID: &str = "cpu";
const FREQ_THRESHOLD_MHZ: f64 = 50.0;

fn is_cpu_dir_name(name: &str) -> bool {
    name.strip_prefix("cpu")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

#[derive(Debug, Clone)]
struct CpuFreqSample {
    cpu: String,
    cur_khz: Option<u64>,
    max_khz: Option<u64>,
    min_khz: Option<u64>,
    governor: Option<String>,
}

pub struct CpuSource;

impl CpuSource {
    pub fn new() -> Self {
        Self
    }
}

fn online_cpu_dirs(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let cpus_root = root.join("devices/system/cpu");
    let entries = match fs::read_dir(&cpus_root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_cpu_dir_name(&name) {
            let online = sysfs::read_sysfs_i64(&entry.path().join("online")).unwrap_or(1);
            if online != 0 {
                dirs.push(entry.path());
            }
        }
    }
    dirs.sort();
    dirs
}

fn read_cpu_freq(dir: &std::path::Path) -> CpuFreqSample {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cpu0".into());
    let cpufreq = dir.join("cpufreq");
    CpuFreqSample {
        cpu: name,
        cur_khz: sysfs::read_sysfs_u64(&cpufreq.join("scaling_cur_freq")),
        max_khz: sysfs::read_sysfs_u64(&cpufreq.join("scaling_max_freq")),
        min_khz: sysfs::read_sysfs_u64(&cpufreq.join("scaling_min_freq")),
        governor: sysfs::read_sysfs_string(&cpufreq.join("scaling_governor")),
    }
}

fn khz_to_mhz(khz: u64) -> f64 {
    khz as f64 / 1000.0
}

impl ContextSource for CpuSource {
    fn id(&self) -> &'static str {
        ID
    }

    fn diff_policy(&self) -> ContextDiffPolicy {
        ContextDiffPolicy::Custom
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let root = sysfs::sysfs_root();
        let cpu_dirs = online_cpu_dirs(&root);
        if cpu_dirs.is_empty() {
            return snapshot_unavailable(ID, ts_unix_ms, "no online cpufreq sysfs nodes found");
        }

        let samples: Vec<CpuFreqSample> = cpu_dirs.iter().map(|p| read_cpu_freq(p)).collect();
        let cur_mhz: Vec<f64> = samples
            .iter()
            .filter_map(|s| s.cur_khz.map(khz_to_mhz))
            .collect();
        if cur_mhz.is_empty() {
            return snapshot_unavailable(
                ID,
                ts_unix_ms,
                "cpufreq present but no scaling_cur_freq readings",
            );
        }

        let min_mhz = cur_mhz.iter().copied().fold(f64::INFINITY, f64::min);
        let max_mhz = cur_mhz.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let avg_mhz = cur_mhz.iter().sum::<f64>() / cur_mhz.len() as f64;

        let max_cap_mhz = samples
            .iter()
            .filter_map(|s| s.max_khz.map(khz_to_mhz))
            .max_by(f64::total_cmp);
        let governor = samples
            .iter()
            .find_map(|s| s.governor.clone())
            .unwrap_or_default();

        let per_cpu: serde_json::Map<String, serde_json::Value> = samples
            .iter()
            .map(|s| {
                (
                    s.cpu.clone(),
                    json!({
                        "cur_khz": s.cur_khz,
                        "max_khz": s.max_khz,
                        "min_khz": s.min_khz,
                        "governor": s.governor,
                    }),
                )
            })
            .collect();

        let mut values = vec![
            ContextValue::num("cur_freq_min_mhz", min_mhz),
            ContextValue::num("cur_freq_avg_mhz", avg_mhz),
            ContextValue::num("cur_freq_max_mhz", max_mhz),
            ContextValue::num("online_cpus", cpu_dirs.len() as f64),
            ContextValue::str("governor", governor),
        ];
        if let Some(max_cap_mhz) = max_cap_mhz {
            values.push(ContextValue::num("scaling_max_freq_mhz", max_cap_mhz));
        }

        snapshot_ok(ID, ts_unix_ms, values, json!({ "cpus": per_cpu }))
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        let Some(previous) = previous else {
            return Vec::new();
        };
        if previous.health != "ok" || current.health != "ok" {
            return Vec::new();
        }

        let mut transitions = Vec::new();
        push_str_transition(
            previous,
            current,
            &mut transitions,
            "governor",
            "cpu.governor",
        );
        push_num_transition(
            previous,
            current,
            &mut transitions,
            "scaling_max_freq_mhz",
            "cpu.scaling_max_freq_mhz",
            FREQ_THRESHOLD_MHZ,
        );
        push_num_transition(
            previous,
            current,
            &mut transitions,
            "online_cpus",
            "cpu.online_cpus",
            0.5,
        );
        transitions
    }
}

fn push_str_transition(
    previous: &ContextSnapshot,
    current: &ContextSnapshot,
    transitions: &mut Vec<ContextTransition>,
    raw_key: &str,
    full_key: &str,
) {
    let old = context_str(previous, raw_key);
    let new = context_str(current, raw_key);
    if old != new {
        transitions.push(crate::context::value_transition(
            ID,
            current.ts_unix_ms,
            full_key,
            &json!(old),
            &json!(new),
        ));
    }
}

fn push_num_transition(
    previous: &ContextSnapshot,
    current: &ContextSnapshot,
    transitions: &mut Vec<ContextTransition>,
    raw_key: &str,
    full_key: &str,
    min_delta: f64,
) {
    let old = context_num(previous, raw_key);
    let new = context_num(current, raw_key);
    let changed = match (old, new) {
        (Some(old), Some(new)) => (old - new).abs() >= min_delta,
        _ => old != new,
    };
    if changed {
        transitions.push(crate::context::value_transition(
            ID,
            current.ts_unix_ms,
            full_key,
            &json!(old),
            &json!(new),
        ));
    }
}

fn context_num(snapshot: &ContextSnapshot, key: &str) -> Option<f64> {
    snapshot
        .values
        .iter()
        .find(|v| v.key == key)
        .and_then(|v| v.value_num)
}

fn context_str(snapshot: &ContextSnapshot, key: &str) -> Option<String> {
    snapshot
        .values
        .iter()
        .find(|v| v.key == key)
        .and_then(|v| v.value_str.clone())
}

/// Fraction of samples (0–100) where values lie in [low, high].
pub fn freq_band_pct_values(vals: &[f64], low_mhz: f64, high_mhz: f64) -> Option<f64> {
    if vals.is_empty() {
        return None;
    }
    let in_band = vals
        .iter()
        .filter(|v| (low_mhz..=high_mhz).contains(v))
        .count();
    Some((in_band as f64 / vals.len() as f64) * 100.0)
}

/// Fraction of context samples (0–100) where `cpu.cur_freq_min_mhz` lies in [low, high].
pub fn context_freq_band_pct(
    rows: &[crate::model::ContextValueRow],
    low_mhz: f64,
    high_mhz: f64,
) -> Option<f64> {
    let vals: Vec<f64> = rows
        .iter()
        .filter(|r| r.key == "cpu.cur_freq_min_mhz")
        .filter_map(|r| r.value_num)
        .collect();
    freq_band_pct_values(&vals, low_mhz, high_mhz)
}

pub fn cpu_freq_dashboard_warning(
    pct_545: Option<f64>,
    pct_1400: Option<f64>,
) -> Option<&'static str> {
    if pct_545.is_some_and(|p| p >= 50.0) {
        return Some(
            "CPU minimum frequency stayed near 544–545 MHz for most of this window. This may be the Framework BIOS 4.04 frequency-lock bug; see docs/framework-146-long-idle-capture.md.",
        );
    }
    if pct_1400.is_some_and(|p| p >= 50.0) {
        return Some(
            "CPU minimum frequency stayed near 1400 MHz for most of this window. This may be an alternate frequency-lock state on Framework AMD laptops.",
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_u64(path: &std::path::Path, value: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    fn write_str(path: &std::path::Path, value: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value).unwrap();
    }

    #[test]
    fn reads_aggregate_freq_from_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let cpu0 = dir.path().join("devices/system/cpu/cpu0");
        let cpufreq = cpu0.join("cpufreq");
        write_u64(&cpu0.join("online"), 1);
        write_u64(&cpufreq.join("scaling_cur_freq"), 545_000);
        write_u64(&cpufreq.join("scaling_max_freq"), 4_800_000);
        write_str(&cpufreq.join("scaling_governor"), "powersave");

        let cpu1 = dir.path().join("devices/system/cpu/cpu1");
        let cpufreq1 = cpu1.join("cpufreq");
        write_u64(&cpu1.join("online"), 1);
        write_u64(&cpufreq1.join("scaling_cur_freq"), 550_000);

        std::env::set_var("FRAMELOG_SYSFS_ROOT", dir.path());
        let mut src = CpuSource::new();
        let snap = src.sample(0);
        std::env::remove_var("FRAMELOG_SYSFS_ROOT");

        assert_eq!(snap.health, "ok");
        let min = snap
            .values
            .iter()
            .find(|v| v.key == "cur_freq_min_mhz")
            .and_then(|v| v.value_num)
            .unwrap();
        assert!((min - 545.0).abs() < 0.1);
    }

    #[test]
    fn freq_band_pct_detects_545() {
        let rows = vec![
            crate::model::ContextValueRow {
                snapshot_id: 1,
                ts_unix_ms: 0,
                source_id: "cpu".into(),
                key: "cpu.cur_freq_min_mhz".into(),
                value_num: Some(544.0),
                value_str: None,
            },
            crate::model::ContextValueRow {
                snapshot_id: 1,
                ts_unix_ms: 1000,
                source_id: "cpu".into(),
                key: "cpu.cur_freq_min_mhz".into(),
                value_num: Some(545.0),
                value_str: None,
            },
        ];
        let pct = context_freq_band_pct(&rows, 500.0, 620.0).unwrap();
        assert!((pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn omits_scaling_cap_when_max_freq_missing() {
        let dir = tempfile::tempdir().unwrap();
        let cpu0 = dir.path().join("devices/system/cpu/cpu0");
        let cpufreq = cpu0.join("cpufreq");
        write_u64(&cpu0.join("online"), 1);
        write_u64(&cpufreq.join("scaling_cur_freq"), 545_000);
        write_str(&cpufreq.join("scaling_governor"), "powersave");

        std::env::set_var("FRAMELOG_SYSFS_ROOT", dir.path());
        let mut src = CpuSource::new();
        let snap = src.sample(0);
        std::env::remove_var("FRAMELOG_SYSFS_ROOT");

        assert_eq!(snap.health, "ok");
        assert!(!snap.values.iter().any(|v| v.key == "scaling_max_freq_mhz"));
    }

    #[test]
    fn diff_ignores_regular_frequency_swings() {
        let mut old = snapshot_ok(
            ID,
            0,
            vec![
                ContextValue::num("cur_freq_min_mhz", 545.0),
                ContextValue::str("governor", "powersave"),
            ],
            json!({}),
        );
        old.source_id = ID.into();
        let mut new = snapshot_ok(
            ID,
            1,
            vec![
                ContextValue::num("cur_freq_min_mhz", 546.0),
                ContextValue::num("cur_freq_avg_mhz", 2800.0),
                ContextValue::num("cur_freq_max_mhz", 4800.0),
                ContextValue::str("governor", "powersave"),
            ],
            json!({}),
        );
        new.source_id = ID.into();

        let transitions = CpuSource::new().diff(Some(&old), &new);
        assert!(transitions.is_empty());
    }

    #[test]
    fn governor_change_emits_single_transition() {
        let old = snapshot_ok(
            ID,
            0,
            vec![
                ContextValue::num("cur_freq_min_mhz", 545.0),
                ContextValue::str("governor", "powersave"),
            ],
            json!({}),
        );
        let new = snapshot_ok(
            ID,
            1,
            vec![
                ContextValue::num("cur_freq_min_mhz", 546.0),
                ContextValue::str("governor", "performance"),
            ],
            json!({}),
        );

        let transitions = CpuSource::new().diff(Some(&old), &new);
        assert_eq!(
            transitions
                .iter()
                .filter(|t| t.key == "cpu.governor")
                .count(),
            1
        );
    }
}
