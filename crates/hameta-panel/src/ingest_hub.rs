use hameta_common::IngestEventItem;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

pub const INGEST_RING_CAPACITY: usize = 500;
const INGEST_BROADCAST_CAPACITY: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleIngestEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub item: IngestEventItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestHubStats {
    pub latest_seq: u64,
    pub ring_len: usize,
    pub ring_capacity: usize,
    pub subscribers: usize,
    pub total_published: u64,
}

/// 实时入库事件广播与环形回放中心（供 `/api/v1/console/ingest-stream` SSE 与 `Last-Event-ID` 断点续传使用）。
pub struct IngestHub {
    next_seq: AtomicU64,
    ring: Mutex<VecDeque<ConsoleIngestEvent>>,
    tx: broadcast::Sender<ConsoleIngestEvent>,
}

impl Default for IngestHub {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(INGEST_BROADCAST_CAPACITY);
        Self {
            next_seq: AtomicU64::new(0),
            ring: Mutex::new(VecDeque::with_capacity(INGEST_RING_CAPACITY)),
            tx,
        }
    }
}

impl IngestHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 原子分配单调递增 `seq`，写入 500 条有界环形缓存并同步按序推入广播通道。
    pub fn publish_batch(&self, items: Vec<IngestEventItem>) -> usize {
        if items.is_empty() {
            return 0;
        }
        let count = items.len();
        let mut guard = self
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for item in items {
            let seq = self
                .next_seq
                .fetch_add(1, Ordering::Relaxed)
                .saturating_add(1);
            let ev = ConsoleIngestEvent { seq, item };
            if guard.len() >= INGEST_RING_CAPACITY {
                guard.pop_front();
            }
            guard.push_back(ev.clone());
            let _ = self.tx.send(ev);
        }
        count
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConsoleIngestEvent> {
        self.tx.subscribe()
    }

    /// 按升序返回历史缓冲事件：`last_event_id` 存在时仅返回 `seq > since`；否则返回最新至多 `limit` 条。
    pub fn replay_after(
        &self,
        last_event_id: Option<u64>,
        limit: usize,
    ) -> Vec<ConsoleIngestEvent> {
        let cap = limit.min(INGEST_RING_CAPACITY);
        if cap == 0 {
            return Vec::new();
        }
        let guard = self
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match last_event_id {
            Some(since) => {
                let mut filtered: Vec<ConsoleIngestEvent> =
                    guard.iter().filter(|e| e.seq > since).cloned().collect();
                if filtered.len() > cap {
                    let drain = filtered.len() - cap;
                    filtered.drain(..drain);
                }
                filtered
            }
            None => {
                let skip = guard.len().saturating_sub(cap);
                guard.iter().skip(skip).cloned().collect()
            }
        }
    }

    pub fn stats(&self) -> IngestHubStats {
        let guard = self
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let latest_seq = self.next_seq.load(Ordering::Relaxed);
        IngestHubStats {
            latest_seq,
            ring_len: guard.len(),
            ring_capacity: INGEST_RING_CAPACITY,
            subscribers: self.tx.receiver_count(),
            total_published: latest_seq,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hameta_common::IngestEventType;

    fn sample_item(idx: usize) -> IngestEventItem {
        IngestEventItem {
            info_hash: format!("{idx:040x}"),
            event_type: IngestEventType::Indexed,
            worker_id: "indexer-1".to_string(),
            manticore_id: idx as i64 + 1,
            batch_id: 1,
            raw_name: format!("raw-{idx}"),
            refine_name: format!("clean-{idx}"),
            cleaned: true,
            total_size: 2048,
            file_count: 1,
            files_preview: Vec::new(),
            hot: 1,
            hot_delta: 1,
            latency_ms: 4,
            updated_at: 1_763_800_000,
        }
    }

    #[test]
    fn ring_truncates_and_replays_after_seq_without_gaps() {
        let hub = IngestHub::new();
        let mut rx = hub.subscribe();

        let batch: Vec<IngestEventItem> = (0..520).map(sample_item).collect();
        assert_eq!(hub.publish_batch(batch), 520);

        let st = hub.stats();
        assert_eq!(st.latest_seq, 520);
        assert_eq!(st.ring_len, INGEST_RING_CAPACITY);
        assert_eq!(st.ring_capacity, 500);
        assert_eq!(st.subscribers, 1);

        let recent = hub.replay_after(None, 5);
        let seqs: Vec<u64> = recent.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![516, 517, 518, 519, 520]);

        let resumed = hub.replay_after(Some(518), 50);
        let resumed_seqs: Vec<u64> = resumed.iter().map(|e| e.seq).collect();
        assert_eq!(resumed_seqs, vec![519, 520]);

        let first_broadcast = rx.try_recv().unwrap();
        assert_eq!(first_broadcast.seq, 1);
    }
}
