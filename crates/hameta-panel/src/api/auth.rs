use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::{
    AuthUser, SESSION_TTL_SECS, build_clear_session_cookie, build_session_cookie,
    extract_session_token,
};
use crate::server::PanelState;
use axum::Json;
use axum::extract::State;
use axum::http::header::{SET_COOKIE, USER_AGENT};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use hameta_common::{generate_token, hash_password, hash_token, verify_password};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthUserInfo {
    pub id: i64,
    pub username: String,
    pub last_login_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct SessionSummary {
    pub client_ip: String,
    pub user_agent: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Serialize)]
pub struct AuthMeResponse {
    pub user: AuthUserInfo,
    pub session: SessionSummary,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

pub async fn login_handler(
    State(state): State<Arc<PanelState>>,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    if !state.db.is_initialized().await? {
        return Err(ApiError::forbidden(
            "NOT_INITIALIZED",
            "系统尚未初始化，请先访问引导向导完成配置",
        ));
    }

    let client_ip = extract_client_ip(&headers);
    if let Some(wait_secs) = state.login_limiter.check_limited(&client_ip) {
        return Err(ApiError::too_many_requests(
            format!("登录失败次数过多，请在 {wait_secs} 秒后重试"),
            wait_secs,
        ));
    }

    let username = req.username.trim();
    let Some(user) = state.db.find_admin_by_username(username).await? else {
        state.login_limiter.record_failure(&client_ip);
        return Err(ApiError::unauthorized("用户名或密码错误"));
    };

    if !verify_password(&req.password, &user.password_hash) {
        state.login_limiter.record_failure(&client_ip);
        return Err(ApiError::unauthorized("用户名或密码错误"));
    }

    state.login_limiter.clear(&client_ip);
    let _ = state.db.update_last_login(user.id).await;

    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let raw_token = generate_token();
    let token_hash = hash_token(&raw_token);
    let session = state
        .db
        .create_session(
            &token_hash,
            user.id,
            &client_ip,
            user_agent,
            SESSION_TTL_SECS,
        )
        .await?;

    let _ = state
        .db
        .record_audit(
            &user.username,
            "auth.login",
            &user.username,
            "{}",
            &client_ip,
        )
        .await;

    let mut resp = (
        StatusCode::OK,
        Json(AuthMeResponse {
            user: AuthUserInfo {
                id: user.id,
                username: user.username,
                last_login_at: user.last_login_at,
                created_at: user.created_at,
            },
            session: SessionSummary {
                client_ip: session.client_ip,
                user_agent: session.user_agent,
                created_at: session.created_at,
                expires_at: session.expires_at,
            },
        }),
    )
        .into_response();

    if let Ok(cookie) = HeaderValue::from_str(&build_session_cookie(&raw_token, SESSION_TTL_SECS)) {
        resp.headers_mut().insert(SET_COOKIE, cookie);
    }
    Ok(resp)
}

pub async fn logout_handler(
    State(state): State<Arc<PanelState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(raw_token) = extract_session_token(&headers) {
        let token_hash = hash_token(&raw_token);
        let _ = state.db.delete_session(&token_hash).await;
    }

    let mut resp = (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&build_clear_session_cookie()) {
        resp.headers_mut().insert(SET_COOKIE, cookie);
    }
    Ok(resp)
}

pub async fn me_handler(auth: AuthUser) -> Json<AuthMeResponse> {
    Json(AuthMeResponse {
        user: AuthUserInfo {
            id: auth.user.id,
            username: auth.user.username,
            last_login_at: auth.user.last_login_at,
            created_at: auth.user.created_at,
        },
        session: SessionSummary {
            client_ip: auth.session.client_ip,
            user_agent: auth.session.user_agent,
            created_at: auth.session.created_at,
            expires_at: auth.session.expires_at,
        },
    })
}

pub async fn change_password_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Response, ApiError> {
    if !verify_password(&req.current_password, &auth.user.password_hash) {
        return Err(ApiError::bad_request(
            "INVALID_PASSWORD",
            "当前原密码验证失败",
        ));
    }
    if req.new_password.chars().count() < 6 {
        return Err(ApiError::bad_request(
            "WEAK_PASSWORD",
            "新密码长度至少需要 6 位",
        ));
    }

    let new_hash = hash_password(&req.new_password)?;
    state
        .db
        .update_admin_password(auth.user.id, &new_hash)
        .await?;
    state.db.delete_all_sessions().await?;

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
            auth.user.id,
            &client_ip,
            user_agent,
            SESSION_TTL_SECS,
        )
        .await?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "auth.password_change",
            &auth.user.username,
            "{}",
            &client_ip,
        )
        .await;

    let mut resp = (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "message": "管理员密码已更新，其余会话已失效"
        })),
    )
        .into_response();

    if let Ok(cookie) = HeaderValue::from_str(&build_session_cookie(&raw_token, SESSION_TTL_SECS)) {
        resp.headers_mut().insert(SET_COOKIE, cookie);
    }
    Ok(resp)
}
