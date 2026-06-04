use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use libamdgpu_top::DevicePath;
use libamdgpu_top::AMDGPU::{GpuMetrics, MetricsInfo};

use crate::error::{Error, Result};
use crate::hardware::drm;
use crate::model::MetricSample;
use crate::throttle::active_flags;

pub trait Collector: Send + Sync {
    fn sample(&self) -> Result<Vec<MetricSample>>;
}

pub struct LibAmdgpuCollector {
    apu_only: bool,
}

impl LibAmdgpuCollector {
    pub fn new(apu_only: bool) -> Self {
        Self { apu_only }
    }
}

impl Collector for LibAmdgpuCollector {
    fn sample(&self) -> Result<Vec<MetricSample>> {
        let devices = DevicePath::get_device_path_list();
        if devices.is_empty() {
            return Err(Error::Collector(
                "no AMDGPU devices found (is the amdgpu driver loaded?)".into(),
            ));
        }

        let ts_unix_ms = crate::util::current_ts_ms();
        let mut samples = Vec::new();

        for device in devices {
            if self.apu_only && !drm::looks_like_apu(&device.device_name, None, None) {
                continue;
            }

            let metrics = match GpuMetrics::get_from_sysfs_path(&device.sysfs_path) {
                Ok(m) => m,
                Err(err) => {
                    tracing::debug!(pci = %device.pci, %err, "gpu_metrics unavailable");
                    continue;
                }
            };

            let indep = metrics
                .get_indep_throttle_status()
                .or_else(|| metrics.get_indep_throttle_status_without_check())
                .unwrap_or(0);

            let temperature_core_raw = metrics.get_temperature_core();
            let temperature_core_max = temperature_core_raw
                .as_ref()
                .and_then(|v| v.iter().copied().max())
                .map(centi_celsius_to_celsius);

            let average_temperature_core_raw = metrics.get_average_temperature_core();

            let temperature_core_raw_json = temperature_core_raw.as_ref();
            let average_temperature_core_raw_json = average_temperature_core_raw.as_ref();

            let temperature_core_celsius = temperature_core_raw_json.map(|values| {
                values
                    .iter()
                    .copied()
                    .map(centi_celsius_to_celsius)
                    .collect::<Vec<_>>()
            });
            let average_temperature_core_celsius =
                average_temperature_core_raw_json.map(|values| {
                    values
                        .iter()
                        .copied()
                        .map(centi_celsius_to_celsius)
                        .collect::<Vec<_>>()
                });

            let device_id_hex = device.device_id.map(|id| format!("{:04x}", id));
            let is_apu =
                drm::looks_like_apu(&device.device_name, Some("1002"), device_id_hex.as_deref());
            let (apu_power_mw, apu_power_source) = resolve_apu_power_mw(&metrics, is_apu);

            let mut extra = serde_json::json!({
                "average_apu_power_mw": metrics.get_average_apu_power(),
                "average_cpu_power_mw": metrics.get_average_cpu_power(),
                "average_socket_power_mw": metrics.get_average_socket_power(),
                "average_gfx_power_mw": metrics.get_average_gfx_power_u32()
                    .or_else(|| metrics.get_average_gfx_power().map(|v| v as u32)),
                "average_dgpu_power_mw": metrics.get_average_dgpu_power(),
                "temperature_core_raw_centi_c": temperature_core_raw_json,
                "temperature_core_c": temperature_core_celsius,
                "average_temperature_core_raw_centi_c": average_temperature_core_raw_json,
                "average_temperature_core_c": average_temperature_core_celsius,
            });
            if let Some(source) = apu_power_source {
                extra["apu_power_source"] = serde_json::Value::String(source.to_string());
            }

            samples.push(MetricSample {
                ts_unix_ms,
                device_pci: device.pci.to_string(),
                device_name: device.device_name.clone(),
                throttle_status_raw: metrics.get_throttle_status(),
                indep_throttle_status: indep,
                active_flags: active_flags(indep),
                apu_power_mw,
                stapm_limit_mw: metrics.get_stapm_power_limit(),
                current_stapm_limit_mw: metrics.get_current_stapm_power_limit(),
                temperature_core_max,
                extra_json: Some(extra.to_string()),
            });
        }

        if samples.is_empty() {
            return Err(Error::Collector(
                "no gpu_metrics samples collected from any device".into(),
            ));
        }

        Ok(samples)
    }
}

