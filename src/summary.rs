use std::collections::HashMap;

use crate::model::{
    ContextTransition, FlagActivitySummary, FlagTransition, MetricSample, WindowSummary,
};
use crate::throttle;

pub fn summarize_window(
    from_ms: i64,
    to_ms: i64,
    samples: &[MetricSample],
    throttle_transitions: &[FlagTransition],
    context_transitions: &[ContextTransition],
) -> WindowSummary {
    let sample_count = samples.len() as i64;

    let mut flag_stats: HashMap<String, (u64, u64)> = HashMap::new();
    for t in throttle_transitions {
        let entry = flag_stats.entry(t.flag_name.clone()).or_insert((0, 0));
        if t.new_value {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
    }

    let mut flag_active_samples: HashMap<String, u64> = HashMap::new();
    for s in samples {
        for flag in &s.active_flags {
            *flag_active_samples.entry(flag.clone()).or_insert(0) += 1;
        }
    }

    for flag in flag_active_samples.keys() {
        flag_stats.entry(flag.clone()).or_insert((0, 0));
    }

    let mut flag_activity: Vec<FlagActivitySummary> = flag_stats
        .into_iter()
        .map(|(flag_name, (assert_count, clear_count))| {
            let active = flag_active_samples.get(&flag_name).copied().unwrap_or(0);
            let pct = if sample_count > 0 {
                (active as f64 / sample_count as f64) * 100.0
            } else {
                0.0
            };
            FlagActivitySummary {
                flag_name,
                active_sample_pct: pct,
                assert_count,
                clear_count,
            }
        })
        .collect();
    flag_activity.sort_by(|a, b| a.flag_name.cmp(&b.flag_name));

    let apu_powers: Vec<u32> = samples.iter().filter_map(|s| s.apu_power_mw).collect();
    let max_apu_power_mw = apu_powers.iter().copied().max();
    let avg_apu_power_mw = if apu_powers.is_empty() {
        None
    } else {
        let sum: u64 = apu_powers.iter().map(|v| u64::from(*v)).sum();
        Some((sum / apu_powers.len() as u64) as u32)
    };
    let max_temperature_core = samples
        .iter()
        .filter_map(|s| s.temperature_core_max)
        .max_by(f64::total_cmp);

    let warnings = throttle::prochot_warnings_from_activity(&flag_activity);

    WindowSummary {
        from_ms,
        to_ms,
        sample_count,
        throttle_transition_count: throttle_transitions.len() as i64,
        context_transition_count: context_transitions.len() as i64,
        flag_activity,
        max_apu_power_mw,
        avg_apu_power_mw,
        max_temperature_core,
        warnings,
    }
}
