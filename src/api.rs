use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::station::{Command, Handle};

pub fn router() -> Router<Handle> {
    Router::new()
        .route("/api/state", get(get_state))
        .route("/api/plug", post(plug))
        .route("/api/unplug", post(unplug))
        .route("/api/swipe", post(swipe))
        .route("/api/stop", post(stop))
        .route("/api/boot", post(boot))
        .route("/api/reconnect", post(reconnect))
        .route("/api/heartbeat_interval", post(hb_interval))
        .route("/api/tags", post(add_tag))
        .route("/api/tags/:id_tag", delete(remove_tag))
        .route("/api/fault", post(fault))
        .route("/api/meter", post(meter))
}

async fn get_state(State(h): State<Handle>) -> impl IntoResponse {
    let s = h.state.lock().await.clone();
    Json(s)
}

#[derive(Deserialize)]
struct ConnectorOnly {
    connector_id: i32,
}

async fn plug(State(h): State<Handle>, Json(b): Json<ConnectorOnly>) -> impl IntoResponse {
    send(
        &h,
        Command::PlugIn {
            connector_id: b.connector_id,
        },
    )
    .await
}

async fn unplug(State(h): State<Handle>, Json(b): Json<ConnectorOnly>) -> impl IntoResponse {
    send(
        &h,
        Command::Unplug {
            connector_id: b.connector_id,
        },
    )
    .await
}

#[derive(Deserialize)]
struct SwipeBody {
    connector_id: i32,
    id_tag: String,
}

async fn swipe(State(h): State<Handle>, Json(b): Json<SwipeBody>) -> impl IntoResponse {
    send(
        &h,
        Command::SwipeCard {
            connector_id: b.connector_id,
            id_tag: b.id_tag,
        },
    )
    .await
}

#[derive(Deserialize)]
struct StopBody {
    connector_id: i32,
    #[serde(default)]
    reason: Option<String>,
}

async fn stop(State(h): State<Handle>, Json(b): Json<StopBody>) -> impl IntoResponse {
    send(
        &h,
        Command::StopCharge {
            connector_id: b.connector_id,
            reason: b.reason.unwrap_or_else(|| "Local".into()),
        },
    )
    .await
}

async fn boot(State(h): State<Handle>) -> impl IntoResponse {
    send(&h, Command::SendBoot).await
}

async fn reconnect(State(h): State<Handle>) -> impl IntoResponse {
    send(&h, Command::Reconnect).await
}

#[derive(Deserialize)]
struct HbBody {
    seconds: i32,
}

async fn hb_interval(State(h): State<Handle>, Json(b): Json<HbBody>) -> impl IntoResponse {
    send(&h, Command::SetHeartbeatInterval(b.seconds)).await
}

#[derive(Deserialize)]
struct TagBody {
    id_tag: String,
    label: String,
    #[serde(default = "accepted")]
    status: String,
}
fn accepted() -> String {
    "Accepted".into()
}

async fn add_tag(State(h): State<Handle>, Json(b): Json<TagBody>) -> impl IntoResponse {
    send(
        &h,
        Command::AddTag {
            id_tag: b.id_tag,
            label: b.label,
            status: b.status,
        },
    )
    .await
}

async fn remove_tag(State(h): State<Handle>, Path(id_tag): Path<String>) -> impl IntoResponse {
    send(&h, Command::RemoveTag(id_tag)).await
}

#[derive(Deserialize)]
struct FaultBody {
    connector_id: i32,
    faulted: bool,
}

async fn fault(State(h): State<Handle>, Json(b): Json<FaultBody>) -> impl IntoResponse {
    send(
        &h,
        Command::SetFaulted {
            connector_id: b.connector_id,
            faulted: b.faulted,
        },
    )
    .await
}

async fn meter(State(h): State<Handle>, Json(b): Json<ConnectorOnly>) -> impl IntoResponse {
    send(
        &h,
        Command::TriggerMeterValues {
            connector_id: b.connector_id,
        },
    )
    .await
}

async fn send(h: &Handle, cmd: Command) -> impl IntoResponse {
    match h.cmd_tx.send(cmd).await {
        Ok(_) => (StatusCode::ACCEPTED, "ok").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}
