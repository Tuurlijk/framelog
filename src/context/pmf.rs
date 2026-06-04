use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::context::{
    snapshot_ok, snapshot_unavailable, value_transition, ContextDiffPolicy, ContextSource,
    ThresholdRule,
};
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

const ID: &str = "pmf";
const LIMITS_FILE: &str = "amd_pmf/current_power_limits";
const POWER_DELTA_MW: f64 = 1000.0;
const TEMP_DELTA_C: f64 = 1.0;
const PMF_THRESHOLDS: &[ThresholdRule] = &[
    ThresholdRule {
        key: "spl_mw",
        min_delta: POWER_DELTA_MW,
    },
    ThresholdRule {
        key: "fppt_mw",
        min_delta: POWER_DELTA_MW,
    },
    ThresholdRule {
        key: "sppt_mw",
        min_delta: POWER_DELTA_MW,
    },
    ThresholdRule {
        key: "sppt_apu_only_mw",
        min_delta: POWER_DELTA_MW,
    },
    ThresholdRule {
        key: "stt_min_c",
        min_delta: TEMP_DELTA_C,
    },
    ThresholdRule {
        key: "stt_apu_c",
        min_delta: TEMP_DELTA_C,
    },
    ThresholdRule {
        key: "stt_hs2_c",
        min_delta: TEMP_DELTA_C,
    },
];

pub struct PmfSource {
    limits_path: PathBuf,
}

impl PmfSource {
    pub fn new() -> Self {
        Self {
            limits_path: debugfs_root().join(LIMITS_FILE),
        }
    }

    #[cfg(test)]
    pub fn with_limits_path(path: PathBuf) -> Self {
        Self { limits_path: path }
    }
}

pub fn debugfs_root() -> PathBuf {
    std::env::var("FRAMELOG_DEBUGFS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/sys/kernel/debug"))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedPmfLimits {
    pub spl_mw: Option<u32>,
    pub fppt_mw: Option<u32>,
    pub sppt_mw: Option<u32>,
    pub sppt_apu_only_mw: Option<u32>,
    pub stt_min_c: Option<u32>,
    pub stt_apu_c: Option<u32>,
    pub stt_hs2_c: Option<u32>,
}

pub fn parse_current_power_limits(line: &str) -> Option<ParsedPmfLimits> {
    let mut out = ParsedPmfLimits::default();
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        let Some((key, value_str)) = token.split_once(':') else {
            i += 1;
            continue;
        };
        let value = if value_str.is_empty() {
            i += 1;
            if i >= tokens.len() {
                break;
            }
            let Ok(v) = tokens[i].parse::<u32>() else {
                continue;
            };
            v
        } else if let Ok(v) = value_str.parse::<u32>() {
            v
        } else {
            i += 1;
            continue;
        };
        match key {
            "spl" => out.spl_mw = Some(value),
            "fppt" => out.fppt_mw = Some(value),
            "sppt" => out.sppt_mw = Some(value),
            "sppt_apu_only" => out.sppt_apu_only_mw = Some(value),
            "stt_min" => out.stt_min_c = Some(value),
            "stt[APU]" => out.stt_apu_c = Some(value),
            "stt[HS2]" => out.stt_hs2_c = Some(value),
            _ => {}
        }
        i += 1;
    }
    if out.spl_mw.is_some() || out.sppt_mw.is_some() {
        Some(out)
    } else {
        None
    }
}

fn read_limits(path: &Path) -> Result<(ParsedPmfLimits, String), String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
    let line = raw.lines().next().unwrap_or("").trim().to_string();
    let parsed = parse_current_power_limits(&line)
        .ok_or_else(|| format!("unrecognized PMF limits line: {line}"))?;
    Ok((parsed, line))
}

fn limits_to_values(limits: &ParsedPmfLimits) -> Vec<ContextValue> {
    let mut values = Vec::new();
    if let Some(v) = limits.spl_mw {
        values.push(ContextValue::num("spl_mw", v as f64));
    }
    if let Some(v) = limits.fppt_mw {
        values.push(ContextValue::num("fppt_mw", v as f64));
    }
    if let Some(v) = limits.sppt_mw {
        values.push(ContextValue::num("sppt_mw", v as f64));
    }
    if let Some(v) = limits.sppt_apu_only_mw {
        values.push(ContextValue::num("sppt_apu_only_mw", v as f64));
    }
    if let Some(v) = limits.stt_min_c {
        values.push(ContextValue::num("stt_min_c", v as f64));
    }
    if let Some(v) = limits.stt_apu_c {
        values.push(ContextValue::num("stt_apu_c", v as f64));
    }
    if let Some(v) = limits.stt_hs2_c {
        values.push(ContextValue::num("stt_hs2_c", v as f64));
    }
    values
}

