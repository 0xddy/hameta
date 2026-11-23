use anyhow::{Context, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chrono::Utc;
use hameta_common::{
    ManticoreAuthMode, ManticoreServiceConfig, TABLE_TORRENTS, TorrentRecord, hash_to_id,
};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, ClientBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

const EXPECTED_TORRENTS_COLUMNS: &[&str] = &[
    "id",
    "info_hash",
    "raw_name",
    "refine_name",
    "file_count",
    "file_info",
    "hot",
    "created_at",
    "updated_at",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSchemaInfo {
    pub field: String,
    pub column_type: String,
    pub properties: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaCheckReport {
    pub table_exists: bool,
    pub compatible: bool,
    pub columns: Vec<ColumnSchemaInfo>,
    pub missing_columns: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TableStatusInfo {
    pub indexed_documents: Option<u64>,
    pub disk_bytes: Option<u64>,
    pub ram_bytes: Option<u64>,
    pub raw_rows: BTreeMap<String, String>,
}

/// Manticore HTTP 客户端（严格保护存量 `torrents` 表结构，严禁自动 `ALTER`/`DROP`/`CREATE`）。
#[derive(Clone)]
pub struct ManticoreClient {
    client: Client,
    base_url: String,
}

impl ManticoreClient {
    pub fn new(config: &ManticoreServiceConfig) -> Result<Self> {
        config.validate().context("Manticore 配置无效")?;
        let base_url = config.url.trim().trim_end_matches('/').to_string();

        let mut builder = ClientBuilder::new()
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(usize::try_from(config.max_connections.max(1)).unwrap_or(128))
            .timeout(Duration::from_secs(config.request_timeout_secs.max(1)))
            .connect_timeout(Duration::from_secs(3))
            .tcp_keepalive(Duration::from_secs(60))
            .tcp_nodelay(true);

        let mut headers = HeaderMap::new();
        match config.effective_auth_mode() {
            ManticoreAuthMode::None => {}
            ManticoreAuthMode::Basic => {
                if let Some((username, password)) = config.basic_auth() {
                    let token = STANDARD.encode(format!("{username}:{password}"));
                    let mut value = HeaderValue::from_str(&format!("Basic {token}"))
                        .context("构造 Manticore Basic Authorization 请求头失败")?;
                    value.set_sensitive(true);
                    headers.insert(AUTHORIZATION, value);
                }
            }
            ManticoreAuthMode::Bearer => {
                if let Some(token) = config.bearer_token() {
                    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                        .context("构造 Manticore Bearer Authorization 请求头失败")?;
                    value.set_sensitive(true);
                    headers.insert(AUTHORIZATION, value);
                }
            }
            ManticoreAuthMode::Header => {
                if let Some((name, val)) = config.custom_header_auth() {
                    let header_name = HeaderName::from_bytes(name.as_bytes())
                        .context("解析 Manticore 自定义鉴权请求头名称失败")?;
                    let mut value = HeaderValue::from_str(val)
                        .context("构造 Manticore 自定义鉴权请求头值失败")?;
                    value.set_sensitive(true);
                    headers.insert(header_name, value);
                }
            }
        }
        if !headers.is_empty() {
            builder = builder.default_headers(headers);
        }

        let client = builder.build().context("构建 Manticore HTTP 客户端失败")?;
        Ok(Self { client, base_url })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 只读检查 `torrents` 表是否存在及其字段是否满足锁定契约。
    pub async fn describe_torrents(&self) -> Result<SchemaCheckReport> {
        let query = format!("DESCRIBE {TABLE_TORRENTS}");
        let url = format!("{}/sql", self.base_url);
        let resp = self
            .client
            .post(&url)
            .form(&[("mode", "raw"), ("query", query.as_str())])
            .send()
            .await
            .context("连接 Manticore /sql 执行 DESCRIBE 失败")?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .context("读取 Manticore DESCRIBE 响应体失败")?;

        if !status.is_success() {
            if is_missing_table_error(&text) {
                return Ok(SchemaCheckReport {
                    table_exists: false,
                    compatible: false,
                    columns: Vec::new(),
                    missing_columns: EXPECTED_TORRENTS_COLUMNS
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                    message: format!("Manticore 中不存在表 {TABLE_TORRENTS}"),
                });
            }
            return Err(format_manticore_http_error(
                "Manticore DESCRIBE 请求",
                status,
                &text,
            ));
        }

        parse_describe_response(&text)
    }

    /// 仅在空库首次初始化时由运维人员显式触发，不可在 Worker 启动路径自动调用。
    pub async fn create_torrents_table_if_missing(&self) -> Result<()> {
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {TABLE_TORRENTS} (info_hash string, raw_name text, refine_name text, file_count int, file_info json, hot int, created_at timestamp, updated_at timestamp) engine='columnar' ngram_len='1' ngram_chars='cjk'"
        );
        self.execute_raw_sql(&create_sql)
            .await
            .with_context(|| format!("显式创建 {TABLE_TORRENTS} 表失败"))?;
        Ok(())
    }

    pub async fn table_status(&self) -> Result<TableStatusInfo> {
        let query = format!("SHOW TABLE {TABLE_TORRENTS} STATUS");
        let results = self.execute_raw_sql(&query).await?;
        let mut info = TableStatusInfo::default();
        if let Some(first) = results.first()
            && let Some(rows) = first.get("data").and_then(Value::as_array)
        {
            for row in rows {
                let name = row
                    .get("Variable_name")
                    .or_else(|| row.get("variable_name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let val_str = match row.get("Value").or_else(|| row.get("value")) {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                if name.is_empty() {
                    continue;
                }
                match name.as_str() {
                    "indexed_documents" => info.indexed_documents = val_str.parse::<u64>().ok(),
                    "disk_bytes" => info.disk_bytes = val_str.parse::<u64>().ok(),
                    "ram_bytes" => info.ram_bytes = val_str.parse::<u64>().ok(),
                    _ => {}
                }
                info.raw_rows.insert(name, val_str);
            }
        }
        Ok(info)
    }

    pub async fn execute_raw_sql(&self, query: &str) -> Result<Vec<Value>> {
        self.execute_raw_sql_opt_timeout(query, None).await
    }

    pub async fn execute_raw_sql_with_timeout(
        &self,
        query: &str,
        timeout: Duration,
    ) -> Result<Vec<Value>> {
        self.execute_raw_sql_opt_timeout(query, Some(timeout)).await
    }

    async fn execute_raw_sql_opt_timeout(
        &self,
        query: &str,
        timeout: Option<Duration>,
    ) -> Result<Vec<Value>> {
        let url = format!("{}/sql", self.base_url);
        let mut req = self
            .client
            .post(&url)
            .form(&[("mode", "raw"), ("query", query)]);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        let resp = req.send().await.context("发送 Manticore /sql 请求失败")?;

        let status = resp.status();
        let text = resp.text().await.context("读取 Manticore SQL 响应失败")?;
        if !status.is_success() {
            return Err(format_manticore_http_error(
                "Manticore SQL 执行",
                status,
                &text,
            ));
        }
        validate_sql_response(&text)
    }

    pub async fn search_json(&self, body: &Value) -> Result<Value> {
        let url = format!("{}/search", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .context("发送 Manticore /search 请求失败")?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .context("读取 Manticore /search 响应失败")?;
        if !status.is_success() {
            return Err(format_manticore_http_error(
                "Manticore /search 请求",
                status,
                &text,
            ));
        }
        serde_json::from_str(&text).context("解析 Manticore /search JSON 响应失败")
    }

    pub async fn batch_insert(&self, records: &[TorrentRecord]) -> Result<usize> {
        if records.is_empty() {
            return Ok(0);
        }

        let records_by_id = writable_records(records);
        if records_by_id.is_empty() {
            return Ok(0);
        }

        let mut bulk_body = String::with_capacity(records_by_id.len() * 256);
        let mut write_count = 0_u64;

        for (id, record) in records_by_id {
            let file_info_json =
                serde_json::to_string(&record.file_info).unwrap_or_else(|_| "[]".to_string());
            let action = json!({
                "replace": {
                    "index": TABLE_TORRENTS,
                    "id": id,
                    "doc": {
                        "info_hash": record.info_hash,
                        "raw_name": record.raw_name,
                        "refine_name": record.refine_name.as_deref().unwrap_or(""),
                        "file_count": record.file_count,
                        "file_info": file_info_json,
                        "hot": record.hot,
                        "created_at": record.created_at.timestamp(),
                        "updated_at": record.updated_at.timestamp(),
                    }
                }
            });
            bulk_body.push_str(&action.to_string());
            bulk_body.push('\n');
            write_count += 1;
        }

        self.execute_bulk(&bulk_body, Some(write_count))
            .await
            .context("Manticore 批量 replace 执行失败")?;
        Ok(usize::try_from(write_count).unwrap_or(usize::MAX))
    }

    pub async fn batch_update_hot(&self, updates: &HashMap<String, i32>) -> Result<usize> {
        if updates.is_empty() {
            return Ok(0);
        }

        let now_ts = Utc::now().timestamp();
        let mut hash_to_id_map: HashMap<String, i64> = HashMap::new();
        let mut valid_ids = Vec::new();

        for hash in updates.keys() {
            let id = hash_to_id(hash);
            if id > 0 {
                hash_to_id_map.insert(hash.clone(), id);
                valid_ids.push(id);
            }
        }
        if valid_ids.is_empty() {
            return Ok(0);
        }

        let mut existing_docs: HashMap<i64, ExistingHotDoc> = HashMap::new();
        let search_url = format!("{}/search", self.base_url);

        for chunk in valid_ids.chunks(500) {
            let search_body = json!({
                "index": TABLE_TORRENTS,
                "query": { "in": { "id": chunk } },
                "_source": [
                    "info_hash",
                    "raw_name",
                    "refine_name",
                    "file_count",
                    "file_info",
                    "hot",
                    "created_at"
                ],
                "limit": chunk.len()
            });
            let resp = self
                .client
                .post(&search_url)
                .json(&search_body)
                .send()
                .await
                .context("查询 Manticore 存量热度记录失败")?;
            if resp.status().is_success() {
                let json_resp: Value = resp.json().await.context("解析热度查询响应失败")?;
                if let Some(hits) = json_resp["hits"]["hits"].as_array() {
                    for hit in hits {
                        let id = parse_hit_id(&hit["_id"]);
                        if id <= 0 {
                            continue;
                        }
                        let src = &hit["_source"];
                        existing_docs.insert(
                            id,
                            ExistingHotDoc {
                                info_hash: src["info_hash"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                raw_name: src["raw_name"].as_str().unwrap_or_default().to_string(),
                                refine_name: src["refine_name"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                file_count: i32::try_from(src["file_count"].as_i64().unwrap_or(0))
                                    .unwrap_or(0),
                                file_info: normalize_file_info_value(src.get("file_info")),
                                hot: i32::try_from(src["hot"].as_i64().unwrap_or(0)).unwrap_or(0),
                                created_at: parse_i64_field(&src["created_at"]).unwrap_or(now_ts),
                            },
                        );
                    }
                }
            } else {
                let st = resp.status();
                let err_body = resp.text().await.unwrap_or_default();
                return Err(format_manticore_http_error(
                    "查询 Manticore 存量热度",
                    st,
                    &err_body,
                ));
            }
        }

        // Manticore engine='columnar' 不支持原地部分 update 属性，统一构造完整文档整行 replace 保证存量兼容
        let mut lines = Vec::new();
        let mut updated_count = 0_usize;

        for (hash, increment) in updates {
            if let Some(&id) = hash_to_id_map.get(hash)
                && let Some(doc) = existing_docs.get(&id)
            {
                let info_hash = if doc.info_hash.is_empty() {
                    hash.as_str()
                } else {
                    doc.info_hash.as_str()
                };
                let new_hot = doc.hot.saturating_add(*increment);
                let action = json!({
                    "replace": {
                        "index": TABLE_TORRENTS,
                        "id": id,
                        "doc": {
                            "info_hash": info_hash,
                            "raw_name": doc.raw_name,
                            "refine_name": doc.refine_name,
                            "file_count": doc.file_count,
                            "file_info": doc.file_info,
                            "hot": new_hot,
                            "created_at": doc.created_at,
                            "updated_at": now_ts
                        }
                    }
                });
                lines.push(action.to_string());
                updated_count += 1;
            }
        }

        if !lines.is_empty() {
            for chunk in lines.chunks(500) {
                let mut chunk_body = chunk.join("\n");
                chunk_body.push('\n');
                let expected = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
                self.execute_bulk(&chunk_body, Some(expected))
                    .await
                    .context("Manticore 批量 replace 更新热度执行失败")?;
            }
        }

        Ok(updated_count)
    }

    /// 按 63-bit 整型主键批量删除 `torrents` 记录（每批最多 200 条）。
    pub async fn delete_by_ids(&self, ids: &[i64]) -> Result<usize> {
        let mut unique: Vec<i64> = ids.iter().copied().filter(|&id| id > 0).collect();
        unique.sort_unstable();
        unique.dedup();
        if unique.is_empty() {
            return Ok(0);
        }

        let mut deleted_total = 0_usize;
        for chunk in unique.chunks(200) {
            let id_list = chunk
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("DELETE FROM {TABLE_TORRENTS} WHERE id IN ({id_list})");
            let results = self
                .execute_raw_sql(&sql)
                .await
                .context("Manticore 批量删除记录失败")?;
            let chunk_deleted = results
                .first()
                .and_then(|v| v.get("total"))
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(chunk.len());
            deleted_total += chunk_deleted;
        }
        Ok(deleted_total)
    }

    async fn execute_bulk(&self, body: &str, expected_writes: Option<u64>) -> Result<()> {
        if body.is_empty() {
            return Ok(());
        }
        let url = format!("{}/bulk", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/x-ndjson")
            .body(body.to_string())
            .send()
            .await
            .context("发送 Manticore /bulk 请求失败")?;

        let status = resp.status();
        let text = resp.text().await.context("读取 Manticore /bulk 响应失败")?;
        if !status.is_success() {
            return Err(format_manticore_http_error(
                "Manticore /bulk 请求",
                status,
                &text,
            ));
        }

        let operations = body.lines().filter(|line| !line.trim().is_empty()).count();
        validate_bulk_response(&text, operations, expected_writes)
    }
}

fn format_manticore_http_error(
    action: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> anyhow::Error {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return anyhow::anyhow!(
            "{action}鉴权失败 (HTTP {status})：反向代理拒绝访问，请检查 Manticore 鉴权模式与凭据配置"
        );
    }
    let trimmed = body.trim();
    let snippet: String = trimmed.chars().take(512).collect();
    if trimmed.chars().count() > 512 {
        anyhow::anyhow!("{action}失败 (HTTP {status}): {snippet}...")
    } else {
        anyhow::anyhow!("{action}失败 (HTTP {status}): {snippet}")
    }
}

pub fn writable_records(records: &[TorrentRecord]) -> Vec<(i64, &TorrentRecord)> {
    let mut unique_records: BTreeMap<&str, &TorrentRecord> = BTreeMap::new();
    for record in records {
        if record.info_hash.len() == 40 {
            unique_records.insert(record.info_hash.as_str(), record);
        }
    }

    let mut records_by_id: BTreeMap<i64, &TorrentRecord> = BTreeMap::new();
    for record in unique_records.values() {
        let id = hash_to_id(&record.info_hash);
        if id == 0 {
            continue;
        }
        if records_by_id.insert(id, record).is_some() {
            tracing::warn!("批次内出现 63-bit ID 哈希碰撞: id={id}");
        }
    }
    records_by_id.into_iter().collect()
}

pub fn writable_info_hashes(records: &[TorrentRecord]) -> Vec<String> {
    writable_records(records)
        .into_iter()
        .map(|(_, record)| record.info_hash.clone())
        .collect()
}

fn response_has_error(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

pub fn is_missing_table_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("unknown table")
        || lower.contains("does not exist")
        || lower.contains("no such table")
        || lower.contains("requires an existing table")
}

pub fn parse_describe_response(text: &str) -> Result<SchemaCheckReport> {
    let results: Value = serde_json::from_str(text).context("DESCRIBE 响应不是合法 JSON")?;
    let arr = results.as_array().context("DESCRIBE 响应顶层必须是数组")?;
    anyhow::ensure!(!arr.is_empty(), "DESCRIBE 响应结果为空");
    let first = arr[0].as_object().context("DESCRIBE 首项必须是对象")?;

    if response_has_error(first.get("error")) {
        let err_text = first.get("error").map(Value::to_string).unwrap_or_default();
        if is_missing_table_error(&err_text) {
            return Ok(SchemaCheckReport {
                table_exists: false,
                compatible: false,
                columns: Vec::new(),
                missing_columns: EXPECTED_TORRENTS_COLUMNS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                message: format!("Manticore 中不存在表 {TABLE_TORRENTS}: {err_text}"),
            });
        }
        anyhow::bail!("Manticore DESCRIBE 返回错误: {err_text}");
    }

    let rows = first
        .get("data")
        .and_then(Value::as_array)
        .context("DESCRIBE 响应缺少 data 数组")?;

    let mut columns = Vec::with_capacity(rows.len());
    let mut present_names = HashSet::new();

    for row in rows {
        let field = row
            .get("Field")
            .or_else(|| row.get("field"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let column_type = row
            .get("Type")
            .or_else(|| row.get("type"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let properties = row
            .get("Properties")
            .or_else(|| row.get("properties"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !field.is_empty() {
            present_names.insert(field.to_ascii_lowercase());
            columns.push(ColumnSchemaInfo {
                field,
                column_type,
                properties,
            });
        }
    }

    let missing_columns: Vec<String> = EXPECTED_TORRENTS_COLUMNS
        .iter()
        .filter(|col| !present_names.contains(**col))
        .map(|col| (*col).to_string())
        .collect();

    let compatible = missing_columns.is_empty();
    let message = if compatible {
        format!("{TABLE_TORRENTS} 表结构校验通过（共 {} 列）", columns.len())
    } else {
        format!(
            "{TABLE_TORRENTS} 缺少必需字段: {}",
            missing_columns.join(", ")
        )
    };

    Ok(SchemaCheckReport {
        table_exists: true,
        compatible,
        columns,
        missing_columns,
        message,
    })
}

pub fn validate_sql_response(text: &str) -> Result<Vec<Value>> {
    let value: Value = serde_json::from_str(text).context("SQL 响应不是合法 JSON")?;
    let results = value
        .as_array()
        .context("SQL 响应顶层必须是结果数组")?
        .clone();
    anyhow::ensure!(!results.is_empty(), "SQL 响应不包含任何结果项");
    for result in &results {
        let obj = result.as_object().context("SQL 结果项必须是对象")?;
        anyhow::ensure!(obj.contains_key("error"), "SQL 结果缺少 error 状态字段");
        anyhow::ensure!(
            !response_has_error(obj.get("error")),
            "Manticore SQL 执行失败: {:?}",
            obj.get("error")
        );
    }
    Ok(results)
}

/// 严格校验 Manticore `/bulk` 回执，支持分组事务与逐条回执，坚决拒绝任何残缺或歧义响应。
pub fn validate_bulk_response(
    text: &str,
    operations: usize,
    expected_writes: Option<u64>,
) -> Result<()> {
    let value: Value = serde_json::from_str(text).context("Bulk 响应不是合法 JSON")?;
    let response = value.as_object().context("Bulk 响应顶层必须是对象")?;

    anyhow::ensure!(
        response.get("errors").and_then(Value::as_bool) == Some(false),
        "Bulk 响应未确认无错误 (errors != false)"
    );
    anyhow::ensure!(
        !response_has_error(response.get("error")),
        "Bulk 响应顶层包含错误信息"
    );
    if let Some(skipped) = response.get("skipped_lines") {
        anyhow::ensure!(
            skipped.as_u64() == Some(0),
            "Bulk 响应存在被跳过行: skipped_lines={skipped}"
        );
    }
    if let Some(line) = response.get("current_line") {
        anyhow::ensure!(
            line.as_u64().is_some_and(|l| l > operations as u64),
            "Bulk 响应处理游标未到达末尾 (current_line={line}, ops={operations})"
        );
    }

    let items = response
        .get("items")
        .and_then(Value::as_array)
        .context("Bulk 响应缺少 items 回执数组")?;
    anyhow::ensure!(!items.is_empty(), "Bulk 响应 items 数组为空");

    let mut grouped = false;
    let mut writes = 0_u64;

    for item in items {
        let obj = item.as_object().context("Bulk 回执单项必须是对象")?;
        anyhow::ensure!(obj.len() == 1, "Bulk 回执单项结构存在歧义");
        let (action, result) = obj.iter().next().context("Bulk 回执单项为空")?;
        let res_obj = result.as_object().context("Bulk 回执详情必须是对象")?;
        anyhow::ensure!(
            !response_has_error(res_obj.get("error")),
            "Bulk 单项包含错误"
        );
        if let Some(status) = res_obj.get("status") {
            anyhow::ensure!(
                status.as_u64().is_some_and(|s| (200..300).contains(&s)),
                "Bulk 单项 HTTP 状态非成功: {status}"
            );
        }

        match action.as_str() {
            "bulk" => {
                grouped = true;
                anyhow::ensure!(
                    res_obj.contains_key("status"),
                    "分组 Bulk 回执缺少 status 字段"
                );
                let created = res_obj
                    .get("created")
                    .and_then(Value::as_u64)
                    .context("分组 Bulk 回执缺少 created 计数")?;
                let updated = res_obj
                    .get("updated")
                    .and_then(Value::as_u64)
                    .context("分组 Bulk 回执缺少 updated 计数")?;
                writes = writes
                    .checked_add(created)
                    .and_then(|v| v.checked_add(updated))
                    .context("Bulk 写入总数溢出")?;
            }
            "replace" | "index" | "insert" | "create" => {
                anyhow::ensure!(
                    matches!(
                        res_obj.get("result").and_then(Value::as_str),
                        Some("created" | "updated")
                    ),
                    "Bulk 写入回执缺少 created/updated 状态"
                );
                writes += 1;
            }
            "update" => {
                anyhow::ensure!(
                    res_obj.get("updated").and_then(Value::as_u64).is_some()
                        || res_obj.get("result").and_then(Value::as_str) == Some("updated"),
                    "Bulk update 回执缺少确认字段"
                );
            }
            _ => anyhow::bail!("未预期的 Bulk 操作回执类型: {action}"),
        }
    }

    if !grouped {
        anyhow::ensure!(
            items.len() == operations,
            "逐条 Bulk 回执数量与请求操作数不符: {} != {operations}",
            items.len()
        );
    } else if expected_writes.is_none() {
        anyhow::ensure!(
            response
                .get("current_line")
                .and_then(Value::as_u64)
                .is_some_and(|l| l > operations as u64),
            "分组 Update 回执缺少完整的 current_line 行确认"
        );
    }

    if let Some(expected) = expected_writes {
        anyhow::ensure!(
            writes == expected,
            "Bulk 实际确认写入 {writes} 条，预期 {expected} 条"
        );
    }

    Ok(())
}

struct ExistingHotDoc {
    info_hash: String,
    raw_name: String,
    refine_name: String,
    file_count: i32,
    file_info: String,
    hot: i32,
    created_at: i64,
}

fn parse_hit_id(val: &Value) -> i64 {
    match val {
        Value::Number(n) => n.as_i64().unwrap_or(0),
        Value::String(s) => s.trim().parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

fn parse_i64_field(val: &Value) -> Option<i64> {
    match val {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

pub fn normalize_file_info_value(val: Option<&Value>) -> String {
    match val {
        Some(Value::String(s)) => {
            if s.trim().is_empty() {
                "[]".to_string()
            } else {
                s.clone()
            }
        }
        Some(other @ (Value::Array(_) | Value::Object(_))) => other.to_string(),
        _ => "[]".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_validation_accepts_valid_grouped_and_individual_and_rejects_truncated_body() {
        let grouped = r#"{"items":[{"bulk":{"table":"torrents","created":2,"updated":0,"deleted":0,"status":201}}],"current_line":3,"skipped_lines":0,"errors":false,"error":""}"#;
        assert!(validate_bulk_response(grouped, 2, Some(2)).is_ok());
        assert!(validate_bulk_response(grouped, 3, Some(3)).is_err());

        // 截断或非 JSON 即使在 HTTP 200 下也必须严格报错
        assert!(validate_bulk_response(&grouped[..grouped.len() - 10], 2, Some(2)).is_err());

        let per_op = r#"{"items":[{"replace":{"_index":"torrents","_id":1,"result":"created","status":201}},{"replace":{"_index":"torrents","_id":2,"result":"updated","status":200}}],"errors":false,"error":""}"#;
        assert!(validate_bulk_response(per_op, 2, Some(2)).is_ok());
        assert!(validate_bulk_response(per_op, 3, Some(3)).is_err());

        let grouped_update = r#"{"items":[{"bulk":{"created":0,"updated":0,"status":200}}],"current_line":101,"errors":false}"#;
        assert!(validate_bulk_response(grouped_update, 100, None).is_ok());
        let grouped_update_incomplete = r#"{"items":[{"bulk":{"created":0,"updated":0,"status":200}}],"current_line":50,"errors":false}"#;
        assert!(validate_bulk_response(grouped_update_incomplete, 100, None).is_err());

        let skipped = r#"{"items":[{"bulk":{"created":1,"updated":0,"status":200}}],"current_line":3,"skipped_lines":1,"errors":false}"#;
        assert!(validate_bulk_response(skipped, 2, Some(1)).is_err());
    }

    #[test]
    fn parse_describe_verifies_locked_columns_and_detects_missing_table() {
        let ok_json = json!([{
            "columns": [{"Field":{"type":"string"}}, {"Type":{"type":"string"}}, {"Properties":{"type":"string"}}],
            "data": [
                {"Field": "id", "Type": "bigint", "Properties": ""},
                {"Field": "info_hash", "Type": "string", "Properties": ""},
                {"Field": "raw_name", "Type": "text", "Properties": "indexed stored"},
                {"Field": "refine_name", "Type": "text", "Properties": "indexed stored"},
                {"Field": "file_count", "Type": "uint", "Properties": ""},
                {"Field": "file_info", "Type": "json", "Properties": ""},
                {"Field": "hot", "Type": "uint", "Properties": ""},
                {"Field": "created_at", "Type": "timestamp", "Properties": ""},
                {"Field": "updated_at", "Type": "timestamp", "Properties": ""}
            ],
            "total": 9,
            "error": ""
        }])
        .to_string();
        let report = parse_describe_response(&ok_json).unwrap();
        assert!(report.table_exists);
        assert!(report.compatible);
        assert!(report.missing_columns.is_empty());

        let incomplete_json = json!([{
            "data": [{"Field": "id", "Type": "bigint"}],
            "error": ""
        }])
        .to_string();
        let bad_report = parse_describe_response(&incomplete_json).unwrap();
        assert!(bad_report.table_exists);
        assert!(!bad_report.compatible);
        assert!(
            bad_report
                .missing_columns
                .contains(&"info_hash".to_string())
        );

        let missing_json = json!([{
            "error": "unknown table 'torrents'"
        }])
        .to_string();
        let missing_report = parse_describe_response(&missing_json).unwrap();
        assert!(!missing_report.table_exists);
        assert!(!missing_report.compatible);
    }

    #[test]
    fn normalizes_file_info_and_hit_id_variants() {
        let arr = json!([{"path": "movie.mkv", "size": 1024}]);
        assert_eq!(
            normalize_file_info_value(Some(&arr)),
            r#"[{"path":"movie.mkv","size":1024}]"#
        );
        let raw_str = json!(r#"[{"path":"a.mkv","size":2}]"#);
        assert_eq!(
            normalize_file_info_value(Some(&raw_str)),
            r#"[{"path":"a.mkv","size":2}]"#
        );
        assert_eq!(normalize_file_info_value(None), "[]");
        assert_eq!(normalize_file_info_value(Some(&Value::Null)), "[]");

        assert_eq!(parse_hit_id(&json!(12345)), 12345);
        assert_eq!(parse_hit_id(&json!("67890")), 67890);
    }

    #[tokio::test]
    async fn manticore_client_sends_reverse_proxy_auth_headers_and_reports_401() {
        use axum::Router;
        use axum::extract::State;
        use axum::http::{HeaderMap as AxumHeaders, StatusCode};
        use axum::response::IntoResponse;
        use axum::routing::post;
        use std::sync::{Arc, Mutex};

        let expected: Arc<Mutex<(String, String)>> = Arc::new(Mutex::new((
            "authorization".to_string(),
            format!("Basic {}", STANDARD.encode("nginx_user:nginx_pass")),
        )));

        async fn mock_sql(
            State(expected): State<Arc<Mutex<(String, String)>>>,
            headers: AxumHeaders,
        ) -> impl IntoResponse {
            let (name, val) = expected.lock().unwrap().clone();
            let got = headers
                .get(name.as_str())
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if got == val {
                let body = json!([{
                    "columns": [{"Field":{"type":"string"}}, {"Type":{"type":"string"}}, {"Properties":{"type":"string"}}],
                    "data": [
                        {"Field": "id", "Type": "bigint", "Properties": ""},
                        {"Field": "info_hash", "Type": "string", "Properties": ""},
                        {"Field": "raw_name", "Type": "text", "Properties": ""},
                        {"Field": "refine_name", "Type": "text", "Properties": ""},
                        {"Field": "file_count", "Type": "uint", "Properties": ""},
                        {"Field": "file_info", "Type": "json", "Properties": ""},
                        {"Field": "hot", "Type": "uint", "Properties": ""},
                        {"Field": "created_at", "Type": "timestamp", "Properties": ""},
                        {"Field": "updated_at", "Type": "timestamp", "Properties": ""}
                    ],
                    "error": ""
                }])
                .to_string();
                (StatusCode::OK, body)
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    "<html><body>401 Authorization Required (nginx)</body></html>".to_string(),
                )
            }
        }

        let app = Router::new()
            .route("/sql", post(mock_sql))
            .with_state(expected.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let base_url = format!("http://{addr}");

        // 1. Basic Auth
        let basic_client = ManticoreClient::new(&ManticoreServiceConfig {
            url: base_url.clone(),
            auth_mode: ManticoreAuthMode::Basic,
            username: "nginx_user".to_string(),
            password: "nginx_pass".to_string(),
            ..ManticoreServiceConfig::default()
        })
        .unwrap();
        let report = basic_client.describe_torrents().await.unwrap();
        assert!(report.table_exists && report.compatible);

        // 2. Bearer Token（传入带 "Bearer " 前缀的输入也应规整发送）
        *expected.lock().unwrap() = (
            "authorization".to_string(),
            "Bearer proxy-secret".to_string(),
        );
        let bearer_client = ManticoreClient::new(&ManticoreServiceConfig {
            url: base_url.clone(),
            auth_mode: ManticoreAuthMode::Bearer,
            bearer_token: "Bearer proxy-secret".to_string(),
            ..ManticoreServiceConfig::default()
        })
        .unwrap();
        assert!(bearer_client.describe_torrents().await.unwrap().compatible);

        // 3. 自定义请求头 API Key
        *expected.lock().unwrap() = ("x-api-key".to_string(), "k-42".to_string());
        let header_client = ManticoreClient::new(&ManticoreServiceConfig {
            url: base_url.clone(),
            auth_mode: ManticoreAuthMode::Header,
            header_name: "X-API-Key".to_string(),
            header_value: "k-42".to_string(),
            ..ManticoreServiceConfig::default()
        })
        .unwrap();
        assert!(header_client.describe_torrents().await.unwrap().compatible);

        // 4. 凭据不匹配时 HTTP 401 清晰报错
        let unauth_client = ManticoreClient::new(&ManticoreServiceConfig {
            url: base_url,
            auth_mode: ManticoreAuthMode::None,
            ..ManticoreServiceConfig::default()
        })
        .unwrap();
        let err = unauth_client.describe_torrents().await.unwrap_err();
        let err_msg = err.to_string();
        assert!(err_msg.contains("鉴权失败"), "unexpected err: {err_msg}");
        assert!(err_msg.contains("401"), "unexpected err: {err_msg}");

        server_task.abort();
    }
}
