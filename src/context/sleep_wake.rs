use serde_json::json;

use crate::context::{snapshot_ok, value_transition, ContextDiffPolicy, ContextSource};
use crate::journal;
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

const ID: &str = "sleep";

pub struct SleepWakeSource {
    interval_ms: u64,
    last_tick_ms: Option<i64>,
    last_event: Option<String>,
    last_journal_query_ms: Option<i64>,
}

impl SleepWakeSource {
    pub fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            last_tick_ms: None,
            last_event: None,
            last_journal_query_ms: None,
        }
    }

    fn journal_query_interval_ms(&self) -> i64 {
        let from_interval = (self.interval_ms as i64).saturating_mul(10);
        from_interval.max(30_000)
    }

    fn gap_wake_threshold_ms(&self) -> i64 {
        let from_interval = (self.interval_ms as i64).saturating_mul(5);
        from_interval.max(10_000)
    }
}

impl ContextSource for SleepWakeSource {
    fn id(&self) -> &'static str {
        ID
    }

    fn diff_policy(&self) -> ContextDiffPolicy {
        ContextDiffPolicy::Custom
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let mut event = "awake".to_string();

        if let Some(last) = self.last_tick_ms {
            let gap = ts_unix_ms.saturating_sub(last);
            if gap >= self.gap_wake_threshold_ms() {
                event = "wake".into();
                self.last_event = Some(event.clone());
            }
        }

        let query_journal = self
            .last_journal_query_ms
            .map(|last| ts_unix_ms.saturating_sub(last) >= self.journal_query_interval_ms())
            .unwrap_or(true);
        if query_journal {
            self.last_journal_query_ms = Some(ts_unix_ms);
            if let Ok(recent) = journal::recent_sleep_wake_events(ts_unix_ms - 120_000) {
                if let Some(latest) = recent.iter().max_by_key(|m| m.ts_unix_ms) {
                    event = latest.event.clone();
                    self.last_event = Some(event.clone());
                }
            }
        } else if let Some(last) = &self.last_event {
            event.clone_from(last);
        }

        self.last_tick_ms = Some(ts_unix_ms);

        let values = vec![ContextValue::str("event", event.clone())];
        snapshot_ok(ID, ts_unix_ms, values, json!({ "event": event }))
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        let mut transitions = Vec::new();

        let old = previous
            .and_then(|p| p.values.iter().find(|v| v.key == "event"))
            .and_then(|v| v.value_str.clone());
        let new = current
            .values
            .iter()
            .find(|v| v.key == "event")
            .and_then(|v| v.value_str.clone());

        if old != new {
            if let (Some(o), Some(n)) = (old, new) {
                let key = match n.as_str() {
                    "sleep" => "system.sleep",
                    "wake" => "system.wake",
                    _ => "system.power_event",
                };
                transitions.push(value_transition(
                    ID,
                    current.ts_unix_ms,
                    key,
                    &json!(o),
                    &json!(n),
                ));
            }
        }

        transitions
    }
}
