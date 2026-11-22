use crate::engine::{CompiledFilterEngine, FilterCategory, FilterOutcome, RuleHit};
use crate::error::FilterError;
use crate::schema::{FilterBundle, PolicyAction};
use arc_swap::{ArcSwap, Guard};
use chrono::Utc;
use hameta_common::TorrentMessage;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const DEFAULT_REJECTED_SAMPLE_CAPACITY: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedSample {
    pub info_hash: String,
    pub raw_name: String,
    pub total_size: u64,
    pub file_count: usize,
    pub hit: RuleHit,
    pub rejected_at_unix: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleHitStat {
    pub category: FilterCategory,
    #[serde(default)]
    pub action: PolicyAction,
    pub rule_id: String,
    pub rule_label: String,
    pub hits: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterStatsSnapshot {
    pub active_version: i64,
    pub total_evaluated: u64,
    pub total_accepted: u64,
    pub total_rejected: u64,
    pub rule_hits: Vec<RuleHitStat>,
    pub recent_rejected: Vec<RejectedSample>,
}

/// 线程安全、支持 `ArcSwap` 无停机热切换、按规则归因与有界拦截样本缓冲的过滤运行时。
pub struct FilterRuntime {
    engine: ArcSwap<CompiledFilterEngine>,
    total_evaluated: AtomicU64,
    total_accepted: AtomicU64,
    total_rejected: AtomicU64,
    rule_hits: Mutex<HashMap<(FilterCategory, PolicyAction, String), RuleHitStat>>,
    recent_rejected: Mutex<VecDeque<RejectedSample>>,
    sample_capacity: usize,
}

impl FilterRuntime {
    pub fn new(bundle: &FilterBundle) -> Result<Self, FilterError> {
        Self::with_sample_capacity(bundle, DEFAULT_REJECTED_SAMPLE_CAPACITY)
    }

    pub fn with_sample_capacity(
        bundle: &FilterBundle,
        sample_capacity: usize,
    ) -> Result<Self, FilterError> {
        let engine = CompiledFilterEngine::compile(bundle)?;
        Ok(Self {
            engine: ArcSwap::from_pointee(engine),
            total_evaluated: AtomicU64::new(0),
            total_accepted: AtomicU64::new(0),
            total_rejected: AtomicU64::new(0),
            rule_hits: Mutex::new(HashMap::new()),
            recent_rejected: Mutex::new(VecDeque::with_capacity(sample_capacity)),
            sample_capacity,
        })
    }

    pub fn active_version(&self) -> i64 {
        self.engine.load().version()
    }

    pub fn load_engine(&self) -> Guard<Arc<CompiledFilterEngine>> {
        self.engine.load()
    }

    /// 预编译新版规则并原子切换生效；若编译失败，原线上规则保持完好不变。
    pub fn reload(&self, bundle: &FilterBundle) -> Result<i64, FilterError> {
        let next = CompiledFilterEngine::compile(bundle)?;
        let version = next.version();
        self.engine.store(Arc::new(next));
        Ok(version)
    }

    /// 无锁加载当前规则进行判定，并更新统计与拦截样本缓冲。
    pub fn evaluate_and_record(&self, msg: &TorrentMessage) -> FilterOutcome {
        let outcome = self.engine.load().evaluate(msg);
        self.total_evaluated.fetch_add(1, Ordering::Relaxed);

        match &outcome {
            FilterOutcome::Accepted { allowed_by, .. } => {
                self.total_accepted.fetch_add(1, Ordering::Relaxed);
                if let Some(hit) = allowed_by {
                    self.record_rule_hit(hit);
                }
            }
            FilterOutcome::Rejected { hit, .. } => {
                self.total_rejected.fetch_add(1, Ordering::Relaxed);
                self.record_rule_hit(hit);
                self.record_rejection_sample(msg, hit);
            }
        }

        outcome
    }

    fn record_rule_hit(&self, hit: &RuleHit) {
        let mut map = self
            .rule_hits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (hit.category, hit.action, hit.rule_id.clone());
        let entry = map.entry(key).or_insert_with(|| RuleHitStat {
            category: hit.category,
            action: hit.action,
            rule_id: hit.rule_id.clone(),
            rule_label: hit.rule_label.clone(),
            hits: 0,
        });
        entry.hits += 1;
        entry.rule_label = hit.rule_label.clone();
    }

    fn record_rejection_sample(&self, msg: &TorrentMessage, hit: &RuleHit) {
        if self.sample_capacity > 0 {
            let mut ring = self
                .recent_rejected
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if ring.len() >= self.sample_capacity {
                ring.pop_front();
            }
            ring.push_back(RejectedSample {
                info_hash: msg.info_hash.clone(),
                raw_name: msg.name.clone(),
                total_size: msg.total_size,
                file_count: msg.file_count,
                hit: hit.clone(),
                rejected_at_unix: Utc::now().timestamp(),
            });
        }
    }

    /// 导出按命中次数降序排列的规则归因统计及最新拦截样本（最新在前）。
    pub fn snapshot_stats(&self, top_n: usize) -> FilterStatsSnapshot {
        let mut rule_hits: Vec<RuleHitStat> = {
            let map = self
                .rule_hits
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            map.values().cloned().collect()
        };
        rule_hits.sort_by(|a, b| b.hits.cmp(&a.hits).then_with(|| a.rule_id.cmp(&b.rule_id)));
        if top_n > 0 {
            rule_hits.truncate(top_n);
        }

        let recent_rejected = {
            let ring = self
                .recent_rejected
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            ring.iter().rev().cloned().collect()
        };

        FilterStatsSnapshot {
            active_version: self.active_version(),
            total_evaluated: self.total_evaluated.load(Ordering::Relaxed),
            total_accepted: self.total_accepted.load(Ordering::Relaxed),
            total_rejected: self.total_rejected.load(Ordering::Relaxed),
            rule_hits,
            recent_rejected,
        }
    }
}
