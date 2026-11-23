use crate::error::CommonError;
use crate::hash::TABLE_TORRENTS;
use crate::profiles::crawl_profile_defaults;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn is_valid_log_level(level: &str) -> bool {
    matches!(
        level.trim().to_ascii_lowercase().as_str(),
        "error" | "warn" | "info" | "debug" | "trace"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisServiceConfig {
    /// 可选直接指定完整连接串（非空时优先于分段字段）
    pub url: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub db: u8,
    pub tls: bool,
    pub conn_timeout_secs: u64,
    pub wait_timeout_secs: u64,
    pub recycle_timeout_secs: u64,
    pub crawler_pool_size: usize,
    pub indexer_pool_size: usize,
    /// 严格兼容线上存量去重键前缀，默认 `metadata:indexed:`
    pub indexed_key_prefix: String,
    pub torrent_stream: String,
    pub hot_stream: String,
    pub consumer_group: String,
    pub max_stream_len: usize,
    pub max_message_size: usize,
}

impl Default for RedisServiceConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            host: "127.0.0.1".to_string(),
            port: 6379,
            username: String::new(),
            password: String::new(),
            db: 0,
            tls: false,
            conn_timeout_secs: 3,
            wait_timeout_secs: 2,
            recycle_timeout_secs: 3,
            crawler_pool_size: 100,
            indexer_pool_size: 128,
            indexed_key_prefix: "metadata:indexed:".to_string(),
            torrent_stream: "hameta:stream:torrent:new".to_string(),
            hot_stream: "hameta:stream:torrent:hot".to_string(),
            consumer_group: "hameta:cg:indexer".to_string(),
            max_stream_len: 3_000_000,
            max_message_size: 1_048_576,
        }
    }
}

impl RedisServiceConfig {
    /// 生成实际连接的 Redis URL。若显式 `url` 非空则直接采用，否则由分段字段拼接。
    pub fn effective_url(&self) -> String {
        let explicit = self.url.trim();
        if !explicit.is_empty() {
            return explicit.to_string();
        }
        let scheme = if self.tls { "rediss" } else { "redis" };
        let auth = match (self.username.trim(), self.password.as_str()) {
            ("", "") => String::new(),
            ("", pass) => format!(":{pass}@"),
            (user, pass) => format!("{user}:{pass}@"),
        };
        let host = self.host.trim();
        if self.db == 0 {
            format!("{scheme}://{auth}{host}:{}", self.port)
        } else {
            format!("{scheme}://{auth}{host}:{}/{}", self.port, self.db)
        }
    }

    pub fn validate(&self) -> Result<(), CommonError> {
        let url = self.effective_url();
        if !(url.starts_with("redis://") || url.starts_with("rediss://")) {
            return Err(CommonError::InvalidConfig(
                "Redis URL 必须以 redis:// 或 rediss:// 开头".to_string(),
            ));
        }
        if self.url.trim().is_empty() && (self.host.trim().is_empty() || self.port == 0) {
            return Err(CommonError::InvalidConfig(
                "Redis 主机地址与端口不能为空或为 0".to_string(),
            ));
        }
        if self.crawler_pool_size == 0 || self.indexer_pool_size == 0 {
            return Err(CommonError::InvalidConfig(
                "Redis 连接池大小必须大于 0".to_string(),
            ));
        }
        if self.indexed_key_prefix.trim().is_empty()
            || self.torrent_stream.trim().is_empty()
            || self.hot_stream.trim().is_empty()
            || self.consumer_group.trim().is_empty()
        {
            return Err(CommonError::InvalidConfig(
                "Redis 队列与前缀名称不能为空".to_string(),
            ));
        }
        if self.max_stream_len == 0 || self.max_message_size < 1024 {
            return Err(CommonError::InvalidConfig(
                "Stream 裁剪上限需 >= 1，单条消息大小上限需 >= 1024 字节".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManticoreAuthMode {
    #[default]
    None,
    Basic,
    Bearer,
    Header,
}

impl ManticoreAuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic => "basic",
            Self::Bearer => "bearer",
            Self::Header => "header",
        }
    }
}

fn is_valid_http_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn is_reserved_transport_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "content-length" | "transfer-encoding" | "connection"
    )
}

fn is_valid_http_header_value(val: &str) -> bool {
    !val.is_empty()
        && val
            .bytes()
            .all(|b| b == b'\t' || (0x20..=0x7E).contains(&b))
}

