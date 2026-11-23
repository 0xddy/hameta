use crate::error::FilterError;
use crate::preset::{default_ad_phrases, default_ad_regexes};
use crate::schema::{AdPhraseRule, AdRegexRule, CleanerOptions};
use crate::smart::{
    cross_clean_with_files, normalize_confusables_and_noise, strip_scored_ad_spans,
};
use aho_corasick::{AhoCorasick, MatchKind};
use hameta_common::FileInfo;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

/// 裸域名/无前缀站点 tag 保守 TLD 列表（排除易与国别码、影视词或文件扩展名混淆的 cn/live/vip/top/club/info/cc/tv/me/in/mov/zip 等）。
const BARE_TLDS: &str = "com|net|org|biz|xyz|pw|icu|mobi|ws";

/// IANA 根域 TLD 大全（ASCII），编译期嵌入 `data/iana-tlds.txt`。
const IANA_TLDS_RAW: &str = include_str!("../data/iana-tlds.txt");

fn parse_tld_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.to_ascii_lowercase())
        .collect()
}

fn iana_tld_alternation() -> &'static str {
    static ALT: OnceLock<String> = OnceLock::new();
    ALT.get_or_init(|| parse_tld_lines(IANA_TLDS_RAW).join("|"))
}

pub(crate) fn iana_tld_set() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| parse_tld_lines(IANA_TLDS_RAW).into_iter().collect())
}

fn is_conservative_bare_tld(tld: &str) -> bool {
    matches!(
        tld.to_ascii_lowercase().as_str(),
        "com" | "net" | "org" | "biz" | "xyz" | "pw" | "icu" | "mobi" | "ws"
    )
}

fn is_invisible_or_bad_control(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
    ) || (c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
}

