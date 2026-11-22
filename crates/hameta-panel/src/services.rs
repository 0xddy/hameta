use deadpool_redis::{Config as RedisPoolConfig, Pool, PoolConfig, Runtime, Timeouts};
use hameta_common::{ManticoreServiceConfig, RedisServiceConfig, ServiceKind, generate_token};
use hameta_indexer::{ManticoreClient, SchemaCheckReport, TableStatusInfo};
use hameta_storage::Database;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisTestReport {
    pub ok: bool,
    pub latency_ms: u64,
    pub redis_version: Option<String>,
    pub streams_supported: bool,
    pub xautoclaim_supported: bool,
    pub used_memory_human: Option<String>,
    pub dbsize: Option<u64>,
    pub rw_probe_ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManticoreTestReport {
    pub ok: bool,
    pub latency_ms: u64,
    pub schema: Option<SchemaCheckReport>,
    pub status: Option<TableStatusInfo>,
    pub message: String,
}

///  Panel 进程自身持有的 Redis 与 Manticore 运行时客户端池，支持服务配置保存/回滚后无缝热重建。
#[derive(Default)]
pub struct ServiceRuntimePool {
    redis: RwLock<Option<(RedisServiceConfig, Pool)>>,
    manticore: RwLock<Option<(ManticoreServiceConfig, ManticoreClient)>>,
}

impl ServiceRuntimePool {
    pub async fn init_from_db(&self, db: &Database) {
        if let Ok(Some(row)) = db.get_active_service_config(ServiceKind::Redis).await
            && let Ok(cfg) = serde_json::from_str::<RedisServiceConfig>(&row.config_json)
            && let Err(err) = self.reload_redis(&cfg).await
        {
            tracing::warn!("Panel 启动初始化 Redis 连接池失败: {err}");
        }

        if let Ok(Some(row)) = db.get_active_service_config(ServiceKind::Manticore).await
            && let Ok(cfg) = serde_json::from_str::<ManticoreServiceConfig>(&row.config_json)
            && let Err(err) = self.reload_manticore(&cfg).await
        {
            tracing::warn!("Panel 启动初始化 Manticore 客户端失败: {err}");
        }
    }

    pub async fn reload_redis(&self, cfg: &RedisServiceConfig) -> anyhow::Result<()> {
        cfg.validate()?;
        let pool = build_redis_pool(cfg)?;
        let mut guard = self.redis.write().await;
        *guard = Some((cfg.clone(), pool));
        Ok(())
    }

    pub async fn reload_manticore(&self, cfg: &ManticoreServiceConfig) -> anyhow::Result<()> {
        cfg.validate()?;
        let client = ManticoreClient::new(cfg)?;
        let mut guard = self.manticore.write().await;
        *guard = Some((cfg.clone(), client));
        Ok(())
    }

    pub async fn redis_pool(&self) -> Option<(RedisServiceConfig, Pool)> {
        self.redis.read().await.clone()
    }

    pub async fn manticore_client(&self) -> Option<(ManticoreServiceConfig, ManticoreClient)> {
        self.manticore.read().await.clone()
    }
}

pub fn build_redis_pool(cfg: &RedisServiceConfig) -> anyhow::Result<Pool> {
    let url = cfg.effective_url();
    let mut pool_cfg = RedisPoolConfig::from_url(&url);
    pool_cfg.pool = Some(PoolConfig {
        max_size: cfg.indexer_pool_size.clamp(4, 64),
        timeouts: Timeouts {
            wait: Some(Duration::from_secs(cfg.wait_timeout_secs.max(1))),
            create: Some(Duration::from_secs(cfg.conn_timeout_secs.max(1))),
            recycle: Some(Duration::from_secs(cfg.recycle_timeout_secs.max(1))),
        },
        ..Default::default()
    });
    pool_cfg.create_pool(Some(Runtime::Tokio1)).map_err(|e| {
        anyhow::anyhow!(
            "创建 Redis 连接池失败: {}",
            sanitize_secret_in_text(&e.to_string())
        )
    })
}

/// 实测 Redis 连通性、延迟、版本号（Stream/XAUTOCLAIM 兼容性）、内存占用及读写探测。
pub async fn test_redis_connection(cfg: &RedisServiceConfig) -> RedisTestReport {
    if let Err(err) = cfg.validate() {
        return RedisTestReport {
            ok: false,
            latency_ms: 0,
            redis_version: None,
            streams_supported: false,
            xautoclaim_supported: false,
            used_memory_human: None,
            dbsize: None,
            rw_probe_ok: false,
            message: err.to_string(),
        };
    }

    let timeout_dur = Duration::from_secs(cfg.conn_timeout_secs.clamp(2, 15));
    let started = Instant::now();

    let url = cfg.effective_url();
    let client = match redis::Client::open(url) {
        Ok(c) => c,
        Err(err) => {
            return RedisTestReport {
                ok: false,
                latency_ms: 0,
                redis_version: None,
                streams_supported: false,
                xautoclaim_supported: false,
                used_memory_human: None,
                dbsize: None,
                rw_probe_ok: false,
                message: format!(
                    "Redis 客户端初始化失败: {}",
                    sanitize_secret_in_text(&err.to_string())
                ),
            };
        }
    };

    let mut conn =
        match tokio::time::timeout(timeout_dur, client.get_multiplexed_async_connection()).await {
            Ok(Ok(c)) => c,
            Ok(Err(err)) => {
                return RedisTestReport {
                    ok: false,
                    latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(0),
                    redis_version: None,
                    streams_supported: false,
                    xautoclaim_supported: false,
                    used_memory_human: None,
                    dbsize: None,
                    rw_probe_ok: false,
                    message: format!(
                        "连接 Redis 失败: {}",
                        sanitize_secret_in_text(&err.to_string())
                    ),
                };
            }
            Err(_) => {
                return RedisTestReport {
                    ok: false,
                    latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(0),
                    redis_version: None,
                    streams_supported: false,
                    xautoclaim_supported: false,
                    used_memory_human: None,
                    dbsize: None,
                    rw_probe_ok: false,
                    message: format!("连接 Redis 超时（{}s）", timeout_dur.as_secs()),
                };
            }
        };

    let ping_started = Instant::now();
    let pong: Result<String, _> =
        tokio::time::timeout(timeout_dur, redis::cmd("PING").query_async(&mut conn))
            .await
            .map_err(|_| anyhow::anyhow!("PING 超时"))
            .and_then(|r| r.map_err(|e| anyhow::anyhow!("{e}")));

    let latency_ms = u64::try_from(ping_started.elapsed().as_millis()).unwrap_or(0);
    if let Err(err) = pong {
        return RedisTestReport {
            ok: false,
            latency_ms,
            redis_version: None,
            streams_supported: false,
            xautoclaim_supported: false,
            used_memory_human: None,
            dbsize: None,
            rw_probe_ok: false,
            message: format!(
                "Redis PING/认证失败: {}",
                sanitize_secret_in_text(&err.to_string())
            ),
        };
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
    let dbsize: Option<u64> = redis::cmd("DBSIZE").query_async(&mut conn).await.ok();

    let redis_version = extract_info_field(&info_server, "redis_version");
    let used_memory_human = extract_info_field(&info_memory, "used_memory_human");

    let (streams_supported, xautoclaim_supported) = redis_version
        .as_deref()
        .map(parse_redis_semver_support)
        .unwrap_or((true, true));

    // 读写权限探测（使用独立 10s 短 TTL 临时键并立即删除）
    let probe_key = format!("hameta:probe:{}", &generate_token()[..16]);
    let rw_probe_ok = async {
        let _: () = redis::cmd("SET")
            .arg(&probe_key)
            .arg("ok")
            .arg("EX")
            .arg(10)
            .query_async(&mut conn)
            .await?;
        let val: String = redis::cmd("GET")
            .arg(&probe_key)
            .query_async(&mut conn)
            .await?;
        let _: usize = redis::cmd("DEL")
            .arg(&probe_key)
            .query_async(&mut conn)
            .await?;
        Ok::<bool, redis::RedisError>(val == "ok")
    }
    .await
    .unwrap_or(false);

    let ok = streams_supported && rw_probe_ok;
    let ver_display = redis_version.as_deref().unwrap_or("未知");
    let message = if !streams_supported {
        format!("Redis 版本 ({ver_display}) 低于 5.0，不支持 Streams（需升级至 5.0+，推荐 6.2+）")
    } else if !rw_probe_ok {
        format!(
            "已连接 Redis v{ver_display}（{latency_ms}ms），但读写探针失败（请检查 ACL 读写权限）"
        )
    } else if !xautoclaim_supported {
        format!(
            "连接成功（v{ver_display}, 延迟 {latency_ms}ms），低于 6.2 将自动启用 XPENDING+XCLAIM 兼容模式"
        )
    } else {
        format!(
            "连接成功（Redis v{ver_display}, 延迟 {latency_ms}ms, Streams/XAUTOCLAIM 全部就绪）"
        )
    };

    RedisTestReport {
        ok,
        latency_ms,
        redis_version,
        streams_supported,
        xautoclaim_supported,
        used_memory_human,
        dbsize,
        rw_probe_ok,
        message,
    }
}

/// 实测 Manticore HTTP 连通性并执行只读 `DESCRIBE torrents` 表结构校验。
pub async fn test_manticore_connection(cfg: &ManticoreServiceConfig) -> ManticoreTestReport {
    if let Err(err) = cfg.validate() {
        return ManticoreTestReport {
            ok: false,
            latency_ms: 0,
            schema: None,
            status: None,
            message: err.to_string(),
        };
    }

    let client = match ManticoreClient::new(cfg) {
        Ok(c) => c,
        Err(err) => {
            return ManticoreTestReport {
                ok: false,
                latency_ms: 0,
                schema: None,
                status: None,
                message: err.to_string(),
            };
        }
    };

    let started = Instant::now();
    let schema = match client.describe_torrents().await {
        Ok(report) => report,
        Err(err) => {
            return ManticoreTestReport {
                ok: false,
                latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(0),
                schema: None,
                status: None,
                message: format!("连接或校验 Manticore 失败: {err}"),
            };
        }
    };
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(0);

    let status = if schema.table_exists {
        client.table_status().await.ok()
    } else {
        None
    };

    let ok = schema.table_exists && schema.compatible;
    let message = if !schema.table_exists {
        format!(
            "已连通 Manticore（延迟 {latency_ms}ms），但未找到 torrents 表（空库可点击初始化建表）"
        )
    } else if !schema.compatible {
        format!("torrents 表结构不兼容: {}", schema.message)
    } else if let Some(docs) = status.as_ref().and_then(|s| s.indexed_documents) {
        format!("连接成功（延迟 {latency_ms}ms），torrents 表兼容，存量文档约 {docs} 条")
    } else {
        format!("连接成功（延迟 {latency_ms}ms），{}", schema.message)
    };

    ManticoreTestReport {
        ok,
        latency_ms,
        schema: Some(schema),
        status,
        message,
    }
}

#[derive(Debug)]
pub enum InitTableError {
    InvalidConfig(String),
    AlreadyExists,
    Backend(String),
}

/// 仅当目标 Manticore 不存在 `torrents` 表时显式创建；表已存在则拒绝触碰。
pub async fn init_manticore_table_if_absent(
    cfg: &ManticoreServiceConfig,
) -> Result<(), InitTableError> {
    cfg.validate()
        .map_err(|e| InitTableError::InvalidConfig(e.to_string()))?;
    let client =
        ManticoreClient::new(cfg).map_err(|e| InitTableError::InvalidConfig(e.to_string()))?;
    let schema = client
        .describe_torrents()
        .await
        .map_err(|e| InitTableError::Backend(format!("检查 torrents 表状态失败: {e}")))?;
    if schema.table_exists {
        return Err(InitTableError::AlreadyExists);
    }
    client
        .create_torrents_table_if_missing()
        .await
        .map_err(|e| InitTableError::Backend(format!("创建 torrents 表失败: {e}")))?;
    Ok(())
}

pub fn sanitize_secret_in_text(input: &str) -> String {
    let mut result = input.to_string();
    for scheme in ["redis://", "rediss://", "http://", "https://"] {
        let mut search_from = 0;
        while let Some(rel_idx) = result[search_from..].find(scheme) {
            let start = search_from + rel_idx + scheme.len();
            if let Some(at_rel) = result[start..].find('@') {
                let slash_rel = result[start..].find(['/', ' ', '\r', '\n']);
                if slash_rel.is_none_or(|s| at_rel < s) {
                    let at_pos = start + at_rel;
                    if result[start..at_pos].contains(':') {
                        result.replace_range(start..at_pos, "<redacted>");
                        search_from = start + "<redacted>".len() + 1;
                        continue;
                    }
                }
            }
            search_from = start;
        }
    }
    result
}

fn extract_info_field(info: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    info.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix(&prefix)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

fn parse_redis_semver_support(version: &str) -> (bool, bool) {
    let mut parts = version.split('.');
    let major = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let minor = parts
        .next()
        .and_then(|s| {
            let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u32>().ok()
        })
        .unwrap_or(0);
    let streams = major >= 5;
    let xautoclaim = major > 6 || (major == 6 && minor >= 2);
    (streams, xautoclaim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_credentials_and_checks_redis_versions() {
        let leaked = "failed connecting to redis://:super_secret_pw@127.0.0.1:6379/0";
        let clean = sanitize_secret_in_text(leaked);
        assert!(!clean.contains("super_secret_pw"));
        assert!(clean.contains("<redacted>@127.0.0.1:6379"));

        assert_eq!(parse_redis_semver_support("4.0.14"), (false, false));
        assert_eq!(parse_redis_semver_support("5.0.9"), (true, false));
        assert_eq!(parse_redis_semver_support("6.0.16"), (true, false));
        assert_eq!(parse_redis_semver_support("6.2.14"), (true, true));
        assert_eq!(parse_redis_semver_support("7.4.1"), (true, true));
    }
}