fn strip_bearer_prefix(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed
        .as_bytes()
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"bearer "))
    {
        trimmed[7..].trim()
    } else {
        trimmed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ManticoreServiceConfig {
    pub url: String,
    pub auth_mode: ManticoreAuthMode,
    pub username: String,
    pub password: String,
    pub bearer_token: String,
    pub header_name: String,
    pub header_value: String,
    /// 存量表名锁定为 `torrents`
    pub table_name: String,
    pub request_timeout_secs: u64,
    pub max_connections: u32,
}

impl Default for ManticoreServiceConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:9308".to_string(),
            auth_mode: ManticoreAuthMode::None,
            username: String::new(),
            password: String::new(),
            bearer_token: String::new(),
            header_name: String::new(),
            header_value: String::new(),
            table_name: TABLE_TORRENTS.to_string(),
            request_timeout_secs: 10,
            max_connections: 1280,
        }
    }
}

impl ManticoreServiceConfig {
    /// 兼容未显式设置 `auth_mode` 的历史配置：若存在 `username`/`password` 则自动视为 `Basic`。
    pub fn effective_auth_mode(&self) -> ManticoreAuthMode {
        if self.auth_mode == ManticoreAuthMode::None
            && (!self.username.trim().is_empty() || !self.password.is_empty())
        {
            ManticoreAuthMode::Basic
        } else {
            self.auth_mode
        }
    }

    /// 返回归一化 `auth_mode` 后的配置副本。
    pub fn normalized(&self) -> Self {
        let mut cloned = self.clone();
        cloned.auth_mode = self.effective_auth_mode();
        cloned
    }

    pub fn has_auth(&self) -> bool {
        self.effective_auth_mode() != ManticoreAuthMode::None
    }

    pub fn basic_auth(&self) -> Option<(&str, &str)> {
        if self.effective_auth_mode() == ManticoreAuthMode::Basic
            && !self.username.trim().is_empty()
            && !self.password.is_empty()
        {
            Some((self.username.trim(), self.password.as_str()))
        } else {
            None
        }
    }

    pub fn bearer_token(&self) -> Option<&str> {
        if self.effective_auth_mode() != ManticoreAuthMode::Bearer {
            return None;
        }
        let token = strip_bearer_prefix(&self.bearer_token);
        if token.is_empty() { None } else { Some(token) }
    }

    pub fn custom_header_auth(&self) -> Option<(&str, &str)> {
        if self.effective_auth_mode() != ManticoreAuthMode::Header {
            return None;
        }
        let name = self.header_name.trim();
        let val = self.header_value.trim();
        if name.is_empty() || val.is_empty() {
            None
        } else {
            Some((name, val))
        }
    }

