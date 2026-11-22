use crate::cleaner::TitleCleaner;
use crate::error::FilterError;
use crate::schema::{
    CleanerOptions, CompositeLogic, ExtensionScope, FileConstraintMode, FilterBundle, FilterGates,
    KeywordMatchMode, PolicyAction, RuleMatcher, TextTarget, TitlePatternKind, normalize_extension,
};
use hameta_common::TorrentMessage;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

const MAX_COMPOSITE_DEPTH: usize = 3;

fn file_name_char_count(path: &str) -> usize {
    path.rsplit('/').next().unwrap_or(path).chars().count()
}

fn file_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterCategory {
    EmptyName,
    TitleLength,
    EmptyFiles,
    AllEmptyFiles,
    MaxEmptyFiles,
    MaxFileCount,
    MaxFileNameLength,
    MinTotalSize,
    MaxTotalSize,
    FileDensity,
    KeywordGroup,
    ExtensionRule,
    TitlePatternPreset,
    FileConstraint,
    SubstantiveContent,
    SizeRatio,
    CustomRegex,
    Composite,
    EmptyRefinedName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleHit {
    pub category: FilterCategory,
    #[serde(default)]
    pub action: PolicyAction,
    pub rule_id: String,
    pub rule_label: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FilterOutcome {
    Accepted {
        refined_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allowed_by: Option<RuleHit>,
    },
    Rejected {
        refined_name: Option<String>,
        hit: RuleHit,
    },
}

impl FilterOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted { .. })
    }

    pub fn refined_name(&self) -> Option<&str> {
        match self {
            Self::Accepted { refined_name, .. } => Some(refined_name.as_str()),
            Self::Rejected { refined_name, .. } => refined_name.as_deref(),
        }
    }

    pub fn hit(&self) -> Option<&RuleHit> {
        match self {
            Self::Accepted { .. } => None,
            Self::Rejected { hit, .. } => Some(hit),
        }
    }

    pub fn allowed_by(&self) -> Option<&RuleHit> {
        match self {
            Self::Accepted { allowed_by, .. } => allowed_by.as_ref(),
            Self::Rejected { .. } => None,
        }
    }
}

struct CompiledExtSet {
    single: HashSet<String>,
    dotted: Vec<String>,
}

impl CompiledExtSet {
    fn from_list(exts: &[String]) -> Self {
        let mut single = HashSet::new();
        let mut dotted = Vec::new();
        for raw in exts {
            let norm = normalize_extension(raw);
            if norm.is_empty() {
                continue;
            }
            if norm.contains('.') {
                let suffix = format!(".{norm}");
                if !dotted.contains(&suffix) {
                    dotted.push(suffix);
                }
            } else {
                single.insert(norm);
            }
        }
        Self { single, dotted }
    }

    fn is_empty(&self) -> bool {
        self.single.is_empty() && self.dotted.is_empty()
    }

    fn matches_ext(&self, ext_lower: Option<&str>, full_name: &str) -> Option<String> {
        if let Some(ext) = ext_lower
            && self.single.contains(ext)
        {
            return Some(ext.to_string());
        }
        if !self.dotted.is_empty() {
            let lower = full_name.to_ascii_lowercase();
            for suffix in &self.dotted {
                if lower.ends_with(suffix) {
                    return Some(suffix.trim_start_matches('.').to_string());
                }
            }
        }
        None
    }
}

enum CompiledMatcher {
    KeywordGroup {
        target: TextTarget,
        match_mode: KeywordMatchMode,
        case_sensitive: bool,
        words_raw: Vec<String>,
        words_lower: Vec<String>,
    },
    ExtensionRule {
        scope: ExtensionScope,
        exts: CompiledExtSet,
    },
    TitlePatternPreset {
        patterns: Vec<TitlePatternKind>,
        min_cyrillic_chars: usize,
    },
    FileConstraint {
        exts: CompiledExtSet,
        extra_regexes: Vec<Regex>,
        mode: FileConstraintMode,
        min_file_size: Option<u64>,
        max_file_size: Option<u64>,
        min_file_name_length: Option<usize>,
        max_file_name_length: Option<usize>,
    },
    SubstantiveContent {
        junk_exts: CompiledExtSet,
        include_system_junk: bool,
        extra_junk_regexes: Vec<Regex>,
        min_files: usize,
        min_total_size: u64,
    },
    SizeRatio {
        exts: CompiledExtSet,
        include_split_archives: bool,
        extra_regexes: Vec<Regex>,
        max_ratio: f64,
    },
    CustomRegex {
        target: TextTarget,
        match_all: bool,
        patterns: Vec<(String, Regex)>,
    },
    Composite {
        logic: CompositeLogic,
        conditions: Vec<CompiledMatcher>,
    },
}

