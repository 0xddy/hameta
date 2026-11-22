use crate::error::FilterError;
use crate::preset::{default_ad_phrases, default_ad_regexes};
use crate::schema::{AdPhraseRule, AdRegexRule, CleanerOptions};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

/// 站点 tag 常见 TLD（保守列表，去掉与正常词汇易混淆的 in/me/co/to/am/tv/io 等）。
const BARE_TLDS: &str = "com|net|org|info|biz|xyz|cc|pw|top|vip|club|site|online|shop|su|live|fun|icu|wang|ren|kim|mobi|la|ws|pm|cn";

/// IANA 根域 TLD 大全（ASCII），编译期嵌入 `data/iana-tlds.txt`。
const IANA_TLDS_RAW: &str = include_str!("../data/iana-tlds.txt");

fn parse_tld_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(String::from)
        .collect()
}

fn iana_tld_alternation() -> &'static str {
    static ALT: OnceLock<String> = OnceLock::new();
    ALT.get_or_init(|| parse_tld_lines(IANA_TLDS_RAW).join("|"))
}

fn compile_static_regex(pattern: &str, name: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|err| panic!("静态清洗正则 {name} 编译失败: {err}"))
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleaningStep {
    pub stage: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub before: String,
    pub after: String,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleaningTrace {
    pub raw_name: String,
    pub refined_name: String,
    pub steps: Vec<CleaningStep>,
}

#[derive(Debug, Clone)]
struct CompiledAdRegex {
    regex: Regex,
    replacement: String,
}

/// 支持动态配置阶段开关、广告短语与广告正则的纯本地标题清洗器。
#[derive(Debug, Clone)]
pub struct TitleCleaner {
    options: CleanerOptions,
    ad_phrases: Vec<String>,
    ad_regexes: Vec<CompiledAdRegex>,
}

impl TitleCleaner {
    /// 按启用标记与阶段开关编译动态广告短语与广告正则词库。
    pub fn compile_with_options(
        phrases: &[AdPhraseRule],
        regexes: &[AdRegexRule],
        options: &CleanerOptions,
    ) -> Result<Self, FilterError> {
        let mut active_phrases: Vec<String> = phrases
            .iter()
            .filter(|item| item.enabled)
            .map(|item| item.phrase.trim())
            .filter(|phrase| !phrase.is_empty())
            .map(String::from)
            .collect();
        // 先匹配较长短语，避免「更多资源」误伤「更多资源访问」的剩余部分。
        active_phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.chars().count()));

        let mut active_regexes = Vec::new();
        for item in regexes {
            if !item.enabled {
                continue;
            }
            let pattern = item.pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            let compiled = Regex::new(pattern).map_err(|err| FilterError::InvalidRegex {
                category: "ad_regexes".to_string(),
                rule_id: item.id.clone(),
                pattern: item.pattern.clone(),
                message: err.to_string(),
            })?;
            active_regexes.push(CompiledAdRegex {
                regex: compiled,
                replacement: item.replacement.clone(),
            });
        }

        Ok(Self {
            options: options.clone(),
            ad_phrases: active_phrases,
            ad_regexes: active_regexes,
        })
    }

    /// 使用默认清洗阶段开关编译动态广告短语与广告正则词库。
    pub fn compile(phrases: &[AdPhraseRule], regexes: &[AdRegexRule]) -> Result<Self, FilterError> {
        Self::compile_with_options(phrases, regexes, &CleanerOptions::default())
    }

    /// 构造基于内置 `ad-phrases`（19 条）与 `ad-regexes`（1 条）的默认清洗器。
    pub fn default_baseline() -> Self {
        Self::compile(&default_ad_phrases(), &default_ad_regexes())
            .unwrap_or_else(|err| panic!("内置默认广告词库编译失败: {err}"))
    }

    pub fn options(&self) -> &CleanerOptions {
        &self.options
    }

    /// 执行完整 8 阶段标题清洗管线，返回最终 `refine_name`。
    pub fn refine(&self, raw_name: &str) -> String {
        self.run_pipeline(raw_name, None)
    }

    /// 执行清洗并记录每个阶段的变换明细（供 Playground 调试使用）。
    pub fn refine_with_trace(&self, raw_name: &str) -> CleaningTrace {
        let mut steps = Vec::with_capacity(8);
        let refined_name = self.run_pipeline(raw_name, Some(&mut steps));
        CleaningTrace {
            raw_name: raw_name.to_string(),
            refined_name,
            steps,
        }
    }

    fn run_pipeline(&self, raw_name: &str, mut trace: Option<&mut Vec<CleaningStep>>) -> String {
        let mut text = raw_name.to_string();

        text = apply_stage(
            &mut trace,
            "nfkc",
            self.options.enable_nfkc,
            text,
            |input| input.nfkc().collect::<String>().trim().to_string(),
        );
        text = apply_stage(
            &mut trace,
            "deobfuscate_brackets",
            self.options.enable_deobfuscate_brackets,
            text,
            collapse_obfuscated_brackets,
        );
        text = apply_stage(
            &mut trace,
            "collapse_spaced_chars",
            self.options.enable_collapse_spaced_chars,
            text,
            |input| collapse_spaced_chars(input).into_owned(),
        );
        text = apply_stage(
            &mut trace,
            "strip_leading_site_prefix",
            self.options.enable_strip_leading_site_prefix,
            text,
            strip_leading_site_prefix,
        );
        text = apply_stage(
            &mut trace,
            "strip_urls_and_domains",
            self.options.enable_strip_urls_and_domains,
            text,
            |input| {
                let s = url_regex().replace_all(input, "").into_owned();
                let s = www_regex().replace_all(&s, "").into_owned();
                strip_bare_domains(&s)
            },
        );
        text = apply_stage(
            &mut trace,
            "remove_ad_regexes",
            self.options.enable_ad_regexes,
            text,
            |input| self.remove_ad_regexes(input),
        );
        text = apply_stage(
            &mut trace,
            "remove_ad_phrases",
            self.options.enable_ad_phrases,
            text,
            |input| self.remove_ad_phrases(input),
        );
        apply_stage(
            &mut trace,
            "cleanup",
            self.options.enable_cleanup,
            text,
            cleanup,
        )
    }

    fn remove_ad_phrases(&self, text: &str) -> String {
        let mut result = text.to_string();
        for phrase in &self.ad_phrases {
            if result.contains(phrase) {
                result = result.replace(phrase, " ");
            }
        }
        result
    }

    fn remove_ad_regexes(&self, text: &str) -> String {
        let mut result = text.to_string();
        for item in &self.ad_regexes {
            result = item
                .regex
                .replace_all(&result, item.replacement.as_str())
                .into_owned();
        }
        result
    }
}

