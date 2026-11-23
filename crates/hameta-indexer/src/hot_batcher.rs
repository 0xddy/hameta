use crate::ipc::IngestEventReporter;
use crate::manticore::ManticoreClient;
use crate::stats::IndexerAppStats;
use crate::streams::RedisStreamsConsumer;
use chrono::Utc;
use hameta_common::{IndexerWorkerConfig, IngestEventItem, IngestEventType, hash_to_id};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::interval;

#[derive(Debug, Clone)]
pub struct HotQueueItem {
    pub entry_id: String,
    pub info_hash: String,
}

/// 热点增量聚合批处理器，更新成功后批量对 `hot_stream` 执行 `XACK`。
pub struct HotBatcher {
    manticore: Arc<ManticoreClient>,
    streams: Arc<RedisStreamsConsumer>,
    config: IndexerWorkerConfig,
    stats: Arc<IndexerAppStats>,
    instance_id: String,
    ingest_reporter: IngestEventReporter,
    batch_seq: AtomicU64,
}

impl HotBatcher {
    pub fn new(
        manticore: Arc<ManticoreClient>,
        streams: Arc<RedisStreamsConsumer>,
        config: IndexerWorkerConfig,
        stats: Arc<IndexerAppStats>,
    ) -> Self {
        Self {
            manticore,
            streams,
            config,
            stats,
            instance_id: "standalone-indexer".to_string(),
            ingest_reporter: IngestEventReporter::disabled(),
            batch_seq: AtomicU64::new(0),
        }
    }

    pub fn with_ingest_reporter(
        mut self,
        instance_id: impl Into<String>,
        ingest_reporter: IngestEventReporter,
    ) -> Self {
        self.instance_id = instance_id.into();
        self.ingest_reporter = ingest_reporter;
        self
    }

    pub async fn run(self, mut rx: mpsc::Receiver<HotQueueItem>) {
        let batch_size = self.config.batch_size.max(1);
        let flush_dur = Duration::from_millis(self.config.flush_interval_ms.max(50));

        let mut counts: HashMap<String, i32> = HashMap::with_capacity(batch_size);
        let mut entry_ids: Vec<String> = Vec::with_capacity(batch_size);
        let mut last_flush = Instant::now();
        let mut ticker = interval(Duration::from_millis(500));

        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(item) => {
                            *counts.entry(item.info_hash).or_insert(0) += 1;
                            entry_ids.push(item.entry_id);
                            self.stats.hot_buffered.store(entry_ids.len(), Ordering::Relaxed);
                            if entry_ids.len() >= batch_size || counts.len() >= batch_size {
                                self.flush(&mut counts, &mut entry_ids).await;
                                last_flush = Instant::now();
                            }
                        }
                        None => {
                            if !entry_ids.is_empty() {
                                self.flush(&mut counts, &mut entry_ids).await;
                            }
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    if !entry_ids.is_empty() && last_flush.elapsed() >= flush_dur {
                        self.flush(&mut counts, &mut entry_ids).await;
                        last_flush = Instant::now();
                    }
                }
            }
        }
    }

    async fn flush(&self, counts: &mut HashMap<String, i32>, entry_ids: &mut Vec<String>) {
        if entry_ids.is_empty() {
            return;
        }
        let flush_started = Instant::now();
        let batch_id = self
            .batch_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        match self.manticore.batch_update_hot(counts).await {
            Ok(updated) => {
                let latency_ms =
                    u64::try_from(flush_started.elapsed().as_millis()).unwrap_or(u64::MAX);
                self.stats.hot_records_updated.fetch_add(
                    u64::try_from(updated).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
                if updated > 0 {
                    let now_ts = Utc::now().timestamp();
                    for (info_hash, delta) in counts.iter().take(32) {
                        let manticore_id = hash_to_id(info_hash);
                        if manticore_id <= 0 {
                            continue;
                        }
                        self.ingest_reporter.try_emit(IngestEventItem {
                            info_hash: info_hash.clone(),
                            event_type: IngestEventType::HotUpdated,
                            worker_id: self.instance_id.clone(),
                            manticore_id,
                            batch_id,
                            raw_name: String::new(),
                            refine_name: String::new(),
                            cleaned: false,
                            total_size: 0,
                            file_count: 0,
                            files_preview: Vec::new(),
                            hot: *delta,
                            hot_delta: *delta,
                            latency_ms,
                            updated_at: now_ts,
                        });
                    }
                }
                if let Err(err) = self
                    .streams
                    .xack_batch(self.streams.hot_stream(), entry_ids)
                    .await
                {
                    tracing::warn!("热点更新成功但批量 XACK 失败: {err}");
                } else {
                    self.stats.stream_xacked_hot.fetch_add(
                        u64::try_from(entry_ids.len()).unwrap_or(u64::MAX),
                        Ordering::Relaxed,
                    );
                }
            }
            Err(err) => {
                // 写库失败不 XACK，等待 PEL 接管或下轮重试
                self.stats.hot_update_errors.fetch_add(1, Ordering::Relaxed);
                tracing::error!("Manticore 热点批量更新失败（保留 PEL 未 XACK）: {err:#}");
            }
        }
        counts.clear();
        entry_ids.clear();
        self.stats.hot_buffered.store(0, Ordering::Relaxed);
    }
}
