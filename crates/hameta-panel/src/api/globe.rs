use crate::auth::AuthUser;
use crate::metrics_collector::GlobeTrafficSnapshot;
use crate::server::PanelState;
use axum::Json;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{self, Stream};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

pub async fn globe_snapshot_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Json<GlobeTrafficSnapshot> {
    Json(state.metrics.current_globe_snapshot().await)
}

pub async fn globe_stream_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.metrics.subscribe_globe();
    let initial = state.metrics.current_globe_snapshot().await;
    let init_seq = initial.seq;
    let init_data = serde_json::to_string(&initial).unwrap_or_else(|_| "{}".to_string());

    let head = stream::once(async move {
        Ok::<Event, Infallible>(
            Event::default()
                .id(init_seq.to_string())
                .event("globe")
                .data(init_data),
        )
    });

    let tail = stream::unfold((rx, init_seq), |(mut rx, mut last_seq)| async move {
        loop {
            match rx.recv().await {
                Ok(snap) => {
                    if snap.seq <= last_seq {
                        continue;
                    }
                    last_seq = snap.seq;
                    let json = serde_json::to_string(&snap).unwrap_or_else(|_| "{}".to_string());
                    let frame = Event::default()
                        .id(snap.seq.to_string())
                        .event("globe")
                        .data(json);
                    return Some((Ok::<Event, Infallible>(frame), (rx, last_seq)));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    let merged = futures::StreamExt::chain(head, tail);
    Sse::new(merged).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
