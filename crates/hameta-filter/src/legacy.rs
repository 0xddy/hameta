use crate::error::FilterError;
use crate::preset::{
    ARCHIVE_EXTS, BAIT_KEYWORDS, CRACK_KEYWORDS, DANGEROUS_EXEC_FILE_EXTS, DOC_EXTS,
    EXECUTABLE_TITLE_EXTS, MALWARE_KEYWORDS, PLACEHOLDER_KEYWORDS, SHORTCUT_JUNK_EXTS,
    default_online_filter_policy,
};
use crate::schema::{
    AdPhraseRule, AdRegexRule, CleanerOptions, DensityGate, ExtensionScope,
    FILTER_POLICY_SCHEMA_VERSION, FileConstraintMode, FilterBundle, FilterGates, FilterPolicy,
    KeywordMatchMode, PolicyAction, PolicyRule, RuleMatcher, TextTarget, TitlePatternKind,
    ToggleU64, ToggleUsize, normalize_extension,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LegacyLimitsConfig {
    pub min_name_length: usize,
    pub max_name_length: usize,
    pub max_total_size: u64,
    pub min_file_count_for_density_check: usize,
    pub min_avg_file_size: u64,
    pub max_file_name_length: usize,
    pub max_file_count: usize,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyKeywordItem {
    id: String,
    value: String,
    enabled: Option<bool>,
    note: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyPatternItem {
    id: String,
    pattern: String,
    enabled: Option<bool>,
    note: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyCustomRuleConfig {
    id: String,
    name: String,
    patterns: Vec<String>,
    min_name_length: usize,
    max_name_length: usize,
    min_file_size: u64,
    max_file_size: u64,
    enabled: Option<bool>,
    note: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyTorrentRuleConfig {
    id: String,
    name: String,
    junk_patterns: Vec<String>,
    min_substantive_files: usize,
    min_substantive_total_size: u64,
    max_empty_files: usize,
    reject_if_all_empty: bool,
    min_total_size: u64,
    archive_patterns: Vec<String>,
    max_archive_size_ratio: f64,
    enabled: Option<bool>,
    note: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum KeywordEntry {
    Plain(String),
    Struct(LegacyKeywordItem),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PatternEntry {
    Plain(String),
    Struct(LegacyPatternItem),
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct MixedLegacyFilterConfig {
    blocked_rules: Vec<KeywordEntry>,
    blocked_name_patterns: Vec<PatternEntry>,
    blocked_file_patterns: Vec<PatternEntry>,
    custom_rules: Vec<LegacyCustomRuleConfig>,
    torrent_rules: Vec<LegacyTorrentRuleConfig>,
    limits: LegacyLimitsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedFilterData {
    pub source_format: String,
    pub policy: FilterPolicy,
    pub ad_phrases: Option<Vec<AdPhraseRule>>,
    pub ad_regexes: Option<Vec<AdRegexRule>>,
    pub conversion_notes: Vec<String>,
}

/// 解析并智能转换旧版 `torrent-indexer/config.json`（既支持完整配置根对象，也支持单独的 `filter` 对象）。
pub fn parse_legacy_filter_json(raw_json: &str) -> Result<FilterPolicy, FilterError> {
    let value: serde_json::Value = serde_json::from_str(raw_json)
        .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| FilterError::InvalidLegacyJson("JSON 根节点必须是对象".to_string()))?;

    let filter_value = match object.get("filter") {
        Some(inner) => {
            if !inner.is_object() {
                return Err(FilterError::InvalidLegacyJson(
                    "filter 字段必须是对象".to_string(),
                ));
            }
            inner.clone()
        }
        None => value,
    };

    let mixed: MixedLegacyFilterConfig = serde_json::from_value(filter_value)
        .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
    let (policy, _notes) = convert_mixed_to_v2(&mixed, Some("legacy_imported"));
    Ok(policy)
}

/// 解析数据库存储的 `filter_json`（自动兼容 v2 `FilterPolicy` 与历史 v1 `FilterRuleSet`）。
pub fn parse_stored_policy_json(raw_json: &str) -> Result<FilterPolicy, FilterError> {
    let value: serde_json::Value = serde_json::from_str(raw_json)
        .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| FilterError::InvalidLegacyJson("存储规则 JSON 必须是对象".to_string()))?;

    if obj.contains_key("gates") || obj.contains_key("schema_version") {
        let mut policy: FilterPolicy = serde_json::from_value(value)
            .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
        if policy.schema_version == 0 {
            policy.schema_version = FILTER_POLICY_SCHEMA_VERSION;
        }
        return Ok(policy);
    }

    let mixed: MixedLegacyFilterConfig =
        serde_json::from_value(value).map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
    let (policy, _notes) = convert_mixed_to_v2(&mixed, Some("v1_migrated"));
    Ok(policy)
}

/// 自动识别并导入任意格式 JSON：Hameta v2 完整数据包 / 策略、Hameta v1 规则包、或旧版 `config.json`。
pub fn import_any_filter_json(raw_json: &str) -> Result<ImportedFilterData, FilterError> {
    let value: serde_json::Value = serde_json::from_str(raw_json)
        .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| FilterError::InvalidLegacyJson("JSON 根节点必须是对象".to_string()))?;

    // 1. Hameta v2 FilterBundle（包含 policy 对象）
    if let Some(policy_val) = obj.get("policy") {
        if !policy_val.is_object() {
            return Err(FilterError::InvalidLegacyJson(
                "policy 字段必须是对象".to_string(),
            ));
        }
        let has_phrases = obj.contains_key("ad_phrases");
        let has_regexes = obj.contains_key("ad_regexes");
        let bundle: FilterBundle = serde_json::from_value(value)
            .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
        let ad_phrases = has_phrases.then_some(bundle.ad_phrases);
        let ad_regexes = has_regexes.then_some(bundle.ad_regexes);
        let rule_count = bundle.policy.rules.len();
        return Ok(ImportedFilterData {
            source_format: "hameta_v2".to_string(),
            policy: bundle.policy,
            ad_phrases,
            ad_regexes,
            conversion_notes: vec![format!(
                "已直接载入 Hameta v2 策略包（{rule_count} 条规则）"
            )],
        });
    }

    // 2. 单独的 Hameta v2 FilterPolicy（包含 gates 对象）
    if obj.contains_key("gates") {
        let mut policy: FilterPolicy = serde_json::from_value(value)
            .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
        if policy.schema_version == 0 {
            policy.schema_version = FILTER_POLICY_SCHEMA_VERSION;
        }
        let rule_count = policy.rules.len();
        return Ok(ImportedFilterData {
            source_format: "hameta_v2_policy".to_string(),
            policy,
            ad_phrases: None,
            ad_regexes: None,
            conversion_notes: vec![format!("已直接载入 Hameta v2 策略（{rule_count} 条规则）")],
        });
    }

    // 3. Hameta v1 FilterBundle（包含 rules 对象）
    if let Some(rules_val) = obj.get("rules").filter(|v| v.is_object()) {
        let mixed: MixedLegacyFilterConfig = serde_json::from_value(rules_val.clone())
            .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
        let (policy, mut notes) = convert_mixed_to_v2(&mixed, Some("v1_imported"));
        let ad_phrases: Option<Vec<AdPhraseRule>> = obj
            .get("ad_phrases")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
        let ad_regexes: Option<Vec<AdRegexRule>> = obj
            .get("ad_regexes")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
        notes.insert(
            0,
            "检测到 Hameta v1 规则包格式，已智能升级为 v2 策略结构".to_string(),
        );
        return Ok(ImportedFilterData {
            source_format: "hameta_v1".to_string(),
            policy,
            ad_phrases,
            ad_regexes,
            conversion_notes: notes,
        });
    }

    // 4. 旧版 torrent-indexer config.json 或裸 filter 对象
    let filter_value = match obj.get("filter") {
        Some(inner) => {
            if !inner.is_object() {
                return Err(FilterError::InvalidLegacyJson(
                    "filter 字段必须是对象".to_string(),
                ));
            }
            inner.clone()
        }
        None => value,
    };

    let mixed: MixedLegacyFilterConfig = serde_json::from_value(filter_value)
        .map_err(|e| FilterError::InvalidLegacyJson(e.to_string()))?;
    let (policy, mut notes) = convert_mixed_to_v2(&mixed, Some("legacy_imported"));
    notes.insert(
        0,
        "检测到旧版 filter JSON，已智能转译为 v2 友好策略卡片".to_string(),
    );
    Ok(ImportedFilterData {
        source_format: "legacy_config".to_string(),
        policy,
        ad_phrases: None,
        ad_regexes: None,
        conversion_notes: notes,
    })
}

fn convert_mixed_to_v2(
    mixed: &MixedLegacyFilterConfig,
    preset_tag: Option<&str>,
) -> (FilterPolicy, Vec<String>) {
    let mut notes = Vec::new();
    let mut rules = Vec::new();

    // 1. 门禁转换
    let mut gates = FilterGates {
        reject_empty_name: true,
        reject_garbled_name: true,
        min_name_length: if mixed.limits.min_name_length > 0 {
            ToggleUsize::on(mixed.limits.min_name_length)
        } else {
            ToggleUsize::off(4)
        },
        max_name_length: if mixed.limits.max_name_length > 0 {
            ToggleUsize::on(mixed.limits.max_name_length)
        } else {
            ToggleUsize::off(128)
        },
        reject_empty_files: true,
        reject_all_empty_files: false,
        max_empty_files: ToggleUsize::off(0),
        max_file_count: if mixed.limits.max_file_count > 0 {
            ToggleUsize::on(mixed.limits.max_file_count)
        } else {
            ToggleUsize::off(2000)
        },
        max_file_name_length: if mixed.limits.max_file_name_length > 0 {
            ToggleUsize::on(mixed.limits.max_file_name_length)
        } else {
            ToggleUsize::off(255)
        },
        min_total_size: ToggleU64::off(1),
        max_total_size: if mixed.limits.max_total_size > 0 {
            ToggleU64::on(mixed.limits.max_total_size)
        } else {
            ToggleU64::off(2_199_023_255_552)
        },
        density_check: if mixed.limits.min_file_count_for_density_check > 0 {
            DensityGate::on(
                mixed.limits.min_file_count_for_density_check,
                mixed.limits.min_avg_file_size,
            )
        } else {
            DensityGate::off(100, 102_400)
        },
    };

    // 2. 关键词黑名单智能分组转换
    convert_keywords(&mixed.blocked_rules, &mut rules, &mut notes);

    // 3. custom_rules -> FileConstraint
    convert_custom_rules(&mixed.custom_rules, &mut rules, &mut notes);

    // 4. torrent_rules -> Gates + SubstantiveContent + SizeRatio
    convert_torrent_rules(&mixed.torrent_rules, &mut gates, &mut rules, &mut notes);

    // 5. blocked_name_patterns -> TitlePatternPreset + ExtensionRule(TorrentName) + CustomRegex
    convert_name_patterns(&mixed.blocked_name_patterns, &mut rules, &mut notes);

    // 6. blocked_file_patterns -> ExtensionRule(AnyFile) + CustomRegex
    convert_file_patterns(&mixed.blocked_file_patterns, &mut rules, &mut notes);

    let final_preset_id = if rules_match_online_baseline(&gates, &rules) {
        Some("online_standard".to_string())
    } else {
        preset_tag.map(String::from)
    };

    (
        FilterPolicy {
            schema_version: FILTER_POLICY_SCHEMA_VERSION,
            preset_id: final_preset_id,
            gates,
            cleaner: CleanerOptions::default(),
            rules,
        },
        notes,
    )
}

fn rules_match_online_baseline(gates: &FilterGates, rules: &[PolicyRule]) -> bool {
    let baseline = default_online_filter_policy();
    &baseline.gates == gates && baseline.rules.len() == rules.len() && baseline.rules == rules
}

fn convert_keywords(entries: &[KeywordEntry], out: &mut Vec<PolicyRule>, notes: &mut Vec<String>) {
    let mut enabled_words = Vec::new();
    let mut disabled_words = Vec::new();

    for entry in entries {
        match entry {
            KeywordEntry::Plain(s) => {
                let t = s.trim();
                if !t.is_empty() {
                    enabled_words.push(t.to_string());
                }
            }
            KeywordEntry::Struct(item) => {
                let t = item.value.trim();
                if t.is_empty() {
                    continue;
                }
                if item.enabled.unwrap_or(true) {
                    enabled_words.push(t.to_string());
                } else {
                    disabled_words.push(t.to_string());
                }
            }
        }
    }

    if enabled_words.is_empty() && disabled_words.is_empty() {
        return;
    }

    let malware_set: HashSet<&str> = MALWARE_KEYWORDS.iter().copied().collect();
    let crack_set: HashSet<&str> = CRACK_KEYWORDS.iter().copied().collect();
    let bait_set: HashSet<&str> = BAIT_KEYWORDS.iter().copied().collect();
    let placeholder_set: HashSet<&str> = PLACEHOLDER_KEYWORDS.iter().copied().collect();

    let mut malware_words = Vec::new();
    let mut crack_words = Vec::new();
    let mut bait_words = Vec::new();
    let mut placeholder_words = Vec::new();
    let mut custom_words = Vec::new();

    for w in enabled_words {
        let lower = w.to_lowercase();
        if malware_set.contains(lower.as_str()) {
            malware_words.push(w);
        } else if crack_set.contains(lower.as_str()) {
            crack_words.push(w);
        } else if bait_set.contains(lower.as_str()) {
            bait_words.push(w);
        } else if placeholder_set.contains(lower.as_str()) {
            placeholder_words.push(w);
        } else {
            custom_words.push(w);
        }
    }

    // 只有命中 >= 4 个已知语义词时才拆分成 4 组，小规模自定义导入保持 1 张简洁卡片
    let known_count =
        malware_words.len() + crack_words.len() + bait_words.len() + placeholder_words.len();
    if known_count >= 4 {
        let mut group_count = 0;
        if !malware_words.is_empty() {
            group_count += 1;
            out.push(PolicyRule {
                id: "kw-malware".to_string(),
                name: "恶意软件与木马关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全防护".to_string(), "关键词".to_string()],
                note: "从旧版 blocked_rules 自动归组".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: malware_words,
                },
            });
        }
        if !crack_words.is_empty() {
            group_count += 1;
            out.push(PolicyRule {
                id: "kw-crack".to_string(),
                name: "破解与激活外挂关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["软件破解".to_string(), "关键词".to_string()],
                note: "从旧版 blocked_rules 自动归组".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: crack_words,
                },
            });
        }
        if !bait_words.is_empty() {
            group_count += 1;
            out.push(PolicyRule {
                id: "kw-bait".to_string(),
                name: "密码与安装说明诱饵关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["诱饵推广".to_string(), "关键词".to_string()],
                note: "从旧版 blocked_rules 自动归组".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: bait_words,
                },
            });
        }
        if !placeholder_words.is_empty() {
            group_count += 1;
            out.push(PolicyRule {
                id: "kw-placeholder".to_string(),
                name: "无效占位标题关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["垃圾占位".to_string(), "关键词".to_string()],
                note: "从旧版 blocked_rules 自动归组".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: placeholder_words,
                },
            });
        }
        if !custom_words.is_empty() {
            group_count += 1;
            out.push(PolicyRule {
                id: "kw-custom".to_string(),
                name: "自定义关键词黑名单".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["关键词".to_string()],
                note: "旧版配置中额外的自定义黑名单关键词".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: custom_words,
                },
            });
        }
        notes.push(format!(
            "已将旧版关键词自动归纳为 {group_count} 组语义关键词卡片"
        ));
    } else {
        let mut combined = Vec::new();
        combined.extend(malware_words);
        combined.extend(crack_words);
        combined.extend(bait_words);
        combined.extend(placeholder_words);
        combined.extend(custom_words);
        if !combined.is_empty() {
            let count = combined.len();
            out.push(PolicyRule {
                id: "kw-imported".to_string(),
                name: "导入标题关键词黑名单".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["关键词".to_string()],
                note: "从旧版 blocked_rules 转换".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: combined,
                },
            });
            notes.push(format!("已转换 {count} 条标题关键词为 1 张关键词组规则"));
        }
    }

    if !disabled_words.is_empty() {
        out.push(PolicyRule {
            id: "kw-disabled".to_string(),
            name: "已停用关键词（兼容保留）".to_string(),
            enabled: false,
            action: PolicyAction::Reject,
            tags: vec!["关键词".to_string()],
            note: "保留原 v1 中已关闭的关键词".to_string(),
            matcher: RuleMatcher::KeywordGroup {
                target: TextTarget::RawName,
                match_mode: KeywordMatchMode::Contains,
                case_sensitive: false,
                words: disabled_words,
            },
        });
    }
}

