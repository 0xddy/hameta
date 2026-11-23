import { apiRequest } from "./client";
import type { WorkerRuntimeState } from "./overview";

export type FilterCategory =
  | "empty_name"
  | "title_length"
  | "empty_files"
  | "all_empty_files"
  | "max_empty_files"
  | "max_file_count"
  | "max_file_name_length"
  | "min_total_size"
  | "max_total_size"
  | "file_density"
  | "keyword_group"
  | "extension_rule"
  | "title_pattern_preset"
  | "file_constraint"
  | "substantive_content"
  | "size_ratio"
  | "custom_regex"
  | "composite"
  | "empty_refined_name";

export type PolicyAction = "reject" | "allow";
export type TextTarget = "raw_name" | "refined_name" | "any_file_path" | "any_file_name";
export type KeywordMatchMode = "contains" | "word_boundary";
export type ExtensionScope = "torrent_name" | "any_file" | "all_files";
export type TitlePatternKind =
  | "cyrillic"
  | "digits_only"
  | "sha1_hex"
  | "md5_hex"
  | "uuid"
  | "decimal_code"
  | "magnet_prefix"
  | "http_url_prefix"
  | "split_archive_suffix";
export type FileConstraintMode = "every_matched" | "at_least_one_matched";
export type CompositeLogic = "all" | "any";

export interface ToggleUsize {
  enabled: boolean;
  value: number;
}

export interface ToggleU64 {
  enabled: boolean;
  value: number;
}

export interface DensityGate {
  enabled: boolean;
  min_file_count: number;
  min_avg_file_size: number;
}

export interface FilterGates {
  reject_empty_name: boolean;
  min_name_length: ToggleUsize;
  max_name_length: ToggleUsize;
  reject_empty_files: boolean;
  reject_all_empty_files: boolean;
  max_empty_files: ToggleUsize;
  max_file_count: ToggleUsize;
  max_file_name_length: ToggleUsize;
  min_total_size: ToggleU64;
  max_total_size: ToggleU64;
  density_check: DensityGate;
}

export interface CleanerOptions {
  enable_nfkc: boolean;
  enable_confusable_norm: boolean;
  enable_deobfuscate_brackets: boolean;
  enable_collapse_spaced_chars: boolean;
  enable_strip_leading_site_prefix: boolean;
  enable_strip_urls_and_domains: boolean;
  enable_ad_regexes: boolean;
  enable_ad_phrases: boolean;
  enable_span_ad_scoring: boolean;
  span_ad_threshold: number;
  enable_filename_cross_clean: boolean;
  enable_cleanup: boolean;
  reject_if_empty_after_clean: boolean;
  min_refined_name_length: ToggleUsize;
}

export type RuleMatcher =
  | {
      type: "keyword_group";
      target: TextTarget;
      match_mode: KeywordMatchMode;
      case_sensitive: boolean;
      words: string[];
    }
  | {
      type: "extension_rule";
      scope: ExtensionScope;
      extensions: string[];
    }
  | {
      type: "title_pattern_preset";
      patterns: TitlePatternKind[];
      min_cyrillic_chars: number;
    }
  | {
      type: "file_constraint";
      extensions: string[];
      extra_regexes: string[];
      mode: FileConstraintMode;
      min_file_size: number | null;
      max_file_size: number | null;
      min_file_name_length: number | null;
      max_file_name_length: number | null;
    }
  | {
      type: "substantive_content";
      junk_extensions: string[];
      include_system_junk: boolean;
      extra_junk_regexes: string[];
      min_files: number;
      min_total_size: number;
    }
  | {
      type: "size_ratio";
      extensions: string[];
      include_split_archives: boolean;
      extra_regexes: string[];
      max_ratio: number;
    }
  | {
      type: "custom_regex";
      target: TextTarget;
      match_all: boolean;
      patterns: string[];
    }
  | {
      type: "composite";
      logic: CompositeLogic;
      conditions: RuleMatcher[];
    };

export interface PolicyRule {
  id: string;
  name: string;
  enabled: boolean;
  action: PolicyAction;
  tags: string[];
  note: string;
  matcher: RuleMatcher;
}

export interface FilterPolicy {
  schema_version: number;
  preset_id: string | null;
  gates: FilterGates;
  cleaner: CleanerOptions;
  rules: PolicyRule[];
}

export interface AdPhraseRule {
  id: string;
  phrase: string;
  enabled: boolean;
  note: string;
}

export interface AdRegexRule {
  id: string;
  pattern: string;
  replacement: string;
  enabled: boolean;
  note: string;
}

export interface FilterBundle {
  version: number;
  policy: FilterPolicy;
  ad_phrases: AdPhraseRule[];
  ad_regexes: AdRegexRule[];
}

