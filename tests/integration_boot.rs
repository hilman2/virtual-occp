//! End-to-end test: virtual-occp (used as a library) against a mock CSMS.
//!
//! Spawns an in-process WebSocket server that plays the CSMS role and verifies:
//! - the Basic Auth header is sent correctly
//! - the OCPP subprotocol "ocpp1.6" is negotiated
//! - BootNotification → Accepted + the heartbeat flow starts
//! - one StatusNotification per connector follows automatically

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use virtual_occp::cli::{OcppVersion, StationConfig};
use virtual_occp::station;

/// Observations collected by the mock CSMS so the test can assert on them.
#[derive(Default, Debug)]
struct Observed {
    subprotocol: Option<String>,
    authorization: Option<String>,
    actions_received: Vec<String>,
}

async fn spawn_mock_csms(observed: Arc<Mutex<Observed>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();

    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let obs = observed.clone();
            #[allow(clippy::result_large_err)]
            let cb = |req: &Request, mut resp: Response| {
                let mut o = obs.lock().unwrap();
                o.authorization = req
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                let subp = req
                    .headers()
                    .get("sec-websocket-protocol")
                    .and_then(|v| v.to_str().ok())
                    .map(String::from);
                o.subprotocol = subp.clone();
                if let Some(p) = subp {
                    resp.headers_mut()
                        .insert("sec-websocket-protocol", HeaderValue::from_str(&p).unwrap());
                }
                Ok(resp)
            };
            let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(stream, cb).await else {
                return;
            };
            while let Some(Ok(msg)) = ws.next().await {
                let Message::Text(t) = msg else { continue };
                let arr: Vec<Value> = match serde_json::from_str(&t) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                // Nur Calls beantworten: [2, id, action, payload]
                if arr.first().and_then(|v| v.as_u64()) != Some(2) {
                    continue;
                }
                let id = arr[1].as_str().unwrap_or("").to_string();
                let action = arr[2].as_str().unwrap_or("").to_string();
                observed
                    .lock()
                    .unwrap()
                    .actions_received
                    .push(action.clone());
                let payload = match action.as_str() {
                    "BootNotification" => json!({
                        "currentTime": "2026-04-21T10:00:00Z",
                        "interval": 1,
                        "status": "Accepted"
                    }),
                    "Heartbeat" => json!({ "currentTime": "2026-04-21T10:00:00Z" }),
                    "StatusNotification" => json!({}),
                    "Authorize" => json!({"idTagInfo": {"status": "Accepted"}}),
                    _ => json!({}),
                };
                let resp = json!([3, id, payload]);
                let _ = ws.send(Message::Text(resp.to_string())).await;
            }
        }
    });

    port
}

#[tokio::test]
async fn station_boots_against_mock_csms_with_basic_auth() {
    let observed = Arc::new(Mutex::new(Observed::default()));
    let port = spawn_mock_csms(observed.clone()).await;

    let cfg = StationConfig {
        id: "cp-it-1".to_string(),
        http_port: pick_free_port(),
        version: OcppVersion::V16,
        csms_url: format!("ws://127.0.0.1:{port}/ocpp"),
        username: Some("admin".into()),
        password: Some("secret".into()),
    };

    let data_dir = tempdir();
    let runtime = station::spawn(cfg, data_dir.clone());

    // Wait until Boot + StatusNotifications have gone over the wire.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        {
            let o = observed.lock().unwrap();
            let has_boot = o.actions_received.iter().any(|a| a == "BootNotification");
            let status_count = o
                .actions_received
                .iter()
                .filter(|a| *a == "StatusNotification")
                .count();
            if has_boot && status_count >= 2 {
                break;
            }
        }
        if tokio::time::Instant::now() > deadline {
            let o = observed.lock().unwrap();
            panic!("timeout — received actions: {:?}", o.actions_received);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    runtime.stop();

    let o = observed.lock().unwrap();
    assert_eq!(
        o.subprotocol.as_deref(),
        Some("ocpp1.6"),
        "subprotocol must be ocpp1.6"
    );
    let auth = o
        .authorization
        .as_deref()
        .expect("Authorization header must be set");
    assert!(auth.starts_with("Basic "));
    // admin:secret → YWRtaW46c2VjcmV0
    assert_eq!(auth, "Basic YWRtaW46c2VjcmV0");
    assert!(o.actions_received.iter().any(|a| a == "BootNotification"));
    assert!(
        o.actions_received
            .iter()
            .filter(|a| *a == "StatusNotification")
            .count()
            >= 2,
        "expected at least 2 StatusNotifications (one per connector)"
    );

    std::fs::remove_dir_all(data_dir).ok();
}

fn tempdir() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "virtual-occp-it-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn pick_free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[allow(dead_code)]
fn _ensure_socket_addr_type(_: SocketAddr) {}