/// 尝试将 `(?i)\.(ext1|ext2)$` 或 `(?i)\.ext$` 等扩展名正则无损提取为小写扩展名集合。
fn try_extract_extensions_regex(pattern: &str) -> Option<Vec<String>> {
    let trimmed = pattern.trim();
    let rest = trimmed.strip_prefix("(?i)")?.trim_start();
    let body = rest.strip_prefix(r"\.")?.strip_suffix('$')?;
    let raw_parts: Vec<&str> =
        if let Some(inner) = body.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            if let Some(stripped) = inner.strip_prefix("?:") {
                stripped.split('|').collect()
            } else {
                inner.split('|').collect()
            }
        } else {
            vec![body]
        };
    if raw_parts.is_empty() {
        return None;
    }
    let mut exts = Vec::with_capacity(raw_parts.len());
    for part in raw_parts {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        exts.push(normalize_extension(part));
    }
    Some(exts)
}

fn classify_extension_rule_name(exts: &[String], scope: ExtensionScope) -> (String, Vec<String>) {
    let set: HashSet<&str> = exts.iter().map(String::as_str).collect();
    if exts.len() == 1 && exts[0] == "torrent" {
        return (
            "种子内含套娃 .torrent 文件".to_string(),
            vec!["扩展名".to_string(), "套娃".to_string()],
        );
    }
    if exts.iter().all(|e| ARCHIVE_EXTS.contains(&e.as_str())) && set.len() >= 5 {
        return (
            "标题结尾为压缩包/镜像后缀".to_string(),
            vec!["扩展名".to_string(), "标题".to_string()],
        );
    }
    if exts
        .iter()
        .all(|e| EXECUTABLE_TITLE_EXTS.contains(&e.as_str()))
        && set.len() >= 10
    {
        let name = if scope == ExtensionScope::AnyFile {
            "种子内含高危可执行/移动端安装包".to_string()
        } else {
            "标题结尾为可执行/脚本/安装包后缀".to_string()
        };
        return (name, vec!["安全防护".to_string(), "扩展名".to_string()]);
    }
    if exts
        .iter()
        .all(|e| DANGEROUS_EXEC_FILE_EXTS.contains(&e.as_str()))
        && set.len() >= 5
    {
        return (
            "种子内含高危可执行/移动端安装包".to_string(),
            vec!["安全防护".to_string(), "扩展名".to_string()],
        );
    }
    if exts.iter().all(|e| DOC_EXTS.contains(&e.as_str())) && set.len() >= 5 {
        return (
            "标题结尾为办公/宏文档/电子书诱饵后缀".to_string(),
            vec!["扩展名".to_string(), "标题".to_string()],
        );
    }
    if exts
        .iter()
        .all(|e| SHORTCUT_JUNK_EXTS.contains(&e.as_str()))
    {
        return (
            "标题结尾为快捷方式/网页/文本后缀".to_string(),
            vec!["扩展名".to_string(), "标题".to_string()],
        );
    }
    let name = match scope {
        ExtensionScope::TorrentName => "标题拦截后缀列表",
        ExtensionScope::AnyFile => "文件拦截后缀列表",
        ExtensionScope::AllFiles => "全文件匹配后缀列表",
    };
    (name.to_string(), vec!["扩展名".to_string()])
}

