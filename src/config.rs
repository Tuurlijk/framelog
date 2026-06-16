use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Config {
    pub db_path: PathBuf,
    pub interval_ms: u64,
    pub bind: SocketAddr,
    pub journal_before_secs: i64,
    pub journal_after_secs: i64,
    pub apu_only: bool,
    pub fake: bool,
    /// Rolling retention window for telemetry rows. `0` disables automatic pruning.
    pub retention_hours: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
            interval_ms: 1000,
            bind: "127.0.0.1:8787".parse().expect("valid bind address"),
            journal_before_secs: 60,
            journal_after_secs: 30,
            apu_only: false,
            fake: false,
            retention_hours: 24,
        }
    }
}

pub fn default_db_path() -> PathBuf {
    default_data_path()
}

fn default_data_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/share/framelog/framelog.db");
    }
    PathBuf::from("/var/lib/framelog/framelog.db")
}
