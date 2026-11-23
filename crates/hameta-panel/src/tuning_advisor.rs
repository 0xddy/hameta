use crate::supervisor::Supervisor;
use chrono::Utc;
use hameta_common::{
    CrawlProfileDefaults, CrawlerWorkerConfig, WorkerRole, WorkerRuntimeState,
    crawl_profile_defaults, current_soft_open_file_limit,
};
use hameta_storage::Database;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// 默认专项采集时长：1 小时（3600 秒）。
pub const DEFAULT_TUNING_DURATION_SECS: u64 = 3_600;
/// 默认采样间隔：5 秒（1 小时共 720 个紧凑内存样本，单实例内存占用 < 300 KB）。
pub const DEFAULT_SAMPLE_INTERVAL_SECS: u64 = 5;
/// 自动滚存内存环形缓冲区最大样本数（5s * 720 = 1 小时）。
pub const ROLLING_RING_MAX_SAMPLES: usize = 720;
/// 单次专项采集最大样本数上限（2 小时 @ 5s）。
pub const MAX_SESSION_SAMPLES: usize = 1_440;
/// 允许的最短与最长专项采集时长。
pub const MIN_TUNING_DURATION_SECS: u64 = 60;
pub const MAX_TUNING_DURATION_SECS: u64 = 7_200;

/// 优化目标偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TuningGoal {
    /// 稳态高产：优先消除 95% 降速、队列陈旧和丢包，保持高有效抓取率。
    #[default]
    Balanced,
    /// 极致吞吐：在硬件与带宽护栏内尽可能拉高探测速率与并发抓取。
    Throughput,
    /// 低资源节能：优先压低内存、连接跟踪与小包 PPS 压力。
    LowResource,
}

impl TuningGoal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Throughput => "throughput",
            Self::LowResource => "low_resource",
        }
    }

    pub fn from_opt_str(raw: Option<&str>) -> Self {
        match raw
            .map(str::trim)
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "throughput" | "max" | "aggressive" => Self::Throughput,
            "low_resource" | "conservative" | "eco" => Self::LowResource,
            _ => Self::Balanced,
        }
    }
}

