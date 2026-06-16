use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use std::sync::Mutex as StdMutex;

use tokio::sync::Mutex;
use tokio::time;

use crate::collector::Collector;
use crate::config::Config;
use crate::context::ContextRegistry;
use crate::error::Result;
use crate::journal;
use crate::model::ContextValue;
use crate::store::Store;
use crate::system_info;
use crate::throttle::diff_flags;

const SYSTEM_INFO_REFRESH: Duration = Duration::from_secs(24 * 60 * 60);
const PRUNE_INTERVAL: Duration = Duration::from_secs(60 * 60);

pub struct CollectorService {
    store: Arc<Store>,
    collector: Arc<dyn Collector>,
    config: Config,
    last_masks: Arc<Mutex<HashMap<String, u64>>>,
    context: Arc<StdMutex<ContextRegistry>>,
}

impl CollectorService {
    pub fn new(store: Arc<Store>, collector: Arc<dyn Collector>, config: Config) -> Self {
        let context = ContextRegistry::new(&config);
        Self {
            store,
            collector,
            config,
            last_masks: Arc::new(Mutex::new(HashMap::new())),
            context: Arc::new(StdMutex::new(context)),
        }
    }

    pub async fn run(self) -> Result<()> {
        self.run_loop().await
    }

    /// Run the collector until `duration` elapses (used by guided capture sessions).
    pub async fn run_for(self, duration: Duration) -> Result<()> {
        match time::timeout(duration, self.run_loop()).await {
            Ok(result) => result,
            Err(_) => {
                tracing::info!("collector stopped after bounded run");
                Ok(())
            }
        }
    }