fn num_value(snap: &ContextSnapshot, key: &str) -> Option<f64> {
    snap.values
        .iter()
        .find(|v| v.key == key)
        .and_then(|v| v.value_num)
}

fn is_power_key(key: &str) -> bool {
    key.ends_with("_mw")
}

fn pmf_value_changed(key: &str, old: f64, new: f64) -> bool {
    if is_power_key(key) {
        (old - new).abs() >= POWER_DELTA_MW
    } else {
        (old - new).abs() >= TEMP_DELTA_C
    }
}

impl ContextSource for PmfSource {
    fn id(&self) -> &'static str {
        ID
    }

    fn diff_policy(&self) -> ContextDiffPolicy {
        ContextDiffPolicy::Threshold(PMF_THRESHOLDS)
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        match read_limits(&self.limits_path) {
            Ok((limits, raw_line)) => {
                let values = limits_to_values(&limits);
                snapshot_ok(
                    ID,
                    ts_unix_ms,
                    values,
                    json!({
                        "path": self.limits_path.display().to_string(),
                        "raw_line": raw_line,
                        "spl_mw": limits.spl_mw,
                        "fppt_mw": limits.fppt_mw,
                        "sppt_mw": limits.sppt_mw,
                        "sppt_apu_only_mw": limits.sppt_apu_only_mw,
                        "stt_min_c": limits.stt_min_c,
                        "stt_apu_c": limits.stt_apu_c,
                        "stt_hs2_c": limits.stt_hs2_c,
                    }),
                )
            }
            Err(e) => snapshot_unavailable(ID, ts_unix_ms, &e),
        }
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        let Some(prev) = previous else {
            return Vec::new();
        };
        if prev.health != "ok" || current.health != "ok" {
            return Vec::new();
        }

        let mut transitions = Vec::new();
        for value in &current.values {
            let key = format!("{ID}.{}", value.key);
            let new_v = value.to_json();
            let old_v = prev
                .values
                .iter()
                .find(|v| v.key == value.key)
                .map(|v| v.to_json())
                .unwrap_or(json!(null));

            let changed = match (num_value(prev, &value.key), value.value_num) {
                (Some(old), Some(new)) => pmf_value_changed(&value.key, old, new),
                _ => old_v != new_v,
            };

            if changed {
                transitions.push(value_transition(
                    ID,
                    current.ts_unix_ms,
                    &key,
                    &old_v,
                    &new_v,
                ));
            }
        }
        transitions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_kernel_current_power_limits_line() {
        let line =
            "spl:45000 fppt:65000 sppt:54000 sppt_apu_only:0 stt_min:0 stt[APU]:45 stt[HS2]: 58";
        let parsed = parse_current_power_limits(line).unwrap();
        assert_eq!(parsed.spl_mw, Some(45_000));
        assert_eq!(parsed.fppt_mw, Some(65_000));
        assert_eq!(parsed.sppt_mw, Some(54_000));
        assert_eq!(parsed.stt_hs2_c, Some(58));
    }

    #[test]
    fn pmf_power_transition_requires_one_watt() {
        assert!(!pmf_value_changed("spl_mw", 45_000.0, 45_500.0));
        assert!(pmf_value_changed("spl_mw", 45_000.0, 46_500.0));
        assert!(!pmf_value_changed("stt_hs2_c", 58.0, 58.5));
        assert!(pmf_value_changed("stt_hs2_c", 58.0, 60.0));
    }

    #[test]
    fn missing_debugfs_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        let mut source = PmfSource::with_limits_path(path);
        let snap = source.sample(1);
        assert_eq!(snap.health, "unavailable");
    }

    #[test]
    fn reads_fixture_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("current_power_limits");
        let mut file = fs::File::create(&path).unwrap();
        writeln!(
            file,
            "spl:35000 fppt:45000 sppt:40000 sppt_apu_only:0 stt_min:0 stt[APU]:40 stt[HS2]: 0"
        )
        .unwrap();
        let mut source = PmfSource::with_limits_path(path);
        let snap = source.sample(1);
        assert_eq!(snap.health, "ok");
        assert_eq!(num_value(&snap, "spl_mw"), Some(35_000.0));
    }
}
