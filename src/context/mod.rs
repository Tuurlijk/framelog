mod battery;
mod display;
mod fake;
mod gpu_power;
pub mod pmf;
mod power;
mod power_profile;
mod sleep_wake;
pub mod sysfs;

use serde_json::{json, Value};
use std::collections::HashMap;

use crate::config::Config;
use crate::context::battery::BatterySource;
use crate::context::display::DisplaySource;
use crate::context::fake::{FakeContextSource, FakeGpuPowerSource, FakePmfSource};
use crate::context::gpu_power::GpuPowerSource;
use crate::context::pmf::PmfSource;
use crate::context::power::PowerSource;
use crate::context::power_profile::PowerProfileSource;
use crate::context::sleep_wake::SleepWakeSource;
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

pub trait ContextSource: Send + Sync {
    fn id(&self) -> &'static str;
    fn diff_policy(&self) -> ContextDiffPolicy {
        ContextDiffPolicy::Default
    }
    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot;
    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        default_diff(previous, current)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextDiffPolicy {
    Default,
    Custom,
    Threshold(&'static [ThresholdRule]),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThresholdRule {
    pub key: &'static str,
    pub min_delta: f64,
}

pub struct ContextRegistry {
    sources: Vec<Box<dyn ContextSource>>,
    source_index: HashMap<String, usize>,
    last_snapshots: HashMap<String, ContextSnapshot>,
}

impl ContextRegistry {
    pub fn new(config: &Config) -> Self {
        let sources: Vec<Box<dyn ContextSource>> = if config.fake {
            vec![
                Box::new(FakeContextSource::new()),
                Box::new(FakePmfSource::new()),
                Box::new(FakeGpuPowerSource::new()),
            ]
        } else {
            vec![
                Box::new(PowerSource::new()),
                Box::new(BatterySource::new()),
                Box::new(DisplaySource::new()),
                Box::new(PowerProfileSource::new()),
                Box::new(SleepWakeSource::new(config.interval_ms)),
                Box::new(PmfSource::new()),
                Box::new(GpuPowerSource::new()),
            ]
        };

        let source_index = sources
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id().to_string(), i))
            .collect();

        Self {
            sources,
            source_index,
            last_snapshots: HashMap::new(),
        }
    }

    pub fn sample_all(&mut self, ts_unix_ms: i64) -> Vec<ContextSnapshot> {
        self.sources
            .iter_mut()
            .map(|s| s.sample(ts_unix_ms))
            .collect()
    }

    pub fn transitions_for_tick(
        &mut self,
        snapshots: &[ContextSnapshot],
    ) -> Vec<ContextTransition> {
        let mut out = Vec::new();
        for snap in snapshots {
            let prev = self.last_snapshots.get(&snap.source_id);
            let transitions = if let Some(&idx) = self.source_index.get(&snap.source_id) {
                match self.sources[idx].diff_policy() {
                    ContextDiffPolicy::Default => default_diff(prev, snap),
                    ContextDiffPolicy::Custom => self.sources[idx].diff(prev, snap),
                    ContextDiffPolicy::Threshold(rules) => threshold_diff(prev, snap, rules),
                }
            } else {
                default_diff(prev, snap)
            };
            out.extend(transitions);
            self.last_snapshots
                .insert(snap.source_id.clone(), snap.clone());
        }
        out
    }
}

pub fn default_diff(
    previous: Option<&ContextSnapshot>,
    current: &ContextSnapshot,
) -> Vec<ContextTransition> {
    let mut transitions = Vec::new();
    for value in &current.values {
        let key = format!("{}.{}", current.source_id, value.key);
        let new_v = value.to_json();
        let old_v = previous
            .and_then(|p| p.values.iter().find(|v| v.key == value.key))
            .map(|v| v.to_json())
            .unwrap_or(Value::Null);
        if old_v != new_v {
            transitions.push(ContextTransition {
                id: 0,
                ts_unix_ms: current.ts_unix_ms,
                snapshot_id: 0,
                source_id: current.source_id.clone(),
                key,
                old_value: old_v,
                new_value: new_v,
            });
        }
    }
    transitions
}