impl CompiledMatcher {
    fn category(&self) -> FilterCategory {
        match self {
            Self::KeywordGroup { .. } => FilterCategory::KeywordGroup,
            Self::ExtensionRule { .. } => FilterCategory::ExtensionRule,
            Self::TitlePatternPreset { .. } => FilterCategory::TitlePatternPreset,
            Self::FileConstraint { .. } => FilterCategory::FileConstraint,
            Self::SubstantiveContent { .. } => FilterCategory::SubstantiveContent,
            Self::SizeRatio { .. } => FilterCategory::SizeRatio,
            Self::CustomRegex { .. } => FilterCategory::CustomRegex,
            Self::Composite { .. } => FilterCategory::Composite,
        }
    }
}

struct CompiledPolicyRule {
    rule_id: String,
    rule_label: String,
    action: PolicyAction,
    matcher: CompiledMatcher,
}

struct FileEntry<'a> {
    path: &'a str,
    basename: &'a str,
    name_char_len: usize,
    ext_lower: Option<String>,
    is_system_junk: bool,
    is_split_archive: bool,
    size: u64,
}

struct TorrentEvalContext<'a> {
    msg: &'a TorrentMessage,
    raw_trimmed: &'a str,
    raw_lower: String,
    refined_name: &'a str,
    refined_lower: String,
    title_ext_lower: Option<String>,
    files: Vec<FileEntry<'a>>,
}

impl<'a> TorrentEvalContext<'a> {
    fn build(msg: &'a TorrentMessage, raw_trimmed: &'a str, refined_name: &'a str) -> Self {
        let title_ext_lower = extract_extension_lower(raw_trimmed);
        let mut files = Vec::with_capacity(msg.files.len());
        for f in &msg.files {
            let basename = file_basename(&f.path);
            let name_char_len = basename.chars().count();
            let ext_lower = extract_extension_lower(basename);
            let is_system_junk = check_system_junk(&f.path, basename);
            let is_split_archive = check_split_archive(&f.path);
            files.push(FileEntry {
                path: &f.path,
                basename,
                name_char_len,
                ext_lower,
                is_system_junk,
                is_split_archive,
                size: f.size,
            });
        }
        Self {
            msg,
            raw_trimmed,
            raw_lower: msg.name.to_lowercase(),
            refined_name,
            refined_lower: refined_name.to_lowercase(),
            title_ext_lower,
            files,
        }
    }
}

/// 不可变已编译 Filter Policy v2 引擎（可安全通过 `ArcSwap` 原子切换）。
pub struct CompiledFilterEngine {
    version: i64,
    gates: FilterGates,
    cleaner_options: CleanerOptions,
    cleaner: TitleCleaner,
    rules: Vec<CompiledPolicyRule>,
}

impl CompiledFilterEngine {
    /// 编译并校验 Filter Policy v2 策略及清洗词库。
    pub fn compile(bundle: &FilterBundle) -> Result<Self, FilterError> {
        let policy = &bundle.policy;
        let cleaner = TitleCleaner::compile_with_options(
            &bundle.ad_phrases,
            &bundle.ad_regexes,
            &policy.cleaner,
        )?;

        let mut rules = Vec::with_capacity(policy.rules.len());
        for (idx, rule) in policy.rules.iter().enumerate() {
            let rule_id = if rule.id.trim().is_empty() {
                format!("rule-{}", idx + 1)
            } else {
                rule.id.clone()
            };
            check_composite_depth(&rule.matcher, 0, &rule_id)?;
            if !rule.enabled {
                continue;
            }
            let matcher = compile_matcher(&rule.matcher, &rule_id)?;
            let label = if rule.name.trim().is_empty() {
                rule_id.clone()
            } else {
                rule.name.clone()
            };
            rules.push(CompiledPolicyRule {
                rule_id,
                rule_label: label,
                action: rule.action,
                matcher,
            });
        }

        Ok(Self {
            version: bundle.version,
            gates: policy.gates.clone(),
            cleaner_options: policy.cleaner.clone(),
            cleaner,
            rules,
        })
    }

    pub fn version(&self) -> i64 {
        self.version
    }

    pub fn cleaner(&self) -> &TitleCleaner {
        &self.cleaner
    }

    pub fn is_valid(&self, msg: &TorrentMessage) -> bool {
        self.evaluate(msg).is_accepted()
    }

