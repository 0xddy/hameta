use hameta_common::current_process_rss_bytes;
use hameta_filter::{FilterRuntime, FilterStatsSnapshot};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Debug, Default)]
pub struct IndexerAppStats {
    pub torrent_messages_read: AtomicU64,
    pub hot_messages_read: AtomicU64,
    pub malformed_messages_acked: AtomicU64,
    pub poison_messages_acked: AtomicU64,
    pub filter_accepted: AtomicU64,
    pub filter_rejected: AtomicU64,
    pub batches_written: AtomicU64,
    pub batches_failed: AtomicU64,
    pub records_written: AtomicU64,
    pub redis_indexed_marked: AtomicU64,
    pub redis_mark_errors: AtomicU64,
    pub stream_xacked_new: AtomicU64,
    pub stream_xacked_hot: AtomicU64,
    pub hot_records_updated: AtomicU64,
    pub hot_update_errors: AtomicU64,
    pub reclaimed_new: AtomicU64,
    pub reclaimed_hot: AtomicU64,
    pub torrent_buffered: AtomicUsize,
    pub torrent_in_flight: AtomicUsize,
    pub hot_buffered: AtomicUsize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerAppMetrics {
    pub torrent_messages_read: u64,
    pub hot_messages_read: u64,
    pub malformed_messages_acked: u64,
    pub poison_messages_acked: u64,
    pub filter_accepted: u64,
    pub filter_rejected: u64,
    pub batches_written: u64,
    pub batches_failed: u64,
    pub records_written: u64,
    pub redis_indexed_marked: u64,
    pub redis_mark_errors: u64,
    pub stream_xacked_new: u64,
    pub stream_xacked_hot: u64,
    pub hot_records_updated: u64,
    pub hot_update_errors: u64,
    pub reclaimed_new: u64,
    pub reclaimed_hot: u64,
    pub torrent_buffered: usize,
    pub torrent_in_flight: usize,
    pub hot_buffered: usize,
}

impl IndexerAppStats {
    pub fn snapshot(&self) -> IndexerAppMetrics {
        IndexerAppMetrics {
            torrent_messages_read: self.torrent_messages_read.load(Ordering::Relaxed),
            hot_messages_read: self.hot_messages_read.load(Ordering::Relaxed),
            malformed_messages_acked: self.malformed_messages_acked.load(Ordering::Relaxed),
            poison_messages_acked: self.poison_messages_acked.load(Ordering::Relaxed),
            filter_accepted: self.filter_accepted.load(Ordering::Relaxed),
            filter_rejected: self.filter_rejected.load(Ordering::Relaxed),
            batches_written: self.batches_written.load(Ordering::Relaxed),
            batches_failed: self.batches_failed.load(Ordering::Relaxed),
            records_written: self.records_written.load(Ordering::Relaxed),
            redis_indexed_marked: self.redis_indexed_marked.load(Ordering::Relaxed),
            redis_mark_errors: self.redis_mark_errors.load(Ordering::Relaxed),
            stream_xacked_new: self.stream_xacked_new.load(Ordering::Relaxed),
            stream_xacked_hot: self.stream_xacked_hot.load(Ordering::Relaxed),
            hot_records_updated: self.hot_records_updated.load(Ordering::Relaxed),
            hot_update_errors: self.hot_update_errors.load(Ordering::Relaxed),
            reclaimed_new: self.reclaimed_new.load(Ordering::Relaxed),
            reclaimed_hot: self.reclaimed_hot.load(Ordering::Relaxed),
            torrent_buffered: self.torrent_buffered.load(Ordering::Relaxed),
            torrent_in_flight: self.torrent_in_flight.load(Ordering::Relaxed),
            hot_buffered: self.hot_buffered.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerSnapshot {
    pub uptime_secs: u64,
    pub active_rule_version: i64,
    pub redis_pool_size: usize,
    pub redis_pool_available: usize,
    pub rss_bytes: u64,
    pub app: IndexerAppMetrics,
    pub filter: FilterStatsSnapshot,
}

pub fn build_indexer_snapshot(
    started_at: Instant,
    redis_pool_size: usize,
    redis_pool_available: usize,
    app: &IndexerAppStats,
    filter: &FilterRuntime,
) -> IndexerSnapshot {
    let filter_snap = filter.snapshot_stats(50);
    IndexerSnapshot {
        uptime_secs: started_at.elapsed().as_secs(),
        active_rule_version: filter_snap.active_version,
        redis_pool_size,
        redis_pool_available,
        rss_bytes: current_process_rss_bytes(),
        app: app.snapshot(),
        filter: filter_snap,
    }
}