export interface FilterVersionDto {
  id: number;
  version: number;
  status: "draft" | "published" | "archived";
  bundle: FilterBundle;
  created_by: string;
  note: string;
  created_at: number;
  published_at: number | null;
}

export interface IndexerRuleStatusDto {
  id: string;
  display_name: string;
  runtime_state: WorkerRuntimeState;
  pid: number | null;
  active_rule_version: number | null;
}

export interface FilterStateResponse {
  published: FilterVersionDto | null;
  draft: FilterVersionDto | null;
  has_draft: boolean;
  online_indexers: IndexerRuleStatusDto[];
}

export interface IndexerReloadResult {
  instance_id: string;
  ok: boolean;
  active_rule_version: number | null;
  message: string;
}

export interface PublishOrRollbackFilterResponse {
  published: FilterVersionDto;
  reload_results: IndexerReloadResult[];
}

export interface ValidateFilterResponse {
  ok: boolean;
  counts: {
    total_rules: number;
    enabled_rules: number;
    reject_rules: number;
    allow_rules: number;
    enabled_gates: number;
    ad_phrases: number;
    ad_regexes: number;
  };
}

export interface ImportLegacyFilterResponse {
  source_format: string;
  conversion_notes: string[];
  bundle: FilterBundle;
  saved_draft: FilterVersionDto | null;
}

export interface PolicyPresetInfo {
  id: string;
  name: string;
  summary: string;
  badge: string;
  rule_count: number;
  allow_count: number;
  reject_count: number;
  bundle: FilterBundle;
}

export interface RuleTemplateInfo {
  id: string;
  name: string;
  group: string;
  description: string;
  rule: PolicyRule;
}

export interface ExtensionPackInfo {
  id: string;
  name: string;
  description: string;
  extensions: string[];
}

export interface KeywordPackInfo {
  id: string;
  name: string;
  description: string;
  words: string[];
}

export interface FilterPresetsResponse {
  policy_presets: PolicyPresetInfo[];
  rule_templates: RuleTemplateInfo[];
  extension_packs: ExtensionPackInfo[];
  keyword_packs: KeywordPackInfo[];
}

export interface RuleHit {
  category: FilterCategory;
  action: PolicyAction;
  rule_id: string;
  rule_label: string;
  detail: string;
}

export interface RuleHitStat {
  category: FilterCategory;
  action: PolicyAction;
  rule_id: string;
  rule_label: string;
  hits: number;
}

export interface WorkerRejectedSampleDto {
  worker_id: string;
  info_hash: string;
  raw_name: string;
  total_size: number;
  file_count: number;
  hit: RuleHit;
  rejected_at_unix: number;
}

export interface IndexerStatBrief {
  id: string;
  display_name: string;
  runtime_state: WorkerRuntimeState;
  active_rule_version: number | null;
  total_evaluated: number;
  total_accepted: number;
  total_rejected: number;
}

export interface DiscoveredAdCandidate {
  phrase: string;
  occurrences: number;
  distinct_titles: number;
  suggested_score: number;
  sample_titles: string[];
  last_seen_unix: number;
}

export interface DiscoverCandidatesResponse {
  source: string;
  scanned_titles: number;
  candidates: DiscoveredAdCandidate[];
}

export interface FilterStatsResponse {
  total_evaluated: number;
  total_accepted: number;
  total_rejected: number;
  rule_hits: RuleHitStat[];
  recent_rejected: WorkerRejectedSampleDto[];
  workers: IndexerStatBrief[];
  discovered_ad_candidates?: DiscoveredAdCandidate[];
}

export interface PlaygroundFileInfo {
  path: string;
  size: number;
}

export interface PlaygroundInput {
  info_hash?: string;
  name: string;
  files?: PlaygroundFileInfo[];
}

export interface CleaningStep {
  stage: string;
  enabled: boolean;
  before: string;
  after: string;
  changed: boolean;
  detail?: string | null;
}

export interface CleaningTrace {
  raw_name: string;
  refined_name: string;
  steps: CleaningStep[];
}

export interface PlaygroundItemResult {
  sample_index: number;
  info_hash: string;
  raw_name: string;
  accepted: boolean;
  refined_name: string;
  hit: RuleHit | null;
  allowed_by: RuleHit | null;
  cleaning_trace: CleaningTrace;
}

export type AbDiffStatus =
  | "unchanged_accepted"
  | "unchanged_rejected"
  | "newly_rejected"
  | "newly_accepted"
  | "refined_name_changed";

