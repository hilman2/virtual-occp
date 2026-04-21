//! Station actor: orchestrates the OCPP WebSocket, state machine, heartbeat, and
//! MeterValues, and exposes command/event channels to the web server.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, protocol::Message};

use crate::cli::{OcppVersion, StationConfig};
use crate::frame::Frame;
use crate::ocpp::{v16, v201};
use crate::state::{ConnectorState, HistoryEntry, LogEntry, StationState};
use crate::web;

/// Commands sent from the web layer to the station.
#[derive(Debug, Clone)]
pub enum Command {
    PlugIn {
        connector_id: i32,
    },
    Unplug {
        connector_id: i32,
    },
    SwipeCard {
        connector_id: i32,
        id_tag: String,
    },
    StopCharge {
        connector_id: i32,
        reason: String,
    },
    Reconnect,
    SendBoot,
    SetHeartbeatInterval(i32),
    AddTag {
        id_tag: String,
        label: String,
        status: String,
    },
    RemoveTag(String),
    SetFaulted {
        connector_id: i32,
        faulted: bool,
    },
    TriggerMeterValues {
        connector_id: i32,
    },
}

/// Events emitted by the actor, forwarded onto the SSE stream.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Snapshot { state: Box<StationState> },
    Log { entry: LogEntry },
}

#[derive(Clone)]
pub struct Handle {
    pub cmd_tx: mpsc::Sender<Command>,
    pub event_tx: broadcast::Sender<Event>,
    pub state: Arc<Mutex<StationState>>,
}

/// Runtime wrapper around a spawned station — `stop()` aborts actor + web server together.
pub struct StationRuntime {
    #[allow(dead_code)]
    pub handle: Handle,
    join: tokio::task::JoinHandle<()>,
}

impl StationRuntime {
    pub fn stop(self) {
        self.join.abort();
    }
    pub fn is_running(&self) -> bool {
        !self.join.is_finished()
    }
}

