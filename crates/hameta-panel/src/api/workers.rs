use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::server::PanelState;
use crate::supervisor::WorkerStatusView;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use hameta_common::{
    CrawlProfileDefaults, CrawlProfileInfo, CrawlerWorkerConfig, DesiredState, IndexerWorkerConfig,
    WorkerRole, WorkerRuntimeState, all_crawl_profiles, crawl_profile_defaults,
    current_soft_open_file_limit,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct DhtProfilesResponse {
    pub profiles: Vec<CrawlProfileInfo>,
    pub system_soft_nofile: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct WorkerSummaryDto {
    #[serde(flatten)]
    pub status: WorkerStatusView,
    pub summary: Value,
}

#[derive(Debug, Deserialize)]
pub struct CreateWorkerRequest {
    pub id: String,
    pub role: WorkerRole,
    pub display_name: String,
    pub config: Value,
    #[serde(default = "default_true")]
    pub auto_restart: bool,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: i64,
    #[serde(default = "default_backoff_secs")]
    pub restart_backoff_secs: i64,
}

fn default_true() -> bool {
    true
}
fn default_max_restarts() -> i64 {
    5
}
fn default_backoff_secs() -> i64 {
    3
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkerPolicyRequest {
    pub display_name: String,
    pub auto_restart: bool,
    pub max_restarts: i64,
    pub restart_backoff_secs: i64,
}

#[derive(Debug, Serialize)]
pub struct WorkerConfigDetailResponse {
    pub id: String,
    pub role: WorkerRole,
    pub version: i64,
    pub config: Value,
    pub profile_defaults: Option<CrawlProfileDefaults>,
    pub note: String,
    pub created_by: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct UpdateWorkerConfigRequest {
    pub config: Value,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub apply_restart: bool,
}

#[derive(Debug, Serialize)]
pub struct UpdateWorkerConfigResponse {
    pub version: i64,
    pub config: Value,
    pub restarted: bool,
    pub restart_error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VersionsQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct WorkerConfigVersionItem {
    pub id: i64,
    pub version: i64,
    pub config: Value,
    pub created_by: String,
    pub note: String,
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct RollbackWorkerConfigRequest {
    pub target_version: i64,
    #[serde(default)]
    pub apply_restart: bool,
}

fn is_valid_worker_id(id: &str) -> bool {
    let trimmed = id.trim();
    (2..=48).contains(&trimmed.len())
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn normalize_and_validate_config(role: WorkerRole, raw: Value) -> Result<Value, ApiError> {
    match role {
        WorkerRole::Crawler => {
            let cfg: CrawlerWorkerConfig = serde_json::from_value(raw).map_err(|e| {
                ApiError::bad_request(
                    "INVALID_WORKER_CONFIG",
                    format!("Crawler 参数校验失败: {e}"),
                )
            })?;
            cfg.validate()?;
            serde_json::to_value(cfg).map_err(|e| {
                ApiError::internal("序列化 Crawler 配置失败").with_detail(e.to_string())
            })
        }
        WorkerRole::Indexer => {
            let cfg: IndexerWorkerConfig = serde_json::from_value(raw).map_err(|e| {
                ApiError::bad_request(
                    "INVALID_WORKER_CONFIG",
                    format!("Indexer 参数校验失败: {e}"),
                )
            })?;
            cfg.validate()?;
            serde_json::to_value(cfg).map_err(|e| {
                ApiError::internal("序列化 Indexer 配置失败").with_detail(e.to_string())
            })
        }
    }
}

async fn ensure_no_crawler_port_conflict(
    state: &PanelState,
    exclude_id: Option<&str>,
    port: u16,
) -> Result<(), ApiError> {
    let all = state.db.list_workers().await?;
    for w in all {
        if w.role != WorkerRole::Crawler.as_str() {
            continue;
        }
        if exclude_id == Some(w.id.as_str()) {
            continue;
        }
        if let Some(cfg_row) = state.db.get_active_worker_config(&w.id).await?
            && let Ok(cfg) = serde_json::from_str::<CrawlerWorkerConfig>(&cfg_row.config_json)
            && cfg.port == port
        {
            return Err(ApiError::conflict(
                "PORT_CONFLICT",
                format!("UDP 端口 {port} 已被节点 {} 占用", w.id),
            ));
        }
    }
    Ok(())
}

fn summarize_worker_config(role: WorkerRole, config_json: &str) -> Value {
    match role {
        WorkerRole::Crawler => serde_json::from_str::<CrawlerWorkerConfig>(config_json)
            .map(|c| {
                serde_json::json!({
                    "port": c.port,
                    "crawl_profile": c.crawl_profile,
                    "netmode": c.netmode,
                    "log_level": c.log_level,
                    "max_worker_count": c.metadata.max_worker_count,
                })
            })
            .unwrap_or(Value::Null),
        WorkerRole::Indexer => serde_json::from_str::<IndexerWorkerConfig>(config_json)
            .map(|c| {
                serde_json::json!({
                    "torrent_workers": c.torrent_workers,
                    "insert_workers": c.insert_workers,
                    "batch_size": c.batch_size,
                    "log_level": c.log_level,
                })
            })
            .unwrap_or(Value::Null),
    }
}

pub async fn get_dht_profiles_handler(_auth: AuthUser) -> Json<DhtProfilesResponse> {
    Json(DhtProfilesResponse {
        profiles: all_crawl_profiles(),
        system_soft_nofile: current_soft_open_file_limit(),
    })
}

pub async fn list_workers_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Result<Json<Vec<WorkerSummaryDto>>, ApiError> {
    let statuses = state
        .supervisor
        .list_worker_statuses()
        .await
        .map_err(|e| ApiError::internal("读取 Worker 状态失败").with_detail(e.to_string()))?;

    let mut out = Vec::with_capacity(statuses.len());
    for st in statuses {
        let summary = match state.db.get_active_worker_config(&st.id).await? {
            Some(row) => summarize_worker_config(st.role, &row.config_json),
            None => Value::Null,
        };
        out.push(WorkerSummaryDto {
            status: st,
            summary,
        });
    }
    Ok(Json(out))
}

pub async fn create_worker_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<CreateWorkerRequest>,
) -> Result<Json<Value>, ApiError> {
    let id = req.id.trim();
    if !is_valid_worker_id(id) {
        return Err(ApiError::bad_request(
            "INVALID_WORKER_ID",
            "实例 ID 必须为 2~48 位字母、数字、下划线或短横线",
        ));
    }
    if req.display_name.trim().is_empty() {
        return Err(ApiError::bad_request("INVALID_INPUT", "显示名称不能为空"));
    }
    if state.db.get_worker(id).await?.is_some() {
        return Err(ApiError::conflict(
            "WORKER_EXISTS",
            format!("Worker 实例 {id} 已存在"),
        ));
    }

    let normalized = normalize_and_validate_config(req.role, req.config)?;
    if req.role == WorkerRole::Crawler
        && let Some(port) = normalized.get("port").and_then(Value::as_u64)
        && let Ok(p) = u16::try_from(port)
    {
        ensure_no_crawler_port_conflict(&state, None, p).await?;
    }

    let config_json = serde_json::to_string(&normalized)
        .map_err(|e| ApiError::internal("序列化配置失败").with_detail(e.to_string()))?;
    let row = state
        .db
        .create_worker(
            id,
            req.role,
            req.display_name.trim(),
            &config_json,
            &auth.user.username,
        )
        .await?;
    state
        .db
        .update_worker_policy(
            id,
            req.display_name.trim(),
            req.auto_restart,
            req.max_restarts,
            req.restart_backoff_secs,
        )
        .await?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.create",
            id,
            &serde_json::json!({ "role": req.role.as_str() }).to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(serde_json::json!({
        "id": row.id,
        "role": req.role,
        "display_name": req.display_name.trim(),
        "version": 1
    })))
}

pub async fn delete_worker_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let statuses = state
        .supervisor
        .list_worker_statuses()
        .await
        .map_err(|e| ApiError::internal("读取 Worker 状态失败").with_detail(e.to_string()))?;
    let Some(target) = statuses.into_iter().find(|w| w.id == id) else {
        return Err(ApiError::not_found(format!("Worker 实例 {id} 不存在")));
    };
    if !matches!(
        target.runtime_state,
        WorkerRuntimeState::Stopped | WorkerRuntimeState::Crashed
    ) {
        return Err(ApiError::conflict(
            "WORKER_RUNNING",
            "运行中或正在排空的实例禁止删除，请先停止该节点",
        ));
    }

    state.db.delete_worker(&id).await?;
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.delete",
            &id,
            "{}",
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn start_worker_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state.db.get_worker(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("Worker 实例 {id} 不存在")));
    }
    state
        .db
        .set_worker_desired_state(&id, DesiredState::Running)
        .await?;
    if let Err(err) = state.supervisor.start_worker(&id).await {
        let _ = state
            .db
            .set_worker_desired_state(&id, DesiredState::Stopped)
            .await;
        return Err(ApiError::bad_request(
            "WORKER_START_FAILED",
            err.to_string(),
        ));
    }

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.start",
            &id,
            "{}",
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn stop_worker_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state.db.get_worker(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("Worker 实例 {id} 不存在")));
    }
    state
        .db
        .set_worker_desired_state(&id, DesiredState::Stopped)
        .await?;
    state
        .supervisor
        .stop_worker(&id, 15)
        .await
        .map_err(|e| ApiError::internal("停止 Worker 失败").with_detail(e.to_string()))?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.stop",
            &id,
            "{}",
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn restart_worker_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if state.db.get_worker(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("Worker 实例 {id} 不存在")));
    }
    state
        .db
        .set_worker_desired_state(&id, DesiredState::Running)
        .await?;
    state
        .supervisor
        .restart_worker(&id, 15)
        .await
        .map_err(|e| ApiError::bad_request("WORKER_RESTART_FAILED", e.to_string()))?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.restart",
            &id,
            "{}",
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn update_worker_policy_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateWorkerPolicyRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.display_name.trim().is_empty() {
        return Err(ApiError::bad_request("INVALID_INPUT", "显示名称不能为空"));
    }
    state
        .db
        .update_worker_policy(
            &id,
            req.display_name.trim(),
            req.auto_restart,
            req.max_restarts,
            req.restart_backoff_secs,
        )
        .await?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.policy.update",
            &id,
            &serde_json::json!({
                "auto_restart": req.auto_restart,
                "max_restarts": req.max_restarts,
                "restart_backoff_secs": req.restart_backoff_secs,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn get_worker_config_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<WorkerConfigDetailResponse>, ApiError> {
    let row = state
        .db
        .get_worker(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Worker 实例 {id} 不存在")))?;
    let role: WorkerRole = row
        .role
        .parse()
        .map_err(|e: String| ApiError::internal(e))?;
    let cfg_row = state
        .db
        .get_active_worker_config(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Worker 实例 {id} 无激活配置")))?;

    let config: Value = serde_json::from_str(&cfg_row.config_json)
        .map_err(|e| ApiError::internal("解析 Worker 配置失败").with_detail(e.to_string()))?;

    let profile_defaults = if role == WorkerRole::Crawler {
        config
            .get("crawl_profile")
            .and_then(Value::as_str)
            .and_then(crawl_profile_defaults)
    } else {
        None
    };

    Ok(Json(WorkerConfigDetailResponse {
        id,
        role,
        version: cfg_row.version,
        config,
        profile_defaults,
        note: cfg_row.note,
        created_by: cfg_row.created_by,
        created_at: cfg_row.created_at,
    }))
}

pub async fn update_worker_config_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateWorkerConfigRequest>,
) -> Result<Json<UpdateWorkerConfigResponse>, ApiError> {
    let row = state
        .db
        .get_worker(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Worker 实例 {id} 不存在")))?;
    let role: WorkerRole = row
        .role
        .parse()
        .map_err(|e: String| ApiError::internal(e))?;

    let normalized = normalize_and_validate_config(role, req.config)?;
    if role == WorkerRole::Crawler
        && let Some(port) = normalized.get("port").and_then(Value::as_u64)
        && let Ok(p) = u16::try_from(port)
    {
        ensure_no_crawler_port_conflict(&state, Some(&id), p).await?;
    }

    let config_json = serde_json::to_string(&normalized)
        .map_err(|e| ApiError::internal("序列化配置失败").with_detail(e.to_string()))?;
    let note = if req.note.trim().is_empty() {
        "面板参数更新"
    } else {
        req.note.trim()
    };
    let saved = state
        .db
        .save_worker_config_version(&id, &config_json, &auth.user.username, note)
        .await?;

    let (restarted, restart_error) = if req.apply_restart {
        let statuses = state
            .supervisor
            .list_worker_statuses()
            .await
            .unwrap_or_default();
        let active = statuses.into_iter().find(|w| w.id == id).is_some_and(|w| {
            matches!(
                w.runtime_state,
                WorkerRuntimeState::Running | WorkerRuntimeState::Starting
            )
        });
        if active {
            match state.supervisor.restart_worker(&id, 15).await {
                Ok(()) => (true, None),
                Err(err) => (false, Some(err.to_string())),
            }
        } else {
            (false, None)
        }
    } else {
        (false, None)
    };

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.config.update",
            &id,
            &serde_json::json!({
                "version": saved.version,
                "apply_restart": req.apply_restart,
                "restarted": restarted,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(UpdateWorkerConfigResponse {
        version: saved.version,
        config: normalized,
        restarted,
        restart_error,
    }))
}

pub async fn list_worker_config_versions_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<VersionsQuery>,
) -> Result<Json<Vec<WorkerConfigVersionItem>>, ApiError> {
    if state.db.get_worker(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("Worker 实例 {id} 不存在")));
    }
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let rows = state.db.list_worker_config_versions(&id, limit).await?;
    let items = rows
        .into_iter()
        .map(|r| {
            let config = match serde_json::from_str(&r.config_json) {
                Ok(v) => v,
                Err(_) => Value::String(r.config_json),
            };
            WorkerConfigVersionItem {
                id: r.id,
                version: r.version,
                config,
                created_by: r.created_by,
                note: r.note,
                created_at: r.created_at,
            }
        })
        .collect();
    Ok(Json(items))
}

pub async fn rollback_worker_config_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<RollbackWorkerConfigRequest>,
) -> Result<Json<UpdateWorkerConfigResponse>, ApiError> {
    let row = state
        .db
        .get_worker(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Worker 实例 {id} 不存在")))?;
    let role: WorkerRole = row
        .role
        .parse()
        .map_err(|e: String| ApiError::internal(e))?;

    let saved = state
        .db
        .rollback_worker_config(&id, req.target_version, &auth.user.username)
        .await?;
    let config: Value = serde_json::from_str(&saved.config_json)
        .map_err(|e| ApiError::internal("解析回滚配置失败").with_detail(e.to_string()))?;

    if role == WorkerRole::Crawler
        && let Some(port) = config.get("port").and_then(Value::as_u64)
        && let Ok(p) = u16::try_from(port)
    {
        ensure_no_crawler_port_conflict(&state, Some(&id), p).await?;
    }

    let (restarted, restart_error) = if req.apply_restart {
        let statuses = state
            .supervisor
            .list_worker_statuses()
            .await
            .unwrap_or_default();
        let active = statuses.into_iter().find(|w| w.id == id).is_some_and(|w| {
            matches!(
                w.runtime_state,
                WorkerRuntimeState::Running | WorkerRuntimeState::Starting
            )
        });
        if active {
            match state.supervisor.restart_worker(&id, 15).await {
                Ok(()) => (true, None),
                Err(err) => (false, Some(err.to_string())),
            }
        } else {
            (false, None)
        }
    } else {
        (false, None)
    };

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.config.rollback",
            &id,
            &serde_json::json!({
                "target_version": req.target_version,
                "new_version": saved.version,
                "restarted": restarted,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(UpdateWorkerConfigResponse {
        version: saved.version,
        config,
        restarted,
        restart_error,
    }))
}

#[derive(Debug, Serialize)]
pub struct WorkerDiagnosticsResponse {
    pub id: String,
    pub display_name: String,
    pub role: WorkerRole,
    pub runtime_state: WorkerRuntimeState,
    pub pid: Option<u32>,
    pub worker_port: Option<u16>,
    pub active_config_version: i64,
    pub active_rule_version: Option<i64>,
    pub started_at: Option<i64>,
    pub restart_count: u64,
    pub last_error: Option<String>,
    pub captured_at_unix: Option<u64>,
    pub snapshot: Option<Value>,
    pub config: Value,
    pub profile_defaults: Option<CrawlProfileDefaults>,
}

pub async fn get_worker_diagnostics_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<WorkerDiagnosticsResponse>, ApiError> {
    if state.db.get_worker(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("Worker 实例 {id} 不存在")));
    }

    state.supervisor.poll_worker_snapshot_once(&id).await;

    let statuses = state
        .supervisor
        .list_worker_statuses()
        .await
        .map_err(|e| ApiError::internal("读取 Worker 状态失败").with_detail(e.to_string()))?;
    let target = statuses
        .into_iter()
        .find(|w| w.id == id)
        .ok_or_else(|| ApiError::not_found(format!("Worker 实例 {id} 不存在")))?;

    let config = match state.db.get_active_worker_config(&id).await? {
        Some(row) => serde_json::from_str::<Value>(&row.config_json).unwrap_or(Value::Null),
        None => Value::Null,
    };
    let profile_defaults = if target.role == WorkerRole::Crawler {
        config
            .get("crawl_profile")
            .and_then(Value::as_str)
            .and_then(crawl_profile_defaults)
    } else {
        None
    };

    let (captured_at_unix, snapshot) = match target.snapshot {
        Some(env) => (Some(env.captured_at_unix), Some(env.payload)),
        None => (None, None),
    };

    Ok(Json(WorkerDiagnosticsResponse {
        id: target.id,
        display_name: target.display_name,
        role: target.role,
        runtime_state: target.runtime_state,
        pid: target.pid,
        worker_port: target.worker_port,
        active_config_version: target.active_config_version,
        active_rule_version: target.active_rule_version,
        started_at: target.started_at,
        restart_count: target.restart_count,
        last_error: target.last_error,
        captured_at_unix,
        snapshot,
        config,
        profile_defaults,
    }))
}
