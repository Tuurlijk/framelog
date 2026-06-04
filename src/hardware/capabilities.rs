use serde::Serialize;

use crate::collector::CollectorKind;
use crate::context::pmf;
use crate::hardware::drm::{self, DrmCard};
use crate::hardware::intel;
use crate::version;

#[derive(Clone, Debug, Serialize)]
pub struct CapabilitiesReport {
    pub framelog_version: &'static str,
    pub git_hash: &'static str,
    pub selected_collector: String,
    pub recommended_collector: String,
    pub drm_cards: Vec<DrmCardSummary>,
    pub signals: SignalCapabilities,
    pub context_sources: Vec<ContextSourceCapability>,
    pub evidence_doctor: EvidenceDoctorCapabilities,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DrmCardSummary {
    pub card: String,
    pub pci: Option<String>,
    pub vendor_id: Option<String>,
    pub device_id: Option<String>,
    pub driver: Option<String>,
    pub likely_apu: bool,
    pub likely_dgpu: bool,
    pub gpu_metrics_available: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SignalCapabilities {
    pub amdgpu_throttle_flags: bool,
    pub amdgpu_gpu_metrics: bool,
    pub package_power: bool,
    pub package_power_label: String,
    pub core_temperature: bool,
    pub stapm_limits: bool,
    pub pmf_limits: bool,
    pub dgpu_runtime: bool,
    pub intel_rapl_sysfs: bool,
    pub intel_hwmon_power: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContextSourceCapability {
    pub id: String,
    pub available: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct EvidenceDoctorCapabilities {
    pub framework_146_rules: bool,
    pub amd_throttle_rules: bool,
    pub generic_power_thermal_rules: bool,
}

pub fn probe(selected: CollectorKind, apu_only: bool) -> CapabilitiesReport {
    let mut notes = Vec::new();
    let drm_root = drm::sysfs_drm_root();
    let drm_cards = scan_drm_summaries(&drm_root, &mut notes);
    let has_amdgpu = drm_cards
        .iter()
        .any(|c| c.driver.as_deref() == Some("amdgpu"));
    let has_amdgpu_metrics = drm_cards.iter().any(|c| c.gpu_metrics_available);
    let has_amd_apu = drm_cards
        .iter()
        .any(|c| c.likely_apu && c.gpu_metrics_available);
    let has_amd_dgpu = drm_cards.iter().any(|c| c.likely_dgpu);
    let intel_rapl = intel::rapl_sysfs_present();
    let intel_hwmon_power = intel::hwmon_package_power_mw().is_some();
    let pmf_ok = pmf_limits_readable();
    let dgpu_runtime = drm_cards.iter().any(|c| c.likely_dgpu) || has_amdgpu;

    let recommended = recommend_collector(
        has_amdgpu_metrics,
        intel_rapl || intel_hwmon_power,
        &mut notes,
    );

    let amdgpu_throttle = matches!(selected, CollectorKind::Fake)
        || (has_amdgpu_metrics && !matches!(selected, CollectorKind::Intel));
    let package_power = match selected {
        CollectorKind::Intel => intel_hwmon_power || intel_rapl,
        CollectorKind::Fake => true,
        _ => has_amdgpu_metrics || has_amd_apu,
    };
    let package_power_label = if amdgpu_throttle && has_amd_apu {
        "APU package power".to_string()
    } else {
        "Package power".to_string()
    };

    let signals = SignalCapabilities {
        amdgpu_throttle_flags: amdgpu_throttle,
        amdgpu_gpu_metrics: has_amdgpu_metrics && !matches!(selected, CollectorKind::Intel),
        package_power,
        package_power_label,
        core_temperature: has_amdgpu_metrics
            || intel_hwmon_power
            || intel::hwmon_max_core_temp_c().is_some(),
        stapm_limits: has_amdgpu_metrics && has_amd_apu,
        pmf_limits: pmf_ok,
        dgpu_runtime,
        intel_rapl_sysfs: intel_rapl,
        intel_hwmon_power,
    };

    let context_sources = probe_context_sources(pmf_ok, &drm_cards);
    let evidence_doctor = EvidenceDoctorCapabilities {
        framework_146_rules: amdgpu_throttle && pmf_ok,
        amd_throttle_rules: amdgpu_throttle,
        generic_power_thermal_rules: true,
    };

    if apu_only
        && has_amd_dgpu
        && matches!(
            selected,
            CollectorKind::LibAmdgpu | CollectorKind::AmdgpuTopJson
        )
    {
        notes.push(
            "--apu-only skips discrete GPU throttle samples; omit it on dual-GPU Framework laptops."
                .into(),
        );
    }
    if !has_amdgpu_metrics
        && matches!(
            selected,
            CollectorKind::LibAmdgpu | CollectorKind::AmdgpuTopJson
        )
    {
        notes.push(
            "lib-amdgpu needs amdgpu gpu_metrics; try --collector intel on Intel-only Framework models."
                .into(),
        );
    }

    CapabilitiesReport {
        framelog_version: version::VERSION,
        git_hash: version::GIT_HASH,
        selected_collector: collector_kind_name(selected).to_string(),
        recommended_collector: recommended,
        drm_cards,
        signals,
        context_sources,
        evidence_doctor,
        notes,
    }
}

pub fn collector_kind_name(kind: CollectorKind) -> &'static str {
    match kind {
        CollectorKind::LibAmdgpu => "lib-amdgpu",
        CollectorKind::Fake => "fake",
        CollectorKind::AmdgpuTopJson => "amdgpu-top-json",
        CollectorKind::Intel => "intel",
    }
}

fn recommend_collector(
    has_amdgpu_metrics: bool,
    has_intel_power: bool,
    notes: &mut Vec<String>,
) -> String {
    if has_amdgpu_metrics {
        "lib-amdgpu".into()
    } else if has_intel_power {
        notes.push(
            "No amdgpu gpu_metrics; intel collector reads package power from RAPL/hwmon.".into(),
        );
        "intel".into()
    } else {
        notes
            .push("No supported GPU power source detected; throttle timeline may be empty.".into());
        "none".into()
    }
}

fn scan_drm_summaries(root: &std::path::Path, notes: &mut Vec<String>) -> Vec<DrmCardSummary> {
    match drm::scan_cards(root) {
        Ok(cards) => cards.iter().map(|c| drm_card_summary(root, c)).collect(),
        Err(e) => {
            notes.push(format!("DRM scan failed: {e}"));
            Vec::new()
        }
    }
}

fn drm_card_summary(root: &std::path::Path, card: &DrmCard) -> DrmCardSummary {
    let device = root.join(&card.card).join("device");
    let gpu_metrics_available = card.is_amdgpu() && intel::amdgpu_gpu_metrics_readable(&device);
    DrmCardSummary {
        card: card.card.clone(),
        pci: card.pci.clone(),
        vendor_id: card.vendor_id.clone(),
        device_id: card.device_id.clone(),
        driver: card.driver.clone(),
        likely_apu: card.likely_apu(),
        likely_dgpu: card.likely_dgpu(),
        gpu_metrics_available,
    }
}

fn pmf_limits_readable() -> bool {
    let path = pmf::debugfs_root().join("amd_pmf/current_power_limits");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| pmf::parse_current_power_limits(s.lines().next().unwrap_or("")))
        .is_some()
}

fn probe_context_sources(pmf_ok: bool, cards: &[DrmCardSummary]) -> Vec<ContextSourceCapability> {
    vec![
        ContextSourceCapability {
            id: "power".into(),
            available: true,
            detail: "sysfs platform power".into(),
        },
        ContextSourceCapability {
            id: "battery".into(),
            available: true,
            detail: "sysfs fuel gauge".into(),
        },
        ContextSourceCapability {
            id: "display".into(),
            available: true,
            detail: "DRM connector count".into(),
        },
        ContextSourceCapability {
            id: "profile".into(),
            available: true,
            detail: "power profiles daemon (D-Bus)".into(),
        },
        ContextSourceCapability {
            id: "sleep".into(),
            available: true,
            detail: "journal sleep/wake markers".into(),
        },
        ContextSourceCapability {
            id: "pmf".into(),
            available: pmf_ok,
            detail: if pmf_ok {
                "debugfs amd_pmf/current_power_limits".into()
            } else {
                "requires /sys/kernel/debug/amd_pmf/current_power_limits".into()
            },
        },
        ContextSourceCapability {
            id: "gpu_power".into(),
            available: !cards.is_empty(),
            detail: "DRM runtime status for detected cards".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommends_lib_amdgpu_when_metrics_present() {
        let mut notes = Vec::new();
        assert_eq!(recommend_collector(true, true, &mut notes), "lib-amdgpu");
    }

    #[test]
    fn recommends_intel_without_amdgpu_metrics() {
        let mut notes = Vec::new();
        assert_eq!(recommend_collector(false, true, &mut notes), "intel");
        assert!(!notes.is_empty());
    }
}
