use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::hardware::drm;
use crate::model::{BiosInfo, CpuInfo, GpuInfo, MemoryInfo, OsInfo, StorageDeviceInfo, SystemInfo};

pub fn collect_system_info() -> SystemInfo {
    let mut warnings = Vec::new();
    let proc_root = procfs_root();
    let sys_root = sysfs_root();
    let etc_root = etc_root();

    let cpu = collect_cpu(&proc_root, &sys_root, &mut warnings);
    let gpus = collect_gpus(&sys_root);
    let memory = collect_memory(&proc_root, &sys_root, &mut warnings);
    let storage = collect_storage(&sys_root);
    let bios = collect_bios(&sys_root);
    let os = collect_os(&proc_root, &etc_root);
    let health = if warnings.is_empty() {
        "ok"
    } else {
        "degraded"
    }
    .to_string();

    SystemInfo {
        schema_version: 1,
        collected_at_ms: crate::util::current_ts_ms(),
        health,
        warnings,
        cpu,
        gpus,
        memory,
        storage,
        bios,
        os,
    }
}

fn procfs_root() -> PathBuf {
    std::env::var("FRAMELOG_PROCFS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/proc"))
}

fn sysfs_root() -> PathBuf {
    std::env::var("FRAMELOG_SYSFS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/sys"))
}

fn etc_root() -> PathBuf {
    std::env::var("FRAMELOG_ETC_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc"))
}

fn collect_cpu(proc_root: &Path, sys_root: &Path, warnings: &mut Vec<String>) -> CpuInfo {
    let cpuinfo = read_trimmed(&proc_root.join("cpuinfo"));
    let mut cpu = cpuinfo.as_deref().map(parse_cpuinfo).unwrap_or_default();
    if cpuinfo.is_none() {
        warnings.push("cpuinfo unavailable".into());
    }

    cpu.max_frequency_mhz =
        read_u64(&sys_root.join("devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"))
            .map(|khz| khz / 1000);
    cpu
}

fn parse_cpuinfo(raw: &str) -> CpuInfo {
    let mut vendor = None;
    let mut model_name = None;
    let mut logical_cpus = 0_u32;
    let mut physical_core_pairs = HashSet::new();
    let mut current_physical_id: Option<String> = None;
    let mut current_core_id: Option<String> = None;

    for line in raw.lines() {
        let Some((key, value)) = split_cpuinfo_line(line) else {
            continue;
        };
        match key {
            "vendor_id" | "CPU implementer" => {
                vendor.get_or_insert_with(|| value.to_string());
            }
            "model name" | "Hardware" => {
                model_name.get_or_insert_with(|| value.to_string());
            }
            "processor" => {
                logical_cpus += 1;
                if let (Some(phys), Some(core)) =
                    (current_physical_id.take(), current_core_id.take())
                {
                    physical_core_pairs.insert((phys, core));
                }
            }
            "physical id" => current_physical_id = Some(value.to_string()),
            "core id" => current_core_id = Some(value.to_string()),
            _ => continue,
        };
    }
    if let (Some(phys), Some(core)) = (current_physical_id, current_core_id) {
        physical_core_pairs.insert((phys, core));
    }

    CpuInfo {
        vendor,
        model_name,
        logical_cpus: (logical_cpus > 0).then_some(logical_cpus),
        physical_cores: (!physical_core_pairs.is_empty())
            .then_some(physical_core_pairs.len() as u32),
        max_frequency_mhz: None,
    }
}

fn split_cpuinfo_line(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    Some((key.trim(), value.trim()))
}

fn collect_memory(proc_root: &Path, sys_root: &Path, warnings: &mut Vec<String>) -> MemoryInfo {
    let meminfo = read_trimmed(&proc_root.join("meminfo"));
    let mut memory = meminfo.as_deref().map(parse_meminfo).unwrap_or_default();
    if meminfo.is_none() {
        warnings.push("meminfo unavailable".into());
    }

    let dmi_root = sys_root.join("devices/virtual/dmi/id");
    memory.configured_speed_mt_s = read_u64(&dmi_root.join("memory_speed"));
    memory
}

fn parse_meminfo(raw: &str) -> MemoryInfo {
    let mut memory = MemoryInfo::default();
    for line in raw.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let value = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok());
        match key {
            "MemTotal" => memory.total_kib = value,
            "MemAvailable" => memory.available_kib = value,
            _ => {}
        }
    }
    memory
}

