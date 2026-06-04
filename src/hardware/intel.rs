use std::fs;
use std::path::Path;

use libamdgpu_top::AMDGPU::GpuMetrics;

use crate::collector::{valid_power_mw_u32, Collector};
use crate::error::Result;
use crate::hardware::drm;
use crate::model::MetricSample;

const HWMON_ROOT: &str = "/sys/class/hwmon";
const RAPL_ROOT: &str = "/sys/class/powercap";

pub struct IntelCollector;

impl IntelCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Collector for IntelCollector {
    fn sample(&self) -> Result<Vec<MetricSample>> {
        let ts_unix_ms = crate::util::current_ts_ms();
        let package_power_mw = hwmon_package_power_mw().or_else(rapl_constraint_mw);
        let temperature_core_max = hwmon_max_core_temp_c();

        let (device_pci, device_name) = primary_intel_drm_identity();

        let package_power_source = if hwmon_package_power_mw().is_some() {
            Some("hwmon_power1_input")
        } else if rapl_constraint_mw().is_some() {
            Some("rapl_constraint_power_limit")
        } else {
            None
        };
        let extra = serde_json::json!({
            "collector": "intel",
            "package_power_source": package_power_source,
            "intel_rapl_present": rapl_sysfs_present(),
        });

        Ok(vec![MetricSample {
            ts_unix_ms,
            device_pci,
            device_name,
            throttle_status_raw: None,
            indep_throttle_status: 0,
            active_flags: Vec::new(),
            apu_power_mw: package_power_mw,
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max,
            extra_json: Some(extra.to_string()),
        }])
    }
}

pub fn rapl_sysfs_present() -> bool {
    fs::read_dir(RAPL_ROOT)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("intel-rapl"))
}

pub fn hwmon_package_power_mw() -> Option<u32> {
    let root = Path::new(HWMON_ROOT);
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let hwmon = entry.path();
        let name = read_trimmed(&hwmon.join("name"))?;
        let name_l = name.to_ascii_lowercase();
        if !(name_l.contains("coretemp") || name_l.contains("package") || name_l.contains("cpu")) {
            continue;
        }
        if let Some(uw) = read_u64(&hwmon.join("power1_input")) {
            // hwmon power inputs are typically in microwatts.
            let mw = (uw / 1000) as u32;
            if valid_power_mw_u32(mw) {
                return Some(mw);
            }
        }
    }
    None
}

pub fn hwmon_max_core_temp_c() -> Option<f64> {
    let root = Path::new(HWMON_ROOT);
    let entries = fs::read_dir(root).ok()?;
    let mut max_temp: Option<f64> = None;
    for entry in entries.flatten() {
        let hwmon = entry.path();
        let name = read_trimmed(&hwmon.join("name")).unwrap_or_default();
        let name_l = name.to_ascii_lowercase();
        if !(name_l.contains("coretemp") || name_l.contains("cpu")) {
            continue;
        }
        for i in 1..=32 {
            let path = hwmon.join(format!("temp{i}_input"));
            let Some(milli_c) = read_i64(&path) else {
                break;
            };
            let c = milli_c as f64 / 1000.0;
            max_temp = Some(max_temp.map_or(c, |m| m.max(c)));
        }
    }
    max_temp
}

fn rapl_constraint_mw() -> Option<u32> {
    let root = Path::new(RAPL_ROOT);
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("intel-rapl") {
            continue;
        }
        let base = entry.path();
        if let Some(uw) = read_u64(&base.join("constraint_0_power_limit_uw")) {
            let mw = (uw / 1000) as u32;
            if valid_power_mw_u32(mw) {
                return Some(mw);
            }
        }
    }
    None
}

fn primary_intel_drm_identity() -> (String, String) {
    let root = drm::sysfs_drm_root();
    if let Ok(cards) = drm::scan_cards(&root) {
        for card in &cards {
            if card.driver.as_deref() == Some("i915")
                || card
                    .vendor_id
                    .as_deref()
                    .is_some_and(|v| v.eq_ignore_ascii_case("0x8086"))
            {
                return (
                    card.pci.clone().unwrap_or_else(|| "intel".into()),
                    format!("Intel {}", card.card),
                );
            }
        }
        if let Some(card) = cards.first() {
            return (
                card.pci.clone().unwrap_or_else(|| "unknown".into()),
                card.card.clone(),
            );
        }
    }
    ("intel".into(), "Intel (package)".into())
}

fn read_trimmed(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

fn read_i64(path: &Path) -> Option<i64> {
    read_trimmed(path)?.parse().ok()
}

/// Probe whether amdgpu gpu_metrics works on a DRM device path (for capability reports).
pub fn amdgpu_gpu_metrics_readable(sysfs_device: &Path) -> bool {
    GpuMetrics::get_from_sysfs_path(sysfs_device).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reads_hwmon_power_from_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let hwmon = dir.path().join("hwmon0");
        fs::create_dir_all(&hwmon).unwrap();
        fs::write(hwmon.join("name"), "coretemp\n").unwrap();
        fs::write(hwmon.join("power1_input"), "32000000\n").unwrap();

        // Direct read using same logic on fixture path
        let uw = read_u64(&hwmon.join("power1_input")).unwrap();
        let mw = (uw / 1000) as u32;
        assert_eq!(mw, 32_000);
    }
}