fn recognize_title_preset(pattern: &str) -> Option<(TitlePatternKind, Option<usize>)> {
    let p = pattern.trim();
    match p {
        r"\p{Cyrillic}{2,}" => return Some((TitlePatternKind::Cyrillic, Some(2))),
        r"^\d+$" => return Some((TitlePatternKind::DigitsOnly, None)),
        r"^[0-9a-fA-F]{40}$" | r"(?i)^[0-9a-f]{40}$" => {
            return Some((TitlePatternKind::Sha1Hex, None));
        }
        r"^[0-9a-fA-F]{32}$" | r"(?i)^[0-9a-f]{32}$" => {
            return Some((TitlePatternKind::Md5Hex, None));
        }
        r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$" => {
            return Some((TitlePatternKind::Uuid, None));
        }
        r"^\d+\.\d{3}$" => return Some((TitlePatternKind::DecimalCode, None)),
        r"(?i)^magnet:\?" | r"^magnet:\?" => return Some((TitlePatternKind::MagnetPrefix, None)),
        r"(?i)^https?://" | r"^https?://" => return Some((TitlePatternKind::HttpUrlPrefix, None)),
        _ => {}
    }

    if let Some(inner) = p
        .strip_prefix(r"\p{Cyrillic}{")
        .and_then(|s| s.strip_suffix(",}"))
        && let Ok(n) = inner.parse::<usize>()
        && n >= 1
    {
        return Some((TitlePatternKind::Cyrillic, Some(n)));
    }

    None
}

