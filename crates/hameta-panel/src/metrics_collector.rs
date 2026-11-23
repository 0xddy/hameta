use crate::services::ServiceRuntimePool;
use crate::supervisor::{Supervisor, load_geoip_config};
use chrono::{TimeZone, Utc};
use hameta_common::{
    GeoIpConfig, MetricsHourPoint, MetricsMinutePoint, WorkerRole, WorkerRuntimeState,
    current_soft_open_file_limit,
};
use hameta_indexer::TableStatusInfo;
use hameta_indexer::manticore::is_missing_table_error;
use hameta_storage::Database;
use redis::Value as RedisValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio_util::sync::CancellationToken;

const RING_CAPACITY: usize = 600;
const MANTICORE_STATUS_TTL: Duration = Duration::from_secs(60);
const REDIS_PROBE_TTL: Duration = Duration::from_secs(3);
const GEOIP_CONFIG_TTL: Duration = Duration::from_secs(5);
const GLOBE_MAX_COUNTRIES: usize = 64;
/// Crawler 瞬时速率滑动窗口（5s 平滑，消除 1s 抖动且响应迅速）。
const CRAWLER_RATE_WINDOW_MS: i64 = 5_000;
/// Indexer 瞬时速率滑动窗口（15s 覆盖 3 个默认 5s 刷库周期，避免批次间隙跳 0）。
const INDEXER_RATE_WINDOW_MS: i64 = 15_000;
/// 单 Worker 计数历史最大保留时长。
const WORKER_HISTORY_RETENTION_MS: i64 = 25_000;
/// 重复快照容忍保持时长：超过该时长仍无新快照则归零，避免掩盖进程卡死。
const SNAPSHOT_STALE_HOLD_MS: i64 = 6_000;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OverviewHealth {
    pub panel_ok: bool,
    pub redis_ok: bool,
    pub redis_message: Option<String>,
    pub manticore_ok: bool,
    pub manticore_message: Option<String>,
    pub running_crawlers: usize,
    pub total_crawlers: usize,
    pub running_indexers: usize,
    pub total_indexers: usize,
    pub crashed_workers: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OverviewFunnel {
    pub hashes_per_min: f64,
    pub dedup_blocked_per_min: f64,
    pub metadata_succeeded_per_min: f64,
    pub stream_pushed_per_min: f64,
    pub filter_accepted_per_min: f64,
    pub filter_rejected_per_min: f64,
    pub indexed_written_per_min: f64,
    pub hot_pushed_per_min: f64,
    pub hot_updated_per_min: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OverviewKpi {
    pub actual_qps: f64,
    pub indexed_written_per_min: f64,
    pub estimated_daily_docs: f64,
    pub today_indexed_docs: i64,
    pub torrents_total_docs: Option<u64>,
    pub torrents_disk_bytes: Option<u64>,
    pub torrents_status_cached_at: Option<i64>,
    pub stream_new_len: i64,
    pub stream_hot_len: i64,
    pub stream_new_pending: i64,
    pub stream_new_lag: i64,
    pub total_rss_bytes: i64,
    pub udp_rx_bps: f64,
    pub udp_tx_bps: f64,
    #[serde(default)]
    pub udp_rx_pps: f64,
    #[serde(default)]
    pub udp_tx_pps: f64,
    pub metadata_rx_bps: f64,
    pub system_soft_nofile: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobeChannelRates {
    pub get_peers_pps: f64,
    pub announce_peer_pps: f64,
    pub response_pps: f64,
    pub find_node_ping_pps: f64,
    pub error_pps: f64,
    pub other_pps: f64,
    pub parse_error_pps: f64,
    pub invalid_udp_pps: f64,
    pub queue_full_pps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobeCountryTraffic {
    pub iso_code: String,
    pub is_home: bool,
    pub pps: f64,
    pub bps: f64,
    pub share_pct: f64,
    pub get_peers_pps: f64,
    pub announce_peer_pps: f64,
    pub response_pps: f64,
    pub find_node_ping_pps: f64,
    pub other_pps: f64,
    pub cumulative_packets: u64,
    pub cumulative_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobeGeoIpStatus {
    pub enabled: bool,
    pub mmdb_loaded: bool,
    #[serde(default)]
    pub mmdb_file_ready: bool,
    pub mmdb_path: String,
    pub mmdb_size_bytes: u64,
    pub mmdb_build_epoch: u64,
    pub database_type: String,
    pub error: Option<String>,
    pub home_country: String,
    pub sample_rate_divisor: u16,
    pub adaptive_sample_threshold_pps: u64,
    pub cache_hit_ratio: f64,
    pub avg_lookup_ns: u64,
    pub samples_dropped_pps: f64,
    pub unknown_pps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobeTrafficSnapshot {
    pub seq: u64,
    pub captured_at: i64,
    pub running_crawlers: usize,
    pub udp_rx_pps: f64,
    pub udp_tx_pps: f64,
    pub udp_rx_bps: f64,
    pub udp_tx_bps: f64,
    pub ipv4_pps: f64,
    pub ipv6_pps: f64,
    pub channels: GlobeChannelRates,
    pub geoip: GlobeGeoIpStatus,
    pub countries: Vec<GlobeCountryTraffic>,
    pub other_countries_pps: f64,
    pub other_countries_bps: f64,
    pub active_country_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverviewWorkerBrief {
    pub id: String,
    pub display_name: String,
    pub role: WorkerRole,
    pub runtime_state: WorkerRuntimeState,
    pub pid: Option<u32>,
    pub restart_count: u64,
    pub rss_bytes: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OverviewSnapshot {
    pub captured_at: i64,
    pub health: OverviewHealth,
    pub funnel: OverviewFunnel,
    pub kpi: OverviewKpi,
    pub workers: Vec<OverviewWorkerBrief>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryPointDto {
    pub ts: i64,
    pub hashes_per_min: f64,
    pub metadata_succeeded_per_min: f64,
    pub stream_pushed_per_min: f64,
    pub filtered_per_min: f64,
    pub indexed_written_per_min: f64,
    pub hot_updated_per_min: f64,
    pub actual_qps: f64,
    pub stream_new_len: i64,
    pub stream_hot_len: i64,
    pub stream_new_lag: i64,
    pub udp_rx_bps: f64,
    pub udp_tx_bps: f64,
    pub metadata_rx_bps: f64,
    pub total_rss_bytes: i64,
}

#[derive(Debug, Clone, Default)]
struct GeoCountryPrevTotals {
    packets: u64,
    bytes: u64,
    get_peers: u64,
    announce_peer: u64,
    response: u64,
    find_node_ping: u64,
    other: u64,
}

#[derive(Debug, Default)]
struct GlobeCountryAgg {
    pps: f64,
    bps: f64,
    get_peers_pps: f64,
    announce_peer_pps: f64,
    response_pps: f64,
    find_node_ping_pps: f64,
    other_pps: f64,
    cumulative_packets: u64,
    cumulative_bytes: u64,
}

#[derive(Debug, Clone, Default)]
struct WorkerPrevTotals {
    uptime_secs: u64,
    captured_at_ms: i64,
    // Crawler counters
    hashes_received: u64,
    early_blocked: u64,
    metadata_succeeded: u64,
    torrents_pushed: u64,
    hot_pushed: u64,
    queries_sent: u64,
    udp_rx_bytes: u64,
    udp_tx_bytes: u64,
    udp_rx_packets: u64,
    udp_tx_packets: u64,
    udp_rx_ipv4_packets: u64,
    udp_rx_ipv6_packets: u64,
    inbound_ping: u64,
    inbound_find_node: u64,
    inbound_get_peers: u64,
    inbound_announce_peer: u64,
    inbound_response: u64,
    inbound_error: u64,
    inbound_other: u64,
    inbound_parse_error: u64,
    udp_invalid: u64,
    udp_queue_full: u64,
    geo_samples_dropped: u64,
    geo_unknown_packets: u64,
    geo_countries: HashMap<String, GeoCountryPrevTotals>,
    metadata_bytes: u64,
    // Indexer counters
    filter_accepted: u64,
    filter_rejected: u64,
    records_written: u64,
    hot_records_updated: u64,
}

#[derive(Debug, Clone, Default)]
struct RedisStreamProbe {
    ok: bool,
    message: Option<String>,
    stream_new_len: i64,
    stream_hot_len: i64,
    stream_new_pending: i64,
    stream_new_lag: i64,
}

#[derive(Debug, Clone)]
struct CachedManticoreStatus {
    ok: bool,
    message: Option<String>,
    status: Option<TableStatusInfo>,
    fetched_at: Instant,
    cached_at_unix: i64,
}

#[derive(Debug, Default)]
struct MinuteAccumulator {
    ts_minute: i64,
    samples: u64,
    hashes_per_min_sum: f64,
    metadata_succeeded_per_min_sum: f64,
    stream_pushed_per_min_sum: f64,
    filtered_per_min_sum: f64,
    indexed_written_per_min_sum: f64,
    hot_updated_per_min_sum: f64,
    actual_qps_sum: f64,
    udp_rx_bps_sum: f64,
    udp_tx_bps_sum: f64,
    metadata_rx_bps_sum: f64,
    rss_sum: i64,
    last_stream_new_len: i64,
    last_stream_hot_len: i64,
    last_stream_new_lag: i64,
}

#[derive(Debug, Default)]
struct HourAccumulator {
    ts_hour: i64,
    hashes_delta_sum: f64,
    metadata_delta_sum: f64,
    stream_pushed_delta_sum: f64,
    filtered_delta_sum: f64,
    indexed_delta_sum: f64,
    hot_delta_sum: f64,
    qps_sum: f64,
    samples: u64,
    max_stream_new_lag: i64,
    rss_sum: i128,
}

#[derive(Debug, Clone, Default)]
struct LocalGeoIpProbe {
    valid: bool,
    resolved_path: String,
    size_bytes: u64,
    build_epoch: u64,
    database_type: String,
    error: Option<String>,
    fingerprint: Option<(String, u64, Option<std::time::SystemTime>)>,
}

#[derive(Debug, Clone, Default)]
struct WorkerCounterHistory {
    last_snapshot_seq: u64,
    last_uptime_secs: u64,
    last_records_written: u64,
    samples: VecDeque<WorkerPrevTotals>,
}

impl WorkerCounterHistory {
    /// 写入新快照（按 `snapshot_seq` 去重，避免轮询相位竞态写入 0 增量重复点）。
    /// 返回本次新快照产生的精确 `records_written` 增量（仅用于今日累计计数，不重复叠加）。
    fn record_snapshot(&mut self, snapshot_seq: u64, sample: WorkerPrevTotals) -> u64 {
        let restarted = !self.samples.is_empty() && sample.uptime_secs < self.last_uptime_secs;
        if restarted {
            self.samples.clear();
            self.last_snapshot_seq = 0;
            self.last_records_written = sample.records_written;
        }

        let is_duplicate =
            !self.samples.is_empty() && snapshot_seq != 0 && snapshot_seq == self.last_snapshot_seq;
        if is_duplicate {
            return 0;
        }

        let delta_written = if self.samples.is_empty() {
            0
        } else {
            sample
                .records_written
                .saturating_sub(self.last_records_written)
        };

        self.last_snapshot_seq = snapshot_seq;
        self.last_uptime_secs = sample.uptime_secs;
        self.last_records_written = sample.records_written;

        let now_ms = sample.captured_at_ms;
        self.samples.push_back(sample);
        // 保留至少 2 个点（且最早点可覆盖 WORKER_HISTORY_RETENTION_MS），用于准确计算滑窗差分
        while self.samples.len() > 2
            && self.samples.get(1).is_some_and(|second| {
                now_ms.saturating_sub(second.captured_at_ms) > WORKER_HISTORY_RETENTION_MS
            })
        {
            self.samples.pop_front();
        }

        delta_written
    }

    /// 选取 `(基准点, 最新点, 窗口时长秒)` 计算滑动窗口平均速率。
    /// 若超过 `SNAPSHOT_STALE_HOLD_MS` 无任何新快照到达，返回 `None` 让速率自然衰减为 0。
    fn rate_window(
        &self,
        now_ms: i64,
        window_ms: i64,
    ) -> Option<(&WorkerPrevTotals, &WorkerPrevTotals, f64)> {
        let latest = self.samples.back()?;
        if now_ms.saturating_sub(latest.captured_at_ms) > SNAPSHOT_STALE_HOLD_MS {
            return None;
        }
        if self.samples.len() < 2 {
            return None;
        }
        // 在历史队列中找距离 latest 刚超过 window_ms 的最新点；若总跨度不足 window_ms，则退化为最早点
        let mut baseline = self.samples.front()?;
        for candidate in self.samples.iter().rev().skip(1) {
            baseline = candidate;
            if latest
                .captured_at_ms
                .saturating_sub(candidate.captured_at_ms)
                >= window_ms
            {
                break;
            }
        }
        let dt_ms = latest
            .captured_at_ms
            .saturating_sub(baseline.captured_at_ms);
        if dt_ms < 200 {
            return None;
        }
        Some((baseline, latest, dt_ms as f64 / 1000.0))
    }
}

struct CollectorInner {
    prev_workers: HashMap<String, WorkerCounterHistory>,
    ring: VecDeque<OverviewSnapshot>,
    minute_acc: Option<MinuteAccumulator>,
    hour_acc: Option<HourAccumulator>,
    last_redis_probe: Option<(Instant, RedisStreamProbe)>,
    last_manticore_status: Option<CachedManticoreStatus>,
    last_geoip_cfg: Option<(Instant, GeoIpConfig)>,
    last_geoip_probe: Option<(Instant, LocalGeoIpProbe)>,
    last_prune_at: Option<Instant>,
    today_ymd: String,
    today_indexed_docs: i64,
}

impl Default for CollectorInner {
    fn default() -> Self {
        Self {
            prev_workers: HashMap::new(),
            ring: VecDeque::with_capacity(RING_CAPACITY),
            minute_acc: None,
            hour_acc: None,
            last_redis_probe: None,
            last_manticore_status: None,
            last_geoip_cfg: None,
            last_geoip_probe: None,
            last_prune_at: None,
            today_ymd: Utc::now().format("%Y-%m-%d").to_string(),
            today_indexed_docs: 0,
        }
    }
}

/// 全局指标采集器：1 秒采样内存环形缓冲、60 秒节流 Manticore 表状态缓存与分钟/小时 SQLite 降采样。
pub struct MetricsCollector {
    db: Database,
    supervisor: Arc<Supervisor>,
    services: Arc<ServiceRuntimePool>,
    inner: Mutex<CollectorInner>,
    latest: RwLock<Option<OverviewSnapshot>>,
    latest_globe: RwLock<Option<GlobeTrafficSnapshot>>,
    globe_seq: AtomicU64,
    tx: broadcast::Sender<OverviewSnapshot>,
    globe_tx: broadcast::Sender<GlobeTrafficSnapshot>,
}

impl MetricsCollector {
    pub fn new(
        db: Database,
        supervisor: Arc<Supervisor>,
        services: Arc<ServiceRuntimePool>,
    ) -> Arc<Self> {
        let (tx, _) = broadcast::channel(32);
        let (globe_tx, _) = broadcast::channel(32);
        Arc::new(Self {
            db,
            supervisor,
            services,
            inner: Mutex::new(CollectorInner::default()),
            latest: RwLock::new(None),
            latest_globe: RwLock::new(None),
            globe_seq: AtomicU64::new(0),
            tx,
            globe_tx,
        })
    }

    pub fn spawn_background_loop(self: &Arc<Self>, shutdown: CancellationToken) {
        let collector = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = ticker.tick() => {
                        if let Err(err) = collector.sample_once().await {
                            tracing::debug!("MetricsCollector 采样周期跳过: {err}");
                        }
                    }
                }
            }
        });
    }

    pub async fn current_snapshot(&self) -> OverviewSnapshot {
        if let Some(snap) = self.latest.read().await.clone() {
            return snap;
        }
        match self.sample_once().await {
            Ok(snap) => snap,
            Err(_) => OverviewSnapshot {
                captured_at: Utc::now().timestamp(),
                health: OverviewHealth {
                    panel_ok: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }

    pub async fn current_globe_snapshot(&self) -> GlobeTrafficSnapshot {
        if let Some(snap) = self.latest_globe.read().await.clone() {
            return snap;
        }
        let _ = self.sample_once().await;
        if let Some(snap) = self.latest_globe.read().await.clone() {
            return snap;
        }
        let geo_cfg = load_geoip_config(&self.db).await;
        let local_probe = self.probe_local_geoip_cached(&geo_cfg).await;
        let globe_geoip = build_globe_geoip_status(&geo_cfg, &local_probe, &[], 0.0, 0.0);
        GlobeTrafficSnapshot {
            captured_at: Utc::now().timestamp(),
            geoip: globe_geoip,
            ..Default::default()
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<OverviewSnapshot> {
        self.tx.subscribe()
    }

    pub fn subscribe_globe(&self) -> broadcast::Receiver<GlobeTrafficSnapshot> {
        self.globe_tx.subscribe()
    }

    /// 服务配置初始化或变更后立即清除探测缓存并刷新最新快照。
    pub async fn invalidate_service_caches(&self) {
        {
            let mut guard = self.inner.lock().await;
            guard.last_redis_probe = None;
            guard.last_manticore_status = None;
            guard.last_geoip_cfg = None;
            guard.last_geoip_probe = None;
        }
        *self.latest.write().await = None;
        *self.latest_globe.write().await = None;
        let _ = self.sample_once().await;
    }

    pub async fn recent_ring_minute_fallback(&self) -> Vec<HistoryPointDto> {
        let guard = self.inner.lock().await;
        guard
            .ring
            .iter()
            .map(|s| HistoryPointDto {
                ts: s.captured_at,
                hashes_per_min: s.funnel.hashes_per_min,
                metadata_succeeded_per_min: s.funnel.metadata_succeeded_per_min,
                stream_pushed_per_min: s.funnel.stream_pushed_per_min,
                filtered_per_min: s.funnel.filter_rejected_per_min,
                indexed_written_per_min: s.funnel.indexed_written_per_min,
                hot_updated_per_min: s.funnel.hot_updated_per_min,
                actual_qps: s.kpi.actual_qps,
                stream_new_len: s.kpi.stream_new_len,
                stream_hot_len: s.kpi.stream_hot_len,
                stream_new_lag: s.kpi.stream_new_lag,
                udp_rx_bps: s.kpi.udp_rx_bps,
                udp_tx_bps: s.kpi.udp_tx_bps,
                metadata_rx_bps: s.kpi.metadata_rx_bps,
                total_rss_bytes: s.kpi.total_rss_bytes,
            })
            .collect()
    }

    pub async fn sample_once(&self) -> anyhow::Result<OverviewSnapshot> {
        let now_dt = Utc::now();
        let now_secs = now_dt.timestamp();
        let now_ms = now_dt.timestamp_millis();
        let today_ymd = now_dt.format("%Y-%m-%d").to_string();

        let workers = self.supervisor.list_worker_statuses().await?;

        // 分别探测 Redis（3s 缓存）、Manticore（60s 缓存）与 GeoIP 配置（5s 缓存）
        let redis_probe = self.probe_redis_cached().await;
        let manticore_status = self.probe_manticore_cached(now_secs).await;
        let geoip_cfg = self.load_geoip_config_cached().await;
        let local_geoip_probe = self.probe_local_geoip_cached(&geoip_cfg).await;

        let mut health = OverviewHealth {
            panel_ok: true,
            redis_ok: redis_probe.ok,
            redis_message: redis_probe.message.clone(),
            manticore_ok: manticore_status.ok,
            manticore_message: manticore_status.message.clone(),
            ..Default::default()
        };

        let mut funnel = OverviewFunnel::default();
        let mut actual_qps = 0.0_f64;
        let mut udp_rx_bps = 0.0_f64;
        let mut udp_tx_bps = 0.0_f64;
        let mut udp_rx_pps = 0.0_f64;
        let mut udp_tx_pps = 0.0_f64;
        let mut ipv4_pps = 0.0_f64;
        let mut ipv6_pps = 0.0_f64;
        let mut metadata_rx_bps = 0.0_f64;
        let mut total_rss_bytes = 0_i64;
        let mut worker_briefs = Vec::with_capacity(workers.len());
        let mut channels = GlobeChannelRates::default();
        let mut geo_samples_dropped_pps = 0.0_f64;
        let mut geo_unknown_pps = 0.0_f64;
        let mut geo_country_agg: HashMap<String, GlobeCountryAgg> = HashMap::new();
        let mut crawler_geo_views: Vec<Value> = Vec::new();

        let mut guard = self.inner.lock().await;
        if guard.today_ymd != today_ymd {
            guard.today_ymd = today_ymd;
            guard.today_indexed_docs = 0;
        }
        let running_worker_ids: HashSet<&str> = workers
            .iter()
            .filter(|w| w.runtime_state == WorkerRuntimeState::Running)
            .map(|w| w.id.as_str())
            .collect();
        guard
            .prev_workers
            .retain(|id, _| running_worker_ids.contains(id.as_str()));

        for w in &workers {
            match w.role {
                WorkerRole::Crawler => {
                    health.total_crawlers += 1;
                    if w.runtime_state == WorkerRuntimeState::Running {
                        health.running_crawlers += 1;
                    }
                }
                WorkerRole::Indexer => {
                    health.total_indexers += 1;
                    if w.runtime_state == WorkerRuntimeState::Running {
                        health.running_indexers += 1;
                    }
                }
            }
            if w.runtime_state == WorkerRuntimeState::Crashed {
                health.crashed_workers += 1;
            }

            let rss_bytes = w
                .snapshot
                .as_ref()
                .and_then(|s| s.payload.get("rss_bytes"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            total_rss_bytes = total_rss_bytes.saturating_add(i64::try_from(rss_bytes).unwrap_or(0));

            if w.runtime_state == WorkerRuntimeState::Running
                && let Some(env) = &w.snapshot
            {
                let p = &env.payload;
                let uptime_secs = p.get("uptime_secs").and_then(Value::as_u64).unwrap_or(0);
                let sample_ts_ms = if w.snapshot_fetched_at_ms > 0 {
                    w.snapshot_fetched_at_ms
                } else {
                    now_ms
                };

                match w.role {
                    WorkerRole::Crawler => {
                        let hashes = json_u64(p, &["dht", "hashes_received"]);
                        let early_blocked = json_u64(p, &["app", "early_filter_blocked"]);
                        let meta_ok = json_u64(p, &["app", "torrents_downloaded"]);
                        let pushed = json_u64(p, &["app", "torrents_pushed"]);
                        let hot_pushed = json_u64(p, &["app", "hot_pushed"]);
                        let queries = json_u64(p, &["dht", "queries_new"])
                            .saturating_add(json_u64(p, &["dht", "queries_revisit"]))
                            .saturating_add(json_u64(p, &["dht", "queries_bootstrap"]));
                        let udp_rx = json_u64(p, &["observability", "udp_rx_bytes"]);
                        let udp_tx = json_u64(p, &["observability", "udp_tx_bytes"]);
                        let udp_rx_packets = json_u64(p, &["observability", "udp_rx_packets"])
                            .max(json_u64(p, &["dht", "udp_received"]));
                        let udp_tx_packets = json_u64(p, &["observability", "udp_tx_packets"]);
                        let udp_rx_ipv4_packets =
                            json_u64(p, &["observability", "udp_rx_ipv4_packets"]);
                        let udp_rx_ipv6_packets =
                            json_u64(p, &["observability", "udp_rx_ipv6_packets"]);
                        let inbound_ping = json_u64(p, &["observability", "inbound_ping"]);
                        let inbound_find_node =
                            json_u64(p, &["observability", "inbound_find_node"]);
                        let inbound_get_peers =
                            json_u64(p, &["observability", "inbound_get_peers"]);
                        let inbound_announce_peer =
                            json_u64(p, &["observability", "inbound_announce_peer"]);
                        let inbound_response = json_u64(p, &["observability", "inbound_response"]);
                        let inbound_error = json_u64(p, &["observability", "inbound_error"]);
                        let inbound_other = json_u64(p, &["observability", "inbound_other"]);
                        let inbound_parse_error =
                            json_u64(p, &["observability", "inbound_parse_error"]);
                        let udp_invalid = json_u64(p, &["dht", "udp_invalid"]);
                        let udp_queue_full = json_u64(p, &["dht", "udp_queue_full"]);
                        let meta_bytes =
                            json_u64(p, &["observability", "metadata_bytes_downloaded"]);

                        let mut geo_samples_dropped = 0_u64;
                        let mut geo_unknown_packets = 0_u64;
                        let mut geo_countries = HashMap::new();
                        if let Some(geo_val) = p.get("geo") {
                            crawler_geo_views.push(geo_val.clone());
                            geo_samples_dropped = json_u64(geo_val, &["samples_dropped"]);
                            geo_unknown_packets = json_u64(geo_val, &["unknown_packets"]);
                            geo_countries = extract_geo_countries(geo_val);
                            for (iso, cur) in &geo_countries {
                                let agg = geo_country_agg.entry(iso.clone()).or_default();
                                agg.cumulative_packets =
                                    agg.cumulative_packets.saturating_add(cur.packets);
                                agg.cumulative_bytes =
                                    agg.cumulative_bytes.saturating_add(cur.bytes);
                            }
                        }

                        let history = guard.prev_workers.entry(w.id.clone()).or_default();
                        history.record_snapshot(
                            w.snapshot_seq,
                            WorkerPrevTotals {
                                uptime_secs,
                                captured_at_ms: sample_ts_ms,
                                hashes_received: hashes,
                                early_blocked,
                                metadata_succeeded: meta_ok,
                                torrents_pushed: pushed,
                                hot_pushed,
                                queries_sent: queries,
                                udp_rx_bytes: udp_rx,
                                udp_tx_bytes: udp_tx,
                                udp_rx_packets,
                                udp_tx_packets,
                                udp_rx_ipv4_packets,
                                udp_rx_ipv6_packets,
                                inbound_ping,
                                inbound_find_node,
                                inbound_get_peers,
                                inbound_announce_peer,
                                inbound_response,
                                inbound_error,
                                inbound_other,
                                inbound_parse_error,
                                udp_invalid,
                                udp_queue_full,
                                geo_samples_dropped,
                                geo_unknown_packets,
                                geo_countries,
                                metadata_bytes: meta_bytes,
                                ..Default::default()
                            },
                        );

                        if let Some((prev_s, cur_s, dt_secs)) =
                            history.rate_window(now_ms, CRAWLER_RATE_WINDOW_MS)
                        {
                            let per_min_scale = 60.0 / dt_secs;
                            funnel.hashes_per_min +=
                                (cur_s.hashes_received.saturating_sub(prev_s.hashes_received))
                                    as f64
                                    * per_min_scale;
                            funnel.dedup_blocked_per_min +=
                                (cur_s.early_blocked.saturating_sub(prev_s.early_blocked)) as f64
                                    * per_min_scale;
                            funnel.metadata_succeeded_per_min += (cur_s
                                .metadata_succeeded
                                .saturating_sub(prev_s.metadata_succeeded))
                                as f64
                                * per_min_scale;
                            funnel.stream_pushed_per_min +=
                                (cur_s.torrents_pushed.saturating_sub(prev_s.torrents_pushed))
                                    as f64
                                    * per_min_scale;
                            funnel.hot_pushed_per_min +=
                                (cur_s.hot_pushed.saturating_sub(prev_s.hot_pushed)) as f64
                                    * per_min_scale;
                            actual_qps += (cur_s.queries_sent.saturating_sub(prev_s.queries_sent))
                                as f64
                                / dt_secs;
                            udp_rx_bps += (cur_s.udp_rx_bytes.saturating_sub(prev_s.udp_rx_bytes))
                                as f64
                                / dt_secs;
                            udp_tx_bps += (cur_s.udp_tx_bytes.saturating_sub(prev_s.udp_tx_bytes))
                                as f64
                                / dt_secs;
                            udp_rx_pps +=
                                (cur_s.udp_rx_packets.saturating_sub(prev_s.udp_rx_packets)) as f64
                                    / dt_secs;
                            udp_tx_pps +=
                                (cur_s.udp_tx_packets.saturating_sub(prev_s.udp_tx_packets)) as f64
                                    / dt_secs;
                            ipv4_pps += (cur_s
                                .udp_rx_ipv4_packets
                                .saturating_sub(prev_s.udp_rx_ipv4_packets))
                                as f64
                                / dt_secs;
                            ipv6_pps += (cur_s
                                .udp_rx_ipv6_packets
                                .saturating_sub(prev_s.udp_rx_ipv6_packets))
                                as f64
                                / dt_secs;
                            metadata_rx_bps +=
                                (cur_s.metadata_bytes.saturating_sub(prev_s.metadata_bytes)) as f64
                                    / dt_secs;

                            channels.get_peers_pps += (cur_s
                                .inbound_get_peers
                                .saturating_sub(prev_s.inbound_get_peers))
                                as f64
                                / dt_secs;
                            channels.announce_peer_pps += (cur_s
                                .inbound_announce_peer
                                .saturating_sub(prev_s.inbound_announce_peer))
                                as f64
                                / dt_secs;
                            channels.response_pps += (cur_s
                                .inbound_response
                                .saturating_sub(prev_s.inbound_response))
                                as f64
                                / dt_secs;
                            let prev_fp =
                                prev_s.inbound_ping.saturating_add(prev_s.inbound_find_node);
                            let cur_fp = cur_s.inbound_ping.saturating_add(cur_s.inbound_find_node);
                            channels.find_node_ping_pps +=
                                (cur_fp.saturating_sub(prev_fp)) as f64 / dt_secs;
                            channels.error_pps +=
                                (cur_s.inbound_error.saturating_sub(prev_s.inbound_error)) as f64
                                    / dt_secs;
                            channels.other_pps +=
                                (cur_s.inbound_other.saturating_sub(prev_s.inbound_other)) as f64
                                    / dt_secs;
                            channels.parse_error_pps += (cur_s
                                .inbound_parse_error
                                .saturating_sub(prev_s.inbound_parse_error))
                                as f64
                                / dt_secs;
                            channels.invalid_udp_pps +=
                                (cur_s.udp_invalid.saturating_sub(prev_s.udp_invalid)) as f64
                                    / dt_secs;
                            channels.queue_full_pps +=
                                (cur_s.udp_queue_full.saturating_sub(prev_s.udp_queue_full)) as f64
                                    / dt_secs;

                            geo_samples_dropped_pps += (cur_s
                                .geo_samples_dropped
                                .saturating_sub(prev_s.geo_samples_dropped))
                                as f64
                                / dt_secs;
                            geo_unknown_pps += (cur_s
                                .geo_unknown_packets
                                .saturating_sub(prev_s.geo_unknown_packets))
                                as f64
                                / dt_secs;

                            for (iso, cur) in &cur_s.geo_countries {
                                let prev_c = prev_s.geo_countries.get(iso);
                                let p_pkts = prev_c.map_or(0, |c| c.packets);
                                let p_bytes = prev_c.map_or(0, |c| c.bytes);
                                let p_gp = prev_c.map_or(0, |c| c.get_peers);
                                let p_ap = prev_c.map_or(0, |c| c.announce_peer);
                                let p_resp = prev_c.map_or(0, |c| c.response);
                                let p_fp = prev_c.map_or(0, |c| c.find_node_ping);
                                let p_other = prev_c.map_or(0, |c| c.other);

                                let agg = geo_country_agg.entry(iso.clone()).or_default();
                                agg.pps += (cur.packets.saturating_sub(p_pkts)) as f64 / dt_secs;
                                agg.bps += (cur.bytes.saturating_sub(p_bytes)) as f64 / dt_secs;
                                agg.get_peers_pps +=
                                    (cur.get_peers.saturating_sub(p_gp)) as f64 / dt_secs;
                                agg.announce_peer_pps +=
                                    (cur.announce_peer.saturating_sub(p_ap)) as f64 / dt_secs;
                                agg.response_pps +=
                                    (cur.response.saturating_sub(p_resp)) as f64 / dt_secs;
                                agg.find_node_ping_pps +=
                                    (cur.find_node_ping.saturating_sub(p_fp)) as f64 / dt_secs;
                                agg.other_pps +=
                                    (cur.other.saturating_sub(p_other)) as f64 / dt_secs;
                            }
                        }
                    }
                    WorkerRole::Indexer => {
                        let accepted = json_u64(p, &["app", "filter_accepted"]);
                        let rejected = json_u64(p, &["app", "filter_rejected"]);
                        let written = json_u64(p, &["app", "records_written"]);
                        let hot_updated = json_u64(p, &["app", "hot_records_updated"]);

                        let delta_written = {
                            let history = guard.prev_workers.entry(w.id.clone()).or_default();
                            history.record_snapshot(
                                w.snapshot_seq,
                                WorkerPrevTotals {
                                    uptime_secs,
                                    captured_at_ms: sample_ts_ms,
                                    filter_accepted: accepted,
                                    filter_rejected: rejected,
                                    records_written: written,
                                    hot_records_updated: hot_updated,
                                    ..Default::default()
                                },
                            )
                        };
                        guard.today_indexed_docs = guard
                            .today_indexed_docs
                            .saturating_add(i64::try_from(delta_written).unwrap_or(0));

                        if let Some(history) = guard.prev_workers.get(&w.id)
                            && let Some((prev_s, cur_s, dt_secs)) =
                                history.rate_window(now_ms, INDEXER_RATE_WINDOW_MS)
                        {
                            let per_min_scale = 60.0 / dt_secs;
                            funnel.filter_accepted_per_min +=
                                (cur_s.filter_accepted.saturating_sub(prev_s.filter_accepted))
                                    as f64
                                    * per_min_scale;
                            funnel.filter_rejected_per_min +=
                                (cur_s.filter_rejected.saturating_sub(prev_s.filter_rejected))
                                    as f64
                                    * per_min_scale;
                            funnel.indexed_written_per_min +=
                                (cur_s.records_written.saturating_sub(prev_s.records_written))
                                    as f64
                                    * per_min_scale;
                            funnel.hot_updated_per_min += (cur_s
                                .hot_records_updated
                                .saturating_sub(prev_s.hot_records_updated))
                                as f64
                                * per_min_scale;
                        }
                    }
                }
            }

            worker_briefs.push(OverviewWorkerBrief {
                id: w.id.clone(),
                display_name: w.display_name.clone(),
                role: w.role,
                runtime_state: w.runtime_state,
                pid: w.pid,
                restart_count: w.restart_count,
                rss_bytes,
                last_error: w.last_error.clone(),
            });
        }

        let running_crawlers = health.running_crawlers;
        let snapshot = OverviewSnapshot {
            captured_at: now_secs,
            health,
            kpi: OverviewKpi {
                actual_qps,
                indexed_written_per_min: funnel.indexed_written_per_min,
                estimated_daily_docs: funnel.indexed_written_per_min * 1440.0,
                today_indexed_docs: guard.today_indexed_docs,
                torrents_total_docs: manticore_status
                    .status
                    .as_ref()
                    .and_then(|s| s.indexed_documents),
                torrents_disk_bytes: manticore_status.status.as_ref().and_then(|s| s.disk_bytes),
                torrents_status_cached_at: Some(manticore_status.cached_at_unix),
                stream_new_len: redis_probe.stream_new_len,
                stream_hot_len: redis_probe.stream_hot_len,
                stream_new_pending: redis_probe.stream_new_pending,
                stream_new_lag: redis_probe.stream_new_lag,
                total_rss_bytes,
                udp_rx_bps,
                udp_tx_bps,
                udp_rx_pps,
                udp_tx_pps,
                metadata_rx_bps,
                system_soft_nofile: current_soft_open_file_limit(),
            },
            funnel,
            workers: worker_briefs,
        };

        let globe_geoip = build_globe_geoip_status(
            &geoip_cfg,
            &local_geoip_probe,
            &crawler_geo_views,
            geo_samples_dropped_pps,
            geo_unknown_pps,
        );
        let (countries, other_countries_pps, other_countries_bps, active_country_count) =
            finalize_globe_countries(geo_country_agg, &globe_geoip.home_country);
        let globe_snapshot = GlobeTrafficSnapshot {
            seq: self
                .globe_seq
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1),
            captured_at: now_secs,
            running_crawlers,
            udp_rx_pps,
            udp_tx_pps,
            udp_rx_bps,
            udp_tx_bps,
            ipv4_pps,
            ipv6_pps,
            channels,
            geoip: globe_geoip,
            countries,
            other_countries_pps,
            other_countries_bps,
            active_country_count,
        };

        if guard.ring.len() >= RING_CAPACITY {
            guard.ring.pop_front();
        }
        guard.ring.push_back(snapshot.clone());

        let (minute_to_flush, hour_to_flush, should_prune) =
            update_accumulators(&mut guard, &snapshot);
        drop(guard);

        // 只在面板完成初始化后执行历史表持久化写入，避免无意义写入
        if self.db.is_initialized().await.unwrap_or(false) {
            if let Some(m) = minute_to_flush {
                let _ = self.db.upsert_metrics_minute(&m).await;
            }
            if let Some(h) = hour_to_flush {
                let _ = self.db.upsert_metrics_hour(&h).await;
            }
            if should_prune {
                let min_before = now_secs - 7 * 24 * 3600;
                let hour_before = now_secs - 30 * 24 * 3600;
                let _ = self.db.prune_metrics(min_before, hour_before).await;
            }
        }

        {
            let mut latest_w = self.latest.write().await;
            *latest_w = Some(snapshot.clone());
        }
        {
            let mut latest_g = self.latest_globe.write().await;
            *latest_g = Some(globe_snapshot.clone());
        }
        let _ = self.tx.send(snapshot.clone());
        let _ = self.globe_tx.send(globe_snapshot);

        Ok(snapshot)
    }

    async fn load_geoip_config_cached(&self) -> GeoIpConfig {
        {
            let guard = self.inner.lock().await;
            if let Some((at, ref cached)) = guard.last_geoip_cfg
                && at.elapsed() < GEOIP_CONFIG_TTL
            {
                return cached.clone();
            }
        }
        let cfg = load_geoip_config(&self.db).await;
        let mut guard = self.inner.lock().await;
        guard.last_geoip_cfg = Some((Instant::now(), cfg.clone()));
        cfg
    }

    async fn probe_local_geoip_cached(&self, geo_cfg: &GeoIpConfig) -> LocalGeoIpProbe {
        let resolved = geo_cfg.resolve_mmdb_path(self.supervisor.data_dir());
        let resolved_str = resolved.to_string_lossy().into_owned();
        {
            let guard = self.inner.lock().await;
            if let Some((at, ref cached)) = guard.last_geoip_probe
                && at.elapsed() < GEOIP_CONFIG_TTL
                && cached.resolved_path == resolved_str
            {
                return cached.clone();
            }
        }

        let prev_probe = {
            let guard = self.inner.lock().await;
            guard.last_geoip_probe.as_ref().map(|(_, p)| p.clone())
        };

        let probed = tokio::task::spawn_blocking(move || {
            let meta = match std::fs::metadata(&resolved) {
                Ok(m) => m,
                Err(err) => {
                    return LocalGeoIpProbe {
                        valid: false,
                        resolved_path: resolved_str,
                        error: Some(format!("未找到有效的 GeoLite2-Country.mmdb 离线库: {err}")),
                        ..Default::default()
                    };
                }
            };
            let size_bytes = meta.len();
            let modified = meta.modified().ok();
            let fp = (resolved_str.clone(), size_bytes, modified);

            if let Some(prev) = prev_probe
                && prev.fingerprint.as_ref() == Some(&fp)
            {
                return prev;
            }

            match maxminddb::Reader::open_readfile(&resolved) {
                Ok(reader) => {
                    let md = reader.metadata();
                    if md.node_count == 0 {
                        return LocalGeoIpProbe {
                            valid: false,
                            resolved_path: resolved_str,
                            size_bytes,
                            error: Some("MMDB 元数据无效 (node_count = 0)".to_string()),
                            fingerprint: Some(fp),
                            ..Default::default()
                        };
                    }
                    LocalGeoIpProbe {
                        valid: true,
                        resolved_path: resolved_str,
                        size_bytes,
                        build_epoch: md.build_epoch,
                        database_type: md.database_type.clone(),
                        error: None,
                        fingerprint: Some(fp),
                    }
                }
                Err(err) => LocalGeoIpProbe {
                    valid: false,
                    resolved_path: resolved_str,
                    size_bytes,
                    error: Some(format!("MMDB 文件损坏或格式不兼容: {err}")),
                    fingerprint: Some(fp),
                    ..Default::default()
                },
            }
        })
        .await
        .unwrap_or_else(|err| LocalGeoIpProbe {
            valid: false,
            error: Some(format!("探测 MMDB 离线库失败: {err}")),
            ..Default::default()
        });

        let mut guard = self.inner.lock().await;
        guard.last_geoip_probe = Some((Instant::now(), probed.clone()));
        probed
    }

    async fn probe_redis_cached(&self) -> RedisStreamProbe {
        {
            let guard = self.inner.lock().await;
            if let Some((at, ref cached)) = guard.last_redis_probe
                && at.elapsed() < REDIS_PROBE_TTL
            {
                return cached.clone();
            }
        }

        let probe = match self.services.redis_pool().await {
            Some((cfg, pool)) => match pool.get().await {
                Ok(mut conn) => {
                    let new_len: i64 = redis::cmd("XLEN")
                        .arg(&cfg.torrent_stream)
                        .query_async(&mut conn)
                        .await
                        .unwrap_or(0);
                    let hot_len: i64 = redis::cmd("XLEN")
                        .arg(&cfg.hot_stream)
                        .query_async(&mut conn)
                        .await
                        .unwrap_or(0);

                    let mut pending = 0_i64;
                    let mut lag = 0_i64;
                    if let Ok(groups_val) = redis::cmd("XINFO")
                        .arg("GROUPS")
                        .arg(&cfg.torrent_stream)
                        .query_async::<RedisValue>(&mut conn)
                        .await
                    {
                        let (p, l) = extract_group_metrics(groups_val, &cfg.consumer_group);
                        pending = p;
                        lag = l;
                    }

                    RedisStreamProbe {
                        ok: true,
                        message: None,
                        stream_new_len: new_len,
                        stream_hot_len: hot_len,
                        stream_new_pending: pending,
                        stream_new_lag: lag,
                    }
                }
                Err(err) => RedisStreamProbe {
                    ok: false,
                    message: Some(format!("Redis 连接池不可用: {err}")),
                    ..Default::default()
                },
            },
            None => RedisStreamProbe {
                ok: false,
                message: Some("Redis 服务尚未配置".to_string()),
                ..Default::default()
            },
        };

        let mut guard = self.inner.lock().await;
        guard.last_redis_probe = Some((Instant::now(), probe.clone()));
        probe
    }

    async fn probe_manticore_cached(&self, now_secs: i64) -> CachedManticoreStatus {
        {
            let guard = self.inner.lock().await;
            if let Some(ref cached) = guard.last_manticore_status
                && cached.fetched_at.elapsed() < MANTICORE_STATUS_TTL
            {
                return cached.clone();
            }
        }

        let fetched = match self.services.manticore_client().await {
            Some((_, client)) => match client.table_status().await {
                Ok(st) => CachedManticoreStatus {
                    ok: true,
                    message: None,
                    status: Some(st),
                    fetched_at: Instant::now(),
                    cached_at_unix: now_secs,
                },
                Err(err) => CachedManticoreStatus {
                    ok: false,
                    message: Some(if is_missing_table_error(&err.to_string()) {
                        "Manticore 已连通，但尚未创建 torrents 表（请前往「服务配置 → Manticore Search」点击「自动创建 torrents 表」）".to_string()
                    } else {
                        format!("Manticore 查询失败: {err}")
                    }),
                    status: None,
                    fetched_at: Instant::now(),
                    cached_at_unix: now_secs,
                },
            },
            None => CachedManticoreStatus {
                ok: false,
                message: Some("Manticore 服务尚未配置".to_string()),
                status: None,
                fetched_at: Instant::now(),
                cached_at_unix: now_secs,
            },
        };

        let mut guard = self.inner.lock().await;
        guard.last_manticore_status = Some(fetched.clone());
        fetched
    }
}

fn update_accumulators(
    inner: &mut CollectorInner,
    snap: &OverviewSnapshot,
) -> (Option<MetricsMinutePoint>, Option<MetricsHourPoint>, bool) {
    let ts_minute = (snap.captured_at / 60) * 60;
    let ts_hour = (snap.captured_at / 3600) * 3600;

    let mut flushed_minute = None;
    let cur_min = inner.minute_acc.get_or_insert_with(|| MinuteAccumulator {
        ts_minute,
        ..Default::default()
    });
    if cur_min.ts_minute != ts_minute {
        if cur_min.samples > 0 {
            flushed_minute = Some(acc_to_minute_point(cur_min));
        }
        *cur_min = MinuteAccumulator {
            ts_minute,
            ..Default::default()
        };
    }

    cur_min.samples += 1;
    cur_min.hashes_per_min_sum += snap.funnel.hashes_per_min;
    cur_min.metadata_succeeded_per_min_sum += snap.funnel.metadata_succeeded_per_min;
    cur_min.stream_pushed_per_min_sum += snap.funnel.stream_pushed_per_min;
    cur_min.filtered_per_min_sum += snap.funnel.filter_rejected_per_min;
    cur_min.indexed_written_per_min_sum += snap.funnel.indexed_written_per_min;
    cur_min.hot_updated_per_min_sum += snap.funnel.hot_updated_per_min;
    cur_min.actual_qps_sum += snap.kpi.actual_qps;
    cur_min.udp_rx_bps_sum += snap.kpi.udp_rx_bps;
    cur_min.udp_tx_bps_sum += snap.kpi.udp_tx_bps;
    cur_min.metadata_rx_bps_sum += snap.kpi.metadata_rx_bps;
    cur_min.rss_sum = snap.kpi.total_rss_bytes;
    cur_min.last_stream_new_len = snap.kpi.stream_new_len;
    cur_min.last_stream_hot_len = snap.kpi.stream_hot_len;
    cur_min.last_stream_new_lag = snap.kpi.stream_new_lag;

    // 当天小时级降采样累计，每次分钟切换同时顺带 upsert 当小时最新值
    let cur_hour = inner.hour_acc.get_or_insert_with(|| HourAccumulator {
        ts_hour,
        ..Default::default()
    });
    let mut flushed_hour = None;
    if cur_hour.ts_hour != ts_hour {
        if cur_hour.samples > 0 {
            flushed_hour = Some(acc_to_hour_point(cur_hour));
        }
        *cur_hour = HourAccumulator {
            ts_hour,
            ..Default::default()
        };
    }
    cur_hour.samples += 1;
    cur_hour.hashes_delta_sum += snap.funnel.hashes_per_min / 60.0;
    cur_hour.metadata_delta_sum += snap.funnel.metadata_succeeded_per_min / 60.0;
    cur_hour.stream_pushed_delta_sum += snap.funnel.stream_pushed_per_min / 60.0;
    cur_hour.filtered_delta_sum += snap.funnel.filter_rejected_per_min / 60.0;
    cur_hour.indexed_delta_sum += snap.funnel.indexed_written_per_min / 60.0;
    cur_hour.hot_delta_sum += snap.funnel.hot_updated_per_min / 60.0;
    cur_hour.qps_sum += snap.kpi.actual_qps;
    cur_hour.max_stream_new_lag = cur_hour.max_stream_new_lag.max(snap.kpi.stream_new_lag);
    cur_hour.rss_sum = cur_hour
        .rss_sum
        .saturating_add(i128::from(snap.kpi.total_rss_bytes));

    if flushed_hour.is_none() && flushed_minute.is_some() && cur_hour.samples > 0 {
        flushed_hour = Some(acc_to_hour_point(cur_hour));
    }

    let should_prune = inner
        .last_prune_at
        .is_none_or(|t| t.elapsed() >= Duration::from_secs(3600));
    if should_prune {
        inner.last_prune_at = Some(Instant::now());
    }

    (flushed_minute, flushed_hour, should_prune)
}

fn acc_to_minute_point(acc: &MinuteAccumulator) -> MetricsMinutePoint {
    let n = acc.samples.max(1) as f64;
    MetricsMinutePoint {
        ts_minute: acc.ts_minute,
        hashes_per_min: acc.hashes_per_min_sum / n,
        metadata_succeeded_per_min: acc.metadata_succeeded_per_min_sum / n,
        stream_pushed_per_min: acc.stream_pushed_per_min_sum / n,
        filtered_per_min: acc.filtered_per_min_sum / n,
        indexed_written_per_min: acc.indexed_written_per_min_sum / n,
        hot_updated_per_min: acc.hot_updated_per_min_sum / n,
        actual_qps: acc.actual_qps_sum / n,
        stream_new_len: acc.last_stream_new_len,
        stream_hot_len: acc.last_stream_hot_len,
        stream_new_lag: acc.last_stream_new_lag,
        udp_rx_bps: acc.udp_rx_bps_sum / n,
        udp_tx_bps: acc.udp_tx_bps_sum / n,
        metadata_rx_bps: acc.metadata_rx_bps_sum / n,
        total_rss_bytes: acc.rss_sum,
        avg_cpu_percent: 0.0,
    }
}

fn acc_to_hour_point(acc: &HourAccumulator) -> MetricsHourPoint {
    let n = acc.samples.max(1) as f64;
    let avg_rss = i64::try_from(acc.rss_sum / i128::from(acc.samples.max(1))).unwrap_or(0);
    MetricsHourPoint {
        ts_hour: acc.ts_hour,
        hashes_total: acc.hashes_delta_sum.round() as i64,
        metadata_succeeded_total: acc.metadata_delta_sum.round() as i64,
        stream_pushed_total: acc.stream_pushed_delta_sum.round() as i64,
        filtered_total: acc.filtered_delta_sum.round() as i64,
        indexed_written_total: acc.indexed_delta_sum.round() as i64,
        hot_updated_total: acc.hot_delta_sum.round() as i64,
        avg_qps: acc.qps_sum / n,
        max_stream_new_lag: acc.max_stream_new_lag,
        avg_rss_bytes: avg_rss,
    }
}

fn json_u64(root: &Value, path: &[&str]) -> u64 {
    let mut cur = root;
    for key in path {
        cur = match cur.get(*key) {
            Some(v) => v,
            None => return 0,
        };
    }
    cur.as_u64().unwrap_or(0)
}

fn json_bool(root: &Value, path: &[&str], default_val: bool) -> bool {
    let mut cur = root;
    for key in path {
        cur = match cur.get(*key) {
            Some(v) => v,
            None => return default_val,
        };
    }
    cur.as_bool().unwrap_or(default_val)
}

fn json_str(root: &Value, path: &[&str]) -> String {
    let mut cur = root;
    for key in path {
        cur = match cur.get(*key) {
            Some(v) => v,
            None => return String::new(),
        };
    }
    cur.as_str().unwrap_or("").to_string()
}

fn extract_geo_countries(geo_val: &Value) -> HashMap<String, GeoCountryPrevTotals> {
    let mut out = HashMap::new();
    let Some(map) = geo_val.get("countries").and_then(Value::as_object) else {
        return out;
    };
    for (iso, entry) in map {
        let code = iso.trim().to_ascii_uppercase();
        if code.len() != 2 {
            continue;
        }
        out.insert(
            code,
            GeoCountryPrevTotals {
                packets: entry.get("packets").and_then(Value::as_u64).unwrap_or(0),
                bytes: entry.get("bytes").and_then(Value::as_u64).unwrap_or(0),
                get_peers: entry.get("get_peers").and_then(Value::as_u64).unwrap_or(0),
                announce_peer: entry
                    .get("announce_peer")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                response: entry.get("response").and_then(Value::as_u64).unwrap_or(0),
                find_node_ping: entry
                    .get("find_node_ping")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                other: entry.get("other").and_then(Value::as_u64).unwrap_or(0),
            },
        );
    }
    out
}

fn build_globe_geoip_status(
    geo_cfg: &GeoIpConfig,
    local_probe: &LocalGeoIpProbe,
    crawler_geo_views: &[Value],
    samples_dropped_pps: f64,
    unknown_pps: f64,
) -> GlobeGeoIpStatus {
    let fallback_home = geo_cfg.normalized_home_country();
    if crawler_geo_views.is_empty() {
        let error = if !geo_cfg.enabled {
            Some("GeoIP 国家归因已在设置中停用".to_string())
        } else if local_probe.valid {
            None
        } else {
            Some(
                local_probe
                    .error
                    .clone()
                    .unwrap_or_else(|| "未找到有效的 GeoLite2-Country.mmdb 离线库".to_string()),
            )
        };
        return GlobeGeoIpStatus {
            enabled: geo_cfg.enabled,
            mmdb_loaded: false,
            mmdb_file_ready: local_probe.valid,
            mmdb_path: if local_probe.resolved_path.is_empty() {
                geo_cfg.mmdb_path.clone()
            } else {
                local_probe.resolved_path.clone()
            },
            mmdb_size_bytes: local_probe.size_bytes,
            mmdb_build_epoch: local_probe.build_epoch,
            database_type: local_probe.database_type.clone(),
            error,
            home_country: fallback_home,
            sample_rate_divisor: 1,
            adaptive_sample_threshold_pps: geo_cfg.adaptive_sample_threshold_pps,
            cache_hit_ratio: 0.0,
            avg_lookup_ns: 0,
            samples_dropped_pps,
            unknown_pps,
        };
    }

    let enabled = crawler_geo_views
        .iter()
        .any(|v| json_bool(v, &["enabled"], geo_cfg.enabled));
    let mmdb_loaded = crawler_geo_views
        .iter()
        .any(|v| json_bool(v, &["mmdb_loaded"], false));

    let primary = crawler_geo_views
        .iter()
        .find(|v| json_bool(v, &["mmdb_loaded"], false))
        .unwrap_or(&crawler_geo_views[0]);

    let mmdb_path = {
        let p = json_str(primary, &["mmdb_path"]);
        if p.is_empty() {
            geo_cfg.mmdb_path.clone()
        } else {
            p
        }
    };
    let home_country = {
        let h = json_str(primary, &["home_country"]);
        if h.len() == 2 {
            h.to_ascii_uppercase()
        } else {
            fallback_home
        }
    };

    let sample_rate_divisor = crawler_geo_views
        .iter()
        .filter_map(|v| v.get("sample_rate_divisor").and_then(Value::as_u64))
        .max()
        .and_then(|d| u16::try_from(d).ok())
        .unwrap_or(1)
        .max(1);

    let cache_hits_sum: u64 = crawler_geo_views
        .iter()
        .map(|v| json_u64(v, &["cache_hits"]))
        .sum();
    let cache_misses_sum: u64 = crawler_geo_views
        .iter()
        .map(|v| json_u64(v, &["cache_misses"]))
        .sum();
    let cache_total = cache_hits_sum.saturating_add(cache_misses_sum);
    let cache_hit_ratio = if cache_total > 0 {
        cache_hits_sum as f64 / cache_total as f64
    } else {
        0.0
    };

    let mut ns_sum = 0_u64;
    let mut ns_count = 0_u64;
    for v in crawler_geo_views {
        let ns = json_u64(v, &["avg_lookup_ns"]);
        if ns > 0 {
            ns_sum = ns_sum.saturating_add(ns);
            ns_count += 1;
        }
    }

    let err_opt = if mmdb_loaded {
        None
    } else {
        crawler_geo_views.iter().find_map(|v| {
            v.get("error")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
    };

    let threshold = json_u64(primary, &["adaptive_sample_threshold_pps"]);
    GlobeGeoIpStatus {
        enabled,
        mmdb_loaded,
        mmdb_file_ready: mmdb_loaded || local_probe.valid,
        mmdb_path,
        mmdb_size_bytes: json_u64(primary, &["mmdb_size_bytes"]),
        mmdb_build_epoch: json_u64(primary, &["mmdb_build_epoch"]),
        database_type: json_str(primary, &["database_type"]),
        error: err_opt,
        home_country,
        sample_rate_divisor,
        adaptive_sample_threshold_pps: if threshold > 0 {
            threshold
        } else {
            geo_cfg.adaptive_sample_threshold_pps
        },
        cache_hit_ratio,
        avg_lookup_ns: ns_sum.checked_div(ns_count).unwrap_or(0),
        samples_dropped_pps,
        unknown_pps,
    }
}

fn finalize_globe_countries(
    agg: HashMap<String, GlobeCountryAgg>,
    home_country: &str,
) -> (Vec<GlobeCountryTraffic>, f64, f64, usize) {
    let total_pps: f64 = agg.values().map(|a| a.pps).sum();
    let mut rows: Vec<GlobeCountryTraffic> = agg
        .into_iter()
        .map(|(iso_code, a)| GlobeCountryTraffic {
            is_home: iso_code.eq_ignore_ascii_case(home_country),
            share_pct: if total_pps > 0.0 {
                (a.pps / total_pps) * 100.0
            } else {
                0.0
            },
            iso_code,
            pps: a.pps,
            bps: a.bps,
            get_peers_pps: a.get_peers_pps,
            announce_peer_pps: a.announce_peer_pps,
            response_pps: a.response_pps,
            find_node_ping_pps: a.find_node_ping_pps,
            other_pps: a.other_pps,
            cumulative_packets: a.cumulative_packets,
            cumulative_bytes: a.cumulative_bytes,
        })
        .collect();

    rows.sort_by(|a, b| {
        b.pps
            .total_cmp(&a.pps)
            .then_with(|| b.cumulative_packets.cmp(&a.cumulative_packets))
            .then_with(|| a.iso_code.cmp(&b.iso_code))
    });

    let active_country_count = rows.len();
    let mut other_pps = 0.0_f64;
    let mut other_bps = 0.0_f64;
    if rows.len() > GLOBE_MAX_COUNTRIES {
        for tail in rows.drain(GLOBE_MAX_COUNTRIES..) {
            other_pps += tail.pps;
            other_bps += tail.bps;
        }
    }
    (rows, other_pps, other_bps, active_country_count)
}

fn redis_val_to_str(val: &RedisValue) -> Option<String> {
    match val {
        RedisValue::BulkString(b) => Some(String::from_utf8_lossy(b).into_owned()),
        RedisValue::SimpleString(s) => Some(s.clone()),
        RedisValue::Int(n) => Some(n.to_string()),
        _ => None,
    }
}

fn redis_val_to_i64(val: &RedisValue) -> Option<i64> {
    match val {
        RedisValue::Int(n) => Some(*n),
        RedisValue::BulkString(b) => String::from_utf8_lossy(b).parse::<i64>().ok(),
        RedisValue::SimpleString(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn extract_group_metrics(val: RedisValue, target_group: &str) -> (i64, i64) {
    let RedisValue::Array(groups) = val else {
        return (0, 0);
    };
    for group in groups {
        match group {
            RedisValue::Array(kv) => {
                let mut name = String::new();
                let mut pending = 0_i64;
                let mut lag = 0_i64;
                let mut i = 0;
                while i + 1 < kv.len() {
                    if let Some(k) = redis_val_to_str(&kv[i]) {
                        match k.as_str() {
                            "name" => {
                                if let Some(v) = redis_val_to_str(&kv[i + 1]) {
                                    name = v;
                                }
                            }
                            "pending" => pending = redis_val_to_i64(&kv[i + 1]).unwrap_or(0),
                            "lag" => lag = redis_val_to_i64(&kv[i + 1]).unwrap_or(0),
                            _ => {}
                        }
                    }
                    i += 2;
                }
                if name == target_group {
                    return (pending, lag);
                }
            }
            RedisValue::Map(pairs) => {
                let mut name = String::new();
                let mut pending = 0_i64;
                let mut lag = 0_i64;
                for (k, v) in pairs {
                    if let Some(ks) = redis_val_to_str(&k) {
                        match ks.as_str() {
                            "name" => {
                                if let Some(s) = redis_val_to_str(&v) {
                                    name = s;
                                }
                            }
                            "pending" => pending = redis_val_to_i64(&v).unwrap_or(0),
                            "lag" => lag = redis_val_to_i64(&v).unwrap_or(0),
                            _ => {}
                        }
                    }
                }
                if name == target_group {
                    return (pending, lag);
                }
            }
            _ => {}
        }
    }
    (0, 0)
}

#[allow(dead_code)]
fn _ts_to_ymd(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_globe_geoip_status_reflects_local_mmdb_readiness_when_no_crawlers_running() {
        let cfg = GeoIpConfig::default();

        // 1. 磁盘已有有效 MMDB，但当前无 Crawler 运行：应标记 mmdb_file_ready = true 且无错误文案
        let ready_probe = LocalGeoIpProbe {
            valid: true,
            resolved_path: "/opt/hameta/data/GeoLite2-Country.mmdb".to_string(),
            size_bytes: 3_412_000,
            build_epoch: 1_763_686_800,
            database_type: "GeoLite2-Country".to_string(),
            error: None,
            fingerprint: None,
        };
        let ready_status = build_globe_geoip_status(&cfg, &ready_probe, &[], 0.0, 0.0);
        assert!(!ready_status.mmdb_loaded);
        assert!(ready_status.mmdb_file_ready);
        assert_eq!(ready_status.mmdb_size_bytes, 3_412_000);
        assert_eq!(ready_status.database_type, "GeoLite2-Country");
        assert!(ready_status.error.is_none());

        // 2. 磁盘无 MMDB 且无 Crawler：应标记 mmdb_file_ready = false 并给出缺失说明
        let missing_probe = LocalGeoIpProbe {
            valid: false,
            resolved_path: "/opt/hameta/data/GeoLite2-Country.mmdb".to_string(),
            error: Some("未找到有效的 GeoLite2-Country.mmdb 离线库".to_string()),
            ..Default::default()
        };
        let missing_status = build_globe_geoip_status(&cfg, &missing_probe, &[], 0.0, 0.0);
        assert!(!missing_status.mmdb_loaded);
        assert!(!missing_status.mmdb_file_ready);
        assert!(missing_status.error.as_deref().unwrap().contains("未找到"));

        // 3. 在设置中显式关闭 GeoIP
        let disabled_cfg = GeoIpConfig {
            enabled: false,
            ..cfg
        };
        let disabled_status = build_globe_geoip_status(&disabled_cfg, &ready_probe, &[], 0.0, 0.0);
        assert!(!disabled_status.enabled);
        assert!(!disabled_status.mmdb_loaded);
        assert!(disabled_status.error.as_deref().unwrap().contains("停用"));
    }

    #[test]
    fn indexer_sliding_window_prevents_zero_drops_between_5s_batch_flushes() {
        let mut hist = WorkerCounterHistory::default();
        let mut written_total = 1_000_u64;
        let mut today_sum = 0_u64;

        // 模拟 Indexer 每 5 秒批量落库 100 条（真实速率 1200/min），期间 4 秒增量为 0
        for sec in 0..=20_i64 {
            if sec > 0 && sec % 5 == 0 {
                written_total += 100;
            }
            let now_ms = 1_700_000_000_000 + sec * 1_000;
            let delta = hist.record_snapshot(
                (sec + 1) as u64,
                WorkerPrevTotals {
                    uptime_secs: 100 + sec as u64,
                    captured_at_ms: now_ms,
                    records_written: written_total,
                    ..Default::default()
                },
            );
            today_sum += delta;

            if sec >= 5 {
                let (base, cur, dt) = hist
                    .rate_window(now_ms, INDEXER_RATE_WINDOW_MS)
                    .expect("valid rate window");
                let per_min =
                    cur.records_written.saturating_sub(base.records_written) as f64 * (60.0 / dt);
                // 预热期内任何一秒（含未刷库的 6..9, 11..14 秒）都不应跳 0
                assert!(
                    per_min >= 600.0,
                    "sec={sec} dropped unexpectedly to {per_min}/min"
                );
                // 15s 滑窗满载后，恰好覆盖 3 个 5s 刷库周期，应稳定收敛至真实速率 1200/min
                if sec >= 15 {
                    assert!(
                        (per_min - 1200.0).abs() < 1e-6,
                        "sec={sec} expected steady 1200/min, got {per_min}/min"
                    );
                }
            }
        }
        // 今日累计计数必须精确等于实际新增 400 条，绝不重复叠加
        assert_eq!(today_sum, 400);
    }

    #[test]
    fn duplicate_snapshot_seq_carries_rate_and_decays_when_stale() {
        let mut hist = WorkerCounterHistory::default();
        hist.record_snapshot(
            1,
            WorkerPrevTotals {
                uptime_secs: 10,
                captured_at_ms: 1_000,
                queries_sent: 100,
                ..Default::default()
            },
        );
        hist.record_snapshot(
            2,
            WorkerPrevTotals {
                uptime_secs: 11,
                captured_at_ms: 2_000,
                queries_sent: 200,
                ..Default::default()
            },
        );
        // 同一 snapshot_seq=2 再次到来（HTTP 轮询相位抖动），不应归零
        let dup_delta = hist.record_snapshot(
            2,
            WorkerPrevTotals {
                uptime_secs: 11,
                captured_at_ms: 3_000,
                queries_sent: 200,
                ..Default::default()
            },
        );
        assert_eq!(dup_delta, 0);
        let (b, c, dt) = hist
            .rate_window(3_000, CRAWLER_RATE_WINDOW_MS)
            .expect("should carry rate on duplicate seq");
        let qps = (c.queries_sent - b.queries_sent) as f64 / dt;
        assert!((qps - 100.0).abs() < 0.1);

        // 超过 SNAPSHOT_STALE_HOLD_MS (6s) 仍无新快照则自然衰减返回 None (-> 0)
        assert!(hist.rate_window(9_000, CRAWLER_RATE_WINDOW_MS).is_none());
    }
}
