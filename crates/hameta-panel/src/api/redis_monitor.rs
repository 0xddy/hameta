use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::server::PanelState;
use crate::services::sanitize_secret_in_text;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use deadpool_redis::Pool;
use hameta_common::{RedisServiceConfig, hash_to_id, normalize_info_hash};
use redis::Value as RedisValue;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Serialize)]
pub struct RedisPoolStatusDto {
    pub max_size: usize,
    pub size: usize,
    pub available: usize,
    pub waiting: usize,
}

#[derive(Debug, Serialize)]
pub struct RedisConfigSummaryDto {
    pub torrent_stream: String,
    pub hot_stream: String,
    pub consumer_group: String,
    pub indexed_key_prefix: String,
    pub max_stream_len: usize,
    pub max_message_size: usize,
}

#[derive(Debug, Serialize)]
pub struct RedisStreamConsumerDto {
    pub name: String,
    pub pending: i64,
    pub idle_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct RedisStreamGroupDto {
    pub name: String,
    pub consumers: i64,
    pub pending: i64,
    pub lag: Option<i64>,
    pub last_delivered_id: Option<String>,
    pub entries_read: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RedisStreamOverviewDto {
    pub role: &'static str,
    pub stream_name: String,
    pub exists: bool,
    pub length: i64,
    pub entries_added: Option<i64>,
    pub first_entry_id: Option<String>,
    pub last_entry_id: Option<String>,
    pub groups: Vec<RedisStreamGroupDto>,
    pub target_group_consumers: Vec<RedisStreamConsumerDto>,
}

#[derive(Debug, Serialize)]
pub struct RedisOverviewResponse {
    pub ok: bool,
    pub message: Option<String>,
    pub latency_ms: u64,
    pub redis_version: Option<String>,
    pub uptime_secs: Option<u64>,
    pub used_memory_bytes: Option<u64>,
    pub used_memory_human: Option<String>,
    pub used_memory_peak_human: Option<String>,
    pub connected_clients: Option<u64>,
    pub instantaneous_ops_per_sec: Option<u64>,
    pub dbsize: Option<u64>,
    pub pool: RedisPoolStatusDto,
    pub config_summary: RedisConfigSummaryDto,
    pub streams: Vec<RedisStreamOverviewDto>,
}

#[derive(Debug, Deserialize)]
pub struct PreviewStreamQuery {
    pub count: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct StreamEntryPreviewDto {
    pub entry_id: String,
    pub info_hash: Option<String>,
    pub timestamp: Option<String>,
    pub name: Option<String>,
    pub total_size: Option<u64>,
    pub file_count: Option<usize>,
    pub payload_bytes: usize,
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct PreviewStreamResponse {
    pub stream_name: String,
    pub role: &'static str,
    pub total_len: i64,
    pub items: Vec<StreamEntryPreviewDto>,
}

#[derive(Debug, Deserialize)]
pub struct TrimStreamRequest {
    pub max_len: u64,
    #[serde(default = "default_true")]
    pub approximate: bool,
    pub confirm_stream_name: String,
}

#[derive(Debug, Serialize)]
pub struct TrimStreamResponse {
    pub ok: bool,
    pub stream_name: String,
    pub trimmed_count: i64,
    pub remaining_len: i64,
}

#[derive(Debug, Serialize)]
pub struct RedisKeyStateDto {
    pub key: String,
    pub exists: bool,
    pub value: Option<String>,
    pub pttl_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct RedisHashInspectResponse {
    pub info_hash: String,
    pub expected_id: i64,
    pub indexed: RedisKeyStateDto,
    pub pending: RedisKeyStateDto,
    pub rejected: RedisKeyStateDto,
    pub manticore_checked: bool,
    pub manticore_exists: bool,
    pub manticore_raw_name: Option<String>,
    pub manticore_refine_name: Option<String>,
    pub manticore_hot: Option<i32>,
    pub manticore_updated_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ClearHashLocksQuery {
    pub clear_pending: Option<bool>,
    pub clear_rejected: Option<bool>,
    pub clear_indexed: Option<bool>,
}

fn default_true() -> bool {
    true
}

async fn require_redis(state: &PanelState) -> Result<(RedisServiceConfig, Pool), ApiError> {
    let Some(pair) = state.services.redis_pool().await else {
        return Err(ApiError::bad_request(
            "REDIS_NOT_CONFIGURED",
            "尚未配置或连接 Redis 服务",
        ));
    };
    Ok(pair)
}

fn resolve_stream_name<'a>(
    cfg: &'a RedisServiceConfig,
    raw_name: &str,
) -> Result<(&'static str, &'a str), ApiError> {
    let trimmed = raw_name.trim();
    if trimmed == "new" || trimmed == cfg.torrent_stream {
        return Ok(("new", cfg.torrent_stream.as_str()));
    }
    if trimmed == "hot" || trimmed == cfg.hot_stream {
        return Ok(("hot", cfg.hot_stream.as_str()));
    }
    Err(ApiError::bad_request(
        "INVALID_STREAM",
        format!("仅支持受管 Stream（new / hot 或完整配置键名），收到: {trimmed}"),
    ))
}

fn redis_val_to_str(val: &RedisValue) -> Option<String> {
    match val {
        RedisValue::BulkString(b) => Some(String::from_utf8_lossy(b).into_owned()),
        RedisValue::SimpleString(s) => Some(s.clone()),
        RedisValue::VerbatimString { text, .. } => Some(text.clone()),
        RedisValue::Int(n) => Some(n.to_string()),
        RedisValue::Double(f) => Some(f.to_string()),
        RedisValue::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

fn redis_val_to_i64(val: &RedisValue) -> Option<i64> {
    match val {
        RedisValue::Int(n) => Some(*n),
        RedisValue::BulkString(b) => String::from_utf8_lossy(b).trim().parse::<i64>().ok(),
        RedisValue::SimpleString(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn redis_val_to_map(val: &RedisValue) -> HashMap<String, RedisValue> {
    let mut map = HashMap::new();
    match val {
        RedisValue::Map(pairs) => {
            for (k, v) in pairs {
                if let Some(ks) = redis_val_to_str(k) {
                    map.insert(ks, v.clone());
                }
            }
        }
        RedisValue::Array(items) => {
            for pair in items.as_chunks::<2>().0 {
                if let Some(ks) = redis_val_to_str(&pair[0]) {
                    map.insert(ks, pair[1].clone());
                }
            }
        }
        _ => {}
    }
    map
}

fn extract_entry_id_from_xinfo_entry(val: &RedisValue) -> Option<String> {
    match val {
        RedisValue::Array(arr) if !arr.is_empty() => redis_val_to_str(&arr[0]),
        _ => None,
    }
}

fn extract_info_field(info: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    info.lines().find_map(|line| {
        line.trim()
            .strip_prefix(&prefix)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

async fn inspect_single_stream(
    conn: &mut deadpool_redis::Connection,
    role: &'static str,
    stream_name: &str,
    target_group: &str,
) -> RedisStreamOverviewDto {
    let exists_raw: i64 = redis::cmd("EXISTS")
        .arg(stream_name)
        .query_async(conn)
        .await
        .unwrap_or(0);
    if exists_raw == 0 {
        return RedisStreamOverviewDto {
            role,
            stream_name: stream_name.to_string(),
            exists: false,
            length: 0,
            entries_added: None,
            first_entry_id: None,
            last_entry_id: None,
            groups: Vec::new(),
            target_group_consumers: Vec::new(),
        };
    }

    let length: i64 = redis::cmd("XLEN")
        .arg(stream_name)
        .query_async(conn)
        .await
        .unwrap_or(0);

    let mut entries_added = None;
    let mut first_entry_id = None;
    let mut last_entry_id = None;

    if let Ok(xinfo_val) = redis::cmd("XINFO")
        .arg("STREAM")
        .arg(stream_name)
        .query_async::<RedisValue>(conn)
        .await
    {
        let map = redis_val_to_map(&xinfo_val);
        entries_added = map.get("entries-added").and_then(redis_val_to_i64);
        first_entry_id = map
            .get("first-entry")
            .and_then(extract_entry_id_from_xinfo_entry);
        last_entry_id = map
            .get("last-entry")
            .and_then(extract_entry_id_from_xinfo_entry)
            .or_else(|| map.get("last-generated-id").and_then(redis_val_to_str));
    }

    let mut groups = Vec::new();
    if let Ok(RedisValue::Array(items)) = redis::cmd("XINFO")
        .arg("GROUPS")
        .arg(stream_name)
        .query_async::<RedisValue>(conn)
        .await
    {
        for item in items {
            let map = redis_val_to_map(&item);
            let Some(name) = map.get("name").and_then(redis_val_to_str) else {
                continue;
            };
            groups.push(RedisStreamGroupDto {
                name,
                consumers: map.get("consumers").and_then(redis_val_to_i64).unwrap_or(0),
                pending: map.get("pending").and_then(redis_val_to_i64).unwrap_or(0),
                lag: map.get("lag").and_then(redis_val_to_i64),
                last_delivered_id: map.get("last-delivered-id").and_then(redis_val_to_str),
                entries_read: map.get("entries-read").and_then(redis_val_to_i64),
            });
        }
    }

    let mut target_group_consumers = Vec::new();
    if let Ok(RedisValue::Array(items)) = redis::cmd("XINFO")
        .arg("CONSUMERS")
        .arg(stream_name)
        .arg(target_group)
        .query_async::<RedisValue>(conn)
        .await
    {
        for item in items {
            let map = redis_val_to_map(&item);
            let Some(name) = map.get("name").and_then(redis_val_to_str) else {
                continue;
            };
            target_group_consumers.push(RedisStreamConsumerDto {
                name,
                pending: map.get("pending").and_then(redis_val_to_i64).unwrap_or(0),
                idle_ms: map
                    .get("idle")
                    .or_else(|| map.get("inactive"))
                    .and_then(redis_val_to_i64)
                    .unwrap_or(0),
            });
        }
    }

    RedisStreamOverviewDto {
        role,
        stream_name: stream_name.to_string(),
        exists: true,
        length,
        entries_added,
        first_entry_id,
        last_entry_id,
        groups,
        target_group_consumers,
    }
}

pub async fn redis_overview_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Result<Json<RedisOverviewResponse>, ApiError> {
    let (cfg, pool) = require_redis(&state).await?;
    let pool_st = pool.status();
    let pool_dto = RedisPoolStatusDto {
        max_size: pool_st.max_size,
        size: pool_st.size,
        available: pool_st.available,
        waiting: pool_st.waiting,
    };
    let config_summary = RedisConfigSummaryDto {
        torrent_stream: cfg.torrent_stream.clone(),
        hot_stream: cfg.hot_stream.clone(),
        consumer_group: cfg.consumer_group.clone(),
        indexed_key_prefix: cfg.indexed_key_prefix.clone(),
        max_stream_len: cfg.max_stream_len,
        max_message_size: cfg.max_message_size,
    };

    let mut conn = match pool.get().await {
        Ok(c) => c,
        Err(err) => {
            return Ok(Json(RedisOverviewResponse {
                ok: false,
                message: Some(sanitize_secret_in_text(&format!("获取连接失败: {err}"))),
                latency_ms: 0,
                redis_version: None,
                uptime_secs: None,
                used_memory_bytes: None,
                used_memory_human: None,
                used_memory_peak_human: None,
                connected_clients: None,
                instantaneous_ops_per_sec: None,
                dbsize: None,
                pool: pool_dto,
                config_summary,
                streams: Vec::new(),
            }));
        }
    };

    let ping_start = Instant::now();
    let ping_res: Result<String, _> = redis::cmd("PING").query_async(&mut conn).await;
    let latency_ms = u64::try_from(ping_start.elapsed().as_millis()).unwrap_or(0);
    if let Err(err) = ping_res {
        return Ok(Json(RedisOverviewResponse {
            ok: false,
            message: Some(sanitize_secret_in_text(&format!("Redis PING 失败: {err}"))),
            latency_ms,
            redis_version: None,
            uptime_secs: None,
            used_memory_bytes: None,
            used_memory_human: None,
            used_memory_peak_human: None,
            connected_clients: None,
            instantaneous_ops_per_sec: None,
            dbsize: None,
            pool: pool_dto,
            config_summary,
            streams: Vec::new(),
        }));
    }

    let info_server: String = redis::cmd("INFO")
        .arg("server")
        .query_async(&mut conn)
        .await
        .unwrap_or_default();
    let info_memory: String = redis::cmd("INFO")
        .arg("memory")
        .query_async(&mut conn)
        .await
        .unwrap_or_default();
    let info_clients: String = redis::cmd("INFO")
        .arg("clients")
        .query_async(&mut conn)
        .await
        .unwrap_or_default();
    let info_stats: String = redis::cmd("INFO")
        .arg("stats")
        .query_async(&mut conn)
        .await
        .unwrap_or_default();
    let dbsize: Option<u64> = redis::cmd("DBSIZE").query_async(&mut conn).await.ok();

    let new_stream =
        inspect_single_stream(&mut conn, "new", &cfg.torrent_stream, &cfg.consumer_group).await;
    let hot_stream =
        inspect_single_stream(&mut conn, "hot", &cfg.hot_stream, &cfg.consumer_group).await;

    Ok(Json(RedisOverviewResponse {
        ok: true,
        message: None,
        latency_ms,
        redis_version: extract_info_field(&info_server, "redis_version"),
        uptime_secs: extract_info_field(&info_server, "uptime_in_seconds")
            .and_then(|s| s.parse::<u64>().ok()),
        used_memory_bytes: extract_info_field(&info_memory, "used_memory")
            .and_then(|s| s.parse::<u64>().ok()),
        used_memory_human: extract_info_field(&info_memory, "used_memory_human"),
        used_memory_peak_human: extract_info_field(&info_memory, "used_memory_peak_human"),
        connected_clients: extract_info_field(&info_clients, "connected_clients")
            .and_then(|s| s.parse::<u64>().ok()),
        instantaneous_ops_per_sec: extract_info_field(&info_stats, "instantaneous_ops_per_sec")
            .and_then(|s| s.parse::<u64>().ok()),
        dbsize,
        pool: pool_dto,
        config_summary,
        streams: vec![new_stream, hot_stream],
    }))
}

pub async fn redis_stream_preview_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Path(name): Path<String>,
    Query(query): Query<PreviewStreamQuery>,
) -> Result<Json<PreviewStreamResponse>, ApiError> {
    let (cfg, pool) = require_redis(&state).await?;
    let (role, stream_name) = resolve_stream_name(&cfg, &name)?;
    let count = query.count.unwrap_or(20).clamp(1, 100);

    let mut conn = pool.get().await.map_err(|e| {
        ApiError::internal(sanitize_secret_in_text(&format!(
            "获取 Redis 连接失败: {e}"
        )))
    })?;

    let total_len: i64 = redis::cmd("XLEN")
        .arg(stream_name)
        .query_async(&mut conn)
        .await
        .unwrap_or(0);

    let raw_entries: RedisValue = redis::cmd("XREVRANGE")
        .arg(stream_name)
        .arg("+")
        .arg("-")
        .arg("COUNT")
        .arg(count)
        .query_async(&mut conn)
        .await
        .map_err(|e| {
            ApiError::bad_request(
                "REDIS_QUERY_ERROR",
                sanitize_secret_in_text(&format!("预览 Stream 失败: {e}")),
            )
        })?;

    let mut items = Vec::new();
    if let RedisValue::Array(entries) = raw_entries {
        for entry in entries {
            let RedisValue::Array(mut pair) = entry else {
                continue;
            };
            if pair.len() < 2 {
                continue;
            }
            let raw_fields = pair.swap_remove(1);
            let raw_id = pair.swap_remove(0);
            let Some(entry_id) = redis_val_to_str(&raw_id) else {
                continue;
            };

            let field_map = redis_val_to_map(&raw_fields);
            let mut fields = BTreeMap::new();
            for (k, v) in field_map {
                if let Some(vs) = redis_val_to_str(&v) {
                    fields.insert(k, vs);
                }
            }

            let info_hash = fields.get("info_hash").cloned();
            let timestamp = fields.get("timestamp").cloned();
            let payload_str = fields.get("payload").cloned();
            let payload_bytes = payload_str.as_ref().map_or(0, String::len);

            let (mut parsed_name, mut total_size, mut file_count) = (None, None, None);
            if let Some(ref pjson) = payload_str
                && let Ok(v) = serde_json::from_str::<Value>(pjson)
            {
                parsed_name = v.get("name").and_then(Value::as_str).map(str::to_string);
                total_size = v.get("total_size").and_then(Value::as_u64);
                file_count = v
                    .get("file_count")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok());
            }

            items.push(StreamEntryPreviewDto {
                entry_id,
                info_hash,
                timestamp,
                name: parsed_name,
                total_size,
                file_count,
                payload_bytes,
                fields,
            });
        }
    }

    Ok(Json(PreviewStreamResponse {
        stream_name: stream_name.to_string(),
        role,
        total_len,
        items,
    }))
}

pub async fn redis_stream_trim_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(req): Json<TrimStreamRequest>,
) -> Result<Json<TrimStreamResponse>, ApiError> {
    let (cfg, pool) = require_redis(&state).await?;
    let (_role, stream_name) = resolve_stream_name(&cfg, &name)?;

    if req.confirm_stream_name.trim() != stream_name {
        return Err(ApiError::bad_request(
            "CONFIRM_MISMATCH",
            format!("请准确输入 Stream 键名 \"{stream_name}\" 完成高危修剪确认"),
        ));
    }

    let mut conn = pool.get().await.map_err(|e| {
        ApiError::internal(sanitize_secret_in_text(&format!(
            "获取 Redis 连接失败: {e}"
        )))
    })?;

    let mut cmd = redis::cmd("XTRIM");
    cmd.arg(stream_name).arg("MAXLEN");
    if req.approximate {
        cmd.arg("~");
    }
    cmd.arg(req.max_len);

    let trimmed_count: i64 = cmd.query_async(&mut conn).await.map_err(|e| {
        ApiError::bad_request(
            "REDIS_TRIM_ERROR",
            sanitize_secret_in_text(&format!("执行 XTRIM 失败: {e}")),
        )
    })?;
    let remaining_len: i64 = redis::cmd("XLEN")
        .arg(stream_name)
        .query_async(&mut conn)
        .await
        .unwrap_or(0);

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "redis.stream.trim",
            stream_name,
            &json!({
                "max_len": req.max_len,
                "approximate": req.approximate,
                "trimmed_count": trimmed_count,
                "remaining_len": remaining_len,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(TrimStreamResponse {
        ok: true,
        stream_name: stream_name.to_string(),
        trimmed_count,
        remaining_len,
    }))
}

pub async fn redis_hash_inspect_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Path(raw_hash): Path<String>,
) -> Result<Json<RedisHashInspectResponse>, ApiError> {
    let info_hash = normalize_info_hash(&raw_hash)?;
    let expected_id = hash_to_id(&info_hash);
    let (cfg, pool) = require_redis(&state).await?;

    let mut conn = pool.get().await.map_err(|e| {
        ApiError::internal(sanitize_secret_in_text(&format!(
            "获取 Redis 连接失败: {e}"
        )))
    })?;

    let indexed_key = format!("{}{info_hash}", cfg.indexed_key_prefix);
    let pending_key = format!("hameta:lock:pending:{info_hash}");
    let rejected_key = format!("hameta:lock:rejected:{info_hash}");

    let (idx_val, idx_pttl, pend_val, pend_pttl, rej_val, rej_pttl): (
        Option<String>,
        i64,
        Option<String>,
        i64,
        Option<String>,
        i64,
    ) = redis::pipe()
        .get(&indexed_key)
        .pttl(&indexed_key)
        .get(&pending_key)
        .pttl(&pending_key)
        .get(&rejected_key)
        .pttl(&rejected_key)
        .query_async(&mut *conn)
        .await
        .map_err(|e| {
            ApiError::bad_request(
                "REDIS_QUERY_ERROR",
                sanitize_secret_in_text(&format!("查询 Hash 键失败: {e}")),
            )
        })?;

    let mut manticore_checked = false;
    let mut manticore_exists = false;
    let mut manticore_raw_name = None;
    let mut manticore_refine_name = None;
    let mut manticore_hot = None;
    let mut manticore_updated_at = None;

    if let Some((_, mc_client)) = state.services.manticore_client().await
        && expected_id > 0
    {
        let body = json!({
            "index": "torrents",
            "query": {
                "bool": {
                    "must": [
                        { "equals": { "id": expected_id } },
                        { "equals": { "info_hash": info_hash } }
                    ]
                }
            },
            "limit": 1
        });
        if let Ok(resp) = mc_client.search_json(&body).await {
            manticore_checked = true;
            if let Some(hit) = resp["hits"]["hits"].as_array().and_then(|arr| arr.first()) {
                let src = &hit["_source"];
                manticore_exists = true;
                manticore_raw_name = src["raw_name"].as_str().map(str::to_string);
                manticore_refine_name = src["refine_name"].as_str().map(str::to_string);
                manticore_hot = src["hot"].as_i64().and_then(|n| i32::try_from(n).ok());
                manticore_updated_at = src["updated_at"].as_i64();
            }
        }
    }

    Ok(Json(RedisHashInspectResponse {
        info_hash,
        expected_id,
        indexed: RedisKeyStateDto {
            key: indexed_key,
            exists: idx_val.is_some(),
            value: idx_val,
            pttl_ms: idx_pttl,
        },
        pending: RedisKeyStateDto {
            key: pending_key,
            exists: pend_val.is_some(),
            value: pend_val,
            pttl_ms: pend_pttl,
        },
        rejected: RedisKeyStateDto {
            key: rejected_key,
            exists: rej_val.is_some(),
            value: rej_val,
            pttl_ms: rej_pttl,
        },
        manticore_checked,
        manticore_exists,
        manticore_raw_name,
        manticore_refine_name,
        manticore_hot,
        manticore_updated_at,
    }))
}

pub async fn redis_hash_clear_locks_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(raw_hash): Path<String>,
    Query(query): Query<ClearHashLocksQuery>,
) -> Result<Json<Value>, ApiError> {
    let info_hash = normalize_info_hash(&raw_hash)?;
    let (cfg, pool) = require_redis(&state).await?;

    let clear_pending = query.clear_pending.unwrap_or(true);
    let clear_rejected = query.clear_rejected.unwrap_or(true);
    let clear_indexed = query.clear_indexed.unwrap_or(false);

    let mut keys = Vec::new();
    if clear_pending {
        keys.push(format!("hameta:lock:pending:{info_hash}"));
    }
    if clear_rejected {
        keys.push(format!("hameta:lock:rejected:{info_hash}"));
    }
    if clear_indexed {
        keys.push(format!("{}{info_hash}", cfg.indexed_key_prefix));
    }

    if keys.is_empty() {
        return Err(ApiError::bad_request(
            "INVALID_INPUT",
            "请至少勾选一种需要清理的锁或去重标记",
        ));
    }

    let mut conn = pool.get().await.map_err(|e| {
        ApiError::internal(sanitize_secret_in_text(&format!(
            "获取 Redis 连接失败: {e}"
        )))
    })?;

    let deleted_count: i64 = redis::cmd("DEL")
        .arg(&keys)
        .query_async(&mut conn)
        .await
        .map_err(|e| {
            ApiError::bad_request(
                "REDIS_DEL_ERROR",
                sanitize_secret_in_text(&format!("清理 Redis 锁失败: {e}")),
            )
        })?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "redis.hash.clear_locks",
            &info_hash,
            &json!({
                "keys": &keys,
                "deleted_count": deleted_count,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(json!({
        "ok": true,
        "info_hash": info_hash,
        "deleted_keys": keys,
        "deleted_count": deleted_count,
    })))
}
