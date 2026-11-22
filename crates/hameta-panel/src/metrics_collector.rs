use crate::services::ServiceRuntimePool;
use crate::supervisor::Supervisor;
use chrono::{TimeZone, Utc};
use hameta_common::{
    MetricsHourPoint, MetricsMinutePoint, WorkerRole, WorkerRuntimeState,
    current_soft_open_file_limit,
};
use hameta_indexer::TableStatusInfo;
use hameta_storage::Database;
use redis::Value as RedisValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio_util::sync::CancellationToken;

const RING_CAPACITY: usize = 600;
const MANTICORE_STATUS_TTL: Duration = Duration::from_secs(60);
const REDIS_PROBE_TTL: Duration = Duration::from_secs(3);

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
    pub metadata_rx_bps: f64,
    pub system_soft_nofile: Option<usize>,
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

struct CollectorInner {
    prev_workers: HashMap<String, WorkerPrevTotals>,
    ring: VecDeque<OverviewSnapshot>,
    minute_acc: Option<MinuteAccumulator>,
    hour_acc: Option<HourAccumulator>,
    last_redis_probe: Option<(Instant, RedisStreamProbe)>,
    last_manticore_status: Option<CachedManticoreStatus>,
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
    tx: broadcast::Sender<OverviewSnapshot>,
}

impl MetricsCollector {
    pub fn new(
        db: Database,
        supervisor: Arc<Supervisor>,
        services: Arc<ServiceRuntimePool>,
    ) -> Arc<Self> {
        let (tx, _) = broadcast::channel(32);
        Arc::new(Self {
            db,
            supervisor,
            services,
            inner: Mutex::new(CollectorInner::default()),
            latest: RwLock::new(None),
            tx,
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

    pub fn subscribe(&self) -> broadcast::Receiver<OverviewSnapshot> {
        self.tx.subscribe()
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

        // 分别探测 Redis（3s 缓存）与 Manticore（60s 缓存）
        let redis_probe = self.probe_redis_cached().await;
        let manticore_status = self.probe_manticore_cached(now_secs).await;

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
        let mut metadata_rx_bps = 0.0_f64;
        let mut total_rss_bytes = 0_i64;
        let mut indexed_written_delta_in_tick = 0.0_f64;
        let mut worker_briefs = Vec::with_capacity(workers.len());

        let mut guard = self.inner.lock().await;
        if guard.today_ymd != today_ymd {
            guard.today_ymd = today_ymd;
            guard.today_indexed_docs = 0;
        }

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
                let prev = guard.prev_workers.get(&w.id).cloned();

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
                        let meta_bytes =
                            json_u64(p, &["observability", "metadata_bytes_downloaded"]);

                        if let Some(prev_s) = prev
                            && uptime_secs >= prev_s.uptime_secs
                        {
                            let dt_secs =
                                ((now_ms - prev_s.captured_at_ms) as f64 / 1000.0).max(0.2);
                            let per_min_scale = 60.0 / dt_secs;
                            funnel.hashes_per_min += (hashes.saturating_sub(prev_s.hashes_received))
                                as f64
                                * per_min_scale;
                            funnel.dedup_blocked_per_min +=
                                (early_blocked.saturating_sub(prev_s.early_blocked)) as f64
                                    * per_min_scale;
                            funnel.metadata_succeeded_per_min +=
                                (meta_ok.saturating_sub(prev_s.metadata_succeeded)) as f64
                                    * per_min_scale;
                            funnel.stream_pushed_per_min +=
                                (pushed.saturating_sub(prev_s.torrents_pushed)) as f64
                                    * per_min_scale;
                            funnel.hot_pushed_per_min +=
                                (hot_pushed.saturating_sub(prev_s.hot_pushed)) as f64
                                    * per_min_scale;
                            actual_qps +=
                                (queries.saturating_sub(prev_s.queries_sent)) as f64 / dt_secs;
                            udp_rx_bps +=
                                (udp_rx.saturating_sub(prev_s.udp_rx_bytes)) as f64 / dt_secs;
                            udp_tx_bps +=
                                (udp_tx.saturating_sub(prev_s.udp_tx_bytes)) as f64 / dt_secs;
                            metadata_rx_bps +=
                                (meta_bytes.saturating_sub(prev_s.metadata_bytes)) as f64 / dt_secs;
                        }

                        guard.prev_workers.insert(
                            w.id.clone(),
                            WorkerPrevTotals {
                                uptime_secs,
                                captured_at_ms: now_ms,
                                hashes_received: hashes,
                                early_blocked,
                                metadata_succeeded: meta_ok,
                                torrents_pushed: pushed,
                                hot_pushed,
                                queries_sent: queries,
                                udp_rx_bytes: udp_rx,
                                udp_tx_bytes: udp_tx,
                                metadata_bytes: meta_bytes,
                                ..Default::default()
                            },
                        );
                    }
                    WorkerRole::Indexer => {
                        let accepted = json_u64(p, &["app", "filter_accepted"]);
                        let rejected = json_u64(p, &["app", "filter_rejected"]);
                        let written = json_u64(p, &["app", "records_written"]);
                        let hot_updated = json_u64(p, &["app", "hot_records_updated"]);

                        if let Some(prev_s) = prev
                            && uptime_secs >= prev_s.uptime_secs
                        {
                            let dt_secs =
                                ((now_ms - prev_s.captured_at_ms) as f64 / 1000.0).max(0.2);
                            let per_min_scale = 60.0 / dt_secs;
                            let delta_written = written.saturating_sub(prev_s.records_written);
                            indexed_written_delta_in_tick += delta_written as f64;
                            guard.today_indexed_docs = guard
                                .today_indexed_docs
                                .saturating_add(i64::try_from(delta_written).unwrap_or(0));

                            funnel.filter_accepted_per_min +=
                                (accepted.saturating_sub(prev_s.filter_accepted)) as f64
                                    * per_min_scale;
                            funnel.filter_rejected_per_min +=
                                (rejected.saturating_sub(prev_s.filter_rejected)) as f64
                                    * per_min_scale;
                            funnel.indexed_written_per_min += delta_written as f64 * per_min_scale;
                            funnel.hot_updated_per_min +=
                                (hot_updated.saturating_sub(prev_s.hot_records_updated)) as f64
                                    * per_min_scale;
                        }

                        guard.prev_workers.insert(
                            w.id.clone(),
                            WorkerPrevTotals {
                                uptime_secs,
                                captured_at_ms: now_ms,
                                filter_accepted: accepted,
                                filter_rejected: rejected,
                                records_written: written,
                                hot_records_updated: hot_updated,
                                ..Default::default()
                            },
                        );
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

        let _ = indexed_written_delta_in_tick;

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
                metadata_rx_bps,
                system_soft_nofile: current_soft_open_file_limit(),
            },
            funnel,
            workers: worker_briefs,
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
        let _ = self.tx.send(snapshot.clone());

        Ok(snapshot)
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
                    message: Some(format!("Manticore 查询失败: {err}")),
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