fn is_split_archive_pattern_idx(pattern: &str) -> Option<usize> {
    match pattern.trim() {
        r"(?i)\.(zip|rar|7z|iso|img|tar|gz|tgz|bz2|tbz2|xz|txz|cab|ace)\.\d+$" => Some(0),
        r"(?i)\.part\d+\.rar$" => Some(1),
        r"(?i)\.[rz]\d{2}$" => Some(2),
        _ => None,
    }
}

fn convert_name_patterns(
    entries: &[PatternEntry],
    out: &mut Vec<PolicyRule>,
    notes: &mut Vec<String>,
) {
    let mut presets: Vec<TitlePatternKind> = Vec::new();
    let mut min_cyrillic_chars = 2_usize;
    let mut split_flags = [false; 3];
    let mut split_raw: Vec<(usize, String)> = Vec::new();
    let mut ext_groups: Vec<Vec<String>> = Vec::new();
    let mut custom_patterns = Vec::new();
    let mut disabled_patterns = Vec::new();

    for entry in entries {
        let (pat, enabled) = match entry {
            PatternEntry::Plain(s) => (s.trim(), true),
            PatternEntry::Struct(item) => (item.pattern.trim(), item.enabled.unwrap_or(true)),
        };
        if pat.is_empty() {
            continue;
        }
        if !enabled {
            disabled_patterns.push(pat.to_string());
            continue;
        }
        if let Some((kind, cyr_min)) = recognize_title_preset(pat) {
            if !presets.contains(&kind) {
                presets.push(kind);
            }
            if let Some(n) = cyr_min {
                min_cyrillic_chars = n;
            }
            continue;
        }
        if let Some(idx) = is_split_archive_pattern_idx(pat) {
            split_flags[idx] = true;
            split_raw.push((idx, pat.to_string()));
            continue;
        }
        if let Some(exts) = try_extract_extensions_regex(pat) {
            ext_groups.push(exts);
            continue;
        }
        custom_patterns.push(pat.to_string());
    }

    if split_flags.iter().all(|&f| f) {
        if !presets.contains(&TitlePatternKind::SplitArchiveSuffix) {
            presets.push(TitlePatternKind::SplitArchiveSuffix);
        }
    } else {
        for (_idx, pat) in split_raw {
            custom_patterns.push(pat);
        }
    }

    let mut converted_preset = false;
    if !presets.is_empty() {
        converted_preset = true;
        out.push(PolicyRule {
            id: "title-spam-features".to_string(),
            name: "常见垃圾标题结构特征".to_string(),
            enabled: true,
            action: PolicyAction::Reject,
            tags: vec!["标题规范".to_string()],
            note: "由旧版 blocked_name_patterns 自动识别并转为可视化开关".to_string(),
            matcher: RuleMatcher::TitlePatternPreset {
                patterns: presets,
                min_cyrillic_chars,
            },
        });
    }

    let mut ext_rule_count = 0;
    for (idx, exts) in ext_groups.into_iter().enumerate() {
        ext_rule_count += 1;
        let (name, tags) = classify_extension_rule_name(&exts, ExtensionScope::TorrentName);
        out.push(PolicyRule {
            id: format!("ext-title-{}", idx + 1),
            name,
            enabled: true,
            action: PolicyAction::Reject,
            tags,
            note: "由旧版标题正则无损转换为扩展名集合".to_string(),
            matcher: RuleMatcher::ExtensionRule {
                scope: ExtensionScope::TorrentName,
                extensions: exts,
            },
        });
    }

    if converted_preset || ext_rule_count > 0 {
        notes.push(format!(
            "已将旧版标题正则转为{}扩展名规则卡 ({ext_rule_count} 张)",
            if converted_preset {
                "标题结构特征开关 + "
            } else {
                ""
            }
        ));
    }

    if !custom_patterns.is_empty() {
        notes.push(format!(
            "无损保留 {} 条自定义标题正则表达式",
            custom_patterns.len()
        ));
        out.push(PolicyRule {
            id: "re-title-custom".to_string(),
            name: "导入标题自定义正则".to_string(),
            enabled: true,
            action: PolicyAction::Reject,
            tags: vec!["自定义正则".to_string()],
            note: "从旧版 blocked_name_patterns 保留的自定义正则".to_string(),
            matcher: RuleMatcher::CustomRegex {
                target: TextTarget::RawName,
                match_all: false,
                patterns: custom_patterns,
            },
        });
    }

    if !disabled_patterns.is_empty() {
        out.push(PolicyRule {
            id: "re-title-disabled".to_string(),
            name: "已停用标题正则（兼容保留）".to_string(),
            enabled: false,
            action: PolicyAction::Reject,
            tags: vec!["自定义正则".to_string()],
            note: String::new(),
            matcher: RuleMatcher::CustomRegex {
                target: TextTarget::RawName,
                match_all: false,
                patterns: disabled_patterns,
            },
        });
    }
}

