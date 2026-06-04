use sdjournal::Journal;

use crate::error::{Error, Result};
use crate::model::JournalEvent;

const JOURNAL_LIMIT: usize = 200;

/// Realtime bounds (microseconds) for journal queries around a transition timestamp.
pub fn transition_journal_bounds(center_ms: i64, before_secs: i64, after_secs: i64) -> (u64, u64) {
    let center_us = center_ms.saturating_mul(1_000) as u64;
    let before_us = before_secs.saturating_mul(1_000_000) as u64;
    let after_us = after_secs.saturating_mul(1_000_000) as u64;
    let since = center_us.saturating_sub(before_us);
    let until = center_us.saturating_add(after_us);
    (since, until)
}

/// Query systemd journal entries from `before_secs` before through `after_secs` after `center_ms`.
pub fn query_around_transition(
    center_ms: i64,
    before_secs: i64,
    after_secs: i64,
) -> Result<Vec<JournalEvent>> {
    let journal =
        Journal::open_default().map_err(|e| Error::Journal(format!("open journal: {e}")))?;

    let (since, until) = transition_journal_bounds(center_ms, before_secs, after_secs);

    let mut query = journal.query();
    query.since_realtime(since);
    query.until_realtime(until);

    let iter = query
        .iter()
        .map_err(|e| Error::Journal(format!("journal query: {e}")))?;

    let mut events = Vec::new();
    for item in iter {
        if events.len() >= JOURNAL_LIMIT {
            break;
        }

        let entry = item.map_err(|e| Error::Journal(format!("journal entry: {e}")))?;

        let message = entry
            .get("MESSAGE")
            .map(|m| String::from_utf8_lossy(m).into_owned())
            .unwrap_or_default();

        if !message_interesting(&message) {
            continue;
        }

        let ts_unix_ms = entry
            .get("_SOURCE_REALTIME_TIMESTAMP")
            .and_then(|v| std::str::from_utf8(v).ok())
            .and_then(|s| s.parse::<u64>().ok())
            .map(|us| (us / 1000) as i64)
            .unwrap_or(center_ms);

        let unit = entry
            .get("_SYSTEMD_UNIT")
            .or_else(|| entry.get("UNIT"))
            .map(|u| String::from_utf8_lossy(u).into_owned());

        let priority = entry
            .get("PRIORITY")
            .and_then(|p| std::str::from_utf8(p).ok())
            .and_then(|s| s.parse().ok());

        events.push(JournalEvent {
            id: 0,
            transition_id: 0,
            ts_unix_ms,
            unit,
            priority,
            message,
        });
    }

    events.sort_by_key(|e| e.ts_unix_ms);
    Ok(events)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SleepWakeMarker {
    pub ts_unix_ms: i64,
    pub event: String,
}

/// Return coarse sleep/wake markers from recent journal (`sleep`, `wake`, `awake`).
pub fn recent_sleep_wake_events(since_ms: i64) -> Result<Vec<SleepWakeMarker>> {
    let since_us = since_ms.saturating_mul(1_000).max(0) as u64;
    let journal =
        Journal::open_default().map_err(|e| Error::Journal(format!("open journal: {e}")))?;

    let mut query = journal.query();
    query.since_realtime(since_us);

    let iter = query
        .iter()
        .map_err(|e| Error::Journal(format!("journal query: {e}")))?;

    let mut events = Vec::new();
    for item in iter {
        let entry = item.map_err(|e| Error::Journal(format!("journal entry: {e}")))?;
        let message = entry
            .get("MESSAGE")
            .map(|m| String::from_utf8_lossy(m).into_owned())
            .unwrap_or_default();
        let lower = message.to_ascii_lowercase();
        let marker = if lower.contains("suspending") || lower.contains("suspend entry") {
            Some("sleep")
        } else if lower.contains("resuming")
            || lower.contains("resume complete")
            || (lower.contains("systemd-sleep") && lower.contains("post"))
        {
            Some("wake")
        } else if lower.contains("systemd-sleep") && lower.contains("pre") {
            Some("sleep")
        } else {
            None
        };
        if let Some(m) = marker {
            let ts_unix_ms = entry
                .get("_SOURCE_REALTIME_TIMESTAMP")
                .and_then(|v| std::str::from_utf8(v).ok())
                .and_then(|s| s.parse::<u64>().ok())
                .map(|us| (us / 1000) as i64)
                .unwrap_or(since_ms);
            events.push(SleepWakeMarker {
                ts_unix_ms,
                event: m.to_string(),
            });
        }
        if events.len() >= 20 {
            break;
        }
    }

    Ok(events)
}

fn message_interesting(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "amdgpu",
        "suspend",
        "resume",
        "sleep",
        "wake",
        "power",
        "thrott",
        "prochot",
        "spl",
        "stapm",
        "stt",
        "ec ",
        "framework",
        "acpi",
        "d3cold",
        "boco",
        "pmf",
        "s0ix",
        "s3 ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_filter_matches_power_keywords() {
        assert!(message_interesting("CPU package power limited"));
        assert!(message_interesting("PM: suspend entry (s2idle)"));
        assert!(!message_interesting("random unrelated log line"));
    }

    #[test]
    fn transition_journal_bounds_asymmetric() {
        let center_ms = 1_000_000;
        let (since, until) = transition_journal_bounds(center_ms, 60, 30);
        assert_eq!(since, 940_000_000);
        assert_eq!(until, 1_030_000_000);
    }
}