fn centi_celsius_to_celsius(value: u16) -> f64 {
    f64::from(value) / 100.0
}

pub(crate) fn valid_power_mw_u32(value: u32) -> bool {
    value != 0 && value != u32::MAX && value != u16::MAX as u32
}

fn valid_power_mw_u16(value: u16) -> bool {
    value != 0 && value != u16::MAX
}

/// `average_apu_power` exists only in gpu_metrics v3.0+. On v2.x APUs the same
/// package reading is usually exposed as `average_socket_power`.
fn resolve_apu_power_mw(metrics: &GpuMetrics, is_apu: bool) -> (Option<u32>, Option<&'static str>) {
    if !is_apu {
        return (None, None);
    }

    if let Some(v) = metrics
        .get_average_apu_power()
        .filter(|v| valid_power_mw_u32(*v))
    {
        return (Some(v), Some("average_apu_power"));
    }

    if let Some(v) = metrics
        .get_average_socket_power()
        .filter(|v| valid_power_mw_u32(*v))
    {
        return (Some(v), Some("average_socket_power"));
    }

    let cpu = metrics
        .get_average_cpu_power()
        .filter(|v| valid_power_mw_u16(*v))
        .map(u32::from);
    let gfx = metrics
        .get_average_gfx_power_u32()
        .or_else(|| metrics.get_average_gfx_power().map(u32::from))
        .filter(|v| valid_power_mw_u32(*v));

    match (cpu, gfx) {
        (Some(c), Some(g)) => (Some(c.saturating_add(g)), Some("cpu_plus_gfx_power")),
        (Some(c), None) => (Some(c), Some("average_cpu_power")),
        (None, Some(g)) => (Some(g), Some("average_gfx_power")),
        (None, None) => (None, None),
    }
}

/// Synthetic collector for UI/tests without AMDGPU hardware.
pub struct FakeCollector {
    tick: AtomicU64,
}

impl FakeCollector {
    pub fn new() -> Self {
        Self {
            tick: AtomicU64::new(0),
        }
    }
}

impl Default for FakeCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector for FakeCollector {
    fn sample(&self) -> Result<Vec<MetricSample>> {
        use libamdgpu_top::AMDGPU::ThrottlerBit;

        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        let spl_on = tick % 20 < 10;
        let prochot_on = tick % 30 >= 15 && tick % 30 < 25;

        let mut mask = 0u64;
        if spl_on {
            mask |= 1u64 << (ThrottlerBit::SPL as u64);
        }
        if prochot_on {
            mask |= 1u64 << (ThrottlerBit::PROCHOT_CPU as u64);
        }

        Ok(vec![MetricSample {
            ts_unix_ms: crate::util::current_ts_ms(),
            device_pci: "0000:00:00.0".into(),
            device_name: "Fake APU".into(),
            throttle_status_raw: None,
            indep_throttle_status: mask,
            active_flags: active_flags(mask),
            apu_power_mw: Some(if spl_on { 35_000 } else { 54_000 }),
            stapm_limit_mw: Some(if spl_on { 35_000 } else { 54_000 }),
            current_stapm_limit_mw: Some(if spl_on { 35_000 } else { 54_000 }),
            temperature_core_max: Some(74.0),
            extra_json: None,
        }])
    }
}

/// Shell-out adapter: `amdgpu_top -J -gm -n 1 --select-apu` (optional debug path).
pub struct AmdgpuTopJsonCollector {
    apu_only: bool,
}

impl AmdgpuTopJsonCollector {
    pub fn new(apu_only: bool) -> Self {
        Self { apu_only }
    }
}

