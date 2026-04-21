//! Persistent + runtime state of a station. Stored as JSON on disk.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cli::OcppVersion;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConnectorState {
    Available,
    PluggedIn,  // cable connected, not yet authorized
    Authorized, // RFID accepted, waiting to start transaction
    Charging,
    SuspendedEV,
    SuspendedEVSE,
    Finishing,
    Unavailable,
    Faulted,
}

impl ConnectorState {
    /// Map to an OCPP 1.6 status string.
    pub fn to_v16(&self) -> &'static str {
        match self {
            ConnectorState::Available => "Available",
            ConnectorState::PluggedIn => "Preparing",
            ConnectorState::Authorized => "Preparing",
            ConnectorState::Charging => "Charging",
            ConnectorState::SuspendedEV => "SuspendedEV",
            ConnectorState::SuspendedEVSE => "SuspendedEVSE",
            ConnectorState::Finishing => "Finishing",
            ConnectorState::Unavailable => "Unavailable",
            ConnectorState::Faulted => "Faulted",
        }
    }
    /// Map to an OCPP 2.0.1 connectorStatus string.
    pub fn to_v201(&self) -> &'static str {
        match self {
            ConnectorState::Available => "Available",
            ConnectorState::PluggedIn
            | ConnectorState::Authorized
            | ConnectorState::Charging
            | ConnectorState::SuspendedEV
            | ConnectorState::SuspendedEVSE
            | ConnectorState::Finishing => "Occupied",
            ConnectorState::Unavailable => "Unavailable",
            ConnectorState::Faulted => "Faulted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connector {
    pub id: i32,
    pub state: ConnectorState,
    pub meter_wh: i64,
    pub max_power_w: i32,
    /// Active transaction (OCPP 1.6: numeric id as string; OCPP 2.0.1: UUID string).
    pub transaction_id: Option<String>,
    pub current_tag: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfidTag {
    pub id_tag: String,
    pub label: String,
    /// Accepted | Blocked | Expired | Invalid
    pub status: String,
    #[serde(default)]
    pub parent_id_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: DateTime<Utc>,
    pub direction: String, // "->" / "<-" / "info" / "warn" / "error"
    pub action: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationState {
    pub id: String,
    pub version: OcppVersion,
    pub csms_url: String,
    pub vendor: String,
    pub model: String,
    pub firmware_version: String,
    pub serial_number: String,
    pub connected: bool,
    pub last_heartbeat: Option<DateTime<Utc>>,
    pub heartbeat_interval_s: i32,
    pub boot_accepted: bool,
    pub connectors: Vec<Connector>,
    pub tags: Vec<RfidTag>,
    #[serde(default)]
    pub config_keys: HashMap<String, String>,
    /// Recent log entries (ring buffer).
    #[serde(default)]
    pub log: Vec<LogEntry>,
    /// Completed transaction history.
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub transaction_id: String,
    pub connector_id: i32,
    pub id_tag: String,
    pub started: DateTime<Utc>,
    pub ended: DateTime<Utc>,
    pub wh_consumed: i64,
}

impl StationState {
    pub fn new_default(id: String, version: OcppVersion, csms_url: String) -> Self {
        let tags = vec![
            RfidTag {
                id_tag: "DEADBEEF01".into(),
                label: "Default chip (accepted)".into(),
                status: "Accepted".into(),
                parent_id_tag: None,
            },
            RfidTag {
                id_tag: "BADC0DE002".into(),
                label: "Blocked chip".into(),
                status: "Blocked".into(),
                parent_id_tag: None,
            },
        ];
        let connectors = vec![
            Connector {
                id: 1,
                state: ConnectorState::Available,
                meter_wh: 0,
                max_power_w: 22_000,
                transaction_id: None,
                current_tag: None,
                started_at: None,
            },
            Connector {
                id: 2,
                state: ConnectorState::Available,
                meter_wh: 0,
                max_power_w: 22_000,
                transaction_id: None,
                current_tag: None,
                started_at: None,
            },
        ];
        StationState {
            id,
            version,
            csms_url,
            vendor: "VirtualOCPP".into(),
            model: "VBox-2".into(),
            firmware_version: env!("CARGO_PKG_VERSION").to_string(),
            serial_number: format!("VOCPP-{}", uuid::Uuid::new_v4().simple()),
            connected: false,
            last_heartbeat: None,
            heartbeat_interval_s: 30,
            boot_accepted: false,
            connectors,
            tags,
            config_keys: default_config_keys(),
            log: Vec::new(),
            history: Vec::new(),
        }
    }

    pub fn load_or_init(data_dir: &Path, id: &str, version: OcppVersion, csms_url: &str) -> Self {
        let path = Self::file_path(data_dir, id);
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(mut s) = serde_json::from_str::<StationState>(&text) {
                // CLI args may override the persisted CSMS URL and version.
                s.csms_url = csms_url.to_string();
                s.version = version;
                return s;
            }
        }
        Self::new_default(id.to_string(), version, csms_url.to_string())
    }

    pub fn file_path(data_dir: &Path, id: &str) -> PathBuf {
        data_dir.join(format!("{id}.json"))
    }

    pub fn save(&self, data_dir: &Path) -> anyhow::Result<()> {
        let path = Self::file_path(data_dir, &self.id);
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn push_log(&mut self, entry: LogEntry) {
        self.log.push(entry);
        if self.log.len() > 500 {
            let drop = self.log.len() - 500;
            self.log.drain(0..drop);
        }
    }

    pub fn connector_mut(&mut self, id: i32) -> Option<&mut Connector> {
        self.connectors.iter_mut().find(|c| c.id == id)
    }

    pub fn connector(&self, id: i32) -> Option<&Connector> {
        self.connectors.iter().find(|c| c.id == id)
    }
}

fn default_config_keys() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("HeartbeatInterval".into(), "30".into());
    m.insert("MeterValueSampleInterval".into(), "10".into());
    m.insert("NumberOfConnectors".into(), "2".into());
    m.insert("AuthorizationCacheEnabled".into(), "true".into());
    m.insert("LocalAuthorizeOffline".into(), "true".into());
    m.insert("LocalPreAuthorize".into(), "false".into());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_station_has_two_connectors_and_some_tags() {
        let s = StationState::new_default("cp1".into(), OcppVersion::V16, "ws://x".into());
        assert_eq!(s.connectors.len(), 2);
        assert_eq!(s.connectors[0].id, 1);
        assert_eq!(s.connectors[1].id, 2);
        assert!(s.tags.iter().any(|t| t.status == "Accepted"));
        assert!(s.tags.iter().any(|t| t.status == "Blocked"));
    }

    #[test]
    fn connector_state_mapping_v16() {
        assert_eq!(ConnectorState::Available.to_v16(), "Available");
        assert_eq!(ConnectorState::PluggedIn.to_v16(), "Preparing");
        assert_eq!(ConnectorState::Authorized.to_v16(), "Preparing");
        assert_eq!(ConnectorState::Charging.to_v16(), "Charging");
        assert_eq!(ConnectorState::Finishing.to_v16(), "Finishing");
        assert_eq!(ConnectorState::Faulted.to_v16(), "Faulted");
    }

    #[test]
    fn connector_state_mapping_v201_groups_occupied() {
        assert_eq!(ConnectorState::Available.to_v201(), "Available");
        assert_eq!(ConnectorState::PluggedIn.to_v201(), "Occupied");
        assert_eq!(ConnectorState::Charging.to_v201(), "Occupied");
        assert_eq!(ConnectorState::Finishing.to_v201(), "Occupied");
        assert_eq!(ConnectorState::Faulted.to_v201(), "Faulted");
        assert_eq!(ConnectorState::Unavailable.to_v201(), "Unavailable");
    }

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = tempdir();
        let mut s =
            StationState::new_default("cpX".into(), OcppVersion::V201, "wss://csms/p".into());
        s.vendor = "TestVendor".into();
        s.heartbeat_interval_s = 77;
        s.save(tmp.path()).unwrap();

        let loaded =
            StationState::load_or_init(tmp.path(), "cpX", OcppVersion::V16, "ws://other/p");
        assert_eq!(loaded.vendor, "TestVendor");
        assert_eq!(loaded.heartbeat_interval_s, 77);
        // CLI overrides must win.
        assert_eq!(loaded.version, OcppVersion::V16);
        assert_eq!(loaded.csms_url, "ws://other/p");
    }

    #[test]
    fn load_or_init_creates_default_when_missing() {
        let tmp = tempdir();
        let s = StationState::load_or_init(tmp.path(), "fresh", OcppVersion::V16, "ws://x");
        assert_eq!(s.id, "fresh");
        assert_eq!(s.connectors.len(), 2);
        assert!(s.log.is_empty());
    }

    #[test]
    fn push_log_enforces_ring_buffer_cap() {
        let mut s = StationState::new_default("cp".into(), OcppVersion::V16, "ws://x".into());
        for i in 0..600 {
            s.push_log(LogEntry {
                ts: Utc::now(),
                direction: "info".into(),
                action: None,
                message: format!("{i}"),
            });
        }
        assert_eq!(s.log.len(), 500, "ring buffer must cap at 500");
        assert_eq!(s.log.last().unwrap().message, "599");
    }

    /// Helper: creates a temp directory that is removed on drop.
    fn tempdir() -> TempDir {
        let base = std::env::temp_dir();
        let mut i = 0u64;
        loop {
            let p = base.join(format!("virtual-occp-test-{}-{}", std::process::id(), i));
            if std::fs::create_dir(&p).is_ok() {
                return TempDir(p);
            }
            i += 1;
        }
    }

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
