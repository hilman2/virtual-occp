//! Web server + REST API for the Station Manager.

use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::Deserialize;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

use crate::cli::OcppVersion;
use crate::manager::{Manager, StationDef};

#[derive(RustEmbed)]
#[folder = "manager-assets/"]
struct ManagerAssets;

pub async fn serve(mgr: Manager, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/api/manager/stations", get(list).post(create))
        .route("/api/manager/stations/:id", delete(remove))
        .route("/api/manager/stations/:id/start", post(start))
        .route("/api/manager/stations/:id/stop", post(stop))
        .route("/api/manager/stations/:id", axum::routing::put(update))
        .route("/*file", get(static_file))
        .layer(CorsLayer::permissive())
        .with_state(mgr);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Station Manager listening on http://{addr}");
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// --------- Static ---------

async fn index() -> impl IntoResponse {
    serve_file("index.html")
}

async fn static_file(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    serve_file(if path.is_empty() { "index.html" } else { path })
}

fn serve_file(path: &str) -> Response {
    let (file, actual) = match ManagerAssets::get(path) {
        Some(f) => (f, path.to_string()),
        None => match ManagerAssets::get("index.html") {
            Some(f) => (f, "index.html".to_string()),
            None => return (StatusCode::NOT_FOUND, "not found").into_response(),
        },
    };
    let mime = mime_guess::from_path(&actual).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}

// --------- API ---------

async fn list(State(m): State<Manager>) -> impl IntoResponse {
    Json(m.list().await)
}

#[derive(Deserialize)]
struct CreateBody {
    id: String,
    http_port: u16,
    version: OcppVersion,
    csms_url: String,
    #[serde(default = "default_true")]
    autostart: bool,
    #[serde(default = "default_true")]
    start_now: bool,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}
fn default_true() -> bool {
    true
}

fn normalize_credentials(b: &mut CreateBody) {
    // If the URL contains user:pass@, move the credentials into the explicit fields.
    let (clean, u_url, p_url) = crate::cli::split_credentials(&b.csms_url);
    b.csms_url = clean;
    if b.username.as_deref().unwrap_or("").is_empty() {
        b.username = u_url;
    }
    if b.password.as_deref().unwrap_or("").is_empty() {
        b.password = p_url;
    }
    if b.username.as_deref() == Some("") {
        b.username = None;
    }
    if b.password.as_deref() == Some("") {
        b.password = None;
    }
}

async fn create(State(m): State<Manager>, Json(mut b): Json<CreateBody>) -> Response {
    if !(b.csms_url.starts_with("ws://") || b.csms_url.starts_with("wss://")) {
        return (
            StatusCode::BAD_REQUEST,
            "csms_url must start with ws:// or wss://",
        )
            .into_response();
    }
    if b.id.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "id must not be empty").into_response();
    }
    normalize_credentials(&mut b);
    let def = StationDef {
        id: b.id,
        http_port: b.http_port,
        version: b.version,
        csms_url: b.csms_url,
        autostart: b.autostart,
        username: b.username,
        password: b.password,
    };
    if let Err(e) = m.upsert(def.clone()).await {
        return (StatusCode::BAD_REQUEST, format!("{e}")).into_response();
    }
    if b.start_now {
        if let Err(e) = m.start(&def.id).await {
            return (
                StatusCode::BAD_REQUEST,
                format!("Created, but start failed: {e}"),
            )
                .into_response();
        }
    }
    (StatusCode::CREATED, "ok").into_response()
}

async fn update(
    State(m): State<Manager>,
    Path(id): Path<String>,
    Json(mut b): Json<CreateBody>,
) -> Response {
    if b.id != id {
        return (StatusCode::BAD_REQUEST, "id mismatch").into_response();
    }
    // Updates require the station to be stopped first.
    let _ = m.stop(&id).await;
    normalize_credentials(&mut b);
    let def = StationDef {
        id: b.id,
        http_port: b.http_port,
        version: b.version,
        csms_url: b.csms_url,
        autostart: b.autostart,
        username: b.username,
        password: b.password,
    };
    if let Err(e) = m.upsert(def.clone()).await {
        return (StatusCode::BAD_REQUEST, format!("{e}")).into_response();
    }
    if b.start_now {
        if let Err(e) = m.start(&def.id).await {
            return (
                StatusCode::BAD_REQUEST,
                format!("Update ok, but start failed: {e}"),
            )
                .into_response();
        }
    }
    (StatusCode::OK, "ok").into_response()
}

async fn remove(State(m): State<Manager>, Path(id): Path<String>) -> Response {
    match m.delete(&id).await {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}")).into_response(),
    }
}

async fn start(State(m): State<Manager>, Path(id): Path<String>) -> Response {
    match m.start(&id).await {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}")).into_response(),
    }
}

async fn stop(State(m): State<Manager>, Path(id): Path<String>) -> Response {
    match m.stop(&id).await {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e}")).into_response(),
    }
}
