use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use hameta_common::{
    IngestEventItem, WorkerDrainStatus, WorkerIngestEventsRequest, WorkerReadyRequest,
    WorkerRegisterRequest, WorkerReloadRulesRequest, WorkerReloadRulesResponse, WorkerRole,
    WorkerShutdownRequest, WorkerSnapshotEnvelope, verify_bearer_token,
};
use hameta_filter::{FilterBundle, FilterRuntime};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// 非阻塞批量入库事件上报器：将 Manticore 写库成功记录通过有界通道异步推送给 Panel SSE Hub。
#[derive(Clone, Default)]
pub struct IngestEventReporter {
    tx: Option<mpsc::Sender<IngestEventItem>>,
}

impl IngestEventReporter {
    pub fn disabled() -> Self {
        Self { tx: None }
    }

    pub fn try_emit(&self, item: IngestEventItem) {
        if let Some(ref tx) = self.tx {
            let _ = tx.try_send(item);
        }
    }

    pub fn spawn(
        panel_ipc_port: u16,
        ipc_token: &str,
        instance_id: &str,
        shutdown: CancellationToken,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        Self::spawn_to_url(
            if panel_ipc_port == 0 || ipc_token.is_empty() {
                None
            } else {
                Some(format!(
                    "http://127.0.0.1:{panel_ipc_port}/internal/v1/ingest-events"
                ))
            },
            ipc_token,
            instance_id,
            shutdown,
        )
    }

    fn spawn_to_url(
        url_opt: Option<String>,
        ipc_token: &str,
        instance_id: &str,
        shutdown: CancellationToken,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let Some(url) = url_opt else {
            return (Self::disabled(), tokio::spawn(async {}));
        };
        let token = ipc_token.to_string();
        let inst_id = instance_id.to_string();
        let (tx, mut rx) = mpsc::channel::<IngestEventItem>(2_048);

        let handle = tokio::spawn(async move {
            let Ok(client) = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .connect_timeout(Duration::from_secs(1))
                .build()
            else {
                tracing::warn!("初始化 IngestEventReporter HTTP 客户端失败");
                return;
            };

            let mut buffer = Vec::<IngestEventItem>::with_capacity(64);
            let mut flush_tick = tokio::time::interval(Duration::from_millis(200));
            flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        while let Ok(item) = rx.try_recv() {
                            buffer.push(item);
                            if buffer.len() >= 64 {
                                post_ingest_batch(&client, &url, &token, &inst_id, &mut buffer).await;
                            }
                        }
                        if !buffer.is_empty() {
                            post_ingest_batch(&client, &url, &token, &inst_id, &mut buffer).await;
                        }
                        break;
                    }
                    maybe_item = rx.recv() => {
                        match maybe_item {
                            Some(item) => {
                                buffer.push(item);
                                while buffer.len() < 64 {
                                    match rx.try_recv() {
                                        Ok(more) => buffer.push(more),
                                        Err(_) => break,
                                    }
                                }
                                if buffer.len() >= 64 {
                                    post_ingest_batch(&client, &url, &token, &inst_id, &mut buffer).await;
                                }
                            }
                            None => {
                                if !buffer.is_empty() {
                                    post_ingest_batch(&client, &url, &token, &inst_id, &mut buffer).await;
                                }
                                break;
                            }
                        }
                    }
                    _ = flush_tick.tick() => {
                        if !buffer.is_empty() {
                            post_ingest_batch(&client, &url, &token, &inst_id, &mut buffer).await;
                        }
                    }
                }
            }
        });

        (Self { tx: Some(tx) }, handle)
    }
}

async fn post_ingest_batch(
    client: &reqwest::Client,
    url: &str,
    ipc_token: &str,
    instance_id: &str,
    buffer: &mut Vec<IngestEventItem>,
) {
    if buffer.is_empty() {
        return;
    }
    let events = std::mem::take(buffer);
    let req = WorkerIngestEventsRequest {
        instance_id: instance_id.to_string(),
        events,
    };
    if let Err(err) = client.post(url).bearer_auth(ipc_token).json(&req).send().await {
        tracing::debug!("上报入库事件批次到 Panel 失败: {err}");
    }
}

