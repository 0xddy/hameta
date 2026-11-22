use anyhow::{Context, Result};
use deadpool_redis::{Config as RedisPoolConfig, Pool, PoolConfig, Runtime, Timeouts};
use dht_crawler::prelude::TorrentInfo;
use hameta_common::{FileInfo, RedisServiceConfig, TorrentMessage};
use redis::Script;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const POLICY_REJECTION_TTL_SECS: u64 = 24 * 60 * 60;
const RELEASE_BATCH_LIMIT: usize = 64;
static CLAIM_SEQUENCE: AtomicU64 = AtomicU64::new(0);

static CLAIM_METADATA_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::new(
        r#"
        if redis.call('EXISTS', KEYS[1]) == 1 then
            return {1, 0}
        end
        if redis.call('GET', KEYS[3]) == ARGV[3] then
            return {3, redis.call('PTTL', KEYS[3])}
        end
        if redis.call('SET', KEYS[2], ARGV[2], 'NX', 'EX', ARGV[1]) then
            return {0, 0}
        end
        return {2, redis.call('PTTL', KEYS[2])}
        "#,
    )
});

static PUBLISH_TORRENT_STREAM_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::new(
        r#"
        local function kind(key) return redis.call('TYPE', key).ok end
        local stream_type = kind(KEYS[1])
        if stream_type ~= 'none' and stream_type ~= 'stream' then
            return redis.error_reply('torrent stream key has wrong type: ' .. stream_type)
        end
        for i = 2, 4 do
            local t = kind(KEYS[i])
            if t ~= 'none' and t ~= 'string' then
                return redis.error_reply('metadata lock key has wrong type: ' .. t)
            end
        end
        if redis.call('GET', KEYS[4]) == ARGV[1] then
            return 2
        end
        if redis.call('EXISTS', KEYS[3]) == 1 then
            if redis.call('GET', KEYS[2]) == ARGV[1] then
                redis.call('DEL', KEYS[2])
            end
            return 3
        end
        local owner = redis.call('GET', KEYS[2])
        if (owner and owner ~= ARGV[1]) or (not owner and ARGV[6] == '1') then
            return 4
        end
        redis.call('XADD', KEYS[1], 'MAXLEN', '~', ARGV[5], '*', 'info_hash', ARGV[2], 'payload', ARGV[3])
        redis.call('SET', KEYS[4], ARGV[1], 'EX', ARGV[7])
        redis.call('SET', KEYS[2], ARGV[1], 'EX', ARGV[4])
        return 1
        "#,
    )
});

static RELEASE_CLAIMS_BATCH_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::new(
        r#"
        local released = 0
        for i, key in ipairs(KEYS) do
            if redis.call('GET', key) == ARGV[i + 1] then
                if tonumber(ARGV[1]) <= 0 then
                    redis.call('DEL', key)
                else
                    redis.call('PEXPIRE', key, ARGV[1])
                end
                released = released + 1
            end
        end
        return released
        "#,
    )
});

static REJECT_OVERSIZE_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::new(
        r#"
        if redis.call('EXISTS', KEYS[3]) == 1 then
            if redis.call('GET', KEYS[1]) == ARGV[1] then
                redis.call('DEL', KEYS[1])
            end
            return 2
        end
        local owner = redis.call('GET', KEYS[1])
        if (owner and owner ~= ARGV[1]) or (not owner and ARGV[4] == '1') then
            return 0
        end
        redis.call('SET', KEYS[2], ARGV[2], 'EX', ARGV[3])
        if owner == ARGV[1] then
            redis.call('DEL', KEYS[1])
        end
        return 1
        "#,
    )
});

#[derive(Debug, Clone)]
pub struct MetadataClaim {
    pub token: String,
    pub redis_claimed: bool,
}