fn strip_invisible_and_nfkc(input: &str) -> String {
    input
        .chars()
        .filter(|&c| !is_invisible_or_bad_control(c))
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .nfkc()
        .collect::<String>()
        .trim()
        .to_string()
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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

/// 支持动态配置阶段开关、智能语块打分、主文件名对齐与广告词库的纯本地标题清洗器。
#[derive(Debug, Clone)]
pub struct TitleCleaner {
    options: CleanerOptions,
    ad_phrase_ac: Option<AhoCorasick>,
    active_phrases_lower: HashSet<String>,
    ad_regexes: Vec<CompiledAdRegex>,
}

impl TitleCleaner {
    /// 按启用标记与阶段开关编译动态广告短语与广告正则词库。
    pub fn compile_with_options(
        phrases: &[AdPhraseRule],
        regexes: &[AdRegexRule],
        options: &CleanerOptions,
    ) -> Result<Self, FilterError> {
        let mut seen = HashSet::new();
        let mut active_phrases = Vec::new();
        for item in phrases {
            if !item.enabled {
                continue;
            }
            let normalized = if options.enable_nfkc {
                strip_invisible_and_nfkc(&item.phrase)
            } else {
                item.phrase.trim().to_string()
            };
            if normalized.is_empty() {
                continue;
            }
            if seen.insert(normalized.to_ascii_lowercase()) {
                active_phrases.push(normalized);
            }
        }
        let ad_phrase_ac = if active_phrases.is_empty() {
            None
        } else {
            Some(
                AhoCorasick::builder()
                    .ascii_case_insensitive(true)
                    .match_kind(MatchKind::LeftmostLongest)
                    .build(&active_phrases)
                    .map_err(|err| {
                        FilterError::InvalidRule(format!("构建广告短语匹配器失败: {err}"))
                    })?,
            )
        };

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

        let mut normalized_options = options.clone();
        normalized_options.span_ad_threshold = normalized_options.span_ad_threshold.clamp(20, 95);

        Ok(Self {
            options: normalized_options,
            ad_phrase_ac,
            active_phrases_lower: seen,
            ad_regexes: active_regexes,
        })
    }

    /// 使用默认清洗阶段开关编译动态广告短语与广告正则词库。
    pub fn compile(phrases: &[AdPhraseRule], regexes: &[AdRegexRule]) -> Result<Self, FilterError> {
        Self::compile_with_options(phrases, regexes, &CleanerOptions::default())
    }

    /// 构造基于内置 `ad-phrases` 与 `ad-regexes` 的默认清洗器。
    pub fn default_baseline() -> Self {
        Self::compile(&default_ad_phrases(), &default_ad_regexes())
            .unwrap_or_else(|err| panic!("内置默认广告词库编译失败: {err}"))
    }

    pub fn options(&self) -> &CleanerOptions {
        &self.options
    }

    pub fn active_phrases_lower(&self) -> &HashSet<String> {
        &self.active_phrases_lower
    }

    /// 执行完整 11 阶段纯标题清洗管线，返回最终 `refine_name`。
    pub fn refine(&self, raw_name: &str) -> String {
        self.run_pipeline(raw_name, &[], None)
    }

    /// 结合种子内文件列表执行完整 11 阶段标题清洗管线，返回最终 `refine_name`。
    pub fn refine_with_files(&self, raw_name: &str, files: &[FileInfo]) -> String {
        self.run_pipeline(raw_name, files, None)
    }

    /// 执行纯标题清洗并记录每个阶段的变换明细（供 Playground 调试使用）。
    pub fn refine_with_trace(&self, raw_name: &str) -> CleaningTrace {
        self.refine_with_files_trace(raw_name, &[])
    }

    /// 结合文件列表执行清洗并记录每个阶段的变换明细（供 Playground 调试使用）。
    pub fn refine_with_files_trace(&self, raw_name: &str, files: &[FileInfo]) -> CleaningTrace {
        let mut steps = Vec::with_capacity(11);
        let refined_name = self.run_pipeline(raw_name, files, Some(&mut steps));
        CleaningTrace {
            raw_name: raw_name.to_string(),
            refined_name,
            steps,
        }
    }

    fn run_pipeline(
        &self,
        raw_name: &str,
        files: &[FileInfo],
        mut trace: Option<&mut Vec<CleaningStep>>,
    ) -> String {
        let mut text = raw_name.to_string();

        text = apply_stage(
            &mut trace,
            "nfkc",
            self.options.enable_nfkc,
            text,
            strip_invisible_and_nfkc,
        );
        text = apply_stage(
            &mut trace,
            "confusable_norm",
            self.options.enable_confusable_norm,
            text,
            normalize_confusables_and_noise,
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
            collapse_spaced_chars,
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
        text = apply_stage_with_detail(
            &mut trace,
            "span_ad_scoring",
            self.options.enable_span_ad_scoring,
            text,
            |input| {
                let (cleaned, hits) = strip_scored_ad_spans(input, self.options.span_ad_threshold);
                let detail = if hits.is_empty() {
                    None
                } else {
                    Some(
                        hits.into_iter()
                            .map(|h| {
                                if h.reasons.is_empty() {
                                    format!("剥离「{}」({}分)", h.text, h.score)
                                } else {
                                    format!(
                                        "剥离「{}」({}分: {})",
                                        h.text,
                                        h.score,
                                        h.reasons.join("+")
                                    )
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("；"),
                    )
                };
                (cleaned, detail)
            },
        );
        text = apply_stage_with_detail(
            &mut trace,
            "filename_cross_clean",
            self.options.enable_filename_cross_clean,
            text,
            |input| cross_clean_with_files(input, files),
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
        let Some(ac) = &self.ad_phrase_ac else {
            return text.to_string();
        };
        let mut result = String::with_capacity(text.len());
        let mut last = 0;
        for m in ac.find_iter(text) {
            result.push_str(&text[last..m.start()]);
            result.push(' ');
            last = m.end();
        }
        if last == 0 {
            return text.to_string();
        }
        result.push_str(&text[last..]);
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
    apply_stage_with_detail(trace, stage, enabled, before, |input| (op(input), None))
}

fn apply_stage_with_detail(
    trace: &mut Option<&mut Vec<CleaningStep>>,
    stage: &str,
    enabled: bool,
    before: String,
    op: impl FnOnce(&str) -> (String, Option<String>),
) -> String {
    let (after, detail) = if enabled {
        op(&before)
    } else {
        (before.clone(), None)
    };
    if let Some(steps) = trace.as_deref_mut() {
        let changed = enabled && before != after;
        steps.push(CleaningStep {
            stage: stage.to_string(),
            enabled,
            before,
            after: after.clone(),
            changed,
            detail,
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

            if should_collapse_bracket_inner(inner) {
                let collapsed: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
                format!("{open}{collapsed}{close}")
            } else {
                whole.to_string()
            }
        })
        .into_owned()
}

fn should_collapse_bracket_inner(inner: &str) -> bool {
    let tokens: Vec<&str> = inner.split_whitespace().collect();
    if tokens.len() >= 3 && tokens.iter().all(|t| t.chars().count() == 1) {
        return true;
    }
    if bracket_spaced_dot_regex().is_match(inner) {
        let collapsed: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
        if bracket_domain_regex().is_match(&collapsed) {
            let lower = collapsed.to_ascii_lowercase();
            let tld = lower.rsplit('.').next().unwrap_or("");
            if lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("www.")
            {
                return iana_tld_set().contains(tld);
            }
            return is_conservative_bare_tld(tld);
        }
    }
    false
}

pub(crate) fn is_cjk_han(c: char) -> bool {
    matches!(
        c as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2A6DF
    )
}

fn collapse_spaced_cjk(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;

    for m in spaced_cjk_regex().find_iter(text) {
        let mut span_start = m.start();
        let mut span_end = m.end();

        if text[..span_start]
            .chars()
            .next_back()
            .is_some_and(is_cjk_han)
        {
            let mut it = text[span_start..span_end].char_indices();
            if let Some((_, _first_han)) = it.next()
                && let Some((after_first, _)) = it.next()
            {
                let mut rest_offset = span_start + after_first;
                while rest_offset < span_end
                    && text[rest_offset..span_end]
                        .chars()
                        .next()
                        .is_some_and(|c| matches!(c, ' ' | '\t'))
                {
                    rest_offset += 1;
                }
                span_start = rest_offset;
            }
        }

        if text[span_end..].chars().next().is_some_and(is_cjk_han)
            && let Some((rel_last_idx, _last_han)) =
                text[span_start..span_end].char_indices().next_back()
        {
            let mut new_end = span_start + rel_last_idx;
            while new_end > span_start
                && text[span_start..new_end]
                    .chars()
                    .next_back()
                    .is_some_and(|c| matches!(c, ' ' | '\t'))
            {
                new_end -= 1;
            }
            span_end = new_end;
        }

        if span_start >= span_end {
            continue;
        }
        let span = &text[span_start..span_end];
        if span.chars().filter(|&c| is_cjk_han(c)).count() < 3 {
            continue;
        }

        out.push_str(&text[last..span_start]);
        out.extend(span.chars().filter(|&c| !matches!(c, ' ' | '\t')));
        last = span_end;
    }

    if last == 0 {
        return text.to_string();
    }
    out.push_str(&text[last..]);
    out
}

fn collapse_spaced_domains(text: &str) -> String {
    let pass1 = spaced_www_domain_regex()
        .replace_all(text, |caps: &regex::Captures| {
            let prefix = caps.get(1).map_or("", |m| m.as_str());
            let domain = caps.get(2).map_or("", |m| m.as_str());
            if !domain.chars().any(char::is_whitespace) {
                return caps[0].to_string();
            }
            let collapsed: String = domain.chars().filter(|c| !c.is_whitespace()).collect();
            let tld = collapsed
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if iana_tld_set().contains(&tld) {
                format!("{prefix}{collapsed}")
            } else {
                caps[0].to_string()
            }
        })
        .into_owned();

    spaced_bare_domain_regex()
        .replace_all(&pass1, |caps: &regex::Captures| {
            let prefix = caps.get(1).map_or("", |m| m.as_str());
            let domain = caps.get(2).map_or("", |m| m.as_str());
            if !domain.chars().any(char::is_whitespace) {
                return caps[0].to_string();
            }
            let collapsed: String = domain.chars().filter(|c| !c.is_whitespace()).collect();
            let mut parts = collapsed.split('.');
            let first = parts.next().unwrap_or("");
            let tld = parts.next().unwrap_or("");
            if parts.next().is_none() && first.len() >= 2 && is_conservative_bare_tld(tld) {
                format!("{prefix}{collapsed}")
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
}

fn collapse_spaced_chars(text: &str) -> String {
    let s = collapse_spaced_domains(text);
    collapse_spaced_cjk(&s)
}

fn strip_leading_site_prefix(text: &str) -> String {
    let mut current = text.to_string();
    for _ in 0..16 {
        let after_bracket = leading_bracketed_site_prefix_regex()
            .replace(&current, "")
            .into_owned();
        let next = leading_site_prefix_regex()
            .replace(&after_bracket, "")
            .into_owned();
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn strip_bare_domains(text: &str) -> String {
    bare_domain_regex()
        .replace_all(text, |caps: &regex::Captures| {
            caps.get(1)
                .or_else(|| caps.get(2))
                .map_or("", |m| m.as_str())
                .to_string()
        })
        .into_owned()
}

fn cleanup(text: &str) -> String {
    let mut current = text.to_string();
    for _ in 0..8 {
        let stripped = empty_bracket_regex().replace_all(&current, "").into_owned();
        if stripped == current {
            break;
        }
        current = stripped;
    }
    current = multi_dot_regex().replace_all(&current, ".").into_owned();
    current = multi_space_regex().replace_all(&current, " ").into_owned();
    current = space_before_known_ext_regex()
        .replace_all(&current, "$1")
        .into_owned();
    current = trailing_sep_before_extension_regex()
        .replace_all(&current, "$1")
        .into_owned();
    current = strip_edge_separators(&current);
    current.trim().to_string()
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
    RE.get_or_init(|| {
        compile_static_regex(
            r"\[[^\[\]]*\]|【[^【】]*】|\([^()]*\)|\{[^{}]*\}|「[^「」]*」|『[^『』]*』|〔[^〔〕]*〕",
            "bracket",
        )
    })
}

fn bracket_spaced_dot_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"\s+\.|\.\s+", "bracket_spaced_dot"))
}

fn bracket_domain_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_static_regex(
            r"(?i)^(?:https?://)?(?:www\.)?[a-z0-9-]+(?:\.[a-z0-9-]+)+$",
            "bracket_domain",
        )
    })
}

fn spaced_cjk_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"(?:\p{Han}[ \t]+){2,}\p{Han}", "spaced_cjk"))
}

fn spaced_www_domain_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_static_regex(
            r"(?i)(^|[^a-z0-9])(w[ \t]*w[ \t]*w[ \t]*\.[ \t]*(?:[a-z0-9](?:[ \t]+\b[a-z0-9]\b)+|[a-z0-9-]+)(?:[ \t]*\.[ \t]*(?:[a-z0-9](?:[ \t]+\b[a-z0-9]\b)+|[a-z0-9-]+))+)",
            "spaced_www_domain",
        )
    })
}

fn spaced_bare_domain_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_static_regex(
            r"(?i)(^|[^a-z0-9])((?:[a-z0-9](?:[ \t]+\b[a-z0-9]\b)+|[a-z0-9-]{2,})[ \t]*\.[ \t]*(?:[a-z0-9](?:[ \t]+\b[a-z0-9]\b)+|[a-z0-9-]{2,}))",
            "spaced_bare_domain",
        )
    })
}

