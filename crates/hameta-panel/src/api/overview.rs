use crate::api::error::ApiError;
use crate::auth::AuthUser;
use crate::metrics_collector::{HistoryPointDto, OverviewSnapshot};
use crate::server::PanelState;
use axum::Json;
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use chrono::Utc;
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub range: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OverviewHistoryResponse {
    pub range: String,
    pub granularity: &'static str,
    pub points: Vec<HistoryPointDto>,
}

pub async fn overview_snapshot_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Json<OverviewSnapshot> {
    Json(state.metrics.current_snapshot().await)
}

pub async fn overview_history_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<OverviewHistoryResponse>, ApiError> {
    let range = query
        .range
        .as_deref()
        .unwrap_or("1h")
        .trim()
        .to_ascii_lowercase();
    let now = Utc::now().timestamp();

    match range.as_str() {
        "1h" | "6h" | "24h" => {
            let (secs, limit) = match range.as_str() {
                "1h" => (3600_i64, 120_i64),
                "6h" => (6 * 3600_i64, 420_i64),
                _ => (24 * 3600_i64, 1500_i64),
            };
            let rows = state.db.query_metrics_minute(now - secs, limit).await?;
            let mut points: Vec<HistoryPointDto> = rows
                .into_iter()
                .map(|r| HistoryPointDto {
                    ts: r.ts_minute,
                    hashes_per_min: r.hashes_per_min,
                    metadata_succeeded_per_min: r.metadata_succeeded_per_min,
                    stream_pushed_per_min: r.stream_pushed_per_min,
                    filtered_per_min: r.filtered_per_min,
                    indexed_written_per_min: r.indexed_written_per_min,
                    hot_updated_per_min: r.hot_updated_per_min,
                    actual_qps: r.actual_qps,
                    stream_new_len: r.stream_new_len,
                    stream_hot_len: r.stream_hot_len,
                    stream_new_lag: r.stream_new_lag,
                    udp_rx_bps: r.udp_rx_bps,
                    udp_tx_bps: r.udp_tx_bps,
                    metadata_rx_bps: r.metadata_rx_bps,
                    total_rss_bytes: r.total_rss_bytes,
                })
                .collect();

            if points.is_empty() {
                points = state.metrics.recent_ring_minute_fallback().await;
            }
            Ok(Json(OverviewHistoryResponse {
                range,
                granularity: "minute",
                points,
            }))
        }
        "7d" | "30d" => {
            let (secs, limit) = if range == "7d" {
                (7 * 86_400_i64, 200_i64)
            } else {
                (30 * 86_400_i64, 800_i64)
            };
            let rows = state.db.query_metrics_hour(now - secs, limit).await?;
            let points = rows
                .into_iter()
                .map(|r| HistoryPointDto {
                    ts: r.ts_hour,
                    hashes_per_min: r.hashes_total as f64 / 60.0,
                    metadata_succeeded_per_min: r.metadata_succeeded_total as f64 / 60.0,
                    stream_pushed_per_min: r.stream_pushed_total as f64 / 60.0,
                    filtered_per_min: r.filtered_total as f64 / 60.0,
                    indexed_written_per_min: r.indexed_written_total as f64 / 60.0,
                    hot_updated_per_min: r.hot_updated_total as f64 / 60.0,
                    actual_qps: r.avg_qps,
                    stream_new_len: 0,
                    stream_hot_len: 0,
                    stream_new_lag: r.max_stream_new_lag,
                    udp_rx_bps: 0.0,
                    udp_tx_bps: 0.0,
                    metadata_rx_bps: 0.0,
                    total_rss_bytes: r.avg_rss_bytes,
                })
                .collect();
            Ok(Json(OverviewHistoryResponse {
                range,
                granularity: "hour",
                points,
            }))
        }
        _ => Err(ApiError::bad_request(
            "INVALID_RANGE",
            "不支持的时间窗口（仅支持 1h / 6h / 24h / 7d / 30d）",
        )),
    }
}

pub async fn overview_stream_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let initial = state.metrics.current_snapshot().await;
    let init_data = serde_json::to_string(&initial).unwrap_or_else(|_| "{}".to_string());
    let first = stream::once(async move {
        Ok::<Event, Infallible>(Event::default().event("snapshot").data(init_data))
    });

    let rx = state.metrics.subscribe();
    let tail = stream::unfold((rx, Instant::now()), |(mut rx, mut last_sent)| async move {
        loop {
            match rx.recv().await {
                Ok(snap) => {
                    if last_sent.elapsed() < Duration::from_millis(1800) {
                        continue;
                    }
                    last_sent = Instant::now();
                    let json = serde_json::to_string(&snap).unwrap_or_else(|_| "{}".to_string());
                    let ev = Event::default().event("snapshot").data(json);
                    return Some((Ok::<Event, Infallible>(ev), (rx, last_sent)));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    let merged = futures::StreamExt::chain(first, tail);
    Sse::new(merged).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
