use crate::cache::{MetadataCache, MetadataCacheState};
use crate::redis_streams::{ClaimDecision, MetadataClaim, PublishOutcome, RedisStreamsClient};
use crate::stats::CrawlerAppStats;
use dht_crawler::prelude::{MetadataFetchCompletionStatus, TorrentInfo};
use dht_crawler::scheduler::MetadataAdmission;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SINK_BACKPRESSURE_DENOMINATOR: usize = 20;
const SINK_BACKPRESSURE_NUMERATOR: usize = 17; // 85%

pub struct MetadataHandler {
    redis: Arc<RedisStreamsClient>,
    cache: Arc<MetadataCache>,
    stats: Arc<CrawlerAppStats>,
    active_claims: Mutex<HashMap<String, MetadataClaim>>,
    hot_tx: async_channel::Sender<String>,
    sink_capacity: usize,
}

impl MetadataHandler {
    pub fn new(
        redis: Arc<RedisStreamsClient>,
        cache: Arc<MetadataCache>,
        stats: Arc<CrawlerAppStats>,
        hot_tx: async_channel::Sender<String>,
        sink_capacity: usize,
    ) -> Self {
        Self {
            redis,
            cache,
            stats,
            active_claims: Mutex::new(HashMap::new()),
            hot_tx,
            sink_capacity: sink_capacity.max(1),
        }
    }

    /// 高频入口同步粗过滤：未在本地缓存中明确命中已索引/占用的 Hash 才允许进入有界队列。
    pub fn should_accept_early(&self, info_hash: &str) -> bool {
        if self.has_active_claim(info_hash) {
            self.stats
                .early_filter_blocked
                .fetch_add(1, Ordering::Relaxed);
            self.stats
                .cache_hits_pending
                .fetch_add(1, Ordering::Relaxed);
            return false;
        }
        match self.cache.check_state(info_hash) {
            Some(MetadataCacheState::Indexed) => {
                self.stats
                    .cache_hits_indexed
                    .fetch_add(1, Ordering::Relaxed);
                self.stats
                    .early_filter_blocked
                    .fetch_add(1, Ordering::Relaxed);
                self.enqueue_hot(info_hash);
                false
            }
            Some(MetadataCacheState::Pending { .. }) => {
                self.stats
                    .cache_hits_pending
                    .fetch_add(1, Ordering::Relaxed);
                self.stats
                    .early_filter_blocked
                    .fetch_add(1, Ordering::Relaxed);
                false
            }
            Some(MetadataCacheState::PolicyRejected { .. }) => {
                self.stats
                    .cache_hits_rejected
                    .fetch_add(1, Ordering::Relaxed);
                self.stats
                    .early_filter_blocked
                    .fetch_add(1, Ordering::Relaxed);
                false
            }
            None => {
                self.stats
                    .early_filter_passed
                    .fetch_add(1, Ordering::Relaxed);
                true
            }
        }
    }

