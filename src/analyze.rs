//! Deterministic Evidence Doctor analysis over export bundles.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

use crate::model::{
    AnalysisOptions, AnalysisReport, ContextTransition, ContextValueRow, EvidenceItem,
    ExportBundle, Finding, FindingConfidence, FindingSeverity, FlagTransition, JournalEvent,
    MetricSample, WindowSummary, EXPORT_FORMAT,
};
use crate::throttle;

const SCHEMA_VERSION: u32 = 1;
const MIN_SAMPLES: usize = 5;
const SPL_ACTIVE_PCT_THRESHOLD: f64 = 10.0;
const DGPU_SUSPENDED_PCT_THRESHOLD: f64 = 50.0;
const LOW_APU_POWER_MW: u32 = 45_000;
const LOW_PMF_SPL_MW: f64 = 40_000.0;
const THERMAL_FLAG_ACTIVE_PCT: f64 = 15.0;
const THERMAL_TEMP_C: f64 = 85.0;
const CPU_LOCK_545_LOW_MHZ: f64 = 500.0;
const CPU_LOCK_545_HIGH_MHZ: f64 = 620.0;
const CPU_LOCK_1400_LOW_MHZ: f64 = 1350.0;
const CPU_LOCK_1400_HIGH_MHZ: f64 = 1450.0;
const CPU_FREQ_LOCK_PCT: f64 = 50.0;

#[derive(Clone, Debug)]
struct AnalysisInput {
    from_ms: i64,
    to_ms: i64,
    device_pci: Option<String>,
    issue_tag: Option<String>,
    samples: Vec<MetricSample>,
    throttle_transitions: Vec<FlagTransition>,
    throttle_journal_events: Vec<JournalEvent>,
    context_values: Vec<ContextValueRow>,
    context_transitions: Vec<ContextTransition>,
    context_journal_events: Vec<JournalEvent>,
    system: Option<crate::model::SystemInfo>,
    summary: Option<WindowSummary>,
    export_warnings: Vec<String>,
}

pub fn validate_export_bundle(bundle: &ExportBundle) -> Result<(), String> {
    if bundle.format != EXPORT_FORMAT {
        return Err(format!(
            "unsupported export format: {} (expected {EXPORT_FORMAT})",
            bundle.format
        ));
    }
    if bundle.schema_version != 1 {
        return Err(format!(
            "unsupported schema_version: {} (expected 1)",
            bundle.schema_version
        ));
    }
    Ok(())
}

pub fn analyze_bundle(bundle: &ExportBundle, opts: &AnalysisOptions) -> AnalysisReport {
    let input = prepare_input(bundle, opts);
    run_analysis(input)
}

/// Attach a fresh Evidence Doctor report to an export bundle (e.g. after `build_export_bundle`).
pub fn attach_analysis(bundle: &mut ExportBundle, opts: &AnalysisOptions) {
    bundle.analysis = Some(analyze_bundle(bundle, opts));
}