fn apply_stage(
    trace: &mut Option<&mut Vec<CleaningStep>>,
    stage: &str,
    enabled: bool,
    before: String,
    op: impl FnOnce(&str) -> String,
) -> String {
    let after = if enabled { op(&before) } else { before.clone() };
    if let Some(steps) = trace.as_deref_mut() {
        let changed = enabled && before != after;
        steps.push(CleaningStep {
            stage: stage.to_string(),
            enabled,
            before,
            after: after.clone(),
            changed,
        });
    }
    after
}

fn collapse_obfuscated_brackets(text: &str) -> String {
    bracket_regex()
        .replace_all(text, |caps: &regex::Captures| {
            let whole = &caps[0];
            let mut chars = whole.chars();
            let open = chars.next().unwrap_or('[');
            let close = whole.chars().last().unwrap_or(']');
            let inner = &whole[open.len_utf8()..whole.len() - close.len_utf8()];

            if is_spaced_out(inner) {
                let collapsed: String = inner.chars().filter(|c| *c != ' ').collect();
                format!("{open}{collapsed}{close}")
            } else {
                whole.to_string()
            }
        })
        .into_owned()
}

fn is_spaced_out(inner: &str) -> bool {
    let tokens: Vec<&str> = inner.split_whitespace().collect();
    tokens.len() >= 3 && tokens.iter().all(|t| t.chars().count() == 1)
}