fn collect_gpus(sys_root: &Path) -> Vec<GpuInfo> {
    drm::scan_cards(&sys_root.join("class/drm"))
        .unwrap_or_default()
        .into_iter()
        .map(|card| GpuInfo {
            card: card.card,
            vendor_id: card.vendor_id,
            device_id: card.device_id,
            driver: card.driver,
            pci_address: card.pci,
        })
        .collect()
}

fn collect_storage(sys_root: &Path) -> Vec<StorageDeviceInfo> {
    let block_root = sys_root.join("block");
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(block_root) else {
        return out;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("loop") || name.starts_with("ram") {
            continue;
        }
        let base = entry.path();
        let rotational = read_u64(&base.join("queue/rotational")).map(|v| v != 0);
        if rotational == Some(true) {
            continue;
        }
        let logical_block_size = read_u64(&base.join("queue/logical_block_size"));
        let physical_block_size = read_u64(&base.join("queue/physical_block_size"));
        let sectors = read_u64(&base.join("size"));
        let size_bytes = sectors.map(|s| s.saturating_mul(512));

        out.push(StorageDeviceInfo {
            name,
            model: read_trimmed(&base.join("device/model")),
            firmware_revision: read_trimmed(&base.join("device/firmware_rev"))
                .or_else(|| read_trimmed(&base.join("device/rev"))),
            size_bytes,
            rotational,
            logical_block_size,
            physical_block_size,
        });
    }
    out
}

fn collect_bios(sys_root: &Path) -> BiosInfo {
    let dmi = sys_root.join("class/dmi/id");
    BiosInfo {
        bios_vendor: read_trimmed(&dmi.join("bios_vendor")),
        bios_version: read_trimmed(&dmi.join("bios_version")),
        bios_date: read_trimmed(&dmi.join("bios_date")),
        board_vendor: read_trimmed(&dmi.join("board_vendor")),
        board_name: read_trimmed(&dmi.join("board_name")),
        product_name: read_trimmed(&dmi.join("product_name")),
        product_version: read_trimmed(&dmi.join("product_version")),
    }
}

fn collect_os(proc_root: &Path, etc_root: &Path) -> OsInfo {
    let release = parse_os_release(&read_trimmed(&etc_root.join("os-release")).unwrap_or_default());
    OsInfo {
        kernel_release: read_trimmed(&proc_root.join("sys/kernel/osrelease")),
        kernel_version: read_trimmed(&proc_root.join("version")),
        distro_name: release.get("NAME").cloned(),
        distro_version: release.get("VERSION").cloned(),
        pretty_name: release.get("PRETTY_NAME").cloned(),
    }
}

fn parse_os_release(raw: &str) -> std::collections::HashMap<String, String> {
    raw.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.trim_matches('"').to_string()))
        })
        .collect()
}

fn read_trimmed(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpuinfo() {
        let cpu = parse_cpuinfo(
            r#"
processor   : 0
vendor_id   : AuthenticAMD
model name  : AMD Ryzen Test
physical id : 0
core id     : 0
processor   : 1
vendor_id   : AuthenticAMD
model name  : AMD Ryzen Test
physical id : 0
core id     : 1
"#,
        );
        assert_eq!(cpu.vendor.as_deref(), Some("AuthenticAMD"));
        assert_eq!(cpu.model_name.as_deref(), Some("AMD Ryzen Test"));
        assert_eq!(cpu.logical_cpus, Some(2));
        assert_eq!(cpu.physical_cores, Some(2));
    }

    #[test]
    fn parses_meminfo() {
        let mem = parse_meminfo(
            r#"
MemTotal:       32768000 kB
MemAvailable:  12345000 kB
"#,
        );
        assert_eq!(mem.total_kib, Some(32_768_000));
        assert_eq!(mem.available_kib, Some(12_345_000));
    }

    #[test]
    fn parses_os_release() {
        let os = parse_os_release(
            r#"
NAME="Arch Linux"
VERSION="rolling"
PRETTY_NAME="Arch Linux"
"#,
        );
        assert_eq!(os.get("NAME").map(String::as_str), Some("Arch Linux"));
        assert_eq!(os.get("VERSION").map(String::as_str), Some("rolling"));
    }

    #[test]
    fn detects_card_names() {
        assert!(drm::is_card_name("card0"));
        assert!(!drm::is_card_name("card0-DP-1"));
        assert!(!drm::is_card_name("renderD128"));
    }
}
