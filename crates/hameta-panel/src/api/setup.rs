use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::{SESSION_TTL_SECS, build_session_cookie};
use crate::server::PanelState;
use crate::services::{
    InitTableError, ManticoreTestReport, RedisTestReport, init_manticore_table_if_absent,
    test_manticore_connection, test_redis_connection,
};
use axum::Json;
use axum::extract::{FromRequestParts, State};
use axum::http::header::{SET_COOKIE, USER_AGENT};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use hameta_common::{
    CrawlerWorkerConfig, IndexerWorkerConfig, ManticoreServiceConfig, RedisServiceConfig,
    ServiceKind, WorkerRole, crawl_profile_defaults, generate_token, hash_password, hash_token,
};
use hameta_filter::{
    CompiledFilterEngine, FilterBundle, default_online_filter_bundle, parse_legacy_filter_json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct SetupStatusResponse {
    pub initialized: bool,
    pub has_admin: bool,
    pub has_redis: bool,
    pub has_manticore: bool,
    pub version: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct SetupTestRedisRequest {
    pub redis: RedisServiceConfig,
}

#[derive(Debug, Deserialize)]
pub struct SetupTestManticoreRequest {
    pub manticore: ManticoreServiceConfig,
}

#[derive(Debug, Deserialize)]
pub struct SetupInitRequest {
    pub admin_username: String,
    pub admin_password: String,
    pub redis: RedisServiceConfig,
    pub manticore: ManticoreServiceConfig,
    #[serde(default)]
    pub legacy_filter_json: Option<String>,
    #[serde(default)]
    pub filter_bundle: Option<FilterBundle>,
    #[serde(default = "default_true")]
    pub create_default_workers: bool,
    #[serde(default)]
    pub crawler_port: Option<u16>,
    #[serde(default)]
    pub crawler_profile: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SetupInitResponse {
    pub initialized: bool,
    pub username: String,
    pub filter_version: i64,
    pub created_workers: Vec<String>,
}

pub struct RequireUninitialized;

impl FromRequestParts<Arc<PanelState>> for RequireUninitialized {
    type Rejection = ApiError;

    async fn from_request_parts(
        _parts: &mut Parts,
        state: &Arc<PanelState>,
    ) -> Result<Self, Self::Rejection> {
        if state.db.is_initialized().await? {
            return Err(ApiError::forbidden(
                "ALREADY_INITIALIZED",
                "系统已完成初始化，初始化向导已锁定，请登录后台运维面板",
            ));
        }
        Ok(Self)
    }
}

pub async fn setup_status_handler(
    State(state): State<Arc<PanelState>>,
) -> Result<Json<SetupStatusResponse>, ApiError> {
    let initialized = state.db.is_initialized().await?;
    let has_admin = state.db.get_primary_admin().await?.is_some();
    let has_redis = state
        .db
        .get_active_service_config(ServiceKind::Redis)
        .await?
        .is_some();
    let has_manticore = state
        .db
        .get_active_service_config(ServiceKind::Manticore)
        .await?
        .is_some();

    Ok(Json(SetupStatusResponse {
        initialized,
        has_admin,
        has_redis,
        has_manticore,
        version: env!("CARGO_PKG_VERSION"),
    }))
}

pub async fn setup_test_redis_handler(
    _: RequireUninitialized,
    Json(req): Json<SetupTestRedisRequest>,
) -> Result<Json<RedisTestReport>, ApiError> {
    Ok(Json(test_redis_connection(&req.redis).await))
}

pub async fn setup_test_manticore_handler(
    _: RequireUninitialized,
    Json(req): Json<SetupTestManticoreRequest>,
) -> Result<Json<ManticoreTestReport>, ApiError> {
    Ok(Json(test_manticore_connection(&req.manticore).await))
}

pub async fn setup_init_manticore_table_handler(
    _: RequireUninitialized,
    Json(req): Json<SetupTestManticoreRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match init_manticore_table_if_absent(&req.manticore).await {
        Ok(()) => Ok(Json(serde_json::json!({
            "ok": true,
            "message": "已在 Manticore 空库成功初始化 torrents 表"
        }))),
        Err(InitTableError::InvalidConfig(msg)) => {
            Err(ApiError::bad_request("INVALID_CONFIG", msg))
        }
        Err(InitTableError::AlreadyExists) => Err(ApiError::conflict(
            "TABLE_ALREADY_EXISTS",
            "Manticore 已存在 torrents 表，严禁重复创建或覆盖存量结构",
        )),
        Err(InitTableError::Backend(msg)) => Err(ApiError::bad_request("MANTICORE_ERROR", msg)),
    }
}

pub async fn setup_init_handler(
    _: RequireUninitialized,
    State(state): State<Arc<PanelState>>,
    headers: HeaderMap,
    Json(req): Json<SetupInitRequest>,
) -> Result<Response, ApiError> {
    let _guard = state.setup_lock.lock().await;
    if state.db.is_initialized().await? {
        return Err(ApiError::forbidden(
            "ALREADY_INITIALIZED",
            "系统已完成初始化，初始化向导已锁定，请登录后台运维面板",
        ));
    }

    let username = req.admin_username.trim();
    if username.chars().count() < 2 {
        return Err(ApiError::bad_request(
            "INVALID_USERNAME",
            "管理员用户名至少需要 2 个字符",
        ));
    }
    if req.admin_password.chars().count() < 6 {
        return Err(ApiError::bad_request(
            "WEAK_PASSWORD",
            "管理员密码长度至少需要 6 位",
        ));
    }

    req.redis.validate()?;
    req.manticore.validate()?;

    let mut bundle = req
        .filter_bundle
        .unwrap_or_else(default_online_filter_bundle);
    if let Some(raw_legacy) = req.legacy_filter_json.as_deref()
        && !raw_legacy.trim().is_empty()
    {
        bundle.policy = parse_legacy_filter_json(raw_legacy)?;
    }
    CompiledFilterEngine::compile(&bundle)?;

    let mut crawler_cfg = CrawlerWorkerConfig::default();
    if let Some(port) = req.crawler_port {
        if port == 0 {
            return Err(ApiError::bad_request(
                "INVALID_CONFIG",
                "默认爬虫 UDP 监听端口不能为 0",
            ));
        }
        crawler_cfg.port = port;
    }
    if let Some(profile) = req.crawler_profile.as_deref() {
        let trimmed = profile.trim();
        if !trimmed.is_empty() {
            if crawl_profile_defaults(trimmed).is_none() {
                return Err(ApiError::bad_request(
                    "INVALID_PROFILE",
                    format!("未知的采集档位: {trimmed}"),
                ));
            }
            crawler_cfg.crawl_profile = trimmed.to_string();
        }
    }
    crawler_cfg.validate()?;

    let redis_json = serde_json::to_string(&req.redis)
        .map_err(|e| ApiError::internal("序列化 Redis 配置失败").with_detail(e.to_string()))?;
    let manticore_json = serde_json::to_string(&req.manticore)
        .map_err(|e| ApiError::internal("序列化 Manticore 配置失败").with_detail(e.to_string()))?;
    let filter_rules_json = serde_json::to_string(&bundle.policy)
        .map_err(|e| ApiError::internal("序列化过滤规则失败").with_detail(e.to_string()))?;
    let ad_phrases_json = serde_json::to_string(&bundle.ad_phrases)
        .map_err(|e| ApiError::internal("序列化广告短语失败").with_detail(e.to_string()))?;
    let ad_regexes_json = serde_json::to_string(&bundle.ad_regexes)
        .map_err(|e| ApiError::internal("序列化广告正则失败").with_detail(e.to_string()))?;

    let password_hash = hash_password(&req.admin_password)?;
    let admin = state
        .db
        .reset_admin_credentials(username, &password_hash, "setup")
        .await?;

    state
        .db
        .save_service_config(ServiceKind::Redis, &redis_json, username, "初始化配置")
        .await?;
    state
        .db
        .save_service_config(
            ServiceKind::Manticore,
            &manticore_json,
            username,
            "初始化配置",
        )
        .await?;

    state
        .db
        .save_filter_draft(
            &filter_rules_json,
            &ad_phrases_json,
            &ad_regexes_json,
            username,
            "初始线上规则预设",
        )
        .await?;
    let published_filter = state
        .db
        .publish_filter_draft(username, Some("初始化发布线上过滤基线"))
        .await?;

    let mut created_workers = Vec::new();
    if req.create_default_workers {
        if state.db.get_worker("crawler-1").await?.is_none() {
            let crawler_json = serde_json::to_string(&crawler_cfg).map_err(|e| {
                ApiError::internal("序列化爬虫默认配置失败").with_detail(e.to_string())
            })?;
            state
                .db
                .create_worker(
                    "crawler-1",
                    WorkerRole::Crawler,
                    "默认 DHT 采集节点 #1",
                    &crawler_json,
                    username,
                )
                .await?;
            created_workers.push("crawler-1".to_string());
        }
        if state.db.get_worker("indexer-1").await?.is_none() {
            let indexer_json =
                serde_json::to_string(&IndexerWorkerConfig::default()).map_err(|e| {
                    ApiError::internal("序列化索引默认配置失败").with_detail(e.to_string())
                })?;
            state
                .db
                .create_worker(
                    "indexer-1",
                    WorkerRole::Indexer,
                    "默认索引入库节点 #1",
                    &indexer_json,
                    username,
                )
                .await?;
            created_workers.push("indexer-1".to_string());
        }
    }

    state.db.mark_initialized().await?;
    state.services.init_from_db(&state.db).await;

    let client_ip = extract_client_ip(&headers);
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let raw_token = generate_token();
    let token_hash = hash_token(&raw_token);
    state
        .db
        .create_session(
            &token_hash,
            admin.id,
            &client_ip,
            user_agent,
            SESSION_TTL_SECS,
        )
        .await?;

    let audit_detail = serde_json::json!({
        "username": admin.username,
        "redis_host": req.redis.host,
        "redis_port": req.redis.port,
        "manticore_url": req.manticore.url,
        "filter_version": published_filter.version,
        "created_workers": created_workers,
    })
    .to_string();
    let _ = state
        .db
        .record_audit(
            &admin.username,
            "setup.init",
            "system",
            &audit_detail,
            &client_ip,
        )
        .await;

    let mut resp = (
        StatusCode::OK,
        Json(SetupInitResponse {
            initialized: true,
            username: admin.username,
            filter_version: published_filter.version,
            created_workers,
        }),
    )
        .into_response();

    if let Ok(cookie) = HeaderValue::from_str(&build_session_cookie(&raw_token, SESSION_TTL_SECS)) {
        resp.headers_mut().insert(SET_COOKIE, cookie);
    }

    Ok(resp)
}
