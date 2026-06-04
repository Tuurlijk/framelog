use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::analyze;
use crate::collector::{make_collector, CollectorKind};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::model::{AnalysisOptions, AnalysisReport, CaptureSessionMeta, ExportBundle, SystemInfo};
use crate::service::CollectorService;
use crate::store::Store;
use crate::system_info;

const JOURNAL_DRAIN_SECS: u64 = 3;
const FRAMEWORK_146_URL: &str =
    "https://github.com/FrameworkComputer/SoftwareFirmwareIssueTracker/issues/146";

pub struct CaptureOptions {
    pub duration: Duration,
    pub issue: String,
    pub output_dir: Option<PathBuf>,
    pub device_pci: Option<String>,
    pub skip_pre_backup: bool,
}

pub async fn run_capture(
    config: Config,
    collector_kind: CollectorKind,
    opts: CaptureOptions,
) -> Result<PathBuf> {
    let store = std::sync::Arc::new(Store::open(&config.db_path).await?);
    let out_dir = capture_output_dir(&opts)?;
    tokio::fs::create_dir_all(&out_dir).await?;

    println!("capture output: {}", out_dir.display());
    println!(
        "stop framelog.service (or any other collector using {}) before capture",
        config.db_path.display()
    );

    store.assert_not_collecting(5_000).await?;

    if !opts.skip_pre_backup {
        match existing_bounds(&store).await {
            Ok((from, to)) => {
                let bundle = store
                    .export_bundle_unbounded(from, to, opts.device_pci.as_deref())
                    .await?;
                let path = out_dir.join("pre_capture_backup.json");
                write_json(&path, &bundle).await?;
                println!("pre-capture backup: {}", path.display());
            }
            Err(Error::BadRequest(message)) if message == "no telemetry data to dump" => {}
            Err(err) => return Err(err),
        }
    }

    store.reset_data_collection().await?;

    let started_at_ms = crate::util::current_ts_ms();
    store
        .insert_capture_marker(started_at_ms, "capture_start", Some(&opts.issue))
        .await?;

    let system = tokio::task::spawn_blocking(system_info::collect_system_info)
        .await
        .map_err(|e| Error::Other(format!("system inventory task failed: {e}")))?;
    store.insert_system_info_snapshot(&system).await?;
    write_json(&out_dir.join("system.json"), &system).await?;

    let collector = make_collector(collector_kind, config.apu_only);
    let service = CollectorService::new(std::sync::Arc::clone(&store), collector, config.clone());
    println!(
        "collecting for {} (issue: {})",
        format_duration(opts.duration),
        opts.issue
    );
    service.run_for(opts.duration).await?;

    tokio::time::sleep(Duration::from_secs(JOURNAL_DRAIN_SECS)).await;

    let ended_at_ms = crate::util::current_ts_ms();
    store
        .insert_capture_marker(ended_at_ms, "capture_end", None)
        .await?;

    let markers = store
        .list_capture_markers(started_at_ms, ended_at_ms)
        .await?;
    let bundle = store
        .export_bundle_unbounded(started_at_ms, ended_at_ms, opts.device_pci.as_deref())
        .await?;

    let manifest = CaptureSessionMeta {
        issue: opts.issue.clone(),
        duration_secs: opts.duration.as_secs(),
        started_at_ms,
        ended_at_ms,
        device_pci_filter: opts.device_pci.clone(),
        markers: markers.clone(),
        framework_issue_url: framework_issue_url(&opts.issue),
    };

    write_json(&out_dir.join("export.json"), &bundle).await?;
    write_json(&out_dir.join("manifest.json"), &manifest).await?;

    let analysis = bundle.analysis.clone().unwrap_or_else(|| {
        analyze::analyze_bundle(
            &bundle,
            &AnalysisOptions {
                from_ms: Some(started_at_ms),
                to_ms: Some(ended_at_ms),
                device_pci: opts.device_pci.clone(),
                issue_tag: Some(opts.issue.clone()),
            },
        )
    });
    write_json(&out_dir.join("analysis.json"), &analysis).await?;

    let report = render_report(&manifest, &bundle, &system, &analysis);
    let summary = render_summary(&manifest, &analysis);
    tokio::fs::write(out_dir.join("report.md"), report).await?;
    tokio::fs::write(out_dir.join("summary.txt"), summary).await?;

    println!("capture complete: {}", out_dir.display());
    println!("  export.json   - full bundle for UI playback");
    println!("  analysis.json - deterministic findings");
    println!("  report.md     - human-readable findings");
    println!("  summary.txt   - paste into GitHub issue comments");
    Ok(out_dir)
}

