use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use hameta_common::{
    WorkerDrainStatus, WorkerReadyRequest, WorkerRegisterRequest, WorkerRole,
    WorkerShutdownRequest, WorkerSnapshotEnvelope, verify_bearer_token,
};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

pub type SnapshotProvider = Arc<dyn Fn() -> serde_json::Value + Send + Sync>;

#[derive(Clone)]
pub struct CrawlerIpcState {
    pub instance_id: String,
    pub ipc_token: String,
    pub draining: Arc<AtomicBool>,
    pub drain_completed: Arc<AtomicBool>,
    pub graceful_timeout_secs: Arc<AtomicU64>,
    pub queued_items: Arc<dyn Fn() -> usize + Send + Sync>,
    pub in_flight_items: Arc<dyn Fn() -> usize + Send + Sync>,
    pub snapshot_provider: SnapshotProvider,
    pub shutdown_token: CancellationToken,
}

pub fn build_crawler_ipc_router(state: Arc<CrawlerIpcState>) -> Router {
    Router::new()
        .route("/internal/v1/snapshot", get(snapshot_handler))
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
    State(state): State<Arc<CrawlerIpcState>>,
    headers: HeaderMap,
) -> Response {
    if !is_authorized(&headers, &state.ipc_token) {
        return unauthorized_response();
    }
    let payload = (state.snapshot_provider)();
    let env = WorkerSnapshotEnvelope {
        instance_id: state.instance_id.clone(),
        role: WorkerRole::Crawler,
        captured_at_unix: u64::try_from(Utc::now().timestamp()).unwrap_or_default(),
        payload,
    };
    (StatusCode::OK, Json(env)).into_response()
}

async fn drain_status_handler(
    State(state): State<Arc<CrawlerIpcState>>,
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
    State(state): State<Arc<CrawlerIpcState>>,
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
    state: Arc<CrawlerIpcState>,
    ipc_shutdown: CancellationToken,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("Crawler 绑定 Loopback IPC 端口失败")?;
    let addr = listener
        .local_addr()
        .context("读取 Crawler IPC 监听地址失败")?;
    let app = build_crawler_ipc_router(state);
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(async move { ipc_shutdown.cancelled().await })
            .await
        {
            tracing::warn!("Crawler IPC 服务异常退出: {err}");
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
        .timeout(std::time::Duration::from_secs(5))
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
        .context("向 Panel 上报 Worker 注册信息失败")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "Panel 拒绝 Worker 注册: status={}",
        resp.status()
    );
    Ok(())
}

pub async fn notify_panel_ready(
    panel_ipc_port: u16,
    ipc_token: &str,
    instance_id: &str,
) -> Result<()> {
    if panel_ipc_port == 0 || ipc_token.is_empty() {
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .context("构建 IPC HTTP 客户端失败")?;
    let url = format!("http://127.0.0.1:{panel_ipc_port}/internal/v1/ready");
    let req = WorkerReadyRequest {
        instance_id: instance_id.to_string(),
        rule_version: None,
    };
    let resp = client
        .post(&url)
        .bearer_auth(ipc_token)
        .json(&req)
        .send()
        .await
        .context("向 Panel 上报 Worker 就绪状态失败")?;
    anyhow::ensure!(
        resp.status().is_success(),
        "Panel 确认 Worker 就绪失败: status={}",
        resp.status()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn crawler_ipc_auth_snapshot_and_shutdown_flow() {
        let token = "test-ipc-token-123".to_string();
        let shutdown = CancellationToken::new();
        let state = Arc::new(CrawlerIpcState {
            instance_id: "crawler-1".to_string(),
            ipc_token: token.clone(),
            draining: Arc::new(AtomicBool::new(false)),
            drain_completed: Arc::new(AtomicBool::new(false)),
            graceful_timeout_secs: Arc::new(AtomicU64::new(15)),
            queued_items: Arc::new(|| 4),
            in_flight_items: Arc::new(|| 2),
            snapshot_provider: Arc::new(|| json!({ "uptime_secs": 10 })),
            shutdown_token: shutdown.clone(),
        });
        let app = build_crawler_ipc_router(state.clone());

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

        let snap = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/internal/v1/snapshot")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(snap.status(), StatusCode::OK);
        let body = to_bytes(snap.into_body(), usize::MAX).await.unwrap();
        let env: WorkerSnapshotEnvelope = serde_json::from_slice(&body).unwrap();
        assert_eq!(env.instance_id, "crawler-1");
        assert_eq!(env.payload["uptime_secs"], 10);

        let shut = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/v1/shutdown")
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"graceful_timeout_secs":9}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(shut.status(), StatusCode::OK);
        assert!(shutdown.is_cancelled());
        assert_eq!(state.graceful_timeout_secs.load(Ordering::Relaxed), 9);

        let drain = app
            .oneshot(
                Request::builder()
                    .uri("/internal/v1/drain-status")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let d_body = to_bytes(drain.into_body(), usize::MAX).await.unwrap();
        let d_status: WorkerDrainStatus = serde_json::from_slice(&d_body).unwrap();
        assert!(d_status.draining);
        assert_eq!(d_status.queued_items, 4);
        assert_eq!(d_status.in_flight_items, 2);
        assert!(!d_status.completed);
    }
}