    /// 按 3 段流水线执行评估：基础门禁 -> 标题清洗 -> 有序 Allow/Reject 策略链。
    pub fn evaluate(&self, msg: &TorrentMessage) -> FilterOutcome {
        let trimmed_name = msg.name.trim();

        // 阶段 1：基础门禁（无需编译正则，快路径剪枝）
        if self.gates.reject_empty_name && trimmed_name.is_empty() {
            return rejected(
                None,
                FilterCategory::EmptyName,
                "gate:empty_name",
                "空标题检查",
                "种子标题为空",
            );
        }

        let title_len = trimmed_name.chars().count();
        if self.gates.min_name_length.enabled
            && self.gates.min_name_length.value > 0
            && title_len < self.gates.min_name_length.value
        {
            return rejected(
                None,
                FilterCategory::TitleLength,
                "gate:min_name_length",
                "标题最小长度限制",
                format!(
                    "标题字符数 {title_len} 小于最小限制 {}",
                    self.gates.min_name_length.value
                ),
            );
        }

        if self.gates.max_name_length.enabled
            && self.gates.max_name_length.value > 0
            && title_len > self.gates.max_name_length.value
        {
            return rejected(
                None,
                FilterCategory::TitleLength,
                "gate:max_name_length",
                "标题最大长度限制",
                format!(
                    "标题字符数 {title_len} 超过最大限制 {}",
                    self.gates.max_name_length.value
                ),
            );
        }

        if self.gates.reject_empty_files && (msg.file_count == 0 || msg.files.is_empty()) {
            return rejected(
                None,
                FilterCategory::EmptyFiles,
                "gate:empty_files",
                "文件列表非空检查",
                "种子文件列表为空",
            );
        }

        if self.gates.max_file_count.enabled
            && self.gates.max_file_count.value > 0
            && msg.file_count > self.gates.max_file_count.value
        {
            return rejected(
                None,
                FilterCategory::MaxFileCount,
                "gate:max_file_count",
                "最大文件数量限制",
                format!(
                    "文件数 {} 超过上限 {}",
                    msg.file_count, self.gates.max_file_count.value
                ),
            );
        }

        if self.gates.max_file_name_length.enabled
            && self.gates.max_file_name_length.value > 0
            && let Some(bad_file) = msg
                .files
                .iter()
                .find(|f| file_name_char_count(&f.path) > self.gates.max_file_name_length.value)
        {
            return rejected(
                None,
                FilterCategory::MaxFileNameLength,
                "gate:max_file_name_length",
                "单文件名长度限制",
                format!(
                    "文件名超长 ({} > {}): {}",
                    file_name_char_count(&bad_file.path),
                    self.gates.max_file_name_length.value,
                    bad_file.path
                ),
            );
        }

        if self.gates.min_total_size.enabled
            && self.gates.min_total_size.value > 0
            && msg.total_size < self.gates.min_total_size.value
        {
            return rejected(
                None,
                FilterCategory::MinTotalSize,
                "gate:min_total_size",
                "整种最小体积限制",
                format!(
                    "总大小 {} 小于下限 {}",
                    msg.total_size, self.gates.min_total_size.value
                ),
            );
        }

        if self.gates.max_total_size.enabled
            && self.gates.max_total_size.value > 0
            && msg.total_size > self.gates.max_total_size.value
        {
            return rejected(
                None,
                FilterCategory::MaxTotalSize,
                "gate:max_total_size",
                "整种最大体积限制",
                format!(
                    "总大小 {} 超过上限 {}",
                    msg.total_size, self.gates.max_total_size.value
                ),
            );
        }

        if self.gates.density_check.enabled
            && self.gates.density_check.min_file_count > 0
            && msg.file_count > self.gates.density_check.min_file_count
        {
            let avg_file_size = msg.total_size / msg.file_count as u64;
            if avg_file_size < self.gates.density_check.min_avg_file_size {
                return rejected(
                    None,
                    FilterCategory::FileDensity,
                    "gate:file_density",
                    "多文件平均体积密度检查",
                    format!(
                        "文件数 {} > {} 且平均单文件体积 {} < {}",
                        msg.file_count,
                        self.gates.density_check.min_file_count,
                        avg_file_size,
                        self.gates.density_check.min_avg_file_size
                    ),
                );
            }
        }

        if (self.gates.reject_all_empty_files || self.gates.max_empty_files.enabled)
            && !msg.files.is_empty()
        {
            let empty_count = msg.files.iter().filter(|f| f.size == 0).count();
            if self.gates.reject_all_empty_files && empty_count == msg.files.len() {
                return rejected(
                    None,
                    FilterCategory::AllEmptyFiles,
                    "gate:all_empty_files",
                    "全空种子检查",
                    "种子内全部文件大小为 0",
                );
            }
            if self.gates.max_empty_files.enabled && empty_count > self.gates.max_empty_files.value
            {
                return rejected(
                    None,
                    FilterCategory::MaxEmptyFiles,
                    "gate:max_empty_files",
                    "零字节文件数上限",
                    format!(
                        "空文件数 {empty_count} 超过上限 {}",
                        self.gates.max_empty_files.value
                    ),
                );
            }
        }

        // 阶段 2：标题清洗
        let refined = self.cleaner.refine(&msg.name);

        // 阶段 3：单遍预索引文件列表 + 按优先级执行 Allow / Reject 规则链
        let mut allowed_by: Option<RuleHit> = None;
        if !self.rules.is_empty() {
            let ctx = TorrentEvalContext::build(msg, trimmed_name, &refined);
            for rule in &self.rules {
                if let Some(detail) = eval_matcher(&rule.matcher, rule.action, &ctx) {
                    let hit = RuleHit {
                        category: rule.matcher.category(),
                        action: rule.action,
                        rule_id: rule.rule_id.clone(),
                        rule_label: rule.rule_label.clone(),
                        detail,
                    };
                    match rule.action {
                        PolicyAction::Reject => {
                            return FilterOutcome::Rejected {
                                refined_name: Some(refined),
                                hit,
                            };
                        }
                        PolicyAction::Allow => {
                            allowed_by = Some(hit);
                            break;
                        }
                    }
                }
            }
        }

        if self.cleaner_options.reject_if_empty_after_clean && refined.is_empty() {
            return rejected(
                Some(String::new()),
                FilterCategory::EmptyRefinedName,
                "cleaner:empty_refined_name",
                "清洗后有效标题为空",
                "标题经清洗剥离站点广告与域名后无剩余有效内容",
            );
        }

        FilterOutcome::Accepted {
            refined_name: refined,
            allowed_by,
        }
    }
}