async fn existing_bounds(store: &Store) -> Result<(i64, i64)> {
    let bounds = store.time_bounds().await?;
    let from = bounds
        .min_ts_ms
        .ok_or_else(|| Error::BadRequest("no telemetry data to dump".into()))?;
    let to = bounds
        .max_ts_ms
        .ok_or_else(|| Error::BadRequest("no telemetry data to dump".into()))?;
    Ok((from, to))
}

fn capture_output_dir(opts: &CaptureOptions) -> Result<PathBuf> {
    if let Some(dir) = &opts.output_dir {
        return Ok(dir.clone());
    }
    let slug = sanitize_slug(&opts.issue);
    let stamp = crate::util::current_ts_ms();
    Ok(PathBuf::from(format!("framelog-capture-{slug}-{stamp}")))
}

fn sanitize_slug(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
        } else if ch == '-' || ch == '_' || ch.is_whitespace() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "capture".into()
    } else {
        trimmed.to_string()
    }
}

pub fn parse_duration(raw: &str) -> Result<Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::BadRequest("duration must not be empty".into()));
    }
    if let Some(num) = raw.strip_suffix('s') {
        let secs: u64 = num
            .trim()
            .parse()
            .map_err(|_| Error::BadRequest(format!("invalid duration: {raw}")))?;
        return nonzero_duration(raw, secs);
    }
    if let Some(num) = raw.strip_suffix('m') {
        let mins: u64 = num
            .trim()
            .parse()
            .map_err(|_| Error::BadRequest(format!("invalid duration: {raw}")))?;
        return nonzero_duration(raw, mins.saturating_mul(60));
    }
    if let Some(num) = raw.strip_suffix('h') {
        let hours: u64 = num
            .trim()
            .parse()
            .map_err(|_| Error::BadRequest(format!("invalid duration: {raw}")))?;
        return nonzero_duration(raw, hours.saturating_mul(3600));
    }
    let secs: u64 = raw
        .parse()
        .map_err(|_| Error::BadRequest(format!("invalid duration: {raw}; use 20m, 1h, or 300s")))?;
    nonzero_duration(raw, secs)
}

fn nonzero_duration(raw: &str, secs: u64) -> Result<Duration> {
    if secs == 0 {
        return Err(Error::BadRequest(format!(
            "duration must be greater than zero: {raw}"
        )));
    }
    Ok(Duration::from_secs(secs))
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 && secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs >= 60 && secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn framework_issue_url(issue: &str) -> Option<String> {
    let lower = issue.to_ascii_lowercase();
    if lower.contains("146") || lower.contains("framework") {
        Some(FRAMEWORK_146_URL.to_string())
    } else {
        None
    }
}

async fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    tokio::fs::write(path, bytes).await?;
    Ok(())
}

fn render_summary(meta: &CaptureSessionMeta, analysis: &AnalysisReport) -> String {
    let mut lines = vec![
        format!("framelog capture - issue: {}", meta.issue),
        format!("window: {} -> {}", meta.started_at_ms, meta.ended_at_ms),
        format!("verdict: {}", analysis.overall_verdict),
    ];
    for f in analysis.findings.iter().take(3) {
        lines.push(format!("- [{:?}] {}: {}", f.confidence, f.title, f.summary));
    }
    if let Some(url) = &meta.framework_issue_url {
        lines.push(format!("reference: {url}"));
    }
    lines.join("\n")
}

