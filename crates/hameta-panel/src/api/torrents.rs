use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::server::PanelState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use chrono::{TimeZone, Utc};
use hameta_common::{FileInfo, TorrentRecord, hash_to_id, normalize_info_hash};
use hameta_indexer::ManticoreClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct TorrentSummaryDto {
    pub id: i64,
    pub info_hash: String,
    pub magnet_link: String,
    pub raw_name: String,
    pub refine_name: String,
    pub file_count: i32,
    pub total_size: u64,
    pub hot: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TorrentDetailDto {
    pub id: i64,
    pub info_hash: String,
    pub magnet_link: String,
    pub raw_name: String,
    pub refine_name: String,
    pub file_count: i32,
    pub total_size: u64,
    pub files: Vec<FileInfo>,
    pub hot: i32,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct ListTorrentsQuery {
    pub q: Option<String>,
    pub sort_by: Option<String>,
    pub sort_dir: Option<String>,
    pub min_hot: Option<i64>,
    pub max_hot: Option<i64>,
    pub min_files: Option<i64>,
    pub max_files: Option<i64>,
    pub since_ts: Option<i64>,
    pub until_ts: Option<i64>,
    pub empty_refine_only: Option<bool>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ListTorrentsResponse {
    pub items: Vec<TorrentSummaryDto>,
    pub total: u64,
    pub total_relation: String,
    pub page: usize,
    pub page_size: usize,
    pub took_ms: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateTorrentRequest {
    pub info_hash: String,
    pub raw_name: String,
    #[serde(default)]
    pub refine_name: Option<String>,
    #[serde(default)]
    pub files: Vec<FileInfo>,
    #[serde(default = "default_hot")]
    pub hot: i32,
    #[serde(default = "default_true")]
    pub sync_redis_indexed: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTorrentRequest {
    pub raw_name: Option<String>,
    pub refine_name: Option<String>,
    pub hot: Option<i32>,
    pub files: Option<Vec<FileInfo>>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteTorrentQuery {
    pub keep_redis_indexed: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct BatchDeleteTorrentsRequest {
    pub ids: Vec<i64>,
    #[serde(default = "default_true")]
    pub keep_redis_indexed: bool,
}

fn default_hot() -> i32 {
    1
}

fn default_true() -> bool {
    true
}

async fn require_manticore(state: &PanelState) -> Result<ManticoreClient, ApiError> {
    let Some((_, client)) = state.services.manticore_client().await else {
        return Err(ApiError::bad_request(
            "MANTICORE_NOT_CONFIGURED",
            "尚未配置或连接 Manticore 服务",
        ));
    };
    Ok(client)
}

pub fn build_manticore_search_body(
    query: &ListTorrentsQuery,
    page: usize,
    page_size: usize,
) -> Value {
    let mut must: Vec<Value> = Vec::new();
    let mut base_match: Option<Value> = None;

    if let Some(raw_q) = query.q.as_deref() {
        let trimmed = raw_q.trim();
        if !trimmed.is_empty() {
            if let Ok(norm_hash) = normalize_info_hash(trimmed) {
                let id = hash_to_id(&norm_hash);
                must.push(json!({ "equals": { "id": id } }));
                must.push(json!({ "equals": { "info_hash": norm_hash } }));
            } else {
                base_match = Some(json!({ "match": { "raw_name,refine_name": trimmed } }));
            }
        }
    }

    if query.min_hot.is_some() || query.max_hot.is_some() {
        let mut r = serde_json::Map::new();
        if let Some(v) = query.min_hot {
            r.insert("gte".to_string(), json!(v));
        }
        if let Some(v) = query.max_hot {
            r.insert("lte".to_string(), json!(v));
        }
        must.push(json!({ "range": { "hot": Value::Object(r) } }));
    }

    if query.min_files.is_some() || query.max_files.is_some() {
        let mut r = serde_json::Map::new();
        if let Some(v) = query.min_files {
            r.insert("gte".to_string(), json!(v));
        }
        if let Some(v) = query.max_files {
            r.insert("lte".to_string(), json!(v));
        }
        must.push(json!({ "range": { "file_count": Value::Object(r) } }));
    }

    if query.since_ts.is_some() || query.until_ts.is_some() {
        let mut r = serde_json::Map::new();
        if let Some(v) = query.since_ts {
            r.insert("gte".to_string(), json!(v));
        }
        if let Some(v) = query.until_ts {
            r.insert("lte".to_string(), json!(v));
        }
        must.push(json!({ "range": { "created_at": Value::Object(r) } }));
    }

    let query_obj = match (base_match, must.is_empty()) {
        (Some(m), true) => m,
        (None, true) => json!({ "match_all": {} }),
        (Some(m), false) => {
            must.insert(0, m);
            json!({ "bool": { "must": must } })
        }
        (None, false) => json!({ "bool": { "must": must } }),
    };

    let sort_col = match query.sort_by.as_deref().unwrap_or("updated_at") {
        "created_at" => "created_at",
        "hot" => "hot",
        "file_count" => "file_count",
        _ => "updated_at",
    };
    let sort_dir = match query.sort_dir.as_deref().unwrap_or("desc") {
        "asc" => "asc",
        _ => "desc",
    };

    let offset = page.saturating_sub(1).saturating_mul(page_size);
    let max_matches = (offset + page_size).max(1000);

    json!({
        "index": "torrents",
        "query": query_obj,
        "sort": [{ sort_col: { "order": sort_dir } }],
        "limit": page_size,
        "offset": offset,
        "options": { "max_matches": max_matches }
    })
}

fn parse_hit_id(val: &Value) -> i64 {
    match val {
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(s) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn parse_files_from_source(src: &Value) -> Vec<FileInfo> {
    match src.get("file_info") {
        Some(Value::String(s)) => serde_json::from_str::<Vec<FileInfo>>(s).unwrap_or_default(),
        Some(arr @ Value::Array(_)) => {
            serde_json::from_value::<Vec<FileInfo>>(arr.clone()).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn parse_hit_detail(hit: &Value) -> TorrentDetailDto {
    let id = parse_hit_id(&hit["_id"]);
    let src = &hit["_source"];
    let info_hash = src["info_hash"].as_str().unwrap_or_default().to_string();
    let raw_name = src["raw_name"].as_str().unwrap_or_default().to_string();
    let refine_name = src["refine_name"].as_str().unwrap_or_default().to_string();
    let file_count = i32::try_from(src["file_count"].as_i64().unwrap_or(0)).unwrap_or(0);
    let hot = i32::try_from(src["hot"].as_i64().unwrap_or(0)).unwrap_or(0);
    let created_at = src["created_at"].as_i64().unwrap_or(0);
    let updated_at = src["updated_at"].as_i64().unwrap_or(0);
    let files = parse_files_from_source(src);
    let total_size = files
        .iter()
        .fold(0_u64, |acc, f| acc.saturating_add(f.size));

    TorrentDetailDto {
        id,
        magnet_link: format!("magnet:?xt=urn:btih:{info_hash}"),
        info_hash,
        raw_name,
        refine_name,
        file_count,
        total_size,
        files,
        hot,
        created_at,
        updated_at,
    }
}

async fn find_torrent_detail(
    client: &ManticoreClient,
    id_or_hash: &str,
) -> Result<Option<TorrentDetailDto>, ApiError> {
    let trimmed = id_or_hash.trim();
    let (id, expect_hash) = if let Ok(norm) = normalize_info_hash(trimmed) {
        (hash_to_id(&norm), Some(norm))
    } else if let Ok(num) = trimmed.parse::<i64>()
        && num > 0
    {
        (num, None)
    } else {
        return Err(ApiError::bad_request(
            "INVALID_TORRENT_ID",
            "无效的种子 ID 或 40 位 info_hash",
        ));
    };

    let body = json!({
        "index": "torrents",
        "query": { "equals": { "id": id } },
        "limit": 1
    });
    let resp = client
        .search_json(&body)
        .await
        .map_err(|e| ApiError::bad_request("MANTICORE_QUERY_ERROR", e.to_string()))?;
    let Some(hit) = resp["hits"]["hits"].as_array().and_then(|arr| arr.first()) else {
        return Ok(None);
    };
    let detail = parse_hit_detail(hit);
    if let Some(ref h) = expect_hash
        && detail.info_hash != *h
    {
        return Ok(None);
    }
    Ok(Some(detail))
}

async fn sync_redis_indexed_keys(
    state: &PanelState,
    hashes: &[String],
    keep_indexed: bool,
) -> Result<(), ApiError> {
    if hashes.is_empty() {
        return Ok(());
    }
    let Some((cfg, pool)) = state.services.redis_pool().await else {
        return Ok(());
    };
    let mut conn = pool.get().await.map_err(|e| {
        ApiError::internal("获取 Redis 连接同步标记失败").with_detail(e.to_string())
    })?;

    let mut pipe = redis::pipe();
    for hash in hashes {
        let indexed_key = format!("{}{hash}", cfg.indexed_key_prefix);
        let pending_key = format!("hameta:lock:pending:{hash}");
        if keep_indexed {
            pipe.set(&indexed_key, "1").ignore();
        } else {
            pipe.del(&indexed_key).ignore();
        }
        pipe.del(&pending_key).ignore();
    }
    pipe.query_async::<()>(&mut *conn)
        .await
        .map_err(|e| ApiError::internal("执行 Redis 标记同步失败").with_detail(e.to_string()))?;
    Ok(())
}

pub async fn list_torrents_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Query(query): Query<ListTorrentsQuery>,
) -> Result<Json<ListTorrentsResponse>, ApiError> {
    let client = require_manticore(&state).await?;
    let page = query.page.unwrap_or(1).clamp(1, 1000);
    let page_size = query.page_size.unwrap_or(20).clamp(5, 100);

    let search_body = build_manticore_search_body(&query, page, page_size);
    let resp = client
        .search_json(&search_body)
        .await
        .map_err(|e| ApiError::bad_request("MANTICORE_SEARCH_ERROR", e.to_string()))?;

    let took_ms = resp["took"].as_u64().unwrap_or(0);
    let total = resp["hits"]["total"].as_u64().unwrap_or(0);
    let total_relation = resp["hits"]["total_relation"]
        .as_str()
        .unwrap_or("eq")
        .to_string();

    let empty_only = query.empty_refine_only.unwrap_or(false);
    let mut items = Vec::new();
    if let Some(hits) = resp["hits"]["hits"].as_array() {
        for hit in hits {
            let detail = parse_hit_detail(hit);
            if empty_only && !detail.refine_name.trim().is_empty() {
                continue;
            }
            items.push(TorrentSummaryDto {
                id: detail.id,
                info_hash: detail.info_hash,
                magnet_link: detail.magnet_link,
                raw_name: detail.raw_name,
                refine_name: detail.refine_name,
                file_count: detail.file_count,
                total_size: detail.total_size,
                hot: detail.hot,
                created_at: detail.created_at,
                updated_at: detail.updated_at,
            });
        }
    }

    Ok(Json(ListTorrentsResponse {
        items,
        total,
        total_relation,
        page,
        page_size,
        took_ms,
    }))
}

pub async fn get_torrent_detail_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Path(id): Path<String>,
) -> Result<Json<TorrentDetailDto>, ApiError> {
    let client = require_manticore(&state).await?;
    let detail = find_torrent_detail(&client, &id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("未找到种子记录: {id}")))?;
    Ok(Json(detail))
}

pub async fn create_torrent_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<CreateTorrentRequest>,
) -> Result<Json<TorrentDetailDto>, ApiError> {
    let info_hash = normalize_info_hash(&req.info_hash)?;
    let id = hash_to_id(&info_hash);
    if id <= 0 {
        return Err(ApiError::bad_request(
            "INVALID_INFO_HASH",
            "该 info_hash 映射到保留主键 0，无法入库",
        ));
    }
    let raw_name = req.raw_name.trim().to_string();
    if raw_name.is_empty() {
        return Err(ApiError::bad_request(
            "INVALID_INPUT",
            "原始标题 (raw_name) 不能为空",
        ));
    }

    let refine_name = req
        .refine_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| raw_name.clone());

    let client = require_manticore(&state).await?;
    let now = Utc::now();
    let record = TorrentRecord {
        info_hash: info_hash.clone(),
        raw_name,
        refine_name: Some(refine_name),
        file_count: i32::try_from(req.files.len()).unwrap_or(i32::MAX),
        file_info: req.files,
        hot: req.hot.max(1),
        created_at: now,
        updated_at: now,
    };
    client
        .batch_insert(&[record])
        .await
        .map_err(|e| ApiError::bad_request("MANTICORE_WRITE_ERROR", e.to_string()))?;

    if req.sync_redis_indexed {
        let _ = sync_redis_indexed_keys(&state, std::slice::from_ref(&info_hash), true).await;
    }

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "torrent.create",
            &info_hash,
            &json!({ "id": id, "sync_redis": req.sync_redis_indexed }).to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    let saved = find_torrent_detail(&client, &info_hash)
        .await?
        .ok_or_else(|| ApiError::internal("写入成功但未能回读新记录"))?;
    Ok(Json(saved))
}

pub async fn update_torrent_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<UpdateTorrentRequest>,
) -> Result<Json<TorrentDetailDto>, ApiError> {
    let client = require_manticore(&state).await?;
    let existing = find_torrent_detail(&client, &id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("未找到种子记录: {id}")))?;

    let next_raw = req
        .raw_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.raw_name);
    let next_refine = req
        .refine_name
        .map(|s| s.trim().to_string())
        .unwrap_or(existing.refine_name);
    let next_files = req.files.unwrap_or(existing.files);
    let next_hot = req.hot.unwrap_or(existing.hot);
    let created_dt = Utc
        .timestamp_opt(existing.created_at, 0)
        .single()
        .unwrap_or_else(Utc::now);

    let record = TorrentRecord {
        info_hash: existing.info_hash.clone(),
        raw_name: next_raw,
        refine_name: Some(next_refine),
        file_count: i32::try_from(next_files.len()).unwrap_or(i32::MAX),
        file_info: next_files,
        hot: next_hot,
        created_at: created_dt,
        updated_at: Utc::now(),
    };
    client
        .batch_insert(&[record])
        .await
        .map_err(|e| ApiError::bad_request("MANTICORE_WRITE_ERROR", e.to_string()))?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "torrent.update",
            &existing.info_hash,
            &json!({ "id": existing.id }).to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    let updated = find_torrent_detail(&client, &existing.info_hash)
        .await?
        .ok_or_else(|| ApiError::internal("更新成功但未能回读记录"))?;
    Ok(Json(updated))
}

pub async fn delete_torrent_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DeleteTorrentQuery>,
) -> Result<Json<Value>, ApiError> {
    let client = require_manticore(&state).await?;
    let existing = find_torrent_detail(&client, &id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("未找到种子记录: {id}")))?;
    let keep_redis = query.keep_redis_indexed.unwrap_or(true);

    client
        .delete_by_ids(&[existing.id])
        .await
        .map_err(|e| ApiError::bad_request("MANTICORE_DELETE_ERROR", e.to_string()))?;
    sync_redis_indexed_keys(
        &state,
        std::slice::from_ref(&existing.info_hash),
        keep_redis,
    )
    .await?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "torrent.delete",
            &existing.info_hash,
            &json!({ "id": existing.id, "keep_redis_indexed": keep_redis }).to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(json!({
        "ok": true,
        "deleted_id": existing.id,
        "info_hash": existing.info_hash,
        "keep_redis_indexed": keep_redis
    })))
}

pub async fn batch_delete_torrents_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<BatchDeleteTorrentsRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.ids.is_empty() {
        return Err(ApiError::bad_request("INVALID_INPUT", "请选择至少一条记录"));
    }
    if req.ids.len() > 200 {
        return Err(ApiError::bad_request(
            "TOO_MANY_ITEMS",
            "单次批量删除上限为 200 条",
        ));
    }

    let client = require_manticore(&state).await?;
    let lookup_body = json!({
        "index": "torrents",
        "query": { "in": { "id": &req.ids } },
        "_source": ["info_hash"],
        "limit": req.ids.len()
    });
    let resp = client
        .search_json(&lookup_body)
        .await
        .map_err(|e| ApiError::bad_request("MANTICORE_SEARCH_ERROR", e.to_string()))?;

    let mut hashes = Vec::new();
    if let Some(hits) = resp["hits"]["hits"].as_array() {
        for hit in hits {
            if let Some(h) = hit["_source"]["info_hash"].as_str()
                && let Ok(norm) = normalize_info_hash(h)
            {
                hashes.push(norm);
            }
        }
    }

    let deleted = client
        .delete_by_ids(&req.ids)
        .await
        .map_err(|e| ApiError::bad_request("MANTICORE_DELETE_ERROR", e.to_string()))?;
    sync_redis_indexed_keys(&state, &hashes, req.keep_redis_indexed).await?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "torrent.batch_delete",
            "torrents",
            &json!({
                "requested_count": req.ids.len(),
                "deleted_count": deleted,
                "keep_redis_indexed": req.keep_redis_indexed
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(json!({
        "ok": true,
        "deleted": deleted,
        "redis_synced": hashes.len(),
        "keep_redis_indexed": req.keep_redis_indexed
    })))
}