fn convert_file_patterns(
    entries: &[PatternEntry],
    out: &mut Vec<PolicyRule>,
    notes: &mut Vec<String>,
) {
    let mut ext_groups = Vec::new();
    let mut custom_patterns = Vec::new();
    let mut disabled_patterns = Vec::new();

    for entry in entries {
        let (pat, enabled) = match entry {
            PatternEntry::Plain(s) => (s.trim(), true),
            PatternEntry::Struct(item) => (item.pattern.trim(), item.enabled.unwrap_or(true)),
        };
        if pat.is_empty() {
            continue;
        }
        if !enabled {
            disabled_patterns.push(pat.to_string());
            continue;
        }
        if let Some(exts) = try_extract_extensions_regex(pat) {
            ext_groups.push(exts);
        } else {
            custom_patterns.push(pat.to_string());
        }
    }

    let ext_count = ext_groups.len();
    for (idx, exts) in ext_groups.into_iter().enumerate() {
        let (name, tags) = classify_extension_rule_name(&exts, ExtensionScope::AnyFile);
        out.push(PolicyRule {
            id: format!("ext-file-{}", idx + 1),
            name,
            enabled: true,
            action: PolicyAction::Reject,
            tags,
            note: "由旧版文件正则无损转换为扩展名集合".to_string(),
            matcher: RuleMatcher::ExtensionRule {
                scope: ExtensionScope::AnyFile,
                extensions: exts,
            },
        });
    }
    if ext_count > 0 {
        notes.push(format!(
            "已将 {ext_count} 条旧版文件正则转为可扩展后缀芯片规则"
        ));
    }

    if !custom_patterns.is_empty() {
        out.push(PolicyRule {
            id: "re-file-custom".to_string(),
            name: "导入文件路径自定义正则".to_string(),
            enabled: true,
            action: PolicyAction::Reject,
            tags: vec!["自定义正则".to_string()],
            note: String::new(),
            matcher: RuleMatcher::CustomRegex {
                target: TextTarget::AnyFilePath,
                match_all: false,
                patterns: custom_patterns,
            },
        });
    }

    if !disabled_patterns.is_empty() {
        out.push(PolicyRule {
            id: "re-file-disabled".to_string(),
            name: "已停用文件正则（兼容保留）".to_string(),
            enabled: false,
            action: PolicyAction::Reject,
            tags: vec!["自定义正则".to_string()],
            note: String::new(),
            matcher: RuleMatcher::CustomRegex {
                target: TextTarget::AnyFilePath,
                match_all: false,
                patterns: disabled_patterns,
            },
        });
    }
}

