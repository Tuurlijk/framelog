use crate::analyze;
use crate::error::{Error, Result};
use crate::model::{AnalysisOptions, ExportBundle};
use crate::store::query::{ContextTransitionQuery, FlagTransitionQuery};
use crate::store::{Store, EXPORT_TRANSITION_LIMIT, MAX_EXPORT_SAMPLES, MAX_EXPORT_WINDOW_MS};

#[derive(Clone, Debug)]
pub struct ExportOptions<'a> {
    pub from_ms: i64,
    pub to_ms: i64,
    pub device_pci: Option<&'a str>,
    pub enforce_web_limits: bool,
}

impl Store {
    pub async fn export_bundle(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<ExportBundle> {
        let mut bundle = self
            .build_export_bundle(ExportOptions {
                from_ms,
                to_ms,
                device_pci,
                enforce_web_limits: true,
            })
            .await?;
        analyze::attach_analysis(
            &mut bundle,
            &AnalysisOptions {
                from_ms: Some(from_ms),
                to_ms: Some(to_ms),
                device_pci: device_pci.map(str::to_string),
                issue_tag: None,
            },
        );
        Ok(bundle)
    }

    pub async fn export_bundle_unbounded(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<ExportBundle> {
        self.build_export_bundle(ExportOptions {
            from_ms,
            to_ms,
            device_pci,
            enforce_web_limits: false,
        })
        .await
    }

    pub async fn build_analysis_bundle(
        &self,
        from_ms: i64,
        to_ms: i64,
        device_pci: Option<&str>,
    ) -> Result<ExportBundle> {
        if from_ms > to_ms {
            return Err(Error::BadRequest("from must be <= to".into()));
        }
        if to_ms - from_ms > MAX_EXPORT_WINDOW_MS {
            return Err(Error::BadRequest(format!(
                "analysis window exceeds {} days; narrow the range",
                MAX_EXPORT_WINDOW_MS / (24 * 60 * 60 * 1000)
            )));
        }

        let sample_count = self
            .count_samples_in_range(from_ms, to_ms, device_pci)
            .await?;
        if sample_count > MAX_EXPORT_SAMPLES {
            return Err(Error::BadRequest(format!(
                "analysis would include {sample_count} samples (max {MAX_EXPORT_SAMPLES}); narrow the range or filter by device"
            )));
        }

        let samples = self.list_samples(from_ms, to_ms, device_pci).await?;
        let throttle_transitions = self
            .list_flag_transitions(FlagTransitionQuery {
                from_ms,
                to_ms,
                limit: EXPORT_TRANSITION_LIMIT,
                device_pci,
                flag_name: None,
                direction: None,
                newest_first: false,
            })
            .await?;
        let throttle_ids: Vec<i64> = throttle_transitions.iter().map(|t| t.id).collect();
        let throttle_journal_events = self.journal_for_transitions(&throttle_ids).await?;

        let context_values = self.list_context_values(from_ms, to_ms).await?;
        let context_transitions = self
            .list_context_transitions_query(ContextTransitionQuery {
                from_ms,
                to_ms,
                limit: EXPORT_TRANSITION_LIMIT,
                source_id: None,
                key: None,
                newest_first: false,
            })
            .await?;
        let ctx_ids: Vec<i64> = context_transitions.iter().map(|t| t.id).collect();
        let context_journal_events = self.journal_for_context_transitions(&ctx_ids).await?;
        let system = self.latest_system_info().await?;
        let summary = self
            .window_summary_sql(from_ms, to_ms, device_pci)
            .await
            .ok();

        Ok(ExportBundle {
            schema_version: 1,
            format: crate::model::EXPORT_FORMAT.to_string(),
            exported_at_ms: crate::util::current_ts_ms(),
            from_ms,
            to_ms,
            device_pci_filter: device_pci.map(str::to_string),
            devices: Vec::new(),
            samples,
            throttle_transitions,
            throttle_journal_events,
            context_snapshots: Vec::new(),
            context_values,
            context_transitions,
            context_journal_events,
            system,
            summary,
            export_warnings: Vec::new(),
            analysis: None,
        })
    }

    pub async fn build_export_bundle(&self, opts: ExportOptions<'_>) -> Result<ExportBundle> {
        let ExportOptions {
            from_ms,
            to_ms,
            device_pci,
            enforce_web_limits,
        } = opts;

        if from_ms > to_ms {
            return Err(Error::BadRequest("from must be <= to".into()));
        }
        if enforce_web_limits && to_ms - from_ms > MAX_EXPORT_WINDOW_MS {
            return Err(Error::BadRequest(format!(
                "export window exceeds {} days; narrow the range",
                MAX_EXPORT_WINDOW_MS / (24 * 60 * 60 * 1000)
            )));
        }

        let sample_count = self
            .count_samples_in_range(from_ms, to_ms, device_pci)
            .await?;
        if enforce_web_limits && sample_count > MAX_EXPORT_SAMPLES {
            return Err(Error::BadRequest(format!(
                "export would include {sample_count} samples (max {MAX_EXPORT_SAMPLES}); narrow the range or filter by device"
            )));
        }

        let devices = self.devices().await?;
        let samples = self.list_samples(from_ms, to_ms, device_pci).await?;

        let throttle_total = self
            .count_flag_transitions(from_ms, to_ms, device_pci)
            .await?;
        let throttle_truncated = throttle_total > EXPORT_TRANSITION_LIMIT;
        let throttle_transitions = self
            .list_flag_transitions(FlagTransitionQuery {
                from_ms,
                to_ms,
                limit: EXPORT_TRANSITION_LIMIT,
                device_pci,
                flag_name: None,
                direction: None,
                newest_first: false,
            })
            .await?;
        let throttle_ids: Vec<i64> = throttle_transitions.iter().map(|t| t.id).collect();
        let throttle_journal_events = self.journal_for_transitions(&throttle_ids).await?;

        let context_snapshots = self.list_context_snapshots(from_ms, to_ms).await?;
        let context_values = self.list_context_values(from_ms, to_ms).await?;
        let context_total = self.count_context_transitions(from_ms, to_ms).await?;
        let context_truncated = context_total > EXPORT_TRANSITION_LIMIT;
        let context_transitions = self
            .list_context_transitions_query(ContextTransitionQuery {
                from_ms,
                to_ms,
                limit: EXPORT_TRANSITION_LIMIT,
                source_id: None,
                key: None,
                newest_first: false,
            })
            .await?;
        let ctx_ids: Vec<i64> = context_transitions.iter().map(|t| t.id).collect();
        let context_journal_events = self.journal_for_context_transitions(&ctx_ids).await?;
        let system = self.latest_system_info().await?;
        let summary = self
            .window_summary_sql(from_ms, to_ms, device_pci)
            .await
            .ok();

        let mut export_warnings = Vec::new();
        if throttle_truncated {
            export_warnings.push(format!(
                "throttle transitions truncated to {EXPORT_TRANSITION_LIMIT} of {throttle_total} in this window (oldest events omitted)"
            ));
        }
        if context_truncated {
            export_warnings.push(format!(
                "context transitions truncated to {EXPORT_TRANSITION_LIMIT} of {context_total} in this window (oldest events omitted)"
            ));
        }

        Ok(ExportBundle {
            schema_version: 1,
            format: crate::model::EXPORT_FORMAT.to_string(),
            exported_at_ms: crate::util::current_ts_ms(),
            from_ms,
            to_ms,
            device_pci_filter: device_pci.map(str::to_string),
            devices,
            samples,
            throttle_transitions,
            throttle_journal_events,
            context_snapshots,
            context_values,
            context_transitions,
            context_journal_events,
            system,
            summary,
            export_warnings,
            analysis: None,
        })
    }
}
