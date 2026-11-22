use moka::sync::Cache;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataCacheState {
    Pending {
        retry_after: Instant,
        failure_cooldown: bool,
    },
    Indexed,
    PolicyRejected {
        retry_after: Instant,
    },
}

/// 本地有界元数据状态缓存，避免高频重复 `announce`/`get_peers` 冲击 Redis。
pub struct MetadataCache {
    cache: Cache<Vec<u8>, MetadataCacheState>,
    failure_backoff: Duration,
}

impl MetadataCache {
    pub fn new(max_capacity: usize, time_to_live_secs: u64, failure_backoff_secs: u64) -> Self {
        let cache = Cache::builder()
            .max_capacity(max_capacity.max(1) as u64)
            .time_to_live(Duration::from_secs(time_to_live_secs.max(1)))
            .build();
        Self {
            cache,
            failure_backoff: Duration::from_secs(failure_backoff_secs),
        }
    }

    pub fn get_sync(&self, hash_bytes: &[u8]) -> Option<MetadataCacheState> {
        let state = self.cache.get(hash_bytes)?;
        let expires = match state {
            MetadataCacheState::Pending { retry_after, .. }
            | MetadataCacheState::PolicyRejected { retry_after } => Some(retry_after),
            MetadataCacheState::Indexed => None,
        };
        if expires.is_some_and(|deadline| Instant::now() >= deadline) {
            self.cache.invalidate(hash_bytes);
            return None;
        }
        Some(state)
    }

    pub fn check_state(&self, info_hash: &str) -> Option<MetadataCacheState> {
        let key = info_hash_cache_key(info_hash);
        self.get_sync(&key)
    }

    pub fn mark_indexed(&self, info_hash: &str) {
        self.cache
            .insert(info_hash_cache_key(info_hash), MetadataCacheState::Indexed);
    }

    pub fn mark_pending(&self, info_hash: &str, retry_after: Duration) {
        self.cache.insert(
            info_hash_cache_key(info_hash),
            MetadataCacheState::Pending {
                retry_after: Instant::now() + retry_after,
                failure_cooldown: false,
            },
        );
    }

    pub fn mark_policy_rejected(&self, info_hash: &str, retry_after: Duration) {
        self.cache.insert(
            info_hash_cache_key(info_hash),
            MetadataCacheState::PolicyRejected {
                retry_after: Instant::now() + retry_after,
            },
        );
    }

    pub fn mark_failure_cooldown(&self, info_hash: &str) {
        let key = info_hash_cache_key(info_hash);
        if self.failure_backoff.is_zero() {
            self.cache.invalidate(&key);
            return;
        }
        self.cache.insert(
            key,
            MetadataCacheState::Pending {
                retry_after: Instant::now() + self.failure_backoff,
                failure_cooldown: true,
            },
        );
    }

    pub fn entry_count(&self) -> u64 {
        self.cache.entry_count()
    }
}

pub fn info_hash_cache_key(info_hash: &str) -> Vec<u8> {
    let encoded = info_hash.as_bytes();
    if encoded.len().is_multiple_of(2) {
        let mut decoded = Vec::with_capacity(encoded.len() / 2);
        for pair in encoded.chunks_exact(2) {
            let Some(high) = hex_nibble(pair[0]) else {
                return encoded.to_vec();
            };
            let Some(low) = hex_nibble(pair[1]) else {
                return encoded.to_vec();
            };
            decoded.push((high << 4) | low);
        }
        return decoded;
    }
    encoded.to_vec()
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_lifecycle_and_expiry() {
        let cache = MetadataCache::new(100, 300, 30);
        let h_idx = "0123456789abcdef0123456789abcdef01234567";
        let h_pend = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let h_exp = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        cache.mark_indexed(h_idx);
        assert_eq!(cache.check_state(h_idx), Some(MetadataCacheState::Indexed));

        cache.mark_pending(h_pend, Duration::from_secs(60));
        assert!(matches!(
            cache.check_state(h_pend),
            Some(MetadataCacheState::Pending {
                failure_cooldown: false,
                ..
            })
        ));

        cache.mark_failure_cooldown(h_pend);
        assert!(matches!(
            cache.check_state(h_pend),
            Some(MetadataCacheState::Pending {
                failure_cooldown: true,
                ..
            })
        ));

        cache.mark_policy_rejected(h_exp, Duration::ZERO);
        assert_eq!(cache.check_state(h_exp), None);
    }
}
