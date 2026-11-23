use crate::api::api_v1_router;
use crate::assets::serve_spa;
use crate::auth::LoginRateLimiter;
use crate::metrics_collector::MetricsCollector;
use crate::services::ServiceRuntimePool;
use crate::supervisor::Supervisor;
use crate::torrent_tasks::TorrentTaskManager;
use crate::tuning_advisor::TuningAdvisor;
use anyhow::Context;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use hameta_storage::Database;
use serde::Serialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

#[derive(Debug, Clone)]
pub struct PanelConfig {
    pub bind_addr: String,
    pub data_dir: PathBuf,
    pub dev_mode: bool,
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:8080".to_string(),
            data_dir: PathBuf::from("./data"),
            dev_mode: false,
        }
    }
}

#[derive(Clone)]
pub struct PanelState {
    pub db: Database,
    pub config: PanelConfig,
    pub supervisor: Arc<Supervisor>,
    pub services: Arc<ServiceRuntimePool>,
    pub metrics: Arc<MetricsCollector>,
    pub torrent_tasks: Arc<TorrentTaskManager>,
    pub tuning_advisor: Arc<TuningAdvisor>,
    pub login_limiter: Arc<LoginRateLimiter>,
    pub setup_lock: Arc<Mutex<()>>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    initialized: bool,
}

pub fn build_router(state: Arc<PanelState>) -> Router {
    Router::new()
        .route("/healthz", get(health_handler))
        .nest("/api/v1", api_v1_router())
        .fallback(serve_spa)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health_handler(State(state): State<Arc<PanelState>>) -> Response {
    match state.db.is_initialized().await {
        Ok(initialized) => (
            StatusCode::OK,
            Json(HealthResponse {
                status: "ok",
                version: env!("CARGO_PKG_VERSION"),
                initialized,
            }),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("healthz 检查数据库状态失败: {err}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "degraded",
                    version: env!("CARGO_PKG_VERSION"),
                    initialized: false,
                }),
            )
                .into_response()
        }
    }
}

async fn wait_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!("监听停机信号失败: {err}");
        }
    }
}

pub async fn run_panel(config: PanelConfig) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("创建数据目录失败: {}", config.data_dir.display()))?;
    let db_path = config.data_dir.join("hameta.db");
    let db = Database::open(&db_path)
        .await
        .with_context(|| format!("打开 SQLite 数据库失败: {}", db_path.display()))?;

    crate::nofile::apply_saved_nofile_on_startup(&db).await;

    let services = Arc::new(ServiceRuntimePool::default());
    services.init_from_db(&db).await;

    let supervisor = Supervisor::new(db.clone(), config.data_dir.clone()).await?;
    supervisor.spawn_snapshot_poller();
    if let Err(err) = supervisor.reconcile_desired_workers().await {
        tracing::warn!("面板启动恢复期望运行中 Worker 失败: {err:#}");
    }

    let metrics = MetricsCollector::new(db.clone(), supervisor.clone(), services.clone());
    let collector_cancel = tokio_util::sync::CancellationToken::new();
    metrics.spawn_background_loop(collector_cancel.clone());

    let torrent_tasks = TorrentTaskManager::new(db.clone(), services.clone());
    torrent_tasks.recover_stale_tasks_on_startup().await;

    let tuning_advisor = TuningAdvisor::new(db.clone(), supervisor.clone());
    tuning_advisor.spawn_background_loop(collector_cancel.clone());

    let addr: SocketAddr = config
        .bind_addr
        .parse()
        .with_context(|| format!("无效的面板监听地址: {}", config.bind_addr))?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("绑定监听地址失败: {addr}"))?;
    let local_addr = listener.local_addr().context("读取面板监听地址失败")?;

    let state = Arc::new(PanelState {
        db,
        config,
        supervisor: supervisor.clone(),
        services,
        metrics,
        torrent_tasks: torrent_tasks.clone(),
        tuning_advisor,
        login_limiter: Arc::new(LoginRateLimiter::default()),
        setup_lock: Arc::new(Mutex::new(())),
    });
    let app = build_router(state);

    tracing::info!("hameta panel 已启动: http://{local_addr}/");

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            wait_shutdown_signal().await;
            tracing::info!("收到退出信号，hameta panel 正在优雅停机并通知所有 Worker 排空...");
        })
        .await
        .context("Web 服务运行异常退出");

    collector_cancel.cancel();
    torrent_tasks.pause_active_on_shutdown().await;
    supervisor.shutdown_all().await;
    serve_result
}