pub type SnapshotProvider = Arc<dyn Fn() -> serde_json::Value + Send + Sync>;

#[derive(Clone)]
pub struct IndexerIpcState {
    pub instance_id: String,
    pub ipc_token: String,
    pub filter_runtime: Arc<FilterRuntime>,
    pub draining: Arc<AtomicBool>,
    pub drain_completed: Arc<AtomicBool>,
    pub graceful_timeout_secs: Arc<AtomicU64>,
    pub queued_items: Arc<dyn Fn() -> usize + Send + Sync>,
    pub in_flight_items: Arc<dyn Fn() -> usize + Send + Sync>,
    pub snapshot_provider: SnapshotProvider,
    pub shutdown_token: CancellationToken,
}

pub fn build_indexer_ipc_router(state: Arc<IndexerIpcState>) -> Router {
    Router::new()
        .route("/internal/v1/snapshot", get(snapshot_handler))
        .route("/internal/v1/rules/reload", post(reload_rules_handler))
        .route("/internal/v1/drain-status", get(drain_status_handler))
        .route("/internal/v1/shutdown", post(shutdown_handler))
        .with_state(state)
}

fn is_authorized(headers: &HeaderMap, expected: &str) -> bool {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    verify_bearer_token(auth, expected)
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "code": "UNAUTHORIZED", "message": "无效的本地 IPC 凭据" })),
    )
        .into_response()
}

async fn snapshot_handler(
    State(state): State<Arc<IndexerIpcState>>,
    headers: HeaderMap,
) -> Response {
    if !is_authorized(&headers, &state.ipc_token) {
        return unauthorized_response();
    }
    let payload = (state.snapshot_provider)();
    let env = WorkerSnapshotEnvelope {
        instance_id: state.instance_id.clone(),
        role: WorkerRole::Indexer,
        captured_at_unix: u64::try_from(Utc::now().timestamp()).unwrap_or_default(),
        payload,
    };
    (StatusCode::OK, Json(env)).into_response()
}

async fn reload_rules_handler(
    State(state): State<Arc<IndexerIpcState>>,
    headers: HeaderMap,
    Json(req): Json<WorkerReloadRulesRequest>,
) -> Response {
    if !is_authorized(&headers, &state.ipc_token) {
        return unauthorized_response();
    }

    let mut bundle: FilterBundle = match serde_json::from_value(req.filter_bundle) {
        Ok(b) => b,
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "code": "INVALID_FILTER_BUNDLE",
                    "message": format!("解析规则包 JSON 失败: {err}")
                })),
            )
                .into_response();
        }
    };
    bundle.version = req.version;

    match state.filter_runtime.reload(&bundle) {
        Ok(active_rule_version) => {
            tracing::info!("Indexer 已通过 ArcSwap 热切换过滤规则版本: v{active_rule_version}");
            (
                StatusCode::OK,
                Json(WorkerReloadRulesResponse {
                    active_rule_version,
                }),
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "code": "FILTER_COMPILE_ERROR",
                "message": format!("过滤规则预编译校验失败，线上版本保持不变: {err}")
            })),
        )
            .into_response(),
    }
}

async fn drain_status_handler(
    State(state): State<Arc<IndexerIpcState>>,
    headers: HeaderMap,
) -> Response {
    if !is_authorized(&headers, &state.ipc_token) {
        return unauthorized_response();
    }
    let status = WorkerDrainStatus {
        draining: state.draining.load(Ordering::Relaxed),
        queued_items: (state.queued_items)(),
        in_flight_items: (state.in_flight_items)(),
        completed: state.drain_completed.load(Ordering::Relaxed),
    };
    (StatusCode::OK, Json(status)).into_response()
}