fn convert_custom_rules(
    items: &[LegacyCustomRuleConfig],
    out: &mut Vec<PolicyRule>,
    notes: &mut Vec<String>,
) {
    for (idx, item) in items.iter().enumerate() {
        let mut extensions = Vec::new();
        let mut extra_regexes = Vec::new();
        for pat in &item.patterns {
            let trimmed = pat.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(exts) = try_extract_extensions_regex(trimmed) {
                extensions.extend(exts);
            } else {
                extra_regexes.push(trimmed.to_string());
            }
        }
        let name = if !item.name.trim().is_empty() {
            item.name.trim().to_string()
        } else if !extensions.is_empty()
            && extensions
                .iter()
                .any(|e| e == "mp4" || e == "mkv" || e == "avi")
        {
            "视频文件最小体积与文件名长度约束".to_string()
        } else {
            format!("文件约束规则 #{}", idx + 1)
        };
        let id = if item.id.trim().is_empty() {
            format!("fc-{}", idx + 1)
        } else {
            item.id.clone()
        };
        out.push(PolicyRule {
            id,
            name,
            enabled: item.enabled.unwrap_or(true),
            action: PolicyAction::Reject,
            tags: vec!["文件约束".to_string()],
            note: item.note.clone(),
            matcher: RuleMatcher::FileConstraint {
                extensions,
                extra_regexes,
                mode: FileConstraintMode::AtLeastOneMatched,
                min_file_size: (item.min_file_size > 0).then_some(item.min_file_size),
                max_file_size: (item.max_file_size > 0).then_some(item.max_file_size),
                min_file_name_length: (item.min_name_length > 0).then_some(item.min_name_length),
                max_file_name_length: (item.max_name_length > 0).then_some(item.max_name_length),
            },
        });
    }
    if !items.is_empty() {
        notes.push(format!(
            "已将 {} 条旧版 custom_rules 升级为文件体积/文件名约束规则",
            items.len()
        ));
    }
}