fn rejected(
    refined_name: Option<String>,
    category: FilterCategory,
    rule_id: impl Into<String>,
    rule_label: impl Into<String>,
    detail: impl Into<String>,
) -> FilterOutcome {
    FilterOutcome::Rejected {
        refined_name,
        hit: RuleHit {
            category,
            action: PolicyAction::Reject,
            rule_id: rule_id.into(),
            rule_label: rule_label.into(),
            detail: detail.into(),
        },
    }
}

fn check_composite_depth(
    matcher: &RuleMatcher,
    depth: usize,
    rule_id: &str,
) -> Result<(), FilterError> {
    if depth > MAX_COMPOSITE_DEPTH {
        return Err(FilterError::InvalidRule(format!(
            "规则 [{rule_id}] 组合条件嵌套深度超过上限 ({MAX_COMPOSITE_DEPTH})"
        )));
    }
    if let RuleMatcher::Composite { conditions, .. } = matcher {
        for sub in conditions {
            check_composite_depth(sub, depth + 1, rule_id)?;
        }
    }
    Ok(())
}

fn compile_regex_list(
    category: &str,
    rule_id: &str,
    patterns: &[String],
) -> Result<Vec<(String, Regex)>, FilterError> {
    let mut out = Vec::new();
    for pat in patterns {
        let trimmed = pat.trim();
        if trimmed.is_empty() {
            continue;
        }
        let regex = Regex::new(trimmed).map_err(|err| FilterError::InvalidRegex {
            category: category.to_string(),
            rule_id: rule_id.to_string(),
            pattern: pat.clone(),
            message: err.to_string(),
        })?;
        out.push((trimmed.to_string(), regex));
    }
    Ok(out)
}

