use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use crate::context::{default_diff, snapshot_ok, ContextSource};
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

pub struct FakeContextSource {
    tick: AtomicU64,
}

impl FakeContextSource {
    pub fn new() -> Self {
        Self {
            tick: AtomicU64::new(0),
        }
    }
}

impl ContextSource for FakeContextSource {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let t = self.tick.fetch_add(1, Ordering::Relaxed);
        let ac = t % 40 < 30;
        let watts = if ac { 45.0 + (t % 10) as f64 } else { 0.0 };
        let pct = 80.0 - (t % 50) as f64;
        let state = if t % 60 < 45 {
            "charging"
        } else {
            "discharging"
        };
        let external = t % 25 < 8;
        let profile = match (t / 20) % 3 {
            0 => "balanced",
            1 => "performance",
            _ => "power-saver",
        };
        let sleep_event = if t == 50 {
            "sleep"
        } else if t == 51 {
            "wake"
        } else {
            "awake"
        };

        let values = vec![
            ContextValue::num("ac_connected", if ac { 1.0 } else { 0.0 }),
            ContextValue::num("input_watts", watts),
            ContextValue::num("adapter_watts_reported", 180.0),
            ContextValue::num("percent", pct),
            ContextValue::str("state", state),
            ContextValue::num("external_connected", if external { 1.0 } else { 0.0 }),
            ContextValue::str("profile", profile),
            ContextValue::str("sleep_event", sleep_event),
        ];

        snapshot_ok(
            "fake",
            ts_unix_ms,
            values,
            json!({
                "ac_connected": ac,
                "input_watts": watts,
                "battery_percent": pct,
                "battery_state": state,
                "external_connected": external,
                "profile": profile,
                "sleep_event": sleep_event,
            }),
        )
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        default_diff(previous, current)
    }
}

pub struct FakePmfSource {
    tick: AtomicU64,
}

impl FakePmfSource {
    pub fn new() -> Self {
        Self {
            tick: AtomicU64::new(0),
        }
    }
}

impl ContextSource for FakePmfSource {
    fn id(&self) -> &'static str {
        "pmf"
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let t = self.tick.fetch_add(1, Ordering::Relaxed);
        let dgpu_asleep = t % 30 >= 20;
        let spl = if dgpu_asleep { 35_000.0 } else { 54_000.0 };
        let stt_hs2 = if dgpu_asleep { 0.0 } else { 58.0 };
        let values = vec![
            ContextValue::num("spl_mw", spl),
            ContextValue::num("sppt_mw", spl),
            ContextValue::num("fppt_mw", spl + 10_000.0),
            ContextValue::num("stt_hs2_c", stt_hs2),
        ];
        snapshot_ok(
            "pmf",
            ts_unix_ms,
            values,
            json!({ "fake": true, "dgpu_asleep": dgpu_asleep }),
        )
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        default_diff(previous, current)
    }
}

pub struct FakeGpuPowerSource {
    tick: AtomicU64,
}

impl FakeGpuPowerSource {
    pub fn new() -> Self {
        Self {
            tick: AtomicU64::new(0),
        }
    }
}

impl ContextSource for FakeGpuPowerSource {
    fn id(&self) -> &'static str {
        "gpu_power"
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        let t = self.tick.fetch_add(1, Ordering::Relaxed);
        let asleep = t % 30 >= 20;
        let status = if asleep { "suspended" } else { "active" };
        let values = vec![
            ContextValue::str("dgpu_runtime_status", status),
            ContextValue::num("dgpu_runtime_suspended", if asleep { 1.0 } else { 0.0 }),
            ContextValue::num("dgpu_d3cold_allowed", 1.0),
        ];
        snapshot_ok(
            "gpu_power",
            ts_unix_ms,
            values,
            json!({ "fake": true, "dgpu_runtime_status": status }),
        )
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        default_diff(previous, current)
    }
}