    pub async fn admit_metadata(&self, info_hash: &str) -> MetadataAdmission {
        if self.is_sink_backpressured() {
            self.stats.admission_defer.fetch_add(1, Ordering::Relaxed);
            return MetadataAdmission::RetryAfter(Duration::from_secs(2));
        }

        if self.has_active_claim(info_hash) {
            self.stats
                .cache_hits_pending
                .fetch_add(1, Ordering::Relaxed);
            self.stats.admission_defer.fetch_add(1, Ordering::Relaxed);
            return MetadataAdmission::RetryAfter(Duration::from_secs(5));
        }

        if let Some(state) = self.cache.check_state(info_hash) {
            return self.admission_from_cached(info_hash, state);
        }

        let initial_claim = MetadataClaim::new(false);
        {
            let mut guard = self
                .active_claims
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match guard.entry(info_hash.to_string()) {
                Entry::Vacant(slot) => {
                    slot.insert(initial_claim.clone());
                }
                Entry::Occupied(_) => {
                    self.stats
                        .cache_hits_pending
                        .fetch_add(1, Ordering::Relaxed);
                    self.stats.admission_defer.fetch_add(1, Ordering::Relaxed);
                    return MetadataAdmission::RetryAfter(Duration::from_secs(5));
                }
            }
        }

        match self
            .redis
            .claim_metadata(info_hash, initial_claim.clone())
            .await
        {
            Ok(ClaimDecision::Acquired(acquired)) => {
                let retained = {
                    let mut guard = self
                        .active_claims
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(cur) = guard.get_mut(info_hash)
                        && cur.token == acquired.token
                    {
                        *cur = acquired.clone();
                        true
                    } else {
                        false
                    }
                };
                if retained {
                    self.stats
                        .redis_claim_acquired
                        .fetch_add(1, Ordering::Relaxed);
                    self.stats.admission_fetch.fetch_add(1, Ordering::Relaxed);
                    MetadataAdmission::Fetch
                } else {
                    if let Err(err) = self
                        .redis
                        .release_claims(&[(info_hash.to_string(), acquired)])
                        .await
                    {
                        tracing::debug!("丢弃未启动租约失败: info_hash={info_hash}, error={err}");
                    }
                    self.stats.admission_defer.fetch_add(1, Ordering::Relaxed);
                    MetadataAdmission::RetryAfter(Duration::from_secs(5))
                }
            }
            Ok(ClaimDecision::AlreadyIndexed) => {
                self.finish_claim(info_hash, &initial_claim.token);
                self.cache.mark_indexed(info_hash);
                self.enqueue_hot(info_hash);
                self.stats
                    .redis_claim_indexed
                    .fetch_add(1, Ordering::Relaxed);
                self.stats.admission_skip.fetch_add(1, Ordering::Relaxed);
                MetadataAdmission::Skip
            }
            Ok(ClaimDecision::Pending { retry_after }) => {
                self.finish_claim(info_hash, &initial_claim.token);
                self.cache.mark_pending(info_hash, retry_after);
                self.stats
                    .redis_claim_pending
                    .fetch_add(1, Ordering::Relaxed);
                self.stats.admission_defer.fetch_add(1, Ordering::Relaxed);
                MetadataAdmission::RetryAfter(retry_after)
            }
            Ok(ClaimDecision::PolicyRejected { retry_after }) => {
                self.finish_claim(info_hash, &initial_claim.token);
                self.cache.mark_policy_rejected(info_hash, retry_after);
                self.stats
                    .redis_claim_rejected
                    .fetch_add(1, Ordering::Relaxed);
                self.stats.admission_skip.fetch_add(1, Ordering::Relaxed);
                MetadataAdmission::Skip
            }
            Err(err) => {
                // Redis 短暂故障时选择 Defer 退避重试，而非在不知道是否已索引时盲目并发拉取导致洪水。
                self.finish_claim(info_hash, &initial_claim.token);
                self.stats
                    .redis_claim_errors
                    .fetch_add(1, Ordering::Relaxed);
                self.stats.admission_defer.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("Redis 准入检查异常，已延迟重试 info_hash={info_hash}: {err}");
                MetadataAdmission::RetryAfter(Duration::from_secs(3))
            }
        }
    }

    pub async fn deliver_torrent(&self, torrent: TorrentInfo) -> bool {
        let Some(claim) = self.claim_for_delivery(&torrent.info_hash) else {
            self.stats
                .torrents_claim_lost
                .fetch_add(1, Ordering::Relaxed);
            return false;
        };

        match self.redis.push_torrent(&torrent, &claim).await {
            Ok(outcome) => {
                match outcome {
                    PublishOutcome::Submitted => {
                        self.stats.torrents_pushed.fetch_add(1, Ordering::Relaxed);
                        self.cache
                            .mark_pending(&torrent.info_hash, Duration::from_secs(30));
                    }
                    PublishOutcome::DuplicateDelivery => {
                        self.stats
                            .torrents_duplicate_delivery
                            .fetch_add(1, Ordering::Relaxed);
                        self.cache
                            .mark_pending(&torrent.info_hash, Duration::from_secs(30));
                    }
                    PublishOutcome::AlreadyIndexed => {
                        self.stats
                            .torrents_already_indexed
                            .fetch_add(1, Ordering::Relaxed);
                        self.cache.mark_indexed(&torrent.info_hash);
                        self.enqueue_hot(&torrent.info_hash);
                    }
                    PublishOutcome::ClaimLost => {
                        self.stats
                            .torrents_claim_lost
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    PublishOutcome::OversizeRejected => {
                        self.stats
                            .torrents_oversize_rejected
                            .fetch_add(1, Ordering::Relaxed);
                        self.cache
                            .mark_policy_rejected(&torrent.info_hash, Duration::from_secs(300));
                    }
                }
                self.finish_claim(&torrent.info_hash, &claim.token);
                true
            }
            Err(err) => {
                self.stats
                    .torrents_push_errors
                    .fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    "投递种子至 Redis Stream 失败: info_hash={}, error={err}",
                    torrent.info_hash
                );
                let _ = self
                    .defer_failure_for_claim(&torrent.info_hash, &claim)
                    .await;
                false
            }
        }
    }

