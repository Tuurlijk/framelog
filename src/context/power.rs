use std::fs;

use serde_json::json;

use crate::context::{
    bool_transition, snapshot_error, snapshot_ok, sysfs, value_transition, ContextDiffPolicy,
    ContextSource,
};
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

const ID: &str = "power";

pub struct PowerSource;

impl PowerSource {
    pub fn new() -> Self {
        Self
    }
}

fn is_ac_type(t: &str) -> bool {
    matches!(t, "Mains" | "USB" | "USB_C" | "USB_PD" | "Wireless" | "UPS")
}

fn is_input_type(t: &str) -> bool {
    is_ac_type(t) || t.contains("UCSI") || t.starts_with("USB")
}

pub fn adapter_watts_from_design(
    voltage_max_uv: Option<u64>,
    current_max_ua: Option<u64>,
) -> Option<f64> {
    match (voltage_max_uv, current_max_ua) {
        (Some(v), Some(c)) if v > 0 && c > 0 => Some(sysfs::micro_watts_from_vi(v, c)),
        _ => None,
    }
}

impl ContextSource for PowerSource {
    fn id(&self) -> &'static str {
        ID
    }

    fn diff_policy(&self) -> ContextDiffPolicy {
        ContextDiffPolicy::Custom
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let root = sysfs::sysfs_root().join("class/power_supply");
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) => {
                return snapshot_error(ID, ts_unix_ms, format!("read {root:?}: {e}"));
            }
        };

        let mut supplies = Vec::new();
        let mut ac_connected = false;
        let mut input_watts = 0.0f64;
        let mut adapter_watts_reported = None;

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let base = entry.path();
            let supply_type = sysfs::read_sysfs_string(&base.join("type")).unwrap_or_default();
            let online = sysfs::read_sysfs_i64(&base.join("online")).unwrap_or(0);
            let status = sysfs::read_sysfs_string(&base.join("status"));
            let usb_type = sysfs::read_sysfs_string(&base.join("usb_type"));
            let manufacturer = sysfs::read_sysfs_string(&base.join("manufacturer"));
            let model_name = sysfs::read_sysfs_string(&base.join("model_name"));
            let scope = sysfs::read_sysfs_string(&base.join("scope"));
            let voltage_uv = sysfs::read_sysfs_u64(&base.join("voltage_now"));
            let voltage_max_uv = sysfs::read_sysfs_u64(&base.join("voltage_max_design"));
            let current_ua = sysfs::read_sysfs_u64(&base.join("current_now"));
            let current_max_ua = sysfs::read_sysfs_u64(&base.join("current_max"));
            let power_uw = sysfs::read_sysfs_i64(&base.join("power_now"));

            let watts = match (voltage_uv, current_ua) {
                (Some(v), Some(c)) => Some(sysfs::micro_watts_from_vi(v, c)),
                _ => power_uw.map(sysfs::micro_watts_to_watts),
            };

            if is_ac_type(&supply_type) && online != 0 {
                ac_connected = true;
                if let Some(design_watts) =
                    adapter_watts_from_design(voltage_max_uv, current_max_ua)
                {
                    adapter_watts_reported = Some(
                        adapter_watts_reported
                            .map(|w: f64| w.max(design_watts))
                            .unwrap_or(design_watts),
                    );
                }
            }
            if is_input_type(&supply_type) && online != 0 {
                if let Some(w) = watts {
                    input_watts = input_watts.max(w);
                }
            }

            supplies.push(json!({
                "name": name,
                "type": supply_type,
                "online": online,
                "status": status,
                "usb_type": usb_type,
                "manufacturer": manufacturer,
                "model_name": model_name,
                "scope": scope,
                "voltage_uv": voltage_uv,
                "voltage_max_design_uv": voltage_max_uv,
                "current_ua": current_ua,
                "current_max_ua": current_max_ua,
                "power_uw": power_uw,
                "watts": watts,
            }));
        }

        let mut values = vec![
            ContextValue::num("ac_connected", if ac_connected { 1.0 } else { 0.0 }),
            ContextValue::num("input_watts", input_watts),
        ];
        if let Some(w) = adapter_watts_reported {
            values.push(ContextValue::num("adapter_watts_reported", w));
        }

        snapshot_ok(
            ID,
            ts_unix_ms,
            values,
            json!({
                "supplies": supplies,
                "ac_connected": ac_connected,
                "input_watts": input_watts,
                "adapter_watts_reported": adapter_watts_reported,
            }),
        )
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        let mut transitions = Vec::new();

        let old_ac = previous
            .and_then(|p| p.values.iter().find(|v| v.key == "ac_connected"))
            .and_then(|v| v.value_num)
            .map(|n| n >= 0.5)
            .unwrap_or(false);
        let new_ac = current
            .values
            .iter()
            .find(|v| v.key == "ac_connected")
            .and_then(|v| v.value_num)
            .map(|n| n >= 0.5)
            .unwrap_or(false);
        if old_ac != new_ac {
            transitions.push(bool_transition(
                ID,
                current.ts_unix_ms,
                "power.ac_connected",
                old_ac,
                new_ac,
            ));
        }

        let old_w = previous
            .and_then(|p| p.values.iter().find(|v| v.key == "input_watts"))
            .and_then(|v| v.value_num)
            .unwrap_or(0.0);
        let new_w = current
            .values
            .iter()
            .find(|v| v.key == "input_watts")
            .and_then(|v| v.value_num)
            .unwrap_or(0.0);
        if (old_w - new_w).abs() >= 5.0 {
            transitions.push(value_transition(
                ID,
                current.ts_unix_ms,
                "power.input_watts",
                &json!(old_w),
                &json!(new_w),
            ));
        }

        let old_adapter = previous
            .and_then(|p| p.values.iter().find(|v| v.key == "adapter_watts_reported"))
            .and_then(|v| v.value_num);
        let new_adapter = current
            .values
            .iter()
            .find(|v| v.key == "adapter_watts_reported")
            .and_then(|v| v.value_num);
        if let (Some(old), Some(new)) = (old_adapter, new_adapter) {
            if (old - new).abs() >= 5.0 {
                transitions.push(value_transition(
                    ID,
                    current.ts_unix_ms,
                    "power.adapter_watts_reported",
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
    use std::path::Path;

    fn write_string(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn adapter_watts_from_design_values() {
        let w = adapter_watts_from_design(Some(20_000_000), Some(9_000_000)).unwrap();
        assert!((w - 180.0).abs() < 1.0);
    }

    #[test]
    fn sysfs_fixture_includes_reported_adapter_watts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("class/power_supply/ACAD");
        write_string(&root.join("type"), "Mains\n");
        write_string(&root.join("online"), "1\n");
        write_string(&root.join("voltage_max_design"), "20000000\n");
        write_string(&root.join("current_max"), "9000000\n");
        write_string(&root.join("voltage_now"), "20000000\n");
        write_string(&root.join("current_now"), "8000000\n");

        std::env::set_var("FRAMELOG_SYSFS_ROOT", dir.path());
        let mut source = PowerSource::new();
        let snap = source.sample(1);
        std::env::remove_var("FRAMELOG_SYSFS_ROOT");

        assert_eq!(snap.health, "ok");
        let reported = snap
            .values
            .iter()
            .find(|v| v.key == "adapter_watts_reported")
            .and_then(|v| v.value_num)
            .unwrap();
        assert!((reported - 180.0).abs() < 1.0);

        let raw: serde_json::Value = serde_json::from_str(&snap.raw_json).unwrap();
        let supply = &raw["supplies"][0];
        assert!(supply.get("serial_number").is_none());
    }
}