fn collapse_spaced_chars(text: &str) -> Cow<'_, str> {
    spaced_chars_regex().replace_all(text, |caps: &regex::Captures| {
        caps[0].split_whitespace().collect::<String>()
    })
}

fn strip_leading_site_prefix(text: &str) -> String {
    let text = leading_bracketed_site_prefix_regex()
        .replace(text, "")
        .into_owned();
    leading_site_prefix_regex().replace(&text, "").into_owned()
}

fn strip_bare_domains(text: &str) -> String {
    bare_domain_regex()
        .replace_all(text, |caps: &regex::Captures| caps[1].to_string())
        .into_owned()
}

fn cleanup(text: &str) -> String {
    let mut text = empty_bracket_regex().replace_all(text, "").into_owned();
    text = multi_dot_regex().replace_all(&text, ".").into_owned();
    text = multi_space_regex().replace_all(&text, " ").into_owned();
    text = trailing_sep_before_extension_regex()
        .replace_all(&text, "$1")
        .into_owned();
    text = strip_edge_separators(&text);
    text.trim().to_string()
}

fn strip_edge_separators(text: &str) -> String {
    let mut text = text.trim().to_string();
    loop {
        let stripped = edge_sep_regex().replace_all(&text, "").into_owned();
        let stripped = stripped.trim().to_string();
        if stripped == text {
            break text;
        }
        text = stripped;
    }
}

fn bracket_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"\[[^\[\]]*\]|【[^【】]*】|\([^()]*\)", "bracket"))
}

fn spaced_chars_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"(?:\b\w\b\s+){2,}\b\w\b", "spaced_chars"))
}

fn leading_bracketed_site_prefix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pattern = format!(
            r"(?i)^\s*(?:\[[^\[\]]*(?:https?://)?(?:www\.)?[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{})\b[^\[\]]*\]|【[^【】]*(?:https?://)?(?:www\.)?[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{})\b[^【】]*】)\s*(?:\s+|\s*-\s+|[-_:|]+\s*)?",
            iana_tld_alternation(),
            iana_tld_alternation()
        );
        compile_static_regex(&pattern, "leading_bracketed_site_prefix")
    })
}

fn leading_site_prefix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pattern = format!(
            r"(?i)^\s*(?:(?:https?://)?www\.[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{})\b(?:\s+|\s*-\s+|[-_:|]+\s*)|(?:https?://)?[a-z0-9-]+\.(?:{})\b(?:\s+|\s*-\s+|[-_:|]+\s*))",
            iana_tld_alternation(),
            iana_tld_alternation()
        );
        compile_static_regex(&pattern, "leading_site_prefix")
    })
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"(?i)\bhttps?://[^\s\[\]【】()]+", "url"))
}

fn www_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pattern = format!(
            r"(?i)www\.[a-z0-9-]+(?:\.[a-z0-9-]+)*?\.(?:{})\b",
            iana_tld_alternation()
        );
        compile_static_regex(&pattern, "www")
    })
}

fn bare_domain_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pattern = format!(r"(?i)(^|[\s\[\]【】()_-])[\w\-]+(?:\.[\w\-]+)*?\.(?:{BARE_TLDS})\b");
        compile_static_regex(&pattern, "bare_domain")
    })
}

fn empty_bracket_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"\[\s*\]|【\s*】|\(\s*\)|\{\s*\}", "empty_bracket"))
}

fn multi_dot_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"\.{2,}", "multi_dot"))
}

fn multi_space_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"\s{2,}", "multi_space"))
}

fn trailing_sep_before_extension_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_static_regex(
            r"(?i)[._-](\.[a-z0-9]{1,6}\b)",
            "trailing_sep_before_extension",
        )
    })
}

fn edge_sep_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"^[\s._\-,=*#|~]+|[\s._\-,=*#|~]+$", "edge_sep"))
}
