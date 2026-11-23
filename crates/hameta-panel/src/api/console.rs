use crate::auth::AuthUser;
use crate::ingest_hub::{ConsoleIngestEvent, IngestHubStats};
use crate::server::PanelState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::{self, Stream};
use hameta_common::IngestEventType;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_RECENT_LIMIT: usize = 100;
const MAX_RECENT_LIMIT: usize = 500;
const DEFAULT_SSE_REPLAY: usize = 50;
const MAX_SSE_EPS: u32 = 200;

#[derive(Debug, Deserialize)]
pub struct ConsoleRecentQuery {
    pub limit: Option<usize>,
    pub event_type: Option<String>,
    pub worker_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConsoleRecentResponse {
    pub stats: IngestHubStats,
    pub events: Vec<ConsoleIngestEvent>,
}

#[derive(Debug, Deserialize)]
pub struct ConsoleStreamQuery {
    pub event_type: Option<String>,
    pub worker_id: Option<String>,
    pub replay: Option<usize>,
    pub max_eps: Option<u32>,
}

struct StreamUnfoldState {
    rx: tokio::sync::broadcast::Receiver<ConsoleIngestEvent>,
    last_sent_seq: u64,
    event_type: Option<IngestEventType>,
    worker_id: Option<String>,
    max_eps: Option<u32>,
    window_start: Instant,
    sent_in_window: u32,
    throttled_dropped: u64,
    pending_event: Option<ConsoleIngestEvent>,
}

pub async fn console_recent_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Query(query): Query<ConsoleRecentQuery>,
) -> Json<ConsoleRecentResponse> {
    let hub = state.supervisor.ingest_hub();
    let event_type = parse_event_type_filter(query.event_type.as_deref());
    let worker_id = normalize_worker_filter(query.worker_id.as_deref());
    let limit = query
        .limit
        .unwrap_or(DEFAULT_RECENT_LIMIT)
        .clamp(1, MAX_RECENT_LIMIT);

    // 当带有过滤条件时扫描全部环形缓存以尽量填满 limit
    let fetch_cap = if event_type.is_some() || worker_id.is_some() {
        MAX_RECENT_LIMIT
    } else {
        limit
    };
    let mut events: Vec<ConsoleIngestEvent> = hub
        .replay_after(None, fetch_cap)
        .into_iter()
        .filter(|ev| matches_console_filters(ev, event_type, worker_id.as_deref()))
        .collect();
    if events.len() > limit {
        let drop_n = events.len() - limit;
        events.drain(..drop_n);
    }

    Json(ConsoleRecentResponse {
        stats: hub.stats(),
        events,
    })
}

