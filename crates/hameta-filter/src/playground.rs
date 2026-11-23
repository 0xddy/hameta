use crate::cleaner::CleaningTrace;
use crate::engine::{CompiledFilterEngine, FilterOutcome, RuleHit};
use hameta_common::{FileInfo, TorrentMessage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaygroundInput {
    #[serde(default)]
    pub info_hash: String,
    pub name: String,
    #[serde(default)]
    pub files: Vec<FileInfo>,
}

impl PlaygroundInput {
    /// 仅提供标题文本快速构造试跑样本（自动合成一个 100 MiB 安全视频占位文件，避免误触文件列表空/体积限制）。
    pub fn from_title_only(title: &str) -> Self {
        Self {
            info_hash: "0000000000000000000000000000000000000000".to_string(),
            name: title.to_string(),
            files: vec![FileInfo {
                path: "sample/video.mkv".to_string(),
                size: 100 * 1024 * 1024,
            }],
        }
    }

    pub fn to_torrent_message(&self) -> TorrentMessage {
        let total_size = self
            .files
            .iter()
            .map(|f| f.size)
            .fold(0_u64, u64::saturating_add);
        TorrentMessage {
            info_hash: if self.info_hash.trim().is_empty() {
                "0000000000000000000000000000000000000000".to_string()
            } else {
                self.info_hash.trim().to_ascii_lowercase()
            },
            name: self.name.clone(),
            total_size,
            file_count: self.files.len(),
            files: self.files.clone(),
            timestamp: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaygroundItemResult {
    pub sample_index: usize,
    pub info_hash: String,
    pub raw_name: String,
    pub accepted: bool,
    pub refined_name: String,
    pub hit: Option<RuleHit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_by: Option<RuleHit>,
    pub cleaning_trace: CleaningTrace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbDiffStatus {
    UnchangedAccepted,
    UnchangedRejected,
    NewlyRejected,
    NewlyAccepted,
    RefinedNameChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbComparisonItem {
    pub sample_index: usize,
    pub info_hash: String,
    pub raw_name: String,
    pub diff_status: AbDiffStatus,
    pub published_accepted: bool,
    pub published_refined_name: String,
    pub published_hit: Option<RuleHit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_allowed_by: Option<RuleHit>,
    pub draft_accepted: bool,
    pub draft_refined_name: String,
    pub draft_hit: Option<RuleHit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_allowed_by: Option<RuleHit>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbComparisonSummary {
    pub total: usize,
    pub unchanged_accepted: usize,
    pub unchanged_rejected: usize,
    pub newly_rejected: usize,
    pub newly_accepted: usize,
    pub refined_name_changed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbComparisonReport {
    pub published_version: i64,
    pub draft_version: i64,
    pub summary: AbComparisonSummary,
    pub items: Vec<AbComparisonItem>,
}

pub fn evaluate_playground_one(
    engine: &CompiledFilterEngine,
    sample_index: usize,
    sample: &TorrentMessage,
) -> PlaygroundItemResult {
    let cleaning_trace = engine
        .cleaner()
        .refine_with_files_trace(&sample.name, &sample.files);
    let outcome = engine.evaluate(sample);

    match outcome {
        FilterOutcome::Accepted {
            refined_name,
            allowed_by,
        } => PlaygroundItemResult {
            sample_index,
            info_hash: sample.info_hash.clone(),
            raw_name: sample.name.clone(),
            accepted: true,
            refined_name,
            hit: None,
            allowed_by,
            cleaning_trace,
        },
        FilterOutcome::Rejected { refined_name, hit } => PlaygroundItemResult {
            sample_index,
            info_hash: sample.info_hash.clone(),
            raw_name: sample.name.clone(),
            accepted: false,
            refined_name: refined_name.unwrap_or_else(|| cleaning_trace.refined_name.clone()),
            hit: Some(hit),
            allowed_by: None,
            cleaning_trace,
        },
    }
}

pub fn evaluate_playground_batch(
    engine: &CompiledFilterEngine,
    samples: &[TorrentMessage],
) -> Vec<PlaygroundItemResult> {
    samples
        .iter()
        .enumerate()
        .map(|(idx, sample)| evaluate_playground_one(engine, idx, sample))
        .collect()
}

pub fn compare_playground_ab(
    published: &CompiledFilterEngine,
    draft: &CompiledFilterEngine,
    samples: &[TorrentMessage],
) -> AbComparisonReport {
    let mut summary = AbComparisonSummary {
        total: samples.len(),
        ..AbComparisonSummary::default()
    };
    let mut items = Vec::with_capacity(samples.len());

    for (idx, sample) in samples.iter().enumerate() {
        let pub_res = evaluate_playground_one(published, idx, sample);
        let draft_res = evaluate_playground_one(draft, idx, sample);

        let diff_status = match (pub_res.accepted, draft_res.accepted) {
            (true, false) => {
                summary.newly_rejected += 1;
                AbDiffStatus::NewlyRejected
            }
            (false, true) => {
                summary.newly_accepted += 1;
                AbDiffStatus::NewlyAccepted
            }
            (false, false) => {
                summary.unchanged_rejected += 1;
                AbDiffStatus::UnchangedRejected
            }
            (true, true) => {
                if pub_res.refined_name != draft_res.refined_name {
                    summary.refined_name_changed += 1;
                    AbDiffStatus::RefinedNameChanged
                } else {
                    summary.unchanged_accepted += 1;
                    AbDiffStatus::UnchangedAccepted
                }
            }
        };

        items.push(AbComparisonItem {
            sample_index: idx,
            info_hash: sample.info_hash.clone(),
            raw_name: sample.name.clone(),
            diff_status,
            published_accepted: pub_res.accepted,
            published_refined_name: pub_res.refined_name,
            published_hit: pub_res.hit,
            published_allowed_by: pub_res.allowed_by,
            draft_accepted: draft_res.accepted,
            draft_refined_name: draft_res.refined_name,
            draft_hit: draft_res.hit,
            draft_allowed_by: draft_res.allowed_by,
        });
    }

    AbComparisonReport {
        published_version: published.version(),
        draft_version: draft.version(),
        summary,
        items,
    }
}