fn is_system_junk_regex(pattern: &str) -> bool {
    matches!(
        pattern.trim(),
        r"(?i)\.ds_store$"
            | r"(?i)thumbs\.db$"
            | r"(?i)desktop\.ini$"
            | r"(?i)(^|/)__macosx/"
            | r"(?i)(^|/)\._"
    )
}

fn is_split_archive_ratio_regex(pattern: &str) -> bool {
    matches!(
        pattern.trim(),
        r"(?i)\.(zip|rar|7z|iso|img|tar|gz|tgz|bz2|tbz2|xz|txz|cab|ace)\.\d+$"
            | r"(?i)\.part\d+\.rar$"
            | r"(?i)\.[rz]\d{2}$"
    )
}

fn convert_torrent_rules(
    items: &[LegacyTorrentRuleConfig],
    gates: &mut FilterGates,
    out: &mut Vec<PolicyRule>,
    notes: &mut Vec<String>,
) {
    let mut converted_whole = 0_usize;
    for (idx, item) in items.iter().enumerate() {
        let enabled = item.enabled.unwrap_or(true);

        // 全局空文件/最小体积直接合并到基础门禁
        if enabled {
            if item.reject_if_all_empty {
                gates.reject_all_empty_files = true;
            }
            if item.min_total_size > 0
                && (!gates.min_total_size.enabled
                    || item.min_total_size > gates.min_total_size.value)
            {
                gates.min_total_size = ToggleU64::on(item.min_total_size);
            }
            if item.max_empty_files > 0
                && (!gates.max_empty_files.enabled
                    || item.max_empty_files < gates.max_empty_files.value)
            {
                gates.max_empty_files = ToggleUsize::on(item.max_empty_files);
            }
        }

        if !item.junk_patterns.is_empty() {
            let mut junk_extensions = Vec::new();
            let mut extra_junk_regexes = Vec::new();
            let mut include_system_junk = false;
            for pat in &item.junk_patterns {
                let trimmed = pat.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if is_system_junk_regex(trimmed) {
                    include_system_junk = true;
                } else if let Some(exts) = try_extract_extensions_regex(trimmed) {
                    junk_extensions.extend(exts);
                } else {
                    extra_junk_regexes.push(trimmed.to_string());
                }
            }
            converted_whole += 1;
            out.push(PolicyRule {
                id: format!("sub-{}", idx + 1),
                name: if item.name.trim().is_empty() || item.name.starts_with("整种规则") {
                    "排除垃圾与快捷方式后的实质文件底线".to_string()
                } else {
                    item.name.trim().to_string()
                },
                enabled,
                action: PolicyAction::Reject,
                tags: vec!["整种质检".to_string()],
                note: item.note.clone(),
                matcher: RuleMatcher::SubstantiveContent {
                    junk_extensions,
                    include_system_junk,
                    extra_junk_regexes,
                    min_files: item.min_substantive_files.max(1),
                    min_total_size: item.min_substantive_total_size,
                },
            });
        }

        if !item.archive_patterns.is_empty() && item.max_archive_size_ratio > 0.0 {
            let mut extensions = Vec::new();
            let mut extra_regexes = Vec::new();
            let mut include_split_archives = false;
            for pat in &item.archive_patterns {
                let trimmed = pat.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if is_split_archive_ratio_regex(trimmed) {
                    include_split_archives = true;
                } else if let Some(exts) = try_extract_extensions_regex(trimmed) {
                    extensions.extend(exts);
                } else {
                    extra_regexes.push(trimmed.to_string());
                }
            }
            converted_whole += 1;
            let pct = (item.max_archive_size_ratio * 100.0).round();
            out.push(PolicyRule {
                id: format!("ratio-{}", idx + 1),
                name: if item.name.trim().is_empty() || item.name.starts_with("整种规则") {
                    format!("压缩包体积占比上限（{pct:.0}%）")
                } else {
                    item.name.trim().to_string()
                },
                enabled,
                action: PolicyAction::Reject,
                tags: vec!["压缩包治理".to_string()],
                note: item.note.clone(),
                matcher: RuleMatcher::SizeRatio {
                    extensions,
                    include_split_archives,
                    extra_regexes,
                    max_ratio: item.max_archive_size_ratio,
                },
            });
        }
    }

    if converted_whole > 0 || gates.reject_all_empty_files || gates.min_total_size.enabled {
        notes.push("已将旧版 torrent_rules 拆分为门禁项、实质内容检查与占比上限规则".to_string());
    }
}