pub fn threshold_diff(
    previous: Option<&ContextSnapshot>,
    current: &ContextSnapshot,
    rules: &[ThresholdRule],
) -> Vec<ContextTransition> {
    let Some(previous) = previous else {
        return Vec::new();
    };
    if previous.health != "ok" || current.health != "ok" {
        return Vec::new();
    }

    let mut transitions = Vec::new();
    for value in &current.values {
        let key = format!("{}.{}", current.source_id, value.key);
        let new_v = value.to_json();
        let old_v = previous
            .values
            .iter()
            .find(|v| v.key == value.key)
            .map(|v| v.to_json())
            .unwrap_or(Value::Null);

        let changed = match (num_value(previous, &value.key), value.value_num) {
            (Some(old), Some(new)) => rules
                .iter()
                .find(|rule| rule.key == value.key)
                .map(|rule| (old - new).abs() >= rule.min_delta)
                .unwrap_or(old_v != new_v),
            _ => old_v != new_v,
        };

        if changed {
            transitions.push(value_transition(
                &current.source_id,
                current.ts_unix_ms,
                &key,
                &old_v,
                &new_v,
            ));
        }
    }
    transitions
}

fn num_value(snap: &ContextSnapshot, key: &str) -> Option<f64> {
    snap.values
        .iter()
        .find(|v| v.key == key)
        .and_then(|v| v.value_num)
}

pub fn snapshot_ok(
    source_id: &str,
    ts_unix_ms: i64,
    values: Vec<ContextValue>,
    raw: Value,
) -> ContextSnapshot {
    ContextSnapshot {
        source_id: source_id.to_string(),
        ts_unix_ms,
        health: "ok".into(),
        values,
        raw_json: raw.to_string(),
        error_message: None,
    }
}

pub fn snapshot_error(source_id: &str, ts_unix_ms: i64, message: String) -> ContextSnapshot {
    ContextSnapshot {
        source_id: source_id.to_string(),
        ts_unix_ms,
        health: "error".into(),
        values: Vec::new(),
        raw_json: json!({ "error": message }).to_string(),
        error_message: Some(message),
    }
}

pub fn snapshot_unavailable(source_id: &str, ts_unix_ms: i64, reason: &str) -> ContextSnapshot {
    ContextSnapshot {
        source_id: source_id.to_string(),
        ts_unix_ms,
        health: "unavailable".into(),
        values: Vec::new(),
        raw_json: json!({ "reason": reason }).to_string(),
        error_message: Some(reason.to_string()),
    }
}

pub fn bool_transition(
    source_id: &str,
    ts_unix_ms: i64,
    key: &str,
    old: bool,
    new: bool,
) -> ContextTransition {
    ContextTransition {
        id: 0,
        ts_unix_ms,
        snapshot_id: 0,
        source_id: source_id.to_string(),
        key: key.to_string(),
        old_value: json!(old),
        new_value: json!(new),
    }
}

pub fn value_transition(
    source_id: &str,
    ts_unix_ms: i64,
    key: &str,
    old: &Value,
    new: &Value,
) -> ContextTransition {
    ContextTransition {
        id: 0,
        ts_unix_ms,
        snapshot_id: 0,
        source_id: source_id.to_string(),
        key: key.to_string(),
        old_value: old.clone(),
        new_value: new.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_diff_ignores_small_numeric_changes() {
        let previous = snapshot_ok(
            "pmf",
            1,
            vec![ContextValue::num("spl_mw", 35_000.0)],
            json!({}),
        );
        let current = snapshot_ok(
            "pmf",
            2,
            vec![ContextValue::num("spl_mw", 35_500.0)],
            json!({}),
        );
        let transitions = threshold_diff(
            Some(&previous),
            &current,
            &[ThresholdRule {
                key: "spl_mw",
                min_delta: 1000.0,
            }],
        );
        assert!(transitions.is_empty());
    }

    #[test]
    fn threshold_diff_emits_large_numeric_changes() {
        let previous = snapshot_ok(
            "pmf",
            1,
            vec![ContextValue::num("spl_mw", 35_000.0)],
            json!({}),
        );
        let current = snapshot_ok(
            "pmf",
            2,
            vec![ContextValue::num("spl_mw", 36_500.0)],
            json!({}),
        );
        let transitions = threshold_diff(
            Some(&previous),
            &current,
            &[ThresholdRule {
                key: "spl_mw",
                min_delta: 1000.0,
            }],
        );
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].key, "pmf.spl_mw");
    }
}
