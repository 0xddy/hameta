use serde::{Deserialize, Serialize};

pub const MAX_CRAWL_PROFILE_LEVEL: u8 = 17;
pub const SCALED_CRAWL_PROFILE_LEVEL: u8 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlProfileDefaults {
    pub find_node_rate_per_sec: u32,
    pub find_node_burst: u32,
    pub find_node_max_in_flight: usize,
    pub max_new_destinations_per_minute: u32,
    pub max_replacements_per_minute: u32,
    pub max_response_rate_per_sec: u32,
    pub max_response_bytes_per_sec: u64,
    pub max_response_rate_per_source: u32,
    pub max_in_flight_per_subnet: usize,
    pub peer_lookup_rate_per_sec: u32,
    pub peer_lookup_burst: u32,
    pub peer_lookup_max_active: usize,
    pub pool_capacity: usize,
    pub responsive_capacity: usize,
    pub routing_snapshot_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrawlProfileInfo {
    pub name: String,
    pub level: u8,
    pub recommended_nofile: u64,
    pub defaults: CrawlProfileDefaults,
}

/// 根据档位名称（`P1`–`P17` 及历史兼容名称 `safe`/`balanced`/`aggressive`/`turbo`）返回核心默认参数。
pub fn crawl_profile_defaults(profile: &str) -> Option<CrawlProfileDefaults> {
    let normalized = profile.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "safe" => Some(CrawlProfileDefaults {
            find_node_rate_per_sec: 80,
            find_node_burst: 16,
            find_node_max_in_flight: 256,
            max_new_destinations_per_minute: 5_000,
            max_replacements_per_minute: 10_000,
            max_response_rate_per_sec: 200,
            max_response_bytes_per_sec: 256 * 1024,
            max_response_rate_per_source: 20,
            max_in_flight_per_subnet: 4,
            peer_lookup_rate_per_sec: 32,
            peer_lookup_burst: 32,
            peer_lookup_max_active: 64,
            pool_capacity: 100_000,
            responsive_capacity: 16_384,
            routing_snapshot_size: 4_096,
        }),
        "balanced" => Some(CrawlProfileDefaults {
            find_node_rate_per_sec: 200,
            find_node_burst: 40,
            find_node_max_in_flight: 512,
            max_new_destinations_per_minute: 10_000,
            max_replacements_per_minute: 25_000,
            max_response_rate_per_sec: 500,
            max_response_bytes_per_sec: 1024 * 1024,
            max_response_rate_per_source: 40,
            max_in_flight_per_subnet: 8,
            peer_lookup_rate_per_sec: 32,
            peer_lookup_burst: 32,
            peer_lookup_max_active: 64,
            pool_capacity: 100_000,
            responsive_capacity: 16_384,
            routing_snapshot_size: 4_096,
        }),
        "aggressive" => Some(CrawlProfileDefaults {
            find_node_rate_per_sec: 500,
            find_node_burst: 100,
            find_node_max_in_flight: 2_048,
            max_new_destinations_per_minute: 20_000,
            max_replacements_per_minute: 60_000,
            max_response_rate_per_sec: 1_200,
            max_response_bytes_per_sec: 4 * 1024 * 1024,
            max_response_rate_per_source: 80,
            max_in_flight_per_subnet: 16,
            peer_lookup_rate_per_sec: 32,
            peer_lookup_burst: 32,
            peer_lookup_max_active: 64,
            pool_capacity: 100_000,
            responsive_capacity: 16_384,
            routing_snapshot_size: 4_096,
        }),
        "turbo" => Some(CrawlProfileDefaults {
            find_node_rate_per_sec: 800,
            find_node_burst: 160,
            find_node_max_in_flight: 4_096,
            max_new_destinations_per_minute: 48_000,
            max_replacements_per_minute: 90_000,
            max_response_rate_per_sec: 2_000,
            max_response_bytes_per_sec: 8 * 1024 * 1024,
            max_response_rate_per_source: 120,
            max_in_flight_per_subnet: 32,
            peer_lookup_rate_per_sec: 32,
            peer_lookup_burst: 32,
            peer_lookup_max_active: 64,
            pool_capacity: 100_000,
            responsive_capacity: 16_384,
            routing_snapshot_size: 4_096,
        }),
        _ => {
            let level = normalized.strip_prefix('p')?.parse::<u8>().ok()?;
            crawl_profile_defaults_for_level(level)
        }
    }
}

