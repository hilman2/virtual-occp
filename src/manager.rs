//! Station Manager: registry of all virtual stations + web server to spawn/stop them.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::cli::{OcppVersion, StationConfig};
use crate::station::{self, StationRuntime};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationDef {
    pub id: String,
    pub http_port: u16,
    pub version: OcppVersion,
    pub csms_url: String,
    #[serde(default = "default_true")]
    pub autostart: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

fn default_true() -> bool {
    true
}

impl StationDef {
    pub fn to_config(&self) -> StationConfig {
        StationConfig {
            id: self.id.clone(),
            http_port: self.http_port,
            version: self.version,
            csms_url: self.csms_url.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManagerFile {
    #[serde(default)]
    pub stations: Vec<StationDef>,
}

struct Entry {
    def: StationDef,
    runtime: Option<StationRuntime>,
}

#[derive(Clone)]
pub struct Manager {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
    data_dir: PathBuf,
}

impl Manager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            data_dir,
        }
    }

    fn manager_file(&self) -> PathBuf {
        self.data_dir.join("manager.json")
    }

    pub async fn load_persisted(&self) -> Result<()> {
        let path = self.manager_file();
        if !path.exists() {
            return Ok(());
        }
        let text = std::fs::read_to_string(&path)?;
        let file: ManagerFile = serde_json::from_str(&text)?;
        let mut inner = self.inner.lock().await;
        for def in file.stations {
            inner
                .entry(def.id.clone())
                .or_insert(Entry { def, runtime: None });
        }
        Ok(())
    }

    async fn persist(&self) -> Result<()> {
        let inner = self.inner.lock().await;
        let file = ManagerFile {
            stations: inner.values().map(|e| e.def.clone()).collect(),
        };
        let path = self.manager_file();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(&file)?)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Insert or replace a station definition. Does NOT start it.
    pub async fn upsert(&self, def: StationDef) -> Result<()> {
        self.assert_port_free(&def.id, def.http_port).await?;
        let mut inner = self.inner.lock().await;
        match inner.get_mut(&def.id) {
            Some(e) => {
                if e.runtime.is_some() {
                    return Err(anyhow!("Station '{}' is running; stop it first", def.id));
                }
                e.def = def;
            }
            None => {
                inner.insert(def.id.clone(), Entry { def, runtime: None });
            }
        }
        drop(inner);
        self.persist().await
    }

    /// Start the station (if it exists and is not already running).
    pub async fn start(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let Some(e) = inner.get_mut(id) else {
            return Err(anyhow!("Unknown station '{id}'"));
        };
        if e.runtime.as_ref().map(|r| r.is_running()).unwrap_or(false) {
            return Err(anyhow!("Station '{id}' is already running"));
        }
        let cfg = e.def.to_config();
        let runtime = station::spawn(cfg, self.data_dir.clone());
        e.runtime = Some(runtime);
        Ok(())
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let Some(e) = inner.get_mut(id) else {
            return Err(anyhow!("Unknown station '{id}'"));
        };
        if let Some(rt) = e.runtime.take() {
            rt.stop();
        }
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        self.stop(id).await.ok();
        let mut inner = self.inner.lock().await;
        inner.remove(id);
        drop(inner);
        self.persist().await?;
        Ok(())
    }

    pub async fn list(&self) -> Vec<StationStatus> {
        let inner = self.inner.lock().await;
        inner
            .values()
            .map(|e| StationStatus {
                id: e.def.id.clone(),
                http_port: e.def.http_port,
                version: e.def.version,
                csms_url: e.def.csms_url.clone(),
                autostart: e.def.autostart,
                running: e.runtime.as_ref().map(|r| r.is_running()).unwrap_or(false),
                username: e.def.username.clone(),
                has_password: e.def.password.is_some(),
            })
            .collect()
    }

    pub async fn autostart_all(&self) -> Result<()> {
        let ids: Vec<String> = {
            let inner = self.inner.lock().await;
            inner
                .values()
                .filter(|e| e.def.autostart)
                .map(|e| e.def.id.clone())
                .collect()
        };
        for id in ids {
            if let Err(e) = self.start(&id).await {
                tracing::warn!("Autostart '{id}' failed: {e}");
            }
        }
        Ok(())
    }

    async fn assert_port_free(&self, new_id: &str, new_port: u16) -> Result<()> {
        let inner = self.inner.lock().await;
        for e in inner.values() {
            if e.def.id != new_id && e.def.http_port == new_port {
                return Err(anyhow!(
                    "Port {new_port} is already used by station '{}'",
                    e.def.id
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StationStatus {
    pub id: String,
    pub http_port: u16,
    pub version: OcppVersion,
    pub csms_url: String,
    pub autostart: bool,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub has_password: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "virtual-occp-mgr-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn def(id: &str, port: u16) -> StationDef {
        StationDef {
            id: id.into(),
            http_port: port,
            version: OcppVersion::V16,
            csms_url: "ws://localhost:9999/test".into(),
            autostart: false,
            username: None,
            password: None,
        }
    }

    #[tokio::test]
    async fn upsert_persists_and_lists() {
        let dir = tempdir();
        let m = Manager::new(dir.clone());
        m.upsert(def("a", 21000)).await.unwrap();
        m.upsert(def("b", 21001)).await.unwrap();
        let list = m.list().await;
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|s| !s.running));
        assert!(dir.join("manager.json").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn reload_from_disk_restores_defs() {
        let dir = tempdir();
        {
            let m = Manager::new(dir.clone());
            m.upsert(def("a", 21010)).await.unwrap();
        }
        let m2 = Manager::new(dir.clone());
        m2.load_persisted().await.unwrap();
        let list = m2.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "a");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn port_conflict_is_rejected() {
        let dir = tempdir();
        let m = Manager::new(dir.clone());
        m.upsert(def("a", 21020)).await.unwrap();
        let err = m.upsert(def("b", 21020)).await.unwrap_err();
        assert!(format!("{err}").to_lowercase().contains("port"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn same_id_can_be_updated_when_stopped() {
        let dir = tempdir();
        let m = Manager::new(dir.clone());
        m.upsert(def("a", 21030)).await.unwrap();
        let mut d = def("a", 21031);
        d.csms_url = "ws://other/path".into();
        m.upsert(d).await.unwrap();
        let list = m.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].http_port, 21031);
        assert_eq!(list[0].csms_url, "ws://other/path");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn delete_removes_from_registry_and_file() {
        let dir = tempdir();
        let m = Manager::new(dir.clone());
        m.upsert(def("a", 21040)).await.unwrap();
        m.delete("a").await.unwrap();
        assert!(m.list().await.is_empty());
        let persisted = std::fs::read_to_string(dir.join("manager.json")).unwrap();
        let f: ManagerFile = serde_json::from_str(&persisted).unwrap();
        assert!(f.stations.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn start_and_stop_toggle_running_flag() {
        let dir = tempdir();
        let m = Manager::new(dir.clone());
        // Use a high port to avoid clashes in CI.
        m.upsert(def("t1", 31050)).await.unwrap();
        m.start("t1").await.unwrap();
        // Give the spawned task a moment to come up.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let running = m
            .list()
            .await
            .into_iter()
            .find(|s| s.id == "t1")
            .unwrap()
            .running;
        assert!(running, "station should be running");
        m.stop("t1").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let running = m
            .list()
            .await
            .into_iter()
            .find(|s| s.id == "t1")
            .unwrap()
            .running;
        assert!(!running, "station should be stopped");
        std::fs::remove_dir_all(dir).ok();
    }
}
