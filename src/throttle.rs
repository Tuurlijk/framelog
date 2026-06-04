use libamdgpu_top::AMDGPU::{ThrottleStatus, ThrottlerBit};

use crate::model::{FlagActivitySummary, MetricSample};

/// Legacy Yellow Carp / SMU13 `ThrottlerStatus` value with only PROCHOT_CPU (bit 9) and PROCHOT_GFX (bit 10).
pub const LEGACY_YELLOW_CARP_PROCHOT_RAW: u32 = (1 << 9) | (1 << 10);

const PROCHOT_STICKY_ACTIVE_PCT: f64 = 95.0;
const LEGACY_RAW_SAMPLE_PCT: f64 = 90.0;

/// Stable flag names for storage and API (matches `ThrottlerBit` debug names).
pub fn flag_name(bit: ThrottlerBit) -> &'static str {
    match bit {
        ThrottlerBit::PPT0 => "PPT0",
        ThrottlerBit::PPT1 => "PPT1",
        ThrottlerBit::PPT2 => "PPT2",
        ThrottlerBit::PPT3 => "PPT3",
        ThrottlerBit::SPL => "SPL",
        ThrottlerBit::FPPT => "FPPT",
        ThrottlerBit::SPPT => "SPPT",
        ThrottlerBit::SPPT_APU => "SPPT_APU",
        ThrottlerBit::TDC_GFX => "TDC_GFX",
        ThrottlerBit::TDC_SOC => "TDC_SOC",
        ThrottlerBit::TDC_MEM => "TDC_MEM",
        ThrottlerBit::TDC_VDD => "TDC_VDD",
        ThrottlerBit::TDC_CVIP => "TDC_CVIP",
        ThrottlerBit::EDC_CPU => "EDC_CPU",
        ThrottlerBit::EDC_GFX => "EDC_GFX",
        ThrottlerBit::APCC => "APCC",
        ThrottlerBit::TEMP_GPU => "TEMP_GPU",
        ThrottlerBit::TEMP_CORE => "TEMP_CORE",
        ThrottlerBit::TEMP_MEM => "TEMP_MEM",
        ThrottlerBit::TEMP_EDGE => "TEMP_EDGE",
        ThrottlerBit::TEMP_HOTSPOT => "TEMP_HOTSPOT",
        ThrottlerBit::TEMP_SOC => "TEMP_SOC",
        ThrottlerBit::TEMP_VR_GFX => "TEMP_VR_GFX",
        ThrottlerBit::TEMP_VR_SOC => "TEMP_VR_SOC",
        ThrottlerBit::TEMP_VR_MEM0 => "TEMP_VR_MEM0",
        ThrottlerBit::TEMP_VR_MEM1 => "TEMP_VR_MEM1",
        ThrottlerBit::TEMP_LIQUID0 => "TEMP_LIQUID0",
        ThrottlerBit::TEMP_LIQUID1 => "TEMP_LIQUID1",
        ThrottlerBit::VRHOT0 => "VRHOT0",
        ThrottlerBit::VRHOT1 => "VRHOT1",
        ThrottlerBit::PROCHOT_CPU => "PROCHOT_CPU",
        ThrottlerBit::PROCHOT_GPU => "PROCHOT_GPU",
        ThrottlerBit::PPM => "PPM",
        ThrottlerBit::FIT => "FIT",
        ThrottlerBit::Unknown => "Unknown",
    }
}

pub fn active_flags(mask: u64) -> Vec<String> {
    ThrottleStatus::new(mask)
        .get_all_throttler()
        .into_iter()
        .filter(|b| *b != ThrottlerBit::Unknown)
        .map(|b| flag_name(b).to_string())
        .collect()
}

pub fn flag_active(mask: u64, name: &str) -> bool {
    ThrottleStatus::new(mask)
        .get_all_throttler()
        .into_iter()
        .any(|b| flag_name(b) == name)
}