/// Spawn a station as a Tokio task (actor + web server). Returns a runtime handle.
pub fn spawn(cfg: StationConfig, data_dir: PathBuf) -> StationRuntime {
    let state = StationState::load_or_init(&data_dir, &cfg.id, cfg.version, &cfg.csms_url);
    let state = Arc::new(Mutex::new(state));

    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(64);
    let (event_tx, _) = broadcast::channel::<Event>(256);

    let handle = Handle {
        cmd_tx: cmd_tx.clone(),
        event_tx: event_tx.clone(),
        state: state.clone(),
    };

    let h_for_task = handle.clone();
    let join = tokio::spawn(async move {
        let http_port = cfg.http_port;
        let station_id = cfg.id.clone();

        let web_h = h_for_task.clone();
        let web_fut = async move {
            if let Err(e) = web::serve(web_h, http_port, station_id).await {
                tracing::error!("Web server stopped: {e:?}");
            }
        };

        let mut actor = Actor {
            cfg,
            state,
            event_tx,
            data_dir,
            pending_calls: HashMap::new(),
            next_v16_tx_id: 1,
            ws: None,
        };
        let actor_fut = async move {
            if let Err(e) = actor.run(cmd_rx).await {
                tracing::error!("Station actor stopped: {e:?}");
            }
        };

        // Both futures run in the same task so that abort() stops them both cleanly.
        tokio::select! {
            _ = web_fut => {},
            _ = actor_fut => {},
        }
    });

    StationRuntime { handle, join }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Actor {
    cfg: StationConfig,
    state: Arc<Mutex<StationState>>,
    event_tx: broadcast::Sender<Event>,
    data_dir: PathBuf,
    /// In-flight calls awaiting CallResult/CallError.
    /// Value carries the action name and optional connector_id for follow-up logic.
    pending_calls: HashMap<String, PendingCall>,
    next_v16_tx_id: i32,
    ws: Option<WsStream>,
}

#[derive(Debug, Clone)]
struct PendingCall {
    action: String,
    connector_id: Option<i32>,
    id_tag: Option<String>,
}

impl Actor {
    async fn run(&mut self, mut cmd_rx: mpsc::Receiver<Command>) -> Result<()> {
        let mut heartbeat_timer = tokio::time::interval(Duration::from_secs(
            self.state.lock().await.heartbeat_interval_s.max(1) as u64,
        ));
        heartbeat_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut meter_timer = tokio::time::interval(Duration::from_secs(10));
        meter_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut reconnect_backoff_s = 1u64;

        // Initial connection attempt.
        self.try_connect().await;

        loop {
            tokio::select! {
                // Commands coming from the web layer
                cmd = cmd_rx.recv() => {
                    let Some(cmd) = cmd else { break; };
                    if let Err(e) = self.handle_command(cmd).await {
                        self.log_warn(format!("Command error: {e}")).await;
                    }
                }
                // Incoming WebSocket messages
                msg = Self::next_ws(&mut self.ws), if self.ws.is_some() => {
                    match msg {
                        Some(Ok(Message::Text(t))) => {
                            if let Err(e) = self.handle_incoming(&t).await {
                                self.log_warn(format!("Incoming frame error: {e}")).await;
                            }
                        }
                        Some(Ok(Message::Ping(p))) => {
                            if let Some(ws) = self.ws.as_mut() {
                                let _ = ws.send(Message::Pong(p)).await;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            self.disconnect("Close from CSMS").await;
                        }
                        Some(Err(e)) => {
                            self.disconnect(&format!("WebSocket error: {e}")).await;
                        }
                        _ => {}
                    }
                }
                // Heartbeat
                _ = heartbeat_timer.tick() => {
                    let interval = self.state.lock().await.heartbeat_interval_s.max(1) as u64;
                    if heartbeat_timer.period() != Duration::from_secs(interval) {
                        heartbeat_timer = tokio::time::interval(Duration::from_secs(interval));
                        heartbeat_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        continue;
                    }
                    if self.is_connected() && self.state.lock().await.boot_accepted {
                        let _ = self.send_heartbeat().await;
                    } else if !self.is_connected() {
                        // Reconnect attempt while disconnected
                        tokio::time::sleep(Duration::from_secs(reconnect_backoff_s)).await;
                        if self.try_connect().await {
                            reconnect_backoff_s = 1;
                        } else {
                            reconnect_backoff_s = (reconnect_backoff_s * 2).min(30);
                        }
                    }
                }
                // MeterValues / runtime tick
                _ = meter_timer.tick() => {
                    if self.is_connected() {
                        self.tick_charging().await;
                    }
                }
            }
        }
        Ok(())
    }

    async fn next_ws(
        ws: &mut Option<WsStream>,
    ) -> Option<tokio_tungstenite::tungstenite::Result<Message>> {
        match ws {
            Some(s) => s.next().await,
            None => std::future::pending().await,
        }
    }

    fn is_connected(&self) -> bool {
        self.ws.is_some()
    }

    // ============ Connection management ============

    async fn try_connect(&mut self) -> bool {
        let url = format!(
            "{}/{}",
            self.cfg.csms_url.trim_end_matches('/'),
            self.cfg.id
        );
        let auth_info = match (&self.cfg.username, &self.cfg.password) {
            (Some(u), _) => format!(" (Basic auth as '{u}')"),
            _ => String::new(),
        };
        self.log_info(format!("Connecting to CSMS: {url}{auth_info}"))
            .await;

        let mut req = match url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => {
                self.log_warn(format!("Invalid CSMS URL: {e}")).await;
                return false;
            }
        };
        req.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            self.cfg.version.subprotocol().parse().unwrap(),
        );
        if let Some(user) = &self.cfg.username {
            use base64::Engine as _;
            let pass = self.cfg.password.clone().unwrap_or_default();
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
            let header_val = format!("Basic {token}");
            match header_val.parse() {
                Ok(v) => {
                    req.headers_mut().insert("Authorization", v);
                }
                Err(e) => {
                    self.log_warn(format!("Invalid Authorization header: {e}"))
                        .await;
                    return false;
                }
            }
        }

        match tokio_tungstenite::connect_async(req).await {
            Ok((ws, response)) => {
                // Verify negotiated subprotocol.
                let negotiated = response
                    .headers()
                    .get("sec-websocket-protocol")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if negotiated != self.cfg.version.subprotocol() {
                    self.log_warn(format!(
                        "CSMS accepted subprotocol '{}', expected '{}'",
                        negotiated,
                        self.cfg.version.subprotocol()
                    ))
                    .await;
                }
                self.ws = Some(ws);
                {
                    let mut st = self.state.lock().await;
                    st.connected = true;
                }
                self.log_info("Connected.".into()).await;
                // Automatically send BootNotification on connect.
                if let Err(e) = self.send_boot().await {
                    self.log_warn(format!("BootNotification failed: {e}")).await;
                }
                self.push_snapshot().await;
                true
            }
            Err(e) => {
                self.log_warn(format!("Connection failed: {e}")).await;
                false
            }
        }
    }

    async fn disconnect(&mut self, reason: &str) {
        self.ws = None;
        {
            let mut st = self.state.lock().await;
            st.connected = false;
            st.boot_accepted = false;
        }
        self.log_warn(format!("Disconnected: {reason}")).await;
        self.push_snapshot().await;
    }

    // ============ Commands ============

    async fn handle_command(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::PlugIn { connector_id } => self.cmd_plug_in(connector_id).await?,
            Command::Unplug { connector_id } => self.cmd_unplug(connector_id).await?,
            Command::SwipeCard {
                connector_id,
                id_tag,
            } => self.cmd_swipe(connector_id, id_tag).await?,
            Command::StopCharge {
                connector_id,
                reason,
            } => self.cmd_stop(connector_id, reason).await?,
            Command::Reconnect => {
                self.disconnect("Manual reconnect").await;
                self.try_connect().await;
            }
            Command::SendBoot => {
                self.send_boot().await?;
            }
            Command::SetHeartbeatInterval(s) => {
                let mut st = self.state.lock().await;
                st.heartbeat_interval_s = s.max(1);
                st.save(&self.data_dir).ok();
                drop(st);
                self.push_snapshot().await;
            }
            Command::AddTag {
                id_tag,
                label,
                status,
            } => {
                let mut st = self.state.lock().await;
                st.tags.retain(|t| t.id_tag != id_tag);
                st.tags.push(crate::state::RfidTag {
                    id_tag,
                    label,
                    status,
                    parent_id_tag: None,
                });
                st.save(&self.data_dir).ok();
                drop(st);
                self.push_snapshot().await;
            }
            Command::RemoveTag(id_tag) => {
                let mut st = self.state.lock().await;
                st.tags.retain(|t| t.id_tag != id_tag);
                st.save(&self.data_dir).ok();
                drop(st);
                self.push_snapshot().await;
            }
            Command::SetFaulted {
                connector_id,
                faulted,
            } => {
                {
                    let mut st = self.state.lock().await;
                    if let Some(c) = st.connector_mut(connector_id) {
                        c.state = if faulted {
                            ConnectorState::Faulted
                        } else {
                            ConnectorState::Available
                        };
                    }
                }
                self.send_status_notification(connector_id).await?;
                self.push_snapshot().await;
            }
            Command::TriggerMeterValues { connector_id } => {
                self.send_meter_values(connector_id).await?;
            }
        }
        Ok(())
    }

    async fn cmd_plug_in(&mut self, connector_id: i32) -> Result<()> {
        {
            let mut st = self.state.lock().await;
            let Some(c) = st.connector_mut(connector_id) else {
                return Err(anyhow!("Unknown connector {connector_id}"));
            };
            if c.state != ConnectorState::Available {
                return Err(anyhow!("Connector not available: {:?}", c.state));
            }
            c.state = ConnectorState::PluggedIn;
        }
        self.send_status_notification(connector_id).await?;
        self.log_info(format!("Cable plugged in at connector {connector_id}"))
            .await;
        self.push_snapshot().await;
        Ok(())
    }

    async fn cmd_unplug(&mut self, connector_id: i32) -> Result<()> {
        // Stop any running transaction first.
        let running = {
            let st = self.state.lock().await;
            st.connector(connector_id)
                .and_then(|c| c.transaction_id.clone().map(|_| ()))
                .is_some()
        };
        if running {
            self.cmd_stop(connector_id, "EVDisconnected".into()).await?;
        }
        {
            let mut st = self.state.lock().await;
            if let Some(c) = st.connector_mut(connector_id) {
                c.state = ConnectorState::Available;
                c.current_tag = None;
            }
        }
        self.send_status_notification(connector_id).await?;
        self.log_info(format!("Cable unplugged at connector {connector_id}"))
            .await;
        self.push_snapshot().await;
        Ok(())
    }

    async fn cmd_swipe(&mut self, connector_id: i32, id_tag: String) -> Result<()> {
        self.log_info(format!(
            "RFID tag '{id_tag}' presented at connector {connector_id}"
        ))
        .await;
        // Always send Authorize first (even if no cable is plugged in).
        match self.cfg.version {
            OcppVersion::V16 => {
                let (call_id, frame) = Frame::new_call(
                    "Authorize",
                    serde_json::to_value(v16::AuthorizeReq {
                        idTag: id_tag.clone(),
                    })?,
                );
                self.pending_calls.insert(
                    call_id,
                    PendingCall {
                        action: "Authorize".into(),
                        connector_id: Some(connector_id),
                        id_tag: Some(id_tag),
                    },
                );
                self.send_frame(frame).await?;
            }
            OcppVersion::V201 => {
                let (call_id, frame) = Frame::new_call(
                    "Authorize",
                    serde_json::to_value(v201::AuthorizeReq {
                        idToken: v201::IdToken {
                            idToken: id_tag.clone(),
                            kind: "ISO14443".into(),
                        },
                    })?,
                );
                self.pending_calls.insert(
                    call_id,
                    PendingCall {
                        action: "Authorize".into(),
                        connector_id: Some(connector_id),
                        id_tag: Some(id_tag),
                    },
                );
                self.send_frame(frame).await?;
            }
        }
        Ok(())
    }

    async fn cmd_stop(&mut self, connector_id: i32, reason: String) -> Result<()> {
        let (txn, id_tag, meter_wh) = {
            let st = self.state.lock().await;
            let c = st
                .connector(connector_id)
                .ok_or_else(|| anyhow!("Unknown connector"))?;
            let Some(txn) = c.transaction_id.clone() else {
                return Err(anyhow!("No active transaction"));
            };
            (txn, c.current_tag.clone().unwrap_or_default(), c.meter_wh)
        };

        match self.cfg.version {
            OcppVersion::V16 => {
                let tx_id: i32 = txn.parse().unwrap_or(0);
                let req = v16::StopTransactionReq {
                    transactionId: tx_id,
                    idTag: if id_tag.is_empty() {
                        None
                    } else {
                        Some(id_tag.clone())
                    },
                    meterStop: meter_wh as i32,
                    timestamp: Utc::now().to_rfc3339(),
                    reason: Some(reason.clone()),
                    transactionData: None,
                };
                let (call_id, frame) =
                    Frame::new_call("StopTransaction", serde_json::to_value(req)?);
                self.pending_calls.insert(
                    call_id,
                    PendingCall {
                        action: "StopTransaction".into(),
                        connector_id: Some(connector_id),
                        id_tag: None,
                    },
                );
                self.send_frame(frame).await?;
            }
            OcppVersion::V201 => {
                let req = v201::TransactionEventReq {
                    eventType: "Ended".into(),
                    timestamp: Utc::now().to_rfc3339(),
                    triggerReason: "StopAuthorized".into(),
                    seqNo: 2,
                    transactionInfo: v201::TransactionInfo {
                        transactionId: txn.clone(),
                        chargingState: Some("Idle".into()),
                        timeSpentCharging: None,
                        stoppedReason: Some(reason.clone()),
                        remoteStartId: None,
                    },
                    idToken: if id_tag.is_empty() {
                        None
                    } else {
                        Some(v201::IdToken {
                            idToken: id_tag.clone(),
                            kind: "ISO14443".into(),
                        })
                    },
                    evse: Some(v201::EVSE {
                        id: connector_id,
                        connectorId: Some(connector_id),
                    }),
                    meterValue: Some(vec![v201::MeterValue {
                        timestamp: Utc::now().to_rfc3339(),
                        sampledValue: vec![v201::SampledValue {
                            value: meter_wh as f64,
                            context: Some("Transaction.End".into()),
                            measurand: Some("Energy.Active.Import.Register".into()),
                            phase: None,
                            location: None,
                            unitOfMeasure: Some(json!({"unit": "Wh"})),
                        }],
                    }]),
                    offline: None,
                };
                let (call_id, frame) =
                    Frame::new_call("TransactionEvent", serde_json::to_value(req)?);
                self.pending_calls.insert(
                    call_id,
                    PendingCall {
                        action: "TransactionEvent".into(),
                        connector_id: Some(connector_id),
                        id_tag: None,
                    },
                );
                self.send_frame(frame).await?;
            }
        }

        // Close out the transaction locally and append to history.
        {
            let now = Utc::now();
            let mut st = self.state.lock().await;
            if let Some(c) = st.connector_mut(connector_id) {
                let started = c.started_at.unwrap_or(now);
                let tag = c.current_tag.clone().unwrap_or_default();
                st.history.push(HistoryEntry {
                    transaction_id: txn,
                    connector_id,
                    id_tag: tag,
                    started,
                    ended: now,
                    wh_consumed: meter_wh,
                });
                if let Some(c) = st.connector_mut(connector_id) {
                    c.state = ConnectorState::Finishing;
                    c.transaction_id = None;
                    c.current_tag = None;
                    c.started_at = None;
                }
            }
            st.save(&self.data_dir).ok();
        }
        self.send_status_notification(connector_id).await?;
        self.push_snapshot().await;
        Ok(())
    }

    // ============ OCPP outgoing ============

    async fn send_boot(&mut self) -> Result<()> {
        let payload = match self.cfg.version {
            OcppVersion::V16 => {
                let st = self.state.lock().await;
                serde_json::to_value(v16::BootNotificationReq {
                    chargePointVendor: st.vendor.clone(),
                    chargePointModel: st.model.clone(),
                    chargePointSerialNumber: Some(st.serial_number.clone()),
                    firmwareVersion: Some(st.firmware_version.clone()),
                    iccid: None,
                    imsi: None,
                    meterType: None,
                    meterSerialNumber: None,
                })?
            }
            OcppVersion::V201 => {
                let st = self.state.lock().await;
                serde_json::to_value(v201::BootNotificationReq {
                    reason: "PowerUp".into(),
                    chargingStation: v201::ChargingStation {
                        model: st.model.clone(),
                        vendorName: st.vendor.clone(),
                        serialNumber: Some(st.serial_number.clone()),
                        firmwareVersion: Some(st.firmware_version.clone()),
                    },
                })?
            }
        };
        let (call_id, frame) = Frame::new_call("BootNotification", payload);
        self.pending_calls.insert(
            call_id,
            PendingCall {
                action: "BootNotification".into(),
                connector_id: None,
                id_tag: None,
            },
        );
        self.send_frame(frame).await
    }

    async fn send_heartbeat(&mut self) -> Result<()> {
        let (call_id, frame) = Frame::new_call("Heartbeat", json!({}));
        self.pending_calls.insert(
            call_id,
            PendingCall {
                action: "Heartbeat".into(),
                connector_id: None,
                id_tag: None,
            },
        );
        self.send_frame(frame).await
    }

    async fn send_status_notification(&mut self, connector_id: i32) -> Result<()> {
        if !self.is_connected() {
            return Ok(());
        }
        let (action, payload) = match self.cfg.version {
            OcppVersion::V16 => {
                let st = self.state.lock().await;
                let c = st
                    .connector(connector_id)
                    .ok_or_else(|| anyhow!("Unknown connector {connector_id}"))?;
                (
                    "StatusNotification",
                    serde_json::to_value(v16::StatusNotificationReq {
                        connectorId: connector_id,
                        errorCode: "NoError".into(),
                        status: c.state.to_v16().into(),
                        timestamp: Some(Utc::now().to_rfc3339()),
                        info: None,
                    })?,
                )
            }
            OcppVersion::V201 => {
                let st = self.state.lock().await;
                let c = st
                    .connector(connector_id)
                    .ok_or_else(|| anyhow!("Unknown connector {connector_id}"))?;
                (
                    "StatusNotification",
                    serde_json::to_value(v201::StatusNotificationReq {
                        timestamp: Utc::now().to_rfc3339(),
                        connectorStatus: c.state.to_v201().into(),
                        evseId: connector_id,
                        connectorId: connector_id,
                    })?,
                )
            }
        };
        let (call_id, frame) = Frame::new_call(action, payload);
        self.pending_calls.insert(
            call_id,
            PendingCall {
                action: action.into(),
                connector_id: Some(connector_id),
                id_tag: None,
            },
        );
        self.send_frame(frame).await
    }

    async fn send_start_transaction(&mut self, connector_id: i32, id_tag: String) -> Result<()> {
        match self.cfg.version {
            OcppVersion::V16 => {
                let meter_start = {
                    let st = self.state.lock().await;
                    st.connector(connector_id)
                        .map(|c| c.meter_wh as i32)
                        .unwrap_or(0)
                };
                let req = v16::StartTransactionReq {
                    connectorId: connector_id,
                    idTag: id_tag.clone(),
                    meterStart: meter_start,
                    timestamp: Utc::now().to_rfc3339(),
                    reservationId: None,
                };
                let (call_id, frame) =
                    Frame::new_call("StartTransaction", serde_json::to_value(req)?);
                self.pending_calls.insert(
                    call_id,
                    PendingCall {
                        action: "StartTransaction".into(),
                        connector_id: Some(connector_id),
                        id_tag: Some(id_tag),
                    },
                );
                self.send_frame(frame).await
            }
            OcppVersion::V201 => {
                let txn_id = uuid::Uuid::new_v4().simple().to_string();
                {
                    let mut st = self.state.lock().await;
                    if let Some(c) = st.connector_mut(connector_id) {
                        c.transaction_id = Some(txn_id.clone());
                        c.current_tag = Some(id_tag.clone());
                        c.started_at = Some(Utc::now());
                        c.state = ConnectorState::Charging;
                    }
                }
                let meter_wh = {
                    let st = self.state.lock().await;
                    st.connector(connector_id).map(|c| c.meter_wh).unwrap_or(0)
                };
                let req = v201::TransactionEventReq {
                    eventType: "Started".into(),
                    timestamp: Utc::now().to_rfc3339(),
                    triggerReason: "Authorized".into(),
                    seqNo: 0,
                    transactionInfo: v201::TransactionInfo {
                        transactionId: txn_id,
                        chargingState: Some("Charging".into()),
                        timeSpentCharging: Some(0),
                        stoppedReason: None,
                        remoteStartId: None,
                    },
                    idToken: Some(v201::IdToken {
                        idToken: id_tag.clone(),
                        kind: "ISO14443".into(),
                    }),
                    evse: Some(v201::EVSE {
                        id: connector_id,
                        connectorId: Some(connector_id),
                    }),
                    meterValue: Some(vec![v201::MeterValue {
                        timestamp: Utc::now().to_rfc3339(),
                        sampledValue: vec![v201::SampledValue {
                            value: meter_wh as f64,
                            context: Some("Transaction.Begin".into()),
                            measurand: Some("Energy.Active.Import.Register".into()),
                            phase: None,
                            location: None,
                            unitOfMeasure: Some(json!({"unit": "Wh"})),
                        }],
                    }]),
                    offline: None,
                };
                let (call_id, frame) =
                    Frame::new_call("TransactionEvent", serde_json::to_value(req)?);
                self.pending_calls.insert(
                    call_id,
                    PendingCall {
                        action: "TransactionEvent".into(),
                        connector_id: Some(connector_id),
                        id_tag: Some(id_tag),
                    },
                );
                self.send_frame(frame).await?;
                self.send_status_notification(connector_id).await?;
                Ok(())
            }
        }
    }

    async fn send_meter_values(&mut self, connector_id: i32) -> Result<()> {
        let (meter_wh, txn_opt) = {
            let st = self.state.lock().await;
            let c = st
                .connector(connector_id)
                .ok_or_else(|| anyhow!("Unknown connector"))?;
            (c.meter_wh, c.transaction_id.clone())
        };
        match self.cfg.version {
            OcppVersion::V16 => {
                let tx_id = txn_opt.as_deref().and_then(|s| s.parse::<i32>().ok());
                let req = v16::MeterValuesReq {
                    connectorId: connector_id,
                    transactionId: tx_id,
                    meterValue: vec![v16::MeterValueEntry {
                        timestamp: Utc::now().to_rfc3339(),
                        sampledValue: vec![v16::SampledValue {
                            value: meter_wh.to_string(),
                            context: Some("Sample.Periodic".into()),
                            format: Some("Raw".into()),
                            measurand: Some("Energy.Active.Import.Register".into()),
                            phase: None,
                            location: Some("Outlet".into()),
                            unit: Some("Wh".into()),
                        }],
                    }],
                };
                let (call_id, frame) = Frame::new_call("MeterValues", serde_json::to_value(req)?);
                self.pending_calls.insert(
                    call_id,
                    PendingCall {
                        action: "MeterValues".into(),
                        connector_id: Some(connector_id),
                        id_tag: None,
                    },
                );
                self.send_frame(frame).await
            }
            OcppVersion::V201 => {
                let Some(txn) = txn_opt else {
                    return Ok(());
                };
                let req = v201::TransactionEventReq {
                    eventType: "Updated".into(),
                    timestamp: Utc::now().to_rfc3339(),
                    triggerReason: "MeterValuePeriodic".into(),
                    seqNo: 1,
                    transactionInfo: v201::TransactionInfo {
                        transactionId: txn,
                        chargingState: Some("Charging".into()),
                        timeSpentCharging: None,
                        stoppedReason: None,
                        remoteStartId: None,
                    },
                    idToken: None,
                    evse: Some(v201::EVSE {
                        id: connector_id,
                        connectorId: Some(connector_id),
                    }),
                    meterValue: Some(vec![v201::MeterValue {
                        timestamp: Utc::now().to_rfc3339(),
                        sampledValue: vec![v201::SampledValue {
                            value: meter_wh as f64,
                            context: Some("Sample.Periodic".into()),
                            measurand: Some("Energy.Active.Import.Register".into()),
                            phase: None,
                            location: None,
                            unitOfMeasure: Some(json!({"unit": "Wh"})),
                        }],
                    }]),
                    offline: None,
                };
                let (call_id, frame) =
                    Frame::new_call("TransactionEvent", serde_json::to_value(req)?);
                self.pending_calls.insert(
                    call_id,
                    PendingCall {
                        action: "TransactionEvent".into(),
                        connector_id: Some(connector_id),
                        id_tag: None,
                    },
                );
                self.send_frame(frame).await
            }
        }
    }

    async fn send_frame(&mut self, frame: Frame) -> Result<()> {
        let wire = frame.to_wire();
        let action = match &frame {
            Frame::Call { action, .. } => Some(action.clone()),
            _ => None,
        };
        if let Some(ws) = self.ws.as_mut() {
            ws.send(Message::Text(wire.clone())).await?;
            self.log_sent(action, &wire).await;
            Ok(())
        } else {
            Err(anyhow!("Not connected"))
        }
    }

    // ============ OCPP incoming ============

    async fn handle_incoming(&mut self, raw: &str) -> Result<()> {
        let frame = Frame::from_wire(raw)?;
        match frame {
            Frame::Call {
                id,
                action,
                payload,
            } => {
                self.log_recv(Some(action.clone()), raw).await;
                self.handle_incoming_call(id, action, payload).await
            }
            Frame::Result { id, payload } => {
                let pending = self.pending_calls.remove(&id);
                let action_label = pending.as_ref().map(|p| p.action.clone());
                self.log_recv(action_label.clone(), raw).await;
                if let Some(p) = pending {
                    self.handle_call_result(p, payload).await?;
                }
                Ok(())
            }
            Frame::Error {
                id,
                code,
                description,
                ..
            } => {
                let pending = self.pending_calls.remove(&id);
                self.log_recv(pending.as_ref().map(|p| p.action.clone()), raw)
                    .await;
                self.log_warn(format!("CallError: {code} / {description}"))
                    .await;
                Ok(())
            }
        }
    }

    async fn handle_incoming_call(
        &mut self,
        id: String,
        action: String,
        payload: Value,
    ) -> Result<()> {
        // Respond politely to every CSMS→CP call, even when we only implement it minimally.
        let response_payload = match action.as_str() {
            // OCPP 1.6
            "Reset" => json!({ "status": "Accepted" }),
            "ChangeAvailability" => json!({ "status": "Accepted" }),
            "ChangeConfiguration" => json!({ "status": "Accepted" }),
            "GetConfiguration" => {
                let st = self.state.lock().await;
                let keys: Vec<Value> = st
                    .config_keys
                    .iter()
                    .map(|(k, v)| json!({"key": k, "readonly": false, "value": v}))
                    .collect();
                json!({ "configurationKey": keys, "unknownKey": [] })
            }
            "RemoteStartTransaction" => {
                let id_tag = payload
                    .get("idTag")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let connector_id = payload
                    .get("connectorId")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(1) as i32;
                // Trigger a remote start locally.
                let _ = self.cmd_swipe(connector_id, id_tag).await;
                json!({ "status": "Accepted" })
            }
            "RemoteStopTransaction" => {
                let tx_id = payload
                    .get("transactionId")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let connector_id = {
                    let st = self.state.lock().await;
                    st.connectors
                        .iter()
                        .find(|c| {
                            c.transaction_id
                                .as_deref()
                                .and_then(|s| s.parse::<i64>().ok())
                                == Some(tx_id)
                        })
                        .map(|c| c.id)
                };
                if let Some(cid) = connector_id {
                    let _ = self.cmd_stop(cid, "Remote".into()).await;
                    json!({ "status": "Accepted" })
                } else {
                    json!({ "status": "Rejected" })
                }
            }
            "UnlockConnector" => json!({ "status": "Unlocked" }),
            "TriggerMessage" => {
                let requested = payload
                    .get("requestedMessage")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let connector_id = payload
                    .get("connectorId")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(1) as i32;
                let accepted = match requested {
                    "BootNotification" => {
                        let _ = self.send_boot().await;
                        true
                    }
                    "Heartbeat" => {
                        let _ = self.send_heartbeat().await;
                        true
                    }
                    "StatusNotification" => {
                        let _ = self.send_status_notification(connector_id).await;
                        true
                    }
                    "MeterValues" => {
                        let _ = self.send_meter_values(connector_id).await;
                        true
                    }
                    _ => false,
                };
                json!({ "status": if accepted { "Accepted" } else { "NotImplemented" } })
            }
            // OCPP 2.0.1
            "GetVariables" => json!({ "getVariableResult": [] }),
            "SetVariables" => json!({ "setVariableResult": [] }),
            "RequestStartTransaction" => json!({ "status": "Accepted" }),
            "RequestStopTransaction" => json!({ "status": "Accepted" }),
            "GetBaseReport" => json!({ "status": "Accepted" }),
            _ => {
                self.log_warn(format!(
                    "Unknown action '{action}', replying NotImplemented"
                ))
                .await;
                let err = Frame::new_error(
                    id,
                    "NotImplemented",
                    &format!("Action '{action}' not implemented"),
                );
                return self.send_frame(err).await;
            }
        };
        let resp = Frame::new_result(id, response_payload);
        self.send_frame(resp).await
    }

    async fn handle_call_result(&mut self, pending: PendingCall, payload: Value) -> Result<()> {
        match pending.action.as_str() {
            "BootNotification" => {
                let status = payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Rejected");
                let interval = payload
                    .get("interval")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(30) as i32;
                {
                    let mut st = self.state.lock().await;
                    st.boot_accepted = status == "Accepted";
                    st.heartbeat_interval_s = interval.max(1);
                    st.save(&self.data_dir).ok();
                }
                self.log_info(format!("Boot: {status} (interval={interval}s)"))
                    .await;
                if status == "Accepted" {
                    // Send an initial StatusNotification for each connector.
                    let ids: Vec<i32> = {
                        let st = self.state.lock().await;
                        st.connectors.iter().map(|c| c.id).collect()
                    };
                    for cid in ids {
                        let _ = self.send_status_notification(cid).await;
                    }
                }
                self.push_snapshot().await;
            }
            "Heartbeat" => {
                let mut st = self.state.lock().await;
                st.last_heartbeat = Some(Utc::now());
                drop(st);
                self.push_snapshot().await;
            }
            "Authorize" => {
                let cid = pending.connector_id.unwrap_or(1);
                let accepted = match self.cfg.version {
                    OcppVersion::V16 => {
                        payload
                            .get("idTagInfo")
                            .and_then(|v| v.get("status"))
                            .and_then(|v| v.as_str())
                            == Some("Accepted")
                    }
                    OcppVersion::V201 => {
                        payload
                            .get("idTokenInfo")
                            .and_then(|v| v.get("status"))
                            .and_then(|v| v.as_str())
                            == Some("Accepted")
                    }
                };
                let id_tag = pending.id_tag.clone().unwrap_or_default();
                self.log_info(format!(
                    "Authorize '{id_tag}' → {}",
                    if accepted { "Accepted" } else { "Rejected" }
                ))
                .await;

                if accepted {
                    // Start a transaction if the cable is already plugged in,
                    // otherwise just remember the tag.
                    let plugged = {
                        let st = self.state.lock().await;
                        st.connector(cid)
                            .map(|c| {
                                matches!(
                                    c.state,
                                    ConnectorState::PluggedIn | ConnectorState::Authorized
                                )
                            })
                            .unwrap_or(false)
                    };
                    if plugged {
                        {
                            let mut st = self.state.lock().await;
                            if let Some(c) = st.connector_mut(cid) {
                                c.state = ConnectorState::Authorized;
                                c.current_tag = Some(id_tag.clone());
                            }
                        }
                        self.send_start_transaction(cid, id_tag).await?;
                    } else {
                        self.log_warn(
                            "No cable plugged in — Authorize without StartTransaction".into(),
                        )
                        .await;
                    }
                }
                self.push_snapshot().await;
            }
            "StartTransaction" => {
                // OCPP 1.6
                let tx_id = payload
                    .get("transactionId")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32;
                let accepted = payload
                    .get("idTagInfo")
                    .and_then(|v| v.get("status"))
                    .and_then(|v| v.as_str())
                    == Some("Accepted");
                let cid = pending.connector_id.unwrap_or(1);
                {
                    let mut st = self.state.lock().await;
                    if let Some(c) = st.connector_mut(cid) {
                        if accepted {
                            c.transaction_id = Some(tx_id.to_string());
                            c.current_tag = pending.id_tag.clone();
                            c.started_at = Some(Utc::now());
                            c.state = ConnectorState::Charging;
                        } else {
                            c.state = ConnectorState::PluggedIn;
                            c.current_tag = None;
                        }
                    }
                    st.save(&self.data_dir).ok();
                }
                self.next_v16_tx_id = (tx_id + 1).max(self.next_v16_tx_id);
                self.log_info(format!(
                    "StartTransaction: id={tx_id}, status={}",
                    if accepted { "Accepted" } else { "Rejected" }
                ))
                .await;
                self.send_status_notification(cid).await?;
                self.push_snapshot().await;
            }
            "StopTransaction" => {
                self.log_info("StopTransaction confirmed".into()).await;
                // Finishing → PluggedIn after a short moment.
                let cid = pending.connector_id.unwrap_or(1);
                {
                    let mut st = self.state.lock().await;
                    if let Some(c) = st.connector_mut(cid) {
                        if matches!(c.state, ConnectorState::Finishing) {
                            c.state = ConnectorState::PluggedIn;
                        }
                    }
                    st.save(&self.data_dir).ok();
                }
                self.send_status_notification(cid).await?;
                self.push_snapshot().await;
            }
            "TransactionEvent" => {
                self.log_info("TransactionEvent confirmed".into()).await;
                let st = self.state.lock().await;
                st.save(&self.data_dir).ok();
            }
            _ => {}
        }
        Ok(())
    }

    // ============ Runtime ticks ============

    async fn tick_charging(&mut self) {
        let charging: Vec<(i32, i32)> = {
            let st = self.state.lock().await;
            st.connectors
                .iter()
                .filter(|c| matches!(c.state, ConnectorState::Charging))
                .map(|c| (c.id, c.max_power_w))
                .collect()
        };
        let any = !charging.is_empty();
        for (cid, max_w) in charging {
            let delta_wh = (max_w as i64) * 10 / 3600; // 10s at max power
            {
                let mut st = self.state.lock().await;
                if let Some(c) = st.connector_mut(cid) {
                    c.meter_wh += delta_wh;
                }
                st.save(&self.data_dir).ok();
            }
            let _ = self.send_meter_values(cid).await;
        }
        if any {
            self.push_snapshot().await;
        }
    }

    // ============ Log + event publishing ============

    async fn push_snapshot(&self) {
        let snap = {
            let st = self.state.lock().await;
            st.save(&self.data_dir).ok();
            st.clone()
        };
        let _ = self.event_tx.send(Event::Snapshot {
            state: Box::new(snap),
        });
    }

    async fn log_sent(&mut self, action: Option<String>, raw: &str) {
        self.push_log_entry(LogEntry {
            ts: Utc::now(),
            direction: "->".into(),
            action,
            message: raw.to_string(),
        })
        .await;
    }

    async fn log_recv(&mut self, action: Option<String>, raw: &str) {
        self.push_log_entry(LogEntry {
            ts: Utc::now(),
            direction: "<-".into(),
            action,
            message: raw.to_string(),
        })
        .await;
    }

    async fn log_info(&mut self, msg: String) {
        tracing::info!(station=%self.cfg.id, "{msg}");
        self.push_log_entry(LogEntry {
            ts: Utc::now(),
            direction: "info".into(),
            action: None,
            message: msg,
        })
        .await;
    }

    async fn log_warn(&mut self, msg: String) {
        tracing::warn!(station=%self.cfg.id, "{msg}");
        self.push_log_entry(LogEntry {
            ts: Utc::now(),
            direction: "warn".into(),
            action: None,
            message: msg,
        })
        .await;
    }

    async fn push_log_entry(&mut self, entry: LogEntry) {
        {
            let mut st = self.state.lock().await;
            st.push_log(entry.clone());
        }
        let _ = self.event_tx.send(Event::Log { entry });
    }
}

fn _unused_dt_marker(_: DateTime<Utc>) {}
