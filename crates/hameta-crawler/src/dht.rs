use crate::rlimit::{clamp_metadata_workers, effective_metadata_worker_count};
use anyhow::Result;
use dht_crawler::prelude::*;
use hameta_common::{
    CrawlProfileDefaults, CrawlerWorkerConfig, NetModeConfig, crawl_profile_defaults,
};

/// 按 L1（档位与端口）-> L2（抓取/发现/PeerLookup）-> L3（引擎细节覆写）顺序组装 `DHTOptions`，
/// 并基于系统当前 `RLIMIT_NOFILE` 自动收敛 `max_worker_count`。
pub fn build_dht_options(
    config: &CrawlerWorkerConfig,
    reserved_fd_count: usize,
) -> Result<DHTOptions> {
    let defaults = crawl_profile_defaults(&config.crawl_profile)
        .ok_or_else(|| anyhow::anyhow!("未知的采集档位: {}", config.crawl_profile))?;
    let max_worker_count =
        effective_metadata_worker_count(config.metadata.max_worker_count, reserved_fd_count);
    Ok(assemble_dht_options(config, defaults, max_worker_count))
}

/// 纯函数版本：支持注入显式 FD 上限以用于确定性单元测试。
pub fn build_dht_options_with_fd_limit(
    config: &CrawlerWorkerConfig,
    reserved_fd_count: usize,
    open_file_limit: Option<usize>,
) -> Result<DHTOptions> {
    let defaults = crawl_profile_defaults(&config.crawl_profile)
        .ok_or_else(|| anyhow::anyhow!("未知的采集档位: {}", config.crawl_profile))?;
    let max_worker_count = clamp_metadata_workers(
        config.metadata.max_worker_count,
        reserved_fd_count,
        open_file_limit,
    );
    Ok(assemble_dht_options(config, defaults, max_worker_count))
}

