use serde_json::json;

use crate::context::{default_diff, snapshot_ok, snapshot_unavailable, ContextSource};
use crate::model::{ContextSnapshot, ContextTransition, ContextValue};

const ID: &str = "profile";

pub struct PowerProfileSource;

impl PowerProfileSource {
    pub fn new() -> Self {
        Self
    }
}

fn try_read(bus: &str, path: &str, iface: &str) -> Result<String, zbus::Error> {
    use zbus::blocking::Connection;

    let conn = Connection::system()?;
    let proxy = zbus::blocking::Proxy::new(&conn, bus, path, iface)?;
    proxy.get_property("ActiveProfile")
}

fn read_active_profile() -> Result<String, String> {
    try_read(
        "net.hadess.PowerProfiles",
        "/net/hadess/PowerProfiles",
        "net.hadess.PowerProfiles",
    )
    .or_else(|_| {
        try_read(
            "org.freedesktop.UPower.PowerProfiles",
            "/org/freedesktop/UPower/PowerProfiles",
            "org.freedesktop.UPower.PowerProfiles",
        )
    })
    .map_err(|e| e.to_string())
}

impl ContextSource for PowerProfileSource {
    fn id(&self) -> &'static str {
        ID
    }

    fn sample(&mut self, ts_unix_ms: i64) -> ContextSnapshot {
        match read_active_profile() {
            Ok(profile) => {
                let values = vec![ContextValue::str("active", profile.clone())];
                snapshot_ok(ID, ts_unix_ms, values, json!({ "active": profile }))
            }
            Err(e) => snapshot_unavailable(ID, ts_unix_ms, &e),
        }
    }

    fn diff(
        &self,
        previous: Option<&ContextSnapshot>,
        current: &ContextSnapshot,
    ) -> Vec<ContextTransition> {
        default_diff(previous, current)
    }
}
