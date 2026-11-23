use serde::{Deserialize, Serialize};

pub const FILTER_POLICY_SCHEMA_VERSION: u32 = 2;

fn default_true() -> bool {
    true
}

fn default_one() -> usize {
    1
}

fn default_schema_version() -> u32 {
    FILTER_POLICY_SCHEMA_VERSION
}

fn default_min_cyrillic_chars() -> usize {
    2
}

fn default_ratio() -> f64 {
    0.7
}

fn default_space() -> String {
    " ".to_string()
}

fn default_min_refined_name_length() -> ToggleUsize {
    ToggleUsize::off(2)
}

fn default_span_ad_threshold() -> u8 {
    65
}

/// 将文件扩展名标准化为小写、无前导点号形式（例如 `.MKV` -> `mkv`）。
pub fn normalize_extension(ext: &str) -> String {
    ext.trim().trim_start_matches('.').to_ascii_lowercase()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToggleUsize {
    pub enabled: bool,
    pub value: usize,
}

impl ToggleUsize {
    pub fn on(value: usize) -> Self {
        Self {
            enabled: true,
            value,
        }
    }

    pub fn off(value: usize) -> Self {
        Self {
            enabled: false,
            value,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToggleU64 {
    pub enabled: bool,
    pub value: u64,
}

impl ToggleU64 {
    pub fn on(value: u64) -> Self {
        Self {
            enabled: true,
            value,
        }
    }

    pub fn off(value: u64) -> Self {
        Self {
            enabled: false,
            value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DensityGate {
    pub enabled: bool,
    pub min_file_count: usize,
    pub min_avg_file_size: u64,
}

impl Default for DensityGate {
    fn default() -> Self {
        Self {
            enabled: false,
            min_file_count: 100,
            min_avg_file_size: 102_400,
        }
    }
}

impl DensityGate {
    pub fn on(min_file_count: usize, min_avg_file_size: u64) -> Self {
        Self {
            enabled: true,
            min_file_count,
            min_avg_file_size,
        }
    }

    pub fn off(min_file_count: usize, min_avg_file_size: u64) -> Self {
        Self {
            enabled: false,
            min_file_count,
            min_avg_file_size,
        }
    }
}

/// 阶段 1：基础门禁（显式布尔开关 + 人性化阈值）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterGates {
    #[serde(default = "default_true")]
    pub reject_empty_name: bool,
    pub min_name_length: ToggleUsize,
    pub max_name_length: ToggleUsize,
    #[serde(default = "default_true")]
    pub reject_empty_files: bool,
    pub reject_all_empty_files: bool,
    pub max_empty_files: ToggleUsize,
    pub max_file_count: ToggleUsize,
    pub max_file_name_length: ToggleUsize,
    pub min_total_size: ToggleU64,
    pub max_total_size: ToggleU64,
    pub density_check: DensityGate,
}

impl Default for FilterGates {
    fn default() -> Self {
        Self {
            reject_empty_name: true,
            min_name_length: ToggleUsize::off(4),
            max_name_length: ToggleUsize::off(128),
            reject_empty_files: true,
            reject_all_empty_files: false,
            max_empty_files: ToggleUsize::off(0),
            max_file_count: ToggleUsize::off(2000),
            max_file_name_length: ToggleUsize::off(255),
            min_total_size: ToggleU64::off(1),
            max_total_size: ToggleU64::off(2_199_023_255_552),
            density_check: DensityGate::off(100, 102_400),
        }
    }
}

/// 阶段 2：标题清洗多阶段开关与清洗后标题处理选项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanerOptions {
    #[serde(default = "default_true")]
    pub enable_nfkc: bool,
    #[serde(default = "default_true")]
    pub enable_confusable_norm: bool,
    #[serde(default = "default_true")]
    pub enable_deobfuscate_brackets: bool,
    #[serde(default = "default_true")]
    pub enable_collapse_spaced_chars: bool,
    #[serde(default = "default_true")]
    pub enable_strip_leading_site_prefix: bool,
    #[serde(default = "default_true")]
    pub enable_strip_urls_and_domains: bool,
    #[serde(default = "default_true")]
    pub enable_ad_regexes: bool,
    #[serde(default = "default_true")]
    pub enable_ad_phrases: bool,
    #[serde(default = "default_true")]
    pub enable_span_ad_scoring: bool,
    #[serde(default = "default_span_ad_threshold")]
    pub span_ad_threshold: u8,
    #[serde(default = "default_true")]
    pub enable_filename_cross_clean: bool,
    #[serde(default = "default_true")]
    pub enable_cleanup: bool,
    #[serde(default = "default_true")]
    pub reject_if_empty_after_clean: bool,
    #[serde(default = "default_min_refined_name_length")]
    pub min_refined_name_length: ToggleUsize,
}

impl Default for CleanerOptions {
    fn default() -> Self {
        Self {
            enable_nfkc: true,
            enable_confusable_norm: true,
            enable_deobfuscate_brackets: true,
            enable_collapse_spaced_chars: true,
            enable_strip_leading_site_prefix: true,
            enable_strip_urls_and_domains: true,
            enable_ad_regexes: true,
            enable_ad_phrases: true,
            enable_span_ad_scoring: true,
            span_ad_threshold: default_span_ad_threshold(),
            enable_filename_cross_clean: true,
            enable_cleanup: true,
            reject_if_empty_after_clean: true,
            min_refined_name_length: default_min_refined_name_length(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    #[default]
    Reject,
    Allow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextTarget {
    #[default]
    RawName,
    RefinedName,
    AnyFilePath,
    AnyFileName,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeywordMatchMode {
    #[default]
    Contains,
    WordBoundary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionScope {
    TorrentName,
    #[default]
    AnyFile,
    AllFiles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlePatternKind {
    Cyrillic,
    DigitsOnly,
    Sha1Hex,
    Md5Hex,
    Uuid,
    DecimalCode,
    MagnetPrefix,
    HttpUrlPrefix,
    SplitArchiveSuffix,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileConstraintMode {
    #[default]
    EveryMatched,
    AtLeastOneMatched,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositeLogic {
    #[default]
    All,
    Any,
}

/// 阶段 3：强类型可组合策略匹配器（7 种原子块 + 1 种复合块）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleMatcher {
    KeywordGroup {
        #[serde(default)]
        target: TextTarget,
        #[serde(default)]
        match_mode: KeywordMatchMode,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        words: Vec<String>,
    },
    ExtensionRule {
        #[serde(default)]
        scope: ExtensionScope,
        #[serde(default)]
        extensions: Vec<String>,
    },
    TitlePatternPreset {
        #[serde(default)]
        patterns: Vec<TitlePatternKind>,
        #[serde(default = "default_min_cyrillic_chars")]
        min_cyrillic_chars: usize,
    },
    FileConstraint {
        #[serde(default)]
        extensions: Vec<String>,
        #[serde(default)]
        extra_regexes: Vec<String>,
        #[serde(default)]
        mode: FileConstraintMode,
        #[serde(default)]
        min_file_size: Option<u64>,
        #[serde(default)]
        max_file_size: Option<u64>,
        #[serde(default)]
        min_file_name_length: Option<usize>,
        #[serde(default)]
        max_file_name_length: Option<usize>,
    },
    SubstantiveContent {
        #[serde(default)]
        junk_extensions: Vec<String>,
        #[serde(default = "default_true")]
        include_system_junk: bool,
        #[serde(default)]
        extra_junk_regexes: Vec<String>,
        #[serde(default = "default_one")]
        min_files: usize,
        #[serde(default)]
        min_total_size: u64,
    },
    SizeRatio {
        #[serde(default)]
        extensions: Vec<String>,
        #[serde(default)]
        include_split_archives: bool,
        #[serde(default)]
        extra_regexes: Vec<String>,
        #[serde(default = "default_ratio")]
        max_ratio: f64,
    },
    CustomRegex {
        #[serde(default)]
        target: TextTarget,
        #[serde(default)]
        match_all: bool,
        #[serde(default)]
        patterns: Vec<String>,
    },
    Composite {
        #[serde(default)]
        logic: CompositeLogic,
        #[serde(default)]
        conditions: Vec<RuleMatcher>,
    },
}

impl Default for RuleMatcher {
    fn default() -> Self {
        Self::KeywordGroup {
            target: TextTarget::RawName,
            match_mode: KeywordMatchMode::Contains,
            case_sensitive: false,
            words: Vec::new(),
        }
    }
}

/// 策略规则链中的单条有序规则（支持 `Reject` 拦截 与 `Allow` 白名单豁免）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub action: PolicyAction,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub note: String,
    pub matcher: RuleMatcher,
}

impl Default for PolicyRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: "未命名策略规则".to_string(),
            enabled: true,
            action: PolicyAction::Reject,
            tags: Vec::new(),
            note: String::new(),
            matcher: RuleMatcher::default(),
        }
    }
}

/// Hameta Filter Policy v2 核心策略定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterPolicy {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub gates: FilterGates,
    #[serde(default)]
    pub cleaner: CleanerOptions,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

impl Default for FilterPolicy {
    fn default() -> Self {
        Self {
            schema_version: FILTER_POLICY_SCHEMA_VERSION,
            preset_id: None,
            gates: FilterGates::default(),
            cleaner: CleanerOptions::default(),
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdPhraseRule {
    pub id: String,
    pub phrase: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub note: String,
}

impl Default for AdPhraseRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            phrase: String::new(),
            enabled: true,
            note: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdRegexRule {
    pub id: String,
    pub pattern: String,
    #[serde(default = "default_space")]
    pub replacement: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub note: String,
}

impl Default for AdRegexRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            pattern: String::new(),
            replacement: " ".to_string(),
            enabled: true,
            note: String::new(),
        }
    }
}

/// 完整过滤与清洗版本包（用于落盘、API 传输及 Indexer `ArcSwap` 热更新）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterBundle {
    pub version: i64,
    pub policy: FilterPolicy,
    pub ad_phrases: Vec<AdPhraseRule>,
    pub ad_regexes: Vec<AdRegexRule>,
}

impl Default for FilterBundle {
    fn default() -> Self {
        Self {
            version: 1,
            policy: FilterPolicy::default(),
            ad_phrases: Vec::new(),
            ad_regexes: Vec::new(),
        }
    }
}