fn compile_matcher(matcher: &RuleMatcher, rule_id: &str) -> Result<CompiledMatcher, FilterError> {
    match matcher {
        RuleMatcher::KeywordGroup {
            target,
            match_mode,
            case_sensitive,
            words,
        } => {
            let mut words_raw = Vec::new();
            let mut words_lower = Vec::new();
            for w in words {
                let t = w.trim();
                if t.is_empty() {
                    continue;
                }
                words_raw.push(t.to_string());
                words_lower.push(t.to_lowercase());
            }
            Ok(CompiledMatcher::KeywordGroup {
                target: *target,
                match_mode: *match_mode,
                case_sensitive: *case_sensitive,
                words_raw,
                words_lower,
            })
        }
        RuleMatcher::ExtensionRule { scope, extensions } => Ok(CompiledMatcher::ExtensionRule {
            scope: *scope,
            exts: CompiledExtSet::from_list(extensions),
        }),
        RuleMatcher::TitlePatternPreset {
            patterns,
            min_cyrillic_chars,
        } => Ok(CompiledMatcher::TitlePatternPreset {
            patterns: patterns.clone(),
            min_cyrillic_chars: (*min_cyrillic_chars).max(1),
        }),
        RuleMatcher::FileConstraint {
            extensions,
            extra_regexes,
            mode,
            min_file_size,
            max_file_size,
            min_file_name_length,
            max_file_name_length,
        } => {
            let compiled_re = compile_regex_list("file_constraint", rule_id, extra_regexes)?
                .into_iter()
                .map(|(_, re)| re)
                .collect();
            Ok(CompiledMatcher::FileConstraint {
                exts: CompiledExtSet::from_list(extensions),
                extra_regexes: compiled_re,
                mode: *mode,
                min_file_size: min_file_size.filter(|&v| v > 0),
                max_file_size: max_file_size.filter(|&v| v > 0),
                min_file_name_length: min_file_name_length.filter(|&v| v > 0),
                max_file_name_length: max_file_name_length.filter(|&v| v > 0),
            })
        }
        RuleMatcher::SubstantiveContent {
            junk_extensions,
            include_system_junk,
            extra_junk_regexes,
            min_files,
            min_total_size,
        } => {
            let compiled_re =
                compile_regex_list("substantive_content", rule_id, extra_junk_regexes)?
                    .into_iter()
                    .map(|(_, re)| re)
                    .collect();
            Ok(CompiledMatcher::SubstantiveContent {
                junk_exts: CompiledExtSet::from_list(junk_extensions),
                include_system_junk: *include_system_junk,
                extra_junk_regexes: compiled_re,
                min_files: *min_files,
                min_total_size: *min_total_size,
            })
        }
        RuleMatcher::SizeRatio {
            extensions,
            include_split_archives,
            extra_regexes,
            max_ratio,
        } => {
            if !(*max_ratio > 0.0 && *max_ratio <= 1.0) {
                return Err(FilterError::InvalidRule(format!(
                    "规则 [{rule_id}] SizeRatio 的 max_ratio ({max_ratio}) 必须在 (0.0, 1.0] 范围内"
                )));
            }
            let compiled_re = compile_regex_list("size_ratio", rule_id, extra_regexes)?
                .into_iter()
                .map(|(_, re)| re)
                .collect();
            Ok(CompiledMatcher::SizeRatio {
                exts: CompiledExtSet::from_list(extensions),
                include_split_archives: *include_split_archives,
                extra_regexes: compiled_re,
                max_ratio: *max_ratio,
            })
        }
        RuleMatcher::CustomRegex {
            target,
            match_all,
            patterns,
        } => Ok(CompiledMatcher::CustomRegex {
            target: *target,
            match_all: *match_all,
            patterns: compile_regex_list("custom_regex", rule_id, patterns)?,
        }),
        RuleMatcher::Composite { logic, conditions } => {
            let mut compiled = Vec::with_capacity(conditions.len());
            for cond in conditions {
                compiled.push(compile_matcher(cond, rule_id)?);
            }
            Ok(CompiledMatcher::Composite {
                logic: *logic,
                conditions: compiled,
            })
        }
    }
}

