use axum::{
    extract::State,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
    routing::get,
    Router,
};
use futures_util::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::station::{Event, Handle};

pub fn router() -> Router<Handle> {
    Router::new().route("/api/events", get(events))
}

async fn events(State(h): State<Handle>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    // Erstes Event: aktueller Snapshot
    let initial = {
        let s = h.state.lock().await.clone();
        SseEvent::default()
            .event("snapshot")
            .json_data(&Event::Snapshot { state: Box::new(s) })
            .unwrap()
    };

    let rx = h.event_tx.subscribe();
    let live = BroadcastStream::new(rx).filter_map(|res| {
        let ev = res.ok()?;
        let kind = match &ev {
            Event::Snapshot { .. } => "snapshot",
            Event::Log { .. } => "log",
        };
        Some(Ok::<_, Infallible>(
            SseEvent::default().event(kind).json_data(&ev).unwrap(),
        ))
    });

    let stream = tokio_stream::once(Ok(initial)).chain(live);

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