    pub async fn handle_fetch_completion(
        &self,
        info_hash: &str,
        status: MetadataFetchCompletionStatus,
    ) {
        match status {
            MetadataFetchCompletionStatus::Accepted => {
                self.stats
                    .metadata_fetch_accepted
                    .fetch_add(1, Ordering::Relaxed);
            }
            MetadataFetchCompletionStatus::FetchFailed => {
                self.stats
                    .metadata_fetch_failed
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(claim) = self.claim_for_delivery(info_hash) {
                    let _ = self.defer_failure_for_claim(info_hash, &claim).await;
                }
            }
            MetadataFetchCompletionStatus::DeliveryRejected => {
                self.stats
                    .metadata_delivery_rejected
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(claim) = self.claim_for_delivery(info_hash) {
                    let _ = self.defer_failure_for_claim(info_hash, &claim).await;
                }
            }
        }
    }

    pub async fn release_remaining_claims(&self) -> anyhow::Result<usize> {
        let claims: Vec<_> = {
            let guard = self
                .active_claims
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.iter().map(|(h, c)| (h.clone(), c.clone())).collect()
        };
        let released = self.redis.release_claims(&claims).await?;
        for (hash, claim) in claims {
            self.finish_claim(&hash, &claim.token);
        }
        Ok(released)
    }

    fn admission_from_cached(
        &self,
        info_hash: &str,
        state: MetadataCacheState,
    ) -> MetadataAdmission {
        match state {
            MetadataCacheState::Indexed => {
                self.stats
                    .cache_hits_indexed
                    .fetch_add(1, Ordering::Relaxed);
                self.stats.admission_skip.fetch_add(1, Ordering::Relaxed);
                self.enqueue_hot(info_hash);
                MetadataAdmission::Skip
            }
            MetadataCacheState::Pending { retry_after, .. } => {
                self.stats
                    .cache_hits_pending
                    .fetch_add(1, Ordering::Relaxed);
                self.stats.admission_defer.fetch_add(1, Ordering::Relaxed);
                let rem = retry_after
                    .saturating_duration_since(std::time::Instant::now())
                    .max(Duration::from_secs(1));
                MetadataAdmission::RetryAfter(rem)
            }
            MetadataCacheState::PolicyRejected { .. } => {
                self.stats
                    .cache_hits_rejected
                    .fetch_add(1, Ordering::Relaxed);
                self.stats.admission_skip.fetch_add(1, Ordering::Relaxed);
                MetadataAdmission::Skip
            }
        }
    }

    fn is_sink_backpressured(&self) -> bool {
        let queued = self.stats.sink_queue_depth.load(Ordering::Relaxed);
        queued.saturating_mul(SINK_BACKPRESSURE_DENOMINATOR)
            >= self
                .sink_capacity
                .saturating_mul(SINK_BACKPRESSURE_NUMERATOR)
    }

    fn has_active_claim(&self, info_hash: &str) -> bool {
        self.active_claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(info_hash)
    }

    fn claim_for_delivery(&self, info_hash: &str) -> Option<MetadataClaim> {
        self.active_claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(info_hash)
            .cloned()
    }

    fn finish_claim(&self, info_hash: &str, token: &str) {
        let mut guard = self
            .active_claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.get(info_hash).is_some_and(|c| c.token == token) {
            guard.remove(info_hash);
        }
    }

    async fn defer_failure_for_claim(
        &self,
        info_hash: &str,
        claim: &MetadataClaim,
    ) -> anyhow::Result<()> {
        let res = self.redis.defer_failed_claim(info_hash, claim).await;
        if self
            .claim_for_delivery(info_hash)
            .is_some_and(|c| c.token == claim.token)
        {
            self.cache.mark_failure_cooldown(info_hash);
            self.finish_claim(info_hash, &claim.token);
        }
        res
    }

    fn enqueue_hot(&self, info_hash: &str) {
        if self.hot_tx.try_send(info_hash.to_string()).is_err() {
            self.stats.hot_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