export interface AbComparisonItem {
  sample_index: number;
  info_hash: string;
  raw_name: string;
  diff_status: AbDiffStatus;
  published_accepted: boolean;
  published_refined_name: string;
  published_hit: RuleHit | null;
  published_allowed_by: RuleHit | null;
  draft_accepted: boolean;
  draft_refined_name: string;
  draft_hit: RuleHit | null;
  draft_allowed_by: RuleHit | null;
}

export interface AbComparisonSummary {
  total: number;
  unchanged_accepted: number;
  unchanged_rejected: number;
  newly_rejected: number;
  newly_accepted: number;
  refined_name_changed: number;
}

export interface AbComparisonReport {
  published_version: number;
  draft_version: number;
  summary: AbComparisonSummary;
  items: AbComparisonItem[];
}

export interface PlaygroundTestResponse {
  mode: "published" | "draft" | "ab_compare";
  published_version: number;
  draft_version: number | null;
  results: PlaygroundItemResult[];
  ab_report: AbComparisonReport | null;
}

export interface PlaygroundSampleResponse {
  source: "manticore_latest" | "manticore_random" | "rejected_recent";
  samples: PlaygroundInput[];
}

export function getFilterState(): Promise<FilterStateResponse> {
  return apiRequest<FilterStateResponse>("/api/v1/filter/state");
}

export function getFilterPresets(): Promise<FilterPresetsResponse> {
  return apiRequest<FilterPresetsResponse>("/api/v1/filter/presets");
}

export function saveFilterDraft(
  bundle: FilterBundle,
  note: string,
): Promise<FilterVersionDto> {
  return apiRequest<FilterVersionDto>("/api/v1/filter/draft", {
    method: "PUT",
    body: JSON.stringify({ bundle, note }),
  });
}

export function discardFilterDraft(): Promise<{ ok: boolean; discarded: boolean }> {
  return apiRequest("/api/v1/filter/draft", { method: "DELETE" });
}

export function validateFilterBundle(bundle: FilterBundle): Promise<ValidateFilterResponse> {
  return apiRequest<ValidateFilterResponse>("/api/v1/filter/validate", {
    method: "POST",
    body: JSON.stringify({ bundle }),
  });
}

export function importLegacyFilter(
  rawJson: string,
  saveAsDraft = true,
  note = "",
): Promise<ImportLegacyFilterResponse> {
  return apiRequest<ImportLegacyFilterResponse>("/api/v1/filter/import-legacy", {
    method: "POST",
    body: JSON.stringify({
      raw_json: rawJson,
      save_as_draft: saveAsDraft,
      note,
    }),
  });
}

export function resetFilterPreset(
  presetId = "online_standard",
  includeDictionary = true,
  note = "",
): Promise<FilterVersionDto> {
  return apiRequest<FilterVersionDto>("/api/v1/filter/reset-preset", {
    method: "POST",
    body: JSON.stringify({
      preset_id: presetId,
      include_dictionary: includeDictionary,
      note,
    }),
  });
}

export function publishFilterDraft(
  note?: string,
): Promise<PublishOrRollbackFilterResponse> {
  return apiRequest<PublishOrRollbackFilterResponse>("/api/v1/filter/publish", {
    method: "POST",
    body: JSON.stringify({ note: note || null }),
  });
}

export function listFilterVersions(limit = 30): Promise<FilterVersionDto[]> {
  return apiRequest<FilterVersionDto[]>(`/api/v1/filter/versions?limit=${limit}`);
}

export function rollbackFilterVersion(
  targetVersion: number,
): Promise<PublishOrRollbackFilterResponse> {
  return apiRequest<PublishOrRollbackFilterResponse>("/api/v1/filter/rollback", {
    method: "POST",
    body: JSON.stringify({ target_version: targetVersion }),
  });
}

export function getFilterStats(): Promise<FilterStatsResponse> {
  return apiRequest<FilterStatsResponse>("/api/v1/filter/stats");
}

export function runPlaygroundTest(payload: {
  mode: "published" | "draft" | "ab_compare";
  draft_bundle?: FilterBundle;
  samples: PlaygroundInput[];
  auto_placeholder_file?: boolean;
}): Promise<PlaygroundTestResponse> {
  return apiRequest<PlaygroundTestResponse>("/api/v1/filter/playground/test", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export function samplePlaygroundTorrents(payload: {
  source: "manticore_latest" | "manticore_random" | "rejected_recent";
  limit?: number;
  keyword?: string;
}): Promise<PlaygroundSampleResponse> {
  return apiRequest<PlaygroundSampleResponse>("/api/v1/filter/playground/sample", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

export function discoverAdCandidates(limit = 300): Promise<DiscoverCandidatesResponse> {
  return apiRequest<DiscoverCandidatesResponse>("/api/v1/filter/discover-candidates", {
    method: "POST",
    body: JSON.stringify({ limit }),
  });
}