    pub fn validate(&self) -> Result<(), CommonError> {
        let url = self.url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(CommonError::InvalidConfig(
                "Manticore URL 必须以 http:// 或 https:// 开头".to_string(),
            ));
        }
        match self.effective_auth_mode() {
            ManticoreAuthMode::None => {}
            ManticoreAuthMode::Basic => {
                let user = self.username.trim();
                if user.is_empty() || self.password.is_empty() {
                    return Err(CommonError::InvalidConfig(
                        "Manticore HTTP Basic Auth 用户名和密码必须同时填写".to_string(),
                    ));
                }
                if user.contains(':') {
                    return Err(CommonError::InvalidConfig(
                        "Manticore HTTP Basic Auth 用户名不能包含冒号 ':'".to_string(),
                    ));
                }
                if self.username.contains(['\r', '\n']) || self.password.contains(['\r', '\n']) {
                    return Err(CommonError::InvalidConfig(
                        "Manticore HTTP Basic Auth 凭据不能包含换行符".to_string(),
                    ));
                }
            }
            ManticoreAuthMode::Bearer => {
                let Some(token) = self.bearer_token() else {
                    return Err(CommonError::InvalidConfig(
                        "已选择 Bearer Token 鉴权，请填写有效的 Token".to_string(),
                    ));
                };
                if !is_valid_http_header_value(token) {
                    return Err(CommonError::InvalidConfig(
                        "Manticore Bearer Token 包含非法 HTTP 请求头字符".to_string(),
                    ));
                }
            }
            ManticoreAuthMode::Header => {
                let name = self.header_name.trim();
                let val = self.header_value.trim();
                if name.is_empty() || val.is_empty() {
                    return Err(CommonError::InvalidConfig(
                        "已选择自定义请求头鉴权，请同时填写请求头名称和值".to_string(),
                    ));
                }
                if !is_valid_http_header_name(name) {
                    return Err(CommonError::InvalidConfig(
                        "Manticore 自定义鉴权请求头名称格式无效".to_string(),
                    ));
                }
                if is_reserved_transport_header(name) {
                    return Err(CommonError::InvalidConfig(
                        "Manticore 自定义鉴权请求头不能覆盖 HTTP 传输保留头".to_string(),
                    ));
                }
                if !is_valid_http_header_value(val) {
                    return Err(CommonError::InvalidConfig(
                        "Manticore 自定义鉴权请求头值包含非法字符".to_string(),
                    ));
                }
            }
        }
        if self.table_name.trim() != TABLE_TORRENTS {
            return Err(CommonError::InvalidConfig(format!(
                "Manticore 目标表名已锁定为 {TABLE_TORRENTS}，禁止修改存量表名"
            )));
        }
        if self.request_timeout_secs == 0 || self.max_connections == 0 {
            return Err(CommonError::InvalidConfig(
                "Manticore 超时与最大连接数必须大于 0".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NetModeConfig {
    #[default]
    #[serde(rename = "ipv4_only")]
    Ipv4Only,
    #[serde(rename = "ipv6_only")]
    Ipv6Only,
    #[serde(rename = "dual", alias = "dual_stack")]
    Dual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerWorkerConfig {
    // L1 基础与档位层
    pub port: u16,
    pub netmode: NetModeConfig,
    pub crawl_profile: String,
    pub log_level: String,
    pub metadata_cache_capacity: usize,
    pub push_concurrency: usize,
    pub metadata_pending_ttl_secs: u64,
    pub metadata_failure_backoff_secs: u64,
    pub hash_queue_capacity: usize,
    // L2 抓取与发现调优层
    pub metadata: CrawlerMetadataConfig,
    pub discovery: CrawlerDiscoveryConfig,
    pub peer_lookup: CrawlerPeerLookupConfig,
    // L3 高级引擎覆写层
    pub crawl: CrawlerCrawlConfig,
}

impl Default for CrawlerWorkerConfig {
    fn default() -> Self {
        Self {
            port: 12313,
            netmode: NetModeConfig::Ipv4Only,
            crawl_profile: "P1".to_string(),
            log_level: "warn".to_string(),
            metadata_cache_capacity: 1_000_000,
            push_concurrency: 16,
            metadata_pending_ttl_secs: 3600,
            metadata_failure_backoff_secs: 60,
            hash_queue_capacity: 4_096,
            metadata: CrawlerMetadataConfig::default(),
            discovery: CrawlerDiscoveryConfig::default(),
            peer_lookup: CrawlerPeerLookupConfig::default(),
            crawl: CrawlerCrawlConfig::default(),
        }
    }
}

impl CrawlerWorkerConfig {
    pub fn validate(&self) -> Result<(), CommonError> {
        if self.port == 0 {
            return Err(CommonError::InvalidConfig(
                "DHT 监听端口不能为 0".to_string(),
            ));
        }
        if crawl_profile_defaults(&self.crawl_profile).is_none() {
            return Err(CommonError::InvalidProfile(self.crawl_profile.clone()));
        }
        if !is_valid_log_level(&self.log_level) {
            return Err(CommonError::InvalidConfig(format!(
                "无效的日志级别: {}",
                self.log_level
            )));
        }
        if self.metadata_cache_capacity == 0
            || self.push_concurrency == 0
            || self.hash_queue_capacity == 0
        {
            return Err(CommonError::InvalidConfig(
                "缓存容量、推送并发数与 Hash 队列容量必须大于 0".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerMetadataConfig {
    pub timeout_secs: u64,
    pub connect_timeout_ms: u64,
    pub max_queue_size: usize,
    pub max_worker_count: usize,
    pub peer_failure_cache_capacity: usize,
    pub peer_failure_ttl_secs: u64,
    pub retry_initial_delay_secs: u64,
    pub retry_max_delay_secs: u64,
    pub retry_max_attempts: usize,
    pub retry_history_ttl_secs: u64,
    pub retry_history_capacity: usize,
}

impl Default for CrawlerMetadataConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 4,
            connect_timeout_ms: 2_000,
            max_queue_size: 10_000,
            max_worker_count: 256,
            peer_failure_cache_capacity: 200_000,
            peer_failure_ttl_secs: 60,
            retry_initial_delay_secs: 62,
            retry_max_delay_secs: 3_600,
            retry_max_attempts: 5,
            retry_history_ttl_secs: 21_600,
            retry_history_capacity: 100_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerDiscoveryConfig {
    pub enabled: bool,
    pub sample_queries_per_second: u32,
    pub sample_burst: u32,
    pub sample_max_in_flight: usize,
    pub sample_query_timeout_secs: u64,
    pub sample_min_interval_secs: u64,
    pub sample_failure_backoff_secs: u64,
    pub sample_no_new_backoff_secs: u64,
    pub sample_max_nodes: usize,
    pub sample_cooldown_capacity: usize,
    pub hash_dedup_capacity: usize,
    pub hash_dedup_ttl_secs: u64,
    pub get_peers_hashes_per_second: u32,
    pub get_peers_burst: u32,
    pub sample_hashes_per_second: u32,
    pub sample_hash_burst: u32,
}

impl Default for CrawlerDiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sample_queries_per_second: 4,
            sample_burst: 8,
            sample_max_in_flight: 16,
            sample_query_timeout_secs: 3,
            sample_min_interval_secs: 300,
            sample_failure_backoff_secs: 900,
            sample_no_new_backoff_secs: 1_800,
            sample_max_nodes: 16_384,
            sample_cooldown_capacity: 100_000,
            hash_dedup_capacity: 100_000,
            hash_dedup_ttl_secs: 900,
            get_peers_hashes_per_second: 8,
            get_peers_burst: 16,
            sample_hashes_per_second: 16,
            sample_hash_burst: 64,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerPeerLookupConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_lookups_per_second: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burst: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_active_lookups: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_queries: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_queries: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerCrawlConfig {
    pub rate_limit: CrawlerRateLimitConfig,
    pub pool: CrawlerPoolConfig,
    pub bootstrap: CrawlerBootstrapConfig,
    pub target: CrawlerTargetConfig,
    pub scheduler: CrawlerSchedulerConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerRateLimitConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_find_node_rate_per_sec: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burst: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_in_flight: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_new_destinations_per_minute: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_response_rate_per_sec: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_response_bytes_per_sec: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_response_rate_per_source: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_pressure_floor_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_replacements_per_minute: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_in_flight_per_subnet: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerPoolConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_probe_ttl_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responsive_capacity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responsive_ttl_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revisit_interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low_watermark: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerBootstrapConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_nodes_per_round: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_backoff_base_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_backoff_max_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerTargetConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub random_walk_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sparse_bucket_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neighbor_sender_id: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrawlerSchedulerConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority_event_channel_capacity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery_event_channel_capacity: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_batch_limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_batch_limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_snapshot_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_refresh_millis: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndexerWorkerConfig {
    pub torrent_workers: usize,
    pub hot_workers: usize,
    pub read_block_ms: u64,
    pub read_batch_size: usize,
    pub insert_workers: usize,
    pub batch_size: usize,
    pub flush_interval_ms: u64,
    pub channel_buffer_size: usize,
    pub hot_channel_buffer_size: usize,
    pub xautoclaim_interval_secs: u64,
    pub xautoclaim_min_idle_ms: u64,
    pub max_delivery_attempts: usize,
    pub log_level: String,
}

impl Default for IndexerWorkerConfig {
    fn default() -> Self {
        Self {
            torrent_workers: 64,
            hot_workers: 4,
            read_block_ms: 5_000,
            read_batch_size: 512,
            insert_workers: 64,
            batch_size: 1_000,
            flush_interval_ms: 5_000,
            channel_buffer_size: 2_000,
            hot_channel_buffer_size: 10_000,
            xautoclaim_interval_secs: 30,
            xautoclaim_min_idle_ms: 120_000,
            max_delivery_attempts: 5,
            log_level: "info".to_string(),
        }
    }
}

impl IndexerWorkerConfig {
    pub fn validate(&self) -> Result<(), CommonError> {
        if self.torrent_workers == 0
            || self.hot_workers == 0
            || self.read_batch_size == 0
            || self.insert_workers == 0
            || self.batch_size == 0
            || self.flush_interval_ms == 0
            || self.channel_buffer_size == 0
            || self.hot_channel_buffer_size == 0
            || self.xautoclaim_interval_secs == 0
            || self.max_delivery_attempts == 0
        {
            return Err(CommonError::InvalidConfig(
                "索引器并发、批次与通道容量参数必须大于 0".to_string(),
            ));
        }
        if !is_valid_log_level(&self.log_level) {
            return Err(CommonError::InvalidConfig(format!(
                "无效的日志级别: {}",
                self.log_level
            )));
        }
        Ok(())
    }
}

/// P3TERX GeoLite.mmdb 仓库 Release 页面与直链下载地址。
pub const DEFAULT_GEOIP_RELEASES_URL: &str = "https://github.com/P3TERX/GeoLite.mmdb/releases";
pub const DEFAULT_GEOIP_DOWNLOAD_URL: &str =
    "https://github.com/P3TERX/GeoLite.mmdb/releases/latest/download/GeoLite2-Country.mmdb";

/// GeoLite2 离线 IP 库配置（用于 DHT 入站流量的国家维度统计与热重载）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeoIpConfig {
    pub enabled: bool,
    pub mmdb_path: String,
    pub download_url: String,
    pub home_country: String,
    pub prefix_cache_slots: usize,
    pub adaptive_sample_threshold_pps: u64,
    pub sample_queue_capacity: usize,
}

impl Default for GeoIpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mmdb_path: "GeoLite2-Country.mmdb".to_string(),
            download_url: DEFAULT_GEOIP_DOWNLOAD_URL.to_string(),
            home_country: "CN".to_string(),
            prefix_cache_slots: 65_536,
            adaptive_sample_threshold_pps: 30_000,
            sample_queue_capacity: 8_192,
        }
    }
}

impl GeoIpConfig {
    pub fn validate(&self) -> Result<(), CommonError> {
        let home = self.home_country.trim();
        if home.len() != 2 || !home.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(CommonError::InvalidConfig(
                "home_country 必须为 2 位 ISO 国家代码（例如 CN / US）".to_string(),
            ));
        }
        let url = self.download_url.trim();
        if !url.is_empty()
            && ((!url.starts_with("https://") && !url.starts_with("http://"))
                || url.chars().any(char::is_whitespace))
        {
            return Err(CommonError::InvalidConfig(
                "download_url 必须为有效的 http:// 或 https:// 下载地址".to_string(),
            ));
        }
        if !(4_096..=1_048_576).contains(&self.prefix_cache_slots) {
            return Err(CommonError::InvalidConfig(
                "GeoIP 前缀缓存槽位数必须在 4096 ~ 1048576 之间".to_string(),
            ));
        }
        if self.adaptive_sample_threshold_pps < 1_000 {
            return Err(CommonError::InvalidConfig(
                "GeoIP 自适应采样触发阈值必须 >= 1000 pkt/s".to_string(),
            ));
        }
        if !(512..=131_072).contains(&self.sample_queue_capacity) {
            return Err(CommonError::InvalidConfig(
                "GeoIP 样本队列容量必须在 512 ~ 131072 之间".to_string(),
            ));
        }
        Ok(())
    }

    pub fn effective_download_url(&self) -> &str {
        let trimmed = self.download_url.trim();
        if trimmed.is_empty() {
            DEFAULT_GEOIP_DOWNLOAD_URL
        } else {
            trimmed
        }
    }

    pub fn normalized_home_country(&self) -> String {
        let trimmed = self.home_country.trim();
        if trimmed.len() == 2 && trimmed.bytes().all(|b| b.is_ascii_alphabetic()) {
            trimmed.to_ascii_uppercase()
        } else {
            "CN".to_string()
        }
    }

    /// 返回收敛到 `[4096, 1_048_576]` 区间内的 2 的幂缓存槽位数。
    pub fn normalized_prefix_cache_slots(&self) -> usize {
        let clamped = self.prefix_cache_slots.clamp(4_096, 1_048_576);
        clamped.next_power_of_two().min(1_048_576)
    }

    pub fn resolve_mmdb_path(&self, data_dir: &Path) -> PathBuf {
        let trimmed = self.mmdb_path.trim();
        if trimmed.is_empty() {
            return data_dir.join("GeoLite2-Country.mmdb");
        }
        let candidate = PathBuf::from(trimmed);
        if candidate.is_absolute() || candidate.exists() {
            candidate
        } else {
            data_dir.join(candidate)
        }
    }
}
