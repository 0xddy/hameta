use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::server::PanelState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use hameta_common::{DEFAULT_SQL_LIMIT, MAX_SQL_LIMIT, prepare_console_sql};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_QUERY_TIMEOUT_SECS: u64 = 60;
const DEFAULT_HEAVY_DDL_TIMEOUT_SECS: u64 = 600;
const MIN_SQL_TIMEOUT_SECS: u64 = 5;
const MAX_SQL_TIMEOUT_SECS: u64 = 1_800;

#[derive(Debug, Deserialize)]
pub struct SqlQueryRequest {
    pub sql: String,
    pub max_rows: Option<usize>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct SqlQueryResponse {
    pub executed_sql: String,
    pub columns: Vec<String>,
    pub rows: Vec<Value>,
    pub row_count: usize,
    pub total: Option<u64>,
    pub warning: Option<String>,
    pub took_ms: u64,
}

fn sql_first_keyword(sql: &str) -> String {
    sql.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
}

fn is_heavy_ddl_keyword(keyword: &str) -> bool {
    matches!(
        keyword,
        "ALTER"
            | "OPTIMIZE"
            | "FLUSH"
            | "TRUNCATE"
            | "RELOAD"
            | "CREATE"
            | "DROP"
            | "ATTACH"
            | "IMPORT"
            | "FREEZE"
            | "UNFREEZE"
    )
}

fn is_schema_mutating_keyword(keyword: &str) -> bool {
    matches!(keyword, "ALTER" | "CREATE" | "DROP")
}

fn resolve_sql_timeout_secs(first_keyword: &str, requested: Option<u64>) -> u64 {
    match requested {
        Some(secs) => secs.clamp(MIN_SQL_TIMEOUT_SECS, MAX_SQL_TIMEOUT_SECS),
        None if is_heavy_ddl_keyword(first_keyword) => DEFAULT_HEAVY_DDL_TIMEOUT_SECS,
        None => DEFAULT_QUERY_TIMEOUT_SECS,
    }
}

fn error_is_timeout(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout)
    }) || err.to_string().to_ascii_lowercase().contains("timed out")
}

pub async fn execute_readonly_sql_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<SqlQueryRequest>,
) -> Result<Json<SqlQueryResponse>, ApiError> {
    let default_limit = req
        .max_rows
        .unwrap_or(DEFAULT_SQL_LIMIT)
        .clamp(1, MAX_SQL_LIMIT);
    let safe_sql = prepare_console_sql(&req.sql, default_limit, MAX_SQL_LIMIT)?;
    let first_word = sql_first_keyword(&safe_sql);
    let timeout_secs = resolve_sql_timeout_secs(&first_word, req.timeout_secs);

    let Some((_, client)) = state.services.manticore_client().await else {
        return Err(ApiError::bad_request(
            "MANTICORE_NOT_CONFIGURED",
            "尚未配置或连接 Manticore 服务",
        ));
    };

    let started = Instant::now();
    let results = match client
        .execute_raw_sql_with_timeout(&safe_sql, Duration::from_secs(timeout_secs))
        .await
    {
        Ok(res) => res,
        Err(err) => {
            // 即使超时断开，Manticore 后台仍可能完成 DDL，预先失效表结构缓存以免后续漏检新列
            if is_schema_mutating_keyword(&first_word) {
                client.invalidate_schema_cache();
            }
            if error_is_timeout(&err) {
                let elapsed_secs = started.elapsed().as_secs();
                let msg = if is_heavy_ddl_keyword(&first_word) {
                    format!(
                        "等待 Manticore 响应超时（已等待 {elapsed_secs}s，超时上限 {timeout_secs}s）。注意：{first_word} 等耗时操作在 Manticore 服务端通常仍在后台继续执行并最终成功，请勿重复提交，稍后执行 DESCRIBE torrents; 或 SHOW THREADS; 确认结果。"
                    )
                } else {
                    format!(
                        "Manticore SQL 查询等待响应超时（已等待 {elapsed_secs}s，超时上限 {timeout_secs}s），可调大超时上限或缩小查询范围后重试。"
                    )
                };
                return Err(ApiError::gateway_timeout("MANTICORE_SQL_TIMEOUT", msg));
            }
            return Err(ApiError::bad_request(
                "MANTICORE_SQL_ERROR",
                format!("{err:#}"),
            ));
        }
    };
    let took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(0);

    // 若执行了结构变更（ALTER / CREATE / DROP），立即刷新 Manticore 表结构缓存（如 total_size 列检测）
    if is_schema_mutating_keyword(&first_word) {
        client.invalidate_schema_cache();
    }

    let first = results.first().cloned().unwrap_or(Value::Null);
    let mut rows: Vec<Value> = first
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for row in &mut rows {
        stringify_unsafe_js_integers(row);
    }

    let mut columns = Vec::new();
    if let Some(cols) = first.get("columns").and_then(Value::as_array) {
        for col_obj in cols {
            if let Some(map) = col_obj.as_object()
                && let Some(first_key) = map.keys().next()
            {
                columns.push(first_key.clone());
            }
        }
    }
    if columns.is_empty()
        && let Some(first_row) = rows.first()
        && let Some(obj) = first_row.as_object()
    {
        columns = obj.keys().cloned().collect();
    }

    let total = first.get("total").and_then(Value::as_u64);
    let warning = first
        .get("warning")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let row_count = rows.len();

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "sql.query",
            "manticore",
            &json!({
                "executed_sql": &safe_sql,
                "row_count": row_count,
                "took_ms": took_ms,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(SqlQueryResponse {
        executed_sql: safe_sql,
        columns,
        rows,
        row_count,
        total,
        warning,
        took_ms,
    }))
}

const JS_MAX_SAFE_INTEGER_I64: i64 = 9_007_199_254_740_991;
const JS_MAX_SAFE_INTEGER_U64: u64 = 9_007_199_254_740_991;

/// 将超出 JavaScript `Number.MAX_SAFE_INTEGER` (2^53 - 1) 的 64 位整数转为十进制字符串，避免前端 JSON.parse 丢精度。
fn stringify_unsafe_js_integers(val: &mut Value) {
    match val {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if i.abs() > JS_MAX_SAFE_INTEGER_I64 {
                    *val = Value::String(i.to_string());
                }
            } else if let Some(u) = n.as_u64()
                && u > JS_MAX_SAFE_INTEGER_U64
            {
                *val = Value::String(u.to_string());
            }
        }
        Value::Array(arr) => {
            for item in arr {
                stringify_unsafe_js_integers(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                stringify_unsafe_js_integers(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_sql_timeout_grants_extended_window_for_ddl_and_clamps_custom() {
        assert_eq!(
            resolve_sql_timeout_secs("ALTER", None),
            DEFAULT_HEAVY_DDL_TIMEOUT_SECS
        );
        assert_eq!(
            resolve_sql_timeout_secs("OPTIMIZE", None),
            DEFAULT_HEAVY_DDL_TIMEOUT_SECS
        );
        assert_eq!(
            resolve_sql_timeout_secs("SELECT", None),
            DEFAULT_QUERY_TIMEOUT_SECS
        );
        assert_eq!(
            resolve_sql_timeout_secs("SELECT", Some(1)),
            MIN_SQL_TIMEOUT_SECS
        );
        assert_eq!(
            resolve_sql_timeout_secs("ALTER", Some(9999)),
            MAX_SQL_TIMEOUT_SECS
        );
        assert_eq!(resolve_sql_timeout_secs("ALTER", Some(300)), 300);
    }
}
