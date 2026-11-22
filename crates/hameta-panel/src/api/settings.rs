use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::server::PanelState;
use crate::services::{
    InitTableError, ManticoreTestReport, RedisTestReport, init_manticore_table_if_absent,
    test_manticore_connection, test_redis_connection,
};
use crate::supervisor::Supervisor;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use hameta_common::{
    ManticoreServiceConfig, RedisServiceConfig, ServiceKind, WorkerRole, WorkerRuntimeState,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
        let cfg: ManticoreServiceConfig = serde_json::from_str(&row.config_json).map_err(|e| {
            ApiError::internal("解析 Manticore 配置 JSON 失败").with_detail(e.to_string())
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
        Some(c) => c,
        None => {
            let row = active_row
                .as_ref()
                .ok_or_else(|| ApiError::not_found("尚未保存 Manticore 配置"))?;
            serde_json::from_str(&row.config_json).map_err(|e| {
                ApiError::internal("解析当前 Manticore 配置失败").with_detail(e.to_string())
            })?
        }
    };

    let report = test_manticore_connection(&cfg).await;
    if let Some(row) = active_row
        && let Ok(active_cfg) = serde_json::from_str::<ManticoreServiceConfig>(&row.config_json)
        && active_cfg == cfg
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
            let detail = serde_json::json!({ "url": cfg.url }).to_string();
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
    req.config.validate()?;
    let config_json = serde_json::to_string(&req.config)
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

    if let Err(err) = state.services.reload_manticore(&req.config).await {
        tracing::warn!(
            "重建 Panel Manticore 客户端失败（配置已落盘 v{}）: {err}",
            saved.version
        );
    }

    let restarted_workers = if req.restart_affected_workers {
        let targets = affected_running_workers(&state.supervisor, ServiceKind::Manticore).await?;
        restart_workers_sequentially(&state.supervisor, targets).await
    } else {
        Vec::new()
    };

    let detail = serde_json::json!({
        "version": saved.version,
        "url": req.config.url,
        "has_basic_auth": req.config.basic_auth().is_some(),
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
        config: req.config,
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
