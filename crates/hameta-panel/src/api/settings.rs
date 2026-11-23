use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::discovery::{ServiceDiscoveryReport, detect_services};
use crate::server::PanelState;
use crate::services::{
    InitTableError, ManticoreTestReport, RedisTestReport, init_manticore_table_if_absent,
    sanitize_secret_in_text, test_manticore_connection, test_redis_connection,
};
use crate::supervisor::{
    CrawlerGeoIpReloadResult, META_GEOIP_CONFIG_KEY, Supervisor, load_geoip_config,
};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use hameta_common::{
    GeoIpConfig, ManticoreAuthMode, ManticoreServiceConfig, RedisServiceConfig, ServiceKind,
    WorkerRole, WorkerRuntimeState,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize)]
pub struct AffectedWorkerItem {
    pub id: String,
    pub display_name: String,
    pub role: WorkerRole,
    pub runtime_state: WorkerRuntimeState,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkerRestartOutcome {
    pub id: String,
    pub role: WorkerRole,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceConfigDetail<T> {
    pub version: i64,
    pub config: T,
    pub last_test_ok: Option<bool>,
    pub last_test_message: Option<String>,
    pub last_tested_at: Option<i64>,
    pub updated_by: String,
    pub note: String,
    pub updated_at: i64,
    pub affected_running_workers: Vec<AffectedWorkerItem>,
}

#[derive(Debug, Serialize)]
pub struct ServicesOverviewResponse {
    pub redis: ServiceConfigDetail<RedisServiceConfig>,
    pub manticore: ServiceConfigDetail<ManticoreServiceConfig>,
}

#[derive(Debug, Deserialize)]
pub struct TestRedisRequest {
    pub config: Option<RedisServiceConfig>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRedisRequest {
    pub config: RedisServiceConfig,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub restart_affected_workers: bool,
}

#[derive(Debug, Deserialize)]
pub struct TestManticoreRequest {
    pub config: Option<ManticoreServiceConfig>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateManticoreRequest {
    pub config: ManticoreServiceConfig,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub restart_affected_workers: bool,
}

#[derive(Debug, Serialize)]
pub struct UpdateServiceResponse<T> {
    pub version: i64,
    pub config: T,
    pub restarted_workers: Vec<WorkerRestartOutcome>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub kind: String,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ServiceHistoryItem {
    pub id: i64,
    pub kind: String,
    pub version: i64,
    pub config: serde_json::Value,
    pub is_active: bool,
    pub last_test_ok: Option<bool>,
    pub last_test_message: Option<String>,
    pub last_tested_at: Option<i64>,
    pub created_by: String,
    pub note: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct RollbackServiceRequest {
    pub kind: String,
    pub target_version: i64,
    #[serde(default)]
    pub restart_affected_workers: bool,
}

#[derive(Debug, Serialize)]
pub struct RollbackServiceResponse {
    pub kind: String,
    pub version: i64,
    pub config: serde_json::Value,
    pub restarted_workers: Vec<WorkerRestartOutcome>,
}

fn is_active_runtime_state(state: WorkerRuntimeState) -> bool {
    matches!(
        state,
        WorkerRuntimeState::Starting | WorkerRuntimeState::Running | WorkerRuntimeState::Draining
    )
}

async fn affected_running_crawlers(
    supervisor: &Supervisor,
) -> Result<Vec<AffectedWorkerItem>, ApiError> {
    let statuses = supervisor
        .list_worker_statuses()
        .await
        .map_err(|e| ApiError::internal("读取 Worker 运行状态失败").with_detail(e.to_string()))?;
    Ok(statuses
        .into_iter()
        .filter(|w| is_active_runtime_state(w.runtime_state) && w.role == WorkerRole::Crawler)
        .map(|w| AffectedWorkerItem {
            id: w.id,
            display_name: w.display_name,
            role: w.role,
            runtime_state: w.runtime_state,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct GeoIpInspectSummary {
    pub resolved_path: String,
    pub mmdb_exists: bool,
    pub mmdb_valid: bool,
    pub mmdb_size_bytes: u64,
    pub mmdb_build_epoch: Option<u64>,
    pub database_type: Option<String>,
    pub node_count: Option<u32>,
    pub status_message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GeoIpSettingsResponse {
    pub config: GeoIpConfig,
    pub resolved_path: String,
    pub mmdb_exists: bool,
    pub mmdb_valid: bool,
    pub mmdb_size_bytes: u64,
    pub mmdb_build_epoch: Option<u64>,
    pub database_type: Option<String>,
    pub node_count: Option<u32>,
    pub status_message: String,
    pub running_crawlers: Vec<AffectedWorkerItem>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct UpdateGeoIpRequest {
    pub config: GeoIpConfig,
    #[serde(default = "default_true")]
    pub reload_crawlers: bool,
}

#[derive(Debug, Serialize)]
pub struct UpdateGeoIpResponse {
    pub config: GeoIpConfig,
    pub inspect: GeoIpInspectSummary,
    pub reload_results: Vec<CrawlerGeoIpReloadResult>,
}

#[derive(Debug, Serialize)]
pub struct ReloadGeoIpResponse {
    pub config: GeoIpConfig,
    pub inspect: GeoIpInspectSummary,
    pub reload_results: Vec<CrawlerGeoIpReloadResult>,
}

static GEOIP_DOWNLOAD_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
const MAX_GEOIP_MMDB_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DownloadGeoIpRequest {
    pub mmdb_path: Option<String>,
    pub download_url: Option<String>,
    #[serde(default = "default_true")]
    pub reload_crawlers: bool,
}

#[derive(Debug, Serialize)]
pub struct DownloadGeoIpResponse {
    pub config: GeoIpConfig,
    pub inspect: GeoIpInspectSummary,
    pub reload_results: Vec<CrawlerGeoIpReloadResult>,
    pub downloaded_bytes: u64,
    pub duration_ms: u64,
    pub source_url: String,
    pub release_tag: Option<String>,
    pub previous_build_epoch: Option<u64>,
    pub changed: bool,
    pub message: String,
}

async fn inspect_geoip_file(cfg: &GeoIpConfig, data_dir: &std::path::Path) -> GeoIpInspectSummary {
    let resolved = cfg.resolve_mmdb_path(data_dir);
    let resolved_path = resolved.to_string_lossy().into_owned();
    let fallback_path = resolved_path.clone();
    tokio::task::spawn_blocking(move || {
        let meta = match std::fs::metadata(&resolved) {
            Ok(m) => m,
            Err(err) => {
                return GeoIpInspectSummary {
                    resolved_path,
                    mmdb_exists: false,
                    mmdb_valid: false,
                    mmdb_size_bytes: 0,
                    mmdb_build_epoch: None,
                    database_type: None,
                    node_count: None,
                    status_message: format!("未找到 MMDB 文件: {err}"),
                };
            }
        };
        let size_bytes = meta.len();
        // SAFETY: 仅临时只读 mmap 打开 MMDB 以读取头部元数据并随即释放句柄。
        match unsafe { maxminddb::Reader::open_mmap(&resolved) } {
            Ok(reader) => {
                let md = reader.metadata();
                GeoIpInspectSummary {
                    resolved_path,
                    mmdb_exists: true,
                    mmdb_valid: true,
                    mmdb_size_bytes: size_bytes,
                    mmdb_build_epoch: Some(md.build_epoch),
                    database_type: Some(md.database_type.clone()),
                    node_count: Some(md.node_count),
                    status_message: format!("MMDB 校验通过 ({})", md.database_type),
                }
            }
            Err(err) => GeoIpInspectSummary {
                resolved_path,
                mmdb_exists: true,
                mmdb_valid: false,
                mmdb_size_bytes: size_bytes,
                mmdb_build_epoch: None,
                database_type: None,
                node_count: None,
                status_message: format!("MMDB 文件损坏或格式不兼容: {err}"),
            },
        }
    })
    .await
    .unwrap_or_else(|err| GeoIpInspectSummary {
        resolved_path: fallback_path,
        mmdb_exists: false,
        mmdb_valid: false,
        mmdb_size_bytes: 0,
        mmdb_build_epoch: None,
        database_type: None,
        node_count: None,
        status_message: format!("检查 MMDB 文件任务异常: {err}"),
    })
}

async fn affected_running_workers(
    supervisor: &Supervisor,
    kind: ServiceKind,
) -> Result<Vec<AffectedWorkerItem>, ApiError> {
    let statuses = supervisor
        .list_worker_statuses()
        .await
        .map_err(|e| ApiError::internal("读取 Worker 运行状态失败").with_detail(e.to_string()))?;
    Ok(statuses
        .into_iter()
        .filter(|w| is_active_runtime_state(w.runtime_state))
        .filter(|w| match kind {
            ServiceKind::Redis => matches!(w.role, WorkerRole::Crawler | WorkerRole::Indexer),
            ServiceKind::Manticore => w.role == WorkerRole::Indexer,
        })
        .map(|w| AffectedWorkerItem {
            id: w.id,
            display_name: w.display_name,
            role: w.role,
            runtime_state: w.runtime_state,
        })
        .collect())
}

async fn restart_workers_sequentially(
    supervisor: &Arc<Supervisor>,
    targets: Vec<AffectedWorkerItem>,
) -> Vec<WorkerRestartOutcome> {
    let mut outcomes = Vec::with_capacity(targets.len());
    for target in targets {
        match supervisor.restart_worker(&target.id, 15).await {
            Ok(()) => outcomes.push(WorkerRestartOutcome {
                id: target.id,
                role: target.role,
                ok: true,
                message: "已完成优雅排空并以新配置重启".to_string(),
            }),
            Err(err) => outcomes.push(WorkerRestartOutcome {
                id: target.id,
                role: target.role,
                ok: false,
                message: format!("滚动重启失败: {err}"),
            }),
        }
    }
    outcomes
}

async fn load_redis_detail(
    state: &PanelState,
) -> Result<ServiceConfigDetail<RedisServiceConfig>, ApiError> {
    let affected = affected_running_workers(&state.supervisor, ServiceKind::Redis).await?;
    if let Some(row) = state
        .db
        .get_active_service_config(ServiceKind::Redis)
        .await?
    {
        let cfg: RedisServiceConfig = serde_json::from_str(&row.config_json).map_err(|e| {
            ApiError::internal("解析 Redis 配置 JSON 失败").with_detail(e.to_string())
        })?;
        return Ok(ServiceConfigDetail {
            version: row.version,
            config: cfg,
            last_test_ok: row.last_test_ok.map(|v| v != 0),
            last_test_message: row.last_test_message,
            last_tested_at: row.last_tested_at,
            updated_by: row.created_by,
            note: row.note,
            updated_at: row.created_at,
            affected_running_workers: affected,
        });
    }
    Ok(ServiceConfigDetail {
        version: 0,
        config: RedisServiceConfig::default(),
        last_test_ok: None,
        last_test_message: None,
        last_tested_at: None,
        updated_by: String::new(),
        note: String::new(),
        updated_at: 0,
        affected_running_workers: affected,
    })
}

async fn load_manticore_detail(
    state: &PanelState,
) -> Result<ServiceConfigDetail<ManticoreServiceConfig>, ApiError> {
    let affected = affected_running_workers(&state.supervisor, ServiceKind::Manticore).await?;
    if let Some(row) = state
        .db
        .get_active_service_config(ServiceKind::Manticore)
        .await?
    {
        let cfg: ManticoreServiceConfig =
            serde_json::from_str::<ManticoreServiceConfig>(&row.config_json)
                .map_err(|e| {
                    ApiError::internal("解析 Manticore 配置 JSON 失败").with_detail(e.to_string())
                })?
                .normalized();
        return Ok(ServiceConfigDetail {
            version: row.version,
            config: cfg,
            last_test_ok: row.last_test_ok.map(|v| v != 0),
            last_test_message: row.last_test_message,
            last_tested_at: row.last_tested_at,
            updated_by: row.created_by,
            note: row.note,
            updated_at: row.created_at,
            affected_running_workers: affected,
        });
    }
    Ok(ServiceConfigDetail {
        version: 0,
        config: ManticoreServiceConfig::default(),
        last_test_ok: None,
        last_test_message: None,
        last_tested_at: None,
        updated_by: String::new(),
        note: String::new(),
        updated_at: 0,
        affected_running_workers: affected,
    })
}

pub async fn detect_services_handler(
    _auth: AuthUser,
) -> Result<Json<ServiceDiscoveryReport>, ApiError> {
    Ok(Json(detect_services().await))
}

pub async fn get_services_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Result<Json<ServicesOverviewResponse>, ApiError> {
    let redis = load_redis_detail(&state).await?;
    let manticore = load_manticore_detail(&state).await?;
    Ok(Json(ServicesOverviewResponse { redis, manticore }))
}

pub async fn test_redis_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Json(req): Json<TestRedisRequest>,
) -> Result<Json<RedisTestReport>, ApiError> {
    let active_row = state
        .db
        .get_active_service_config(ServiceKind::Redis)
        .await?;
    let cfg = match req.config {
        Some(c) => c,
        None => {
            let row = active_row
                .as_ref()
                .ok_or_else(|| ApiError::not_found("尚未保存 Redis 配置"))?;
            serde_json::from_str(&row.config_json).map_err(|e| {
                ApiError::internal("解析当前 Redis 配置失败").with_detail(e.to_string())
            })?
        }
    };

    let report = test_redis_connection(&cfg).await;
    if let Some(row) = active_row
        && let Ok(active_cfg) = serde_json::from_str::<RedisServiceConfig>(&row.config_json)
        && active_cfg == cfg
    {
        let _ = state
            .db
            .update_service_test_result(ServiceKind::Redis, row.version, report.ok, &report.message)
            .await;
    }
    Ok(Json(report))
}

pub async fn update_redis_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<UpdateRedisRequest>,
) -> Result<Json<UpdateServiceResponse<RedisServiceConfig>>, ApiError> {
    req.config.validate()?;
    let config_json = serde_json::to_string(&req.config)
        .map_err(|e| ApiError::internal("序列化 Redis 配置失败").with_detail(e.to_string()))?;

    let note = if req.note.trim().is_empty() {
        "服务配置中心更新"
    } else {
        req.note.trim()
    };
    let saved = state
        .db
        .save_service_config(ServiceKind::Redis, &config_json, &auth.user.username, note)
        .await?;

    if let Err(err) = state.services.reload_redis(&req.config).await {
        tracing::warn!(
            "重建 Panel Redis 连接池失败（配置已落盘 v{}）: {err}",
            saved.version
        );
    }
    state.metrics.invalidate_service_caches().await;

    let restarted_workers = if req.restart_affected_workers {
        let targets = affected_running_workers(&state.supervisor, ServiceKind::Redis).await?;
        restart_workers_sequentially(&state.supervisor, targets).await
    } else {
        Vec::new()
    };

    // 审计详情严禁包含密码明文
    let detail = serde_json::json!({
        "version": saved.version,
        "host": req.config.host,
        "port": req.config.port,
        "db": req.config.db,
        "tls": req.config.tls,
        "explicit_url_set": !req.config.url.trim().is_empty(),
        "restart_affected_workers": req.restart_affected_workers,
        "restarted_count": restarted_workers.len(),
    })
    .to_string();
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "settings.redis.update",
            "redis",
            &detail,
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(UpdateServiceResponse {
        version: saved.version,
        config: req.config,
        restarted_workers,
    }))
}

pub async fn test_manticore_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Json(req): Json<TestManticoreRequest>,
) -> Result<Json<ManticoreTestReport>, ApiError> {
    let active_row = state
        .db
        .get_active_service_config(ServiceKind::Manticore)
        .await?;
    let cfg = match req.config {
        Some(c) => c.normalized(),
        None => {
            let row = active_row
                .as_ref()
                .ok_or_else(|| ApiError::not_found("尚未保存 Manticore 配置"))?;
            serde_json::from_str::<ManticoreServiceConfig>(&row.config_json)
                .map_err(|e| {
                    ApiError::internal("解析当前 Manticore 配置失败").with_detail(e.to_string())
                })?
                .normalized()
        }
    };

    let report = test_manticore_connection(&cfg).await;
    if let Some(row) = active_row
        && let Ok(active_cfg) = serde_json::from_str::<ManticoreServiceConfig>(&row.config_json)
        && active_cfg.normalized() == cfg
    {
        let _ = state
            .db
            .update_service_test_result(
                ServiceKind::Manticore,
                row.version,
                report.ok,
                &report.message,
            )
            .await;
    }
    Ok(Json(report))
}

pub async fn init_manticore_table_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<TestManticoreRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cfg = match req.config {
        Some(c) => c,
        None => {
            let row = state
                .db
                .get_active_service_config(ServiceKind::Manticore)
                .await?
                .ok_or_else(|| ApiError::not_found("尚未保存 Manticore 配置"))?;
            serde_json::from_str(&row.config_json).map_err(|e| {
                ApiError::internal("解析当前 Manticore 配置失败").with_detail(e.to_string())
            })?
        }
    };

    match init_manticore_table_if_absent(&cfg).await {
        Ok(()) => {
            let detail = serde_json::json!({
                "url": sanitize_secret_in_text(&cfg.url),
                "auth_mode": cfg.effective_auth_mode().as_str(),
            })
            .to_string();
            let _ = state
                .db
                .record_audit(
                    &auth.user.username,
                    "settings.manticore.init_table",
                    "torrents",
                    &detail,
                    &extract_client_ip(&headers),
                )
                .await;
            state.metrics.invalidate_service_caches().await;
            Ok(Json(serde_json::json!({
                "ok": true,
                "message": "已在 Manticore 空库成功初始化 torrents 表"
            })))
        }
        Err(InitTableError::InvalidConfig(msg)) => {
            Err(ApiError::bad_request("INVALID_CONFIG", msg))
        }
        Err(InitTableError::AlreadyExists) => Err(ApiError::conflict(
            "TABLE_ALREADY_EXISTS",
            "Manticore 已存在 torrents 表，禁止自动覆盖或修改存量结构",
        )),
        Err(InitTableError::Backend(msg)) => Err(ApiError::bad_request("MANTICORE_ERROR", msg)),
    }
}

pub async fn update_manticore_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<UpdateManticoreRequest>,
) -> Result<Json<UpdateServiceResponse<ManticoreServiceConfig>>, ApiError> {
    let config = req.config.normalized();
    config.validate()?;
    let config_json = serde_json::to_string(&config)
        .map_err(|e| ApiError::internal("序列化 Manticore 配置失败").with_detail(e.to_string()))?;

    let note = if req.note.trim().is_empty() {
        "服务配置中心更新"
    } else {
        req.note.trim()
    };
    let saved = state
        .db
        .save_service_config(
            ServiceKind::Manticore,
            &config_json,
            &auth.user.username,
            note,
        )
        .await?;

    if let Err(err) = state.services.reload_manticore(&config).await {
        tracing::warn!(
            "重建 Panel Manticore 客户端失败（配置已落盘 v{}）: {err}",
            saved.version
        );
    }
    state.metrics.invalidate_service_caches().await;

    let restarted_workers = if req.restart_affected_workers {
        let targets = affected_running_workers(&state.supervisor, ServiceKind::Manticore).await?;
        restart_workers_sequentially(&state.supervisor, targets).await
    } else {
        Vec::new()
    };

    let header_name = (config.auth_mode == ManticoreAuthMode::Header)
        .then(|| config.header_name.trim().to_string());
    let detail = serde_json::json!({
        "version": saved.version,
        "url": sanitize_secret_in_text(&config.url),
        "auth_mode": config.auth_mode.as_str(),
        "has_auth": config.has_auth(),
        "has_basic_auth": config.basic_auth().is_some(),
        "header_name": header_name,
        "restart_affected_workers": req.restart_affected_workers,
        "restarted_count": restarted_workers.len(),
    })
    .to_string();
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "settings.manticore.update",
            "manticore",
            &detail,
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(UpdateServiceResponse {
        version: saved.version,
        config,
        restarted_workers,
    }))
}

pub async fn settings_history_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<ServiceHistoryItem>>, ApiError> {
    let kind: ServiceKind = query
        .kind
        .parse()
        .map_err(|e: String| ApiError::bad_request("INVALID_KIND", e))?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let rows = state.db.list_service_config_versions(kind, limit).await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let config = match serde_json::from_str(&row.config_json) {
            Ok(v) => v,
            Err(_) => serde_json::Value::String(row.config_json),
        };
        out.push(ServiceHistoryItem {
            id: row.id,
            kind: row.kind,
            version: row.version,
            config,
            is_active: row.is_active != 0,
            last_test_ok: row.last_test_ok.map(|v| v != 0),
            last_test_message: row.last_test_message,
            last_tested_at: row.last_tested_at,
            created_by: row.created_by,
            note: row.note,
            created_at: row.created_at,
        });
    }
    Ok(Json(out))
}

pub async fn settings_rollback_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<RollbackServiceRequest>,
) -> Result<Json<RollbackServiceResponse>, ApiError> {
    let kind: ServiceKind = req
        .kind
        .parse()
        .map_err(|e: String| ApiError::bad_request("INVALID_KIND", e))?;

    let saved = state
        .db
        .rollback_service_config(kind, req.target_version, &auth.user.username)
        .await?;

    let config_val: serde_json::Value = serde_json::from_str(&saved.config_json)
        .map_err(|e| ApiError::internal("解析回滚配置 JSON 失败").with_detail(e.to_string()))?;

    match kind {
        ServiceKind::Redis => {
            if let Ok(cfg) = serde_json::from_str::<RedisServiceConfig>(&saved.config_json) {
                let _ = state.services.reload_redis(&cfg).await;
            }
        }
        ServiceKind::Manticore => {
            if let Ok(cfg) = serde_json::from_str::<ManticoreServiceConfig>(&saved.config_json) {
                let _ = state.services.reload_manticore(&cfg).await;
            }
        }
    }
    state.metrics.invalidate_service_caches().await;

    let restarted_workers = if req.restart_affected_workers {
        let targets = affected_running_workers(&state.supervisor, kind).await?;
        restart_workers_sequentially(&state.supervisor, targets).await
    } else {
        Vec::new()
    };

    let detail = serde_json::json!({
        "kind": kind.as_str(),
        "target_version": req.target_version,
        "new_version": saved.version,
        "restart_affected_workers": req.restart_affected_workers,
        "restarted_count": restarted_workers.len(),
    })
    .to_string();
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "settings.service.rollback",
            kind.as_str(),
            &detail,
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(RollbackServiceResponse {
        kind: kind.as_str().to_string(),
        version: saved.version,
        config: config_val,
        restarted_workers,
    }))
}

pub async fn get_geoip_settings_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Result<Json<GeoIpSettingsResponse>, ApiError> {
    let config = load_geoip_config(&state.db).await;
    let inspect = inspect_geoip_file(&config, &state.config.data_dir).await;
    let running_crawlers = affected_running_crawlers(&state.supervisor).await?;

    Ok(Json(GeoIpSettingsResponse {
        config,
        resolved_path: inspect.resolved_path,
        mmdb_exists: inspect.mmdb_exists,
        mmdb_valid: inspect.mmdb_valid,
        mmdb_size_bytes: inspect.mmdb_size_bytes,
        mmdb_build_epoch: inspect.mmdb_build_epoch,
        database_type: inspect.database_type,
        node_count: inspect.node_count,
        status_message: inspect.status_message,
        running_crawlers,
    }))
}

pub async fn update_geoip_settings_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<UpdateGeoIpRequest>,
) -> Result<Json<UpdateGeoIpResponse>, ApiError> {
    req.config.validate()?;
    let mut normalized = req.config;
    normalized.home_country = normalized.normalized_home_country();
    normalized.mmdb_path = normalized.mmdb_path.trim().to_string();
    normalized.download_url = normalized.effective_download_url().to_string();
    normalized.prefix_cache_slots = normalized.normalized_prefix_cache_slots();

    let cfg_json = serde_json::to_string(&normalized)
        .map_err(|e| ApiError::internal("序列化 GeoIP 配置失败").with_detail(e.to_string()))?;
    state.db.set_meta(META_GEOIP_CONFIG_KEY, &cfg_json).await?;

    let reload_results = if req.reload_crawlers {
        state
            .supervisor
            .reload_all_crawlers_geoip(&normalized)
            .await
    } else {
        Vec::new()
    };
    state.metrics.invalidate_service_caches().await;
    let inspect = inspect_geoip_file(&normalized, &state.config.data_dir).await;

    let detail = serde_json::json!({
        "enabled": normalized.enabled,
        "home_country": normalized.home_country,
        "mmdb_path": normalized.mmdb_path,
        "download_url": normalized.download_url,
        "prefix_cache_slots": normalized.prefix_cache_slots,
        "adaptive_sample_threshold_pps": normalized.adaptive_sample_threshold_pps,
        "reload_crawlers": req.reload_crawlers,
        "reloaded_count": reload_results.len(),
        "mmdb_valid": inspect.mmdb_valid,
    })
    .to_string();
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "settings.geoip.update",
            "geoip",
            &detail,
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(UpdateGeoIpResponse {
        config: normalized,
        inspect,
        reload_results,
    }))
}

