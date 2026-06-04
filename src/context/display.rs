use std::fs;

use serde_json::json;

use crate::context::{
    bool_transition, snapshot_error, snapshot_ok, ContextDiffPolicy, ContextSource,
};
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

const ID: &str = "display";

pub struct DisplaySource;

impl DisplaySource {
    pub fn new() -> Self {
        Self
    }
}

pub fn is_internal_connector(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.contains("EDP") || upper.contains("LVDS") || upper.contains("WRITEBACK")
}

pub fn is_external_connector(name: &str, status: &str) -> bool {
    status == "connected" && !is_internal_connector(name)
}

impl ContextSource for DisplaySource {
    fn id(&self) -> &'static str {
        ID
    }

    fn diff_policy(&self) -> ContextDiffPolicy {
        ContextDiffPolicy::Custom
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let root = crate::context::sysfs::sysfs_root().join("class/drm");
        let entries = match fs::read_dir(&root) {
            Ok(e) => e,
            Err(e) => return snapshot_error(ID, ts_unix_ms, format!("read drm: {e}")),
        };

        let mut connectors = Vec::new();
        let mut external_count = 0u32;

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.contains('-') {
                continue;
            }
            let status_path = entry.path().join("status");
            let status = crate::context::sysfs::read_sysfs_string(&status_path)
                .unwrap_or_else(|| "unknown".into());
            let enabled = crate::context::sysfs::read_sysfs_string(&entry.path().join("enabled"));
            let external = is_external_connector(&name, &status);
            if external {
                external_count += 1;
            }
            connectors.push(json!({
                "name": name,
                "status": status,
                "enabled": enabled,
                "external": external,
            }));
        }

        let values = vec![
            ContextValue::num("external_count", external_count as f64),
            ContextValue::num(
                "external_connected",
                if external_count > 0 { 1.0 } else { 0.0 },
            ),
        ];

        snapshot_ok(
            ID,
            ts_unix_ms,
            values,
            json!({ "connectors": connectors, "external_count": external_count }),
        )
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        let old = previous
            .and_then(|p| p.values.iter().find(|v| v.key == "external_connected"))
            .and_then(|v| v.value_num)
            .map(|n| n >= 0.5)
            .unwrap_or(false);
        let new = current
            .values
            .iter()
            .find(|v| v.key == "external_connected")
            .and_then(|v| v.value_num)
            .map(|n| n >= 0.5)
            .unwrap_or(false);
        if old != new {
            return vec![bool_transition(
                ID,
                current.ts_unix_ms,
                "display.external_connected",
                old,
                new,
            )];
        }

        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_detection() {
        assert!(!is_external_connector("card0-eDP-1", "connected"));
        assert!(is_external_connector("card1-DP-1", "connected"));
        assert!(!is_external_connector("card1-DP-1", "disconnected"));
    }
}