impl MetadataClaim {
    pub fn new(redis_claimed: bool) -> Self {
        let seq = CLAIM_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let nonce = RandomState::new().hash_one((now, seq));
        Self {
            token: format!("{}-{now:x}-{nonce:x}-{seq:x}", std::process::id()),
            redis_claimed,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ClaimDecision {
    Acquired(MetadataClaim),
    AlreadyIndexed,
    Pending { retry_after: Duration },
    PolicyRejected { retry_after: Duration },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    Submitted,
    DuplicateDelivery,
    AlreadyIndexed,
    ClaimLost,
    OversizeRejected,
}

/// 面向采集 Worker 的 Redis Streams + Fencing 锁客户端。
pub struct RedisStreamsClient {
    pool: Pool,
    indexed_key_prefix: String,
    torrent_stream: String,
    hot_stream: String,
    max_stream_len: usize,
    max_message_size: usize,
    pending_ttl_secs: u64,
    failure_backoff_secs: u64,
}

impl RedisStreamsClient {
    pub async fn new(
        redis_cfg: &RedisServiceConfig,
        pending_ttl_secs: u64,
        failure_backoff_secs: u64,
    ) -> Result<Self> {
        redis_cfg.validate().context("Redis 配置无效")?;
        let url = redis_cfg.effective_url();
        let mut pool_cfg = RedisPoolConfig::from_url(&url);
        pool_cfg.pool = Some(PoolConfig {
            max_size: redis_cfg.crawler_pool_size.max(1),
            timeouts: Timeouts {
                wait: Some(Duration::from_secs(redis_cfg.wait_timeout_secs.max(1))),
                create: Some(Duration::from_secs(redis_cfg.conn_timeout_secs.max(1))),
                recycle: Some(Duration::from_secs(redis_cfg.recycle_timeout_secs.max(1))),
            },
            ..Default::default()
        });

        let pool = pool_cfg
            .create_pool(Some(Runtime::Tokio1))
            .context("创建 Crawler Redis 连接池失败")?;

        let client = Self {
            pool,
            indexed_key_prefix: redis_cfg.indexed_key_prefix.clone(),
            torrent_stream: redis_cfg.torrent_stream.clone(),
            hot_stream: redis_cfg.hot_stream.clone(),
            max_stream_len: redis_cfg.max_stream_len.max(1),
            max_message_size: redis_cfg.max_message_size.max(1024),
            pending_ttl_secs: pending_ttl_secs.max(1),
            failure_backoff_secs,
        };
        client
            .ping()
            .await
            .context("Crawler 启动前 Redis 连通性检查失败")?;
        Ok(client)
    }

    pub fn indexed_key(&self, info_hash: &str) -> String {
        format!("{}{info_hash}", self.indexed_key_prefix)
    }

    pub fn pending_key(info_hash: &str) -> String {
        format!("hameta:lock:pending:{info_hash}")
    }

    pub fn rejected_key(info_hash: &str) -> String {
        format!("hameta:lock:rejected:{info_hash}")
    }

    pub fn delivery_key(info_hash: &str, token: &str) -> String {
        format!("hameta:lock:delivery:{info_hash}:{token}")
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
            "非预期 Redis PING 响应: {pong}"
        );
        Ok(())
    }

    pub async fn claim_metadata(
        &self,
        info_hash: &str,
        claim: MetadataClaim,
    ) -> Result<ClaimDecision> {
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let (code, pttl_ms): (i64, i64) = CLAIM_METADATA_SCRIPT
            .key(self.indexed_key(info_hash))
            .key(Self::pending_key(info_hash))
            .key(Self::rejected_key(info_hash))
            .arg(self.pending_ttl_secs)
            .arg(&claim.token)
            .arg("oversize")
            .invoke_async(&mut conn)
            .await
            .context("执行 CLAIM_METADATA_SCRIPT 失败")?;

        match code {
            0 => Ok(ClaimDecision::Acquired(MetadataClaim {
                token: claim.token,
                redis_claimed: true,
            })),
            1 => Ok(ClaimDecision::AlreadyIndexed),
            2 => {
                let wait_ms = u64::try_from(pttl_ms).unwrap_or(5_000).clamp(500, 60_000);
                Ok(ClaimDecision::Pending {
                    retry_after: Duration::from_millis(wait_ms),
                })
            }
            3 => {
                let wait_ms = u64::try_from(pttl_ms)
                    .unwrap_or(60_000)
                    .clamp(5_000, POLICY_REJECTION_TTL_SECS * 1000);
                Ok(ClaimDecision::PolicyRejected {
                    retry_after: Duration::from_millis(wait_ms),
                })
            }
            other => anyhow::bail!("CLAIM_METADATA_SCRIPT 返回未知状态码: {other}"),
        }
    }

    pub async fn push_torrent(
        &self,
        torrent: &TorrentInfo,
        claim: &MetadataClaim,
    ) -> Result<PublishOutcome> {
        let payload_json = match serialize_torrent_message(torrent, self.max_message_size)? {
            SerializedTorrentPayload::Ready(json) => json,
            SerializedTorrentPayload::Oversize { .. } => {
                let _ = self
                    .reject_oversize(&torrent.info_hash, claim)
                    .await
                    .context("写入超大消息拒绝锁失败")?;
                return Ok(PublishOutcome::OversizeRejected);
            }
        };

        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let delivery_ttl = self.pending_ttl_secs.clamp(300, 86_400);
        let require_claim = if claim.redis_claimed { "1" } else { "0" };

        let code: i64 = PUBLISH_TORRENT_STREAM_SCRIPT
            .key(&self.torrent_stream)
            .key(Self::pending_key(&torrent.info_hash))
            .key(self.indexed_key(&torrent.info_hash))
            .key(Self::delivery_key(&torrent.info_hash, &claim.token))
            .arg(&claim.token)
            .arg(&torrent.info_hash)
            .arg(&payload_json)
            .arg(self.pending_ttl_secs)
            .arg(self.max_stream_len)
            .arg(require_claim)
            .arg(delivery_ttl)
            .invoke_async(&mut conn)
            .await
            .context("执行 PUBLISH_TORRENT_STREAM_SCRIPT 失败")?;

        match code {
            1 => Ok(PublishOutcome::Submitted),
            2 => Ok(PublishOutcome::DuplicateDelivery),
            3 => Ok(PublishOutcome::AlreadyIndexed),
            4 => Ok(PublishOutcome::ClaimLost),
            other => anyhow::bail!("PUBLISH_TORRENT_STREAM_SCRIPT 返回未知状态码: {other}"),
        }
    }

    pub async fn reject_oversize(&self, info_hash: &str, claim: &MetadataClaim) -> Result<i64> {
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let require_claim = if claim.redis_claimed { "1" } else { "0" };
        let code: i64 = REJECT_OVERSIZE_SCRIPT
            .key(Self::pending_key(info_hash))
            .key(Self::rejected_key(info_hash))
            .key(self.indexed_key(info_hash))
            .arg(&claim.token)
            .arg("oversize")
            .arg(POLICY_REJECTION_TTL_SECS)
            .arg(require_claim)
            .invoke_async(&mut conn)
            .await
            .context("执行 REJECT_OVERSIZE_SCRIPT 失败")?;
        Ok(code)
    }

    pub async fn defer_failed_claim(&self, info_hash: &str, claim: &MetadataClaim) -> Result<()> {
        if !claim.redis_claimed {
            return Ok(());
        }
        let backoff_ms = self.failure_backoff_secs.saturating_mul(1000);
        self.release_claims_with_ttl(&[(info_hash.to_string(), claim.clone())], backoff_ms)
            .await?;
        Ok(())
    }

    pub async fn release_claims(&self, claims: &[(String, MetadataClaim)]) -> Result<usize> {
        self.release_claims_with_ttl(claims, 0).await
    }

    async fn release_claims_with_ttl(
        &self,
        claims: &[(String, MetadataClaim)],
        pexpire_ms: u64,
    ) -> Result<usize> {
        let owned: Vec<_> = claims.iter().filter(|(_, c)| c.redis_claimed).collect();
        if owned.is_empty() {
            return Ok(0);
        }

        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let mut total_released = 0_usize;

        for chunk in owned.chunks(RELEASE_BATCH_LIMIT) {
            let mut inv = RELEASE_CLAIMS_BATCH_SCRIPT.prepare_invoke();
            for (hash, _) in chunk {
                inv.key(Self::pending_key(hash));
            }
            inv.arg(pexpire_ms);
            for (_, claim) in chunk {
                inv.arg(&claim.token);
            }
            let released: usize = inv
                .invoke_async(&mut conn)
                .await
                .context("执行 RELEASE_CLAIMS_BATCH_SCRIPT 失败")?;
            total_released += released;
        }
        Ok(total_released)
    }

    pub async fn push_hot(&self, info_hash: &str, timestamp: u64) -> Result<()> {
        let mut conn = self.pool.get().await.context("获取 Redis 连接失败")?;
        let _id: String = redis::cmd("XADD")
            .arg(&self.hot_stream)
            .arg("MAXLEN")
            .arg("~")
            .arg(self.max_stream_len)
            .arg("*")
            .arg("info_hash")
            .arg(info_hash)
            .arg("timestamp")
            .arg(timestamp.to_string())
            .query_async(&mut conn)
            .await
            .context("写入热点 Stream 失败")?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializedTorrentPayload {
    Ready(String),
    Oversize { bytes: usize, limit: usize },
}

pub fn to_torrent_message(torrent: &TorrentInfo) -> TorrentMessage {
    TorrentMessage {
        info_hash: torrent.info_hash.clone(),
        name: torrent.name.clone(),
        total_size: torrent.total_size,
        file_count: torrent.files.len(),
        files: torrent
            .files
            .iter()
            .map(|f| FileInfo {
                path: f.path.clone(),
                size: f.size,
            })
            .collect(),
        timestamp: torrent.timestamp,
    }
}

pub fn serialize_torrent_message(
    torrent: &TorrentInfo,
    max_message_size: usize,
) -> Result<SerializedTorrentPayload> {
    let msg = to_torrent_message(torrent);
    let json = serde_json::to_string(&msg).context("序列化 TorrentMessage 失败")?;
    if json.len() > max_message_size {
        return Ok(SerializedTorrentPayload::Oversize {
            bytes: json.len(),
            limit: max_message_size,
        });
    }
    Ok(SerializedTorrentPayload::Ready(json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dht_crawler::prelude::FileInfo as DhtFileInfo;

    #[test]
    fn redis_key_namespaces_preserve_legacy_indexed_prefix() {
        let cfg = RedisServiceConfig::default();
        assert_eq!(cfg.indexed_key_prefix, "metadata:indexed:");
        let hash = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            format!("{}{hash}", cfg.indexed_key_prefix),
            "metadata:indexed:0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(
            RedisStreamsClient::pending_key(hash),
            "hameta:lock:pending:0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(
            RedisStreamsClient::rejected_key(hash),
            "hameta:lock:rejected:0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(
            RedisStreamsClient::delivery_key(hash, "tok-1"),
            "hameta:lock:delivery:0123456789abcdef0123456789abcdef01234567:tok-1"
        );
    }

    #[test]
    fn serializes_torrent_message_and_enforces_size_limit() {
        let torrent = TorrentInfo {
            info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
            magnet_link: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_string(),
            name: "ubuntu-24.04-desktop-amd64.iso".to_string(),
            total_size: 4_000_000_000,
            files: vec![DhtFileInfo {
                path: "ubuntu-24.04-desktop-amd64.iso".to_string(),
                size: 4_000_000_000,
            }],
            piece_length: 262_144,
            peers: vec!["127.0.0.1:6881".to_string()],
            timestamp: 1_700_000_000,
        };

        let ready = serialize_torrent_message(&torrent, 1_048_576).unwrap();
        match ready {
            SerializedTorrentPayload::Ready(json) => {
                let parsed: TorrentMessage = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed.info_hash, torrent.info_hash);
                assert_eq!(parsed.file_count, 1);
            }
            other => panic!("expected Ready, got {other:?}"),
        }

        let oversize = serialize_torrent_message(&torrent, 64).unwrap();
        assert!(matches!(
            oversize,
            SerializedTorrentPayload::Oversize { .. }
        ));
    }
}