fn leading_bracketed_site_prefix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let iana = iana_tld_alternation();
        let pattern = format!(
            r"(?i)^\s*(?:\[[^\[\]]*(?:(?:https?://|www\.)[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{iana})|[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{BARE_TLDS}))\b[^\[\]]*\]|【[^【】]*(?:(?:https?://|www\.)[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{iana})|[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{BARE_TLDS}))\b[^【】]*】)\s*(?:[-–—_:|：]+\s*)?"
        );
        compile_static_regex(&pattern, "leading_bracketed_site_prefix")
    })
}

fn leading_site_prefix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let iana = iana_tld_alternation();
        let pattern = format!(
            r"(?i)^\s*(?:(?:https?://)?www\.[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{iana})\b|https?://[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:{iana})\b|[a-z0-9-]+\.(?:{BARE_TLDS})\b)(?:/[^\s\[\]【】()]*)?(?:\s+|[-–—_:|：]+\s*|\s*$)"
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
            r"(?i)www\.[a-z0-9-]+(?:\.[a-z0-9-]+)*?\.(?:{})\b(?:/[^\s\[\]【】()]*)?",
            iana_tld_alternation()
        );
        compile_static_regex(&pattern, "www")
    })
}

fn bare_domain_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        let pattern = format!(
            r"(?i)(^|[\s\[\]【】()_-])(?:[a-z0-9-]{{1,12}}\.)?[a-z0-9-]{{2,}}\.(?:{BARE_TLDS})\b(?:/[^\s\[\]【】()]*)?|(\.)[a-z0-9-]{{2,}}\.(?:{BARE_TLDS})\b(?:/[^\s\[\]【】()]*)?"
        );
        compile_static_regex(&pattern, "bare_domain")
    })
}