pub async fn reload_geoip_settings_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
) -> Result<Json<ReloadGeoIpResponse>, ApiError> {
    let config = load_geoip_config(&state.db).await;
    let reload_results = state.supervisor.reload_all_crawlers_geoip(&config).await;
    state.metrics.invalidate_service_caches().await;
    let inspect = inspect_geoip_file(&config, &state.config.data_dir).await;

    let detail = serde_json::json!({
        "mmdb_path": config.mmdb_path,
        "reloaded_count": reload_results.len(),
        "mmdb_valid": inspect.mmdb_valid,
    })
    .to_string();
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "settings.geoip.reload",
            "geoip",
            &detail,
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(ReloadGeoIpResponse {
        config,
        inspect,
        reload_results,
    }))
}

fn extract_release_tag_from_url_path(path: &str) -> Option<String> {
    let idx = path.find("/releases/download/")?;
    let rest = &path[idx + "/releases/download/".len()..];
    let tag = rest.split('/').next()?.trim();
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}

async fn download_and_validate_mmdb_to_temp(
    source_url: &str,
    target_path: &Path,
) -> Result<(PathBuf, u64, Option<String>), ApiError> {
    if target_path.is_dir() {
        return Err(ApiError::bad_request(
            "INVALID_MMDB_PATH",
            "MMDB 目标路径不能是目录",
        ));
    }
    let file_name = target_path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("INVALID_MMDB_PATH", "无效的 MMDB 文件路径"))?;
    let parent = match target_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| ApiError::internal("创建 GeoIP 数据目录失败").with_detail(e.to_string()))?;

    let captured_tag = Arc::new(StdMutex::new(
        reqwest::Url::parse(source_url)
            .ok()
            .and_then(|u| extract_release_tag_from_url_path(u.path())),
    ));
    let redirect_tag = Arc::clone(&captured_tag);
    let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
        if let Some(tag) = extract_release_tag_from_url_path(attempt.url().path())
            && let Ok(mut guard) = redirect_tag.lock()
        {
            *guard = Some(tag);
        }
        if attempt.previous().len() >= 10 {
            attempt.error("下载链接重定向次数过多")
        } else {
            attempt.follow()
        }
    });

    let client = reqwest::Client::builder()
        .user_agent("hameta-panel/0.1")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .redirect(redirect_policy)
        .build()
        .map_err(|e| ApiError::internal("构建 HTTP 下载客户端失败").with_detail(e.to_string()))?;

    let mut resp = client.get(source_url).send().await.map_err(|e| {
        ApiError::bad_request(
            "GEOIP_DOWNLOAD_FAILED",
            format!("下载 GeoIP 数据库失败: {e}"),
        )
    })?;

    if let Some(tag) = extract_release_tag_from_url_path(resp.url().path())
        && let Ok(mut guard) = captured_tag.lock()
    {
        *guard = Some(tag);
    }

    if !resp.status().is_success() {
        return Err(ApiError::bad_request(
            "GEOIP_DOWNLOAD_FAILED",
            format!("下载失败 (HTTP {}): {}", resp.status(), source_url),
        ));
    }

    if let Some(len) = resp.content_length()
        && len > MAX_GEOIP_MMDB_BYTES
    {
        return Err(ApiError::bad_request(
            "GEOIP_FILE_TOO_LARGE",
            format!("远程文件体积 ({len} B) 超过 256 MiB 上限"),
        ));
    }

    let tmp_path = parent.join(format!(
        ".{}.download-{}.tmp",
        file_name,
        chrono::Utc::now().timestamp_millis()
    ));
    let mut tmp_file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| ApiError::internal("创建 GeoIP 临时文件失败").with_detail(e.to_string()))?;

    let mut downloaded_bytes: u64 = 0;
    let write_res: Result<(), ApiError> = async {
        while let Some(chunk) = resp.chunk().await.map_err(|e| {
            ApiError::bad_request("GEOIP_DOWNLOAD_FAILED", format!("流式下载中断: {e}"))
        })? {
            downloaded_bytes = downloaded_bytes.saturating_add(chunk.len() as u64);
            if downloaded_bytes > MAX_GEOIP_MMDB_BYTES {
                return Err(ApiError::bad_request(
                    "GEOIP_FILE_TOO_LARGE",
                    "下载文件超过 256 MiB 安全上限",
                ));
            }
            tmp_file.write_all(&chunk).await.map_err(|e| {
                ApiError::internal("写入 GeoIP 临时文件失败").with_detail(e.to_string())
            })?;
        }
        tmp_file.flush().await.map_err(|e| {
            ApiError::internal("刷新 GeoIP 临时文件失败").with_detail(e.to_string())
        })?;
        tmp_file.sync_all().await.map_err(|e| {
            ApiError::internal("落盘同步 GeoIP 临时文件失败").with_detail(e.to_string())
        })?;
        Ok(())
    }
    .await;
    drop(tmp_file);

    if let Err(err) = write_res {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(err);
    }

    if downloaded_bytes == 0 {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(ApiError::bad_request(
            "INVALID_MMDB",
            "下载文件校验失败，远端返回空文件 (0 B)",
        ));
    }

    let verify_path = tmp_path.clone();
    let verify_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let reader = maxminddb::Reader::open_readfile(&verify_path)
            .map_err(|e| format!("无法解析为有效 MMDB 文件: {e}"))?;
        let md = reader.metadata();
        if md.node_count == 0 {
            return Err("MMDB 元数据无效 (node_count = 0)".to_string());
        }
        let probe_ip: std::net::IpAddr = "8.8.8.8".parse().expect("valid static ip");
        reader
            .lookup(probe_ip)
            .map_err(|e| format!("MMDB 路由树查询校验失败: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("MMDB 校验任务异常: {e}"));

    if let Err(reason) | Ok(Err(reason)) = verify_result {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(ApiError::bad_request(
            "INVALID_MMDB",
            format!("下载文件校验失败，非有效 MMDB 数据库: {reason}"),
        ));
    }

    let release_tag = captured_tag.lock().ok().and_then(|g| g.clone());
    Ok((tmp_path, downloaded_bytes, release_tag))
}

pub async fn download_geoip_mmdb_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<DownloadGeoIpRequest>,
) -> Result<Json<DownloadGeoIpResponse>, ApiError> {
    let _guard = GEOIP_DOWNLOAD_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .try_lock()
        .map_err(|_| {
            ApiError::conflict(
                "GEOIP_DOWNLOAD_IN_PROGRESS",
                "GeoIP 数据库正在下载或更新中，请稍候",
            )
        })?;

    let started = Instant::now();
    let mut config = load_geoip_config(&state.db).await;
    if let Some(path) = req.mmdb_path {
        config.mmdb_path = path.trim().to_string();
    }
    if config.mmdb_path.trim().is_empty() {
        config.mmdb_path = "GeoLite2-Country.mmdb".to_string();
    }
    if let Some(url) = req.download_url {
        config.download_url = url.trim().to_string();
    }
    config.download_url = config.effective_download_url().to_string();
    config.home_country = config.normalized_home_country();
    config.prefix_cache_slots = config.normalized_prefix_cache_slots();
    config.validate()?;

    let target_path = config.resolve_mmdb_path(&state.config.data_dir);
    let before_inspect = inspect_geoip_file(&config, &state.config.data_dir).await;
    let was_valid = before_inspect.mmdb_valid;
    let previous_build_epoch = before_inspect.mmdb_build_epoch;
    let source_url = config.effective_download_url().to_string();

    let (tmp_path, downloaded_bytes, release_tag) =
        download_and_validate_mmdb_to_temp(&source_url, &target_path).await?;

    if let Err(err) = tokio::fs::rename(&tmp_path, &target_path).await {
        #[cfg(windows)]
        let rename_err = if target_path.exists() {
            let mut disabled_cfg = config.clone();
            disabled_cfg.enabled = false;
            let _ = state
                .supervisor
                .reload_all_crawlers_geoip(&disabled_cfg)
                .await;
            let _ = tokio::fs::remove_file(&target_path).await;
            tokio::fs::rename(&tmp_path, &target_path).await.err()
        } else {
            Some(err)
        };
        #[cfg(not(windows))]
        let rename_err = Some(err);

        if let Some(final_err) = rename_err {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(
                ApiError::internal("替换 GeoIP MMDB 文件失败").with_detail(final_err.to_string())
            );
        }
    }

    let cfg_json = serde_json::to_string(&config)
        .map_err(|e| ApiError::internal("序列化 GeoIP 配置失败").with_detail(e.to_string()))?;
    state.db.set_meta(META_GEOIP_CONFIG_KEY, &cfg_json).await?;

    let reload_results = if req.reload_crawlers {
        state.supervisor.reload_all_crawlers_geoip(&config).await
    } else {
        Vec::new()
    };
    state.metrics.invalidate_service_caches().await;
    let inspect = inspect_geoip_file(&config, &state.config.data_dir).await;

    let changed = !was_valid || previous_build_epoch != inspect.mmdb_build_epoch;
    let duration_ms = started.elapsed().as_millis() as u64;
    let base_msg = if !was_valid {
        "已下载并加载 GeoLite2 离线库"
    } else if changed {
        "已更新至最新 GeoLite2 离线库"
    } else {
        "已拉取最新 Release 并校验通过，当前已是最新版本"
    };
    let message = match &release_tag {
        Some(tag) => format!("{base_msg} (Release {tag})"),
        None => base_msg.to_string(),
    };

    let detail = serde_json::json!({
        "action": if was_valid { "update" } else { "download" },
        "source_url": source_url,
        "release_tag": release_tag,
        "resolved_path": inspect.resolved_path,
        "downloaded_bytes": downloaded_bytes,
        "build_epoch": inspect.mmdb_build_epoch,
        "previous_build_epoch": previous_build_epoch,
        "changed": changed,
        "reloaded_count": reload_results.len(),
    })
    .to_string();
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "settings.geoip.download",
            "geoip",
            &detail,
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(DownloadGeoIpResponse {
        config,
        inspect,
        reload_results,
        downloaded_bytes,
        duration_ms,
        source_url,
        release_tag,
        previous_build_epoch,
        changed,
        message,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderValue, StatusCode, header};
    use axum::response::Response;
    use axum::routing::get;
    use tokio::net::TcpListener;

    fn encode_ctrl(type_num: usize, size: usize, out: &mut Vec<u8>) {
        if type_num <= 7 {
            out.push(((type_num << 5) | size) as u8);
        } else {
            out.push(size as u8);
            out.push((type_num - 7) as u8);
        }
    }

    fn encode_str(s: &str, out: &mut Vec<u8>) {
        encode_ctrl(2, s.len(), out);
        out.extend_from_slice(s.as_bytes());
    }

    fn encode_uint16(val: u16, out: &mut Vec<u8>) {
        let bytes = val.to_be_bytes();
        let trimmed = if bytes[0] == 0 {
            if bytes[1] == 0 { &[][..] } else { &bytes[1..2] }
        } else {
            &bytes[..]
        };
        encode_ctrl(5, trimmed.len(), out);
        out.extend_from_slice(trimmed);
    }

    fn encode_uint32(val: u32, out: &mut Vec<u8>) {
        let bytes = val.to_be_bytes();
        let first_nz = bytes.iter().position(|&b| b != 0).unwrap_or(4);
        let trimmed = &bytes[first_nz..];
        encode_ctrl(6, trimmed.len(), out);
        out.extend_from_slice(trimmed);
    }

    fn encode_uint64(val: u64, out: &mut Vec<u8>) {
        let bytes = val.to_be_bytes();
        let first_nz = bytes.iter().position(|&b| b != 0).unwrap_or(8);
        let trimmed = &bytes[first_nz..];
        encode_ctrl(9, trimmed.len(), out);
        out.extend_from_slice(trimmed);
    }

    fn build_minimal_country_mmdb(iso: &str, build_epoch: u64) -> Vec<u8> {
        let node_count: u32 = 1;
        let record_size: u16 = 24;
        let mut data_section = Vec::new();
        let offset_a = data_section.len();
        encode_ctrl(7, 1, &mut data_section);
        encode_str("country", &mut data_section);
        encode_ctrl(7, 1, &mut data_section);
        encode_str("iso_code", &mut data_section);
        encode_str(iso, &mut data_section);

        let leaf_val = node_count + 16 + offset_a as u32;
        let mut out = Vec::new();
        out.extend_from_slice(&leaf_val.to_be_bytes()[1..4]);
        out.extend_from_slice(&leaf_val.to_be_bytes()[1..4]);
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(&data_section);
        out.extend_from_slice(b"\xab\xcd\xefMaxMind.com");

        encode_ctrl(7, 9, &mut out);
        encode_str("node_count", &mut out);
        encode_uint32(node_count, &mut out);
        encode_str("record_size", &mut out);
        encode_uint16(record_size, &mut out);
        encode_str("ip_version", &mut out);
        encode_uint16(6, &mut out);
        encode_str("binary_format_major_version", &mut out);
        encode_uint16(2, &mut out);
        encode_str("binary_format_minor_version", &mut out);
        encode_uint16(0, &mut out);
        encode_str("build_epoch", &mut out);
        encode_uint64(build_epoch, &mut out);
        encode_str("database_type", &mut out);
        encode_str("GeoLite2-Country", &mut out);
        encode_str("description", &mut out);
        encode_ctrl(7, 1, &mut out);
        encode_str("en", &mut out);
        encode_str("Test", &mut out);
        encode_str("languages", &mut out);
        encode_ctrl(11, 1, &mut out);
        encode_str("en", &mut out);
        out
    }

    #[test]
    fn extract_release_tag_from_github_redirect_path() {
        assert_eq!(
            extract_release_tag_from_url_path(
                "/P3TERX/GeoLite.mmdb/releases/download/2026.11.20/GeoLite2-Country.mmdb"
            ),
            Some("2026.11.20".to_string())
        );
        assert_eq!(
            extract_release_tag_from_url_path("/P3TERX/GeoLite.mmdb/releases/latest"),
            None
        );
    }

    #[tokio::test]
    async fn download_and_validate_mmdb_succeeds_and_rejects_invalid_payload() {
        let mmdb_bytes = Arc::new(build_minimal_country_mmdb("US", 1_763_900_000));
        let payload_clone = Arc::clone(&mmdb_bytes);

        let app = Router::new()
            .route(
                "/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-Country.mmdb",
                get(|| async {
                    let mut resp = Response::new(Body::empty());
                    *resp.status_mut() = StatusCode::FOUND;
                    resp.headers_mut().insert(
                        header::LOCATION,
                        HeaderValue::from_static(
                            "/P3TERX/GeoLite.mmdb/releases/download/2026.11.20/GeoLite2-Country.mmdb",
                        ),
                    );
                    resp
                }),
            )
            .route(
                "/P3TERX/GeoLite.mmdb/releases/download/2026.11.20/GeoLite2-Country.mmdb",
                get(move || {
                    let data = Arc::clone(&payload_clone);
                    async move {
                        let mut resp = Response::new(Body::from((*data).clone()));
                        resp.headers_mut().insert(
                            header::CONTENT_LENGTH,
                            HeaderValue::from_str(&data.len().to_string()).unwrap(),
                        );
                        resp
                    }
                }),
            )
            .route(
                "/invalid.mmdb",
                get(|| async { "<html>502 Bad Gateway</html>" }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let test_dir = std::env::temp_dir().join(format!(
            "hameta-geoip-dl-{}",
            hameta_common::generate_token()
        ));
        let target_path = test_dir.join("nested").join("GeoLite2-Country.mmdb");

        // 1. 合法 MMDB 经重定向后应下载并校验通过
        let valid_url = format!(
            "http://{addr}/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-Country.mmdb"
        );
        let (tmp_path, downloaded_bytes, tag) =
            download_and_validate_mmdb_to_temp(&valid_url, &target_path)
                .await
                .expect("valid test mmdb should download and pass validation");
        assert_eq!(downloaded_bytes, mmdb_bytes.len() as u64);
        assert_eq!(tag.as_deref(), Some("2026.11.20"));
        tokio::fs::rename(&tmp_path, &target_path).await.unwrap();
        assert!(target_path.exists());

        // 2. 无效响应体必须被拒绝，且不得遗留临时文件或破坏已安装的合法 MMDB
        let invalid_url = format!("http://{addr}/invalid.mmdb");
        let err = download_and_validate_mmdb_to_temp(&invalid_url, &target_path)
            .await
            .expect_err("non-mmdb payload must be rejected");
        assert_eq!(err.code, "INVALID_MMDB");

        let leftover_tmps: Vec<_> = std::fs::read_dir(target_path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftover_tmps.is_empty(), "temp file must be cleaned up");
        assert_eq!(
            std::fs::read(&target_path).unwrap(),
            *mmdb_bytes,
            "existing valid mmdb must remain intact"
        );

        server_task.abort();
        let _ = std::fs::remove_dir_all(&test_dir);
    }
}