fn render_report(
    meta: &CaptureSessionMeta,
    bundle: &ExportBundle,
    system: &SystemInfo,
    analysis: &AnalysisReport,
) -> String {
    let summary = bundle.summary.as_ref();
    let mut out = String::new();
    out.push_str("# framelog capture report\n\n");
    out.push_str(&format!("**Issue tag:** `{}`\n\n", meta.issue));
    if let Some(url) = &meta.framework_issue_url {
        out.push_str(&format!("**Framework reference:** [{url}]({url})\n\n"));
    }
    out.push_str("## Session\n\n");
    out.push_str(&format!(
        "- Started: {} ms\n- Ended: {} ms\n- Planned duration: {}s\n",
        meta.started_at_ms, meta.ended_at_ms, meta.duration_secs
    ));
    if !meta.markers.is_empty() {
        out.push_str("\n### Markers\n\n");
        for m in &meta.markers {
            out.push_str(&format!("- `{}` at {} ms", m.label, m.ts_unix_ms));
            if let Some(d) = &m.detail {
                out.push_str(&format!(" ({d})"));
            }
            out.push('\n');
        }
    }

    out.push_str("\n## System\n\n");
    if let Some(model) = &system.cpu.model_name {
        out.push_str(&format!("- CPU: {model}\n"));
    }
    if let Some(kernel) = &system.os.kernel_release {
        out.push_str(&format!("- Kernel: {kernel}\n"));
    }
    for gpu in &system.gpus {
        out.push_str(&format!(
            "- GPU {}: vendor {} device {} driver {}\n",
            gpu.card,
            gpu.vendor_id.as_deref().unwrap_or("?"),
            gpu.device_id.as_deref().unwrap_or("?"),
            gpu.driver.as_deref().unwrap_or("?")
        ));
    }

    out.push_str("\n## Highlights\n\n");
    if let Some(s) = summary {
        out.push_str(&format!(
            "- Samples: {}\n- Throttle transitions: {}\n- Context transitions: {}\n",
            s.sample_count, s.throttle_transition_count, s.context_transition_count
        ));
        if let Some(p) = s.max_apu_power_mw {
            out.push_str(&format!("- Peak APU power: {p} mW\n"));
        }
        if let Some(p) = s.avg_apu_power_mw {
            out.push_str(&format!("- Average APU power: {p} mW\n"));
        }
        if let Some(t) = s.max_temperature_core {
            out.push_str(&format!("- Peak core temperature: {t:.1} C\n"));
        }
        if !s.flag_activity.is_empty() {
            out.push_str("\n### Flag activity\n\n");
            out.push_str(
                "| Flag | Active % | Assert | Clear |\n|------|----------|--------|-------|\n",
            );
            for f in s.flag_activity.iter().take(12) {
                out.push_str(&format!(
                    "| {} | {:.1}% | {} | {} |\n",
                    f.flag_name, f.active_sample_pct, f.assert_count, f.clear_count
                ));
            }
        }
    }

    out.push_str("\n## Evidence Doctor\n\n");
    out.push_str(&format!("**Verdict:** {}\n\n", analysis.overall_verdict));
    if !analysis.warnings.is_empty() {
        out.push_str("### Data limitations\n\n");
        for w in &analysis.warnings {
            out.push_str(&format!("- {w}\n"));
        }
        out.push('\n');
    }
    for f in &analysis.findings {
        out.push_str(&format!(
            "### {} ({:?}, {:?})\n\n{}\n\n",
            f.title, f.severity, f.confidence, f.summary
        ));
    }
    out.push_str("\n## Attachments\n\n");
    out.push_str("- `export.json` - load in the framelog UI (**Load export**)\n");
    out.push_str("- `analysis.json` - structured findings (same rules as `framelog analyze`)\n");
    out.push_str("- `system.json` - machine inventory at capture start\n");
    out.push_str("- `summary.txt` - short paste-ready summary\n");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_suffixes() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("20m").unwrap(), Duration::from_secs(1200));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90));
        assert!(parse_duration("0s").is_err());
    }

    #[test]
    fn sanitize_issue_slug() {
        assert_eq!(sanitize_slug("Framework #146"), "framework-146");
    }
}