/// Category for UI grouping and presets.
pub fn flag_category(name: &str) -> &'static str {
    match name {
        "SPL" | "SPPT" | "FPPT" | "SPPT_APU" | "PPT0" | "PPT1" | "PPT2" | "PPT3" | "PPM"
        | "APCC" | "FIT" => "power",
        "PROCHOT_CPU" | "PROCHOT_GPU" | "TEMP_GPU" | "TEMP_CORE" | "TEMP_MEM" | "TEMP_EDGE"
        | "TEMP_HOTSPOT" | "TEMP_SOC" | "TEMP_VR_GFX" | "TEMP_VR_SOC" | "TEMP_VR_MEM0"
        | "TEMP_VR_MEM1" | "TEMP_LIQUID0" | "TEMP_LIQUID1" | "VRHOT0" | "VRHOT1" => "thermal",
        "TDC_GFX" | "TDC_SOC" | "TDC_MEM" | "TDC_VDD" | "TDC_CVIP" | "EDC_CPU" | "EDC_GFX" => {
            "current"
        }
        _ => "other",
    }
}

pub fn flag_description(name: &str) -> &'static str {
    match name {
        "SPL" => "Socket power limit (slow package limit)",
        "SPPT" => "Slow package power tracking",
        "FPPT" => "Fast package power tracking",
        "PROCHOT_CPU" => {
            "CPU prochot signal (may be sticky on gpu_metrics v2.1; see docs/prochot-status-investigation.md)"
        }
        "PROCHOT_GPU" => {
            "GPU prochot signal (may be sticky on gpu_metrics v2.1; see docs/prochot-status-investigation.md)"
        }
        "TEMP_HOTSPOT" => "Hotspot temperature limit",
        _ => "AMDGPU independent throttle bit",
    }
}

pub fn catalog_flags() -> Vec<&'static str> {
    known_flag_names()
}

fn known_flag_names() -> Vec<&'static str> {
    vec![
        "PPT0",
        "PPT1",
        "PPT2",
        "PPT3",
        "SPL",
        "FPPT",
        "SPPT",
        "SPPT_APU",
        "TDC_GFX",
        "TDC_SOC",
        "TDC_MEM",
        "TDC_VDD",
        "TDC_CVIP",
        "EDC_CPU",
        "EDC_GFX",
        "APCC",
        "TEMP_GPU",
        "TEMP_CORE",
        "TEMP_MEM",
        "TEMP_EDGE",
        "TEMP_HOTSPOT",
        "TEMP_SOC",
        "TEMP_VR_GFX",
        "TEMP_VR_SOC",
        "TEMP_VR_MEM0",
        "TEMP_VR_MEM1",
        "TEMP_LIQUID0",
        "TEMP_LIQUID1",
        "VRHOT0",
        "VRHOT1",
        "PROCHOT_CPU",
        "PROCHOT_GPU",
        "PPM",
        "FIT",
    ]
}

/// Legacy `throttle_status` with only PROCHOT bits set (common sticky pattern on Radeon 780M / v2.1).
pub fn legacy_prochot_only_raw(raw: u32) -> bool {
    raw == LEGACY_YELLOW_CARP_PROCHOT_RAW
}

/// PROCHOT flags always on in the window summary with no assert/clear transitions.
pub fn prochot_sticky_from_activity(activity: &[FlagActivitySummary]) -> bool {
    let Some(cpu) = activity.iter().find(|f| f.flag_name == "PROCHOT_CPU") else {
        return false;
    };
    let Some(gpu) = activity.iter().find(|f| f.flag_name == "PROCHOT_GPU") else {
        return false;
    };
    cpu.active_sample_pct >= PROCHOT_STICKY_ACTIVE_PCT
        && gpu.active_sample_pct >= PROCHOT_STICKY_ACTIVE_PCT
        && cpu.assert_count == 0
        && gpu.assert_count == 0
        && cpu.clear_count == 0
        && gpu.clear_count == 0
}

