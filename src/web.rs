use anyhow::Result;
use axum::Router;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

use crate::api;
use crate::assets;
use crate::sse;
use crate::station::Handle;

pub async fn serve(handle: Handle, port: u16, station_id: String) -> Result<()> {
    let app = Router::new()
        .merge(api::router())
        .merge(sse::router())
        .merge(assets::router())
        .layer(CorsLayer::permissive())
        .with_state(handle);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Station '{station_id}' listening on http://{}", addr);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
