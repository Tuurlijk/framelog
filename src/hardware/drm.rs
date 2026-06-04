use std::fs;
use std::path::{Path, PathBuf};

use crate::context::sysfs;

#[derive(Debug, Clone, Default)]
pub struct DrmCard {
    pub card: String,
    pub pci: Option<String>,
    pub vendor_id: Option<String>,
    pub device_id: Option<String>,
    pub driver: Option<String>,
    pub runtime_status: Option<String>,
    pub runtime_active_ms: Option<u64>,
    pub runtime_suspended_ms: Option<u64>,
    pub control: Option<String>,
    pub d3cold_allowed: Option<bool>,
    pub power_state: Option<String>,
}

impl DrmCard {
    pub fn is_amdgpu(&self) -> bool {
        self.driver.as_deref() == Some("amdgpu")
            || self
                .vendor_id
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case("0x1002"))
    }

    pub fn likely_apu(&self) -> bool {
        looks_like_apu(
            self.driver.as_deref().unwrap_or("amdgpu"),
            self.vendor_id.as_deref(),
            self.device_id.as_deref(),
        )
    }

    pub fn likely_dgpu(&self) -> bool {
        self.is_amdgpu() && !self.likely_apu()
    }
}

pub fn sysfs_drm_root() -> PathBuf {
    sysfs::sysfs_root().join("class/drm")
}

pub fn is_card_name(name: &str) -> bool {
    name.strip_prefix("card")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

pub fn looks_like_apu(name_or_driver: &str, vendor: Option<&str>, device: Option<&str>) -> bool {
    let name = name_or_driver.to_ascii_lowercase();
    if name.contains("apu")
        || name.contains("radeon graphics")
        || (name.contains("radeon") && name.contains("graphics"))
        || name.contains("ryzen")
        || name.contains("phoenix")
        || name.contains("hawk")
        || name.contains("strix")
        || name.contains("780m")
    {
        return true;
    }

    if let (Some(v), Some(dev)) = (vendor, device) {
        let v = v.trim_start_matches("0x");
        let dev = dev.trim_start_matches("0x");
        if v.eq_ignore_ascii_case("1002") && dev.starts_with("15") {
            return true;
        }
    }

    false
}

pub fn pick_dgpu(cards: &[DrmCard]) -> Option<&DrmCard> {
    cards.iter().find(|c| c.likely_dgpu()).or_else(|| {
        cards
            .iter()
            .filter(|c| c.is_amdgpu())
            .max_by_key(|c| c.card.as_str())
    })
}

pub fn scan_cards(root: &Path) -> Result<Vec<DrmCard>, String> {
    let entries = fs::read_dir(root).map_err(|e| format!("read {root:?}: {e}"))?;
    let mut cards = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_card_name(&name) {
            continue;
        }
        cards.push(read_card(&name, &entry.path()));
    }
    cards.sort_by(|a, b| a.card.cmp(&b.card));
    Ok(cards)
}

fn read_card(card_name: &str, card_path: &Path) -> DrmCard {
    let device = card_path.join("device");
    DrmCard {
        card: card_name.to_string(),
        pci: device
            .canonicalize()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
        vendor_id: sysfs::read_sysfs_string(&device.join("vendor")),
        device_id: sysfs::read_sysfs_string(&device.join("device")),
        driver: driver_name(&device),
        runtime_status: sysfs::read_sysfs_string(&device.join("power/runtime_status")),
        runtime_active_ms: sysfs::read_sysfs_u64(&device.join("power/runtime_active_time")),
        runtime_suspended_ms: sysfs::read_sysfs_u64(&device.join("power/runtime_suspended_time")),
        control: sysfs::read_sysfs_string(&device.join("power/control")),
        d3cold_allowed: sysfs::read_sysfs_i64(&device.join("d3cold_allowed")).map(|v| v != 0),
        power_state: sysfs::read_sysfs_string(&device.join("power_state")),
    }
}

pub fn driver_name(device: &Path) -> Option<String> {
    let path = fs::read_link(device.join("driver")).ok()?;
    path.file_name().map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_string(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn apu_heuristic_matches_phoenix_device_id() {
        assert!(looks_like_apu("amdgpu", Some("0x1002"), Some("0x15bf")));
        assert!(!looks_like_apu("amdgpu", Some("0x1002"), Some("0x747e")));
        assert!(looks_like_apu("AMD Radeon Graphics", None, None));
        assert!(looks_like_apu("AMD Radeon 780M Graphics", None, None));
    }

    #[test]
    fn scans_cards_and_picks_dgpu() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("class/drm");
        let apu = root.join("card0/device");
        write_string(&apu.join("vendor"), "0x1002\n");
        write_string(&apu.join("device"), "0x15bf\n");
        write_string(&apu.join("power/runtime_status"), "active\n");

        let dgpu = root.join("card1/device");
        write_string(&dgpu.join("vendor"), "0x1002\n");
        write_string(&dgpu.join("device"), "0x747e\n");
        write_string(&dgpu.join("power/runtime_status"), "suspended\n");
        write_string(&dgpu.join("d3cold_allowed"), "1\n");
        write_string(&dgpu.join("power/runtime_active_time"), "1000\n");
        write_string(&dgpu.join("power/runtime_suspended_time"), "5000\n");

        let cards = scan_cards(&root).unwrap();
        let picked = pick_dgpu(&cards).unwrap();
        assert_eq!(picked.card, "card1");
        assert_eq!(picked.runtime_status.as_deref(), Some("suspended"));
        assert_eq!(picked.d3cold_allowed, Some(true));
    }
}
