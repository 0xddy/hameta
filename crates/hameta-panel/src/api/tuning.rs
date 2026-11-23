use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::server::PanelState;
use crate::tuning_advisor::{
    DEFAULT_SAMPLE_INTERVAL_SECS, DEFAULT_TUNING_DURATION_SECS, TuningConstraints, TuningGoal,
    TuningSessionStatusDto, TuningWorkerOverviewDto, apply_selected_recommendations,
};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use hameta_common::{CrawlerWorkerConfig, WorkerRole, WorkerRuntimeState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct TuningStatusQuery {
    pub source: Option<String>,
    pub up_mbps: Option<f64>,
    pub down_mbps: Option<f64>,
    pub goal: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StartTuningRequest {
    #[serde(default = "default_duration_secs")]
    pub duration_secs: u64,
    #[serde(default = "default_interval_secs")]
    pub sample_interval_secs: u64,
    #[serde(default)]
    pub up_mbps: Option<f64>,
    #[serde(default)]
    pub down_mbps: Option<f64>,
    #[serde(default)]
    pub goal: TuningGoal,
}

fn default_duration_secs() -> u64 {
    DEFAULT_TUNING_DURATION_SECS
}

fn default_interval_secs() -> u64 {
    DEFAULT_SAMPLE_INTERVAL_SECS
}

#[derive(Debug, Deserialize)]
pub struct ApplyTuningRequest {
    pub recommendation_ids: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub up_mbps: Option<f64>,
    #[serde(default)]
    pub down_mbps: Option<f64>,
    #[serde(default)]
    pub goal: Option<TuningGoal>,
    #[serde(default = "default_true")]
    pub apply_restart: bool,
    #[serde(default)]
    pub note: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct ApplyTuningResponse {
    pub version: i64,
    pub applied: Vec<String>,
    pub config: Value,
    pub restarted: bool,
    pub restart_error: Option<String>,
}

fn sanitize_positive_mbps(v: Option<f64>) -> Option<f64> {
    v.filter(|&m| m.is_finite() && m > 0.0)
        .map(|m| m.clamp(1.0, 100_000.0))
}

async fn ensure_crawler_worker(state: &PanelState, id: &str) -> Result<(), ApiError> {
    let row = state
        .db
        .get_worker(id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Worker 实例 {id} 不存在")))?;
    let role: WorkerRole = row
        .role
        .parse()
        .map_err(|e: String| ApiError::internal(e))?;
    if role != WorkerRole::Crawler {
        return Err(ApiError::bad_request(
            "UNSUPPORTED_ROLE",
            "智能参数诊断仅适用于 DHT Crawler 实例",
        ));
    }
    Ok(())
}

pub async fn list_tuning_sessions_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Json<Vec<TuningSessionStatusDto>> {
    Json(state.tuning_advisor.list_session_statuses().await)
}

pub async fn get_worker_tuning_status_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Path(id): Path<String>,
    Query(query): Query<TuningStatusQuery>,
) -> Result<Json<TuningWorkerOverviewDto>, ApiError> {
    ensure_crawler_worker(&state, &id).await?;
    let has_override = query.up_mbps.is_some() || query.down_mbps.is_some() || query.goal.is_some();
    let override_constraints = if has_override {
        Some(TuningConstraints {
            up_mbps: sanitize_positive_mbps(query.up_mbps),
            down_mbps: sanitize_positive_mbps(query.down_mbps),
            goal: TuningGoal::from_opt_str(query.goal.as_deref()),
        })
    } else {
        None
    };

    let overview = state
        .tuning_advisor
        .get_worker_overview(&id, query.source.as_deref(), override_constraints)
        .await
        .map_err(|msg| ApiError::bad_request("TUNING_STATUS_FAILED", msg))?;
    Ok(Json(overview))
}

pub async fn start_worker_tuning_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<StartTuningRequest>,
) -> Result<Json<TuningSessionStatusDto>, ApiError> {
    ensure_crawler_worker(&state, &id).await?;
    let constraints = TuningConstraints {
        up_mbps: sanitize_positive_mbps(req.up_mbps),
        down_mbps: sanitize_positive_mbps(req.down_mbps),
        goal: req.goal,
    };
    let dto = state
        .tuning_advisor
        .start_session(
            &id,
            req.duration_secs,
            req.sample_interval_secs,
            constraints.clone(),
        )
        .await
        .map_err(|msg| ApiError::bad_request("TUNING_START_FAILED", msg))?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.tuning.start",
            &id,
            &json!({
                "duration_secs": dto.target_duration_secs,
                "sample_interval_secs": dto.sample_interval_secs,
                "goal": constraints.goal.as_str(),
                "up_mbps": constraints.up_mbps,
                "down_mbps": constraints.down_mbps,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(dto))
}

pub async fn finish_worker_tuning_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<TuningSessionStatusDto>, ApiError> {
    ensure_crawler_worker(&state, &id).await?;
    let dto = state
        .tuning_advisor
        .finish_session(&id)
        .await
        .map_err(|msg| ApiError::bad_request("TUNING_FINISH_FAILED", msg))?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.tuning.finish",
            &id,
            &json!({
                "sample_count": dto.sample_count,
                "elapsed_secs": dto.elapsed_secs,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(dto))
}

pub async fn cancel_worker_tuning_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    ensure_crawler_worker(&state, &id).await?;
    let removed = state.tuning_advisor.cancel_session(&id).await;
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "worker.tuning.cancel",
            &id,
            &json!({ "removed": removed }).to_string(),
            &extract_client_ip(&headers),
        )
        .await;
    Ok(Json(json!({ "ok": true, "removed": removed })))
}

pub async fn apply_worker_tuning_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ApplyTuningRequest>,
) -> Result<Json<ApplyTuningResponse>, ApiError> {
    ensure_crawler_worker(&state, &id).await?;
    if req.recommendation_ids.is_empty() {
        return Err(ApiError::bad_request(
            "EMPTY_SELECTION",
            "请至少勾选 1 条参数优化建议",
        ));
    }

    let has_override = req.up_mbps.is_some() || req.down_mbps.is_some() || req.goal.is_some();
    let override_constraints = if has_override {
        Some(TuningConstraints {
            up_mbps: sanitize_positive_mbps(req.up_mbps),
            down_mbps: sanitize_positive_mbps(req.down_mbps),
            goal: req.goal.unwrap_or_default(),
        })
    } else {
        None
    };

    let overview = state
        .tuning_advisor
        .get_worker_overview(&id, req.source.as_deref(), override_constraints)
        .await
        .map_err(|msg| ApiError::bad_request("TUNING_REPORT_UNAVAILABLE", msg))?;
    let report = overview.report.ok_or_else(|| {
        ApiError::bad_request(
            "TUNING_REPORT_UNAVAILABLE",
            "当前暂无可用的分析报告，请等待采集至少 2 个样本",
        )
    })?;

    let cfg_row = state
        .db
        .get_active_worker_config(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Worker 实例 {id} 无激活配置")))?;
    let current_cfg: CrawlerWorkerConfig = serde_json::from_str(&cfg_row.config_json)
        .map_err(|e| ApiError::internal("解析当前 Crawler 配置失败").with_detail(e.to_string()))?;

    let (next_cfg, applied) =
        apply_selected_recommendations(&current_cfg, &report, &req.recommendation_ids)
            .map_err(|msg| ApiError::bad_request("TUNING_APPLY_FAILED", msg))?;

    let normalized_val = serde_json::to_value(&next_cfg)
        .map_err(|e| ApiError::internal("序列化新配置失败").with_detail(e.to_string()))?;
    let config_json = serde_json::to_string(&normalized_val)
        .map_err(|e| ApiError::internal("序列化配置 JSON 失败").with_detail(e.to_string()))?;

    let note = req
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            format!(
                "智能优化顾问应用 {} 项建议 ({})",
                applied.len(),
                report.regime_title
            )
        });

    let saved = state
        .db
        .save_worker_config_version(&id, &config_json, &auth.user.username, &note)
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
            "worker.tuning.apply",
            &id,
            &json!({
                "version": saved.version,
                "applied": applied,
                "apply_restart": req.apply_restart,
                "restarted": restarted,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(ApplyTuningResponse {
        version: saved.version,
        applied,
        config: normalized_val,
        restarted,
        restart_error,
    }))
}