fn eval_matcher(
    matcher: &CompiledMatcher,
    action: PolicyAction,
    ctx: &TorrentEvalContext<'_>,
) -> Option<String> {
    match matcher {
        CompiledMatcher::KeywordGroup {
            target,
            match_mode,
            case_sensitive,
            words_raw,
            words_lower,
        } => {
            let words = if *case_sensitive {
                words_raw
            } else {
                words_lower
            };
            if words.is_empty() {
                return None;
            }
            match target {
                TextTarget::RawName => {
                    let text = if *case_sensitive {
                        ctx.msg.name.as_str()
                    } else {
                        ctx.raw_lower.as_str()
                    };
                    for (idx, needle) in words.iter().enumerate() {
                        if matches_keyword(text, needle, *match_mode) {
                            return Some(format!("原始标题命中关键词: {}", words_raw[idx]));
                        }
                    }
                    None
                }
                TextTarget::RefinedName => {
                    let text = if *case_sensitive {
                        ctx.refined_name
                    } else {
                        ctx.refined_lower.as_str()
                    };
                    for (idx, needle) in words.iter().enumerate() {
                        if matches_keyword(text, needle, *match_mode) {
                            return Some(format!("清洗标题命中关键词: {}", words_raw[idx]));
                        }
                    }
                    None
                }
                TextTarget::AnyFilePath | TextTarget::AnyFileName => {
                    for file in &ctx.files {
                        let raw_field = if *target == TextTarget::AnyFilePath {
                            file.path
                        } else {
                            file.basename
                        };
                        let owned;
                        let haystack = if *case_sensitive {
                            raw_field
                        } else {
                            owned = raw_field.to_lowercase();
                            &owned
                        };
                        for (idx, needle) in words.iter().enumerate() {
                            if matches_keyword(haystack, needle, *match_mode) {
                                return Some(format!(
                                    "文件 {} 命中关键词: {}",
                                    file.path, words_raw[idx]
                                ));
                            }
                        }
                    }
                    None
                }
            }
        }
        CompiledMatcher::ExtensionRule { scope, exts } => {
            if exts.is_empty() {
                return None;
            }
            match scope {
                ExtensionScope::TorrentName => exts
                    .matches_ext(ctx.title_ext_lower.as_deref(), ctx.raw_trimmed)
                    .map(|ext| format!("标题后缀命中 .{ext}")),
                ExtensionScope::AnyFile => {
                    for file in &ctx.files {
                        if let Some(ext) =
                            exts.matches_ext(file.ext_lower.as_deref(), file.basename)
                        {
                            return Some(format!("文件 {} 命中后缀 .{ext}", file.path));
                        }
                    }
                    None
                }
                ExtensionScope::AllFiles => {
                    if ctx.files.is_empty() {
                        return None;
                    }
                    let all_match = ctx.files.iter().all(|file| {
                        exts.matches_ext(file.ext_lower.as_deref(), file.basename)
                            .is_some()
                    });
                    all_match.then(|| "全部文件均属于指定扩展名集合".to_string())
                }
            }
        }
        CompiledMatcher::TitlePatternPreset {
            patterns,
            min_cyrillic_chars,
        } => {
            for kind in patterns {
                if matches_title_pattern(ctx.raw_trimmed, *kind, *min_cyrillic_chars) {
                    return Some(format!("标题命中结构特征: {}", title_pattern_label(*kind)));
                }
            }
            None
        }
        CompiledMatcher::FileConstraint {
            exts,
            extra_regexes,
            mode,
            min_file_size,
            max_file_size,
            min_file_name_length,
            max_file_name_length,
        } => {
            if exts.is_empty() && extra_regexes.is_empty() {
                return None;
            }
            let matched: Vec<&FileEntry<'_>> = ctx
                .files
                .iter()
                .filter(|file| {
                    exts.matches_ext(file.ext_lower.as_deref(), file.basename)
                        .is_some()
                        || extra_regexes.iter().any(|re| re.is_match(file.path))
                })
                .collect();
            if matched.is_empty() {
                return None;
            }

            let has_bounds = min_file_size.is_some()
                || max_file_size.is_some()
                || min_file_name_length.is_some()
                || max_file_name_length.is_some();

            if !has_bounds {
                return match action {
                    PolicyAction::Reject => {
                        Some(format!("命中无限制拦截文件规则: {}", matched[0].path))
                    }
                    PolicyAction::Allow => Some(format!("命中指定类型文件: {}", matched[0].path)),
                };
            }

            let find_violation = |file: &FileEntry<'_>| -> Option<String> {
                if let Some(min_len) = *min_file_name_length
                    && file.name_char_len < min_len
                {
                    return Some(format!(
                        "匹配文件名字符数 {} < {min_len}: {}",
                        file.name_char_len, file.path
                    ));
                }
                if let Some(max_len) = *max_file_name_length
                    && file.name_char_len > max_len
                {
                    return Some(format!(
                        "匹配文件名字符数 {} > {max_len}: {}",
                        file.name_char_len, file.path
                    ));
                }
                if let Some(min_sz) = *min_file_size
                    && file.size < min_sz
                {
                    return Some(format!(
                        "匹配文件体积 {} < {min_sz}: {}",
                        file.size, file.path
                    ));
                }
                if let Some(max_sz) = *max_file_size
                    && file.size > max_sz
                {
                    return Some(format!(
                        "匹配文件体积 {} > {max_sz}: {}",
                        file.size, file.path
                    ));
                }
                None
            };

            match (mode, action) {
                (FileConstraintMode::EveryMatched, PolicyAction::Reject) => {
                    for file in &matched {
                        if let Some(reason) = find_violation(file) {
                            return Some(reason);
                        }
                    }
                    None
                }
                (FileConstraintMode::EveryMatched, PolicyAction::Allow) => {
                    if matched.iter().all(|file| find_violation(file).is_none()) {
                        Some(format!(
                            "全部 {} 个匹配文件均满足体积与长度要求",
                            matched.len()
                        ))
                    } else {
                        None
                    }
                }
                (FileConstraintMode::AtLeastOneMatched, PolicyAction::Reject) => {
                    if matched.iter().any(|file| find_violation(file).is_none()) {
                        None
                    } else {
                        let sample_reason = matched
                            .first()
                            .and_then(|f| find_violation(f))
                            .unwrap_or_else(|| "无匹配文件满足体积与长度下限".to_string());
                        Some(format!(
                            "所有 {} 个匹配文件均未达标（示例: {sample_reason}）",
                            matched.len()
                        ))
                    }
                }
                (FileConstraintMode::AtLeastOneMatched, PolicyAction::Allow) => matched
                    .iter()
                    .find(|file| find_violation(file).is_none())
                    .map(|ok| format!("存在达标主文件（{} 字节）: {}", ok.size, ok.path)),
            }
        }
        CompiledMatcher::SubstantiveContent {
            junk_exts,
            include_system_junk,
            extra_junk_regexes,
            min_files,
            min_total_size,
        } => {
            let mut sub_count = 0_usize;
            let mut sub_size = 0_u64;
            for file in &ctx.files {
                let is_junk = (*include_system_junk && file.is_system_junk)
                    || junk_exts
                        .matches_ext(file.ext_lower.as_deref(), file.basename)
                        .is_some()
                    || extra_junk_regexes.iter().any(|re| re.is_match(file.path));
                if !is_junk {
                    sub_count += 1;
                    sub_size = sub_size.saturating_add(file.size);
                }
            }

            let count_ok = sub_count >= *min_files;
            let size_ok = *min_total_size == 0 || sub_size >= *min_total_size;
            let satisfied = count_ok && size_ok;

            match action {
                PolicyAction::Reject => {
                    if !count_ok {
                        Some(format!(
                            "排除垃圾文件后实质文件数 {sub_count} < {min_files}"
                        ))
                    } else if !size_ok {
                        Some(format!("实质文件总大小 {sub_size} < {min_total_size}"))
                    } else {
                        None
                    }
                }
                PolicyAction::Allow => satisfied
                    .then(|| format!("包含 {sub_count} 个有效实质文件（合计 {sub_size} 字节）")),
            }
        }
        CompiledMatcher::SizeRatio {
            exts,
            include_split_archives,
            extra_regexes,
            max_ratio,
        } => {
            if ctx.msg.total_size == 0
                || (exts.is_empty() && !*include_split_archives && extra_regexes.is_empty())
            {
                return None;
            }
            let matched_size: u64 = ctx
                .files
                .iter()
                .filter(|file| {
                    (*include_split_archives && file.is_split_archive)
                        || exts
                            .matches_ext(file.ext_lower.as_deref(), file.basename)
                            .is_some()
                        || extra_regexes.iter().any(|re| re.is_match(file.path))
                })
                .map(|file| file.size)
                .sum();
            let ratio = matched_size as f64 / ctx.msg.total_size as f64;
            match action {
                PolicyAction::Reject => {
                    if ratio >= *max_ratio {
                        Some(format!(
                            "指定类型体积占比 {:.1}% >= 上限 {:.1}%",
                            ratio * 100.0,
                            *max_ratio * 100.0
                        ))
                    } else {
                        None
                    }
                }
                PolicyAction::Allow => {
                    if ratio < *max_ratio {
                        Some(format!(
                            "指定类型体积占比 {:.1}% < 上限 {:.1}%",
                            ratio * 100.0,
                            *max_ratio * 100.0
                        ))
                    } else {
                        None
                    }
                }
            }
        }
        CompiledMatcher::CustomRegex {
            target,
            match_all,
            patterns,
        } => {
            if patterns.is_empty() {
                return None;
            }
            match target {
                TextTarget::RawName => {
                    eval_regex_on_text(patterns, *match_all, ctx.raw_trimmed, "原始标题")
                }
                TextTarget::RefinedName => {
                    eval_regex_on_text(patterns, *match_all, ctx.refined_name, "清洗标题")
                }
                TextTarget::AnyFilePath | TextTarget::AnyFileName => {
                    for file in &ctx.files {
                        let field = if *target == TextTarget::AnyFilePath {
                            file.path
                        } else {
                            file.basename
                        };
                        if let Some(hit_desc) = eval_regex_on_text(
                            patterns,
                            *match_all,
                            field,
                            &format!("文件 {}", file.path),
                        ) {
                            return Some(hit_desc);
                        }
                    }
                    None
                }
            }
        }
        CompiledMatcher::Composite { logic, conditions } => {
            if conditions.is_empty() {
                return None;
            }
            match logic {
                CompositeLogic::All => {
                    let mut details = Vec::with_capacity(conditions.len());
                    for cond in conditions {
                        let d = eval_matcher(cond, action, ctx)?;
                        details.push(d);
                    }
                    Some(format!("组合条件全部满足: {}", details.join(" 且 ")))
                }
                CompositeLogic::Any => {
                    for cond in conditions {
                        if let Some(d) = eval_matcher(cond, action, ctx) {
                            return Some(format!("满足组合条件之一: {d}"));
                        }
                    }
                    None
                }
            }
        }
    }
}