impl Collector for AmdgpuTopJsonCollector {
    fn sample(&self) -> Result<Vec<MetricSample>> {
        use std::process::Command;

        let mut cmd = Command::new("amdgpu_top");
        cmd.args(["-J", "-gm", "-n", "1", "--no-pc"]);
        if self.apu_only {
            cmd.arg("--select-apu");
        }

        let output = cmd
            .output()
            .map_err(|e| Error::Collector(format!("failed to run amdgpu_top: {e}")))?;

        if !output.status.success() {
            return Err(Error::Collector(format!(
                "amdgpu_top exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        parse_amdgpu_top_json(&output.stdout)
    }
}

fn parse_amdgpu_top_json(bytes: &[u8]) -> Result<Vec<MetricSample>> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let ts_unix_ms = crate::util::current_ts_ms();
    let mut samples = Vec::new();

    let devices = value
        .get("devices")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::Collector("amdgpu_top JSON missing devices array".into()))?;

    for device in devices {
        let info = device.get("Info").or_else(|| device.get("info"));
        let pci = info
            .and_then(|i| i.get("PCI"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let name = info
            .and_then(|i| i.get("DeviceName"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let metrics = device
            .get("gpu_metrics")
            .or_else(|| device.get("stat").and_then(|s| s.get("gpu_metrics")));

        let indep = metrics
            .and_then(|m| m.get("Indep Throttle Status"))
            .or_else(|| metrics.and_then(|m| m.get("indep_throttle_status")))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        samples.push(MetricSample {
            ts_unix_ms,
            device_pci: pci,
            device_name: name,
            throttle_status_raw: metrics
                .and_then(|m| m.get("Throttle Status"))
                .or_else(|| metrics.and_then(|m| m.get("throttle_status")))
                .and_then(|v| v.as_u64())
                .map(|v| v as u32),
            indep_throttle_status: indep,
            active_flags: active_flags(indep),
            apu_power_mw: metrics.and_then(|m| {
                json_power_mw(m, "Average Power APU (mW)")
                    .or_else(|| json_power_mw(m, "Average Power Socket (mW)"))
                    .or_else(|| json_power_mw(m, "average_apu_power"))
                    .or_else(|| json_power_mw(m, "average_socket_power"))
            }),
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: None,
            extra_json: metrics.map(|m| m.to_string()),
        });
    }

    if samples.is_empty() {
        return Err(Error::Collector(
            "amdgpu_top JSON contained no devices".into(),
        ));
    }

    Ok(samples)
}

fn json_power_mw(metrics: &serde_json::Value, key: &str) -> Option<u32> {
    let v = metrics.get(key)?.as_u64()? as u32;
    valid_power_mw_u32(v).then_some(v)
}

pub fn make_collector(kind: CollectorKind, apu_only: bool) -> Arc<dyn Collector> {
    match kind {
        CollectorKind::LibAmdgpu => Arc::new(LibAmdgpuCollector::new(apu_only)),
        CollectorKind::Fake => Arc::new(FakeCollector::new()),
        CollectorKind::AmdgpuTopJson => Arc::new(AmdgpuTopJsonCollector::new(apu_only)),
        CollectorKind::Intel => Arc::new(crate::hardware::intel::IntelCollector::new()),
    }
}

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum CollectorKind {
    #[default]
    LibAmdgpu,
    Fake,
    AmdgpuTopJson,
    /// Read-only Intel package power / core temp (RAPL/hwmon); no AMD throttle flags.
    Intel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_collector_toggles_flags() {
        let c = FakeCollector::new();
        let mut masks = Vec::new();
        for _ in 0..25 {
            masks.push(c.sample().unwrap()[0].indep_throttle_status);
        }
        assert!(
            masks.iter().any(|&m| m != masks[0]),
            "fake collector should produce varying masks over 25 ticks"
        );
    }

    #[test]
    fn converts_centi_celsius_temperature() {
        assert_eq!(centi_celsius_to_celsius(10_187), 101.87);
    }

    #[test]
    fn rejects_invalid_power_sentinels() {
        assert!(!valid_power_mw_u32(0));
        assert!(!valid_power_mw_u32(u32::MAX));
        assert!(!valid_power_mw_u32(u16::MAX as u32));
        assert!(valid_power_mw_u32(35_000));
    }
}
