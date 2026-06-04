use libamdgpu_top::AMDGPU::{ThrottleStatus, ThrottlerBit};

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
        "PROCHOT_CPU" => "CPU prochot / thermal throttle signal",
        "PROCHOT_GPU" => "GPU prochot signal",
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
}
