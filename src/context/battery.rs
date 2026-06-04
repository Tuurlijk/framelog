use std::fs;

use serde_json::json;

use crate::context::{
    snapshot_error, snapshot_ok, value_transition, ContextDiffPolicy, ContextSource,
};
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

const ID: &str = "battery";

pub struct BatterySource;

impl BatterySource {
    pub fn new() -> Self {
        Self
    }
}

pub fn normalize_battery_status(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "charging" => "charging".into(),
        "discharging" => "discharging".into(),
        "full" => "full".into(),
        "not charging" | "not-charging" | "not_charging" => "not-charging".into(),
        _ => "unknown".into(),
    }
}

pub fn battery_percent(
    capacity: Option<i64>,
    energy_now: Option<i64>,
    energy_full: Option<i64>,
    charge_now: Option<i64>,
    charge_full: Option<i64>,
) -> Option<f64> {
    if let Some(c) = capacity {
        if (0..=100).contains(&c) {
            return Some(c as f64);
        }
    }
    if let (Some(now), Some(full)) = (energy_now, energy_full) {
        if full > 0 {
            return Some((now as f64 / full as f64) * 100.0);
        }
    }
    if let (Some(now), Some(full)) = (charge_now, charge_full) {
        if full > 0 {
            return Some((now as f64 / full as f64) * 100.0);
        }
    }
    None
}

impl ContextSource for BatterySource {
    fn id(&self) -> &'static str {
        ID
    }

    fn diff_policy(&self) -> ContextDiffPolicy {
        ContextDiffPolicy::Custom
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let root = crate::context::sysfs::sysfs_root().join("class/power_supply");
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) => return snapshot_error(ID, ts_unix_ms, format!("read power_supply: {e}")),
        };

        let mut batteries = Vec::new();
        let mut primary_state = "unknown".to_string();
        let mut primary_percent = None;
        let mut primary_power_w = None;

        for entry in entries.flatten() {
            let base = entry.path();
            let supply_type =
                crate::context::sysfs::read_sysfs_string(&base.join("type")).unwrap_or_default();
            if supply_type != "Battery" {
                continue;
            }

            let name = entry.file_name().to_string_lossy().into_owned();
            let status = crate::context::sysfs::read_sysfs_string(&base.join("status"))
                .map(|s| normalize_battery_status(&s))
                .unwrap_or_else(|| "unknown".into());
            let capacity = crate::context::sysfs::read_sysfs_i64(&base.join("capacity"));
            let energy_now = crate::context::sysfs::read_sysfs_i64(&base.join("energy_now"));
            let energy_full = crate::context::sysfs::read_sysfs_i64(&base.join("energy_full"));
            let charge_now = crate::context::sysfs::read_sysfs_i64(&base.join("charge_now"));
            let charge_full = crate::context::sysfs::read_sysfs_i64(&base.join("charge_full"));
            let power_uw = crate::context::sysfs::read_sysfs_i64(&base.join("power_now"));
            let percent =
                battery_percent(capacity, energy_now, energy_full, charge_now, charge_full);
            let power_w = power_uw.map(crate::context::sysfs::micro_watts_to_watts);

            if primary_percent.is_none() {
                primary_state = status.clone();
                primary_percent = percent;
                primary_power_w = power_w;
            }

            batteries.push(json!({
                "name": name,
                "status": status,
                "percent": percent,
                "power_watts": power_w,
                "capacity": capacity,
                "energy_now": energy_now,
                "energy_full": energy_full,
            }));
        }

        let mut values = vec![ContextValue::str("state", primary_state.clone())];
        if let Some(p) = primary_percent {
            values.push(ContextValue::num("percent", p));
        }
        if let Some(w) = primary_power_w {
            values.push(ContextValue::num("power_watts", w));
        }

        snapshot_ok(ID, ts_unix_ms, values, json!({ "batteries": batteries }))
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        let mut transitions = Vec::new();

        let old_state = previous
            .and_then(|p| p.values.iter().find(|v| v.key == "state"))
            .and_then(|v| v.value_str.clone())
            .unwrap_or_else(|| "unknown".into());
        let new_state = current
            .values
            .iter()
            .find(|v| v.key == "state")
            .and_then(|v| v.value_str.clone())
            .unwrap_or_else(|| "unknown".into());
        if old_state != new_state {
            transitions.push(value_transition(
                ID,
                current.ts_unix_ms,
                "battery.state",
                &json!(old_state),
                &json!(new_state),
            ));
        }

        let old_pct = previous
            .and_then(|p| p.values.iter().find(|v| v.key == "percent"))
            .and_then(|v| v.value_num);
        let new_pct = current
            .values
            .iter()
            .find(|v| v.key == "percent")
            .and_then(|v| v.value_num);
        if let (Some(old), Some(new)) = (old_pct, new_pct) {
            if (old - new).abs() >= 1.0 {
                transitions.push(value_transition(
                    ID,
                    current.ts_unix_ms,
                    "battery.percent",
                    &json!(old),
                    &json!(new),
                ));
            }
        }

        transitions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_normalization() {
        assert_eq!(normalize_battery_status("Charging"), "charging");
        assert_eq!(normalize_battery_status("Not charging"), "not-charging");
    }

    #[test]
    fn percent_from_energy() {
        let p = battery_percent(None, Some(30_000_000), Some(60_000_000), None, None);
        assert!((p.unwrap() - 50.0).abs() < 0.01);
    }
}
