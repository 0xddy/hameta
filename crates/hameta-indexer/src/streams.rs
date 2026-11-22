use anyhow::{Context, Result};
use deadpool_redis::{Config as RedisPoolConfig, Pool, PoolConfig, Runtime, Timeouts};
use hameta_common::{RedisServiceConfig, TorrentMessage, normalize_info_hash};
use redis::Value as RedisValue;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const XACK_CHUNK_SIZE: usize = 500;
const PIPELINE_CHUNK_SIZE: usize = 500;

#[derive(Debug, Clone)]
pub struct ParsedTorrentEntry {
    pub entry_id: String,
    pub message: TorrentMessage,
}

#[derive(Debug, Clone)]
pub struct MalformedEntry {
    pub entry_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
pub struct TorrentReadBatch {
    pub valid: Vec<ParsedTorrentEntry>,
    pub malformed: Vec<MalformedEntry>,
}

#[derive(Debug, Clone)]
pub struct ParsedHotEntry {
    pub entry_id: String,
    pub info_hash: String,
}

#[derive(Debug, Clone, Default)]
pub struct HotReadBatch {
    pub valid: Vec<ParsedHotEntry>,
    pub malformed: Vec<MalformedEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct RawStreamEntry {
    pub entry_id: String,
    pub fields: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct ReclaimTorrentBatch {
    pub valid: Vec<ParsedTorrentEntry>,
    pub ack_immediately: Vec<String>,
    pub poisoned_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ReclaimHotBatch {
    pub valid: Vec<ParsedHotEntry>,
    pub ack_immediately: Vec<String>,
    pub poisoned_count: usize,
}

/// Redis Streams 消费者与 PEL（Pending Entries List）自动接管客户端。
pub struct RedisStreamsConsumer {
    pool: Pool,
    indexed_key_prefix: String,
    torrent_stream: String,
    hot_stream: String,
    consumer_group: String,
    use_xclaim_fallback: AtomicBool,
}

impl RedisStreamsConsumer {
    pub async fn new(cfg: &RedisServiceConfig) -> Result<Self> {
        cfg.validate().context("Redis 配置无效")?;
        let url = cfg.effective_url();
        let mut pool_cfg = RedisPoolConfig::from_url(&url);
        pool_cfg.pool = Some(PoolConfig {
            max_size: cfg.indexer_pool_size.max(1),
            timeouts: Timeouts {
                wait: Some(Duration::from_secs(cfg.wait_timeout_secs.max(1))),
                create: Some(Duration::from_secs(cfg.conn_timeout_secs.max(1))),
                recycle: Some(Duration::from_secs(cfg.recycle_timeout_secs.max(1))),
            },
            ..Default::default()
        });
        let pool = pool_cfg
            .create_pool(Some(Runtime::Tokio1))
            .context("创建 Indexer Redis 连接池失败")?;

        let consumer = Self {
            pool,
            indexed_key_prefix: cfg.indexed_key_prefix.clone(),
            torrent_stream: cfg.torrent_stream.clone(),
            hot_stream: cfg.hot_stream.clone(),
            consumer_group: cfg.consumer_group.clone(),
            use_xclaim_fallback: AtomicBool::new(false),
        };
        consumer.ping().await?;
        Ok(consumer)
    }

    pub fn torrent_stream(&self) -> &str {
        &self.torrent_stream
    }

    pub fn hot_stream(&self) -> &str {
        &self.hot_stream
    }

    pub fn pool_status(&self) -> (usize, usize) {
        let st = self.pool.status();
        (st.size, st.available)
    }

    pub async fn ping(&self) -> Result<()> {
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let pong: String = redis::cmd("PING")
            .query_async(&mut conn)
            .await
            .context("Redis PING 失败")?;
        anyhow::ensure!(
            pong.eq_ignore_ascii_case("PONG"),
            "非预期 PING 响应: {pong}"
        );
        Ok(())
    }

    /// 初始化消费者组（兼容组已存在的 `BUSYGROUP`）。
    pub async fn ensure_consumer_groups(&self) -> Result<()> {
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        for stream in [&self.torrent_stream, &self.hot_stream] {
            let res: redis::RedisResult<String> = redis::cmd("XGROUP")
                .arg("CREATE")
                .arg(stream)
                .arg(&self.consumer_group)
                .arg("0")
                .arg("MKSTREAM")
                .query_async(&mut conn)
                .await;
            match res {
                Ok(_) => {}
                Err(err) if err.to_string().contains("BUSYGROUP") => {}
                Err(err) => {
                    return Err(anyhow::anyhow!(
                        "创建 Redis Stream 消费者组失败 stream={stream}, group={}: {err}",
                        self.consumer_group
                    ));
                }
            }
        }
        Ok(())
    }

    pub async fn read_torrent_batch(
        &self,
        consumer_name: &str,
        count: usize,
        block_ms: u64,
    ) -> Result<TorrentReadBatch> {
        let entries = self
            .xreadgroup_raw(&self.torrent_stream, consumer_name, count, block_ms)
            .await?;
        Ok(parse_torrent_entries(entries))
    }

    pub async fn read_hot_batch(
        &self,
        consumer_name: &str,
        count: usize,
        block_ms: u64,
    ) -> Result<HotReadBatch> {
        let entries = self
            .xreadgroup_raw(&self.hot_stream, consumer_name, count, block_ms)
            .await?;
        Ok(parse_hot_entries(entries))
    }

    async fn xreadgroup_raw(
        &self,
        stream: &str,
        consumer_name: &str,
        count: usize,
        block_ms: u64,
    ) -> Result<Vec<RawStreamEntry>> {
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let val: RedisValue = redis::cmd("XREADGROUP")
            .arg("GROUP")
            .arg(&self.consumer_group)
            .arg(consumer_name)
            .arg("COUNT")
            .arg(count.max(1))
            .arg("BLOCK")
            .arg(block_ms)
            .arg("STREAMS")
            .arg(stream)
            .arg(">")
            .query_async(&mut conn)
            .await
            .with_context(|| format!("执行 XREADGROUP 失败 stream={stream}"))?;

        parse_xreadgroup_reply(val)
    }

    pub async fn xack_batch(&self, stream: &str, entry_ids: &[String]) -> Result<usize> {
        if entry_ids.is_empty() {
            return Ok(0);
        }
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let mut acked = 0_usize;
        for chunk in entry_ids.chunks(XACK_CHUNK_SIZE) {
            let mut cmd = redis::cmd("XACK");
            cmd.arg(stream).arg(&self.consumer_group);
            for id in chunk {
                cmd.arg(id);
            }
            let n: usize = cmd
                .query_async(&mut conn)
                .await
                .with_context(|| format!("执行 XACK 失败 stream={stream}"))?;
            acked += n;
        }
        Ok(acked)
    }

    /// 写入存量兼容已索引键 `<indexed_key_prefix><hash> = "1"`（无 TTL），并清除 pending 租约。
    pub async fn mark_indexed_and_clear_pending(&self, info_hashes: &[String]) -> Result<usize> {
        if info_hashes.is_empty() {
            return Ok(0);
        }
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        for chunk in info_hashes.chunks(PIPELINE_CHUNK_SIZE) {
            let mut pipe = redis::pipe();
            for hash in chunk {
                let indexed_key = format!("{}{hash}", self.indexed_key_prefix);
                let pending_key = format!("hameta:lock:pending:{hash}");
                pipe.set(&indexed_key, "1").ignore();
                pipe.del(&pending_key).ignore();
            }
            pipe.query_async::<()>(&mut *conn)
                .await
                .context("批量置位 Redis metadata:indexed 标记失败")?;
        }
        Ok(info_hashes.len())
    }

    /// 回收空闲超时未确认的 `torrent_stream` PEL 消息，并对超出最大投递次数的毒丸执行识别。
    pub async fn reclaim_stale_torrents(
        &self,
        consumer_name: &str,
        min_idle_ms: u64,
        count: usize,
        max_delivery_attempts: usize,
    ) -> Result<ReclaimTorrentBatch> {
        let claimed = self
            .reclaim_raw_entries(&self.torrent_stream, consumer_name, min_idle_ms, count)
            .await?;
        if claimed.is_empty() {
            return Ok(ReclaimTorrentBatch::default());
        }

        let deliveries = self
            .query_delivery_counts(&self.torrent_stream, &claimed)
            .await
            .unwrap_or_default();

        let mut out = ReclaimTorrentBatch::default();
        let mut remaining = Vec::new();
        for entry in claimed {
            let count = deliveries.get(&entry.entry_id).copied().unwrap_or(1);
            if count > max_delivery_attempts {
                tracing::error!(
                    "检测到新种 Stream 毒丸消息并强制确认清理: stream={}, entry_id={}, delivery_count={count}, limit={max_delivery_attempts}",
                    self.torrent_stream,
                    entry.entry_id
                );
                out.ack_immediately.push(entry.entry_id);
                out.poisoned_count += 1;
            } else {
                remaining.push(entry);
            }
        }

        let parsed = parse_torrent_entries(remaining);
        out.valid = parsed.valid;
        for mal in parsed.malformed {
            tracing::error!(
                "回收的 Stream 新种消息损坏，立即确认丢弃: entry_id={}, reason={}",
                mal.entry_id,
                mal.reason
            );
            out.ack_immediately.push(mal.entry_id);
        }
        Ok(out)
    }

    /// 回收空闲超时未确认的 `hot_stream` PEL 消息。
    pub async fn reclaim_stale_hot(
        &self,
        consumer_name: &str,
        min_idle_ms: u64,
        count: usize,
        max_delivery_attempts: usize,
    ) -> Result<ReclaimHotBatch> {
        let claimed = self
            .reclaim_raw_entries(&self.hot_stream, consumer_name, min_idle_ms, count)
            .await?;
        if claimed.is_empty() {
            return Ok(ReclaimHotBatch::default());
        }

        let deliveries = self
            .query_delivery_counts(&self.hot_stream, &claimed)
            .await
            .unwrap_or_default();

        let mut out = ReclaimHotBatch::default();
        let mut remaining = Vec::new();
        for entry in claimed {
            let count = deliveries.get(&entry.entry_id).copied().unwrap_or(1);
            if count > max_delivery_attempts {
                tracing::error!(
                    "检测到热点 Stream 毒丸消息并强制确认清理: stream={}, entry_id={}, delivery_count={count}",
                    self.hot_stream,
                    entry.entry_id
                );
                out.ack_immediately.push(entry.entry_id);
                out.poisoned_count += 1;
            } else {
                remaining.push(entry);
            }
        }

        let parsed = parse_hot_entries(remaining);
        out.valid = parsed.valid;
        for mal in parsed.malformed {
            tracing::error!(
                "回收的 Stream 热点消息损坏，立即确认丢弃: entry_id={}, reason={}",
                mal.entry_id,
                mal.reason
            );
            out.ack_immediately.push(mal.entry_id);
        }
        Ok(out)
    }

    async fn reclaim_raw_entries(
        &self,
        stream: &str,
        consumer_name: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<RawStreamEntry>> {
        if !self.use_xclaim_fallback.load(Ordering::Relaxed) {
            let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
            let res: redis::RedisResult<RedisValue> = redis::cmd("XAUTOCLAIM")
                .arg(stream)
                .arg(&self.consumer_group)
                .arg(consumer_name)
                .arg(min_idle_ms)
                .arg("0-0")
                .arg("COUNT")
                .arg(count.max(1))
                .query_async(&mut conn)
                .await;

            match res {
                Ok(val) => return parse_xautoclaim_reply(val),
                Err(err) => {
                    let msg = err.to_string().to_ascii_lowercase();
                    if msg.contains("unknown command") || msg.contains("err unknown") {
                        tracing::info!(
                            "Redis 服务端不支持 XAUTOCLAIM（< 6.2），已切换 XPENDING+XCLAIM 兼容模式"
                        );
                        self.use_xclaim_fallback.store(true, Ordering::Relaxed);
                    } else {
                        return Err(anyhow::anyhow!(
                            "执行 XAUTOCLAIM 失败 stream={stream}: {err}"
                        ));
                    }
                }
            }
        }

        self.reclaim_via_xpending_xclaim(stream, consumer_name, min_idle_ms, count)
            .await
    }

    async fn reclaim_via_xpending_xclaim(
        &self,
        stream: &str,
        consumer_name: &str,
        min_idle_ms: u64,
        count: usize,
    ) -> Result<Vec<RawStreamEntry>> {
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let pending_val: RedisValue = redis::cmd("XPENDING")
            .arg(stream)
            .arg(&self.consumer_group)
            .arg("-")
            .arg("+")
            .arg(count.max(1))
            .query_async(&mut conn)
            .await
            .with_context(|| format!("执行 XPENDING 失败 stream={stream}"))?;

        let items = parse_xpending_rows(pending_val)?;
        let stale_ids: Vec<String> = items
            .into_iter()
            .filter(|row| row.idle_ms >= min_idle_ms)
            .map(|row| row.entry_id)
            .collect();
        if stale_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut cmd = redis::cmd("XCLAIM");
        cmd.arg(stream)
            .arg(&self.consumer_group)
            .arg(consumer_name)
            .arg(min_idle_ms);
        for id in &stale_ids {
            cmd.arg(id);
        }
        let claim_val: RedisValue = cmd
            .query_async(&mut conn)
            .await
            .with_context(|| format!("执行 XCLAIM 失败 stream={stream}"))?;
        parse_stream_entry_list(claim_val)
    }

    async fn query_delivery_counts(
        &self,
        stream: &str,
        entries: &[RawStreamEntry],
    ) -> Result<HashMap<String, usize>> {
        let Some(first) = entries.first() else {
            return Ok(HashMap::new());
        };
        let Some(last) = entries.last() else {
            return Ok(HashMap::new());
        };
        let mut conn = self.pool.get().await?;
        let val: RedisValue = redis::cmd("XPENDING")
            .arg(stream)
            .arg(&self.consumer_group)
            .arg(&first.entry_id)
            .arg(&last.entry_id)
            .arg(entries.len().max(1))
            .query_async(&mut conn)
            .await?;
        let rows = parse_xpending_rows(val)?;
        Ok(rows
            .into_iter()
            .map(|r| (r.entry_id, r.deliveries))
            .collect())
    }
}

#[derive(Debug, Clone)]
struct PendingRow {
    entry_id: String,
    idle_ms: u64,
    deliveries: usize,
}

fn redis_value_to_string(val: RedisValue) -> Option<String> {
    match val {
        RedisValue::BulkString(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        RedisValue::SimpleString(s) => Some(s),
        RedisValue::Int(n) => Some(n.to_string()),
        _ => None,
    }
}

fn redis_value_to_u64(val: RedisValue) -> Option<u64> {
    match val {
        RedisValue::Int(n) => u64::try_from(n).ok(),
        RedisValue::BulkString(bytes) => String::from_utf8_lossy(&bytes).parse::<u64>().ok(),
        RedisValue::SimpleString(s) => s.parse::<u64>().ok(),
        _ => None,
    }
}

fn parse_fields_pairs(val: RedisValue) -> HashMap<String, String> {
    let mut map = HashMap::new();
    match val {
        RedisValue::Array(items) => {
            let mut iter = items.into_iter();
            while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                if let (Some(ks), Some(vs)) = (redis_value_to_string(k), redis_value_to_string(v)) {
                    map.insert(ks, vs);
                }
            }
        }
        RedisValue::Map(pairs) => {
            for (k, v) in pairs {
                if let (Some(ks), Some(vs)) = (redis_value_to_string(k), redis_value_to_string(v)) {
                    map.insert(ks, vs);
                }
            }
        }
        _ => {}
    }
    map
}

fn parse_stream_entry(val: RedisValue) -> Option<RawStreamEntry> {
    let RedisValue::Array(mut parts) = val else {
        return None;
    };
    if parts.len() < 2 {
        return None;
    }
    let fields_val = parts.remove(1);
    let id_val = parts.remove(0);
    let entry_id = redis_value_to_string(id_val)?;
    let fields = parse_fields_pairs(fields_val);
    Some(RawStreamEntry { entry_id, fields })
}

fn parse_stream_entry_list(val: RedisValue) -> Result<Vec<RawStreamEntry>> {
    match val {
        RedisValue::Nil => Ok(Vec::new()),
        RedisValue::Array(items) => Ok(items.into_iter().filter_map(parse_stream_entry).collect()),
        other => anyhow::bail!("非预期的 Stream 条目数组类型: {other:?}"),
    }
}

/// 解析 `XREADGROUP` 响应（兼容 RESP2 Array 与 RESP3 Map，超时无数据时返回空 Vec）。
pub fn parse_xreadgroup_reply(val: RedisValue) -> Result<Vec<RawStreamEntry>> {
    match val {
        RedisValue::Nil => Ok(Vec::new()),
        RedisValue::Array(streams) => {
            let mut out = Vec::new();
            for stream_item in streams {
                if let RedisValue::Array(mut pair) = stream_item
                    && pair.len() >= 2
                {
                    let entries_val = pair.remove(1);
                    out.extend(parse_stream_entry_list(entries_val)?);
                }
            }
            Ok(out)
        }
        RedisValue::Map(streams) => {
            let mut out = Vec::new();
            for (_, entries_val) in streams {
                out.extend(parse_stream_entry_list(entries_val)?);
            }
            Ok(out)
        }
        other => anyhow::bail!("非预期的 XREADGROUP 响应类型: {other:?}"),
    }
}

/// 解析 `XAUTOCLAIM` 响应（兼容 Redis 6.2 两元素 `[next_id, entries]` 与 Redis 7+ 三元素 `[next_id, entries, deleted]`）。
pub fn parse_xautoclaim_reply(val: RedisValue) -> Result<Vec<RawStreamEntry>> {
    let RedisValue::Array(mut parts) = val else {
        anyhow::bail!("XAUTOCLAIM 响应顶层必须是数组");
    };
    if parts.len() < 2 {
        return Ok(Vec::new());
    }
    let entries_val = parts.remove(1);
    parse_stream_entry_list(entries_val)
}

fn parse_xpending_rows(val: RedisValue) -> Result<Vec<PendingRow>> {
    let RedisValue::Array(rows) = val else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if let RedisValue::Array(mut cols) = row
            && cols.len() >= 4
        {
            let deliveries = redis_value_to_u64(cols.remove(3))
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(1);
            let idle_ms = redis_value_to_u64(cols.remove(2)).unwrap_or(0);
            let _consumer = cols.remove(1);
            if let Some(entry_id) = redis_value_to_string(cols.remove(0)) {
                out.push(PendingRow {
                    entry_id,
                    idle_ms,
                    deliveries,
                });
            }
        }
    }
    Ok(out)
}

pub fn parse_torrent_entries(entries: Vec<RawStreamEntry>) -> TorrentReadBatch {
    let mut batch = TorrentReadBatch::default();
    for entry in entries {
        let Some(payload) = entry.fields.get("payload") else {
            batch.malformed.push(MalformedEntry {
                entry_id: entry.entry_id,
                reason: "缺少 payload 字段".to_string(),
            });
            continue;
        };
        match serde_json::from_str::<TorrentMessage>(payload) {
            Ok(mut msg) => match normalize_info_hash(&msg.info_hash) {
                Ok(norm) => {
                    msg.info_hash = norm;
                    batch.valid.push(ParsedTorrentEntry {
                        entry_id: entry.entry_id,
                        message: msg,
                    });
                }
                Err(err) => {
                    batch.malformed.push(MalformedEntry {
                        entry_id: entry.entry_id,
                        reason: format!("info_hash 格式无效: {err}"),
                    });
                }
            },
            Err(err) => {
                batch.malformed.push(MalformedEntry {
                    entry_id: entry.entry_id,
                    reason: format!("payload JSON 反序列化失败: {err}"),
                });
            }
        }
    }
    batch
}

pub fn parse_hot_entries(entries: Vec<RawStreamEntry>) -> HotReadBatch {
    let mut batch = HotReadBatch::default();
    for entry in entries {
        let Some(raw_hash) = entry.fields.get("info_hash") else {
            batch.malformed.push(MalformedEntry {
                entry_id: entry.entry_id,
                reason: "热点消息缺少 info_hash 字段".to_string(),
            });
            continue;
        };
        match normalize_info_hash(raw_hash) {
            Ok(info_hash) => {
                batch.valid.push(ParsedHotEntry {
                    entry_id: entry.entry_id,
                    info_hash,
                });
            }
            Err(err) => {
                batch.malformed.push(MalformedEntry {
                    entry_id: entry.entry_id,
                    reason: format!("热点消息 info_hash 无效: {err}"),
                });
            }
        }
    }
    batch
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xreadgroup_and_xautoclaim_replies_including_redis_6_and_7_shapes() {
        let payload = serde_json::json!({
            "info_hash": "0123456789ABCDEF0123456789ABCDEF01234567",
            "name": "sample.mkv",
            "total_size": 1000,
            "file_count": 1,
            "files": [{"path": "sample.mkv", "size": 1000}],
            "timestamp": 1700000000
        })
        .to_string();

        let reply = RedisValue::Array(vec![RedisValue::Array(vec![
            RedisValue::BulkString(b"hameta:stream:torrent:new".to_vec()),
            RedisValue::Array(vec![
                RedisValue::Array(vec![
                    RedisValue::BulkString(b"1700000000000-0".to_vec()),
                    RedisValue::Array(vec![
                        RedisValue::BulkString(b"info_hash".to_vec()),
                        RedisValue::BulkString(
                            b"0123456789abcdef0123456789abcdef01234567".to_vec(),
                        ),
                        RedisValue::BulkString(b"payload".to_vec()),
                        RedisValue::BulkString(payload.into_bytes()),
                    ]),
                ]),
                RedisValue::Array(vec![
                    RedisValue::BulkString(b"1700000000001-0".to_vec()),
                    RedisValue::Array(vec![
                        RedisValue::BulkString(b"payload".to_vec()),
                        RedisValue::BulkString(b"{broken-json".to_vec()),
                    ]),
                ]),
            ]),
        ])]);

        let raw = parse_xreadgroup_reply(reply).unwrap();
        let batch = parse_torrent_entries(raw);
        assert_eq!(batch.valid.len(), 1);
        assert_eq!(
            batch.valid[0].message.info_hash,
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(batch.malformed.len(), 1);
        assert_eq!(batch.malformed[0].entry_id, "1700000000001-0");

        // Redis 6.2 XAUTOCLAIM (2 items) & Redis 7 XAUTOCLAIM (3 items)
        let r6 = RedisValue::Array(vec![
            RedisValue::BulkString(b"0-0".to_vec()),
            RedisValue::Array(vec![RedisValue::Array(vec![
                RedisValue::BulkString(b"1-0".to_vec()),
                RedisValue::Array(vec![
                    RedisValue::BulkString(b"info_hash".to_vec()),
                    RedisValue::BulkString(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_vec()),
                ]),
            ])]),
        ]);
        let r7 = RedisValue::Array(vec![
            RedisValue::BulkString(b"0-0".to_vec()),
            RedisValue::Array(vec![RedisValue::Array(vec![
                RedisValue::BulkString(b"2-0".to_vec()),
                RedisValue::Array(vec![
                    RedisValue::BulkString(b"info_hash".to_vec()),
                    RedisValue::BulkString(b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_vec()),
                ]),
            ])]),
            RedisValue::Array(vec![]),
        ]);
        assert_eq!(parse_xautoclaim_reply(r6).unwrap().len(), 1);
        assert_eq!(parse_xautoclaim_reply(r7).unwrap().len(), 1);
    }
}