async fn shutdown_handler(
    State(state): State<Arc<IndexerIpcState>>,
    headers: HeaderMap,
    Json(req): Json<WorkerShutdownRequest>,
) -> Response {
    if !is_authorized(&headers, &state.ipc_token) {
        return unauthorized_response();
    }
    state
        .graceful_timeout_secs
        .store(req.graceful_timeout_secs.clamp(1, 300), Ordering::Relaxed);
    state.draining.store(true, Ordering::Relaxed);
    state.shutdown_token.cancel();
    (
        StatusCode::OK,
        Json(json!({ "accepted": true, "graceful_timeout_secs": req.graceful_timeout_secs })),
    )
        .into_response()
}

pub async fn start_ipc_server(
    state: Arc<IndexerIpcState>,
    ipc_shutdown: CancellationToken,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("Indexer 绑定 Loopback IPC 端口失败")?;
    let addr = listener
        .local_addr()
        .context("读取 Indexer IPC 监听地址失败")?;
    let app = build_indexer_ipc_router(state);
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(async move { ipc_shutdown.cancelled().await })
            .await
        {
            tracing::warn!("Indexer IPC 服务异常退出: {err}");
        }
    });
    Ok(addr)
}

pub async fn notify_panel_register(
    panel_ipc_port: u16,
    ipc_token: &str,
    instance_id: &str,
    worker_port: u16,
) -> Result<()> {
    if panel_ipc_port == 0 || ipc_token.is_empty() {
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("构建 IPC HTTP 客户端失败")?;
    let url = format!("http://127.0.0.1:{panel_ipc_port}/internal/v1/register");
    let req = WorkerRegisterRequest {
        instance_id: instance_id.to_string(),
        worker_port,
        pid: std::process::id(),
        ipc_token: ipc_token.to_string(),
    };
    let resp = client
        .post(&url)
        .bearer_auth(ipc_token)
        .json(&req)
        .send()
        .await
        .context("向 Panel 上报 Indexer 注册信息失败")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "Panel 拒绝 Indexer 注册: status={}",
        resp.status()
    );
    Ok(())
}