pub fn render_markdown(report: &AnalysisReport) -> String {
    let mut out = String::new();
    out.push_str("# framelog analysis\n\n");
    out.push_str(&format!(
        "**Window:** {} ms → {} ms\n\n",
        report.from_ms, report.to_ms
    ));
    if let Some(dev) = &report.device_pci_filter {
        out.push_str(&format!("**Device filter:** `{dev}`\n\n"));
    }
    out.push_str(&format!("## Verdict\n\n{}\n\n", report.overall_verdict));

    if !report.warnings.is_empty() {
        out.push_str("## Data limitations\n\n");
        for w in &report.warnings {
            out.push_str(&format!("- {w}\n"));
        }
        out.push('\n');
    }

    if !report.key_metrics.is_empty() {
        out.push_str("## Key metrics\n\n");
        for (k, v) in &report.key_metrics {
            out.push_str(&format!("- **{k}:** {v}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Findings\n\n");
    if report.findings.is_empty() {
        out.push_str("_No findings for this window._\n");
    } else {
        for f in &report.findings {
            out.push_str(&format!(
                "### {} ({:?}, {:?} confidence)\n\n",
                f.title, f.severity, f.confidence
            ));
            out.push_str(&format!("{}\n\n", f.summary));
            if !f.evidence.is_empty() {
                out.push_str("Evidence:\n\n");
                for e in &f.evidence {
                    let ts = e
                        .ts_unix_ms
                        .map(|t| format!("{t} ms — "))
                        .unwrap_or_default();
                    out.push_str(&format!("- {ts}**{}:** {}\n", e.label, e.detail));
                }
                out.push('\n');
            }
        }
    }

    out.push_str(
        "\n---\n\nThis report uses deterministic rules over telemetry; it is not a definitive root-cause diagnosis.\n",
    );
    out
}

pub fn render_ai_prompt(report: &AnalysisReport) -> String {
    let mut out = String::from(
        "You are helping interpret a framelog export for Framework Laptop 16 power-limit debugging (issue #146).\n\
         Base your answer only on the facts below; say when evidence is missing.\n\n",
    );
    out.push_str(&format!(
        "Window: {} ms to {} ms\n",
        report.from_ms, report.to_ms
    ));
    out.push_str(&format!("Verdict: {}\n\n", report.overall_verdict));
    if !report.warnings.is_empty() {
        out.push_str("Limitations:\n");
        for w in &report.warnings {
            out.push_str(&format!("- {w}\n"));
        }
        out.push('\n');
    }
    out.push_str("Key metrics:\n");
    for (k, v) in &report.key_metrics {
        out.push_str(&format!("- {k}: {v}\n"));
    }
    out.push_str("\nFindings:\n");
    for f in &report.findings {
        out.push_str(&format!(
            "- [{} / {:?}] {} — {}\n",
            f.id, f.confidence, f.title, f.summary
        ));
    }
    out.push_str(
        "\nQuestions:\n\
         1. Does this match the known Framework #146 pattern (CPU capped while dGPU asleep)?\n\
         2. What additional evidence would increase confidence?\n\
         3. What should be attached to a GitHub bug report?\n",
    );
    out
}

fn prepare_input(bundle: &ExportBundle, opts: &AnalysisOptions) -> AnalysisInput {
    let from_ms = opts.from_ms.unwrap_or(bundle.from_ms);
    let to_ms = opts.to_ms.unwrap_or(bundle.to_ms);
    let device_pci = opts
        .device_pci
        .clone()
        .or_else(|| bundle.device_pci_filter.clone());

    let samples: Vec<_> = bundle
        .samples
        .iter()
        .filter(|s| in_range(s.ts_unix_ms, from_ms, to_ms))
        .filter(|s| device_matches(&s.device_pci, device_pci.as_deref()))
        .cloned()
        .collect();

    let throttle_transitions: Vec<_> = bundle
        .throttle_transitions
        .iter()
        .filter(|t| in_range(t.ts_unix_ms, from_ms, to_ms))
        .filter(|t| device_matches(&t.device_pci, device_pci.as_deref()))
        .cloned()
        .collect();

    let context_values: Vec<_> = bundle
        .context_values
        .iter()
        .filter(|v| in_range(v.ts_unix_ms, from_ms, to_ms))
        .cloned()
        .collect();

    let context_transitions: Vec<_> = bundle
        .context_transitions
        .iter()
        .filter(|t| in_range(t.ts_unix_ms, from_ms, to_ms))
        .cloned()
        .collect();

    let throttle_journal_events: Vec<_> = bundle
        .throttle_journal_events
        .iter()
        .filter(|e| in_range(e.ts_unix_ms, from_ms, to_ms))
        .cloned()
        .collect();

    let context_journal_events: Vec<_> = bundle
        .context_journal_events
        .iter()
        .filter(|e| in_range(e.ts_unix_ms, from_ms, to_ms))
        .cloned()
        .collect();

    let summary = bundle
        .summary
        .as_ref()
        .filter(|s| {
            s.from_ms == from_ms
                && s.to_ms == to_ms
                && bundle.device_pci_filter.as_ref() == device_pci.as_ref()
        })
        .cloned()
        .or_else(|| {
            if samples.is_empty() {
                None
            } else {
                Some(crate::summary::summarize_window(
                    from_ms,
                    to_ms,
                    &samples,
                    &throttle_transitions,
                    &context_transitions,
                ))
            }
        });

    AnalysisInput {
        from_ms,
        to_ms,
        device_pci,
        issue_tag: opts.issue_tag.clone(),
        samples,
        throttle_transitions,
        throttle_journal_events,
        context_values,
        context_transitions,
        context_journal_events,
        system: bundle.system.clone(),
        summary,
        export_warnings: bundle.export_warnings.clone(),
    }
}

fn run_analysis(mut input: AnalysisInput) -> AnalysisReport {
    let analyzed_at_ms = crate::util::current_ts_ms();
    let mut warnings = std::mem::take(&mut input.export_warnings);
    let mut findings = Vec::new();
    let mut key_metrics = BTreeMap::new();

    if input.samples.len() < MIN_SAMPLES {
        warnings.push(format!(
            "Only {} samples in window (need at least {MIN_SAMPLES} for reliable analysis).",
            input.samples.len()
        ));
    }

    if input.system.is_none() {
        warnings.push("No system inventory in export; machine context is unknown.".into());
    }

    if !input.samples.is_empty() && input.samples.iter().all(|s| s.apu_power_mw.is_none()) {
        warnings.push(
            "No package power readings in samples. On AMD APUs gpu_metrics v2.x may expose power as average_socket_power only; re-collect with a current framelog build or inspect extra_json.".into(),
        );
    }

    let amd_throttle = samples_have_amd_throttle(&input.samples);
    if !amd_throttle && !input.samples.is_empty() {
        warnings.push(
            "No AMD throttle flags in this window; SPL/PMF/Framework #146 rules are skipped (Intel or non-AMD collector)."
                .into(),
        );
    }

    let spl_pct = if amd_throttle {
        flag_active_pct(&input, "SPL")
    } else {
        None
    };
    let _prochot_pct = flag_active_pct(&input, "PROCHOT_CPU");
    let max_pwr = input
        .summary
        .as_ref()
        .and_then(|s| s.max_apu_power_mw)
        .or_else(|| input.samples.iter().filter_map(|s| s.apu_power_mw).max());
    let avg_pwr = input
        .summary
        .as_ref()
        .and_then(|s| s.avg_apu_power_mw)
        .or_else(|| {
            let powers: Vec<u32> = input
                .samples
                .iter()
                .filter_map(|s| s.apu_power_mw)
                .collect();
            if powers.is_empty() {
                None
            } else {
                let sum: u64 = powers.iter().map(|v| u64::from(*v)).sum();
                Some((sum / powers.len() as u64) as u32)
            }
        });
    let max_temp = input
        .summary
        .as_ref()
        .and_then(|s| s.max_temperature_core)
        .or_else(|| {
            input
                .samples
                .iter()
                .filter_map(|s| s.temperature_core_max)
                .max_by(f64::total_cmp)
        });

    if let Some(p) = max_pwr {
        key_metrics.insert("max_apu_power_mw".into(), p.to_string());
    }
    if let Some(p) = avg_pwr {
        key_metrics.insert("avg_apu_power_mw".into(), p.to_string());
    }
    if let Some(t) = max_temp {
        key_metrics.insert("max_temperature_core_c".into(), format!("{t:.1}"));
    }
    if let Some(pct) = spl_pct {
        key_metrics.insert("spl_active_sample_pct".into(), format!("{pct:.1}"));
    }

    let dgpu_suspended_pct = context_bool_pct(
        &input.context_values,
        "gpu_power.dgpu_runtime_suspended",
        1.0,
    );
    let dgpu_status = latest_context_str(&input.context_values, "gpu_power.dgpu_runtime_status");
    let ac_connected = latest_context_bool(&input.context_values, "power.ac_connected");
    let profile = latest_context_str(&input.context_values, "profile.active");
    let spl_mw = latest_context_num(&input.context_values, "pmf.spl_mw");
    let pmf_present =
        spl_mw.is_some() || latest_context_num(&input.context_values, "pmf.sppt_mw").is_some();

    if let Some(pct) = dgpu_suspended_pct {
        key_metrics.insert("dgpu_runtime_suspended_pct".into(), format!("{pct:.1}"));
    }
    if let Some(v) = ac_connected {
        key_metrics.insert("ac_connected".into(), v.to_string());
    }
    if let Some(p) = profile {
        key_metrics.insert("power_profile".into(), p);
    }
    if let Some(v) = spl_mw {
        key_metrics.insert("pmf_spl_mw".into(), format!("{v:.0}"));
    }

    let pct_cpu_545 = crate::context::cpu::context_freq_band_pct(
        &input.context_values,
        CPU_LOCK_545_LOW_MHZ,
        CPU_LOCK_545_HIGH_MHZ,
    );
    let pct_cpu_1400 = crate::context::cpu::context_freq_band_pct(
        &input.context_values,
        CPU_LOCK_1400_LOW_MHZ,
        CPU_LOCK_1400_HIGH_MHZ,
    );
    let latest_cpu_min_mhz = latest_context_num(&input.context_values, "cpu.cur_freq_min_mhz");
    if let Some(v) = latest_cpu_min_mhz {
        key_metrics.insert("cpu_cur_freq_min_mhz".into(), format!("{v:.0}"));
    }
    if let Some(pct) = pct_cpu_545 {
        key_metrics.insert("cpu_freq_545_band_pct".into(), format!("{pct:.1}"));
    }
    if let Some(pct) = pct_cpu_1400 {
        key_metrics.insert("cpu_freq_1400_band_pct".into(), format!("{pct:.1}"));
    }

    if pct_cpu_545.is_some_and(|p| p >= CPU_FREQ_LOCK_PCT) {
        let pct = pct_cpu_545.unwrap();
        findings.push(Finding {
            id: "cpu_frequency_lock".into(),
            title: "CPU frequency locked near 544–545 MHz".into(),
            severity: FindingSeverity::Warning,
            confidence: if pct >= 80.0 {
                FindingConfidence::High
            } else {
                FindingConfidence::Medium
            },
            summary: format!(
                "Minimum CPU frequency stayed in the 544–545 MHz band for {pct:.0}% of cpufreq samples. This matches the Framework BIOS 4.04 long-idle frequency-lock report and is separate from the 35W SPL cap pattern."
            ),
            evidence: vec![
                EvidenceItem {
                    ts_unix_ms: latest_context_ts(&input.context_values, "cpu.cur_freq_min_mhz"),
                    label: "cpu.cur_freq_min_mhz (latest)".into(),
                    detail: latest_cpu_min_mhz
                        .map(|v| format!("{v:.0} MHz"))
                        .unwrap_or_else(|| "unknown".into()),
                },
                EvidenceItem {
                    ts_unix_ms: None,
                    label: "cpu_freq_545_band_pct".into(),
                    detail: format!("{pct:.1}%"),
                },
            ],
            related_throttle_transition_ids: vec![],
            related_context_transition_ids: related_context_ids(
                &input.context_transitions,
                "cpu.",
            ),
        });
    } else if pct_cpu_1400.is_some_and(|p| p >= CPU_FREQ_LOCK_PCT) {
        let pct = pct_cpu_1400.unwrap();
        findings.push(Finding {
            id: "cpu_frequency_lock".into(),
            title: "CPU frequency locked near 1400 MHz".into(),
            severity: FindingSeverity::Warning,
            confidence: if pct >= 80.0 {
                FindingConfidence::High
            } else {
                FindingConfidence::Medium
            },
            summary: format!(
                "Minimum CPU frequency stayed in the 1350–1450 MHz band for {pct:.0}% of cpufreq samples. Some Framework AMD laptops report this alternate lock state after long idle."
            ),
            evidence: vec![
                EvidenceItem {
                    ts_unix_ms: latest_context_ts(&input.context_values, "cpu.cur_freq_min_mhz"),
                    label: "cpu.cur_freq_min_mhz (latest)".into(),
                    detail: latest_cpu_min_mhz
                        .map(|v| format!("{v:.0} MHz"))
                        .unwrap_or_else(|| "unknown".into()),
                },
                EvidenceItem {
                    ts_unix_ms: None,
                    label: "cpu_freq_1400_band_pct".into(),
                    detail: format!("{pct:.1}%"),
                },
            ],
            related_throttle_transition_ids: vec![],
            related_context_transition_ids: related_context_ids(
                &input.context_transitions,
                "cpu.",
            ),
        });
    }

    // PMF missing (AMD firmware correlation)
    if amd_throttle && !pmf_present {
        let has_pmf_health = input
            .context_values
            .iter()
            .any(|v| v.source_id == "pmf" || v.key.starts_with("pmf."));
        if !has_pmf_health {
            warnings.push(
                "PMF debugfs limits not present in export (pmf.spl_mw / pmf.sppt_mw missing)."
                    .into(),
            );
        }
        findings.push(Finding {
            id: "pmf_missing".into(),
            title: "PMF limits unavailable".into(),
            severity: FindingSeverity::Info,
            confidence: FindingConfidence::High,
            summary: "AMD PMF power limits were not recorded. SPL/SPPT/FPPT correlation with firmware caps cannot be assessed.".into(),
            evidence: vec![EvidenceItem {
                ts_unix_ms: None,
                label: "pmf".into(),
                detail: "No pmf.* context values in selected window".into(),
            }],
            related_throttle_transition_ids: vec![],
            related_context_transition_ids: vec![],
        });
    }

    // AC / adapter uncertain
    if ac_connected != Some(true) {
        let conf = if ac_connected.is_none() {
            FindingConfidence::Medium
        } else {
            FindingConfidence::High
        };
        findings.push(Finding {
            id: "adapter_or_ac_uncertain".into(),
            title: "AC power context unclear or on battery".into(),
            severity: FindingSeverity::Warning,
            confidence: conf,
            summary: "Charger/AC state is missing or shows not on AC. Power-cap interpretation differs on battery.".into(),
            evidence: vec![EvidenceItem {
                ts_unix_ms: latest_context_ts(&input.context_values, "power.ac_connected"),
                label: "power.ac_connected".into(),
                detail: ac_connected
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unknown".into()),
            }],
            related_throttle_transition_ids: vec![],
            related_context_transition_ids: related_context_ids(&input.context_transitions, "power."),
        });
    }

    let prochot_suspect = amd_throttle && throttle::prochot_suspect_sticky_legacy(&input.samples);
    if prochot_suspect {
        warnings.push(throttle::prochot_dashboard_warning().into());
        findings.push(Finding {
            id: "prochot_suspect_legacy_status".into(),
            title: "PROCHOT flags may be sticky legacy status".into(),
            severity: FindingSeverity::Info,
            confidence: FindingConfidence::High,
            summary: "PROCHOT_CPU and PROCHOT_GPU are always on with throttle_status_raw 1536 (legacy bits 9+10) and no transitions. This pattern is common on gpu_metrics v2.1 APUs and may not indicate real emergency thermal throttling.".into(),
            evidence: vec![
                EvidenceItem {
                    ts_unix_ms: None,
                    label: "throttle_status_raw".into(),
                    detail: "often 1536 (0x600) on Radeon 780M / Yellow Carp".into(),
                },
                EvidenceItem {
                    ts_unix_ms: None,
                    label: "documentation".into(),
                    detail: "docs/prochot-status-investigation.md".into(),
                },
            ],
            related_throttle_transition_ids: vec![],
            related_context_transition_ids: vec![],
        });
    }

    // Thermal shape (exclude sticky legacy PROCHOT from thermal-family aggregation)
    let thermal_flags: &[&str] = if prochot_suspect {
        &["TEMP_HOTSPOT", "TEMP_CORE"]
    } else {
        &["PROCHOT_CPU", "PROCHOT_GPU", "TEMP_HOTSPOT", "TEMP_CORE"]
    };
    let mut thermal_active = 0.0f64;
    if amd_throttle {
        for f in thermal_flags {
            if let Some(p) = flag_active_pct(&input, f) {
                thermal_active = thermal_active.max(p);
            }
        }
    }
    let thermal_dominant = (amd_throttle && thermal_active >= THERMAL_FLAG_ACTIVE_PCT)
        || max_temp.is_some_and(|t| t >= THERMAL_TEMP_C);

    if thermal_dominant {
        let mut evidence = vec![];
        if let Some(t) = max_temp {
            evidence.push(EvidenceItem {
                ts_unix_ms: None,
                label: "max_temperature_core".into(),
                detail: format!("{t:.1} °C"),
            });
        }
        if thermal_active > 0.0 {
            evidence.push(EvidenceItem {
                ts_unix_ms: None,
                label: "thermal_flags".into(),
                detail: format!(
                    "thermal-family flags active up to {thermal_active:.1}% of samples"
                ),
            });
        }
        findings.push(Finding {
            id: "thermal_limit_shape".into(),
            title: "Thermal limiting evidence present".into(),
            severity: FindingSeverity::Important,
            confidence: FindingConfidence::Medium,
            summary: "Thermal prochot or high core temperature appears in this window. This weakens a pure firmware SPL-cap hypothesis.".into(),
            evidence,
            related_throttle_transition_ids: related_throttle_ids(&input.throttle_transitions, thermal_flags),
            related_context_transition_ids: vec![],
        });
    }

    // Power limit without thermal
    let spl_asserted = amd_throttle
        && (spl_pct.is_some_and(|p| p >= SPL_ACTIVE_PCT_THRESHOLD)
            || input
                .throttle_transitions
                .iter()
                .any(|t| t.flag_name == "SPL" && t.new_value));
    let power_flags = ["SPL", "SPPT", "FPPT", "STAPM"];
    if amd_throttle && spl_asserted && !thermal_dominant {
        findings.push(Finding {
            id: "power_limit_without_thermal".into(),
            title: "Power-limit throttle without strong thermal signal".into(),
            severity: FindingSeverity::Important,
            confidence: FindingConfidence::Medium,
            summary: "SPL (or related package power tracking) is active while thermal flags and peak temperature do not dominate.".into(),
            evidence: {
                let mut ev = vec![];
                if let Some(p) = spl_pct {
                    ev.push(EvidenceItem {
                        ts_unix_ms: None,
                        label: "SPL".into(),
                        detail: format!("active on {p:.1}% of samples"),
                    });
                }
                if let Some(p) = max_pwr {
                    ev.push(EvidenceItem {
                        ts_unix_ms: None,
                        label: "max_apu_power_mw".into(),
                        detail: p.to_string(),
                    });
                }
                ev
            },
            related_throttle_transition_ids: related_throttle_ids(&input.throttle_transitions, &power_flags),
            related_context_transition_ids: vec![],
        });
    }

    // Framework #146 shape
    let dgpu_asleep = dgpu_suspended_pct.is_some_and(|p| p >= DGPU_SUSPENDED_PCT_THRESHOLD)
        || dgpu_status.as_deref() == Some("suspended")
        || dgpu_status.as_deref() == Some("inactive");
    let apu_capped = max_pwr.is_some_and(|p| p <= LOW_APU_POWER_MW);
    let pmf_low = spl_mw.is_some_and(|v| v <= LOW_PMF_SPL_MW);
    let spl_active = spl_asserted;

    let mut score = 0u32;
    if dgpu_asleep {
        score += 1;
    }
    if spl_active {
        score += 1;
    }
    if apu_capped {
        score += 1;
    }
    if ac_connected == Some(true) {
        score += 1;
    }
    if pmf_low {
        score += 1;
    }
    if !thermal_dominant {
        score += 1;
    }

    if amd_throttle && score >= 4 {
        let confidence = if score >= 5 && pmf_present {
            FindingConfidence::High
        } else if score >= 4 {
            FindingConfidence::Medium
        } else {
            FindingConfidence::Low
        };
        findings.push(Finding {
            id: "framework_146_shape".into(),
            title: "Matches Framework issue #146 failure shape".into(),
            severity: FindingSeverity::Important,
            confidence,
            summary: "dGPU appears inactive/suspended while package power limit (SPL) and/or low APU power suggest CPU package capping with the discrete GPU asleep.".into(),
            evidence: {
                let mut ev = vec![];
                if let Some(s) = &dgpu_status {
                    ev.push(EvidenceItem {
                        ts_unix_ms: latest_context_ts(&input.context_values, "gpu_power.dgpu_runtime_status"),
                        label: "dgpu_runtime_status".into(),
                        detail: s.clone(),
                    });
                }
                if let Some(p) = dgpu_suspended_pct {
                    ev.push(EvidenceItem {
                        ts_unix_ms: None,
                        label: "dgpu_runtime_suspended".into(),
                        detail: format!("suspended on {p:.1}% of context samples"),
                    });
                }
                if let Some(p) = spl_pct {
                    ev.push(EvidenceItem {
                        ts_unix_ms: None,
                        label: "SPL".into(),
                        detail: format!("active {p:.1}% of samples"),
                    });
                }
                if let Some(p) = max_pwr {
                    ev.push(EvidenceItem {
                        ts_unix_ms: None,
                        label: "max_apu_power_mw".into(),
                        detail: format!("{p} mW (threshold {LOW_APU_POWER_MW})"),
                    });
                }
                if let Some(v) = spl_mw {
                    ev.push(EvidenceItem {
                        ts_unix_ms: latest_context_ts(&input.context_values, "pmf.spl_mw"),
                        label: "pmf.spl_mw".into(),
                        detail: format!("{v:.0} mW"),
                    });
                }
                ev
            },
            related_throttle_transition_ids: related_throttle_ids(&input.throttle_transitions, &["SPL", "PROCHOT_CPU"]),
            related_context_transition_ids: related_context_ids(&input.context_transitions, "gpu_power."),
        });
    }

    // Sleep / wake correlation
    let sleep_related: Vec<_> = input
        .context_transitions
        .iter()
        .filter(|t| {
            t.key.starts_with("system.")
                || t.key.starts_with("gpu_power.")
                || t.key.starts_with("profile.")
        })
        .collect();
    if sleep_related.len() >= 2 {
        let ids: Vec<i64> = sleep_related.iter().map(|t| t.id).collect();
        findings.push(Finding {
            id: "sleep_wake_correlation".into(),
            title: "Sleep, wake, or runtime/profile transitions in window".into(),
            severity: FindingSeverity::Info,
            confidence: FindingConfidence::Medium,
            summary: format!(
                "{} system/gpu/profile context transitions occurred; correlate throttle changes with these timestamps.",
                sleep_related.len()
            ),
            evidence: sleep_related
                .iter()
                .take(8)
                .map(|t| EvidenceItem {
                    ts_unix_ms: Some(t.ts_unix_ms),
                    label: t.key.clone(),
                    detail: format!("{} -> {}", value_short(&t.old_value), value_short(&t.new_value)),
                })
                .collect(),
            related_throttle_transition_ids: vec![],
            related_context_transition_ids: ids,
        });
    }

    enrich_findings(&mut findings, &input);
    findings.sort_by_key(|f| std::cmp::Reverse(finding_rank(f)));

    let overall_verdict = if findings.iter().any(|f| f.id == "framework_146_shape") {
        "Likely matches Framework #146 power-cap pattern while dGPU is inactive; review findings and attach export.json.".into()
    } else if findings.iter().any(|f| f.id == "cpu_frequency_lock") {
        "CPU frequency appears locked in a low band for much of this window; see cpu_frequency_lock and docs/framework-146-long-idle-capture.md.".into()
    } else if findings.iter().any(|f| f.id == "thermal_limit_shape") {
        "Thermal limiting appears more prominent than a pure firmware SPL cap in this window."
            .into()
    } else if findings.is_empty() {
        "No strong diagnostic pattern detected in this window.".into()
    } else {
        "Mixed or inconclusive signals; see findings and data limitations.".into()
    };

    if input.issue_tag.as_deref() == Some("framework-146")
        && !findings.iter().any(|f| f.id == "framework_146_shape")
    {
        warnings.push(
            "Capture tagged framework-146 but automated rules did not reach high confidence for the #146 shape.".into(),
        );
    }

    AnalysisReport {
        schema_version: SCHEMA_VERSION,
        analyzed_at_ms,
        from_ms: input.from_ms,
        to_ms: input.to_ms,
        device_pci_filter: input.device_pci,
        overall_verdict,
        findings,
        warnings,
        key_metrics,
    }
}

fn enrich_findings(findings: &mut [Finding], input: &AnalysisInput) {
    for finding in findings {
        if !finding.related_throttle_transition_ids.is_empty() {
            finding.evidence.push(EvidenceItem {
                ts_unix_ms: None,
                label: "related_throttle_transition_ids".into(),
                detail: compact_ids(&finding.related_throttle_transition_ids),
            });
        }
        if !finding.related_context_transition_ids.is_empty() {
            finding.evidence.push(EvidenceItem {
                ts_unix_ms: None,
                label: "related_context_transition_ids".into(),
                detail: compact_ids(&finding.related_context_transition_ids),
            });
        }

        for (journal_count, event) in input
            .throttle_journal_events
            .iter()
            .filter(|e| {
                finding
                    .related_throttle_transition_ids
                    .contains(&e.transition_id)
            })
            .chain(input.context_journal_events.iter().filter(|e| {
                finding
                    .related_context_transition_ids
                    .contains(&e.transition_id)
            }))
            .enumerate()
        {
            if journal_count >= 8 {
                finding.evidence.push(EvidenceItem {
                    ts_unix_ms: None,
                    label: "journal.more".into(),
                    detail: "Additional journal messages omitted from this finding; inspect the timeline for the full set.".into(),
                });
                break;
            }
            finding.evidence.push(EvidenceItem {
                ts_unix_ms: Some(event.ts_unix_ms),
                label: format!("journal.{}", event.unit.as_deref().unwrap_or("system")),
                detail: shorten(&event.message, 240),
            });
        }
    }
}

fn compact_ids(ids: &[i64]) -> String {
    let mut out = ids
        .iter()
        .take(12)
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if ids.len() > 12 {
        out.push_str(&format!(" … (+{} more)", ids.len() - 12));
    }
    out
}

fn shorten(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out = s
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn finding_rank(f: &Finding) -> u8 {
    let sev = match f.severity {
        FindingSeverity::Important => 3,
        FindingSeverity::Warning => 2,
        FindingSeverity::Info => 1,
    };
    let conf = match f.confidence {
        FindingConfidence::High => 3,
        FindingConfidence::Medium => 2,
        FindingConfidence::Low => 1,
    };
    sev * 10 + conf
}

fn samples_have_amd_throttle(samples: &[crate::model::MetricSample]) -> bool {
    samples
        .iter()
        .any(|s| !s.active_flags.is_empty() || s.indep_throttle_status != 0)
}

fn flag_active_pct(input: &AnalysisInput, flag: &str) -> Option<f64> {
    if let Some(s) = &input.summary {
        if let Some(f) = s.flag_activity.iter().find(|a| a.flag_name == flag) {
            return Some(f.active_sample_pct);
        }
    }
    if input.samples.is_empty() {
        return None;
    }
    let active = input
        .samples
        .iter()
        .filter(|s| s.active_flags.iter().any(|f| f == flag))
        .count();
    Some((active as f64 / input.samples.len() as f64) * 100.0)
}

fn context_bool_pct(rows: &[ContextValueRow], key: &str, when: f64) -> Option<f64> {
    let vals: Vec<_> = rows.iter().filter(|r| r.key == key).collect();
    if vals.is_empty() {
        return None;
    }
    let hit = vals
        .iter()
        .filter(|r| {
            r.value_num.is_some_and(|n| (n - when).abs() < 0.5)
                || r.value_str.as_deref().is_some_and(is_truthy_str)
        })
        .count();
    Some((hit as f64 / vals.len() as f64) * 100.0)
}

fn latest_context_num(rows: &[ContextValueRow], key: &str) -> Option<f64> {
    rows.iter()
        .filter(|r| r.key == key)
        .max_by_key(|r| r.ts_unix_ms)
        .and_then(|r| r.value_num)
}

fn latest_context_str(rows: &[ContextValueRow], key: &str) -> Option<String> {
    rows.iter()
        .filter(|r| r.key == key)
        .max_by_key(|r| r.ts_unix_ms)
        .and_then(|r| r.value_str.clone())
}

fn latest_context_bool(rows: &[ContextValueRow], key: &str) -> Option<bool> {
    let row = rows
        .iter()
        .filter(|r| r.key == key)
        .max_by_key(|r| r.ts_unix_ms)?;
    if let Some(n) = row.value_num {
        return Some(n >= 0.5);
    }
    row.value_str.as_deref().map(is_truthy_str)
}

fn latest_context_ts(rows: &[ContextValueRow], key: &str) -> Option<i64> {
    rows.iter()
        .filter(|r| r.key == key)
        .max_by_key(|r| r.ts_unix_ms)
        .map(|r| r.ts_unix_ms)
}

fn is_truthy_str(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        "true" | "1" | "yes" | "on" | "connected"
    )
}

fn related_throttle_ids(transitions: &[FlagTransition], flags: &[&str]) -> Vec<i64> {
    let names: HashSet<&str> = flags.iter().copied().collect();
    transitions
        .iter()
        .filter(|t| names.contains(t.flag_name.as_str()))
        .map(|t| t.id)
        .collect()
}

fn related_context_ids(transitions: &[ContextTransition], prefix: &str) -> Vec<i64> {
    transitions
        .iter()
        .filter(|t| t.key.starts_with(prefix))
        .map(|t| t.id)
        .collect()
}

fn value_short(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => v.to_string(),
    }
}

fn in_range(ts: i64, from: i64, to: i64) -> bool {
    ts >= from && ts <= to
}

fn device_matches(device_pci: &str, filter: Option<&str>) -> bool {
    filter.is_none_or(|f| f == device_pci)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MetricSample, SystemInfo};

    fn sample(ts: i64, flags: &[&str], power: u32) -> MetricSample {
        MetricSample {
            ts_unix_ms: ts,
            device_pci: "0000:00:00.0".into(),
            device_name: "test".into(),
            throttle_status_raw: None,
            indep_throttle_status: 0,
            active_flags: flags.iter().map(|s| (*s).to_string()).collect(),
            apu_power_mw: Some(power),
            stapm_limit_mw: None,
            current_stapm_limit_mw: None,
            temperature_core_max: Some(65.0),
            extra_json: None,
        }
    }

    fn ctx(ts: i64, key: &str, num: Option<f64>, s: Option<&str>) -> ContextValueRow {
        ContextValueRow {
            snapshot_id: 1,
            ts_unix_ms: ts,
            source_id: key.split('.').next().unwrap_or("x").into(),
            key: key.into(),
            value_num: num,
            value_str: s.map(str::to_string),
        }
    }

    #[test]
    fn framework_146_shape_detected() {
        let bundle = ExportBundle {
            schema_version: 1,
            format: EXPORT_FORMAT.into(),
            exported_at_ms: 0,
            from_ms: 0,
            to_ms: 10_000,
            device_pci_filter: None,
            devices: vec![],
            samples: (0..20).map(|i| sample(i * 500, &["SPL"], 32_000)).collect(),
            throttle_transitions: vec![FlagTransition {
                id: 1,
                ts_unix_ms: 1000,
                sample_id: 1,
                device_pci: "0000:00:00.0".into(),
                flag_name: "SPL".into(),
                old_value: false,
                new_value: true,
            }],
            throttle_journal_events: vec![],
            context_snapshots: vec![],
            context_values: vec![
                ctx(500, "gpu_power.dgpu_runtime_suspended", Some(1.0), None),
                ctx(
                    500,
                    "gpu_power.dgpu_runtime_status",
                    None,
                    Some("suspended"),
                ),
                ctx(500, "power.ac_connected", Some(1.0), None),
                ctx(500, "pmf.spl_mw", Some(35_000.0), None),
            ],
            context_transitions: vec![],
            context_journal_events: vec![],
            system: Some(SystemInfo {
                schema_version: 1,
                collected_at_ms: 0,
                health: "ok".into(),
                warnings: vec![],
                cpu: Default::default(),
                gpus: vec![],
                memory: Default::default(),
                storage: vec![],
                bios: Default::default(),
                os: Default::default(),
            }),
            summary: None,
            export_warnings: vec![],
            analysis: None,
        };
        let report = analyze_bundle(&bundle, &AnalysisOptions::default());
        assert!(report
            .findings
            .iter()
            .any(|f| f.id == "framework_146_shape"));
        assert!(report.overall_verdict.contains("#146"));
    }

    #[test]
    fn prochot_suspect_legacy_finding() {
        let samples: Vec<MetricSample> = (0..10)
            .map(|i| {
                let mut s = sample(i * 500, &["PROCHOT_CPU", "PROCHOT_GPU"], 40_000);
                s.throttle_status_raw = Some(throttle::LEGACY_YELLOW_CARP_PROCHOT_RAW);
                s
            })
            .collect();
        let bundle = ExportBundle {
            schema_version: 1,
            format: EXPORT_FORMAT.into(),
            exported_at_ms: 0,
            from_ms: 0,
            to_ms: 5000,
            device_pci_filter: None,
            devices: vec![],
            samples,
            throttle_transitions: vec![],
            throttle_journal_events: vec![],
            context_snapshots: vec![],
            context_values: vec![],
            context_transitions: vec![],
            context_journal_events: vec![],
            system: None,
            summary: None,
            export_warnings: vec![],
            analysis: None,
        };
        let report = analyze_bundle(&bundle, &AnalysisOptions::default());
        assert!(report
            .findings
            .iter()
            .any(|f| f.id == "prochot_suspect_legacy_status"));
        assert!(report.warnings.iter().any(|w| w.contains("PROCHOT_CPU")));
    }

    #[test]
    fn cpu_frequency_lock_545_detected() {
        let mut rows = Vec::new();
        for i in 0..20 {
            rows.push(ctx(
                i * 500,
                "cpu.cur_freq_min_mhz",
                Some(544.0 + (i % 2) as f64),
                None,
            ));
        }
        let bundle = ExportBundle {
            schema_version: 1,
            format: EXPORT_FORMAT.into(),
            exported_at_ms: 0,
            from_ms: 0,
            to_ms: 10_000,
            device_pci_filter: None,
            devices: vec![],
            samples: (0..20).map(|i| sample(i * 500, &[], 40_000)).collect(),
            throttle_transitions: vec![],
            throttle_journal_events: vec![],
            context_snapshots: vec![],
            context_values: rows,
            context_transitions: vec![],
            context_journal_events: vec![],
            system: None,
            summary: None,
            export_warnings: vec![],
            analysis: None,
        };
        let report = analyze_bundle(&bundle, &AnalysisOptions::default());
        assert!(report.findings.iter().any(|f| f.id == "cpu_frequency_lock"));
        assert!(report.overall_verdict.contains("frequency"));
    }

    #[test]
    fn pmf_missing_finding() {
        let bundle = ExportBundle {
            schema_version: 1,
            format: EXPORT_FORMAT.into(),
            exported_at_ms: 0,
            from_ms: 0,
            to_ms: 5000,
            device_pci_filter: None,
            devices: vec![],
            samples: (0..10).map(|i| sample(i * 500, &["SPL"], 20_000)).collect(),
            throttle_transitions: vec![],
            throttle_journal_events: vec![],
            context_snapshots: vec![],
            context_values: vec![ctx(0, "power.ac_connected", Some(1.0), None)],
            context_transitions: vec![],
            context_journal_events: vec![],
            system: None,
            summary: None,
            export_warnings: vec![],
            analysis: None,
        };
        let report = analyze_bundle(&bundle, &AnalysisOptions::default());
        assert!(report.findings.iter().any(|f| f.id == "pmf_missing"));
    }

    #[test]
    fn ignores_bundle_summary_when_device_filter_differs() {
        let mut fast_gpu = sample(0, &[], 50_000);
        fast_gpu.device_pci = "0000:c1:00.0".into();
        let slow_gpu = sample(5000, &[], 10_000);
        let all_samples = vec![fast_gpu.clone(), slow_gpu.clone()];
        let bundle = ExportBundle {
            schema_version: 1,
            format: EXPORT_FORMAT.into(),
            exported_at_ms: 0,
            from_ms: 0,
            to_ms: 10_000,
            device_pci_filter: None,
            devices: vec![],
            samples: all_samples.clone(),
            throttle_transitions: vec![],
            throttle_journal_events: vec![],
            context_snapshots: vec![],
            context_values: vec![],
            context_transitions: vec![],
            context_journal_events: vec![],
            system: None,
            summary: Some(crate::summary::summarize_window(
                0,
                10_000,
                &all_samples,
                &[],
                &[],
            )),
            export_warnings: vec![],
            analysis: None,
        };
        let report = analyze_bundle(
            &bundle,
            &AnalysisOptions {
                device_pci: Some("0000:00:00.0".into()),
                ..AnalysisOptions::default()
            },
        );
        assert_eq!(
            report
                .key_metrics
                .get("max_apu_power_mw")
                .map(String::as_str),
            Some("10000")
        );
    }

    #[test]
    fn related_throttle_ids_match_exact_flag_names() {
        let transitions = vec![
            FlagTransition {
                id: 1,
                ts_unix_ms: 0,
                sample_id: 1,
                device_pci: "pci".into(),
                flag_name: "SPL".into(),
                old_value: false,
                new_value: true,
            },
            FlagTransition {
                id: 2,
                ts_unix_ms: 0,
                sample_id: 1,
                device_pci: "pci".into(),
                flag_name: "XSPL".into(),
                old_value: false,
                new_value: true,
            },
        ];
        let ids = related_throttle_ids(&transitions, &["SPL"]);
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn validate_rejects_wrong_format() {
        let mut bundle = ExportBundle {
            schema_version: 1,
            format: EXPORT_FORMAT.into(),
            exported_at_ms: 0,
            from_ms: 0,
            to_ms: 1,
            device_pci_filter: None,
            devices: vec![],
            samples: vec![sample(0, &[], 1)],
            throttle_transitions: vec![],
            throttle_journal_events: vec![],
            context_snapshots: vec![],
            context_values: vec![],
            context_transitions: vec![],
            context_journal_events: vec![],
            system: None,
            summary: None,
            export_warnings: vec![],
            analysis: None,
        };
        assert!(validate_export_bundle(&bundle).is_ok());
        bundle.format = "other".into();
        assert!(validate_export_bundle(&bundle).is_err());
    }
}