fn eval_regex_on_text(
    patterns: &[(String, Regex)],
    match_all: bool,
    text: &str,
    label: &str,
) -> Option<String> {
    if match_all {
        if patterns.iter().all(|(_, re)| re.is_match(text)) {
            Some(format!("{label}命中全部 {} 条自定义正则", patterns.len()))
        } else {
            None
        }
    } else {
        for (raw, re) in patterns {
            if re.is_match(text) {
                return Some(format!("{label}命中正则 {raw}"));
            }
        }
        None
    }
}

fn matches_keyword(haystack: &str, needle: &str, mode: KeywordMatchMode) -> bool {
    match mode {
        KeywordMatchMode::Contains => haystack.contains(needle),
        KeywordMatchMode::WordBoundary => {
            let starts_ascii_alnum = needle
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
            let ends_ascii_alnum = needle
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_alphanumeric());
            for (idx, _) in haystack.match_indices(needle) {
                let left_ok = if starts_ascii_alnum {
                    haystack[..idx]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_ascii_alphanumeric())
                } else {
                    true
                };
                if !left_ok {
                    continue;
                }
                let right_start = idx + needle.len();
                let right_ok = if ends_ascii_alnum {
                    haystack[right_start..]
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_ascii_alphanumeric())
                } else {
                    true
                };
                if right_ok {
                    return true;
                }
            }
            false
        }
    }
}

