use std::fs;
use std::path::{Path, PathBuf};

pub fn sysfs_root() -> PathBuf {
    std::env::var("FRAMELOG_SYSFS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/sys"))
}

pub fn read_sysfs_string(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn read_sysfs_i64(path: &Path) -> Option<i64> {
    read_sysfs_string(path)?.parse().ok()
}

pub fn read_sysfs_u64(path: &Path) -> Option<u64> {
    read_sysfs_string(path)?.parse().ok()
}

pub fn micro_watts_from_vi(voltage_uv: u64, current_ua: u64) -> f64 {
    (voltage_uv as f64) * (current_ua as f64) / 1_000_000_000_000.0
}

pub fn micro_watts_to_watts(micro_watts: i64) -> f64 {
    micro_watts as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vi_to_watts() {
        assert!((micro_watts_from_vi(20_000_000, 1_500_000) - 30.0).abs() < 0.01);
    }
}