fn empty_bracket_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_static_regex(
            r"\[[\s._\-,:|/·•—–~+]*\]|【[\s._\-,:|/·•—–~+]*】|\([\s._\-,:|/·•—–~+]*\)|\{[\s._\-,:|/·•—–~+]*\}|「[\s._\-,:|/·•—–~+]*」|『[\s._\-,:|/·•—–~+]*』|〔[\s._\-,:|/·•—–~+]*〕|《[\s._\-,:|/·•—–~+]*》",
            "empty_bracket",
        )
    })
}

fn multi_dot_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"\.{2,}", "multi_dot"))
}

fn multi_space_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_static_regex(r"\s{2,}", "multi_space"))
}

fn space_before_known_ext_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_static_regex(
            r"(?i)\s+(\.(?:mp4|mkv|avi|mov|wmv|flv|webm|m4v|mpg|mpeg|3gp|rm|rmvb|vob|asf|m2ts|ts|flac|ape|wav|mp3|aac|m4a|ogg|zip|rar|7z|iso|pdf|epub|srt|ass|ssa|sub|nfo)\b)",
            "space_before_known_ext",
        )
    })
}

fn trailing_sep_before_extension_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_static_regex(
            r"(?i)[._-]+(\.[a-z0-9]{1,6}\b)",
            "trailing_sep_before_extension",
        )
    })
}

fn edge_sep_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_static_regex(
            r"^[\s._\-,=*#|~:/\\+·•—–、；：]+|[\s._\-,=*#|~:/\\+·•—–、；：]+$",
            "edge_sep",
        )
    })
}
