use crate::api::error::ApiError;
use crate::server::PanelState;
use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::header::COOKIE;
use axum::http::request::Parts;
use hameta_common::hash_token;
use hameta_storage::{AdminSessionRow, AdminUserRow};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SESSION_COOKIE_NAME: &str = "hameta_session";
pub const SESSION_TTL_SECS: i64 = 7 * 24 * 3600;
const LOGIN_FAIL_WINDOW: Duration = Duration::from_secs(300);
const MAX_LOGIN_FAILURES: usize = 5;

/// 按来源 IP 统计 5 分钟内连续失败登录次数，防范密码爆破。
#[derive(Debug, Default)]
pub struct LoginRateLimiter {
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl LoginRateLimiter {
    pub fn check_limited(&self, client_ip: &str) -> Option<u64> {
        let mut guard = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let queue = guard.get_mut(client_ip)?;
        while queue
            .front()
            .is_some_and(|ts| now.duration_since(*ts) > LOGIN_FAIL_WINDOW)
        {
            queue.pop_front();
        }
        if queue.len() >= MAX_LOGIN_FAILURES
            && let Some(oldest) = queue.front()
        {
            let elapsed = now.duration_since(*oldest);
            let wait = LOGIN_FAIL_WINDOW.saturating_sub(elapsed).as_secs().max(1);
            return Some(wait);
        }
        None
    }

    pub fn record_failure(&self, client_ip: &str) {
        let mut guard = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let queue = guard.entry(client_ip.to_string()).or_default();
        while queue
            .front()
            .is_some_and(|ts| now.duration_since(*ts) > LOGIN_FAIL_WINDOW)
        {
            queue.pop_front();
        }
        queue.push_back(now);
    }

    pub fn clear(&self, client_ip: &str) {
        let mut guard = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.remove(client_ip);
    }
}

pub fn extract_session_token(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let trimmed = part.trim();
        if let Some((name, val)) = trimmed.split_once('=')
            && name.trim() == SESSION_COOKIE_NAME
        {
            let token = val.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

pub fn build_session_cookie(raw_token: &str, max_age_secs: i64) -> String {
    format!(
        "{SESSION_COOKIE_NAME}={raw_token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age_secs}"
    )
}

pub fn build_clear_session_cookie() -> String {
    format!("{SESSION_COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}

/// 已通过会话 Cookie 校验的当前登录管理员提取器。
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: AdminUserRow,
    pub session: AdminSessionRow,
    pub token_hash: String,
}

impl FromRequestParts<Arc<PanelState>> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<PanelState>,
    ) -> Result<Self, Self::Rejection> {
        let initialized = state
            .db
            .is_initialized()
            .await
            .map_err(|_| ApiError::internal("读取初始化状态失败"))?;
        if !initialized {
            return Err(ApiError::unauthorized("系统尚未初始化"));
        }

        let Some(raw_token) = extract_session_token(&parts.headers) else {
            return Err(ApiError::unauthorized("请先登录管理员后台"));
        };
        let token_hash = hash_token(&raw_token);
        let Some(session) = state
            .db
            .find_valid_session(&token_hash)
            .await
            .map_err(|_| ApiError::internal("验证登录会话失败"))?
        else {
            return Err(ApiError::unauthorized("登录会话已过期或已注销，请重新登录"));
        };

        let Some(user) = state
            .db
            .find_admin_by_id(session.user_id)
            .await
            .map_err(|_| ApiError::internal("读取管理员信息失败"))?
        else {
            return Err(ApiError::unauthorized("关联管理员账号不存在"));
        };

        let _ = state.db.touch_session(&token_hash).await;
        Ok(AuthUser {
            user,
            session,
            token_hash,
        })
    }
}
