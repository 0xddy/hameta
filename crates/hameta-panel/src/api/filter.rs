use crate::api::error::{ApiError, extract_client_ip};
use crate::auth::AuthUser;
use crate::server::PanelState;
use crate::supervisor::IndexerReloadResult;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use chrono::Utc;
use hameta_common::{FileInfo, TorrentMessage, WorkerRole, WorkerRuntimeState};
use hameta_filter::{
    AbComparisonReport, AdDiscoveryTracker, AdPhraseRule, AdRegexRule, CompiledFilterEngine,
    DiscoveredAdCandidate, FilterBundle, FilterCategory, FilterPresetCatalog, FilterStatsSnapshot,
    PlaygroundInput, PlaygroundItemResult, PolicyAction, RejectedSample, RuleHitStat, TitleCleaner,
    builtin_preset_catalog, compare_playground_ab, default_online_filter_bundle,
    evaluate_playground_batch, import_any_filter_json, merge_discovered_candidates,
    parse_stored_policy_json, policy_preset_bundle, upgrade_legacy_builtin_notes,
};
use hameta_storage::FilterRuleVersionRow;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct FilterVersionDto {
    pub id: i64,
    pub version: i64,
    pub status: String,
    pub bundle: FilterBundle,
    pub created_by: String,
    pub note: String,
    pub created_at: i64,
    pub published_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexerRuleStatusDto {
    pub id: String,
    pub display_name: String,
    pub runtime_state: WorkerRuntimeState,
    pub pid: Option<u32>,
    pub active_rule_version: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct FilterStateResponse {
    pub published: Option<FilterVersionDto>,
    pub draft: Option<FilterVersionDto>,
    pub has_draft: bool,
    pub online_indexers: Vec<IndexerRuleStatusDto>,
}

#[derive(Debug, Deserialize)]
pub struct SaveFilterDraftRequest {
    pub bundle: FilterBundle,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct ValidateFilterRequest {
    pub bundle: FilterBundle,
}

#[derive(Debug, Serialize)]
pub struct FilterRuleCountsDto {
    pub total_rules: usize,
    pub enabled_rules: usize,
    pub reject_rules: usize,
    pub allow_rules: usize,
    pub enabled_gates: usize,
    pub ad_phrases: usize,
    pub ad_regexes: usize,
}

#[derive(Debug, Serialize)]
pub struct ValidateFilterResponse {
    pub ok: bool,
    pub counts: FilterRuleCountsDto,
}

#[derive(Debug, Deserialize)]
pub struct ImportLegacyFilterRequest {
    pub raw_json: String,
    #[serde(default = "default_true")]
    pub save_as_draft: bool,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct ImportLegacyFilterResponse {
    pub source_format: String,
    pub conversion_notes: Vec<String>,
    pub bundle: FilterBundle,
    pub saved_draft: Option<FilterVersionDto>,
}

#[derive(Debug, Deserialize)]
pub struct ResetPresetRequest {
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default = "default_true")]
    pub include_dictionary: bool,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct PublishFilterRequest {
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PublishOrRollbackFilterResponse {
    pub published: FilterVersionDto,
    pub reload_results: Vec<IndexerReloadResult>,
}

#[derive(Debug, Deserialize)]
pub struct ListVersionsQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct RollbackFilterRequest {
    pub target_version: i64,
}

#[derive(Debug, Serialize)]
pub struct WorkerRejectedSampleDto {
    pub worker_id: String,
    #[serde(flatten)]
    pub sample: RejectedSample,
}

#[derive(Debug, Serialize)]
pub struct IndexerStatBrief {
    pub id: String,
    pub display_name: String,
    pub runtime_state: WorkerRuntimeState,
    pub active_rule_version: Option<i64>,
    pub total_evaluated: u64,
    pub total_accepted: u64,
    pub total_rejected: u64,
}

#[derive(Debug, Serialize)]
pub struct FilterStatsResponse {
    pub total_evaluated: u64,
    pub total_accepted: u64,
    pub total_rejected: u64,
    pub rule_hits: Vec<RuleHitStat>,
    pub recent_rejected: Vec<WorkerRejectedSampleDto>,
    pub workers: Vec<IndexerStatBrief>,
    #[serde(default)]
    pub discovered_ad_candidates: Vec<DiscoveredAdCandidate>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoverCandidatesRequest {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct DiscoverCandidatesResponse {
    pub source: String,
    pub scanned_titles: usize,
    pub candidates: Vec<DiscoveredAdCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaygroundMode {
    Published,
    Draft,
    AbCompare,
}

#[derive(Debug, Deserialize)]
pub struct PlaygroundTestRequest {
    #[serde(default = "default_playground_mode")]
    pub mode: PlaygroundMode,
    #[serde(default)]
    pub draft_bundle: Option<FilterBundle>,
    pub samples: Vec<PlaygroundInput>,
    #[serde(default = "default_true")]
    pub auto_placeholder_file: bool,
}

#[derive(Debug, Serialize)]
pub struct PlaygroundTestResponse {
    pub mode: String,
    pub published_version: i64,
    pub draft_version: Option<i64>,
    pub results: Vec<PlaygroundItemResult>,
    pub ab_report: Option<AbComparisonReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleSource {
    ManticoreLatest,
    ManticoreRandom,
    RejectedRecent,
}

#[derive(Debug, Deserialize)]
pub struct PlaygroundSampleRequest {
    pub source: SampleSource,
    pub limit: Option<usize>,
    #[serde(default)]
    pub keyword: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlaygroundSampleResponse {
    pub source: String,
    pub samples: Vec<PlaygroundInput>,
}

fn default_true() -> bool {
    true
}

fn default_playground_mode() -> PlaygroundMode {
    PlaygroundMode::AbCompare
}

fn row_to_bundle(row: &FilterRuleVersionRow) -> Result<FilterBundle, ApiError> {
    let policy = parse_stored_policy_json(&row.filter_json).map_err(|e| {
        ApiError::internal("解析存储的过滤规则 JSON 失败").with_detail(e.to_string())
    })?;
    let mut ad_phrases: Vec<AdPhraseRule> =
        serde_json::from_str(&row.ad_phrases_json).map_err(|e| {
            ApiError::internal("解析存储的广告短语 JSON 失败").with_detail(e.to_string())
        })?;
    let mut ad_regexes: Vec<AdRegexRule> =
        serde_json::from_str(&row.ad_regexes_json).map_err(|e| {
            ApiError::internal("解析存储的广告正则 JSON 失败").with_detail(e.to_string())
        })?;
    upgrade_legacy_builtin_notes(&mut ad_phrases, &mut ad_regexes);
    Ok(FilterBundle {
        version: row.version,
        policy,
        ad_phrases,
        ad_regexes,
    })
}

fn row_to_dto(row: &FilterRuleVersionRow) -> Result<FilterVersionDto, ApiError> {
    let bundle = row_to_bundle(row)?;
    Ok(FilterVersionDto {
        id: row.id,
        version: row.version,
        status: row.status.clone(),
        bundle,
        created_by: row.created_by.clone(),
        note: row.note.clone(),
        created_at: row.created_at,
        published_at: row.published_at,
    })
}

fn ensure_rule_ids(bundle: &mut FilterBundle) {
    for (idx, item) in bundle.policy.rules.iter_mut().enumerate() {
        if item.id.trim().is_empty() {
            item.id = format!("rule-{}", idx + 1);
        }
    }
    for (idx, item) in bundle.ad_phrases.iter_mut().enumerate() {
        if item.id.trim().is_empty() {
            item.id = format!("ad-phrase-{}", idx + 1);
        }
    }
    for (idx, item) in bundle.ad_regexes.iter_mut().enumerate() {
        if item.id.trim().is_empty() {
            item.id = format!("ad-re-{}", idx + 1);
        }
    }
}

fn count_rules(bundle: &FilterBundle) -> FilterRuleCountsDto {
    let g = &bundle.policy.gates;
    let enabled_gates = [
        g.reject_empty_name,
        g.min_name_length.enabled,
        g.max_name_length.enabled,
        g.reject_empty_files,
        g.reject_all_empty_files,
        g.max_empty_files.enabled,
        g.max_file_count.enabled,
        g.max_file_name_length.enabled,
        g.min_total_size.enabled,
        g.max_total_size.enabled,
        g.density_check.enabled,
    ]
    .into_iter()
    .filter(|&v| v)
    .count();

    FilterRuleCountsDto {
        total_rules: bundle.policy.rules.len(),
        enabled_rules: bundle.policy.rules.iter().filter(|r| r.enabled).count(),
        reject_rules: bundle
            .policy
            .rules
            .iter()
            .filter(|r| r.action == PolicyAction::Reject)
            .count(),
        allow_rules: bundle
            .policy
            .rules
            .iter()
            .filter(|r| r.action == PolicyAction::Allow)
            .count(),
        enabled_gates,
        ad_phrases: bundle.ad_phrases.len(),
        ad_regexes: bundle.ad_regexes.len(),
    }
}

async fn load_published_bundle(state: &PanelState) -> Result<FilterBundle, ApiError> {
    if let Some(row) = state.db.get_published_filter_version().await? {
        row_to_bundle(&row)
    } else {
        Ok(default_online_filter_bundle())
    }
}

async fn load_current_dictionary(
    state: &PanelState,
) -> Result<(Vec<AdPhraseRule>, Vec<AdRegexRule>), ApiError> {
    if let Some(draft) = state.db.get_latest_draft_filter_version().await? {
        let b = row_to_bundle(&draft)?;
        return Ok((b.ad_phrases, b.ad_regexes));
    }
    if let Some(published) = state.db.get_published_filter_version().await? {
        let b = row_to_bundle(&published)?;
        return Ok((b.ad_phrases, b.ad_regexes));
    }
    let def = default_online_filter_bundle();
    Ok((def.ad_phrases, def.ad_regexes))
}

fn serialize_bundle_parts(bundle: &FilterBundle) -> Result<(String, String, String), ApiError> {
    let filter_json = serde_json::to_string(&bundle.policy)
        .map_err(|e| ApiError::internal("序列化过滤策略失败").with_detail(e.to_string()))?;
    let ad_phrases_json = serde_json::to_string(&bundle.ad_phrases)
        .map_err(|e| ApiError::internal("序列化广告短语失败").with_detail(e.to_string()))?;
    let ad_regexes_json = serde_json::to_string(&bundle.ad_regexes)
        .map_err(|e| ApiError::internal("序列化广告正则失败").with_detail(e.to_string()))?;
    Ok((filter_json, ad_phrases_json, ad_regexes_json))
}

pub async fn list_filter_presets_handler(
    _auth: AuthUser,
) -> Result<Json<FilterPresetCatalog>, ApiError> {
    Ok(Json(builtin_preset_catalog()))
}

pub async fn get_filter_state_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Result<Json<FilterStateResponse>, ApiError> {
    let published = match state.db.get_published_filter_version().await? {
        Some(row) => Some(row_to_dto(&row)?),
        None => None,
    };
    let draft = match state.db.get_latest_draft_filter_version().await? {
        Some(row) => Some(row_to_dto(&row)?),
        None => None,
    };

    let statuses = state
        .supervisor
        .list_worker_statuses()
        .await
        .map_err(|e| ApiError::internal("读取 Indexer 状态失败").with_detail(e.to_string()))?;
    let online_indexers = statuses
        .into_iter()
        .filter(|w| w.role == WorkerRole::Indexer)
        .map(|w| IndexerRuleStatusDto {
            id: w.id,
            display_name: w.display_name,
            runtime_state: w.runtime_state,
            pid: w.pid,
            active_rule_version: w.active_rule_version,
        })
        .collect();

    Ok(Json(FilterStateResponse {
        has_draft: draft.is_some(),
        published,
        draft,
        online_indexers,
    }))
}

pub async fn save_filter_draft_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(mut req): Json<SaveFilterDraftRequest>,
) -> Result<Json<FilterVersionDto>, ApiError> {
    ensure_rule_ids(&mut req.bundle);
    CompiledFilterEngine::compile(&req.bundle)?;

    let (filter_json, ad_phrases_json, ad_regexes_json) = serialize_bundle_parts(&req.bundle)?;
    let note = if req.note.trim().is_empty() {
        "草稿规则更新"
    } else {
        req.note.trim()
    };

    let saved = state
        .db
        .save_filter_draft(
            &filter_json,
            &ad_phrases_json,
            &ad_regexes_json,
            &auth.user.username,
            note,
        )
        .await?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "filter.draft.save",
            &format!("v{}", saved.version),
            &json!({ "counts": count_rules(&req.bundle), "note": note }).to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(row_to_dto(&saved)?))
}

pub async fn discard_filter_draft_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let discarded = state.db.discard_filter_draft().await?;
    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "filter.draft.discard",
            "draft",
            &json!({ "discarded": discarded }).to_string(),
            &extract_client_ip(&headers),
        )
        .await;
    Ok(Json(json!({ "ok": true, "discarded": discarded })))
}

pub async fn validate_filter_handler(
    _auth: AuthUser,
    Json(mut req): Json<ValidateFilterRequest>,
) -> Result<Json<ValidateFilterResponse>, ApiError> {
    ensure_rule_ids(&mut req.bundle);
    CompiledFilterEngine::compile(&req.bundle)?;
    Ok(Json(ValidateFilterResponse {
        ok: true,
        counts: count_rules(&req.bundle),
    }))
}

pub async fn import_legacy_filter_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<ImportLegacyFilterRequest>,
) -> Result<Json<ImportLegacyFilterResponse>, ApiError> {
    if req.raw_json.trim().is_empty() {
        return Err(ApiError::bad_request(
            "INVALID_INPUT",
            "导入的 JSON 内容不能为空",
        ));
    }

    let imported = import_any_filter_json(&req.raw_json)?;
    let (cur_phrases, cur_regexes) = load_current_dictionary(&state).await?;
    let mut bundle = FilterBundle {
        version: 0,
        policy: imported.policy,
        ad_phrases: imported.ad_phrases.unwrap_or(cur_phrases),
        ad_regexes: imported.ad_regexes.unwrap_or(cur_regexes),
    };
    ensure_rule_ids(&mut bundle);
    CompiledFilterEngine::compile(&bundle)?;

    let saved_draft = if req.save_as_draft {
        let (filter_json, ad_phrases_json, ad_regexes_json) = serialize_bundle_parts(&bundle)?;
        let note = if req.note.trim().is_empty() {
            match imported.source_format.as_str() {
                "hameta_v2" | "hameta_v2_policy" => "导入 Hameta v2 策略包",
                "hameta_v1" => "导入并升级 Hameta v1 规则包",
                _ => "导入并智能转换旧版 config.json",
            }
        } else {
            req.note.trim()
        };
        let row = state
            .db
            .save_filter_draft(
                &filter_json,
                &ad_phrases_json,
                &ad_regexes_json,
                &auth.user.username,
                note,
            )
            .await?;
        let dto = row_to_dto(&row)?;
        bundle.version = dto.version;
        let _ = state
            .db
            .record_audit(
                &auth.user.username,
                "filter.import",
                &format!("v{}", dto.version),
                &json!({
                    "source_format": imported.source_format,
                    "counts": count_rules(&bundle),
                })
                .to_string(),
                &extract_client_ip(&headers),
            )
            .await;
        Some(dto)
    } else {
        None
    };

    Ok(Json(ImportLegacyFilterResponse {
        source_format: imported.source_format,
        conversion_notes: imported.conversion_notes,
        bundle,
        saved_draft,
    }))
}

pub async fn reset_filter_preset_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<ResetPresetRequest>,
) -> Result<Json<FilterVersionDto>, ApiError> {
    let preset_id = req
        .preset_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("online_standard");

    let mut bundle = policy_preset_bundle(preset_id).ok_or_else(|| {
        ApiError::bad_request("INVALID_PRESET", format!("未知的内置过滤预设: {preset_id}"))
    })?;
    if !req.include_dictionary {
        let (ad_phrases, ad_regexes) = load_current_dictionary(&state).await?;
        bundle.ad_phrases = ad_phrases;
        bundle.ad_regexes = ad_regexes;
    }
    ensure_rule_ids(&mut bundle);
    CompiledFilterEngine::compile(&bundle)?;

    let (filter_json, ad_phrases_json, ad_regexes_json) = serialize_bundle_parts(&bundle)?;
    let note = if req.note.trim().is_empty() {
        format!("应用内置预设: {preset_id}")
    } else {
        req.note.trim().to_string()
    };
    let saved = state
        .db
        .save_filter_draft(
            &filter_json,
            &ad_phrases_json,
            &ad_regexes_json,
            &auth.user.username,
            &note,
        )
        .await?;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "filter.reset_preset",
            &format!("v{}", saved.version),
            &json!({
                "preset_id": preset_id,
                "include_dictionary": req.include_dictionary,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(row_to_dto(&saved)?))
}

pub async fn publish_filter_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<PublishFilterRequest>,
) -> Result<Json<PublishOrRollbackFilterResponse>, ApiError> {
    let Some(draft_row) = state.db.get_latest_draft_filter_version().await? else {
        return Err(ApiError::bad_request(
            "NO_DRAFT",
            "当前没有待发布的过滤规则草稿，请先保存草稿后再发布",
        ));
    };

    let mut bundle = row_to_bundle(&draft_row)?;
    ensure_rule_ids(&mut bundle);
    CompiledFilterEngine::compile(&bundle)?;

    let note_opt = req.note.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let published = state
        .db
        .publish_filter_draft(&auth.user.username, note_opt)
        .await?;
    bundle.version = published.version;

    let reload_results = state
        .supervisor
        .reload_all_indexers(published.version, &bundle)
        .await;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "filter.publish",
            &format!("v{}", published.version),
            &json!({
                "version": published.version,
                "reload_count": reload_results.len(),
                "reload_results": &reload_results,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(PublishOrRollbackFilterResponse {
        published: row_to_dto(&published)?,
        reload_results,
    }))
}

pub async fn list_filter_versions_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Query(query): Query<ListVersionsQuery>,
) -> Result<Json<Vec<FilterVersionDto>>, ApiError> {
    let limit = query.limit.unwrap_or(30).clamp(1, 100);
    let rows = state.db.list_filter_versions(limit).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(row_to_dto(row)?);
    }
    Ok(Json(items))
}

pub async fn rollback_filter_handler(
    State(state): State<Arc<PanelState>>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<RollbackFilterRequest>,
) -> Result<Json<PublishOrRollbackFilterResponse>, ApiError> {
    let published = state
        .db
        .rollback_filter_version(req.target_version, &auth.user.username)
        .await?;
    let mut bundle = row_to_bundle(&published)?;
    ensure_rule_ids(&mut bundle);
    CompiledFilterEngine::compile(&bundle)?;

    let reload_results = state
        .supervisor
        .reload_all_indexers(published.version, &bundle)
        .await;

    let _ = state
        .db
        .record_audit(
            &auth.user.username,
            "filter.rollback",
            &format!("v{}", published.version),
            &json!({
                "target_version": req.target_version,
                "new_version": published.version,
                "reload_results": &reload_results,
            })
            .to_string(),
            &extract_client_ip(&headers),
        )
        .await;

    Ok(Json(PublishOrRollbackFilterResponse {
        published: row_to_dto(&published)?,
        reload_results,
    }))
}

fn build_phrase_exclude_set(phrases: &[AdPhraseRule]) -> HashSet<String> {
    phrases
        .iter()
        .map(|p| p.phrase.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

pub async fn get_filter_stats_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
) -> Result<Json<FilterStatsResponse>, ApiError> {
    let statuses = state
        .supervisor
        .list_worker_statuses()
        .await
        .map_err(|e| ApiError::internal("读取 Indexer 状态失败").with_detail(e.to_string()))?;

    let mut total_evaluated = 0_u64;
    let mut total_accepted = 0_u64;
    let mut total_rejected = 0_u64;
    let mut merged_hits: HashMap<(FilterCategory, PolicyAction, String), RuleHitStat> =
        HashMap::new();
    let mut merged_samples: Vec<WorkerRejectedSampleDto> = Vec::new();
    let mut discovery_batches: Vec<Vec<DiscoveredAdCandidate>> = Vec::new();
    let mut workers = Vec::new();

    for w in statuses {
        if w.role != WorkerRole::Indexer {
            continue;
        }
        let mut w_eval = 0_u64;
        let mut w_acc = 0_u64;
        let mut w_rej = 0_u64;

        if let Some(env) = &w.snapshot
            && let Some(filter_val) = env.payload.get("filter")
            && let Ok(snap) = serde_json::from_value::<FilterStatsSnapshot>(filter_val.clone())
        {
            w_eval = snap.total_evaluated;
            w_acc = snap.total_accepted;
            w_rej = snap.total_rejected;

            total_evaluated = total_evaluated.saturating_add(snap.total_evaluated);
            total_accepted = total_accepted.saturating_add(snap.total_accepted);
            total_rejected = total_rejected.saturating_add(snap.total_rejected);

            for stat in snap.rule_hits {
                let key = (stat.category, stat.action, stat.rule_id.clone());
                let entry = merged_hits.entry(key).or_insert_with(|| RuleHitStat {
                    category: stat.category,
                    action: stat.action,
                    rule_id: stat.rule_id,
                    rule_label: stat.rule_label.clone(),
                    hits: 0,
                });
                entry.hits = entry.hits.saturating_add(stat.hits);
                if !stat.rule_label.is_empty() {
                    entry.rule_label = stat.rule_label;
                }
            }

            for sample in snap.recent_rejected {
                merged_samples.push(WorkerRejectedSampleDto {
                    worker_id: w.id.clone(),
                    sample,
                });
            }

            if !snap.discovered_ad_candidates.is_empty() {
                discovery_batches.push(snap.discovered_ad_candidates);
            }
        }

        workers.push(IndexerStatBrief {
            id: w.id,
            display_name: w.display_name,
            runtime_state: w.runtime_state,
            active_rule_version: w.active_rule_version,
            total_evaluated: w_eval,
            total_accepted: w_acc,
            total_rejected: w_rej,
        });
    }

    let mut rule_hits: Vec<RuleHitStat> = merged_hits.into_values().collect();
    rule_hits.sort_by(|a, b| b.hits.cmp(&a.hits).then_with(|| a.rule_id.cmp(&b.rule_id)));
    rule_hits.truncate(50);

    merged_samples.sort_by(|a, b| {
        b.sample
            .rejected_at_unix
            .cmp(&a.sample.rejected_at_unix)
            .then_with(|| a.sample.info_hash.cmp(&b.sample.info_hash))
    });
    merged_samples.truncate(200);

    let exclude_lower = match load_current_dictionary(&state).await {
        Ok((phrases, _)) => build_phrase_exclude_set(&phrases),
        Err(_) => HashSet::new(),
    };
    let discovered_ad_candidates =
        merge_discovered_candidates(discovery_batches, &exclude_lower, 30);

    Ok(Json(FilterStatsResponse {
        total_evaluated,
        total_accepted,
        total_rejected,
        rule_hits,
        recent_rejected: merged_samples,
        workers,
        discovered_ad_candidates,
    }))
}

pub async fn discover_ad_candidates_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Json(req): Json<DiscoverCandidatesRequest>,
) -> Result<Json<DiscoverCandidatesResponse>, ApiError> {
    let limit = req.limit.unwrap_or(300).clamp(20, 500);

    let current_bundle = if let Some(draft) = state.db.get_latest_draft_filter_version().await? {
        row_to_bundle(&draft)?
    } else if let Some(published) = state.db.get_published_filter_version().await? {
        row_to_bundle(&published)?
    } else {
        default_online_filter_bundle()
    };
    let exclude_lower = build_phrase_exclude_set(&current_bundle.ad_phrases);
    let cleaner = TitleCleaner::compile_with_options(
        &current_bundle.ad_phrases,
        &current_bundle.ad_regexes,
        &current_bundle.policy.cleaner,
    )
    .unwrap_or_else(|_| TitleCleaner::default_baseline());

    let mut discovery_batches: Vec<Vec<DiscoveredAdCandidate>> = Vec::new();
    let statuses = state
        .supervisor
        .list_worker_statuses()
        .await
        .unwrap_or_default();
    for w in statuses {
        if w.role != WorkerRole::Indexer {
            continue;
        }
        if let Some(env) = &w.snapshot
            && let Some(filter_val) = env.payload.get("filter")
            && let Ok(snap) = serde_json::from_value::<FilterStatsSnapshot>(filter_val.clone())
            && !snap.discovered_ad_candidates.is_empty()
        {
            discovery_batches.push(snap.discovered_ad_candidates);
        }
    }

    let mut scanned_titles = 0_usize;
    let mut source = "indexer_live".to_string();

    if let Some((_, client)) = state.services.manticore_client().await {
        let body = json!({
            "index": "torrents",
            "query": { "match_all": {} },
            "sort": [{ "updated_at": { "order": "desc" } }],
            "limit": limit,
            "options": { "max_matches": limit.max(1000) }
        });
        if let Ok(resp) = client.search_json(&body).await
            && let Some(hits) = resp["hits"]["hits"].as_array()
        {
            let now_unix = Utc::now().timestamp();
            let mut tracker = AdDiscoveryTracker::new();
            for hit in hits {
                let raw_name = hit["_source"]["raw_name"].as_str().unwrap_or_default();
                if raw_name.trim().is_empty() {
                    continue;
                }
                scanned_titles += 1;
                let cleaned = cleaner.refine(raw_name);
                tracker.observe(&cleaned, now_unix);
            }
            let manticore_cands = tracker.snapshot_candidates(30, &exclude_lower);
            if !manticore_cands.is_empty() {
                discovery_batches.push(manticore_cands);
            }
            source = if discovery_batches.is_empty() || scanned_titles == 0 {
                source
            } else {
                "manticore_and_live".to_string()
            };
        }
    }

    let candidates = merge_discovered_candidates(discovery_batches, &exclude_lower, 30);
    Ok(Json(DiscoverCandidatesResponse {
        source,
        scanned_titles,
        candidates,
    }))
}

fn build_playground_message(
    input: &PlaygroundInput,
    auto_placeholder_file: bool,
) -> TorrentMessage {
    let mut normalized = input.clone();
    if normalized.files.is_empty() && auto_placeholder_file {
        normalized.files = vec![FileInfo {
            path: "sample/video.mkv".to_string(),
            size: 100 * 1024 * 1024,
        }];
    }
    normalized.to_torrent_message()
}

pub async fn playground_test_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Json(req): Json<PlaygroundTestRequest>,
) -> Result<Json<PlaygroundTestResponse>, ApiError> {
    if req.samples.is_empty() {
        return Err(ApiError::bad_request(
            "INVALID_INPUT",
            "请至少提供一条测试样本",
        ));
    }
    if req.samples.len() > 200 {
        return Err(ApiError::bad_request(
            "TOO_MANY_SAMPLES",
            "演练场单次最多支持 200 条样本",
        ));
    }

    let mut published_bundle = load_published_bundle(&state).await?;
    ensure_rule_ids(&mut published_bundle);
    let published_engine = CompiledFilterEngine::compile(&published_bundle)?;

    let messages: Vec<TorrentMessage> = req
        .samples
        .iter()
        .map(|s| build_playground_message(s, req.auto_placeholder_file))
        .collect();

    if req.mode == PlaygroundMode::Published {
        let results = evaluate_playground_batch(&published_engine, &messages);
        return Ok(Json(PlaygroundTestResponse {
            mode: "published".to_string(),
            published_version: published_bundle.version,
            draft_version: None,
            results,
            ab_report: None,
        }));
    }

    let mut draft_bundle = match req.draft_bundle {
        Some(b) => b,
        None => match state.db.get_latest_draft_filter_version().await? {
            Some(row) => row_to_bundle(&row)?,
            None => published_bundle.clone(),
        },
    };
    ensure_rule_ids(&mut draft_bundle);
    let draft_engine = CompiledFilterEngine::compile(&draft_bundle)?;
    let draft_version = Some(draft_engine.version());

    let results = evaluate_playground_batch(&draft_engine, &messages);
    let ab_report = if req.mode == PlaygroundMode::AbCompare {
        Some(compare_playground_ab(
            &published_engine,
            &draft_engine,
            &messages,
        ))
    } else {
        None
    };

    Ok(Json(PlaygroundTestResponse {
        mode: match req.mode {
            PlaygroundMode::Published => "published",
            PlaygroundMode::Draft => "draft",
            PlaygroundMode::AbCompare => "ab_compare",
        }
        .to_string(),
        published_version: published_bundle.version,
        draft_version,
        results,
        ab_report,
    }))
}

fn synthesize_files_from_summary(total_size: u64, file_count: usize) -> Vec<FileInfo> {
    if file_count == 0 {
        return Vec::new();
    }
    let count = file_count.min(2500);
    let per_size = total_size / u64::try_from(count).unwrap_or(1).max(1);
    let remainder = total_size % u64::try_from(count).unwrap_or(1).max(1);
    (0..count)
        .map(|idx| FileInfo {
            path: format!("sample/file-{}.bin", idx + 1),
            size: if idx == 0 {
                per_size.saturating_add(remainder)
            } else {
                per_size
            },
        })
        .collect()
}

fn parse_files_from_source(src: &Value) -> Vec<FileInfo> {
    match src.get("file_info") {
        Some(Value::String(s)) => serde_json::from_str::<Vec<FileInfo>>(s).unwrap_or_default(),
        Some(arr @ Value::Array(_)) => {
            serde_json::from_value::<Vec<FileInfo>>(arr.clone()).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

pub async fn playground_sample_handler(
    State(state): State<Arc<PanelState>>,
    _auth: AuthUser,
    Json(req): Json<PlaygroundSampleRequest>,
) -> Result<Json<PlaygroundSampleResponse>, ApiError> {
    let limit = req.limit.unwrap_or(30).clamp(1, 100);
    let kw_filter = req
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);

    match req.source {
        SampleSource::RejectedRecent => {
            let statuses = state.supervisor.list_worker_statuses().await.map_err(|e| {
                ApiError::internal("读取 Indexer 状态失败").with_detail(e.to_string())
            })?;
            let mut raw_samples: Vec<RejectedSample> = Vec::new();
            for w in statuses {
                if w.role != WorkerRole::Indexer {
                    continue;
                }
                if let Some(env) = &w.snapshot
                    && let Some(filter_val) = env.payload.get("filter")
                    && let Ok(snap) =
                        serde_json::from_value::<FilterStatsSnapshot>(filter_val.clone())
                {
                    raw_samples.extend(snap.recent_rejected);
                }
            }
            raw_samples.sort_by_key(|a| std::cmp::Reverse(a.rejected_at_unix));

            let samples: Vec<PlaygroundInput> = raw_samples
                .into_iter()
                .filter(|s| {
                    kw_filter.as_ref().is_none_or(|kw| {
                        s.raw_name.to_lowercase().contains(kw)
                            || s.info_hash.to_lowercase().contains(kw)
                    })
                })
                .take(limit)
                .map(|s| PlaygroundInput {
                    info_hash: s.info_hash,
                    name: s.raw_name,
                    files: synthesize_files_from_summary(s.total_size, s.file_count),
                })
                .collect();

            Ok(Json(PlaygroundSampleResponse {
                source: "rejected_recent".to_string(),
                samples,
            }))
        }
        SampleSource::ManticoreLatest | SampleSource::ManticoreRandom => {
            let Some((_, client)) = state.services.manticore_client().await else {
                return Err(ApiError::bad_request(
                    "MANTICORE_NOT_CONFIGURED",
                    "尚未配置或连接 Manticore 服务",
                ));
            };

            let query_obj = match req
                .keyword
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(kw) => json!({ "match": { "raw_name,refine_name": kw } }),
                None => json!({ "match_all": {} }),
            };

            let fetch_limit = if req.source == SampleSource::ManticoreRandom {
                (limit.saturating_mul(3)).clamp(limit, 100)
            } else {
                limit
            };

            let offset = if req.source == SampleSource::ManticoreRandom && kw_filter.is_none() {
                if let Ok(status) = client.table_status().await {
                    let docs = usize::try_from(status.indexed_documents.unwrap_or(0)).unwrap_or(0);
                    let window = docs.min(1000).saturating_sub(fetch_limit);
                    let seed = usize::try_from(Utc::now().timestamp_subsec_nanos()).unwrap_or(0);
                    seed.checked_rem(window).unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            };

            let max_matches = (offset + fetch_limit).max(1000);
            let body = json!({
                "index": "torrents",
                "query": query_obj,
                "sort": [{ "updated_at": { "order": "desc" } }],
                "limit": fetch_limit,
                "offset": offset,
                "options": { "max_matches": max_matches }
            });
            let resp = client
                .search_json(&body)
                .await
                .map_err(|e| ApiError::bad_request("MANTICORE_SEARCH_ERROR", e.to_string()))?;

            let mut candidates: Vec<PlaygroundInput> = Vec::new();
            if let Some(hits) = resp["hits"]["hits"].as_array() {
                for hit in hits {
                    let src = &hit["_source"];
                    let info_hash = src["info_hash"].as_str().unwrap_or_default().to_string();
                    let raw_name = src["raw_name"].as_str().unwrap_or_default().to_string();
                    if raw_name.is_empty() {
                        continue;
                    }
                    let files = parse_files_from_source(src);
                    candidates.push(PlaygroundInput {
                        info_hash,
                        name: raw_name,
                        files,
                    });
                }
            }

            if req.source == SampleSource::ManticoreRandom && candidates.len() > limit {
                let n = candidates.len();
                let seed = usize::try_from(Utc::now().timestamp_subsec_nanos()).unwrap_or(17);
                let mut picked = Vec::with_capacity(limit);
                let step = (n / limit).max(1);
                let mut idx = seed % n;
                let mut seen = vec![false; n];
                while picked.len() < limit {
                    while seen[idx] {
                        idx = (idx + 1) % n;
                    }
                    seen[idx] = true;
                    picked.push(candidates[idx].clone());
                    idx = (idx + step) % n;
                }
                candidates = picked;
            } else {
                candidates.truncate(limit);
            }

            Ok(Json(PlaygroundSampleResponse {
                source: match req.source {
                    SampleSource::ManticoreLatest => "manticore_latest",
                    SampleSource::ManticoreRandom => "manticore_random",
                    SampleSource::RejectedRecent => "rejected_recent",
                }
                .to_string(),
                samples: candidates,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_and_reimport_v2_bundle_roundtrips() {
        let mut bundle = default_online_filter_bundle();
        let raw_v2 = serde_json::to_string_pretty(&bundle).unwrap();
        let imported = import_any_filter_json(&raw_v2).unwrap();
        assert_eq!(imported.source_format, "hameta_v2");
        bundle.version = imported.policy.schema_version as i64;
        assert_eq!(imported.policy, bundle.policy);
        let counts = count_rules(&bundle);
        assert!(counts.total_rules >= 10);
        assert_eq!(counts.allow_rules, 0);
    }
}
