use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::server::PanelState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use hameta_common::{DEFAULT_SQL_LIMIT, MAX_SQL_LIMIT, validate_readonly_sql};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
pub struct SqlQueryRequest {
    pub sql: String,
    pub max_rows: Option<usize>,
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
    let safe_sql = validate_readonly_sql(&req.sql, default_limit, MAX_SQL_LIMIT)?;

    let Some((_, client)) = state.services.manticore_client().await else {
        return Err(ApiError::bad_request(
            "MANTICORE_NOT_CONFIGURED",
            "尚未配置或连接 Manticore 服务",
        ));
    };

    let started = Instant::now();
    let results = client
        .execute_raw_sql_with_timeout(&safe_sql, Duration::from_secs(10))
        .await
        .map_err(|e| ApiError::bad_request("MANTICORE_SQL_ERROR", e.to_string()))?;
    let took_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(0);

    let first = results.first().cloned().unwrap_or(Value::Null);
    let rows: Vec<Value> = first
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

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