/// Sticky PROCHOT likely from legacy `throttle_status` mapping, not proven emergency throttle.
pub fn prochot_suspect_sticky_legacy(samples: &[MetricSample]) -> bool {
    if samples.len() < 5 {
        return false;
    }
    let prochot_samples = samples
        .iter()
        .filter(|s| {
            s.active_flags.iter().any(|f| f == "PROCHOT_CPU")
                && s.active_flags.iter().any(|f| f == "PROCHOT_GPU")
        })
        .count();
    if prochot_samples * 100 / samples.len() < 90 {
        return false;
    }
    let raws: Vec<u32> = samples
        .iter()
        .filter_map(|s| s.throttle_status_raw)
        .collect();
    if raws.is_empty() {
        return false;
    }
    let legacy_only = raws.iter().filter(|r| legacy_prochot_only_raw(**r)).count();
    (legacy_only as f64 / raws.len() as f64) * 100.0 >= LEGACY_RAW_SAMPLE_PCT
}

pub fn prochot_dashboard_warning() -> &'static str {
    "PROCHOT_CPU and PROCHOT_GPU are always on with no transitions. On Radeon 780M / gpu_metrics v2.1 this often reflects legacy throttle_status bits 9+10 and may not mean real emergency thermal throttling. Verify temperature and performance; see docs/prochot-status-investigation.md."
}

pub fn prochot_warnings_from_activity(activity: &[FlagActivitySummary]) -> Vec<String> {
    if prochot_sticky_from_activity(activity) {
        vec![prochot_dashboard_warning().into()]
    } else {
        Vec::new()
    }
}

/// Returns `(flag_name, old_active, new_active)` for each changed bit.
pub fn diff_flags(old_mask: u64, new_mask: u64) -> Vec<(String, bool, bool)> {
    let mut changes = Vec::new();
    for bit in 0..64u8 {
        let old = (old_mask >> bit) & 1 == 1;
        let new = (new_mask >> bit) & 1 == 1;
        if old != new {
            let name = flag_name(ThrottlerBit::from(bit));
            if name != "Unknown" {
                changes.push((name.to_string(), old, new));
            }
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use libamdgpu_top::AMDGPU::ThrottlerBit;

    #[test]
    fn spl_bit_is_active() {
        let mask = 1u64 << (ThrottlerBit::SPL as u64);
        assert!(flag_active(mask, "SPL"));
        assert!(!flag_active(mask, "PROCHOT_CPU"));
    }

    #[test]
    fn prochot_cpu_bit_is_active() {
        let mask = 1u64 << (ThrottlerBit::PROCHOT_CPU as u64);
        assert!(flag_active(mask, "PROCHOT_CPU"));
    }

    #[test]
    fn diff_detects_toggle() {
        let old = 1u64 << (ThrottlerBit::SPL as u64);
        let new = old | (1u64 << (ThrottlerBit::PROCHOT_CPU as u64));
        let changes = diff_flags(old, new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, "PROCHOT_CPU");
        assert!(!changes[0].1);
        assert!(changes[0].2);
    }

    #[test]
    fn catalog_covers_known_flags() {
        let catalog = catalog_flags();
        assert_eq!(catalog, known_flag_names());
    }

    #[test]
    fn legacy_prochot_raw_is_1536() {
        assert!(legacy_prochot_only_raw(1536));
        assert!(!legacy_prochot_only_raw(1537));
    }

    #[test]
    fn sticky_prochot_from_activity() {
        let activity = vec![
            FlagActivitySummary {
                flag_name: "PROCHOT_CPU".into(),
                active_sample_pct: 100.0,
                assert_count: 0,
                clear_count: 0,
            },
            FlagActivitySummary {
                flag_name: "PROCHOT_GPU".into(),
                active_sample_pct: 100.0,
                assert_count: 0,
                clear_count: 0,
            },
        ];
        assert!(prochot_sticky_from_activity(&activity));
    }
}
