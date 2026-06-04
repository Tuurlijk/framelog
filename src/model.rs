use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSample {
    pub ts_unix_ms: i64,
    pub device_pci: String,
    pub device_name: String,
    pub throttle_status_raw: Option<u32>,
    pub indep_throttle_status: u64,
    pub active_flags: Vec<String>,
    pub apu_power_mw: Option<u32>,
    pub stapm_limit_mw: Option<u16>,
    pub current_stapm_limit_mw: Option<u16>,
    pub temperature_core_max: Option<f64>,
    pub extra_json: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlagTransition {
    pub id: i64,
    pub ts_unix_ms: i64,
    pub sample_id: i64,
    pub device_pci: String,
    pub flag_name: String,
    pub old_value: bool,
    pub new_value: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEvent {
    pub id: i64,
    pub transition_id: i64,
    pub ts_unix_ms: i64,
    pub unit: Option<String>,
    pub priority: Option<i32>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlagSeriesPoint {
    pub ts_unix_ms: i64,
    pub value: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlagSeries {
    pub flag: String,
    pub points: Vec<FlagSeriesPoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextValue {
    pub key: String,
    pub value_num: Option<f64>,
    pub value_str: Option<String>,
}

impl ContextValue {
    pub fn num(key: impl Into<String>, v: f64) -> Self {
        Self {
            key: key.into(),
            value_num: Some(v),
            value_str: None,
        }
    }

    pub fn str(key: impl Into<String>, v: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value_num: None,
            value_str: Some(v.into()),
        }
    }

    pub fn to_json(&self) -> Value {
        if let Some(n) = self.value_num {
            json!(n)
        } else if let Some(s) = &self.value_str {
            json!(s)
        } else {
            Value::Null
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub source_id: String,
    pub ts_unix_ms: i64,
    pub health: String,
    pub values: Vec<ContextValue>,
    pub raw_json: String,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextTransition {
    pub id: i64,
    pub ts_unix_ms: i64,
    pub snapshot_id: i64,
    pub source_id: String,
    pub key: String,
    pub old_value: Value,
    pub new_value: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSeriesPoint {
    pub ts_unix_ms: i64,
    pub value: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextValueKind {
    Number,
    Enum,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSeries {
    pub key: String,
    pub value_kind: ContextValueKind,
    pub enum_labels: Option<std::collections::HashMap<String, f64>>,
    pub points: Vec<ContextSeriesPoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSeriesPoint {
    pub ts_unix_ms: i64,
    pub value: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSeries {
    pub metric: String,
    pub unit: String,
    pub points: Vec<MetricSeriesPoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlagCatalogEntry {
    pub name: String,
    pub category: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlagActivitySummary {
    pub flag_name: String,
    pub active_sample_pct: f64,
    pub assert_count: u64,
    pub clear_count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowSummary {
    pub from_ms: i64,
    pub to_ms: i64,
    pub sample_count: i64,
    pub throttle_transition_count: i64,
    pub context_transition_count: i64,
    pub flag_activity: Vec<FlagActivitySummary>,
    pub max_apu_power_mw: Option<u32>,
    pub avg_apu_power_mw: Option<u32>,
    pub max_temperature_core: Option<f64>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextSourceStatus {
    pub source_id: String,
    pub ts_unix_ms: i64,
    pub health: String,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimeBounds {
    pub min_ts_ms: Option<i64>,
    pub max_ts_ms: Option<i64>,
    pub sample_count: i64,
    pub context_snapshot_count: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextValueRow {
    pub snapshot_id: i64,
    pub ts_unix_ms: i64,
    pub source_id: String,
    pub key: String,
    pub value_num: Option<f64>,
    pub value_str: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub schema_version: u32,
    pub collected_at_ms: i64,
    pub health: String,
    pub warnings: Vec<String>,
    pub cpu: CpuInfo,
    pub gpus: Vec<GpuInfo>,
    pub memory: MemoryInfo,
    pub storage: Vec<StorageDeviceInfo>,
    pub bios: BiosInfo,
    pub os: OsInfo,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CpuInfo {
    pub vendor: Option<String>,
    pub model_name: Option<String>,
    pub logical_cpus: Option<u32>,
    pub physical_cores: Option<u32>,
    pub max_frequency_mhz: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GpuInfo {
    pub card: String,
    pub vendor_id: Option<String>,
    pub device_id: Option<String>,
    pub driver: Option<String>,
    pub pci_address: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_kib: Option<u64>,
    pub available_kib: Option<u64>,
    pub dimm_count: Option<u32>,
    pub configured_speed_mt_s: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StorageDeviceInfo {
    pub name: String,
    pub model: Option<String>,
    pub firmware_revision: Option<String>,
    pub size_bytes: Option<u64>,
    pub rotational: Option<bool>,
    pub logical_block_size: Option<u64>,
    pub physical_block_size: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BiosInfo {
    pub bios_vendor: Option<String>,
    pub bios_version: Option<String>,
    pub bios_date: Option<String>,
    pub board_vendor: Option<String>,
    pub board_name: Option<String>,
    pub product_name: Option<String>,
    pub product_version: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OsInfo {
    pub kernel_release: Option<String>,
    pub kernel_version: Option<String>,
    pub distro_name: Option<String>,
    pub distro_version: Option<String>,
    pub pretty_name: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureMarker {
    pub id: i64,
    pub ts_unix_ms: i64,
    pub label: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureSessionMeta {
    pub issue: String,
    pub duration_secs: u64,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub device_pci_filter: Option<String>,
    pub markers: Vec<CaptureMarker>,
    pub framework_issue_url: Option<String>,
}

pub const EXPORT_FORMAT: &str = "framelog_export";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Warning,
    Important,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindingConfidence {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub ts_unix_ms: Option<i64>,
    pub label: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub title: String,
    pub severity: FindingSeverity,
    pub confidence: FindingConfidence,
    pub summary: String,
    pub evidence: Vec<EvidenceItem>,
    pub related_throttle_transition_ids: Vec<i64>,
    pub related_context_transition_ids: Vec<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub schema_version: u32,
    pub analyzed_at_ms: i64,
    pub from_ms: i64,
    pub to_ms: i64,
    pub device_pci_filter: Option<String>,
    pub overall_verdict: String,
    pub findings: Vec<Finding>,
    pub warnings: Vec<String>,
    pub key_metrics: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnalysisOptions {
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    pub device_pci: Option<String>,
    pub issue_tag: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportBundle {
    pub schema_version: u32,
    pub format: String,
    pub exported_at_ms: i64,
    pub from_ms: i64,
    pub to_ms: i64,
    pub device_pci_filter: Option<String>,
    pub devices: Vec<(String, String)>,
    pub samples: Vec<MetricSample>,
    pub throttle_transitions: Vec<FlagTransition>,
    pub throttle_journal_events: Vec<JournalEvent>,
    pub context_snapshots: Vec<ContextSnapshot>,
    pub context_values: Vec<ContextValueRow>,
    pub context_transitions: Vec<ContextTransition>,
    pub context_journal_events: Vec<JournalEvent>,
    pub system: Option<SystemInfo>,
    pub summary: Option<WindowSummary>,
    pub export_warnings: Vec<String>,
    pub analysis: Option<AnalysisReport>,
}
