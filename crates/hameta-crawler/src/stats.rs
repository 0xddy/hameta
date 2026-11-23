use crate::geoip::GeoTrafficSnapshot;
use dht_crawler::{
    DhtObservabilitySnapshot, DhtRuntimeSnapshot, DhtRuntimeStats, FixedHistogramSnapshot,
    MetadataConnectionErrorsSnapshot, MetadataTimeoutStagesSnapshot,
};
use hameta_common::current_process_rss_bytes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// Crawler 应用层原子统计计数器。
#[derive(Debug, Default)]
pub struct CrawlerAppStats {
    pub early_filter_passed: AtomicU64,
    pub early_filter_blocked: AtomicU64,
    pub admission_fetch: AtomicU64,
    pub admission_defer: AtomicU64,
    pub admission_skip: AtomicU64,
    pub cache_hits_indexed: AtomicU64,
    pub cache_hits_pending: AtomicU64,
    pub cache_hits_rejected: AtomicU64,
    pub redis_claim_acquired: AtomicU64,
    pub redis_claim_indexed: AtomicU64,
    pub redis_claim_pending: AtomicU64,
    pub redis_claim_rejected: AtomicU64,
    pub redis_claim_errors: AtomicU64,
    pub torrents_downloaded: AtomicU64,
    pub torrents_pushed: AtomicU64,
    pub torrents_duplicate_delivery: AtomicU64,
    pub torrents_already_indexed: AtomicU64,
    pub torrents_claim_lost: AtomicU64,
    pub torrents_oversize_rejected: AtomicU64,
    pub torrents_push_errors: AtomicU64,
    pub torrents_sink_dropped: AtomicU64,
    pub hot_pushed: AtomicU64,
    pub hot_push_errors: AtomicU64,
    pub hot_dropped: AtomicU64,
    pub metadata_fetch_accepted: AtomicU64,
    pub metadata_fetch_failed: AtomicU64,
    pub metadata_delivery_rejected: AtomicU64,
    pub dht_runtime_errors: AtomicU64,
    pub sink_queue_depth: AtomicUsize,
    pub sink_in_flight: AtomicUsize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatsSnapshot {
    pub early_filter_passed: u64,
    pub early_filter_blocked: u64,
    pub admission_fetch: u64,
    pub admission_defer: u64,
    pub admission_skip: u64,
    pub cache_hits_indexed: u64,
    pub cache_hits_pending: u64,
    pub cache_hits_rejected: u64,
    pub redis_claim_acquired: u64,
    pub redis_claim_indexed: u64,
    pub redis_claim_pending: u64,
    pub redis_claim_rejected: u64,
    pub redis_claim_errors: u64,
    pub torrents_downloaded: u64,
    pub torrents_pushed: u64,
    pub torrents_duplicate_delivery: u64,
    pub torrents_already_indexed: u64,
    pub torrents_claim_lost: u64,
    pub torrents_oversize_rejected: u64,
    pub torrents_push_errors: u64,
    pub torrents_sink_dropped: u64,
    pub hot_pushed: u64,
    pub hot_push_errors: u64,
    pub hot_dropped: u64,
    pub metadata_fetch_accepted: u64,
    pub metadata_fetch_failed: u64,
    pub metadata_delivery_rejected: u64,
    pub dht_runtime_errors: u64,
    pub sink_queue_depth: usize,
    pub sink_in_flight: usize,
}

impl CrawlerAppStats {
    pub fn snapshot(&self) -> AppStatsSnapshot {
        AppStatsSnapshot {
            early_filter_passed: self.early_filter_passed.load(Ordering::Relaxed),
            early_filter_blocked: self.early_filter_blocked.load(Ordering::Relaxed),
            admission_fetch: self.admission_fetch.load(Ordering::Relaxed),
            admission_defer: self.admission_defer.load(Ordering::Relaxed),
            admission_skip: self.admission_skip.load(Ordering::Relaxed),
            cache_hits_indexed: self.cache_hits_indexed.load(Ordering::Relaxed),
            cache_hits_pending: self.cache_hits_pending.load(Ordering::Relaxed),
            cache_hits_rejected: self.cache_hits_rejected.load(Ordering::Relaxed),
            redis_claim_acquired: self.redis_claim_acquired.load(Ordering::Relaxed),
            redis_claim_indexed: self.redis_claim_indexed.load(Ordering::Relaxed),
            redis_claim_pending: self.redis_claim_pending.load(Ordering::Relaxed),
            redis_claim_rejected: self.redis_claim_rejected.load(Ordering::Relaxed),
            redis_claim_errors: self.redis_claim_errors.load(Ordering::Relaxed),
            torrents_downloaded: self.torrents_downloaded.load(Ordering::Relaxed),
            torrents_pushed: self.torrents_pushed.load(Ordering::Relaxed),
            torrents_duplicate_delivery: self.torrents_duplicate_delivery.load(Ordering::Relaxed),
            torrents_already_indexed: self.torrents_already_indexed.load(Ordering::Relaxed),
            torrents_claim_lost: self.torrents_claim_lost.load(Ordering::Relaxed),
            torrents_oversize_rejected: self.torrents_oversize_rejected.load(Ordering::Relaxed),
            torrents_push_errors: self.torrents_push_errors.load(Ordering::Relaxed),
            torrents_sink_dropped: self.torrents_sink_dropped.load(Ordering::Relaxed),
            hot_pushed: self.hot_pushed.load(Ordering::Relaxed),
            hot_push_errors: self.hot_push_errors.load(Ordering::Relaxed),
            hot_dropped: self.hot_dropped.load(Ordering::Relaxed),
            metadata_fetch_accepted: self.metadata_fetch_accepted.load(Ordering::Relaxed),
            metadata_fetch_failed: self.metadata_fetch_failed.load(Ordering::Relaxed),
            metadata_delivery_rejected: self.metadata_delivery_rejected.load(Ordering::Relaxed),
            dht_runtime_errors: self.dht_runtime_errors.load(Ordering::Relaxed),
            sink_queue_depth: self.sink_queue_depth.load(Ordering::Relaxed),
            sink_in_flight: self.sink_in_flight.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramMetrics {
    pub bounds: Vec<u64>,
    pub counts: Vec<u64>,
    pub overflow: u64,
    pub count: u64,
    pub sum: u64,
    pub p50: Option<u64>,
    pub p95: Option<u64>,
}

impl From<FixedHistogramSnapshot> for HistogramMetrics {
    fn from(s: FixedHistogramSnapshot) -> Self {
        let p50 = s.percentile(0.50);
        let p95 = s.percentile(0.95);
        Self {
            bounds: s.bounds,
            counts: s.counts,
            overflow: s.overflow,
            count: s.count,
            sum: s.sum,
            p50,
            p95,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutStagesMetrics {
    pub connect_handshake: u64,
    pub tcp_connect: u64,
    pub base_handshake: u64,
    pub extension_handshake_send: u64,
    pub extension_handshake_receive: u64,
    pub piece_request_send: u64,
    pub piece_receive: u64,
    pub validation: u64,
}

impl From<MetadataTimeoutStagesSnapshot> for TimeoutStagesMetrics {
    fn from(s: MetadataTimeoutStagesSnapshot) -> Self {
        Self {
            connect_handshake: s.connect_handshake,
            tcp_connect: s.tcp_connect,
            base_handshake: s.base_handshake,
            extension_handshake_send: s.extension_handshake_send,
            extension_handshake_receive: s.extension_handshake_receive,
            piece_request_send: s.piece_request_send,
            piece_receive: s.piece_receive,
            validation: s.validation,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionErrorsMetrics {
    pub refused: u64,
    pub reset: u64,
    pub aborted: u64,
    pub closed: u64,
    pub timeout: u64,
    pub invalid_handshake: u64,
    pub info_hash_mismatch: u64,
    pub io_other: u64,
    pub other: u64,
}

impl From<MetadataConnectionErrorsSnapshot> for ConnectionErrorsMetrics {
    fn from(s: MetadataConnectionErrorsSnapshot) -> Self {
        Self {
            refused: s.refused,
            reset: s.reset,
            aborted: s.aborted,
            closed: s.closed,
            timeout: s.timeout,
            invalid_handshake: s.invalid_handshake,
            info_hash_mismatch: s.info_hash_mismatch,
            io_other: s.io_other,
            other: s.other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhtRuntimeMetrics {
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
    pub metadata_races_started: u64,
    pub metadata_peer_candidates: u64,
    pub metadata_live_peer_joins: u64,
    pub metadata_peer_canceled: u64,
    pub peer_lookup_requested: u64,
    pub peer_lookup_started: u64,
    pub peer_lookup_rate_limited: u64,
    pub peer_lookup_empty: u64,
    pub peer_lookup_queries: u64,
    pub peer_lookup_responses: u64,
    pub peer_lookup_timeouts: u64,
    pub peer_lookup_send_failures: u64,
    pub peer_lookup_peers_found: u64,
    pub metadata_peer_attempts: u64,
    pub metadata_peer_succeeded: u64,
    pub metadata_peer_failed: u64,
    pub metadata_peer_timeouts: u64,
    pub metadata_connect_failed: u64,
    pub metadata_no_extension: u64,
    pub metadata_peer_failure_cache_hits: u64,
    pub metadata_peer_failure_cache_entries: usize,
    pub node_pool_size: usize,
    pub node_pool_capacity: usize,
    pub node_pool_low_watermark: usize,
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
    pub udp_received: u64,
    pub udp_queue_full: u64,
    pub udp_invalid: u64,
    pub udp_responses_rate_limited: u64,
    pub udp_responses_priority_reserved: u64,
}

impl From<DhtRuntimeSnapshot> for DhtRuntimeMetrics {
    fn from(s: DhtRuntimeSnapshot) -> Self {
        Self {
            hashes_received: s.hashes_received,
            hash_ingress_dropped: s.hash_ingress_dropped,
            hash_ingress_queue_depth: s.hash_ingress_queue_depth,
            hash_ingress_queue_capacity: s.hash_ingress_queue_capacity,
            crawl_priority_queue_depth: s.crawl_priority_queue_depth,
            crawl_priority_queue_capacity: s.crawl_priority_queue_capacity,
            crawl_discovery_queue_depth: s.crawl_discovery_queue_depth,
            crawl_discovery_queue_capacity: s.crawl_discovery_queue_capacity,
            metadata_queue_depth: s.metadata_queue_depth,
            metadata_queue_max: s.metadata_queue_max,
            metadata_in_flight: s.metadata_in_flight,
            metadata_queue_inserted: s.metadata_queue_inserted,
            metadata_queue_deduplicated: s.metadata_queue_deduplicated,
            metadata_queue_evicted: s.metadata_queue_evicted,
            metadata_queue_stale: s.metadata_queue_stale,
            metadata_races_started: s.metadata_races_started,
            metadata_peer_candidates: s.metadata_peer_candidates,
            metadata_live_peer_joins: s.metadata_live_peer_joins,
            metadata_peer_canceled: s.metadata_peer_canceled,
            peer_lookup_requested: s.peer_lookup_requested,
            peer_lookup_started: s.peer_lookup_started,
            peer_lookup_rate_limited: s.peer_lookup_rate_limited,
            peer_lookup_empty: s.peer_lookup_empty,
            peer_lookup_queries: s.peer_lookup_queries,
            peer_lookup_responses: s.peer_lookup_responses,
            peer_lookup_timeouts: s.peer_lookup_timeouts,
            peer_lookup_send_failures: s.peer_lookup_send_failures,
            peer_lookup_peers_found: s.peer_lookup_peers_found,
            metadata_peer_attempts: s.metadata_peer_attempts,
            metadata_peer_succeeded: s.metadata_peer_succeeded,
            metadata_peer_failed: s.metadata_peer_failed,
            metadata_peer_timeouts: s.metadata_peer_timeouts,
            metadata_connect_failed: s.metadata_connect_failed,
            metadata_no_extension: s.metadata_no_extension,
            metadata_peer_failure_cache_hits: s.metadata_peer_failure_cache_hits,
            metadata_peer_failure_cache_entries: s.metadata_peer_failure_cache_entries,
            node_pool_size: s.node_pool_size,
            node_pool_capacity: s.node_pool_capacity,
            node_pool_low_watermark: s.node_pool_low_watermark,
            find_node_in_flight: s.find_node_in_flight,
            find_node_in_flight_max: s.find_node_in_flight_max,
            find_node_effective_rate_per_sec: s.find_node_effective_rate_per_sec,
            queries_new: s.queries_new,
            queries_revisit: s.queries_revisit,
            queries_bootstrap: s.queries_bootstrap,
            responses: s.responses,
            unmatched_responses: s.unmatched_responses,
            timeouts: s.timeouts,
            send_failures: s.send_failures,
            udp_received: s.udp_received,
            udp_queue_full: s.udp_queue_full,
            udp_invalid: s.udp_invalid,
            udp_responses_rate_limited: s.udp_responses_rate_limited,
            udp_responses_priority_reserved: s.udp_responses_priority_reserved,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhtObservabilityMetrics {
    pub udp_rx_packets: u64,
    #[serde(default)]
    pub udp_rx_ipv4_packets: u64,
    #[serde(default)]
    pub udp_rx_ipv6_packets: u64,
    pub udp_rx_bytes: u64,
    pub udp_tx_packets: u64,
    pub udp_tx_bytes: u64,
    pub inbound_ping: u64,
    pub inbound_find_node: u64,
    pub inbound_get_peers: u64,
    pub inbound_announce_peer: u64,
    pub inbound_other: u64,
    #[serde(default)]
    pub inbound_response: u64,
    #[serde(default)]
    pub inbound_error: u64,
    #[serde(default)]
    pub inbound_parse_error: u64,
    pub response_normal: u64,
    pub response_fallback: u64,
    pub response_rate_limited: u64,
    pub response_send_failed: u64,
    pub announce_accepted: u64,
    pub announce_invalid_token: u64,
    pub announce_filtered: u64,
    pub node_admitted: u64,
    pub node_replaced: u64,
    pub node_dropped_duplicate: u64,
    pub node_dropped_rate_limited: u64,
    pub node_dropped_invalid: u64,
    pub metadata_bytes_downloaded: u64,
    pub metadata_failure_timeout: u64,
    pub metadata_failure_connect: u64,
    pub metadata_timeout_stages: TimeoutStagesMetrics,
    pub metadata_connection_errors: ConnectionErrorsMetrics,
    pub metadata_failure_no_extension: u64,
    pub metadata_failure_send: u64,
    pub metadata_failure_size_limit: u64,
    pub metadata_failure_sha1: u64,
    pub metadata_failure_parse: u64,
    pub metadata_failure_peer_closed: u64,
    pub metadata_failure_protocol: u64,
    pub metadata_failure_other: u64,
    pub metadata_failure_reject: u64,
    pub metadata_failure_tcp_connect: u64,
    pub metadata_failure_base_handshake: u64,
    pub peer_cache_timeout_hits: u64,
    pub peer_cache_connect_hits: u64,
    pub peer_cache_endpoint_hits: u64,
    pub peer_cache_metadata_hits: u64,
    pub queue_wait_ms: HistogramMetrics,
    pub fetch_duration_ms: HistogramMetrics,
    pub metadata_size_bytes: HistogramMetrics,
}

impl From<DhtObservabilitySnapshot> for DhtObservabilityMetrics {
    fn from(s: DhtObservabilitySnapshot) -> Self {
        Self {
            udp_rx_packets: s.udp_rx_packets,
            udp_rx_ipv4_packets: s.udp_rx_ipv4_packets,
            udp_rx_ipv6_packets: s.udp_rx_ipv6_packets,
            udp_rx_bytes: s.udp_rx_bytes,
            udp_tx_packets: s.udp_tx_packets,
            udp_tx_bytes: s.udp_tx_bytes,
            inbound_ping: s.inbound_ping,
            inbound_find_node: s.inbound_find_node,
            inbound_get_peers: s.inbound_get_peers,
            inbound_announce_peer: s.inbound_announce_peer,
            inbound_other: s.inbound_other,
            inbound_response: s.inbound_response,
            inbound_error: s.inbound_error,
            inbound_parse_error: s.inbound_parse_error,
            response_normal: s.response_normal,
            response_fallback: s.response_fallback,
            response_rate_limited: s.response_rate_limited,
            response_send_failed: s.response_send_failed,
            announce_accepted: s.announce_accepted,
            announce_invalid_token: s.announce_invalid_token,
            announce_filtered: s.announce_filtered,
            node_admitted: s.node_admitted,
            node_replaced: s.node_replaced,
            node_dropped_duplicate: s.node_dropped_duplicate,
            node_dropped_rate_limited: s.node_dropped_rate_limited,
            node_dropped_invalid: s.node_dropped_invalid,
            metadata_bytes_downloaded: s.metadata_bytes_downloaded,
            metadata_failure_timeout: s.metadata_failure_timeout,
            metadata_failure_connect: s.metadata_failure_connect,
            metadata_timeout_stages: s.metadata_timeout_stages.into(),
            metadata_connection_errors: s.metadata_connection_errors.into(),
            metadata_failure_no_extension: s.metadata_failure_no_extension,
            metadata_failure_send: s.metadata_failure_send,
            metadata_failure_size_limit: s.metadata_failure_size_limit,
            metadata_failure_sha1: s.metadata_failure_sha1,
            metadata_failure_parse: s.metadata_failure_parse,
            metadata_failure_peer_closed: s.metadata_failure_peer_closed,
            metadata_failure_protocol: s.metadata_failure_protocol,
            metadata_failure_other: s.metadata_failure_other,
            metadata_failure_reject: s.metadata_failure_reject,
            metadata_failure_tcp_connect: s.metadata_failure_tcp_connect,
            metadata_failure_base_handshake: s.metadata_failure_base_handshake,
            peer_cache_timeout_hits: s.peer_cache_timeout_hits,
            peer_cache_connect_hits: s.peer_cache_connect_hits,
            peer_cache_endpoint_hits: s.peer_cache_endpoint_hits,
            peer_cache_metadata_hits: s.peer_cache_metadata_hits,
            queue_wait_ms: s.queue_wait_ms.into(),
            fetch_duration_ms: s.fetch_duration_ms.into(),
            metadata_size_bytes: s.metadata_size_bytes.into(),
        }
    }
}

/// 采集节点通过 Loopback IPC 暴露给 Panel 的全量运行时快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerSnapshot {
    pub uptime_secs: u64,
    pub crawl_profile: String,
    pub effective_metadata_workers: usize,
    pub local_cache_entries: u64,
    pub redis_pool_size: usize,
    pub redis_pool_available: usize,
    pub rss_bytes: u64,
    pub app: AppStatsSnapshot,
    pub dht: DhtRuntimeMetrics,
    pub observability: DhtObservabilityMetrics,
    #[serde(default)]
    pub geo: GeoTrafficSnapshot,
    pub yield_counters: BTreeMap<String, u64>,
    pub yield_gauges: BTreeMap<String, u64>,
}

pub struct CrawlerSnapshotInput<'a> {
    pub started_at: Instant,
    pub crawl_profile: &'a str,
    pub effective_metadata_workers: usize,
    pub local_cache_entries: u64,
    pub redis_pool_size: usize,
    pub redis_pool_available: usize,
    pub app_stats: &'a CrawlerAppStats,
    pub dht_stats: &'a DhtRuntimeStats,
    pub geo_snapshot: GeoTrafficSnapshot,
}

pub fn build_crawler_snapshot(input: CrawlerSnapshotInput<'_>) -> CrawlerSnapshot {
    CrawlerSnapshot {
        uptime_secs: input.started_at.elapsed().as_secs(),
        crawl_profile: input.crawl_profile.to_string(),
        effective_metadata_workers: input.effective_metadata_workers,
        local_cache_entries: input.local_cache_entries,
        redis_pool_size: input.redis_pool_size,
        redis_pool_available: input.redis_pool_available,
        rss_bytes: current_process_rss_bytes(),
        app: input.app_stats.snapshot(),
        dht: input.dht_stats.snapshot().into(),
        observability: input.dht_stats.observability_snapshot().into(),
        geo: input.geo_snapshot,
        yield_counters: input.dht_stats.yield_stats().snapshot(),
        yield_gauges: input.dht_stats.yield_stats().gauges_snapshot(),
    }
}
