use crate::smart::extract_edge_discovery_candidates;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const MAX_TRACKED_ENTRIES: usize = 512;
const MAX_BODY_HASHES_PER_ENTRY: usize = 64;
const MAX_SAMPLE_TITLES_PER_ENTRY: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredAdCandidate {
    pub phrase: String,
    pub occurrences: u64,
    pub distinct_titles: usize,
    pub suggested_score: u8,
    pub sample_titles: Vec<String>,
    pub last_seen_unix: i64,
}

#[derive(Debug, Clone)]
struct TrackedEntry {
    phrase: String,
    occurrences: u64,
    distinct_bodies: HashSet<u64>,
    max_score: u8,
    sample_titles: Vec<String>,
    last_seen_unix: i64,
}

fn fnv1a_64_normalized(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, '.' | '-' | '_' | '[' | ']' | '【' | '】' | '(' | ')')
        {
            continue;
        }
        for lc in c.to_lowercase() {
            let mut buf = [0u8; 4];
            let bytes = lc.encode_utf8(&mut buf).as_bytes();
            for &b in bytes {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

fn strip_outer_brackets_lower(text: &str) -> String {
    let t = text.trim();
    let stripped = t
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .or_else(|| t.strip_prefix('【').and_then(|s| s.strip_suffix('】')))
        .or_else(|| t.strip_prefix('(').and_then(|s| s.strip_suffix(')')))
        .or_else(|| t.strip_prefix('（').and_then(|s| s.strip_suffix('）')))
        .or_else(|| t.strip_prefix('「').and_then(|s| s.strip_suffix('」')))
        .or_else(|| t.strip_prefix('『').and_then(|s| s.strip_suffix('』')))
        .or_else(|| t.strip_prefix('《').and_then(|s| s.strip_suffix('》')))
        .unwrap_or(t);
    stripped.trim().to_lowercase()
}

fn truncate_sample(raw: &str, max_chars: usize) -> String {
    let mut chars = raw.trim().chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{collected}…")
    } else {
        collected
    }
}

/// 旁路无监督高频广告模板自发现统计器（有界 Space-Saving + 文档频次估计）。
#[derive(Debug, Default)]
pub struct AdDiscoveryTracker {
    entries: HashMap<String, TrackedEntry>,
}

impl AdDiscoveryTracker {
    pub fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(128),
        }
    }

    pub fn observe(&mut self, text: &str, now_unix: i64) {
        let candidates = extract_edge_discovery_candidates(text);
        if candidates.is_empty() {
            return;
        }

        for (phrase, remainder, score) in candidates {
            let key = phrase.to_lowercase();
            let body_hash = fnv1a_64_normalized(&remainder);

            if let Some(entry) = self.entries.get_mut(&key) {
                entry.occurrences = entry.occurrences.saturating_add(1);
                if score > entry.max_score {
                    entry.max_score = score;
                }
                if entry.distinct_bodies.len() < MAX_BODY_HASHES_PER_ENTRY {
                    entry.distinct_bodies.insert(body_hash);
                }
                if entry.sample_titles.len() < MAX_SAMPLE_TITLES_PER_ENTRY {
                    let preview = truncate_sample(text, 96);
                    if !entry.sample_titles.contains(&preview) {
                        entry.sample_titles.push(preview);
                    }
                }
                if now_unix > entry.last_seen_unix {
                    entry.last_seen_unix = now_unix;
                }
                continue;
            }

            if self.entries.len() >= MAX_TRACKED_ENTRIES
                && let Some(victim_key) = self
                    .entries
                    .iter()
                    .min_by_key(|(_, e)| (e.distinct_bodies.len(), e.occurrences, e.last_seen_unix))
                    .map(|(k, _)| k.clone())
            {
                self.entries.remove(&victim_key);
            }

            let mut distinct_bodies = HashSet::with_capacity(8);
            distinct_bodies.insert(body_hash);
            self.entries.insert(
                key,
                TrackedEntry {
                    phrase,
                    occurrences: 1,
                    distinct_bodies,
                    max_score: score,
                    sample_titles: vec![truncate_sample(text, 96)],
                    last_seen_unix: now_unix,
                },
            );
        }
    }

    pub fn snapshot_candidates(
        &self,
        limit: usize,
        exclude_lower: &HashSet<String>,
    ) -> Vec<DiscoveredAdCandidate> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out: Vec<DiscoveredAdCandidate> = self
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                let inner_key = strip_outer_brackets_lower(&entry.phrase);
                if exclude_lower.contains(key) || exclude_lower.contains(&inner_key) {
                    return None;
                }
                let distinct_titles = entry.distinct_bodies.len();
                let qualified =
                    distinct_titles >= 3 || (distinct_titles >= 2 && entry.max_score >= 45);
                if !qualified {
                    return None;
                }
                Some(DiscoveredAdCandidate {
                    phrase: entry.phrase.clone(),
                    occurrences: entry.occurrences,
                    distinct_titles,
                    suggested_score: entry.max_score,
                    sample_titles: entry.sample_titles.clone(),
                    last_seen_unix: entry.last_seen_unix,
                })
            })
            .collect();

        sort_and_truncate_candidates(&mut out, limit);
        out
    }
}

fn sort_and_truncate_candidates(items: &mut Vec<DiscoveredAdCandidate>, limit: usize) {
    items.sort_by(|a, b| {
        let weight_a = (a.distinct_titles as u64)
            .saturating_mul(100 + u64::from(a.suggested_score))
            .saturating_add(a.occurrences);
        let weight_b = (b.distinct_titles as u64)
            .saturating_mul(100 + u64::from(b.suggested_score))
            .saturating_add(b.occurrences);
        weight_b
            .cmp(&weight_a)
            .then_with(|| b.distinct_titles.cmp(&a.distinct_titles))
            .then_with(|| b.occurrences.cmp(&a.occurrences))
            .then_with(|| a.phrase.cmp(&b.phrase))
    });
    if limit > 0 {
        items.truncate(limit);
    }
}

/// 合并来自多个 Indexer Worker 或离线扫描的候选广告词（按大小写去重并过滤已收录短语）。
pub fn merge_discovered_candidates(
    batches: impl IntoIterator<Item = Vec<DiscoveredAdCandidate>>,
    exclude_lower: &HashSet<String>,
    limit: usize,
) -> Vec<DiscoveredAdCandidate> {
    let mut merged: HashMap<String, DiscoveredAdCandidate> = HashMap::new();
    for batch in batches {
        for cand in batch {
            let key = cand.phrase.trim().to_lowercase();
            let inner_key = strip_outer_brackets_lower(&cand.phrase);
            if key.is_empty() || exclude_lower.contains(&key) || exclude_lower.contains(&inner_key)
            {
                continue;
            }
            merged
                .entry(key)
                .and_modify(|existing| {
                    existing.occurrences = existing.occurrences.saturating_add(cand.occurrences);
                    existing.distinct_titles = existing.distinct_titles.max(cand.distinct_titles);
                    existing.suggested_score = existing.suggested_score.max(cand.suggested_score);
                    existing.last_seen_unix = existing.last_seen_unix.max(cand.last_seen_unix);
                    for sample in &cand.sample_titles {
                        if existing.sample_titles.len() >= MAX_SAMPLE_TITLES_PER_ENTRY {
                            break;
                        }
                        if !existing.sample_titles.contains(sample) {
                            existing.sample_titles.push(sample.clone());
                        }
                    }
                })
                .or_insert(cand);
        }
    }

    let mut items: Vec<DiscoveredAdCandidate> = merged.into_values().collect();
    sort_and_truncate_candidates(&mut items, limit);
    items
}