    async fn run_loop(self) -> Result<()> {
        let mut interval = time::interval(Duration::from_millis(self.config.interval_ms));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut system_info_interval = time::interval(SYSTEM_INFO_REFRESH);
        system_info_interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        tracing::info!(interval_ms = self.config.interval_ms, "collector started");
        self.prune_if_enabled("startup").await;
        self.harvest_system_info().await;
        system_info_interval.tick().await;
        let mut prune_interval = time::interval(PRUNE_INTERVAL);
        prune_interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        prune_interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(err) = self.tick().await {
                        tracing::warn!(%err, "collector tick failed");
                    }
                }
                _ = system_info_interval.tick() => {
                    self.harvest_system_info().await;
                }
                _ = prune_interval.tick() => {
                    self.prune_if_enabled("scheduled").await;
                }
            }
        }
    }

    async fn prune_if_enabled(&self, reason: &'static str) {
        let hours = self.config.retention_hours;
        if hours == 0 {
            return;
        }
        let cutoff = crate::util::current_ts_ms().saturating_sub(hours as i64 * 3_600_000);
        match self.store.prune_older_than(cutoff).await {
            Ok(stats) => {
                let total = stats.journal_events
                    + stats.context_journal_events
                    + stats.transitions
                    + stats.context_transitions
                    + stats.context_values
                    + stats.context_snapshots
                    + stats.samples
                    + stats.capture_markers
                    + stats.system_info_snapshots;
                if total > 0 {
                    tracing::info!(
                        reason,
                        retention_hours = hours,
                        cutoff_ms = cutoff,
                        samples = stats.samples,
                        context_snapshots = stats.context_snapshots,
                        context_values = stats.context_values,
                        transitions = stats.transitions,
                        context_transitions = stats.context_transitions,
                        "pruned old telemetry"
                    );
                }
            }
            Err(err) => tracing::warn!(%err, reason, "retention prune failed"),
        }
    }

    async fn harvest_system_info(&self) {
        let info = tokio::task::spawn_blocking(system_info::collect_system_info).await;
        match info {
            Ok(info) => {
                let collected_at_ms = info.collected_at_ms;
                if let Err(err) = self.store.insert_system_info_snapshot(&info).await {
                    tracing::warn!(%err, "failed to store system inventory");
                } else {
                    tracing::info!(collected_at_ms, health = %info.health, "system inventory harvested");
                }
            }
            Err(err) => tracing::warn!(%err, "system inventory task failed"),
        }
    }

    async fn tick(&self) -> Result<()> {
        if let Err(err) = self.tick_gpu().await {
            tracing::warn!(%err, "gpu collector tick failed; continuing context sampling");
        }
        self.tick_context().await
    }

    async fn tick_gpu(&self) -> Result<()> {
        let samples = self.collector.sample()?;

        for sample in samples {
            let sample_id = self.store.insert_sample(&sample).await?;

            let mut last_masks = self.last_masks.lock().await;
            let previous = last_masks
                .get(&sample.device_pci)
                .copied()
                .unwrap_or(sample.indep_throttle_status);

            let changes = diff_flags(previous, sample.indep_throttle_status);
            last_masks.insert(sample.device_pci.clone(), sample.indep_throttle_status);
            drop(last_masks);

            for (flag_name, old_value, new_value) in changes {
                tracing::info!(
                    pci = %sample.device_pci,
                    %flag_name,
                    old = old_value,
                    new = new_value,
                    "throttle flag transition"
                );

                let transition_id = self
                    .store
                    .insert_transition(
                        sample.ts_unix_ms,
                        sample_id,
                        &sample.device_pci,
                        &flag_name,
                        old_value,
                        new_value,
                    )
                    .await?;

                self.spawn_journal_lookup(transition_id, sample.ts_unix_ms, true);
            }
        }

        Ok(())
    }

    async fn tick_context(&self) -> Result<()> {
        let ts_unix_ms = crate::util::current_ts_ms();
        let context = Arc::clone(&self.context);
        let (snapshots, transitions) = tokio::task::spawn_blocking(move || {
            let mut registry = context
                .lock()
                .map_err(|e| format!("context registry lock poisoned: {e}"))?;
            let snapshots = registry.sample_all(ts_unix_ms);
            let transitions = registry.transitions_for_tick(&snapshots);
            Ok::<_, String>((snapshots, transitions))
        })
        .await
        .map_err(|e| crate::error::Error::Other(format!("context sample task: {e}")))?
        .map_err(crate::error::Error::Other)?;

        let mut snapshot_ids = HashMap::new();
        for snap in &snapshots {
            let snapshot_id = self.store.insert_context_snapshot(snap).await?;
            snapshot_ids.insert(snap.source_id.clone(), snapshot_id);
            for value in &snap.values {
                let full_key = format!("{}.{}", snap.source_id, value.key);
                let stored = ContextValue {
                    key: full_key,
                    value_num: value.value_num,
                    value_str: value.value_str.clone(),
                };
                self.store
                    .insert_context_value(snapshot_id, snap.ts_unix_ms, &snap.source_id, &stored)
                    .await?;
            }
        }

        for mut transition in transitions {
            let Some(snapshot_id) = snapshot_ids.get(&transition.source_id).copied() else {
                tracing::warn!(
                    source = %transition.source_id,
                    key = %transition.key,
                    "skipping context transition without snapshot"
                );
                continue;
            };
            transition.snapshot_id = snapshot_id;
            tracing::info!(
                source = %transition.source_id,
                key = %transition.key,
                "context transition"
            );
            let transition_id = self.store.insert_context_transition(&transition).await?;
            self.spawn_journal_lookup(transition_id, transition.ts_unix_ms, false);
        }

        Ok(())
    }

    fn spawn_journal_lookup(&self, transition_id: i64, ts_unix_ms: i64, throttle: bool) {
        let before = self.config.journal_before_secs;
        let after = self.config.journal_after_secs;
        let store = Arc::clone(&self.store);

        tokio::spawn(async move {
            let events = match tokio::task::spawn_blocking(move || {
                journal::query_around_transition(ts_unix_ms, before, after)
            })
            .await
            {
                Ok(Ok(events)) => events,
                Ok(Err(err)) => {
                    tracing::warn!(%err, "journal lookup failed");
                    return;
                }
                Err(err) => {
                    tracing::warn!(%err, "journal task failed");
                    return;
                }
            };

            for event in events {
                let result = if throttle {
                    store
                        .insert_journal_event(
                            transition_id,
                            event.ts_unix_ms,
                            event.unit.as_deref(),
                            event.priority,
                            &event.message,
                            None,
                        )
                        .await
                } else {
                    store
                        .insert_context_journal_event(
                            transition_id,
                            event.ts_unix_ms,
                            event.unit.as_deref(),
                            event.priority,
                            &event.message,
                        )
                        .await
                };
                if let Err(err) = result {
                    tracing::warn!(%err, "failed to store journal event");
                }
            }
        });
    }
}