fn assemble_dht_options(
    config: &CrawlerWorkerConfig,
    defaults: CrawlProfileDefaults,
    max_worker_count: usize,
) -> DHTOptions {
    let netmode = match config.netmode {
        NetModeConfig::Ipv4Only => NetMode::Ipv4Only,
        NetModeConfig::Ipv6Only => NetMode::Ipv6Only,
        NetModeConfig::Dual => NetMode::DualStack,
    };

    let metadata = MetadataOptions {
        timeout_secs: config.metadata.timeout_secs,
        connect_timeout_ms: config.metadata.connect_timeout_ms,
        max_queue_size: config.metadata.max_queue_size,
        max_worker_count,
        peer_failure_cache_capacity: config.metadata.peer_failure_cache_capacity,
        peer_failure_ttl_secs: config.metadata.peer_failure_ttl_secs,
        retry_initial_delay_secs: config.metadata.retry_initial_delay_secs,
        retry_max_delay_secs: config.metadata.retry_max_delay_secs,
        retry_max_attempts: config.metadata.retry_max_attempts,
        retry_history_ttl_secs: config.metadata.retry_history_ttl_secs,
        retry_history_capacity: config.metadata.retry_history_capacity,
    };

    let discovery = DiscoveryOptions {
        enabled: config.discovery.enabled,
        sample_queries_per_second: config.discovery.sample_queries_per_second,
        sample_burst: config.discovery.sample_burst,
        sample_max_in_flight: config.discovery.sample_max_in_flight,
        sample_query_timeout_secs: config.discovery.sample_query_timeout_secs,
        sample_min_interval_secs: config.discovery.sample_min_interval_secs,
        sample_failure_backoff_secs: config.discovery.sample_failure_backoff_secs,
        sample_no_new_backoff_secs: config.discovery.sample_no_new_backoff_secs,
        sample_max_nodes: config.discovery.sample_max_nodes,
        sample_cooldown_capacity: config.discovery.sample_cooldown_capacity,
        hash_dedup_capacity: config.discovery.hash_dedup_capacity,
        hash_dedup_ttl_secs: config.discovery.hash_dedup_ttl_secs,
        get_peers_hashes_per_second: config.discovery.get_peers_hashes_per_second,
        get_peers_burst: config.discovery.get_peers_burst,
        sample_hashes_per_second: config.discovery.sample_hashes_per_second,
        sample_hash_burst: config.discovery.sample_hash_burst,
    };

    let lookup_default = PeerLookupOptions::default();
    let peer_lookup = PeerLookupOptions {
        max_lookups_per_second: config
            .peer_lookup
            .max_lookups_per_second
            .unwrap_or(defaults.peer_lookup_rate_per_sec),
        burst: config
            .peer_lookup
            .burst
            .unwrap_or(defaults.peer_lookup_burst),
        max_active_lookups: config
            .peer_lookup
            .max_active_lookups
            .unwrap_or(defaults.peer_lookup_max_active),
        max_queries: config
            .peer_lookup
            .max_queries
            .unwrap_or(lookup_default.max_queries),
        max_concurrent_queries: config
            .peer_lookup
            .max_concurrent_queries
            .unwrap_or(lookup_default.max_concurrent_queries),
        lookup_timeout_ms: config
            .peer_lookup
            .lookup_timeout_ms
            .unwrap_or(lookup_default.lookup_timeout_ms),
        query_timeout_ms: config
            .peer_lookup
            .query_timeout_ms
            .unwrap_or(lookup_default.query_timeout_ms),
    };

    let mut crawl = CrawlOptions::default();
    crawl.rate_limit.max_find_node_rate_per_sec = config
        .crawl
        .rate_limit
        .max_find_node_rate_per_sec
        .unwrap_or(defaults.find_node_rate_per_sec);
    crawl.rate_limit.burst = config
        .crawl
        .rate_limit
        .burst
        .unwrap_or(defaults.find_node_burst);
    crawl.rate_limit.max_in_flight = config
        .crawl
        .rate_limit
        .max_in_flight
        .unwrap_or(defaults.find_node_max_in_flight);
    crawl.rate_limit.request_timeout_secs =
        config.crawl.rate_limit.request_timeout_secs.unwrap_or(2);
    crawl.rate_limit.max_new_destinations_per_minute = config
        .crawl
        .rate_limit
        .max_new_destinations_per_minute
        .unwrap_or(defaults.max_new_destinations_per_minute);
    crawl.rate_limit.max_response_rate_per_sec = config
        .crawl
        .rate_limit
        .max_response_rate_per_sec
        .unwrap_or(defaults.max_response_rate_per_sec);
    crawl.rate_limit.max_response_bytes_per_sec = config
        .crawl
        .rate_limit
        .max_response_bytes_per_sec
        .unwrap_or(defaults.max_response_bytes_per_sec);
    crawl.rate_limit.max_response_rate_per_source = config
        .crawl
        .rate_limit
        .max_response_rate_per_source
        .unwrap_or(defaults.max_response_rate_per_source);
    crawl.rate_limit.metadata_pressure_floor_percent = config
        .crawl
        .rate_limit
        .metadata_pressure_floor_percent
        .unwrap_or(25);
    crawl.rate_limit.max_replacements_per_minute = config
        .crawl
        .rate_limit
        .max_replacements_per_minute
        .unwrap_or(defaults.max_replacements_per_minute);
    crawl.rate_limit.max_in_flight_per_subnet = config
        .crawl
        .rate_limit
        .max_in_flight_per_subnet
        .unwrap_or(defaults.max_in_flight_per_subnet);

    crawl.pool.capacity = config.crawl.pool.capacity.unwrap_or(defaults.pool_capacity);
    if let Some(value) = config.crawl.pool.recent_probe_ttl_secs {
        crawl.pool.recent_probe_ttl_secs = value;
    }
    crawl.pool.responsive_capacity = config
        .crawl
        .pool
        .responsive_capacity
        .unwrap_or(defaults.responsive_capacity);
    if let Some(value) = config.crawl.pool.responsive_ttl_secs {
        crawl.pool.responsive_ttl_secs = value;
    }
    if let Some(value) = config.crawl.pool.revisit_interval_secs {
        crawl.pool.revisit_interval_secs = value;
    }
    if let Some(value) = config.crawl.pool.low_watermark {
        crawl.pool.low_watermark = value;
    }

    if let Some(nodes) = &config.crawl.bootstrap.nodes {
        crawl.bootstrap.nodes = nodes.clone();
    }
    if let Some(value) = config.crawl.bootstrap.interval_secs {
        crawl.bootstrap.interval_secs = value;
    }
    if let Some(value) = config.crawl.bootstrap.max_nodes_per_round {
        crawl.bootstrap.max_nodes_per_round = value;
    }
    if let Some(value) = config.crawl.bootstrap.source_backoff_base_secs {
        crawl.bootstrap.source_backoff_base_secs = value;
    }
    if let Some(value) = config.crawl.bootstrap.source_backoff_max_secs {
        crawl.bootstrap.source_backoff_max_secs = value;
    }

    if let Some(value) = config.crawl.target.random_walk_percent {
        crawl.target.random_walk_percent = value;
    }
    if let Some(value) = config.crawl.target.sparse_bucket_percent {
        crawl.target.sparse_bucket_percent = value;
    }
    if let Some(value) = config.crawl.target.neighbor_sender_id {
        crawl.target.neighbor_sender_id = value;
    }

    if let Some(value) = config.crawl.scheduler.priority_event_channel_capacity {
        crawl.scheduler.priority_event_channel_capacity = value;
    }
    if let Some(value) = config.crawl.scheduler.discovery_event_channel_capacity {
        crawl.scheduler.discovery_event_channel_capacity = value;
    }
    if let Some(value) = config.crawl.scheduler.event_batch_limit {
        crawl.scheduler.event_batch_limit = value;
    }
    if let Some(value) = config.crawl.scheduler.node_batch_limit {
        crawl.scheduler.node_batch_limit = value;
    }
    crawl.scheduler.routing_snapshot_size = config
        .crawl
        .scheduler
        .routing_snapshot_size
        .unwrap_or(defaults.routing_snapshot_size);
    if let Some(value) = config.crawl.scheduler.snapshot_refresh_millis {
        crawl.scheduler.snapshot_refresh_millis = value;
    }

    DHTOptions {
        port: config.port,
        netmode,
        hash_queue_capacity: config.hash_queue_capacity,
        metadata,
        peer_lookup,
        discovery,
        crawl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hameta_common::{CrawlerRateLimitConfig, CrawlerTargetConfig};

    #[test]
    fn builds_dht_options_from_p1_and_p17_profiles() {
        let mut cfg = CrawlerWorkerConfig::default();
        let p1 = build_dht_options_with_fd_limit(&cfg, 100, Some(262_144)).unwrap();
        assert_eq!(p1.port, 12313);
        assert_eq!(p1.netmode, NetMode::Ipv4Only);
        assert_eq!(p1.crawl.rate_limit.max_find_node_rate_per_sec, 878);
        assert_eq!(p1.crawl.rate_limit.max_in_flight, 4_498);
        assert_eq!(p1.metadata.max_worker_count, 256);

        cfg.crawl_profile = "P17".to_string();
        cfg.netmode = NetModeConfig::Dual;
        cfg.metadata.max_worker_count = 1024;
        let p17 = build_dht_options_with_fd_limit(&cfg, 100, Some(1024)).unwrap();
        assert_eq!(p17.netmode, NetMode::DualStack);
        assert_eq!(p17.crawl.rate_limit.max_find_node_rate_per_sec, 10_000);
        assert_eq!(p17.crawl.pool.capacity, 200_000);
        assert_eq!(p17.peer_lookup.max_lookups_per_second, 64);
        assert!(p17.metadata.max_worker_count < 1024);
    }

    #[test]
    fn l3_overrides_win_over_profile_defaults() {
        let cfg = CrawlerWorkerConfig {
            crawl_profile: "P15".to_string(),
            crawl: hameta_common::CrawlerCrawlConfig {
                rate_limit: CrawlerRateLimitConfig {
                    max_find_node_rate_per_sec: Some(2_222),
                    metadata_pressure_floor_percent: Some(40),
                    ..Default::default()
                },
                target: CrawlerTargetConfig {
                    random_walk_percent: Some(55),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let opts = build_dht_options_with_fd_limit(&cfg, 0, None).unwrap();
        assert_eq!(opts.crawl.rate_limit.max_find_node_rate_per_sec, 2_222);
        assert_eq!(opts.crawl.rate_limit.metadata_pressure_floor_percent, 40);
        assert_eq!(opts.crawl.target.random_walk_percent, 55);
        // 未覆写字段保持 P15 档位默认
        assert_eq!(opts.crawl.rate_limit.burst, 768);
    }

    #[test]
    fn unknown_profile_returns_error() {
        let cfg = CrawlerWorkerConfig {
            crawl_profile: "P99".to_string(),
            ..Default::default()
        };
        assert!(build_dht_options_with_fd_limit(&cfg, 0, None).is_err());
    }
}