/// 可选外部环境约束（用户可指定服务器上下行带宽上限，留空则纯靠运行指标诊断）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuningConstraints {
    #[serde(default)]
    pub up_mbps: Option<f64>,
    #[serde(default)]
    pub down_mbps: Option<f64>,
    #[serde(default)]
    pub goal: TuningGoal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Collecting,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLevel {
    Insufficient,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationSeverity {
    Critical,
    Warning,
    Opportunity,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationAction {
    Increase,
    Decrease,
    Tune,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingRegime {
    NofileClamped,
    CpuUdpSaturated,
    RedisSinkBottleneck,
    BandwidthCeiling,
    OverfedBackpressure,
    UnderfedStarvation,
    BalancedHealthy,
}

/// 紧凑定长直方图快照（用于跨 1 小时窗口精确计算增量分位数 P50/P95）。
#[derive(Debug, Clone, Default)]
pub struct CompactHistogram {
    pub bounds: Vec<u64>,
    pub counts: Vec<u64>,
    pub overflow: u64,
    pub count: u64,
    pub sum: u64,
}

impl CompactHistogram {
    fn from_json(v: Option<&Value>) -> Self {
        let Some(obj) = v else {
            return Self::default();
        };
        let bounds = obj
            .get("bounds")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        let counts = obj
            .get("counts")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        Self {
            bounds,
            counts,
            overflow: obj.get("overflow").and_then(Value::as_u64).unwrap_or(0),
            count: obj.get("count").and_then(Value::as_u64).unwrap_or(0),
            sum: obj.get("sum").and_then(Value::as_u64).unwrap_or(0),
        }
    }

    /// 计算 `[first, last]` 采样窗口内的增量直方图，过滤进程历史累计数干扰。
    fn delta_since(&self, first: &Self) -> Self {
        if self.bounds.is_empty() || self.bounds != first.bounds || self.count < first.count {
            return self.clone();
        }
        let counts = self
            .counts
            .iter()
            .zip(first.counts.iter())
            .map(|(cur, prev)| cur.saturating_sub(*prev))
            .collect();
        Self {
            bounds: self.bounds.clone(),
            counts,
            overflow: self.overflow.saturating_sub(first.overflow),
            count: self.count.saturating_sub(first.count),
            sum: self.sum.saturating_sub(first.sum),
        }
    }

    fn percentile(&self, q: f64) -> Option<u64> {
        let total: u64 = self
            .counts
            .iter()
            .copied()
            .sum::<u64>()
            .saturating_add(self.overflow);
        if total == 0 || self.bounds.is_empty() {
            return None;
        }
        let target = ((total as f64) * q.clamp(0.0, 1.0)).ceil().max(1.0) as u64;
        let mut acc = 0_u64;
        for (idx, c) in self.counts.iter().enumerate() {
            acc = acc.saturating_add(*c);
            if acc >= target {
                return self
                    .bounds
                    .get(idx)
                    .copied()
                    .or_else(|| self.bounds.last().copied());
            }
        }
        self.bounds.last().copied()
    }
}

/// 单条内存采样点：覆盖 Crawler 全链路核心指标（纯数值无大字符串，内存紧凑）。
#[derive(Debug, Clone, Default)]
pub struct CrawlerMetricSample {
    pub captured_at: i64,
    pub uptime_secs: u64,
    pub config_version: i64,
    pub effective_metadata_workers: usize,
    pub local_cache_entries: u64,
    pub redis_pool_size: usize,
    pub redis_pool_available: usize,
    pub rss_bytes: u64,

    // App counters & gauges
    pub early_filter_passed: u64,
    pub early_filter_blocked: u64,
    pub admission_fetch: u64,
    pub admission_defer: u64,
    pub admission_skip: u64,
    pub redis_claim_acquired: u64,
    pub redis_claim_indexed: u64,
    pub redis_claim_errors: u64,
    pub torrents_downloaded: u64,
    pub torrents_pushed: u64,
    pub torrents_push_errors: u64,
    pub torrents_sink_dropped: u64,
    pub hot_dropped: u64,
    pub metadata_fetch_accepted: u64,
    pub metadata_fetch_failed: u64,
    pub sink_queue_depth: usize,
    pub sink_in_flight: usize,

    // DHT runtime gauges & counters
    pub hashes_received: u64,
    pub hash_ingress_dropped: u64,
    pub hash_ingress_queue_depth: usize,
    pub hash_ingress_queue_capacity: usize,
    pub crawl_priority_queue_depth: usize,
    pub crawl_priority_queue_capacity: usize,
    pub crawl_discovery_queue_depth: usize,
    pub crawl_discovery_queue_capacity: usize,
    pub metadata_queue_depth: usize,
    pub metadata_queue_max: usize,
    pub metadata_in_flight: usize,
    pub metadata_queue_inserted: u64,
    pub metadata_queue_deduplicated: u64,
    pub metadata_queue_evicted: u64,
    pub metadata_queue_stale: u64,
    pub peer_lookup_requested: u64,
    pub peer_lookup_started: u64,
    pub peer_lookup_rate_limited: u64,
    pub peer_lookup_empty: u64,
    pub peer_lookup_queries: u64,
    pub peer_lookup_timeouts: u64,
    pub peer_lookup_peers_found: u64,
    pub metadata_peer_attempts: u64,
    pub metadata_peer_succeeded: u64,
    pub metadata_peer_failed: u64,
    pub metadata_peer_timeouts: u64,
    pub metadata_peer_failure_cache_hits: u64,
    pub metadata_peer_failure_cache_entries: usize,
    pub node_pool_size: usize,
    pub node_pool_capacity: usize,
    pub find_node_in_flight: usize,
    pub find_node_in_flight_max: usize,
    pub find_node_effective_rate_per_sec: u32,
    pub queries_new: u64,
    pub queries_revisit: u64,
    pub queries_bootstrap: u64,
    pub responses: u64,
    pub unmatched_responses: u64,
    pub timeouts: u64,
    pub send_failures: u64,
    pub udp_queue_full: u64,
    pub udp_responses_rate_limited: u64,

    // Observability
    pub udp_rx_packets: u64,
    pub udp_rx_bytes: u64,
    pub udp_tx_packets: u64,
    pub udp_tx_bytes: u64,
    pub response_normal: u64,
    pub response_fallback: u64,
    pub response_rate_limited: u64,
    pub response_send_failed: u64,
    pub node_admitted: u64,
    pub node_replaced: u64,
    pub node_dropped_rate_limited: u64,
    pub metadata_bytes_downloaded: u64,
    pub metadata_failure_timeout: u64,
    pub timeout_tcp_connect: u64,
    pub timeout_base_handshake: u64,
    pub timeout_ext_send: u64,
    pub timeout_ext_recv: u64,
    pub timeout_piece_send: u64,
    pub timeout_piece_recv: u64,
    pub timeout_validation: u64,
    pub conn_err_refused: u64,
    pub conn_err_reset: u64,
    pub conn_err_io_other: u64,

    // Yield counters (BEP-51 discovery & lookup diagnostics)
    pub disc_get_peers_observed: u64,
    pub disc_get_peers_rate_limited: u64,
    pub disc_get_peers_enqueued: u64,
    pub disc_sample_queries: u64,
    pub disc_sample_responses: u64,
    pub disc_sample_hashes_enqueued: u64,
    pub disc_hash_rate_limited: u64,
    pub disc_sample_cooldown_full: u64,
    pub disc_node_cap_dropped: u64,

    // Histograms for window delta percentiles
    pub queue_wait_ms: CompactHistogram,
    pub fetch_duration_ms: CompactHistogram,
    pub metadata_size_bytes: CompactHistogram,
}

impl CrawlerMetricSample {
    pub fn from_snapshot_payload(captured_at: i64, config_version: i64, p: &Value) -> Self {
        Self {
            captured_at,
            uptime_secs: j_u64(p, &["uptime_secs"]),
            config_version,
            effective_metadata_workers: j_usize(p, &["effective_metadata_workers"]),
            local_cache_entries: j_u64(p, &["local_cache_entries"]),
            redis_pool_size: j_usize(p, &["redis_pool_size"]),
            redis_pool_available: j_usize(p, &["redis_pool_available"]),
            rss_bytes: j_u64(p, &["rss_bytes"]),

            early_filter_passed: j_u64(p, &["app", "early_filter_passed"]),
            early_filter_blocked: j_u64(p, &["app", "early_filter_blocked"]),
            admission_fetch: j_u64(p, &["app", "admission_fetch"]),
            admission_defer: j_u64(p, &["app", "admission_defer"]),
            admission_skip: j_u64(p, &["app", "admission_skip"]),
            redis_claim_acquired: j_u64(p, &["app", "redis_claim_acquired"]),
            redis_claim_indexed: j_u64(p, &["app", "redis_claim_indexed"]),
            redis_claim_errors: j_u64(p, &["app", "redis_claim_errors"]),
            torrents_downloaded: j_u64(p, &["app", "torrents_downloaded"]),
            torrents_pushed: j_u64(p, &["app", "torrents_pushed"]),
            torrents_push_errors: j_u64(p, &["app", "torrents_push_errors"]),
            torrents_sink_dropped: j_u64(p, &["app", "torrents_sink_dropped"]),
            hot_dropped: j_u64(p, &["app", "hot_dropped"]),
            metadata_fetch_accepted: j_u64(p, &["app", "metadata_fetch_accepted"]),
            metadata_fetch_failed: j_u64(p, &["app", "metadata_fetch_failed"]),
            sink_queue_depth: j_usize(p, &["app", "sink_queue_depth"]),
            sink_in_flight: j_usize(p, &["app", "sink_in_flight"]),

            hashes_received: j_u64(p, &["dht", "hashes_received"]),
            hash_ingress_dropped: j_u64(p, &["dht", "hash_ingress_dropped"]),
            hash_ingress_queue_depth: j_usize(p, &["dht", "hash_ingress_queue_depth"]),
            hash_ingress_queue_capacity: j_usize(p, &["dht", "hash_ingress_queue_capacity"]),
            crawl_priority_queue_depth: j_usize(p, &["dht", "crawl_priority_queue_depth"]),
            crawl_priority_queue_capacity: j_usize(p, &["dht", "crawl_priority_queue_capacity"]),
            crawl_discovery_queue_depth: j_usize(p, &["dht", "crawl_discovery_queue_depth"]),
            crawl_discovery_queue_capacity: j_usize(p, &["dht", "crawl_discovery_queue_capacity"]),
            metadata_queue_depth: j_usize(p, &["dht", "metadata_queue_depth"]),
            metadata_queue_max: j_usize(p, &["dht", "metadata_queue_max"]),
            metadata_in_flight: j_usize(p, &["dht", "metadata_in_flight"]),
            metadata_queue_inserted: j_u64(p, &["dht", "metadata_queue_inserted"]),
            metadata_queue_deduplicated: j_u64(p, &["dht", "metadata_queue_deduplicated"]),
            metadata_queue_evicted: j_u64(p, &["dht", "metadata_queue_evicted"]),
            metadata_queue_stale: j_u64(p, &["dht", "metadata_queue_stale"]),
            peer_lookup_requested: j_u64(p, &["dht", "peer_lookup_requested"]),
            peer_lookup_started: j_u64(p, &["dht", "peer_lookup_started"]),
            peer_lookup_rate_limited: j_u64(p, &["dht", "peer_lookup_rate_limited"]),
            peer_lookup_empty: j_u64(p, &["dht", "peer_lookup_empty"]),
            peer_lookup_queries: j_u64(p, &["dht", "peer_lookup_queries"]),
            peer_lookup_timeouts: j_u64(p, &["dht", "peer_lookup_timeouts"]),
            peer_lookup_peers_found: j_u64(p, &["dht", "peer_lookup_peers_found"]),
            metadata_peer_attempts: j_u64(p, &["dht", "metadata_peer_attempts"]),
            metadata_peer_succeeded: j_u64(p, &["dht", "metadata_peer_succeeded"]),
            metadata_peer_failed: j_u64(p, &["dht", "metadata_peer_failed"]),
            metadata_peer_timeouts: j_u64(p, &["dht", "metadata_peer_timeouts"]),
            metadata_peer_failure_cache_hits: j_u64(
                p,
                &["dht", "metadata_peer_failure_cache_hits"],
            ),
            metadata_peer_failure_cache_entries: j_usize(
                p,
                &["dht", "metadata_peer_failure_cache_entries"],
            ),
            node_pool_size: j_usize(p, &["dht", "node_pool_size"]),
            node_pool_capacity: j_usize(p, &["dht", "node_pool_capacity"]),
            find_node_in_flight: j_usize(p, &["dht", "find_node_in_flight"]),
            find_node_in_flight_max: j_usize(p, &["dht", "find_node_in_flight_max"]),
            find_node_effective_rate_per_sec: j_u64(p, &["dht", "find_node_effective_rate_per_sec"])
                .min(u64::from(u32::MAX)) as u32,
            queries_new: j_u64(p, &["dht", "queries_new"]),
            queries_revisit: j_u64(p, &["dht", "queries_revisit"]),
            queries_bootstrap: j_u64(p, &["dht", "queries_bootstrap"]),
            responses: j_u64(p, &["dht", "responses"]),
            unmatched_responses: j_u64(p, &["dht", "unmatched_responses"]),
            timeouts: j_u64(p, &["dht", "timeouts"]),
            send_failures: j_u64(p, &["dht", "send_failures"]),
            udp_queue_full: j_u64(p, &["dht", "udp_queue_full"]),
            udp_responses_rate_limited: j_u64(p, &["dht", "udp_responses_rate_limited"]),

            udp_rx_packets: j_u64(p, &["observability", "udp_rx_packets"]),
            udp_rx_bytes: j_u64(p, &["observability", "udp_rx_bytes"]),
            udp_tx_packets: j_u64(p, &["observability", "udp_tx_packets"]),
            udp_tx_bytes: j_u64(p, &["observability", "udp_tx_bytes"]),
            response_normal: j_u64(p, &["observability", "response_normal"]),
            response_fallback: j_u64(p, &["observability", "response_fallback"]),
            response_rate_limited: j_u64(p, &["observability", "response_rate_limited"]),
            response_send_failed: j_u64(p, &["observability", "response_send_failed"]),
            node_admitted: j_u64(p, &["observability", "node_admitted"]),
            node_replaced: j_u64(p, &["observability", "node_replaced"]),
            node_dropped_rate_limited: j_u64(p, &["observability", "node_dropped_rate_limited"]),
            metadata_bytes_downloaded: j_u64(p, &["observability", "metadata_bytes_downloaded"]),
            metadata_failure_timeout: j_u64(p, &["observability", "metadata_failure_timeout"]),
            timeout_tcp_connect: j_u64(
                p,
                &["observability", "metadata_timeout_stages", "tcp_connect"],
            ),
            timeout_base_handshake: j_u64(
                p,
                &["observability", "metadata_timeout_stages", "base_handshake"],
            ),
            timeout_ext_send: j_u64(
                p,
                &[
                    "observability",
                    "metadata_timeout_stages",
                    "extension_handshake_send",
                ],
            ),
            timeout_ext_recv: j_u64(
                p,
                &[
                    "observability",
                    "metadata_timeout_stages",
                    "extension_handshake_receive",
                ],
            ),
            timeout_piece_send: j_u64(
                p,
                &[
                    "observability",
                    "metadata_timeout_stages",
                    "piece_request_send",
                ],
            ),
            timeout_piece_recv: j_u64(
                p,
                &["observability", "metadata_timeout_stages", "piece_receive"],
            ),
            timeout_validation: j_u64(
                p,
                &["observability", "metadata_timeout_stages", "validation"],
            ),
            conn_err_refused: j_u64(
                p,
                &["observability", "metadata_connection_errors", "refused"],
            ),
            conn_err_reset: j_u64(p, &["observability", "metadata_connection_errors", "reset"]),
            conn_err_io_other: j_u64(
                p,
                &["observability", "metadata_connection_errors", "io_other"],
            ),

            disc_get_peers_observed: j_u64(p, &["yield_counters", "discovery_get_peers_observed"]),
            disc_get_peers_rate_limited: j_u64(
                p,
                &["yield_counters", "discovery_get_peers_rate_limited"],
            ),
            disc_get_peers_enqueued: j_u64(p, &["yield_counters", "discovery_get_peers_enqueued"]),
            disc_sample_queries: j_u64(p, &["yield_counters", "discovery_sample_queries"]),
            disc_sample_responses: j_u64(p, &["yield_counters", "discovery_sample_responses"]),
            disc_sample_hashes_enqueued: j_u64(
                p,
                &["yield_counters", "discovery_sample_hashes_enqueued"],
            ),
            disc_hash_rate_limited: j_u64(p, &["yield_counters", "discovery_hash_rate_limited"]),
            disc_sample_cooldown_full: j_u64(
                p,
                &["yield_counters", "discovery_sample_cooldown_full"],
            ),
            disc_node_cap_dropped: j_u64(p, &["yield_counters", "discovery_node_capacity_dropped"]),

            queue_wait_ms: CompactHistogram::from_json(
                p.get("observability").and_then(|o| o.get("queue_wait_ms")),
            ),
            fetch_duration_ms: CompactHistogram::from_json(
                p.get("observability")
                    .and_then(|o| o.get("fetch_duration_ms")),
            ),
            metadata_size_bytes: CompactHistogram::from_json(
                p.get("observability")
                    .and_then(|o| o.get("metadata_size_bytes")),
            ),
        }
    }

    fn approx_mem_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + (self.queue_wait_ms.bounds.len()
                + self.queue_wait_ms.counts.len()
                + self.fetch_duration_ms.bounds.len()
                + self.fetch_duration_ms.counts.len()
                + self.metadata_size_bytes.bounds.len()
                + self.metadata_size_bytes.counts.len())
                * std::mem::size_of::<u64>()
    }
}

/// 窗口内聚合观测摘要（展示在前端报告顶部，与 MySQLTuner 统计块类似）。
#[derive(Debug, Clone, Serialize)]
pub struct TuningWindowSummary {
    pub sample_count: usize,
    pub continuous_samples: usize,
    pub restart_detected: bool,
    pub window_secs: u64,
    pub configured_find_node_qps: u32,
    pub actual_find_node_qps: f64,
    pub effective_find_node_p50: u32,
    pub effective_find_node_min: u32,
    pub throttled_time_pct: f64,
    pub severe_throttled_time_pct: f64,

    pub est_wire_tx_mbps: f64,
    pub est_wire_rx_mbps: f64,
    pub udp_tx_mbps: f64,
    pub udp_rx_mbps: f64,
    pub metadata_rx_mbps: f64,
    pub udp_total_pps: f64,

    pub meta_queue_fill_avg_pct: f64,
    pub meta_queue_fill_p95_pct: f64,
    pub meta_queue_full_time_pct: f64,
    pub meta_queue_wait_p50_ms: Option<u64>,
    pub meta_queue_wait_p95_ms: Option<u64>,
    pub meta_queue_evict_pct: f64,
    pub meta_queue_stale_pct: f64,
    pub hash_ingress_dropped_total: u64,

    pub configured_workers: usize,
    pub effective_workers: usize,
    pub fd_clamped: bool,
    pub worker_util_avg_pct: f64,
    pub worker_util_p95_pct: f64,
    pub fetch_duration_p50_ms: Option<u64>,
    pub fetch_duration_p95_ms: Option<u64>,
    pub torrents_per_min: f64,
    pub job_success_rate_pct: f64,
    pub peer_timeout_rate_pct: f64,
    pub timeout_tcp_connect_share_pct: f64,
    pub timeout_piece_recv_share_pct: f64,
    pub timeout_validation_share_pct: f64,

    pub response_rate_limited_pct: f64,
    pub udp_queue_full_total: u64,
    pub find_node_in_flight_p95_pct: f64,
    pub find_node_unmatched_pct: f64,
    pub node_pool_fill_p95_pct: f64,
    pub node_dropped_rate_limited_pct: f64,
    pub peer_lookup_rate_limited_pct: f64,
    pub peer_lookup_yield_per_started: f64,
    pub discovery_get_peers_limited_pct: f64,
    pub discovery_sample_hash_limited_pct: f64,

    pub local_cache_fill_pct: f64,
    pub redis_pool_exhausted_time_pct: f64,
    pub sink_backpressure_time_pct: f64,
    pub rss_p95_mb: f64,
    pub rss_growth_mb_per_hour: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TuningRecommendation {
    pub id: String,
    pub category: String,
    pub severity: RecommendationSeverity,
    pub action: RecommendationAction,
    pub param_path: String,
    pub param_label: String,
    pub current_display: String,
    pub recommended_display: String,
    pub current_value: Option<Value>,
    pub recommended_value: Option<Value>,
    pub delta_pct: Option<f64>,
    pub evidence: String,
    pub reason: String,
    pub apply_supported: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TuningReport {
    pub worker_id: String,
    pub source: String,
    pub generated_at: i64,
    pub config_version: i64,
    pub confidence: ConfidenceLevel,
    pub confidence_note: String,
    pub health_score: u8,
    pub regime: OperatingRegime,
    pub regime_title: String,
    pub regime_summary: String,
    pub constraints: TuningConstraints,
    pub summary: TuningWindowSummary,
    pub recommendations: Vec<TuningRecommendation>,
    pub increase_count: usize,
    pub decrease_count: usize,
    pub system_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TuningSessionStatusDto {
    pub worker_id: String,
    pub state: SessionState,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub target_duration_secs: u64,
    pub elapsed_secs: u64,
    pub sample_interval_secs: u64,
    pub sample_count: usize,
    pub memory_bytes: usize,
    pub progress_pct: f64,
    pub constraints: TuningConstraints,
}

#[derive(Debug, Clone, Serialize)]
pub struct RollingBufferStatusDto {
    pub sample_count: usize,
    pub max_samples: usize,
    pub window_secs: u64,
    pub memory_bytes: usize,
    pub confidence: ConfidenceLevel,
}

#[derive(Debug, Clone, Serialize)]
pub struct TuningWorkerOverviewDto {
    pub worker_id: String,
    pub session: Option<TuningSessionStatusDto>,
    pub rolling: RollingBufferStatusDto,
    pub report: Option<TuningReport>,
}

struct TuningSession {
    worker_id: String,
    state: SessionState,
    started_at: i64,
    finished_at: Option<i64>,
    target_duration_secs: u64,
    sample_interval_secs: u64,
    last_sample_at: i64,
    constraints: TuningConstraints,
    samples: Vec<CrawlerMetricSample>,
    final_report: Option<TuningReport>,
}

impl TuningSession {
    fn memory_bytes(&self) -> usize {
        self.samples
            .iter()
            .map(CrawlerMetricSample::approx_mem_bytes)
            .sum()
    }

    fn status_dto(&self, now: i64) -> TuningSessionStatusDto {
        let elapsed = match self.finished_at {
            Some(fin) => fin.saturating_sub(self.started_at).max(0) as u64,
            None => now.saturating_sub(self.started_at).max(0) as u64,
        };
        let progress_pct =
            if self.state == SessionState::Completed || self.target_duration_secs == 0 {
                100.0
            } else {
                ((elapsed as f64 / self.target_duration_secs as f64) * 100.0).clamp(0.0, 100.0)
            };
        TuningSessionStatusDto {
            worker_id: self.worker_id.clone(),
            state: self.state,
            started_at: self.started_at,
            finished_at: self.finished_at,
            target_duration_secs: self.target_duration_secs,
            elapsed_secs: elapsed,
            sample_interval_secs: self.sample_interval_secs,
            sample_count: self.samples.len(),
            memory_bytes: self.memory_bytes(),
            progress_pct,
            constraints: self.constraints.clone(),
        }
    }
}

struct AdvisorInner {
    rolling: HashMap<String, VecDeque<CrawlerMetricSample>>,
    sessions: HashMap<String, TuningSession>,
}

/// 纯内存运行时指标采集与智能参数优化顾问（自动 1 小时滚存 + 专项 1 小时采集任务）。
pub struct TuningAdvisor {
    db: Database,
    supervisor: Arc<Supervisor>,
    inner: RwLock<AdvisorInner>,
}

impl TuningAdvisor {
    pub fn new(db: Database, supervisor: Arc<Supervisor>) -> Arc<Self> {
        Arc::new(Self {
            db,
            supervisor,
            inner: RwLock::new(AdvisorInner {
                rolling: HashMap::new(),
                sessions: HashMap::new(),
            }),
        })
    }

    pub fn spawn_background_loop(self: &Arc<Self>, shutdown: CancellationToken) {
        let advisor = self.clone();
        tokio::spawn(async move {
            let mut ticker =
                tokio::time::interval(Duration::from_secs(DEFAULT_SAMPLE_INTERVAL_SECS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = ticker.tick() => {
                        advisor.sample_all_crawlers_once().await;
                    }
                }
            }
        });
    }

    pub async fn sample_all_crawlers_once(&self) {
        let Ok(workers) = self.supervisor.list_worker_statuses().await else {
            return;
        };
        let now = Utc::now().timestamp();
        let mut to_finalize: Vec<String> = Vec::new();

        {
            let mut guard = self.inner.write().await;
            for w in &workers {
                if w.role != WorkerRole::Crawler || w.runtime_state != WorkerRuntimeState::Running {
                    continue;
                }
                let Some(env) = &w.snapshot else {
                    continue;
                };
                let sample = CrawlerMetricSample::from_snapshot_payload(
                    now,
                    w.active_config_version,
                    &env.payload,
                );

                // 1) 更新自动 1 小时滚存环形缓冲
                let ring = guard
                    .rolling
                    .entry(w.id.clone())
                    .or_insert_with(|| VecDeque::with_capacity(ROLLING_RING_MAX_SAMPLES));
                if ring.len() >= ROLLING_RING_MAX_SAMPLES {
                    ring.pop_front();
                }
                ring.push_back(sample.clone());

                // 2) 若存在运行中的专项采集任务，按设定间隔记录样本并检查是否到点完成
                if let Some(sess) = guard.sessions.get_mut(&w.id)
                    && sess.state == SessionState::Collecting
                {
                    let due = sess.samples.is_empty()
                        || now.saturating_sub(sess.last_sample_at)
                            >= sess.sample_interval_secs as i64;
                    if due && sess.samples.len() < MAX_SESSION_SAMPLES {
                        sess.last_sample_at = now;
                        sess.samples.push(sample);
                    }
                    let elapsed = now.saturating_sub(sess.started_at).max(0) as u64;
                    if elapsed >= sess.target_duration_secs
                        || sess.samples.len() >= MAX_SESSION_SAMPLES
                    {
                        to_finalize.push(w.id.clone());
                    }
                }
            }
        }

        for worker_id in to_finalize {
            let _ = self.finish_session(&worker_id).await;
        }
    }

    pub async fn start_session(
        &self,
        worker_id: &str,
        duration_secs: u64,
        sample_interval_secs: u64,
        constraints: TuningConstraints,
    ) -> Result<TuningSessionStatusDto, String> {
        let statuses = self
            .supervisor
            .list_worker_statuses()
            .await
            .map_err(|e| format!("读取 Worker 状态失败: {e}"))?;
        let target = statuses
            .into_iter()
            .find(|w| w.id == worker_id)
            .ok_or_else(|| format!("Worker 实例 {worker_id} 不存在"))?;
        if target.role != WorkerRole::Crawler {
            return Err("当前智能参数诊断仅支持 DHT Crawler 实例".to_string());
        }
        if target.runtime_state != WorkerRuntimeState::Running {
            return Err(format!(
                "Worker 实例 {worker_id} 当前未运行 ({:?})，请先启动再采集指标",
                target.runtime_state
            ));
        }

        // 先拉一次即时快照作为第 1 个基线点
        self.supervisor.poll_worker_snapshot_once(worker_id).await;
        let fresh = self
            .supervisor
            .list_worker_statuses()
            .await
            .ok()
            .and_then(|list| list.into_iter().find(|w| w.id == worker_id));

        let now = Utc::now().timestamp();
        let target_duration_secs =
            duration_secs.clamp(MIN_TUNING_DURATION_SECS, MAX_TUNING_DURATION_SECS);
        let interval = sample_interval_secs.clamp(2, 60);

        let mut samples = Vec::with_capacity(
            ((target_duration_secs / interval) as usize + 4).min(MAX_SESSION_SAMPLES),
        );
        if let Some(w) = fresh
            && let Some(env) = &w.snapshot
        {
            samples.push(CrawlerMetricSample::from_snapshot_payload(
                now,
                w.active_config_version,
                &env.payload,
            ));
        }

        let session = TuningSession {
            worker_id: worker_id.to_string(),
            state: SessionState::Collecting,
            started_at: now,
            finished_at: None,
            target_duration_secs,
            sample_interval_secs: interval,
            last_sample_at: now,
            constraints,
            samples,
            final_report: None,
        };
        let dto = session.status_dto(now);
        let mut guard = self.inner.write().await;
        guard.sessions.insert(worker_id.to_string(), session);
        Ok(dto)
    }

    pub async fn finish_session(&self, worker_id: &str) -> Result<TuningSessionStatusDto, String> {
        let now = Utc::now().timestamp();
        let (samples, constraints) = {
            let mut guard = self.inner.write().await;
            let sess = guard
                .sessions
                .get_mut(worker_id)
                .ok_or_else(|| format!("未找到 {worker_id} 的采集任务"))?;
            if sess.state != SessionState::Collecting {
                return Ok(sess.status_dto(now));
            }
            sess.state = SessionState::Completed;
            sess.finished_at = Some(now);
            (sess.samples.clone(), sess.constraints.clone())
        };

        let report = self
            .build_report_for_samples(worker_id, "session", &samples, constraints)
            .await
            .ok();

        let mut guard = self.inner.write().await;
        let sess = guard
            .sessions
            .get_mut(worker_id)
            .ok_or_else(|| format!("未找到 {worker_id} 的采集任务"))?;
        sess.final_report = report;
        Ok(sess.status_dto(now))
    }

    pub async fn cancel_session(&self, worker_id: &str) -> bool {
        let mut guard = self.inner.write().await;
        guard.sessions.remove(worker_id).is_some()
    }

    pub async fn list_session_statuses(&self) -> Vec<TuningSessionStatusDto> {
        let now = Utc::now().timestamp();
        let guard = self.inner.read().await;
        guard.sessions.values().map(|s| s.status_dto(now)).collect()
    }

    pub async fn get_worker_overview(
        &self,
        worker_id: &str,
        preferred_source: Option<&str>,
        override_constraints: Option<TuningConstraints>,
    ) -> Result<TuningWorkerOverviewDto, String> {
        let now = Utc::now().timestamp();
        let (session_dto, rolling_dto, chosen_source, samples, constraints, cached_final) = {
            let guard = self.inner.read().await;
            let ring = guard.rolling.get(worker_id);
            let ring_samples: Vec<CrawlerMetricSample> = ring
                .map(|q| q.iter().cloned().collect())
                .unwrap_or_default();
            let ring_window_secs = window_duration_secs(&ring_samples);
            let ring_mem: usize = ring_samples
                .iter()
                .map(CrawlerMetricSample::approx_mem_bytes)
                .sum();
            let rolling_dto = RollingBufferStatusDto {
                sample_count: ring_samples.len(),
                max_samples: ROLLING_RING_MAX_SAMPLES,
                window_secs: ring_window_secs,
                memory_bytes: ring_mem,
                confidence: confidence_for_window(ring_samples.len(), ring_window_secs),
            };

            let sess = guard.sessions.get(worker_id);
            let session_dto = sess.map(|s| s.status_dto(now));

            let use_session = match preferred_source {
                Some("ring") => false,
                Some("session") => sess.is_some(),
                _ => sess.is_some(),
            };

            if use_session && let Some(s) = sess {
                let effective_constraints = override_constraints
                    .clone()
                    .unwrap_or_else(|| s.constraints.clone());
                let cached = if override_constraints.is_none() && s.state == SessionState::Completed
                {
                    s.final_report.clone()
                } else {
                    None
                };
                (
                    session_dto,
                    rolling_dto,
                    "session".to_string(),
                    s.samples.clone(),
                    effective_constraints,
                    cached,
                )
            } else {
                (
                    session_dto,
                    rolling_dto,
                    "ring".to_string(),
                    ring_samples,
                    override_constraints.unwrap_or_default(),
                    None,
                )
            }
        };

        let report = if let Some(cached) = cached_final {
            Some(cached)
        } else if samples.len() >= 2 {
            self.build_report_for_samples(worker_id, &chosen_source, &samples, constraints)
                .await
                .ok()
        } else {
            None
        };

        Ok(TuningWorkerOverviewDto {
            worker_id: worker_id.to_string(),
            session: session_dto,
            rolling: rolling_dto,
            report,
        })
    }

    async fn build_report_for_samples(
        &self,
        worker_id: &str,
        source: &str,
        samples: &[CrawlerMetricSample],
        constraints: TuningConstraints,
    ) -> Result<TuningReport, String> {
        let cfg_row = self
            .db
            .get_active_worker_config(worker_id)
            .await
            .map_err(|e| format!("读取 Worker 配置失败: {e}"))?
            .ok_or_else(|| format!("未找到 Worker {worker_id} 的激活配置"))?;
        let cfg: CrawlerWorkerConfig = serde_json::from_str(&cfg_row.config_json)
            .map_err(|e| format!("解析 Crawler 配置失败: {e}"))?;
        let soft_nofile = current_soft_open_file_limit();
        Ok(analyze_crawler_samples(
            worker_id,
            source,
            cfg_row.version,
            &cfg,
            samples,
            constraints,
            soft_nofile,
        ))
    }

    /// 供单元测试注入合成采样序列。
    #[cfg(test)]
    pub async fn inject_rolling_samples(&self, worker_id: &str, samples: Vec<CrawlerMetricSample>) {
        let mut guard = self.inner.write().await;
        let q = guard.rolling.entry(worker_id.to_string()).or_default();
        q.clear();
        for s in samples {
            q.push_back(s);
        }
    }
}

/// 有效参数（将 L3/PeerLookup Optional 覆写与所属 `crawl_profile` 默认值合并后的真实生效值）。
#[derive(Debug, Clone)]
struct EffectiveCrawlerParams {
    find_node_qps: u32,
    find_node_max_in_flight: usize,
    request_timeout_secs: u64,
    max_new_dest_per_min: u32,
    max_response_rate: u32,
    max_response_bytes: u64,
    metadata_pressure_floor_pct: u8,
    max_replacements_per_min: u32,
    pool_capacity: usize,
    revisit_interval_secs: u64,
    peer_lookup_rate: u32,
    peer_lookup_max_active: usize,
    prio_chan_cap: usize,
    disc_chan_cap: usize,
}

impl EffectiveCrawlerParams {
    fn resolve(cfg: &CrawlerWorkerConfig) -> Self {
        let def = crawl_profile_defaults(&cfg.crawl_profile)
            .or_else(|| crawl_profile_defaults("P1"))
            .unwrap_or(CrawlProfileDefaults {
                find_node_rate_per_sec: 878,
                find_node_burst: 176,
                find_node_max_in_flight: 4_498,
                max_new_destinations_per_minute: 52_708,
                max_replacements_per_minute: 98_828,
                max_response_rate_per_sec: 16_471,
                max_response_bytes_per_sec: 9_211_412,
                max_response_rate_per_source: 16_471,
                max_in_flight_per_subnet: 35,
                peer_lookup_rate_per_sec: 32,
                peer_lookup_burst: 32,
                peer_lookup_max_active: 64,
                pool_capacity: 100_000,
                responsive_capacity: 16_384,
                routing_snapshot_size: 4_096,
            });
        let rl = &cfg.crawl.rate_limit;
        let pool = &cfg.crawl.pool;
        let pl = &cfg.peer_lookup;
        let sc = &cfg.crawl.scheduler;
        Self {
            find_node_qps: rl
                .max_find_node_rate_per_sec
                .unwrap_or(def.find_node_rate_per_sec),
            find_node_max_in_flight: rl.max_in_flight.unwrap_or(def.find_node_max_in_flight),
            request_timeout_secs: rl.request_timeout_secs.unwrap_or(2),
            max_new_dest_per_min: rl
                .max_new_destinations_per_minute
                .unwrap_or(def.max_new_destinations_per_minute),
            max_response_rate: rl
                .max_response_rate_per_sec
                .unwrap_or(def.max_response_rate_per_sec),
            max_response_bytes: rl
                .max_response_bytes_per_sec
                .unwrap_or(def.max_response_bytes_per_sec),
            metadata_pressure_floor_pct: rl.metadata_pressure_floor_percent.unwrap_or(25),
            max_replacements_per_min: rl
                .max_replacements_per_minute
                .unwrap_or(def.max_replacements_per_minute),
            pool_capacity: pool.capacity.unwrap_or(def.pool_capacity),
            revisit_interval_secs: pool.revisit_interval_secs.unwrap_or(0),
            peer_lookup_rate: pl
                .max_lookups_per_second
                .unwrap_or(def.peer_lookup_rate_per_sec),
            peer_lookup_max_active: pl.max_active_lookups.unwrap_or(def.peer_lookup_max_active),
            prio_chan_cap: sc.priority_event_channel_capacity.unwrap_or(8_192),
            disc_chan_cap: sc.discovery_event_channel_capacity.unwrap_or(16_384),
        }
    }
}

/// 纯函数智能分析核心：给定配置与内存采样序列，执行因果根因分类并产出不冲突的调增/调减建议。
pub fn analyze_crawler_samples(
    worker_id: &str,
    source: &str,
    config_version: i64,
    cfg: &CrawlerWorkerConfig,
    all_samples: &[CrawlerMetricSample],
    constraints: TuningConstraints,
    system_soft_nofile: Option<usize>,
) -> TuningReport {
    let eff = EffectiveCrawlerParams::resolve(cfg);
    let (seg, restart_detected) = latest_continuous_segment(all_samples);
    let now = Utc::now().timestamp();

    let first = seg.first();
    let last = seg.last();
    let window_secs = match (first, last) {
        (Some(f), Some(l)) => l.captured_at.saturating_sub(f.captured_at).max(1) as u64,
        _ => 0,
    };
    let dt = window_secs.max(1) as f64;
    let n = seg.len().max(1) as f64;

    let (confidence, confidence_note) = match confidence_for_window(seg.len(), window_secs) {
        ConfidenceLevel::High => (
            ConfidenceLevel::High,
            format!(
                "高置信度：已覆盖 {} 分钟稳态样本（{} 个点），适合直接按建议调优",
                window_secs / 60,
                seg.len()
            ),
        ),
        ConfidenceLevel::Medium => (
            ConfidenceLevel::Medium,
            format!(
                "中置信度：已采集 {} 分钟（{} 个点），主要瓶颈已显性化，建议继续采集至 45–60 分钟复核",
                window_secs / 60,
                seg.len()
            ),
        ),
        ConfidenceLevel::Low => (
            ConfidenceLevel::Low,
            format!(
                "低置信度：当前仅采集 {} 秒（{} 个点），可作初步方向参考，完整 1 小时采集更准确",
                window_secs,
                seg.len()
            ),
        ),
        ConfidenceLevel::Insufficient => (
            ConfidenceLevel::Insufficient,
            "样本不足（需至少 2 个连续采样点），请稍等几秒后刷新".to_string(),
        ),
    };

    // 1) 汇总时间序列分位数与百分比指标
    let mut eff_rates: Vec<u32> = Vec::with_capacity(seg.len());
    let mut queue_fills: Vec<f64> = Vec::with_capacity(seg.len());
    let mut worker_utils: Vec<f64> = Vec::with_capacity(seg.len());
    let mut fn_inflight_pcts: Vec<f64> = Vec::with_capacity(seg.len());
    let mut pool_fills: Vec<f64> = Vec::with_capacity(seg.len());
    let mut prio_fills: Vec<f64> = Vec::with_capacity(seg.len());
    let mut disc_fills: Vec<f64> = Vec::with_capacity(seg.len());
    let mut ingress_fills: Vec<f64> = Vec::with_capacity(seg.len());
    let mut rss_mbs: Vec<f64> = Vec::with_capacity(seg.len());

    let mut throttled_ticks = 0usize;
    let mut severe_throttled_ticks = 0usize;
    let mut queue_full_ticks = 0usize;
    let mut redis_exhausted_ticks = 0usize;
    let mut sink_bp_ticks = 0usize;

    for s in seg {
        eff_rates.push(s.find_node_effective_rate_per_sec);
        if eff.find_node_qps > 0 {
            let ratio =
                f64::from(s.find_node_effective_rate_per_sec) / f64::from(eff.find_node_qps);
            if ratio < 0.92 {
                throttled_ticks += 1;
            }
            if ratio < 0.50 {
                severe_throttled_ticks += 1;
            }
        }
        let q_max = s.metadata_queue_max.max(cfg.metadata.max_queue_size).max(1) as f64;
        let q_pct = (s.metadata_queue_depth as f64 / q_max * 100.0).min(100.0);
        queue_fills.push(q_pct);
        if q_pct >= 90.0 {
            queue_full_ticks += 1;
        }

        let eff_w = s
            .effective_metadata_workers
            .max(1)
            .min(cfg.metadata.max_worker_count.max(1)) as f64;
        worker_utils.push((s.metadata_in_flight as f64 / eff_w * 100.0).min(100.0));

        let fn_max = s
            .find_node_in_flight_max
            .max(eff.find_node_max_in_flight)
            .max(1) as f64;
        fn_inflight_pcts.push((s.find_node_in_flight as f64 / fn_max * 100.0).min(100.0));

        let p_max = s.node_pool_capacity.max(eff.pool_capacity).max(1) as f64;
        pool_fills.push((s.node_pool_size as f64 / p_max * 100.0).min(100.0));

        let pr_max = s
            .crawl_priority_queue_capacity
            .max(eff.prio_chan_cap)
            .max(1) as f64;
        prio_fills.push((s.crawl_priority_queue_depth as f64 / pr_max * 100.0).min(100.0));

        let dc_max = s
            .crawl_discovery_queue_capacity
            .max(eff.disc_chan_cap)
            .max(1) as f64;
        disc_fills.push((s.crawl_discovery_queue_depth as f64 / dc_max * 100.0).min(100.0));

        let ing_max = s
            .hash_ingress_queue_capacity
            .max(cfg.hash_queue_capacity)
            .max(1) as f64;
        ingress_fills.push((s.hash_ingress_queue_depth as f64 / ing_max * 100.0).min(100.0));

        if s.redis_pool_size > 0 && s.redis_pool_available <= 1 {
            redis_exhausted_ticks += 1;
        }
        let sink_cap = cfg.push_concurrency.saturating_mul(16).max(64);
        if s.sink_queue_depth.saturating_mul(100) >= sink_cap.saturating_mul(75) {
            sink_bp_ticks += 1;
        }
        rss_mbs.push(s.rss_bytes as f64 / (1024.0 * 1024.0));
    }

    let throttled_time_pct = (throttled_ticks as f64 / n) * 100.0;
    let severe_throttled_time_pct = (severe_throttled_ticks as f64 / n) * 100.0;
    let meta_queue_full_time_pct = (queue_full_ticks as f64 / n) * 100.0;
    let redis_pool_exhausted_time_pct = (redis_exhausted_ticks as f64 / n) * 100.0;
    let sink_backpressure_time_pct = (sink_bp_ticks as f64 / n) * 100.0;

    let meta_queue_fill_avg_pct = mean_f64(&queue_fills);
    let meta_queue_fill_p95_pct = percentile_f64(&queue_fills, 0.95);
    let worker_util_avg_pct = mean_f64(&worker_utils);
    let worker_util_p95_pct = percentile_f64(&worker_utils, 0.95);
    let find_node_in_flight_p95_pct = percentile_f64(&fn_inflight_pcts, 0.95);
    let node_pool_fill_p95_pct = percentile_f64(&pool_fills, 0.95);
    let prio_fill_p95_pct = percentile_f64(&prio_fills, 0.95);
    let disc_fill_p95_pct = percentile_f64(&disc_fills, 0.95);
    let ingress_fill_p95_pct = percentile_f64(&ingress_fills, 0.95);
    let rss_p95_mb = percentile_f64(&rss_mbs, 0.95);
    let effective_find_node_p50 = percentile_u32(&eff_rates, 0.50);
    let effective_find_node_min = eff_rates.iter().copied().min().unwrap_or(0);

    // 2) 窗口增量计算（过滤进程重启前历史累计数）
    let (d_queries, d_revisit, d_responses, d_unmatched, d_send_fail, d_udp_qfull) =
        match (first, last) {
            (Some(f), Some(l)) => {
                let q_f = f
                    .queries_new
                    .saturating_add(f.queries_revisit)
                    .saturating_add(f.queries_bootstrap);
                let q_l = l
                    .queries_new
                    .saturating_add(l.queries_revisit)
                    .saturating_add(l.queries_bootstrap);
                (
                    q_l.saturating_sub(q_f),
                    l.queries_revisit.saturating_sub(f.queries_revisit),
                    l.responses.saturating_sub(f.responses),
                    l.unmatched_responses.saturating_sub(f.unmatched_responses),
                    l.send_failures.saturating_sub(f.send_failures),
                    l.udp_queue_full.saturating_sub(f.udp_queue_full),
                )
            }
            _ => (0, 0, 0, 0, 0, 0),
        };

    let (
        d_udp_rx_bytes,
        d_udp_tx_bytes,
        d_udp_rx_pkts,
        d_udp_tx_pkts,
        d_meta_bytes,
        d_resp_norm,
        d_resp_fb,
        d_resp_lim,
        d_resp_fail,
    ) = match (first, last) {
        (Some(f), Some(l)) => (
            l.udp_rx_bytes.saturating_sub(f.udp_rx_bytes),
            l.udp_tx_bytes.saturating_sub(f.udp_tx_bytes),
            l.udp_rx_packets.saturating_sub(f.udp_rx_packets),
            l.udp_tx_packets.saturating_sub(f.udp_tx_packets),
            l.metadata_bytes_downloaded
                .saturating_sub(f.metadata_bytes_downloaded),
            l.response_normal.saturating_sub(f.response_normal),
            l.response_fallback.saturating_sub(f.response_fallback),
            l.response_rate_limited
                .saturating_sub(f.response_rate_limited)
                .max(
                    l.udp_responses_rate_limited
                        .saturating_sub(f.udp_responses_rate_limited),
                ),
            l.response_send_failed
                .saturating_sub(f.response_send_failed),
        ),
        _ => (0, 0, 0, 0, 0, 0, 0, 0, 0),
    };

    let (
        d_q_ins,
        d_q_evict,
        d_q_stale,
        d_ing_drop,
        d_torrents,
        d_meta_acc,
        d_meta_fail,
        d_peer_att,
        d_peer_to,
    ) = match (first, last) {
        (Some(f), Some(l)) => (
            l.metadata_queue_inserted
                .saturating_sub(f.metadata_queue_inserted),
            l.metadata_queue_evicted
                .saturating_sub(f.metadata_queue_evicted),
            l.metadata_queue_stale
                .saturating_sub(f.metadata_queue_stale),
            l.hash_ingress_dropped
                .saturating_sub(f.hash_ingress_dropped),
            l.torrents_downloaded.saturating_sub(f.torrents_downloaded),
            l.metadata_fetch_accepted
                .saturating_sub(f.metadata_fetch_accepted),
            l.metadata_fetch_failed
                .saturating_sub(f.metadata_fetch_failed),
            l.metadata_peer_attempts
                .saturating_sub(f.metadata_peer_attempts),
            l.metadata_failure_timeout
                .saturating_sub(f.metadata_failure_timeout)
                .max(
                    l.metadata_peer_timeouts
                        .saturating_sub(f.metadata_peer_timeouts),
                ),
        ),
        _ => (0, 0, 0, 0, 0, 0, 0, 0, 0),
    };

    let (d_to_tcp, d_to_hs, d_to_piece, d_to_val, d_io_other) = match (first, last) {
        (Some(f), Some(l)) => {
            let tcp = l.timeout_tcp_connect.saturating_sub(f.timeout_tcp_connect);
            let hs = l
                .timeout_base_handshake
                .saturating_sub(f.timeout_base_handshake)
                .saturating_add(l.timeout_ext_send.saturating_sub(f.timeout_ext_send))
                .saturating_add(l.timeout_ext_recv.saturating_sub(f.timeout_ext_recv));
            let piece = l
                .timeout_piece_send
                .saturating_sub(f.timeout_piece_send)
                .saturating_add(l.timeout_piece_recv.saturating_sub(f.timeout_piece_recv));
            let val = l.timeout_validation.saturating_sub(f.timeout_validation);
            let io_other = l.conn_err_io_other.saturating_sub(f.conn_err_io_other);
            (tcp, hs, piece, val, io_other)
        }
        _ => (0, 0, 0, 0, 0),
    };

    let (
        d_pl_req,
        d_pl_start,
        d_pl_lim,
        d_pl_found,
        d_pl_q,
        d_pl_to,
        d_node_adm,
        d_node_rep,
        d_node_drop_lim,
    ) = match (first, last) {
        (Some(f), Some(l)) => (
            l.peer_lookup_requested
                .saturating_sub(f.peer_lookup_requested),
            l.peer_lookup_started.saturating_sub(f.peer_lookup_started),
            l.peer_lookup_rate_limited
                .saturating_sub(f.peer_lookup_rate_limited),
            l.peer_lookup_peers_found
                .saturating_sub(f.peer_lookup_peers_found),
            l.peer_lookup_queries.saturating_sub(f.peer_lookup_queries),
            l.peer_lookup_timeouts
                .saturating_sub(f.peer_lookup_timeouts),
            l.node_admitted.saturating_sub(f.node_admitted),
            l.node_replaced.saturating_sub(f.node_replaced),
            l.node_dropped_rate_limited
                .saturating_sub(f.node_dropped_rate_limited),
        ),
        _ => (0, 0, 0, 0, 0, 0, 0, 0, 0),
    };

    let (
        d_gp_obs,
        d_gp_lim,
        d_gp_enq,
        d_sm_enq,
        d_sm_lim,
        d_sm_cd_full,
        d_sm_node_drop,
        d_redis_acq,
        d_redis_idx,
        d_redis_err,
        d_push_err,
        d_sink_drop,
        d_hot_drop,
    ) = match (first, last) {
        (Some(f), Some(l)) => (
            l.disc_get_peers_observed
                .saturating_sub(f.disc_get_peers_observed),
            l.disc_get_peers_rate_limited
                .saturating_sub(f.disc_get_peers_rate_limited),
            l.disc_get_peers_enqueued
                .saturating_sub(f.disc_get_peers_enqueued),
            l.disc_sample_hashes_enqueued
                .saturating_sub(f.disc_sample_hashes_enqueued),
            l.disc_hash_rate_limited
                .saturating_sub(f.disc_hash_rate_limited),
            l.disc_sample_cooldown_full
                .saturating_sub(f.disc_sample_cooldown_full),
            l.disc_node_cap_dropped
                .saturating_sub(f.disc_node_cap_dropped),
            l.redis_claim_acquired
                .saturating_sub(f.redis_claim_acquired),
            l.redis_claim_indexed.saturating_sub(f.redis_claim_indexed),
            l.redis_claim_errors.saturating_sub(f.redis_claim_errors),
            l.torrents_push_errors
                .saturating_sub(f.torrents_push_errors),
            l.torrents_sink_dropped
                .saturating_sub(f.torrents_sink_dropped),
            l.hot_dropped.saturating_sub(f.hot_dropped),
        ),
        _ => (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),
    };

    // 增量直方图（仅反映观测窗口内真实排队与抓取耗时）
    let (queue_wait_p50, queue_wait_p95, fetch_dur_p50, fetch_dur_p95) = match (first, last) {
        (Some(f), Some(l)) => {
            let qw = l.queue_wait_ms.delta_since(&f.queue_wait_ms);
            let fd = l.fetch_duration_ms.delta_since(&f.fetch_duration_ms);
            (
                qw.percentile(0.50),
                qw.percentile(0.95),
                fd.percentile(0.50),
                fd.percentile(0.95),
            )
        }
        _ => (None, None, None, None),
    };

    let actual_find_node_qps = d_queries as f64 / dt;
    let udp_rx_bps = d_udp_rx_bytes as f64 / dt;
    let udp_tx_bps = d_udp_tx_bytes as f64 / dt;
    let metadata_rx_bps = d_meta_bytes as f64 / dt;
    let udp_rx_pps = d_udp_rx_pkts as f64 / dt;
    let udp_tx_pps = d_udp_tx_pkts as f64 / dt;
    let udp_total_pps = udp_rx_pps + udp_tx_pps;

    // 估算线速带宽：UDP 加上 L2+IP+UDP 头 (~44B/pkt)，BEP-9 TCP 加上握手/失败连接/TCP头开销 (~1.32x RX + ~0.22x TX)
    let est_wire_rx_bps =
        udp_rx_bps + udp_rx_pps * 44.0 + metadata_rx_bps * 1.32 + (d_peer_att as f64 / dt) * 260.0;
    let est_wire_tx_bps =
        udp_tx_bps + udp_tx_pps * 44.0 + metadata_rx_bps * 0.18 + (d_peer_att as f64 / dt) * 320.0;
    let est_wire_rx_mbps = est_wire_rx_bps * 8.0 / 1_000_000.0;
    let est_wire_tx_mbps = est_wire_tx_bps * 8.0 / 1_000_000.0;
    let udp_rx_mbps = udp_rx_bps * 8.0 / 1_000_000.0;
    let udp_tx_mbps = udp_tx_bps * 8.0 / 1_000_000.0;
    let metadata_rx_mbps = metadata_rx_bps * 8.0 / 1_000_000.0;

    let effective_workers = last
        .map(|s| s.effective_metadata_workers)
        .filter(|&w| w > 0)
        .unwrap_or(cfg.metadata.max_worker_count);
    let fd_clamped = effective_workers < cfg.metadata.max_worker_count;

    let meta_queue_evict_pct = pct(d_q_evict, d_q_ins.max(1));
    let meta_queue_stale_pct = pct(d_q_stale, d_q_ins.saturating_add(d_q_stale).max(1));
    let total_jobs = d_meta_acc.saturating_add(d_meta_fail);
    let job_success_rate_pct = if total_jobs > 0 {
        pct(d_meta_acc, total_jobs)
    } else {
        pct(d_torrents, d_q_ins.max(1)).min(100.0)
    };
    let actual_drain_rate_per_sec = (total_jobs as f64 / dt).max(d_torrents as f64 / dt);
    let torrents_per_min = d_torrents as f64 * 60.0 / dt;
    let peer_timeout_rate_pct = pct(d_peer_to, d_peer_att.max(1));

    let total_stage_to = d_to_tcp
        .saturating_add(d_to_hs)
        .saturating_add(d_to_piece)
        .saturating_add(d_to_val);
    let timeout_tcp_connect_share_pct = pct(d_to_tcp, total_stage_to.max(1));
    let timeout_piece_recv_share_pct = pct(d_to_piece, total_stage_to.max(1));
    let timeout_validation_share_pct = pct(d_to_val, total_stage_to.max(1));

    let total_resp_decisions = d_resp_norm
        .saturating_add(d_resp_fb)
        .saturating_add(d_resp_lim);
    let response_rate_limited_pct = pct(d_resp_lim, total_resp_decisions.max(1));
    let response_fallback_pct = pct(d_resp_fb, total_resp_decisions.max(1));
    let find_node_unmatched_pct = pct(d_unmatched, d_responses.saturating_add(d_unmatched).max(1));
    let revisit_share_pct = pct(d_revisit, d_queries.max(1));

    let total_node_events = d_node_adm
        .saturating_add(d_node_rep)
        .saturating_add(d_node_drop_lim);
    let node_dropped_rate_limited_pct = pct(d_node_drop_lim, total_node_events.max(1));

    let peer_lookup_rate_limited_pct = pct(d_pl_lim, d_pl_req.max(1));
    let peer_lookup_yield_per_started = if d_pl_start > 0 {
        d_pl_found as f64 / d_pl_start as f64
    } else {
        0.0
    };
    let peer_lookup_timeout_pct = pct(d_pl_to, d_pl_q.max(1));

    let discovery_get_peers_limited_pct = pct(
        d_gp_lim,
        d_gp_lim.saturating_add(d_gp_enq).max(d_gp_obs).max(1),
    );
    let discovery_sample_hash_limited_pct = pct(d_sm_lim, d_sm_lim.saturating_add(d_sm_enq).max(1));

    let local_cache_fill_pct = last.map_or(0.0, |s| {
        (s.local_cache_entries as f64 / cfg.metadata_cache_capacity.max(1) as f64 * 100.0)
            .min(100.0)
    });
    let peer_fail_cache_fill_pct = last.map_or(0.0, |s| {
        (s.metadata_peer_failure_cache_entries as f64
            / cfg.metadata.peer_failure_cache_capacity.max(1) as f64
            * 100.0)
            .min(100.0)
    });
    let redis_indexed_miss_pct = pct(d_redis_idx, d_redis_acq.saturating_add(d_redis_idx).max(1));

    let rss_growth_mb_per_hour = match (first, last) {
        (Some(f), Some(l)) if window_secs >= 60 => {
            let diff_mb = (l.rss_bytes as f64 - f.rss_bytes as f64) / (1024.0 * 1024.0);
            diff_mb * (3600.0 / dt)
        }
        _ => 0.0,
    };

    // 带宽余量判定（若用户填写了服务器上下行带宽上限）
    let bw_up_util_pct = constraints
        .up_mbps
        .filter(|&m| m > 0.0)
        .map(|cap| (est_wire_tx_mbps / cap) * 100.0);
    let bw_down_util_pct = constraints
        .down_mbps
        .filter(|&m| m > 0.0)
        .map(|cap| (est_wire_rx_mbps / cap) * 100.0);
    let bw_ceiling_pct = match constraints.goal {
        TuningGoal::Throughput => 90.0,
        TuningGoal::Balanced => 82.0,
        TuningGoal::LowResource => 65.0,
    };
    let tx_saturated = bw_up_util_pct.is_some_and(|p| p >= bw_ceiling_pct);
    let rx_saturated = bw_down_util_pct.is_some_and(|p| p >= bw_ceiling_pct);
    let bw_saturated = tx_saturated || rx_saturated;
    let bw_headroom_ratio = match (bw_up_util_pct, bw_down_util_pct) {
        (Some(u), Some(d)) => ((bw_ceiling_pct - u.max(d)) / bw_ceiling_pct).clamp(0.0, 1.0),
        (Some(u), None) => ((bw_ceiling_pct - u) / bw_ceiling_pct).clamp(0.0, 1.0),
        (None, Some(d)) => ((bw_ceiling_pct - d) / bw_ceiling_pct).clamp(0.0, 1.0),
        (None, None) => match constraints.goal {
            TuningGoal::Throughput => 0.35,
            TuningGoal::Balanced => 0.25,
            TuningGoal::LowResource => 0.0,
        },
    };

    // 3) 全局因果根因分级（决定允许调增还是必须先调减，彻底防止建议打架）
    let udp_cpu_saturated = d_udp_qfull > 0
        || (d_send_fail.saturating_add(d_resp_fail) > 20
            && pct(
                d_send_fail.saturating_add(d_resp_fail),
                d_queries.saturating_add(d_resp_norm).max(1),
            ) >= 1.0)
        || (total_stage_to >= 50 && timeout_validation_share_pct >= 4.0);
    let redis_sink_choked = redis_pool_exhausted_time_pct >= 10.0
        || d_sink_drop > 0
        || d_redis_err >= 10
        || d_push_err >= 10
        || sink_backpressure_time_pct >= 10.0;
    let overfed = throttled_time_pct >= 20.0
        || meta_queue_fill_p95_pct >= 85.0
        || meta_queue_evict_pct >= 5.0
        || queue_wait_p95.is_some_and(|ms| ms >= 15_000);
    let underfed = worker_util_p95_pct < 65.0
        && meta_queue_fill_p95_pct < 25.0
        && !udp_cpu_saturated
        && !bw_saturated
        && !redis_sink_choked
        && !fd_clamped;

    let (regime, regime_title, regime_summary) = if fd_clamped {
        (
            OperatingRegime::NofileClamped,
            "文件描述符 (NOFILE) 硬瓶颈".to_string(),
            format!(
                "配置 BEP-9 并发 {} 已被系统 ulimit -n 强制收敛至 {}，提升并发参数前需先调高进程 NOFILE 上限。",
                cfg.metadata.max_worker_count, effective_workers
            ),
        )
    } else if udp_cpu_saturated {
        (
            OperatingRegime::CpuUdpSaturated,
            "UDP 收包 / CPU 调度饱和".to_string(),
            format!(
                "观测窗口内出现 UDP 队列丢包 {} 次、发送失败 {} 次或校验超时占比 {:.1}%，已抑制所有提速建议并优先收敛负载。",
                d_udp_qfull, d_send_fail, timeout_validation_share_pct
            ),
        )
    } else if redis_sink_choked {
        (
            OperatingRegime::RedisSinkBottleneck,
            "下游 Redis / 推流通道拥堵".to_string(),
            format!(
                "Redis 连接池耗尽占比 {:.1}%、推流反压占比 {:.1}%（丢弃 {} 条），需先拓宽下游推送能力再放开上游抓取。",
                redis_pool_exhausted_time_pct, sink_backpressure_time_pct, d_sink_drop
            ),
        )
    } else if overfed && bw_saturated {
        (
            OperatingRegime::BandwidthCeiling,
            "带宽触顶叠加队列反压".to_string(),
            format!(
                "带宽已达上限（预估 TX {:.1} / RX {:.1} Mbps）且队列 P95 {:.0}% / 降速占比 {:.0}%，应削减无效上游供给而非继续加并发。",
                est_wire_tx_mbps, est_wire_rx_mbps, meta_queue_fill_p95_pct, throttled_time_pct
            ),
        )
    } else if overfed {
        (
            OperatingRegime::OverfedBackpressure,
            "上游供给过载 → 队列反压降速".to_string(),
            format!(
                "观测期有 {:.0}% 时间触发 metadata_pressure 降速（队列 P95 {:.0}%，排队等待 P95 {}），上游 Hash 产速持续超过 BEP-9 消化能力。",
                throttled_time_pct,
                meta_queue_fill_p95_pct,
                fmt_opt_ms(queue_wait_p95)
            ),
        )
    } else if underfed {
        (
            OperatingRegime::UnderfedStarvation,
            "下游并发饥饿 · 有提速挖潜空间".to_string(),
            format!(
                "BEP-9 并发 P95 利用率仅 {:.0}% 且队列水位 P95 {:.0}%，系统资源与队列充裕，可有序调高上游探路或采样供给。",
                worker_util_p95_pct, meta_queue_fill_p95_pct
            ),
        )
    } else {
        (
            OperatingRegime::BalancedHealthy,
            "稳态均衡运行".to_string(),
            format!(
                "队列 P95 {:.0}%、BEP-9 利用率 P95 {:.0}%、降速占比 {:.1}%，整体供需健康，以下为精细化效率与资源微调建议。",
                meta_queue_fill_p95_pct, worker_util_p95_pct, throttled_time_pct
            ),
        )
    };

    let allow_scale_up = !udp_cpu_saturated
        && !redis_sink_choked
        && !bw_saturated
        && constraints.goal != TuningGoal::LowResource;

    // 4) 逐子系统产出因果清晰、互不冲突的调优建议
    let mut recs: Vec<TuningRecommendation> = Vec::new();
    let mut score_penalty: f64 = 0.0;

    // --- 规则 1: OS NOFILE 检查 ---
    let target_workers_for_fd = cfg.metadata.max_worker_count;
    let needed_nofile = ((target_workers_for_fd.saturating_mul(12).saturating_add(132)) * 100)
        .div_ceil(70)
        .max(16_384);
    let rec_nofile = next_nofile_tier(needed_nofile);
    if fd_clamped || system_soft_nofile.is_some_and(|cur| cur < needed_nofile) {
        score_penalty += if fd_clamped { 22.0 } else { 10.0 };
        let cur_fd = system_soft_nofile
            .map(|v| v.to_string())
            .unwrap_or_else(|| "未知".to_string());
        recs.push(TuningRecommendation {
            id: "sys_nofile".to_string(),
            category: "system".to_string(),
            severity: if fd_clamped {
                RecommendationSeverity::Critical
            } else {
                RecommendationSeverity::Warning
            },
            action: RecommendationAction::System,
            param_path: "system.nofile".to_string(),
            param_label: "系统文件描述符上限 (ulimit -n)".to_string(),
            current_display: cur_fd,
            recommended_display: format!("≥ {rec_nofile}"),
            current_value: system_soft_nofile.map(Value::from),
            recommended_value: Some(Value::from(rec_nofile)),
            delta_pct: None,
            evidence: format!(
                "配置 BEP-9 并发 {}（12 路竞速需安全 FD ≥ {}），当前实际生效并发仅 {}",
                cfg.metadata.max_worker_count, needed_nofile, effective_workers
            ),
            reason: "请先在面板顶部「NOFILE LIMIT」或 Docker/systemd LimitNOFILE 提额，否则提高并发配置不会生效。".to_string(),
            apply_supported: false,
        });
    }

    // --- 规则 2: 下游 Redis / Push 并发检查 ---
    if redis_sink_choked {
        score_penalty += 18.0;
        if sink_backpressure_time_pct >= 5.0 || d_sink_drop > 0 || d_hot_drop > 0 {
            let next_push = (cfg.push_concurrency.saturating_mul(2)).clamp(16, 256);
            if next_push > cfg.push_concurrency {
                recs.push(make_num_rec(
                    "inc_push_concurrency",
                    "memory_cache",
                    RecommendationSeverity::Critical,
                    RecommendationAction::Increase,
                    "push_concurrency",
                    "Redis 推送并发数",
                    cfg.push_concurrency as u64,
                    next_push as u64,
                    format!(
                        "推流队列高水位占比 {:.1}%，丢弃种子 {} 条、热度丢弃 {} 条",
                        sink_backpressure_time_pct, d_sink_drop, d_hot_drop
                    ),
                    "下游推流反压会直接触发 admission_defer 阻塞整个 BEP-9 抓取链路，优先加宽推送协程数。",
                ));
            }
        }
        if redis_pool_exhausted_time_pct >= 5.0 || d_redis_err >= 10 {
            let cur_pool = last.map(|s| s.redis_pool_size).unwrap_or(100);
            let rec_pool = (cur_pool.saturating_mul(2)).clamp(128, 512);
            recs.push(TuningRecommendation {
                id: "sys_redis_pool".to_string(),
                category: "system".to_string(),
                severity: RecommendationSeverity::Critical,
                action: RecommendationAction::System,
                param_path: "service.redis.crawler_pool_size".to_string(),
                param_label: "Redis Crawler 连接池大小".to_string(),
                current_display: cur_pool.to_string(),
                recommended_display: rec_pool.to_string(),
                current_value: Some(Value::from(cur_pool)),
                recommended_value: Some(Value::from(rec_pool)),
                delta_pct: Some(delta_pct(cur_pool as f64, rec_pool as f64)),
                evidence: format!(
                    "Redis 连接池可用数 ≤1 占比 {:.1}%，窗口内 claim 错误 {} 次 / push 错误 {} 次",
                    redis_pool_exhausted_time_pct, d_redis_err, d_push_err
                ),
                reason:
                    "前往「服务设置 -> Redis」调大 crawler_pool_size，避免高并发准入检查排队超时。"
                        .to_string(),
                apply_supported: false,
            });
        }
    }

    // --- 规则 3: UDP / CPU 饱和紧急降速 ---
    let mut find_node_handled = false;
    let mut worker_handled = false;

    if udp_cpu_saturated {
        score_penalty += 24.0;
        let target_fn = snap_u32(
            (actual_find_node_qps.max(f64::from(effective_find_node_p50)) * 0.72) as u32,
            20,
            20_000,
        );
        if target_fn < eff.find_node_qps {
            find_node_handled = true;
            recs.push(make_num_rec(
                "dec_find_node_udp_sat",
                "udp_crawl",
                RecommendationSeverity::Critical,
                RecommendationAction::Decrease,
                "crawl.rate_limit.max_find_node_rate_per_sec",
                "主动探测 find_node 速率 (/s)",
                u64::from(eff.find_node_qps),
                u64::from(target_fn),
                format!(
                    "窗口内 UDP 队列溢出 {} 次、发送失败 {} 次，当前实际探测 {:.0}/s",
                    d_udp_qfull, d_send_fail, actual_find_node_qps
                ),
                "UDP 收发通道已过载丢包，调低主动探测速率可同步降低 2–3 倍被动回包压力；并建议检查内核 net.core.rmem_max=33554432。",
            ));
        }
        if timeout_validation_share_pct >= 4.0 && cfg.metadata.max_worker_count > 64 {
            worker_handled = true;
            let next_w = snap_usize(
                (cfg.metadata.max_worker_count as f64 * 0.75) as usize,
                32,
                4_500,
            );
            if next_w < cfg.metadata.max_worker_count {
                recs.push(make_num_rec(
                    "dec_workers_cpu_val",
                    "metadata",
                    RecommendationSeverity::Critical,
                    RecommendationAction::Decrease,
                    "metadata.max_worker_count",
                    "BEP-9 最大并发数",
                    cfg.metadata.max_worker_count as u64,
                    next_w as u64,
                    format!(
                        "metadata validation（SHA1/bencode CPU 校验）超时占比高达 {:.1}%",
                        timeout_validation_share_pct
                    ),
                    "Tokio blocking 线程池已饱和，适当降低并发数可减少上下文切换并提升实际成功产率。",
                ));
            }
        }
    }

    // --- 规则 4: 核心供需平衡 — 上游过载反压（精准治理 95% 降速 & 队列打满！）---
    if overfed {
        score_penalty += if severe_throttled_time_pct >= 30.0 || meta_queue_full_time_pct >= 40.0 {
            24.0
        } else {
            14.0
        };

        // 4A. 判断是否虚高配置了 max_find_node_rate_per_sec（典型案例：300,000 × 5% = 15,000）
        let nominal_inflated = f64::from(eff.find_node_qps) > actual_find_node_qps.max(50.0) * 1.8
            && throttled_time_pct >= 20.0;
        if !find_node_handled
            && (nominal_inflated || bw_saturated || fd_clamped || worker_util_p95_pct >= 88.0)
        {
            // 把名义 QPS 贴合到可持续实际速率的 0.85~0.95 倍，彻底消除锯齿震荡
            let base_sustainable = actual_find_node_qps
                .max(f64::from(effective_find_node_p50))
                .max(40.0);
            let factor = if bw_saturated || meta_queue_full_time_pct >= 50.0 {
                0.85
            } else {
                0.92
            };
            let target_fn = snap_u32((base_sustainable * factor) as u32, 20, 20_000);
            if target_fn < eff.find_node_qps
                && (eff.find_node_qps - target_fn) as f64 / f64::from(eff.find_node_qps.max(1))
                    >= 0.08
            {
                find_node_handled = true;
                recs.push(make_num_rec(
                    "dec_find_node_backpressure",
                    "queue_balance",
                    if severe_throttled_time_pct >= 25.0 {
                        RecommendationSeverity::Critical
                    } else {
                        RecommendationSeverity::Warning
                    },
                    RecommendationAction::Decrease,
                    "crawl.rate_limit.max_find_node_rate_per_sec",
                    "主动探测 find_node 速率 (/s)",
                    u64::from(eff.find_node_qps),
                    u64::from(target_fn),
                    format!(
                        "窗口内 {:.0}% 时间触发降速（严重降速 {:.0}%），名义配置 {}/s，实际有效 P50 仅 {}/s（均速 {:.0}/s）",
                        throttled_time_pct,
                        severe_throttled_time_pct,
                        eff.find_node_qps,
                        effective_find_node_p50,
                        actual_find_node_qps
                    ),
                    "名义速率远超下游消化能力会反复打满队列并触发剧烈降速，将上限收敛至稳态可持续区间可保持队列健康且不掉产率。",
                ));

                // 联动修正 max_new_destinations_per_minute（避免 1.44M/min 与实际 QPS 严重脱节）
                let rec_new_dest = snap_u32(target_fn.saturating_mul(60), 3_000, 1_440_000);
                if eff.max_new_dest_per_min > rec_new_dest.saturating_mul(13) / 10 {
                    recs.push(make_num_rec(
                        "dec_max_new_dest",
                        "udp_crawl",
                        RecommendationSeverity::Info,
                        RecommendationAction::Decrease,
                        "crawl.rate_limit.max_new_destinations_per_minute",
                        "每分钟新目标上限",
                        u64::from(eff.max_new_dest_per_min),
                        u64::from(rec_new_dest),
                        format!(
                            "当前配置 {}/min，按收敛后 find_node 速率 {}/s 匹配约 {}/min",
                            eff.max_new_dest_per_min, target_fn, rec_new_dest
                        ),
                        "使每分钟新目标配额与 find_node 速率对齐，避免瞬时冷启动冲爆路由器或连接跟踪表。",
                    ));
                }
            }
        }

        // 4B. 极端低地板（如 5%）修复
        if eff.metadata_pressure_floor_pct <= 12 && severe_throttled_time_pct >= 15.0 {
            recs.push(make_num_rec(
                "inc_pressure_floor",
                "queue_balance",
                RecommendationSeverity::Warning,
                RecommendationAction::Increase,
                "crawl.rate_limit.metadata_pressure_floor_percent",
                "反压降速底线比例 (%)",
                u64::from(eff.metadata_pressure_floor_pct),
                25,
                format!(
                    "当前降速底线仅 {}%，触发反压时速率被腰斩至极低值形成锯齿震荡",
                    eff.metadata_pressure_floor_pct
                ),
                "搭配合理的 max_find_node_rate_per_sec 将反压底线调回 25%，可让速率平滑回落而非断崖式骤降。",
            ));
        }

        // 4C. 如果还有带宽/FD/CPU 余量且 BEP-9 并发跑满，建议同步调高 max_worker_count 扩大消化口！
        if !worker_handled
            && allow_scale_up
            && !fd_clamped
            && worker_util_p95_pct >= 88.0
            && cfg.metadata.max_worker_count < 4_500
        {
            let step = (0.20 + bw_headroom_ratio * 0.25).clamp(0.15, 0.35);
            let candidate = snap_usize(
                (cfg.metadata.max_worker_count as f64 * (1.0 + step)) as usize,
                32,
                4_500,
            );
            // 确保不超出当前系统 soft_nofile 安全容量
            let fd_safe_max = system_soft_nofile
                .map(|lim| ((lim * 70 / 100).saturating_sub(132)) / 12)
                .unwrap_or(4_500)
                .max(32);
            let next_w = candidate.min(fd_safe_max);
            if next_w > cfg.metadata.max_worker_count {
                worker_handled = true;
                recs.push(make_num_rec(
                    "inc_workers_backpressure",
                    "metadata",
                    RecommendationSeverity::Warning,
                    RecommendationAction::Increase,
                    "metadata.max_worker_count",
                    "BEP-9 最大并发数",
                    cfg.metadata.max_worker_count as u64,
                    next_w as u64,
                    format!(
                        "BEP-9 并发 P95 利用率 {:.0}%、队列 P95 {:.0}%，且当前预估带宽 TX {:.1} / RX {:.1} Mbps 仍有余量",
                        worker_util_p95_pct, meta_queue_fill_p95_pct, est_wire_tx_mbps, est_wire_rx_mbps
                    ),
                    "增加元数据抓取并发槽位可直接提升队列消化速率、降低排队等待时长。",
                ));
            }
        }

        // 4D. 队列满载时，Discovery（不受 metadata_pressure 降速！）若放行过快必须压低
        if cfg.discovery.enabled && (meta_queue_full_time_pct >= 25.0 || throttled_time_pct >= 30.0)
        {
            let disc_total = cfg
                .discovery
                .get_peers_hashes_per_second
                .saturating_add(cfg.discovery.sample_hashes_per_second);
            if disc_total > 16
                && (actual_drain_rate_per_sec <= 1.0
                    || f64::from(disc_total) > actual_drain_rate_per_sec * 0.42)
            {
                let next_gp = snap_u32(
                    (f64::from(cfg.discovery.get_peers_hashes_per_second) * 0.65) as u32,
                    4,
                    512,
                );
                let next_sm = snap_u32(
                    (f64::from(cfg.discovery.sample_hashes_per_second) * 0.65) as u32,
                    4,
                    512,
                );
                if next_gp < cfg.discovery.get_peers_hashes_per_second {
                    recs.push(make_num_rec(
                        "dec_disc_get_peers",
                        "discovery",
                        RecommendationSeverity::Warning,
                        RecommendationAction::Decrease,
                        "discovery.get_peers_hashes_per_second",
                        "被动 get_peers Hash 限速 (/s)",
                        u64::from(cfg.discovery.get_peers_hashes_per_second),
                        u64::from(next_gp),
                        format!(
                            "队列满载期间 discovery 合计上限 {}/s（约占实际消化速率 {:.0}/s 的 {:.0}%），且不参与反压降速",
                            disc_total,
                            actual_drain_rate_per_sec,
                            f64::from(disc_total) / actual_drain_rate_per_sec.max(1.0) * 100.0
                        ),
                        "discovery 通道绕过 metadata_pressure 反压，适当调低纯 Hash 注入速率把队列留给命中率更高的 announce_peer。",
                    ));
                }
                if next_sm < cfg.discovery.sample_hashes_per_second {
                    recs.push(make_num_rec(
                        "dec_disc_sample_hash",
                        "discovery",
                        RecommendationSeverity::Warning,
                        RecommendationAction::Decrease,
                        "discovery.sample_hashes_per_second",
                        "BEP-51 采样 Hash 限速 (/s)",
                        u64::from(cfg.discovery.sample_hashes_per_second),
                        u64::from(next_sm),
                        format!(
                            "队列 P95 达 {:.0}%，BEP-51 采样纯 Hash 需二次 peer_lookup 才可抓取，当前上限 {}/s",
                            meta_queue_fill_p95_pct, cfg.discovery.sample_hashes_per_second
                        ),
                        "削减 BEP-51 采样入队速率可同时缓解 metadata_queue 堆积与 peer_lookup 抢占。",
                    ));
                }
            }
        }

        // 4E. 队列过大导致 Peer 陈旧（排队等待 P95 >= 15s）-> 适当收紧队列容量，保持新鲜度！
        if let Some(qw_p95) = queue_wait_p95
            && qw_p95 >= 15_000
            && cfg.metadata.max_queue_size > 2_048
        {
            let fresh_cap = if actual_drain_rate_per_sec >= 20.0 {
                snap_usize((actual_drain_rate_per_sec * 10.0) as usize, 1_024, 24_576)
            } else {
                snap_usize(
                    (cfg.metadata.max_queue_size as f64 * 0.6) as usize,
                    1_024,
                    24_576,
                )
            };
            if fresh_cap < cfg.metadata.max_queue_size {
                recs.push(make_num_rec(
                    "dec_meta_queue_stale",
                    "queue_balance",
                    RecommendationSeverity::Warning,
                    RecommendationAction::Decrease,
                    "metadata.max_queue_size",
                    "BEP-9 任务队列容量",
                    cfg.metadata.max_queue_size as u64,
                    fresh_cap as u64,
                    format!(
                        "窗口内排队等待 P95 达 {:.1}s（队列陈旧丢弃 {:.1}% / 驱逐 {:.1}%），当前容量 {}",
                        qw_p95 as f64 / 1000.0,
                        meta_queue_stale_pct,
                        meta_queue_evict_pct,
                        cfg.metadata.max_queue_size
                    ),
                    "过大的满载队列会让 Hash 在队中等待过久导致 Peer 下线，将容量收紧至约 10 秒消化量可显著提升抓取成功率。",
                ));
            }
        }

        // 4F. 队列反压下过高的失败重试次数 -> 缩减至 2 次
        if cfg.metadata.retry_max_attempts >= 4
            && (meta_queue_fill_p95_pct >= 75.0 || throttled_time_pct >= 20.0)
        {
            recs.push(make_num_rec(
                "dec_retry_attempts",
                "metadata",
                RecommendationSeverity::Opportunity,
                RecommendationAction::Decrease,
                "metadata.retry_max_attempts",
                "失败最大重试次数",
                cfg.metadata.retry_max_attempts as u64,
                2,
                format!(
                    "当前失败重试 {} 次，队列 P95 {:.0}%、单次抓取成功率 {:.1}%",
                    cfg.metadata.retry_max_attempts, meta_queue_fill_p95_pct, job_success_rate_pct
                ),
                "高负载下反复重试死种子会挤占队列和并发槽位，降至 2 次可把算力让给新鲜种子。",
            ));
        }
    }

    // --- 规则 5: 下游饥饿（有余量可大胆提速！）---
    if underfed && allow_scale_up {
        score_penalty += 8.0;
        // 5A. find_node 已跑满配置上限但下游仍吃不饱 -> 调高 find_node
        if !find_node_handled
            && eff.find_node_qps < 20_000
            && actual_find_node_qps >= f64::from(eff.find_node_qps) * 0.78
        {
            let step = (0.25 + bw_headroom_ratio * 0.35).clamp(0.20, 0.50);
            let next_fn = snap_u32(
                (f64::from(eff.find_node_qps) * (1.0 + step)) as u32,
                40,
                20_000,
            );
            if next_fn > eff.find_node_qps {
                recs.push(make_num_rec(
                    "inc_find_node_underfed",
                    "udp_crawl",
                    RecommendationSeverity::Opportunity,
                    RecommendationAction::Increase,
                    "crawl.rate_limit.max_find_node_rate_per_sec",
                    "主动探测 find_node 速率 (/s)",
                    u64::from(eff.find_node_qps),
                    u64::from(next_fn),
                    format!(
                        "BEP-9 并发 P95 利用率仅 {:.0}%、队列 P95 {:.0}%，而 find_node 已接近配置上限 ({:.0}/{}/s)",
                        worker_util_p95_pct, meta_queue_fill_p95_pct, actual_find_node_qps, eff.find_node_qps
                    ),
                    "下游抓取能力明显富余，提高主动探路速率可扩大 DHT 曝光并提升 announce_peer 产量。",
                ));
            }
        }

        // 5B. Discovery 限流丢弃高 -> 调高 discovery 注入速率
        if cfg.discovery.enabled
            && discovery_get_peers_limited_pct >= 25.0
            && cfg.discovery.get_peers_hashes_per_second < 512
        {
            let next_gp = snap_u32(
                (f64::from(cfg.discovery.get_peers_hashes_per_second) * 1.5) as u32,
                8,
                512,
            );
            if next_gp > cfg.discovery.get_peers_hashes_per_second {
                recs.push(make_num_rec(
                    "inc_disc_get_peers",
                    "discovery",
                    RecommendationSeverity::Opportunity,
                    RecommendationAction::Increase,
                    "discovery.get_peers_hashes_per_second",
                    "被动 get_peers Hash 限速 (/s)",
                    u64::from(cfg.discovery.get_peers_hashes_per_second),
                    u64::from(next_gp),
                    format!(
                        "被动 get_peers 观测被限流拦截 {:.1}%，同时 BEP-9 并发仍有大量空闲（利用率 P95 {:.0}%）",
                        discovery_get_peers_limited_pct, worker_util_p95_pct
                    ),
                    "放宽被动 get_peers 入队限速几乎零额外 UDP 成本即可为闲置抓取并发提供新鲜 Hash。",
                ));
            }
        }
        if cfg.discovery.enabled
            && discovery_sample_hash_limited_pct >= 25.0
            && cfg.discovery.sample_hashes_per_second < 512
        {
            let next_sm = snap_u32(
                (f64::from(cfg.discovery.sample_hashes_per_second) * 1.5) as u32,
                8,
                512,
            );
            if next_sm > cfg.discovery.sample_hashes_per_second {
                recs.push(make_num_rec(
                    "inc_disc_sample_hash",
                    "discovery",
                    RecommendationSeverity::Opportunity,
                    RecommendationAction::Increase,
                    "discovery.sample_hashes_per_second",
                    "BEP-51 采样 Hash 限速 (/s)",
                    u64::from(cfg.discovery.sample_hashes_per_second),
                    u64::from(next_sm),
                    format!(
                        "BEP-51 采样返回的新 Hash 有 {:.1}% 被限速丢弃，下游并发空闲（P95 {:.0}%）",
                        discovery_sample_hash_limited_pct, worker_util_p95_pct
                    ),
                    "提升 sample_hashes_per_second 可充分利用已回包的 BEP-51 采样结果，提升整体产出。",
                ));
            }
        }

        // 5C. 并发严重闲置（< 35%）且节能目标或配置极高 -> 建议适当调低省内存
        if !worker_handled
            && worker_util_p95_pct < 35.0
            && cfg.metadata.max_worker_count >= 512
            && (constraints.goal == TuningGoal::LowResource
                || actual_find_node_qps < f64::from(eff.find_node_qps) * 0.6)
        {
            let next_w = snap_usize(
                ((cfg.metadata.max_worker_count as f64 * (worker_util_p95_pct / 100.0) * 1.5)
                    as usize)
                    .max(128),
                32,
                4_500,
            );
            if next_w < cfg.metadata.max_worker_count {
                worker_handled = true;
                recs.push(make_num_rec(
                    "dec_workers_idle",
                    "metadata",
                    RecommendationSeverity::Info,
                    RecommendationAction::Decrease,
                    "metadata.max_worker_count",
                    "BEP-9 最大并发数",
                    cfg.metadata.max_worker_count as u64,
                    next_w as u64,
                    format!(
                        "窗口内 BEP-9 并发 P95 利用率仅 {:.1}%，当前配置 {} 远超实际需求",
                        worker_util_p95_pct, cfg.metadata.max_worker_count
                    ),
                    "适当收敛闲置并发槽位可减少 FD 预留要求与内存占用。",
                ));
            }
        }
    }

    // --- 规则 6: 带宽超标护栏（当用户设置了上下行带宽上限）---
    if rx_saturated && !worker_handled && cfg.metadata.max_worker_count > 32 {
        score_penalty += 15.0;
        let rx_cap = constraints.down_mbps.unwrap_or(est_wire_rx_mbps);
        let ratio =
            ((rx_cap * (bw_ceiling_pct / 100.0)) / est_wire_rx_mbps.max(0.1)).clamp(0.65, 0.90);
        let next_w = snap_usize(
            (cfg.metadata.max_worker_count as f64 * ratio) as usize,
            16,
            4_500,
        );
        if next_w < cfg.metadata.max_worker_count {
            recs.push(make_num_rec(
                "dec_workers_rx_bw",
                "metadata",
                RecommendationSeverity::Warning,
                RecommendationAction::Decrease,
                "metadata.max_worker_count",
                "BEP-9 最大并发数",
                cfg.metadata.max_worker_count as u64,
                next_w as u64,
                format!(
                    "预估下行带宽 {:.1} Mbps 已达设定上限 {:.0} Mbps 的 {:.0}%（BEP-9 占主力）",
                    est_wire_rx_mbps,
                    rx_cap,
                    bw_down_util_pct.unwrap_or(100.0)
                ),
                "下行打满会导致 TCP 重传激增与超时率飙升，小幅下调并发可在更低丢包下获得更高有效产率。",
            ));
        }
    }

    if let Some(up_cap) = constraints.up_mbps
        && up_cap > 0.0
    {
        let up_cap_bytes = (up_cap * 1_000_000.0 / 8.0) as u64;
        // 若 max_response_bytes_per_sec 比整机上行物理带宽还高，说明完全没有护栏！
        if eff.max_response_bytes > up_cap_bytes {
            let safe_resp_bytes = snap_u64(
                (up_cap_bytes as f64 * 0.65) as u64,
                256 * 1024,
                96 * 1024 * 1024,
            );
            recs.push(make_num_rec(
                "dec_resp_bytes_unguarded",
                "udp_crawl",
                RecommendationSeverity::Warning,
                RecommendationAction::Decrease,
                "crawl.rate_limit.max_response_bytes_per_sec",
                "UDP 回包字节上限 (B/s)",
                eff.max_response_bytes,
                safe_resp_bytes,
                format!(
                    "当前回包字节上限 {:.1} MiB/s 已超出服务器物理上行 {:.0} Mbps ({:.1} MiB/s)",
                    eff.max_response_bytes as f64 / (1024.0 * 1024.0),
                    up_cap,
                    up_cap_bytes as f64 / (1024.0 * 1024.0)
                ),
                "在遇到 DHT 突发反射查询时缺少上行硬护栏会打断 SSH 与 TCP 握手，建议收敛至上行带宽 65% 以内。",
            ));
        }
    }

    // --- 规则 7: 精细超时校准（基于真实增量分阶段超时 + 耗时直方图！）---
    if total_stage_to >= 40 {
        // 7A. 传输分片阶段被截断过多 -> 延长 timeout_secs
        if timeout_piece_recv_share_pct >= 38.0
            && cfg.metadata.timeout_secs < 8
            && fetch_dur_p95.is_none_or(|ms| ms >= cfg.metadata.timeout_secs * 850)
        {
            let next_to = (cfg.metadata.timeout_secs + 2).min(8);
            recs.push(make_num_rec(
                "inc_meta_timeout_piece",
                "metadata",
                RecommendationSeverity::Warning,
                RecommendationAction::Increase,
                "metadata.timeout_secs",
                "单 Peer 抓取总超时 (s)",
                cfg.metadata.timeout_secs,
                next_to,
                format!(
                    "超时的请求中有 {:.1}% 发生在 piece_receive 分片下载阶段（抓取耗时 P95 {}）",
                    timeout_piece_recv_share_pct,
                    fmt_opt_ms(fetch_dur_p95)
                ),
                "大量已握手成功并正在下发元数据的 Peer 因总超时过短被中途掐断，适当放宽总超时可直接挽回有效种子。",
            ));
        } else if cfg.metadata.timeout_secs >= 5
            && timeout_piece_recv_share_pct <= 22.0
            && (overfed || fetch_dur_p95.is_some_and(|ms| ms < cfg.metadata.timeout_secs * 720))
        {
            // 7B. 总超时偏长（例如 6s），但大部分超时死在建连/握手阶段 -> 缩短至 4s 加快槽位周转！
            let next_to = cfg.metadata.timeout_secs.saturating_sub(2).max(4);
            if next_to < cfg.metadata.timeout_secs {
                recs.push(make_num_rec(
                    "dec_meta_timeout_turnover",
                    "metadata",
                    RecommendationSeverity::Opportunity,
                    RecommendationAction::Decrease,
                    "metadata.timeout_secs",
                    "单 Peer 抓取总超时 (s)",
                    cfg.metadata.timeout_secs,
                    next_to,
                    format!(
                        "当前总超时 {}s，但分片下载阶段超时仅占 {:.1}%（建连超时占 {:.1}%，耗时 P95 {}）",
                        cfg.metadata.timeout_secs,
                        timeout_piece_recv_share_pct,
                        timeout_tcp_connect_share_pct,
                        fmt_opt_ms(fetch_dur_p95)
                    ),
                    "缩短总超时至 4s 可加快失联槽位释放，在不增加并发的前提下提升约 25%–35% 队列消化速度。",
                ));
            }
        }

        // 7C. TCP 建连超时占比极高 -> 缩短 connect_timeout_ms 快速淘汰死端口
        if timeout_tcp_connect_share_pct >= 58.0
            && cfg.metadata.connect_timeout_ms >= 2_000
            && overfed
        {
            recs.push(make_num_rec(
                "dec_connect_timeout",
                "metadata",
                RecommendationSeverity::Opportunity,
                RecommendationAction::Decrease,
                "metadata.connect_timeout_ms",
                "TCP 建连超时 (ms)",
                cfg.metadata.connect_timeout_ms,
                1_500,
                format!(
                    "超时明细中 tcp_connect 占比高达 {:.1}%，且当前队列存在积压",
                    timeout_tcp_connect_share_pct
                ),
                "将 TCP 建连超时由 2000ms 收紧至 1500ms 可更快跳过不可达端口，加速多 Peer 竞速轮转。",
            ));
        }
    }

    // --- 规则 8: UDP 回包限流检查 ---
    if (response_rate_limited_pct >= 15.0 || response_fallback_pct >= 12.0)
        && !tx_saturated
        && !udp_cpu_saturated
    {
        score_penalty += 8.0;
        let next_rr = snap_u32(
            (f64::from(eff.max_response_rate) * 1.4) as u32,
            500,
            240_000,
        );
        if next_rr > eff.max_response_rate {
            recs.push(make_num_rec(
                "inc_resp_rate",
                "udp_crawl",
                RecommendationSeverity::Warning,
                RecommendationAction::Increase,
                "crawl.rate_limit.max_response_rate_per_sec",
                "UDP 回包速率上限 (/s)",
                u64::from(eff.max_response_rate),
                u64::from(next_rr),
                format!(
                    "窗口内有 {:.1}% 的入站查询回包被限流丢弃（优先保底通道触发 {:.1}%）",
                    response_rate_limited_pct, response_fallback_pct
                ),
                "大量丢弃入站查询回包会导致远端节点将本机踢出路由表、降低 announce_peer 被动流量。",
            ));
        }
        let next_rb = snap_u64(
            (eff.max_response_bytes as f64 * 1.4) as u64,
            256 * 1024,
            96 * 1024 * 1024,
        );
        let safe_max_rb = constraints
            .up_mbps
            .filter(|&m| m > 0.0)
            .map(|m| ((m * 1_000_000.0 / 8.0) * 0.72) as u64)
            .unwrap_or(96 * 1024 * 1024);
        let capped_rb = next_rb.min(safe_max_rb);
        if capped_rb > eff.max_response_bytes {
            recs.push(make_num_rec(
                "inc_resp_bytes",
                "udp_crawl",
                RecommendationSeverity::Warning,
                RecommendationAction::Increase,
                "crawl.rate_limit.max_response_bytes_per_sec",
                "UDP 回包字节上限 (B/s)",
                eff.max_response_bytes,
                capped_rb,
                format!(
                    "回包限流占比 {:.1}%，当前实测 UDP TX 仅 {:.1} Mbps",
                    response_rate_limited_pct, udp_tx_mbps
                ),
                "同步抬高回包字节配额，避免在包数未超限时被字节桶提前截断。",
            ));
        }
    }

    // --- 规则 9: find_node 在途窗口 & 事务超时检查 ---
    if find_node_in_flight_p95_pct >= 88.0
        && actual_find_node_qps < f64::from(eff.find_node_qps) * 0.85
        && !overfed
        && !udp_cpu_saturated
    {
        let next_if = snap_usize(
            (eff.find_node_max_in_flight as f64 * 1.4) as usize,
            256,
            98_304,
        );
        if next_if > eff.find_node_max_in_flight {
            recs.push(make_num_rec(
                "inc_find_node_inflight",
                "udp_crawl",
                RecommendationSeverity::Warning,
                RecommendationAction::Increase,
                "crawl.rate_limit.max_in_flight",
                "find_node 最大在途事务数",
                eff.find_node_max_in_flight as u64,
                next_if as u64,
                format!(
                    "在途事务饱和度 P95 达 {:.0}%，限制了实际探测速率 ({:.0}/{}/s)",
                    find_node_in_flight_p95_pct, actual_find_node_qps, eff.find_node_qps
                ),
                "按利特尔法则，max_in_flight 需略大于 QPS × request_timeout_secs；放宽窗口可解除发包阻塞。",
            ));
        }
    }

    if d_responses >= 200 && find_node_unmatched_pct >= 12.0 && eff.request_timeout_secs < 4 {
        recs.push(make_num_rec(
            "inc_crawl_req_timeout",
            "udp_crawl",
            RecommendationSeverity::Info,
            RecommendationAction::Increase,
            "crawl.rate_limit.request_timeout_secs",
            "find_node 请求超时 (s)",
            eff.request_timeout_secs,
            eff.request_timeout_secs + 1,
            format!(
                "收到未匹配/迟到响应占比 {:.1}%（{} 条），说明不少跨洲节点在 {}s 后才回包",
                find_node_unmatched_pct, d_unmatched, eff.request_timeout_secs
            ),
            "适当放宽 1 秒请求超时可回收更多远距离高价值路由节点。",
        ));
    }

    // --- 规则 10: PeerLookup 补人通道诊断 ---
    if d_pl_req >= 50 {
        if peer_lookup_rate_limited_pct >= 15.0
            && peer_lookup_yield_per_started >= 0.4
            && !overfed
            && allow_scale_up
            && eff.peer_lookup_rate < 1_024
        {
            let next_pl = snap_u32((f64::from(eff.peer_lookup_rate) * 1.45) as u32, 16, 1_024);
            if next_pl > eff.peer_lookup_rate {
                recs.push(make_num_rec(
                    "inc_peer_lookup_rate",
                    "peer_lookup",
                    RecommendationSeverity::Warning,
                    RecommendationAction::Increase,
                    "peer_lookup.max_lookups_per_second",
                    "主动 PeerLookup 速率 (/s)",
                    u64::from(eff.peer_lookup_rate),
                    u64::from(next_pl),
                    format!(
                        "PeerLookup 请求被限流丢弃 {:.1}%，而每次成功启动平均可捞出 {:.2} 个可用 Peer",
                        peer_lookup_rate_limited_pct, peer_lookup_yield_per_started
                    ),
                    "高回报补人通道被限流卡住，调高速率（并同步放宽 max_active_lookups）可直接提高纯 Hash 抓取成功率。",
                ));
                let next_active = (next_pl as usize * 2).clamp(32, 2_048);
                if next_active > eff.peer_lookup_max_active {
                    recs.push(make_num_rec(
                        "inc_peer_lookup_active",
                        "peer_lookup",
                        RecommendationSeverity::Info,
                        RecommendationAction::Increase,
                        "peer_lookup.max_active_lookups",
                        "PeerLookup 最大并发数",
                        eff.peer_lookup_max_active as u64,
                        next_active as u64,
                        format!(
                            "当前最大活跃并发 {}，与新速率 {}/s 匹配需约 {}",
                            eff.peer_lookup_max_active, next_pl, next_active
                        ),
                        "保持活跃槽位为每秒发起速率的 2 倍左右（对应 2s lookup 超时），防止新速率被并发槽位卡死。",
                    ));
                }
            }
        } else if d_pl_start >= 100
            && peer_lookup_yield_per_started < 0.08
            && peer_lookup_timeout_pct >= 70.0
            && eff.peer_lookup_rate > 16
        {
            let next_pl = snap_u32((f64::from(eff.peer_lookup_rate) * 0.6) as u32, 8, 1_024);
            if next_pl < eff.peer_lookup_rate {
                recs.push(make_num_rec(
                    "dec_peer_lookup_low_yield",
                    "peer_lookup",
                    RecommendationSeverity::Opportunity,
                    RecommendationAction::Decrease,
                    "peer_lookup.max_lookups_per_second",
                    "主动 PeerLookup 速率 (/s)",
                    u64::from(eff.peer_lookup_rate),
                    u64::from(next_pl),
                    format!(
                        "PeerLookup 超时率 {:.1}% 且平均每次仅找到 {:.2} 个 Peer，UDP 投入产出比极低",
                        peer_lookup_timeout_pct, peer_lookup_yield_per_started
                    ),
                    "降低低效的 get_peers 轮询速率可节省 UDP 小包与 PPS 开销。",
                ));
            }
        }
    }

    // --- 规则 11: 入口队列 & 调度器通道 & 浅队列驱逐检查 ---
    if d_ing_drop > 0 || ingress_fill_p95_pct >= 80.0 {
        score_penalty += 8.0;
        let next_hq = (cfg.hash_queue_capacity.saturating_mul(2)).clamp(2_048, 32_768);
        if next_hq > cfg.hash_queue_capacity {
            recs.push(make_num_rec(
                "inc_hash_queue",
                "queue_balance",
                RecommendationSeverity::Warning,
                RecommendationAction::Increase,
                "hash_queue_capacity",
                "Hash 入口队列容量",
                cfg.hash_queue_capacity as u64,
                next_hq as u64,
                format!(
                    "窗口内入口通道丢弃 {} 条 Hash，填充度 P95 达 {:.0}%",
                    d_ing_drop, ingress_fill_p95_pct
                ),
                "加宽 hash_queue_capacity 可平滑突发 announce 洪峰，防止在进入去重调度器前直接丢弃。",
            ));
        }
    }

    if !overfed && meta_queue_evict_pct >= 3.0 && queue_wait_p95.is_some_and(|ms| ms < 4_000) {
        let next_mq = snap_usize(
            (cfg.metadata.max_queue_size as f64 * 1.5) as usize,
            1_024,
            30_000,
        );
        if next_mq > cfg.metadata.max_queue_size {
            recs.push(make_num_rec(
                "inc_meta_queue_burst",
                "queue_balance",
                RecommendationSeverity::Opportunity,
                RecommendationAction::Increase,
                "metadata.max_queue_size",
                "BEP-9 任务队列容量",
                cfg.metadata.max_queue_size as u64,
                next_mq as u64,
                format!(
                    "排队等待 P95 仅 {:.1}s（Peer 很新鲜），但队列突发驱逐率达 {:.1}%",
                    queue_wait_p95.unwrap_or(0) as f64 / 1000.0,
                    meta_queue_evict_pct
                ),
                "当前队列偏浅导致短时波峰驱逐新任务，适度扩容可接住突发流量而不引入陈旧 Peer。",
            ));
        }
    }

    if prio_fill_p95_pct >= 75.0 && eff.prio_chan_cap < 49_152 {
        let next_pc = (eff.prio_chan_cap.saturating_mul(2)).min(49_152);
        recs.push(make_num_rec(
            "inc_prio_chan",
            "udp_crawl",
            RecommendationSeverity::Warning,
            RecommendationAction::Increase,
            "crawl.scheduler.priority_event_channel_capacity",
            "调度器优先事件通道容量",
            eff.prio_chan_cap as u64,
            next_pc as u64,
            format!("priority 事件通道 P95 占用 {:.0}%", prio_fill_p95_pct),
            "拓宽优先通道可防止高 QPS 下的回包路由事件丢弃。",
        ));
    }
    if disc_fill_p95_pct >= 75.0 && eff.disc_chan_cap < 98_304 {
        let next_dc = (eff.disc_chan_cap.saturating_mul(2)).min(98_304);
        recs.push(make_num_rec(
            "inc_disc_chan",
            "udp_crawl",
            RecommendationSeverity::Warning,
            RecommendationAction::Increase,
            "crawl.scheduler.discovery_event_channel_capacity",
            "调度器节点发现通道容量",
            eff.disc_chan_cap as u64,
            next_dc as u64,
            format!("discovery 事件通道 P95 占用 {:.0}%", disc_fill_p95_pct),
            "拓宽发现通道可减少新发现路由节点在入池前的排队丢弃。",
        ));
    }

    // --- 规则 12: 节点池替换与重复探测优化 ---
    if total_node_events >= 500 && node_dropped_rate_limited_pct >= 20.0 && !udp_cpu_saturated {
        let next_rep = snap_u32(
            (f64::from(eff.max_replacements_per_min) * 1.5) as u32,
            10_000,
            4_320_000,
        );
        if next_rep > eff.max_replacements_per_min {
            recs.push(make_num_rec(
                "inc_pool_replacements",
                "udp_crawl",
                RecommendationSeverity::Opportunity,
                RecommendationAction::Increase,
                "crawl.rate_limit.max_replacements_per_minute",
                "节点池每分钟替换上限",
                u64::from(eff.max_replacements_per_min),
                u64::from(next_rep),
                format!(
                    "有 {:.1}% 新发现路由节点因池替换限流被丢弃（池满载 P95 {:.0}%）",
                    node_dropped_rate_limited_pct, node_pool_fill_p95_pct
                ),
                "提高 FIFO 节点池替换速率可加快淘汰僵尸节点、保持活跃路由新鲜度。",
            ));
        }
    }

    if d_queries >= 500
        && revisit_share_pct >= 45.0
        && eff.revisit_interval_secs < 15
        && node_pool_fill_p95_pct >= 60.0
    {
        recs.push(make_num_rec(
            "inc_revisit_interval",
            "udp_crawl",
            RecommendationSeverity::Opportunity,
            RecommendationAction::Increase,
            "crawl.pool.revisit_interval_secs",
            "活跃节点重访冷却间隔 (s)",
            eff.revisit_interval_secs,
            20,
            format!(
                "窗口内重复回访占比达 {:.1}%（当前冷却间隔 {}s），节点池已充裕",
                revisit_share_pct, eff.revisit_interval_secs
            ),
            "设置 20s 重访冷却可避免对同一活跃节点高频重复探测，把发包配额用于探索新区域。",
        ));
    }

    // --- 规则 13: BEP-51 冷却/容量与本地内存缓存调优 ---
    if cfg.discovery.enabled && d_sm_cd_full > 0 && cfg.discovery.sample_cooldown_capacity < 600_000
    {
        let next_cd = (cfg.discovery.sample_cooldown_capacity.saturating_mul(2)).min(600_000);
        recs.push(make_num_rec(
            "inc_sample_cooldown",
            "discovery",
            RecommendationSeverity::Warning,
            RecommendationAction::Increase,
            "discovery.sample_cooldown_capacity",
            "BEP-51 冷却历史表容量",
            cfg.discovery.sample_cooldown_capacity as u64,
            next_cd as u64,
            format!("窗口内触发冷却表满暂停采样 {} 次", d_sm_cd_full),
            "冷却表满会强制暂停 BEP-51 采样，扩容可恢复持续采样。",
        ));
    }
    if cfg.discovery.enabled && d_sm_node_drop >= 50 && cfg.discovery.sample_max_nodes < 131_072 {
        let next_sn = (cfg.discovery.sample_max_nodes.saturating_mul(2)).min(131_072);
        recs.push(make_num_rec(
            "inc_sample_nodes",
            "discovery",
            RecommendationSeverity::Info,
            RecommendationAction::Increase,
            "discovery.sample_max_nodes",
            "BEP-51 候选节点上限",
            cfg.discovery.sample_max_nodes as u64,
            next_sn as u64,
            format!("候选池容量不足丢弃采样节点 {} 次", d_sm_node_drop),
            "适当扩大候选节点池可提高多节点轮询覆盖率。",
        ));
    }

    if local_cache_fill_pct >= 92.0
        && redis_indexed_miss_pct >= 22.0
        && cfg.metadata_cache_capacity < 3_000_000
        && rss_growth_mb_per_hour < 250.0
    {
        let next_cap = snap_usize(
            (cfg.metadata_cache_capacity as f64 * 1.5) as usize,
            100_000,
            3_000_000,
        );
        if next_cap > cfg.metadata_cache_capacity {
            recs.push(make_num_rec(
                "inc_local_cache",
                "memory_cache",
                RecommendationSeverity::Opportunity,
                RecommendationAction::Increase,
                "metadata_cache_capacity",
                "本地去重缓存容量",
                cfg.metadata_cache_capacity as u64,
                next_cap as u64,
                format!(
                    "本地缓存已满 {:.0}%，穿透到 Redis 后命中「已索引」占比达 {:.1}%",
                    local_cache_fill_pct, redis_indexed_miss_pct
                ),
                "扩容本地去重缓存可大幅减少 Redis 查重往返，降低网络延迟与 Redis CPU 消耗。",
            ));
        }
    }

    if peer_fail_cache_fill_pct >= 92.0 && cfg.metadata.peer_failure_cache_capacity < 800_000 {
        let next_pfc = snap_usize(
            (cfg.metadata.peer_failure_cache_capacity as f64 * 1.5) as usize,
            50_000,
            800_000,
        );
        if next_pfc > cfg.metadata.peer_failure_cache_capacity {
            recs.push(make_num_rec(
                "inc_peer_fail_cache",
                "memory_cache",
                RecommendationSeverity::Info,
                RecommendationAction::Increase,
                "metadata.peer_failure_cache_capacity",
                "坏 Peer 失败缓存容量",
                cfg.metadata.peer_failure_cache_capacity as u64,
                next_pfc as u64,
                format!(
                    "失败缓存占用率达 {:.0}%，高并发下旧黑名单被过早淘汰",
                    peer_fail_cache_fill_pct
                ),
                "扩容可避免反复重连近期已超时/拒绝连接的死 Peer。",
            ));
        }
    }

    if d_io_other >= 50 && pct(d_io_other, d_peer_att.max(1)) >= 3.0 {
        score_penalty += 10.0;
        recs.push(TuningRecommendation {
            id: "sys_conntrack_ports".to_string(),
            category: "system".to_string(),
            severity: RecommendationSeverity::Warning,
            action: RecommendationAction::System,
            param_path: "system.sysctl.conntrack_ports".to_string(),
            param_label: "Linux 临时端口段与 Conntrack 表".to_string(),
            current_display: format!("io_other 错误 {d_io_other} 次"),
            recommended_display: "ip_local_port_range=10000 65000 / tcp_tw_reuse=1".to_string(),
            current_value: Some(Value::from(d_io_other)),
            recommended_value: None,
            delta_pct: None,
            evidence: format!(
                "窗口内 TCP 建连出现 io_other 异常 {} 次（占尝试 {:.1}%），常见于临时端口耗尽或 nf_conntrack 满表",
                d_io_other,
                pct(d_io_other, d_peer_att.max(1))
            ),
            reason: "检查 dmesg 是否有 nf_conntrack: table full，并扩大 ip_local_port_range 与 nf_conntrack_max。".to_string(),
            apply_supported: false,
        });
    }

    // 按严重级别与动作排序（Critical > Warning > Opportunity > Info；Decrease 优先于 Increase）
    recs.sort_by_key(|r| {
        let sev = match r.severity {
            RecommendationSeverity::Critical => 0,
            RecommendationSeverity::Warning => 1,
            RecommendationSeverity::Opportunity => 2,
            RecommendationSeverity::Info => 3,
        };
        let act = match r.action {
            RecommendationAction::System => 0,
            RecommendationAction::Decrease => 1,
            RecommendationAction::Tune => 2,
            RecommendationAction::Increase => 3,
        };
        (sev, act)
    });

    let increase_count = recs
        .iter()
        .filter(|r| r.action == RecommendationAction::Increase)
        .count();
    let decrease_count = recs
        .iter()
        .filter(|r| r.action == RecommendationAction::Decrease)
        .count();
    let system_count = recs
        .iter()
        .filter(|r| r.action == RecommendationAction::System)
        .count();

    let health_score = (100.0 - score_penalty).clamp(15.0, 100.0).round() as u8;

    let summary = TuningWindowSummary {
        sample_count: all_samples.len(),
        continuous_samples: seg.len(),
        restart_detected,
        window_secs,
        configured_find_node_qps: eff.find_node_qps,
        actual_find_node_qps: round1(actual_find_node_qps),
        effective_find_node_p50,
        effective_find_node_min,
        throttled_time_pct: round1(throttled_time_pct),
        severe_throttled_time_pct: round1(severe_throttled_time_pct),
        est_wire_tx_mbps: round2(est_wire_tx_mbps),
        est_wire_rx_mbps: round2(est_wire_rx_mbps),
        udp_tx_mbps: round2(udp_tx_mbps),
        udp_rx_mbps: round2(udp_rx_mbps),
        metadata_rx_mbps: round2(metadata_rx_mbps),
        udp_total_pps: round1(udp_total_pps),
        meta_queue_fill_avg_pct: round1(meta_queue_fill_avg_pct),
        meta_queue_fill_p95_pct: round1(meta_queue_fill_p95_pct),
        meta_queue_full_time_pct: round1(meta_queue_full_time_pct),
        meta_queue_wait_p50_ms: queue_wait_p50,
        meta_queue_wait_p95_ms: queue_wait_p95,
        meta_queue_evict_pct: round1(meta_queue_evict_pct),
        meta_queue_stale_pct: round1(meta_queue_stale_pct),
        hash_ingress_dropped_total: d_ing_drop,
        configured_workers: cfg.metadata.max_worker_count,
        effective_workers,
        fd_clamped,
        worker_util_avg_pct: round1(worker_util_avg_pct),
        worker_util_p95_pct: round1(worker_util_p95_pct),
        fetch_duration_p50_ms: fetch_dur_p50,
        fetch_duration_p95_ms: fetch_dur_p95,
        torrents_per_min: round1(torrents_per_min),
        job_success_rate_pct: round1(job_success_rate_pct),
        peer_timeout_rate_pct: round1(peer_timeout_rate_pct),
        timeout_tcp_connect_share_pct: round1(timeout_tcp_connect_share_pct),
        timeout_piece_recv_share_pct: round1(timeout_piece_recv_share_pct),
        timeout_validation_share_pct: round1(timeout_validation_share_pct),
        response_rate_limited_pct: round1(response_rate_limited_pct),
        udp_queue_full_total: d_udp_qfull,
        find_node_in_flight_p95_pct: round1(find_node_in_flight_p95_pct),
        find_node_unmatched_pct: round1(find_node_unmatched_pct),
        node_pool_fill_p95_pct: round1(node_pool_fill_p95_pct),
        node_dropped_rate_limited_pct: round1(node_dropped_rate_limited_pct),
        peer_lookup_rate_limited_pct: round1(peer_lookup_rate_limited_pct),
        peer_lookup_yield_per_started: round2(peer_lookup_yield_per_started),
        discovery_get_peers_limited_pct: round1(discovery_get_peers_limited_pct),
        discovery_sample_hash_limited_pct: round1(discovery_sample_hash_limited_pct),
        local_cache_fill_pct: round1(local_cache_fill_pct),
        redis_pool_exhausted_time_pct: round1(redis_pool_exhausted_time_pct),
        sink_backpressure_time_pct: round1(sink_backpressure_time_pct),
        rss_p95_mb: round1(rss_p95_mb),
        rss_growth_mb_per_hour: round1(rss_growth_mb_per_hour),
    };

    TuningReport {
        worker_id: worker_id.to_string(),
        source: source.to_string(),
        generated_at: now,
        config_version,
        confidence,
        confidence_note,
        health_score,
        regime,
        regime_title,
        regime_summary,
        constraints,
        summary,
        recommendations: recs,
        increase_count,
        decrease_count,
        system_count,
    }
}

/// 将用户勾选的建议 ID 精准应用到 `CrawlerWorkerConfig`，并联动更新强相关附属字段（如 burst）。
pub fn apply_selected_recommendations(
    base: &CrawlerWorkerConfig,
    report: &TuningReport,
    selected_ids: &[String],
) -> Result<(CrawlerWorkerConfig, Vec<String>), String> {
    let selected: HashSet<&str> = selected_ids.iter().map(String::as_str).collect();
    let mut cfg = base.clone();
    let mut applied_labels: Vec<String> = Vec::new();

    for rec in &report.recommendations {
        if !rec.apply_supported || !selected.contains(rec.id.as_str()) {
            continue;
        }
        let Some(val) = rec.recommended_value.as_ref().and_then(Value::as_u64) else {
            continue;
        };
        let applied = match rec.param_path.as_str() {
            "push_concurrency" => {
                cfg.push_concurrency = val as usize;
                true
            }
            "hash_queue_capacity" => {
                cfg.hash_queue_capacity = val as usize;
                true
            }
            "metadata_cache_capacity" => {
                cfg.metadata_cache_capacity = val as usize;
                true
            }
            "metadata.max_worker_count" => {
                cfg.metadata.max_worker_count = val as usize;
                true
            }
            "metadata.max_queue_size" => {
                cfg.metadata.max_queue_size = val as usize;
                true
            }
            "metadata.timeout_secs" => {
                cfg.metadata.timeout_secs = val;
                true
            }
            "metadata.connect_timeout_ms" => {
                cfg.metadata.connect_timeout_ms = val;
                true
            }
            "metadata.retry_max_attempts" => {
                cfg.metadata.retry_max_attempts = val as usize;
                true
            }
            "metadata.peer_failure_cache_capacity" => {
                cfg.metadata.peer_failure_cache_capacity = val as usize;
                true
            }
            "discovery.get_peers_hashes_per_second" => {
                let v = val.min(u64::from(u32::MAX)) as u32;
                cfg.discovery.get_peers_hashes_per_second = v;
                cfg.discovery.get_peers_burst = v.saturating_mul(2).max(8);
                true
            }
            "discovery.sample_hashes_per_second" => {
                let v = val.min(u64::from(u32::MAX)) as u32;
                cfg.discovery.sample_hashes_per_second = v;
                cfg.discovery.sample_hash_burst = v.saturating_mul(2).max(16);
                true
            }
            "discovery.sample_cooldown_capacity" => {
                cfg.discovery.sample_cooldown_capacity = val as usize;
                true
            }
            "discovery.sample_max_nodes" => {
                cfg.discovery.sample_max_nodes = val as usize;
                true
            }
            "peer_lookup.max_lookups_per_second" => {
                let v = val.min(u64::from(u32::MAX)) as u32;
                cfg.peer_lookup.max_lookups_per_second = Some(v);
                cfg.peer_lookup.burst = Some(v);
                true
            }
            "peer_lookup.max_active_lookups" => {
                cfg.peer_lookup.max_active_lookups = Some(val as usize);
                true
            }
            "crawl.rate_limit.max_find_node_rate_per_sec" => {
                let v = val.min(u64::from(u32::MAX)) as u32;
                cfg.crawl.rate_limit.max_find_node_rate_per_sec = Some(v);
                let burst = snap_u32((f64::from(v) * 0.2) as u32, 16, 3_840);
                cfg.crawl.rate_limit.burst = Some(burst);
                true
            }
            "crawl.rate_limit.max_new_destinations_per_minute" => {
                cfg.crawl.rate_limit.max_new_destinations_per_minute =
                    Some(val.min(u64::from(u32::MAX)) as u32);
                true
            }
            "crawl.rate_limit.metadata_pressure_floor_percent" => {
                cfg.crawl.rate_limit.metadata_pressure_floor_percent = Some(val.min(100) as u8);
                true
            }
            "crawl.rate_limit.max_response_rate_per_sec" => {
                cfg.crawl.rate_limit.max_response_rate_per_sec =
                    Some(val.min(u64::from(u32::MAX)) as u32);
                true
            }
            "crawl.rate_limit.max_response_bytes_per_sec" => {
                cfg.crawl.rate_limit.max_response_bytes_per_sec = Some(val);
                true
            }
            "crawl.rate_limit.max_in_flight" => {
                cfg.crawl.rate_limit.max_in_flight = Some(val as usize);
                true
            }
            "crawl.rate_limit.request_timeout_secs" => {
                cfg.crawl.rate_limit.request_timeout_secs = Some(val);
                true
            }
            "crawl.rate_limit.max_replacements_per_minute" => {
                cfg.crawl.rate_limit.max_replacements_per_minute =
                    Some(val.min(u64::from(u32::MAX)) as u32);
                true
            }
            "crawl.pool.revisit_interval_secs" => {
                cfg.crawl.pool.revisit_interval_secs = Some(val);
                true
            }
            "crawl.scheduler.priority_event_channel_capacity" => {
                cfg.crawl.scheduler.priority_event_channel_capacity = Some(val as usize);
                true
            }
            "crawl.scheduler.discovery_event_channel_capacity" => {
                cfg.crawl.scheduler.discovery_event_channel_capacity = Some(val as usize);
                true
            }
            _ => false,
        };
        if applied {
            applied_labels.push(format!("{} -> {}", rec.param_path, rec.recommended_display));
        }
    }

    if applied_labels.is_empty() {
        return Err("未选中任何可自动应用的参数建议".to_string());
    }
    cfg.validate()
        .map_err(|e| format!("应用建议后参数校验未通过: {e}"))?;
    Ok((cfg, applied_labels))
}

#[allow(clippy::too_many_arguments)]
fn make_num_rec(
    id: &str,
    category: &str,
    severity: RecommendationSeverity,
    action: RecommendationAction,
    param_path: &str,
    param_label: &str,
    cur: u64,
    rec: u64,
    evidence: String,
    reason: &str,
) -> TuningRecommendation {
    let is_bytes = param_path.ends_with("bytes_per_sec");
    let fmt_v = |v: u64| {
        if is_bytes {
            format!("{:.1} MiB/s", v as f64 / (1024.0 * 1024.0))
        } else {
            v.to_string()
        }
    };
    TuningRecommendation {
        id: id.to_string(),
        category: category.to_string(),
        severity,
        action,
        param_path: param_path.to_string(),
        param_label: param_label.to_string(),
        current_display: fmt_v(cur),
        recommended_display: fmt_v(rec),
        current_value: Some(Value::from(cur)),
        recommended_value: Some(Value::from(rec)),
        delta_pct: Some(round1(delta_pct(cur as f64, rec as f64))),
        evidence,
        reason: reason.to_string(),
        apply_supported: true,
    }
}

/// 提取最后一个连续无重启/无配置切换的采样段，防止计数器回零干扰增量统计。
fn latest_continuous_segment(samples: &[CrawlerMetricSample]) -> (&[CrawlerMetricSample], bool) {
    if samples.len() <= 1 {
        return (samples, false);
    }
    let mut start_idx = 0usize;
    let mut restarted = false;
    for i in 1..samples.len() {
        if samples[i].uptime_secs < samples[i - 1].uptime_secs
            || samples[i].config_version != samples[i - 1].config_version
        {
            start_idx = i;
            restarted = true;
        }
    }
    (&samples[start_idx..], restarted)
}

fn window_duration_secs(samples: &[CrawlerMetricSample]) -> u64 {
    let (seg, _) = latest_continuous_segment(samples);
    match (seg.first(), seg.last()) {
        (Some(f), Some(l)) => l.captured_at.saturating_sub(f.captured_at).max(0) as u64,
        _ => 0,
    }
}

fn confidence_for_window(sample_count: usize, window_secs: u64) -> ConfidenceLevel {
    if sample_count < 2 {
        ConfidenceLevel::Insufficient
    } else if window_secs >= 2_400 && sample_count >= 120 {
        ConfidenceLevel::High
    } else if window_secs >= 300 && sample_count >= 30 {
        ConfidenceLevel::Medium
    } else {
        ConfidenceLevel::Low
    }
}

fn j_u64(v: &Value, path: &[&str]) -> u64 {
    let mut cur = v;
    for key in path {
        let Some(next) = cur.get(*key) else {
            return 0;
        };
        cur = next;
    }
    cur.as_u64().unwrap_or(0)
}

fn j_usize(v: &Value, path: &[&str]) -> usize {
    usize::try_from(j_u64(v, path)).unwrap_or(0)
}

fn pct(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}

fn delta_pct(cur: f64, next: f64) -> f64 {
    if cur.abs() < f64::EPSILON {
        0.0
    } else {
        ((next - cur) / cur) * 100.0
    }
}

fn mean_f64(vals: &[f64]) -> f64 {
    if vals.is_empty() {
        0.0
    } else {
        vals.iter().sum::<f64>() / vals.len() as f64
    }
}

fn percentile_f64(vals: &[f64], q: f64) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    let mut sorted = vals.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let idx = (((sorted.len() - 1) as f64) * q.clamp(0.0, 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn percentile_u32(vals: &[u32], q: f64) -> u32 {
    if vals.is_empty() {
        return 0;
    }
    let mut sorted = vals.to_vec();
    sorted.sort_unstable();
    let idx = (((sorted.len() - 1) as f64) * q.clamp(0.0, 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn snap_u32(v: u32, min_v: u32, max_v: u32) -> u32 {
    let c = v.clamp(min_v, max_v);
    let step = if c < 100 {
        10
    } else if c < 1_000 {
        20
    } else if c < 5_000 {
        100
    } else {
        500
    };
    ((c + step / 2) / step * step).clamp(min_v, max_v)
}

fn snap_usize(v: usize, min_v: usize, max_v: usize) -> usize {
    let c = v.clamp(min_v, max_v);
    let step = if c < 64 {
        8
    } else if c < 256 {
        16
    } else if c < 1_024 {
        32
    } else {
        64
    };
    ((c + step / 2) / step * step).clamp(min_v, max_v)
}

fn snap_u64(v: u64, step: u64, max_v: u64) -> u64 {
    let s = step.max(1);
    ((v + s / 2) / s * s).clamp(s, max_v)
}

fn next_nofile_tier(required: usize) -> usize {
    for t in [16_384, 32_768, 65_536, 131_072, 262_144, 524_288, 1_048_576] {
        if t >= required {
            return t;
        }
    }
    1_048_576
}

fn fmt_opt_ms(ms: Option<u64>) -> String {
    match ms {
        Some(v) if v >= 1_000 => format!("{:.1}s", v as f64 / 1000.0),
        Some(v) => format!("{v}ms"),
        None => "-".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist_with_bucket(bounds: Vec<u64>, bucket_idx: usize, count: u64) -> CompactHistogram {
        let mut counts = vec![0; bounds.len()];
        if bucket_idx < counts.len() {
            counts[bucket_idx] = count;
        }
        CompactHistogram {
            bounds,
            counts,
            overflow: 0,
            count,
            sum: count * 1_000,
        }
    }

    #[test]
    fn diagnoses_500m_95_percent_throttle_and_full_queue_scenario() {
        let mut cfg = CrawlerWorkerConfig {
            crawl_profile: "P17".to_string(),
            ..Default::default()
        };
        cfg.metadata.timeout_secs = 6;
        cfg.metadata.connect_timeout_ms = 2_000;
        cfg.metadata.max_queue_size = 30_000;
        cfg.metadata.max_worker_count = 4_440;
        cfg.metadata.retry_max_attempts = 5;
        cfg.discovery.get_peers_hashes_per_second = 384;
        cfg.discovery.sample_hashes_per_second = 384;
        cfg.crawl.rate_limit.max_find_node_rate_per_sec = Some(300_000);
        cfg.crawl.rate_limit.max_new_destinations_per_minute = Some(1_440_000);
        cfg.crawl.rate_limit.max_response_bytes_per_sec = Some(201_326_592);
        cfg.crawl.rate_limit.metadata_pressure_floor_percent = Some(5);

        // 模拟 1 小时（720 点 @ 5s）队列持续 99% 满载、95% 降速至 15k/s、排队 25s
        let mut samples = Vec::new();
        for i in 0..720_u64 {
            let t = 1_700_000_000 + (i * 5) as i64;
            samples.push(CrawlerMetricSample {
                captured_at: t,
                uptime_secs: 600 + i * 5,
                config_version: 3,
                effective_metadata_workers: 4_440,
                local_cache_entries: 900_000,
                redis_pool_size: 100,
                redis_pool_available: 85,
                rss_bytes: 1_200_000_000,
                metadata_queue_depth: 29_700,
                metadata_queue_max: 30_000,
                metadata_in_flight: 4_420,
                metadata_queue_inserted: i * 6_000,
                metadata_queue_evicted: i * 1_200,
                metadata_queue_stale: i * 400,
                metadata_fetch_accepted: i * 1_500,
                metadata_fetch_failed: i * 4_000,
                torrents_downloaded: i * 1_500,
                metadata_peer_attempts: i * 25_000,
                metadata_failure_timeout: i * 12_000,
                timeout_tcp_connect: i * 8_000,
                timeout_piece_recv: i * 1_500,
                find_node_effective_rate_per_sec: 15_000,
                queries_new: i * 75_000, // 15,000/s
                udp_tx_bytes: i * 55_000_000,
                udp_rx_bytes: i * 45_000_000,
                udp_tx_packets: i * 250_000,
                udp_rx_packets: i * 260_000,
                metadata_bytes_downloaded: i * 210_000_000, // ~336 Mbps payload -> ~445 Mbps wire
                queue_wait_ms: hist_with_bucket(
                    vec![1_000, 5_000, 15_000, 25_000, 60_000],
                    3,
                    i * 5_000,
                ),
                fetch_duration_ms: hist_with_bucket(
                    vec![500, 1_500, 3_000, 4_000, 6_000],
                    2,
                    i * 5_000,
                ),
                ..Default::default()
            });
        }

        let report = analyze_crawler_samples(
            "crawler-1",
            "session",
            3,
            &cfg,
            &samples,
            TuningConstraints {
                up_mbps: Some(500.0),
                down_mbps: Some(500.0),
                goal: TuningGoal::Balanced,
            },
            Some(262_144),
        );

        assert_eq!(report.confidence, ConfidenceLevel::High);
        assert!(matches!(
            report.regime,
            OperatingRegime::BandwidthCeiling | OperatingRegime::OverfedBackpressure
        ));
        assert!(report.summary.throttled_time_pct >= 99.0);
        assert!(report.decrease_count >= 4);

        let paths: HashSet<&str> = report
            .recommendations
            .iter()
            .map(|r| r.param_path.as_str())
            .collect();
        assert!(paths.contains("crawl.rate_limit.max_find_node_rate_per_sec"));
        assert!(paths.contains("crawl.rate_limit.metadata_pressure_floor_percent"));
        assert!(paths.contains("crawl.rate_limit.max_response_bytes_per_sec"));
        assert!(paths.contains("metadata.max_queue_size"));
        assert!(paths.contains("metadata.timeout_secs"));
        assert!(paths.contains("discovery.get_peers_hashes_per_second"));

        // 验证一键应用全部可应用建议可通过严格校验
        let apply_ids: Vec<String> = report
            .recommendations
            .iter()
            .filter(|r| r.apply_supported)
            .map(|r| r.id.clone())
            .collect();
        let (next_cfg, applied) =
            apply_selected_recommendations(&cfg, &report, &apply_ids).unwrap();
        assert!(!applied.is_empty());
        assert!(
            next_cfg
                .crawl
                .rate_limit
                .max_find_node_rate_per_sec
                .unwrap()
                < 20_000
        );
        assert_eq!(
            next_cfg.crawl.rate_limit.metadata_pressure_floor_percent,
            Some(25)
        );
        assert_eq!(next_cfg.metadata.timeout_secs, 4);
        assert!(next_cfg.metadata.max_queue_size < 30_000);
    }

    #[test]
    fn diagnoses_underfed_starvation_and_recommends_increases() {
        let mut cfg = CrawlerWorkerConfig {
            crawl_profile: "P1".to_string(),
            ..Default::default()
        };
        cfg.metadata.max_worker_count = 512;
        cfg.crawl.rate_limit.max_find_node_rate_per_sec = Some(500);

        let mut samples = Vec::new();
        for i in 0..120_u64 {
            samples.push(CrawlerMetricSample {
                captured_at: 1_700_000_000 + (i * 5) as i64,
                uptime_secs: 100 + i * 5,
                config_version: 1,
                effective_metadata_workers: 512,
                redis_pool_size: 100,
                redis_pool_available: 98,
                metadata_queue_depth: 200,
                metadata_queue_max: 10_000,
                metadata_in_flight: 140, // ~27% util
                find_node_effective_rate_per_sec: 500,
                queries_new: i * 2_450, // ~490/s (98% of cap)
                disc_get_peers_observed: i * 500,
                disc_get_peers_rate_limited: i * 350,
                disc_get_peers_enqueued: i * 40,
                peer_lookup_requested: i * 400,
                peer_lookup_started: i * 160,
                peer_lookup_rate_limited: i * 240,
                peer_lookup_peers_found: i * 320,
                ..Default::default()
            });
        }

        let report = analyze_crawler_samples(
            "crawler-1",
            "ring",
            1,
            &cfg,
            &samples,
            TuningConstraints {
                up_mbps: Some(100.0),
                down_mbps: Some(100.0),
                goal: TuningGoal::Balanced,
            },
            Some(65_536),
        );

        assert_eq!(report.regime, OperatingRegime::UnderfedStarvation);
        assert!(report.increase_count >= 3);
        let inc_paths: HashSet<&str> = report
            .recommendations
            .iter()
            .filter(|r| r.action == RecommendationAction::Increase)
            .map(|r| r.param_path.as_str())
            .collect();
        assert!(inc_paths.contains("crawl.rate_limit.max_find_node_rate_per_sec"));
        assert!(inc_paths.contains("discovery.get_peers_hashes_per_second"));
        assert!(inc_paths.contains("peer_lookup.max_lookups_per_second"));
    }

    #[test]
    fn nofile_clamped_suppresses_worker_increase() {
        let mut cfg = CrawlerWorkerConfig::default();
        cfg.metadata.max_worker_count = 1_024;
        let samples = vec![
            CrawlerMetricSample {
                captured_at: 1000,
                uptime_secs: 100,
                config_version: 1,
                effective_metadata_workers: 60,
                metadata_in_flight: 60,
                metadata_queue_depth: 9_800,
                metadata_queue_max: 10_000,
                ..Default::default()
            },
            CrawlerMetricSample {
                captured_at: 1300,
                uptime_secs: 400,
                config_version: 1,
                effective_metadata_workers: 60,
                metadata_in_flight: 60,
                metadata_queue_depth: 9_900,
                metadata_queue_max: 10_000,
                ..Default::default()
            },
        ];
        let report = analyze_crawler_samples(
            "crawler-1",
            "ring",
            1,
            &cfg,
            &samples,
            TuningConstraints::default(),
            Some(1_024),
        );
        assert_eq!(report.regime, OperatingRegime::NofileClamped);
        assert!(
            report
                .recommendations
                .iter()
                .any(|r| r.id == "sys_nofile" && r.severity == RecommendationSeverity::Critical)
        );
        // 绝不出现要求再加高 max_worker_count 的冲突建议
        assert!(!report.recommendations.iter().any(|r| {
            r.param_path == "metadata.max_worker_count"
                && r.action == RecommendationAction::Increase
        }));
    }

    #[test]
    fn detects_restart_segment_and_piece_receive_timeout_increase() {
        let mut cfg = CrawlerWorkerConfig::default();
        cfg.metadata.timeout_secs = 3;
        cfg.metadata.max_worker_count = 256;

        let samples = vec![
            // 重启前的旧计数（uptime 较大）
            CrawlerMetricSample {
                captured_at: 1_000,
                uptime_secs: 9_000,
                config_version: 1,
                effective_metadata_workers: 256,
                metadata_peer_attempts: 999_999,
                ..Default::default()
            },
            // 重启后的连续新段
            CrawlerMetricSample {
                captured_at: 1_005,
                uptime_secs: 10,
                config_version: 2,
                effective_metadata_workers: 256,
                metadata_in_flight: 200,
                metadata_queue_depth: 3_000,
                metadata_queue_max: 10_000,
                metadata_peer_attempts: 100,
                metadata_failure_timeout: 20,
                timeout_piece_recv: 12,
                timeout_tcp_connect: 8,
                fetch_duration_ms: hist_with_bucket(vec![500, 1_500, 3_000, 5_000], 2, 100),
                ..Default::default()
            },
            CrawlerMetricSample {
                captured_at: 1_605,
                uptime_secs: 610,
                config_version: 2,
                effective_metadata_workers: 256,
                metadata_in_flight: 210,
                metadata_queue_depth: 3_500,
                metadata_queue_max: 10_000,
                metadata_peer_attempts: 2_100,
                metadata_failure_timeout: 520,
                timeout_piece_recv: 362,
                timeout_tcp_connect: 158,
                fetch_duration_ms: hist_with_bucket(vec![500, 1_500, 3_000, 5_000], 2, 2_100),
                ..Default::default()
            },
        ];

        let report = analyze_crawler_samples(
            "crawler-1",
            "ring",
            2,
            &cfg,
            &samples,
            TuningConstraints::default(),
            Some(65_536),
        );
        assert!(report.summary.restart_detected);
        assert_eq!(report.summary.continuous_samples, 2);
        assert!(report.recommendations.iter().any(|r| {
            r.param_path == "metadata.timeout_secs"
                && r.action == RecommendationAction::Increase
                && r.recommended_value == Some(Value::from(5_u64))
        }));
    }
}