fn extract_extension_lower(name: &str) -> Option<String> {
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() || ext.contains('/') || ext.contains('\\') {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

fn check_system_junk(path: &str, basename: &str) -> bool {
    let b_lower = basename.to_ascii_lowercase();
    if b_lower.ends_with(".ds_store")
        || b_lower.ends_with("thumbs.db")
        || b_lower.ends_with("desktop.ini")
        || basename.starts_with("._")
    {
        return true;
    }
    let mut parts = path.split('/').peekable();
    while let Some(seg) = parts.next() {
        if parts.peek().is_some() && seg.eq_ignore_ascii_case("__macosx") {
            return true;
        }
    }
    false
}

fn split_dot_digits_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\.(zip|rar|7z|iso|img|tar|gz|tgz|bz2|tbz2|xz|txz|cab|ace)\.\d+$")
            .unwrap_or_else(|e| panic!("内置 split_dot_digits 正则编译失败: {e}"))
    })
}

fn split_part_rar_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\.part\d+\.rar$")
            .unwrap_or_else(|e| panic!("内置 split_part_rar 正则编译失败: {e}"))
    })
}

fn split_rz_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\.[rz]\d{2}$")
            .unwrap_or_else(|e| panic!("内置 split_rz 正则编译失败: {e}"))
    })
}

fn check_split_archive(text: &str) -> bool {
    split_dot_digits_re().is_match(text)
        || split_part_rar_re().is_match(text)
        || split_rz_re().is_match(text)
}

fn matches_title_pattern(trimmed_name: &str, kind: TitlePatternKind, min_cyrillic: usize) -> bool {
    match kind {
        TitlePatternKind::Cyrillic => {
            static CYR: OnceLock<Regex> = OnceLock::new();
            let re = CYR.get_or_init(|| {
                Regex::new(r"\p{Cyrillic}+")
                    .unwrap_or_else(|e| panic!("内置 Cyrillic 正则编译失败: {e}"))
            });
            re.find_iter(trimmed_name)
                .any(|m| m.as_str().chars().count() >= min_cyrillic)
        }
        TitlePatternKind::DigitsOnly => {
            !trimmed_name.is_empty() && trimmed_name.bytes().all(|b| b.is_ascii_digit())
        }
        TitlePatternKind::Sha1Hex => {
            trimmed_name.len() == 40 && trimmed_name.bytes().all(|b| b.is_ascii_hexdigit())
        }
        TitlePatternKind::Md5Hex => {
            trimmed_name.len() == 32 && trimmed_name.bytes().all(|b| b.is_ascii_hexdigit())
        }
        TitlePatternKind::Uuid => {
            static UUID_RE: OnceLock<Regex> = OnceLock::new();
            let re = UUID_RE.get_or_init(|| {
                Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
                    .unwrap_or_else(|e| panic!("内置 UUID 正则编译失败: {e}"))
            });
            re.is_match(trimmed_name)
        }
        TitlePatternKind::DecimalCode => {
            static DEC_RE: OnceLock<Regex> = OnceLock::new();
            let re = DEC_RE.get_or_init(|| {
                Regex::new(r"^\d+\.\d{3}$")
                    .unwrap_or_else(|e| panic!("内置 DecimalCode 正则编译失败: {e}"))
            });
            re.is_match(trimmed_name)
        }
        TitlePatternKind::MagnetPrefix => {
            let bytes = trimmed_name.as_bytes();
            bytes.len() >= 8 && bytes[..8].eq_ignore_ascii_case(b"magnet:?")
        }
        TitlePatternKind::HttpUrlPrefix => {
            let bytes = trimmed_name.as_bytes();
            (bytes.len() >= 7 && bytes[..7].eq_ignore_ascii_case(b"http://"))
                || (bytes.len() >= 8 && bytes[..8].eq_ignore_ascii_case(b"https://"))
        }
        TitlePatternKind::SplitArchiveSuffix => check_split_archive(trimmed_name),
    }
}

fn title_pattern_label(kind: TitlePatternKind) -> &'static str {
    match kind {
        TitlePatternKind::Cyrillic => "西里尔字母文本",
        TitlePatternKind::DigitsOnly => "纯数字标题",
        TitlePatternKind::Sha1Hex => "40位 SHA1 哈希标题",
        TitlePatternKind::Md5Hex => "32位 MD5 哈希标题",
        TitlePatternKind::Uuid => "UUID 编号标题",
        TitlePatternKind::DecimalCode => "纯小数编号标题",
        TitlePatternKind::MagnetPrefix => "magnet: 磁力前缀标题",
        TitlePatternKind::HttpUrlPrefix => "http(s):// 链接前缀标题",
        TitlePatternKind::SplitArchiveSuffix => "分卷压缩包后缀",
    }
}