pub async fn console_ingest_stream_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    headers: HeaderMap,
    Query(query): Query<ConsoleStreamQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let hub = state.supervisor.ingest_hub();
    let event_type = parse_event_type_filter(query.event_type.as_deref());
    let worker_id = normalize_worker_filter(query.worker_id.as_deref());
    let max_eps = query
        .max_eps
        .filter(|&v| v > 0)
        .map(|v| v.clamp(1, MAX_SSE_EPS));
    let last_event_id = parse_last_event_id(&headers);
    let replay_limit = if last_event_id.is_some() {
        MAX_RECENT_LIMIT
    } else {
        query
            .replay
            .unwrap_or(DEFAULT_SSE_REPLAY)
            .min(MAX_RECENT_LIMIT)
    };

    // 先订阅广播再读取环形缓冲，确保回放与实时流无缝衔接不丢事件
    let rx = hub.subscribe();
    let stats = hub.stats();
    let replayed: Vec<ConsoleIngestEvent> = hub
        .replay_after(last_event_id, replay_limit)
        .into_iter()
        .filter(|ev| matches_console_filters(ev, event_type, worker_id.as_deref()))
        .collect();

    let mut last_sent_seq = last_event_id.unwrap_or(0);
    for ev in &replayed {
        if ev.seq > last_sent_seq {
            last_sent_seq = ev.seq;
        }
    }

    let ready_payload = serde_json::json!({
        "latest_seq": stats.latest_seq,
        "ring_len": stats.ring_len,
        "ring_capacity": stats.ring_capacity,
        "resumed_from": last_event_id,
        "max_eps": max_eps,
    });
    let ready_event = Event::default()
        .event("ready")
        .data(ready_payload.to_string());

    let mut initial_frames: Vec<Result<Event, Infallible>> = Vec::with_capacity(replayed.len() + 1);
    initial_frames.push(Ok(ready_event));
    for ev in replayed {
        let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string());
        initial_frames.push(Ok(Event::default()
            .id(ev.seq.to_string())
            .event("ingest")
            .data(data)));
    }
    let head = stream::iter(initial_frames);

    let init_state = StreamUnfoldState {
        rx,
        last_sent_seq,
        event_type,
        worker_id,
        max_eps,
        window_start: Instant::now(),
        sent_in_window: 0,
        throttled_dropped: 0,
        pending_event: None,
    };

    let tail = stream::unfold(init_state, |mut st| async move {
        loop {
            if let Some(ev) = st.pending_event.take() {
                if st.max_eps.is_some() {
                    st.sent_in_window = st.sent_in_window.saturating_add(1);
                }
                let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string());
                let frame = Event::default()
                    .id(ev.seq.to_string())
                    .event("ingest")
                    .data(data);
                return Some((Ok::<Event, Infallible>(frame), st));
            }

            match st.rx.recv().await {
                Ok(ev) => {
                    if ev.seq <= st.last_sent_seq {
                        continue;
                    }
                    if !matches_console_filters(&ev, st.event_type, st.worker_id.as_deref()) {
                        st.last_sent_seq = ev.seq;
                        continue;
                    }
                    st.last_sent_seq = ev.seq;

                    if let Some(cap) = st.max_eps {
                        if st.window_start.elapsed() >= Duration::from_secs(1) {
                            st.window_start = Instant::now();
                            st.sent_in_window = 0;
                            if st.throttled_dropped > 0 {
                                let skipped = st.throttled_dropped;
                                st.throttled_dropped = 0;
                                st.pending_event = Some(ev);
                                let lag_payload = serde_json::json!({
                                    "skipped": skipped,
                                    "reason": "rate_limit"
                                })
                                .to_string();
                                let frame = Event::default().event("lagged").data(lag_payload);
                                return Some((Ok::<Event, Infallible>(frame), st));
                            }
                        }
                        if st.sent_in_window >= cap {
                            st.throttled_dropped = st.throttled_dropped.saturating_add(1);
                            continue;
                        }
                        st.sent_in_window = st.sent_in_window.saturating_add(1);
                    }

                    let data = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".to_string());
                    let frame = Event::default()
                        .id(ev.seq.to_string())
                        .event("ingest")
                        .data(data);
                    return Some((Ok::<Event, Infallible>(frame), st));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    let total_skipped = skipped.saturating_add(st.throttled_dropped);
                    st.throttled_dropped = 0;
                    let lag_payload = serde_json::json!({ "skipped": total_skipped }).to_string();
                    let frame = Event::default().event("lagged").data(lag_payload);
                    return Some((Ok::<Event, Infallible>(frame), st));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    let merged = futures::StreamExt::chain(head, tail);
    Sse::new(merged).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

fn parse_last_event_id(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
}

fn parse_event_type_filter(raw: Option<&str>) -> Option<IngestEventType> {
    let val = raw?.trim().to_ascii_lowercase();
    match val.as_str() {
        "indexed" => Some(IngestEventType::Indexed),
        "hot_updated" => Some(IngestEventType::HotUpdated),
        _ => None,
    }
}

fn normalize_worker_filter(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("all") {
        None
    } else {
        Some(s.to_string())
    }
}

fn matches_console_filters(
    ev: &ConsoleIngestEvent,
    event_type: Option<IngestEventType>,
    worker_id: Option<&str>,
) -> bool {
    if let Some(expected_type) = event_type
        && ev.item.event_type != expected_type
    {
        return false;
    }
    if let Some(expected_worker) = worker_id
        && ev.item.worker_id != expected_worker
    {
        return false;
    }
    true
}