pub async fn notify_panel_ready(
    panel_ipc_port: u16,
    ipc_token: &str,
    instance_id: &str,
    rule_version: i64,
) -> Result<()> {
    if panel_ipc_port == 0 || ipc_token.is_empty() {
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("构建 IPC HTTP 客户端失败")?;
    let url = format!("http://127.0.0.1:{panel_ipc_port}/internal/v1/ready");
    let req = WorkerReadyRequest {
        instance_id: instance_id.to_string(),
        rule_version: Some(rule_version),
    };
    let resp = client
        .post(&url)
        .bearer_auth(ipc_token)
        .json(&req)
        .send()
        .await
        .context("向 Panel 上报 Indexer 就绪状态失败")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "Panel 确认 Indexer 就绪失败: status={}",
        resp.status()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use hameta_filter::default_online_filter_bundle;
    use tower::ServiceExt;

    #[tokio::test]
    async fn indexer_ipc_hot_reloads_rules_atomically_and_rejects_bad_regex() {
        let mut initial = default_online_filter_bundle();
        initial.version = 1;
        let runtime = Arc::new(FilterRuntime::new(&initial).unwrap());
        let shutdown = CancellationToken::new();
        let token = "indexer-token-xyz".to_string();

        let state = Arc::new(IndexerIpcState {
            instance_id: "indexer-1".to_string(),
            ipc_token: token.clone(),
            filter_runtime: runtime.clone(),
            draining: Arc::new(AtomicBool::new(false)),
            drain_completed: Arc::new(AtomicBool::new(false)),
            graceful_timeout_secs: Arc::new(AtomicU64::new(15)),
            queued_items: Arc::new(|| 0),
            in_flight_items: Arc::new(|| 0),
            snapshot_provider: {
                let r = runtime.clone();
                Arc::new(move || json!({ "active_rule_version": r.active_version() }))
            },
            shutdown_token: shutdown.clone(),
        });
        let app = build_indexer_ipc_router(state);

        // 未授权拒绝
        let unauth = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/internal/v1/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

        // 非法正则热更新返回 400 且原版本 v1 完好
        let mut invalid_bundle = default_online_filter_bundle();
        invalid_bundle.policy.rules.push(hameta_filter::PolicyRule {
            id: "bad-regex".to_string(),
            name: "非法正则测试".to_string(),
            enabled: true,
            action: hameta_filter::PolicyAction::Reject,
            tags: vec![],
            note: String::new(),
            matcher: hameta_filter::RuleMatcher::CustomRegex {
                target: hameta_filter::TextTarget::RawName,
                match_all: false,
                patterns: vec!["([unclosed".to_string()],
            },
        });
        let bad_req = WorkerReloadRulesRequest {
            version: 2,
            filter_bundle: serde_json::to_value(&invalid_bundle).unwrap(),
        };
        let bad_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/v1/rules/reload")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&bad_req).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad_resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(runtime.active_version(), 1);

        // 合法热更新返回 200 并切换到 v2
        let valid_bundle = default_online_filter_bundle();
        let ok_req = WorkerReloadRulesRequest {
            version: 2,
            filter_bundle: serde_json::to_value(&valid_bundle).unwrap(),
        };
        let ok_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/v1/rules/reload")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&ok_req).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok_resp.status(), StatusCode::OK);
        let body = to_bytes(ok_resp.into_body(), usize::MAX).await.unwrap();
        let parsed: WorkerReloadRulesResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.active_rule_version, 2);
        assert_eq!(runtime.active_version(), 2);
    }

    #[tokio::test]
    async fn ingest_event_reporter_batches_and_authenticates() {
        use axum::routing::post;
        use std::sync::Mutex;

        let received: Arc<Mutex<Vec<WorkerIngestEventsRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let rec_clone = received.clone();
        let expected_token = "indexer-ingest-token".to_string();
        let tok_clone = expected_token.clone();

        let mock_app = Router::new().route(
            "/internal/v1/ingest-events",
            post(
                move |headers: HeaderMap, Json(req): Json<WorkerIngestEventsRequest>| {
                    let rec = rec_clone.clone();
                    let expected = tok_clone.clone();
                    async move {
                        let auth = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|v| v.to_str().ok());
                        if !verify_bearer_token(auth, &expected) {
                            return StatusCode::UNAUTHORIZED;
                        }
                        rec.lock().unwrap().push(req);
                        StatusCode::OK
                    }
                },
            ),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_shutdown = CancellationToken::new();
        let srv_cancel = server_shutdown.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, mock_app)
                .with_graceful_shutdown(async move { srv_cancel.cancelled().await })
                .await;
        });

        let reporter_shutdown = CancellationToken::new();
        let (reporter, handle) = IngestEventReporter::spawn(
            port,
            &expected_token,
            "indexer-1",
            reporter_shutdown.clone(),
        );
        reporter.try_emit(IngestEventItem {
            info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
            event_type: hameta_common::IngestEventType::Indexed,
            worker_id: "indexer-1".to_string(),
            manticore_id: 12345,
            batch_id: 1,
            raw_name: "raw".to_string(),
            refine_name: "clean".to_string(),
            cleaned: true,
            total_size: 1024,
            file_count: 1,
            files_preview: Vec::new(),
            hot: 1,
            hot_delta: 1,
            latency_ms: 8,
            updated_at: 1_763_800_000,
        });

        reporter_shutdown.cancel();
        handle.await.unwrap();
        server_shutdown.cancel();

        let guard = received.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard[0].instance_id, "indexer-1");
        assert_eq!(guard[0].events.len(), 1);
        assert_eq!(guard[0].events[0].manticore_id, 12345);
    }
}