/// 返回 `P1`–`P17` 全量档位描述，供后端校验与前端档位对照选择器展示。
pub fn all_crawl_profiles() -> Vec<CrawlProfileInfo> {
    (1..=MAX_CRAWL_PROFILE_LEVEL)
        .filter_map(|level| {
            let defaults = crawl_profile_defaults_for_level(level)?;
            let recommended_nofile = match level {
                1..=10 => 16_384,
                11..=15 => 65_536,
                16 => 131_072,
                _ => 262_144,
            };
            Some(CrawlProfileInfo {
                name: format!("P{level}"),
                level,
                recommended_nofile,
                defaults,
            })
        })
        .collect()
}

fn crawl_profile_defaults_for_level(level: u8) -> Option<CrawlProfileDefaults> {
    if !(1..=MAX_CRAWL_PROFILE_LEVEL).contains(&level) {
        return None;
    }

    match level {
        16 => {
            return Some(CrawlProfileDefaults {
                find_node_rate_per_sec: 5_800,
                find_node_burst: 960,
                find_node_max_in_flight: 24_576,
                max_new_destinations_per_minute: 348_000,
                max_replacements_per_minute: 540_000,
                max_response_rate_per_sec: 90_000,
                max_response_bytes_per_sec: 48 * 1024 * 1024,
                max_response_rate_per_source: 90_000,
                max_in_flight_per_subnet: 192,
                peer_lookup_rate_per_sec: 32,
                peer_lookup_burst: 32,
                peer_lookup_max_active: 64,
                pool_capacity: 100_000,
                responsive_capacity: 16_384,
                routing_snapshot_size: 4_096,
            });
        }
        17 => {
            return Some(CrawlProfileDefaults {
                find_node_rate_per_sec: 10_000,
                find_node_burst: 1_280,
                find_node_max_in_flight: 32_768,
                max_new_destinations_per_minute: 480_000,
                max_replacements_per_minute: 720_000,
                max_response_rate_per_sec: 120_000,
                max_response_bytes_per_sec: 64 * 1024 * 1024,
                max_response_rate_per_source: 120_000,
                max_in_flight_per_subnet: 256,
                peer_lookup_rate_per_sec: 64,
                peer_lookup_burst: 64,
                peer_lookup_max_active: 128,
                pool_capacity: 200_000,
                responsive_capacity: 196_608,
                routing_snapshot_size: 32_768,
            });
        }
        _ => {}
    }

    Some(CrawlProfileDefaults {
        find_node_rate_per_sec: scale_profile_u32(3_840, level),
        find_node_burst: scale_profile_u32(768, level),
        find_node_max_in_flight: scale_profile_usize(19_661, level),
        max_new_destinations_per_minute: scale_profile_u32(230_400, level),
        max_replacements_per_minute: scale_profile_u32(432_000, level),
        max_response_rate_per_sec: scale_profile_u32(72_000, level),
        max_response_bytes_per_sec: scale_profile_u64(192 * 1024 * 1024 / 5, level),
        max_response_rate_per_source: scale_profile_u32(72_000, level),
        max_in_flight_per_subnet: scale_profile_usize(154, level),
        peer_lookup_rate_per_sec: 32,
        peer_lookup_burst: 32,
        peer_lookup_max_active: 64,
        pool_capacity: 100_000,
        responsive_capacity: 16_384,
        routing_snapshot_size: 4_096,
    })
}

fn scale_profile_u32(maximum: u32, level: u8) -> u32 {
    u32::try_from(scale_profile_u64(u64::from(maximum), level)).unwrap_or(maximum)
}

fn scale_profile_usize(maximum: usize, level: u8) -> usize {
    let max_u64 = u64::try_from(maximum).unwrap_or(u64::MAX);
    usize::try_from(scale_profile_u64(max_u64, level)).unwrap_or(maximum)
}

fn scale_profile_u64(maximum: u64, level: u8) -> u64 {
    let reductions = u32::from(SCALED_CRAWL_PROFILE_LEVEL.saturating_sub(level));
    let numerator = 9u128.pow(reductions);
    let denominator = 10u128.pow(reductions);
    let scaled = (u128::from(maximum) * numerator + denominator / 2) / denominator;
    u64::try_from(scaled).unwrap_or(maximum)
}
