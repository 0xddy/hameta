use crate::ipc::IngestEventReporter;
use crate::manticore::{ManticoreClient, writable_info_hashes};
use crate::stats::IndexerAppStats;
use crate::streams::RedisStreamsConsumer;
use hameta_common::{
    IndexerWorkerConfig, IngestEventItem, IngestEventType, TorrentRecord, hash_to_id,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::interval;

#[derive(Debug, Clone)]
pub struct AcceptedTorrentItem {
    pub entry_id: String,
    pub record: TorrentRecord,
}

/// 将过滤通过的种子攒批并发写入 Manticore，写库且置位 `metadata:indexed:*` 成功后才执行批量 `XACK`。
pub struct TorrentBatcher {
    manticore: Arc<ManticoreClient>,
    streams: Arc<RedisStreamsConsumer>,
    config: IndexerWorkerConfig,
    stats: Arc<IndexerAppStats>,
    instance_id: String,
    ingest_reporter: IngestEventReporter,
    batch_seq: Arc<AtomicU64>,
}

impl TorrentBatcher {
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
            batch_seq: Arc::new(AtomicU64::new(0)),
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

    pub async fn run(self, mut rx: mpsc::Receiver<AcceptedTorrentItem>) {
        let max_inflight = self.config.insert_workers.max(1);
        let sem = Arc::new(Semaphore::new(max_inflight));
        let batch_size = self.config.batch_size.max(1);
        let flush_dur = Duration::from_millis(self.config.flush_interval_ms.max(50));

        let mut buffer: Vec<AcceptedTorrentItem> = Vec::with_capacity(batch_size);
        let mut last_flush = Instant::now();
        let mut ticker = interval(Duration::from_millis(200));

        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(item) => {
                            buffer.push(item);
                            self.stats.torrent_buffered.fetch_add(1, Ordering::Relaxed);
                            if buffer.len() >= batch_size {
                                self.spawn_flush(&sem, &mut buffer).await;
                                last_flush = Instant::now();
                            }
                        }
                        None => {
                            self.spawn_flush(&sem, &mut buffer).await;
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    if !buffer.is_empty() && last_flush.elapsed() >= flush_dur {
                        self.spawn_flush(&sem, &mut buffer).await;
                        last_flush = Instant::now();
                    }
                }
            }
        }

        // 等待所有在途批量写库与 XACK 任务彻底排空
        if let Ok(permits) = sem
            .acquire_many(u32::try_from(max_inflight).unwrap_or(u32::MAX))
            .await
        {
            drop(permits);
        }
    }

    async fn spawn_flush(&self, sem: &Arc<Semaphore>, buffer: &mut Vec<AcceptedTorrentItem>) {
        if buffer.is_empty() {
            return;
        }
        let batch_capacity = self.config.batch_size.max(1);
        let batch = std::mem::replace(buffer, Vec::with_capacity(batch_capacity));
        let Ok(permit) = sem.clone().acquire_owned().await else {
            return;
        };

        let manticore = self.manticore.clone();
        let streams = self.streams.clone();
        let stats = self.stats.clone();
        let instance_id = self.instance_id.clone();
        let ingest_reporter = self.ingest_reporter.clone();
        let batch_seq = self.batch_seq.clone();

        tokio::spawn(async move {
            let _permit = permit;
            stats.torrent_in_flight.fetch_add(1, Ordering::Relaxed);
            Self::flush_one(
                &manticore,
                &streams,
                &stats,
                &instance_id,
                &ingest_reporter,
                &batch_seq,
                batch,
            )
            .await;
            stats.torrent_in_flight.fetch_sub(1, Ordering::Relaxed);
        });
    }

    async fn flush_one(
        manticore: &Arc<ManticoreClient>,
        streams: &Arc<RedisStreamsConsumer>,
        stats: &Arc<IndexerAppStats>,
        instance_id: &str,
        ingest_reporter: &IngestEventReporter,
        batch_seq: &AtomicU64,
        batch: Vec<AcceptedTorrentItem>,
    ) {
        let batch_len = batch.len();
        let mut records = Vec::with_capacity(batch_len);
        let mut entry_ids = Vec::with_capacity(batch_len);
        for item in batch {
            entry_ids.push(item.entry_id);
            records.push(item.record);
        }

        let writable_hashes = writable_info_hashes(&records);
        let flush_started = Instant::now();
        let batch_id = batch_seq.fetch_add(1, Ordering::Relaxed).saturating_add(1);

        match manticore.batch_insert(&records).await {
            Ok(written) => {
                let latency_ms =
                    u64::try_from(flush_started.elapsed().as_millis()).unwrap_or(u64::MAX);
                stats
                    .torrent_buffered
                    .fetch_sub(batch_len, Ordering::Relaxed);
                stats.batches_written.fetch_add(1, Ordering::Relaxed);
                stats.records_written.fetch_add(
                    u64::try_from(written).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );

                if written > 0 {
                    for record in records
                        .iter()
                        .filter(|r| hash_to_id(&r.info_hash) > 0)
                        .take(64)
                    {
                        let refine_name = record
                            .refine_name
                            .clone()
                            .unwrap_or_else(|| record.raw_name.clone());
                        let cleaned = refine_name != record.raw_name;
                        let total_size = record
                            .file_info
                            .iter()
                            .fold(0_u64, |acc, f| acc.saturating_add(f.size));
                        ingest_reporter.try_emit(IngestEventItem {
                            info_hash: record.info_hash.clone(),
                            event_type: IngestEventType::Indexed,
                            worker_id: instance_id.to_string(),
                            manticore_id: hash_to_id(&record.info_hash),
                            batch_id,
                            raw_name: record.raw_name.clone(),
                            refine_name,
                            cleaned,
                            total_size,
                            file_count: record.file_count,
                            files_preview: record.file_info.iter().take(20).cloned().collect(),
                            hot: record.hot,
                            hot_delta: 1,
                            latency_ms,
                            updated_at: record.updated_at.timestamp(),
                        });
                    }
                }

                if !writable_hashes.is_empty() {
                    match streams
                        .mark_indexed_and_clear_pending(&writable_hashes)
                        .await
                    {
                        Ok(marked) => {
                            stats.redis_indexed_marked.fetch_add(
                                u64::try_from(marked).unwrap_or(u64::MAX),
                                Ordering::Relaxed,
                            );
                        }
                        Err(err) => {
                            stats.redis_mark_errors.fetch_add(1, Ordering::Relaxed);
                            tracing::warn!(
                                "Manticore 写入已确认但更新 Redis metadata:indexed 标记失败: {err}"
                            );
                        }
                    }
                }

                if let Err(err) = streams
                    .xack_batch(streams.torrent_stream(), &entry_ids)
                    .await
                {
                    tracing::warn!("Manticore 写入成功但批量 XACK 失败: {err}");
                } else {
                    stats.stream_xacked_new.fetch_add(
                        u64::try_from(entry_ids.len()).unwrap_or(u64::MAX),
                        Ordering::Relaxed,
                    );
                }
            }
            Err(err) => {
                // 写库失败绝对不能执行 XACK，让消息保留在 PEL 等待后续接管重试
                stats
                    .torrent_buffered
                    .fetch_sub(batch_len, Ordering::Relaxed);
                stats.batches_failed.fetch_add(1, Ordering::Relaxed);
                if let Some(first) = records.first() {
                    tracing::error!(
                        "Manticore 批量写入失败（本批不执行 XACK，将保留于 PEL 重试）batch_len={}, sample_hash={}: {err:#}",
                        batch_len,
                        first.info_hash
                    );
                } else {
                    tracing::error!("Manticore 批量写入失败: {err:#}");
                }
            }
        }
    }
}
