use serde_json::json;

use crate::context::{
    bool_transition, snapshot_ok, snapshot_unavailable, value_transition, ContextDiffPolicy,
    ContextSource,
};
use crate::hardware::drm;
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

const ID: &str = "gpu_power";

pub struct GpuPowerSource;

impl GpuPowerSource {
    pub fn new() -> Self {
        Self
    }
}

fn runtime_is_suspended(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "suspended" | "suspending"
    )
}

impl ContextSource for GpuPowerSource {
    fn id(&self) -> &'static str {
        ID
    }

    fn diff_policy(&self) -> ContextDiffPolicy {
        ContextDiffPolicy::Custom
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let root = drm::sysfs_drm_root();
        let cards = match drm::scan_cards(&root) {
            Ok(c) => c,
            Err(e) => return snapshot_unavailable(ID, ts_unix_ms, &e),
        };

        if cards.is_empty() {
            return snapshot_unavailable(ID, ts_unix_ms, "no DRM card devices found");
        }

        let dgpu = drm::pick_dgpu(&cards);
        let mut values = Vec::new();

        if let Some(card) = dgpu {
            if let Some(status) = &card.runtime_status {
                values.push(ContextValue::str("dgpu_runtime_status", status.clone()));
                values.push(ContextValue::num(
                    "dgpu_runtime_suspended",
                    if runtime_is_suspended(status) {
                        1.0
                    } else {
                        0.0
                    },
                ));
            }
            if let Some(allowed) = card.d3cold_allowed {
                values.push(ContextValue::num(
                    "dgpu_d3cold_allowed",
                    if allowed { 1.0 } else { 0.0 },
                ));
            }
            if let Some(ms) = card.runtime_active_ms {
                values.push(ContextValue::num("dgpu_runtime_active_ms", ms as f64));
            }
            if let Some(ms) = card.runtime_suspended_ms {
                values.push(ContextValue::num("dgpu_runtime_suspended_ms", ms as f64));
            }
        }

        let card_json: Vec<_> = cards
            .iter()
            .map(|c| {
                json!({
                    "card": c.card,
                    "pci": c.pci,
                    "vendor_id": c.vendor_id,
                    "device_id": c.device_id,
                    "driver": c.driver,
                    "runtime_status": c.runtime_status,
                    "runtime_active_ms": c.runtime_active_ms,
                    "runtime_suspended_ms": c.runtime_suspended_ms,
                    "control": c.control,
                    "d3cold_allowed": c.d3cold_allowed,
                    "power_state": c.power_state,
                    "likely_dgpu": c.likely_dgpu(),
                })
            })
            .collect();

        snapshot_ok(
            ID,
            ts_unix_ms,
            values,
            json!({ "cards": card_json, "dgpu_card": dgpu.map(|c| c.card.clone()) }),
        )
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

        // `runtime_active_time` / `runtime_suspended_time` are monotonic kernel counters that
        // change every sample while the device is in that state. Keep them in snapshots for
        // charts, but only emit transitions for discrete power-state changes.
        let mut transitions = Vec::new();

        let old_d3cold = previous
            .values
            .iter()
            .find(|v| v.key == "dgpu_d3cold_allowed")
            .and_then(|v| v.value_num)
            .map(|n| n >= 0.5)
            .unwrap_or(false);
        let new_d3cold = current
            .values
            .iter()
            .find(|v| v.key == "dgpu_d3cold_allowed")
            .and_then(|v| v.value_num)
            .map(|n| n >= 0.5)
            .unwrap_or(false);
        if old_d3cold != new_d3cold {
            transitions.push(bool_transition(
                ID,
                current.ts_unix_ms,
                "gpu_power.dgpu_d3cold_allowed",
                old_d3cold,
                new_d3cold,
            ));
        }

        let old_suspended = previous
            .values
            .iter()
            .find(|v| v.key == "dgpu_runtime_suspended")
            .and_then(|v| v.value_num)
            .map(|n| n >= 0.5)
            .unwrap_or(false);
        let new_suspended = current
            .values
            .iter()
            .find(|v| v.key == "dgpu_runtime_suspended")
            .and_then(|v| v.value_num)
            .map(|n| n >= 0.5)
            .unwrap_or(false);
        if old_suspended != new_suspended {
            transitions.push(bool_transition(
                ID,
                current.ts_unix_ms,
                "gpu_power.dgpu_runtime_suspended",
                old_suspended,
                new_suspended,
            ));
        }

        let old_status = previous
            .values
            .iter()
            .find(|v| v.key == "dgpu_runtime_status")
            .and_then(|v| v.value_str.clone())
            .unwrap_or_default();
        let new_status = current
            .values
            .iter()
            .find(|v| v.key == "dgpu_runtime_status")
            .and_then(|v| v.value_str.clone())
            .unwrap_or_default();
        if !new_status.is_empty() && old_status != new_status {
            transitions.push(value_transition(
                ID,
                current.ts_unix_ms,
                "gpu_power.dgpu_runtime_status",
                &json!(old_status),
                &json!(new_status),
            ));
        }

        transitions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_counter_changes_do_not_transition() {
        let source = GpuPowerSource::new();
        let prev = ContextSnapshot {
            source_id: ID.into(),
            ts_unix_ms: 1,
            health: "ok".into(),
            values: vec![
                ContextValue::str("dgpu_runtime_status", "active"),
                ContextValue::num("dgpu_runtime_suspended", 0.0),
                ContextValue::num("dgpu_runtime_active_ms", 1_000.0),
                ContextValue::num("dgpu_runtime_suspended_ms", 5_000.0),
            ],
            raw_json: "{}".into(),
            error_message: None,
        };
        let current = ContextSnapshot {
            source_id: ID.into(),
            ts_unix_ms: 2,
            health: "ok".into(),
            values: vec![
                ContextValue::str("dgpu_runtime_status", "active"),
                ContextValue::num("dgpu_runtime_suspended", 0.0),
                ContextValue::num("dgpu_runtime_active_ms", 2_000.0),
                ContextValue::num("dgpu_runtime_suspended_ms", 5_000.0),
            ],
            raw_json: "{}".into(),
            error_message: None,
        };
        let transitions = source.diff(Some(&prev), &current);
        assert!(transitions.is_empty());
    }

    #[test]
    fn runtime_suspended_transition() {
        let source = GpuPowerSource::new();
        let prev = ContextSnapshot {
            source_id: ID.into(),
            ts_unix_ms: 1,
            health: "ok".into(),
            values: vec![
                ContextValue::str("dgpu_runtime_status", "active"),
                ContextValue::num("dgpu_runtime_suspended", 0.0),
            ],
            raw_json: "{}".into(),
            error_message: None,
        };
        let current = ContextSnapshot {
            source_id: ID.into(),
            ts_unix_ms: 2,
            health: "ok".into(),
            values: vec![
                ContextValue::str("dgpu_runtime_status", "suspended"),
                ContextValue::num("dgpu_runtime_suspended", 1.0),
            ],
            raw_json: "{}".into(),
            error_message: None,
        };
        let transitions = source.diff(Some(&prev), &current);
        assert!(transitions
            .iter()
            .any(|t| t.key == "gpu_power.dgpu_runtime_suspended"));
    }
}
