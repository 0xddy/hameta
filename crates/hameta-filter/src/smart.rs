use crate::cleaner::{iana_tld_set, is_cjk_han};
use hameta_common::FileInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;

/// 语块打分剥离命中详情（供 CleaningTrace 与演练场展示）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoredSpanHit {
    pub text: String,
    pub score: u8,
    pub reasons: Vec<String>,
}

const KNOWN_TRAILING_MEDIA_EXTS: &[&str] = &[
    ".mp4", ".mkv", ".avi", ".mov", ".wmv", ".flv", ".webm", ".m4v", ".mpg", ".mpeg", ".3gp",
    ".rm", ".rmvb", ".vob", ".asf", ".m2ts", ".ts", ".flac", ".ape", ".wav", ".mp3", ".aac",
    ".m4a", ".ogg", ".zip", ".rar", ".7z", ".iso", ".pdf", ".epub", ".srt", ".ass", ".ssa",
    ".sub", ".nfo",
];

const HIGH_RISK_TLDS: &[&str] = &[
    "cc", "tv", "vip", "top", "me", "club", "info", "site", "online", "shop", "fun", "live",
    "pro", "click", "link", "buzz", "lat", "lol", "work", "ink", "gg", "to", "la", "io", "tw",
    "hk", "cn", "com", "net", "org", "xyz", "biz", "pw", "icu", "mobi", "ws",
];

const FILE_EXTENSION_COLLISIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "md", "zip", "rar", "7z", "ts", "m2ts", "iso", "pdf", "txt",
    "nfo", "srt", "ass", "ssa", "sub", "exe", "bin", "img", "sh", "py", "js", "ps1", "mp3",
    "flac", "wav", "aac", "png", "jpg", "jpeg", "gif",
];

const GENERIC_MEDIA_LABELS_LOWER: &[&str] = &[
    "web", "dl", "hd", "fhd", "uhd", "bd", "dvd", "cam", "rip", "webrip", "bdrip", "dvdrip",
    "bluray", "remux", "hevc", "avc", "x264", "x265", "h264", "h265", "aac", "ac3", "dts",
    "truehd", "atmos", "flac", "hdr", "hdr10", "dv", "max", "amzn", "nf", "dsnp", "hmax",
    "atvp", "apple", "android", "live", "vip", "top", "cn", "cc", "tv", "me", "pro", "info",
    "album", "movie", "concert", "show", "part", "season", "episode", "series", "film", "video",
    "clean", "raw", "raws", "proper", "repack", "extended", "uncut", "uncensored", "director",
    "directors", "edition", "complete", "pack",
];

const GENERIC_FILE_STEMS: &[&str] = &[
    "video", "movie", "sample", "file", "main", "data", "setup", "clip", "index", "test", "pack",
    "archive", "film", "media", "output", "default", "unknown", "untitled",
];

const JUNK_OR_SIDECAR_EXTS: &[&str] = &[
    "txt", "url", "lnk", "ink", "html", "htm", "nfo", "jpg", "jpeg", "png", "gif", "bmp", "webp",
    "srt", "ass", "ssa", "sub", "torrent", "sfv", "md5", "sha1", "cue", "log",
];

fn is_promo_emoji(c: char) -> bool {
    matches!(
        c,
        '🔥' | '⭐'
            | '🌟'
            | '💥'
            | '⚡'
            | '🚀'
            | '💎'
            | '🎁'
            | '🎉'
            | '🎊'
            | '✨'
            | '📌'
            | '📍'
            | '🔔'
            | '📣'
            | '📢'
            | '🔞'
            | '👇'
            | '👆'
            | '👉'
            | '👈'
            | '❤'
            | '♥'
            | '💕'
            | '💖'
            | '💯'
            | '✅'
            | '✔'
            | '❌'
            | '🔴'
            | '🟡'
            | '🟢'
            | '🔵'
            | '▶'
            | '►'
            | '★'
            | '☆'
            | '\u{FE0E}'
            | '\u{FE0F}'
    )
}

fn map_enclosed_or_dingbat(c: char) -> Option<&'static str> {
    match c {
        '⓪' => Some("0"),
        '❶' | '➀' | '➊' => Some("1"),
        '❷' | '➁' | '➋' => Some("2"),
        '❸' | '➂' | '➌' => Some("3"),
        '❹' | '➃' | '➍' => Some("4"),
        '❺' | '➄' | '➎' => Some("5"),
        '❻' | '➅' | '➏' => Some("6"),
        '❼' | '➆' | '➐' => Some("7"),
        '❽' | '➇' | '➑' => Some("8"),
        '❾' | '➈' | '➒' => Some("9"),
        '❿' | '➉' | '➓' => Some("10"),
        '\u{1F130}'..='\u{1F149}' => Some(ascii_upper_letter(c as u32 - 0x1F130)),
        '\u{1F150}'..='\u{1F169}' => Some(ascii_upper_letter(c as u32 - 0x1F150)),
        '\u{1F170}'..='\u{1F189}' => Some(ascii_upper_letter(c as u32 - 0x1F170)),
        _ => None,
    }
}

fn ascii_upper_letter(idx: u32) -> &'static str {
    const LETTERS: [&str; 26] = [
        "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R",
        "S", "T", "U", "V", "W", "X", "Y", "Z",
    ];
    LETTERS.get(idx as usize).copied().unwrap_or("")
}

fn map_cyrillic_or_greek_homoglyph(c: char) -> Option<char> {
    match c {
        // Cyrillic -> ASCII
        'а' => Some('a'),
        'А' => Some('A'),
        'е' | 'ё' => Some('e'),
        'Е' | 'Ё' => Some('E'),
        'о' => Some('o'),
        'О' => Some('O'),
        'р' => Some('p'),
        'Р' => Some('P'),
        'с' => Some('c'),
        'С' => Some('C'),
        'х' => Some('x'),
        'Х' => Some('X'),
        'у' => Some('y'),
        'У' => Some('Y'),
        'к' => Some('k'),
        'К' => Some('K'),
        'м' => Some('m'),
        'М' => Some('M'),
        'т' => Some('t'),
        'Т' => Some('T'),
        'н' => Some('h'),
        'Н' => Some('H'),
        'в' => Some('b'),
        'В' => Some('B'),
        'і' | 'ї' => Some('i'),
        'І' | 'Ї' => Some('I'),
        'ј' => Some('j'),
        'Ј' => Some('J'),
        'ѕ' => Some('s'),
        'Ѕ' => Some('S'),
        'п' => Some('n'),
        // Greek -> ASCII
        'α' => Some('a'),
        'Α' => Some('A'),
        'ε' => Some('e'),
        'Ε' => Some('E'),
        'ο' => Some('o'),
        'Ο' => Some('O'),
        'ρ' => Some('p'),
        'Ρ' => Some('P'),
        'κ' => Some('k'),
        'Κ' => Some('K'),
        'ν' => Some('v'),
        'Ν' => Some('N'),
        'τ' => Some('t'),
        'Τ' => Some('T'),
        'χ' => Some('x'),
        'Χ' => Some('X'),
        'ι' => Some('i'),
        'Ι' => Some('I'),
        'β' => Some('b'),
        'Β' => Some('B'),
        'μ' => Some('m'),
        'Μ' => Some('M'),
        _ => None,
    }
}

fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '-' | '_')
}

/// L0 增强：词元级同形字归一化 + 非 NFKC 带圈/黑底字符折叠 + 营销符号降噪。
///
/// 安全约束：
/// 1. 仅当词元内部混用 ASCII 字母数字与西里尔/希腊同形字，或折叠后形成已知 TLD 域名时，才将同形字折回 ASCII；
///    纯俄文等正常西里尔词保持原样（确保语言过滤与正常多语种标题不失真）。
/// 2. 不修改全角 ASCII（`\u{FF01}..=\u{FF5E}`），以尊重 `enable_nfkc = false` 阶段开关。
pub fn normalize_confusables_and_noise(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();

    while let Some(&(start_idx, ch)) = chars.peek() {
        if !is_token_char(ch) {
            chars.next();
            if is_promo_emoji(ch) {
                out.push(' ');
            } else if let Some(repl) = map_enclosed_or_dingbat(ch) {
                out.push_str(repl);
            } else {
                out.push(ch);
            }
            continue;
        }

        let mut end_idx = start_idx + ch.len_utf8();
        chars.next();
        while let Some(&(idx, next_c)) = chars.peek() {
            if !is_token_char(next_c) {
                break;
            }
            end_idx = idx + next_c.len_utf8();
            chars.next();
        }
        let token = &input[start_idx..end_idx];

        let mut has_homoglyph = false;
        let mut has_ascii_alnum = false;
        let mut has_enclosed = false;
        for c in token.chars() {
            if map_cyrillic_or_greek_homoglyph(c).is_some() {
                has_homoglyph = true;
            }
            if c.is_ascii_alphanumeric() {
                has_ascii_alnum = true;
            }
            if map_enclosed_or_dingbat(c).is_some() {
                has_enclosed = true;
            }
        }

        if !has_homoglyph && !has_enclosed {
            out.push_str(token);
            continue;
        }

        // 先构造折叠后副本，用于判断是否构成合法 TLD 域名伪装（如 `ооо.соm`）
        let mut folded = String::with_capacity(token.len());
        for c in token.chars() {
            if let Some(repl) = map_enclosed_or_dingbat(c) {
                folded.push_str(repl);
            } else if let Some(ascii) = map_cyrillic_or_greek_homoglyph(c) {
                folded.push(ascii);
            } else {
                folded.push(c);
            }
        }

        let should_fold_homoglyphs = if has_homoglyph {
            if has_ascii_alnum {
                true
            } else if let Some((_, tld)) = folded.rsplit_once('.') {
                !tld.is_empty()
                    && tld.chars().all(|c| c.is_ascii_alphabetic())
                    && iana_tld_set().contains(&tld.to_ascii_lowercase())
            } else {
                false
            }
        } else {
            false
        };

        if should_fold_homoglyphs {
            out.push_str(&folded);
        } else if has_enclosed {
            for c in token.chars() {
                if let Some(repl) = map_enclosed_or_dingbat(c) {
                    out.push_str(repl);
                } else {
                    out.push(c);
                }
            }
        } else {
            out.push_str(token);
        }
    }

    out
}

#[derive(Debug, Clone)]
pub(crate) struct TitleSpan<'a> {
    pub leading_sep: &'a str,
    pub raw: &'a str,
    pub inner: &'a str,
    pub is_bracketed: bool,
    pub has_clause_sep_before: bool,
}

fn matching_close_bracket(open: char) -> Option<char> {
    match open {
        '[' => Some(']'),
        '【' => Some('】'),
        '(' => Some(')'),
        '（' => Some('）'),
        '「' => Some('」'),
        '『' => Some('』'),
        '《' => Some('》'),
        '{' => Some('}'),
        _ => None,
    }
}

fn split_trailing_known_ext(text: &str) -> (&str, Option<&str>) {
    let trimmed_end = text.trim_end();
    let lower = trimmed_end.to_ascii_lowercase();
    for ext in KNOWN_TRAILING_MEDIA_EXTS {
        if lower.ends_with(ext) && trimmed_end.len() > ext.len() {
            let split_at = trimmed_end.len() - ext.len();
            // 确保扩展名前不是空白紧接另一点号，或至少存在有效主体
            let body = trimmed_end[..split_at].trim_end();
            if !body.is_empty() {
                return (body, Some(&trimmed_end[split_at..]));
            }
        }
    }
    (text, None)
}

fn is_clause_separator_char(c: char) -> bool {
    matches!(c, '|' | '｜' | ',' | '，' | ';' | '；' | '!' | '！' | '/')
}

fn push_non_bracket_gap<'a>(
    spans: &mut Vec<TitleSpan<'a>>,
    gap: &'a str,
    mut pending_sep: &'a str,
    mut clause_sep_before: bool,
) -> (&'a str, bool) {
    // 优先按强分句符号（逗号、竖线、斜杠、分号、" - "、" — "）切分，再在内部按空白切词元
    let bytes = gap.as_bytes();
    let mut chunk_start = 0;
    let mut i = 0;

    let mut emit_whitespace_words = |sub: &'a str, p_sep: &mut &'a str, c_sep: &mut bool| {
        let mut cursor = 0;
        let mut local_clause = *c_sep;
        let mut first = true;
        for (idx, c) in sub.char_indices() {
            if c.is_whitespace() {
                if cursor < idx {
                    let word = &sub[cursor..idx];
                    spans.push(TitleSpan {
                        leading_sep: if first { *p_sep } else { "" },
                        raw: word,
                        inner: word,
                        is_bracketed: false,
                        has_clause_sep_before: local_clause,
                    });
                    *p_sep = "";
                    first = false;
                    local_clause = false;
                }
                cursor = idx + c.len_utf8();
            }
        }
        if cursor < sub.len() {
            let word = &sub[cursor..];
            spans.push(TitleSpan {
                leading_sep: if first { *p_sep } else { "" },
                raw: word,
                inner: word,
                is_bracketed: false,
                has_clause_sep_before: local_clause,
            });
            *p_sep = "";
            *c_sep = false;
        }
    };

    while i < bytes.len() {
        let ch = gap[i..].chars().next().unwrap();
        let ch_len = ch.len_utf8();

        let is_spaced_dash = matches!(ch, '-' | '—' | '–')
            && i > 0
            && gap[..i].chars().next_back().is_some_and(char::is_whitespace)
            && gap[i + ch_len..].chars().next().is_some_and(char::is_whitespace);

        if is_clause_separator_char(ch) || is_spaced_dash {
            let before = &gap[chunk_start..i];
            if !before.trim().is_empty() {
                emit_whitespace_words(before, &mut pending_sep, &mut clause_sep_before);
            }
            pending_sep = &gap[i..i + ch_len];
            clause_sep_before = true;
            i += ch_len;
            chunk_start = i;
            continue;
        }
        i += ch_len;
    }

    if chunk_start < gap.len() {
        let tail = &gap[chunk_start..];
        if !tail.trim().is_empty() {
            emit_whitespace_words(tail, &mut pending_sep, &mut clause_sep_before);
        }
    }

    (pending_sep, clause_sep_before)
}

pub(crate) fn segment_spans(body: &str) -> Vec<TitleSpan<'_>> {
    let mut spans = Vec::new();
    let mut cursor = 0;
    let mut idx = 0;
    let mut pending_sep = "";
    let mut clause_sep_before = false;

    while idx < body.len() {
        let ch = body[idx..].chars().next().unwrap();
        if let Some(close) = matching_close_bracket(ch) {
            let search_from = idx + ch.len_utf8();
            // 在同级找到配对闭括号（遇到另一个同类开括号则先跳过，避免跨括错配）
            let mut depth = 1usize;
            let mut close_pos = None;
            for (rel, c) in body[search_from..].char_indices() {
                if c == ch {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        close_pos = Some((search_from + rel, c.len_utf8()));
                        break;
                    }
                }
            }

            if let Some((close_idx, close_len)) = close_pos {
                if idx > cursor {
                    let gap = &body[cursor..idx];
                    (pending_sep, clause_sep_before) =
                        push_non_bracket_gap(&mut spans, gap, pending_sep, clause_sep_before);
                }
                let raw = &body[idx..close_idx + close_len];
                let inner = &body[search_from..close_idx];
                spans.push(TitleSpan {
                    leading_sep: pending_sep,
                    raw,
                    inner: inner.trim(),
                    is_bracketed: true,
                    has_clause_sep_before: clause_sep_before,
                });
                pending_sep = "";
                clause_sep_before = false;
                idx = close_idx + close_len;
                cursor = idx;
                continue;
            }
        }
        idx += ch.len_utf8();
    }

    if cursor < body.len() {
        let tail = &body[cursor..];
        let _ = push_non_bracket_gap(&mut spans, tail, pending_sep, clause_sep_before);
    }

    spans
}

fn media_token_regex() -> &'static RegexLike {
    static RE: OnceLock<RegexLike> = OnceLock::new();
    RE.get_or_init(RegexLike::new)
}

use regex::Regex;

struct RegexLike {
    year: Regex,
    ep_season: Regex,
    res_dim: Regex,
    media_kw: Regex,
    cjk_ep: Regex,
    release_suffix: Regex,
    mixed_alnum_id: Regex,
    contact_handle: Regex,
}

impl RegexLike {
    fn new() -> Self {
        Self {
            year: Regex::new(r"^(?:19\d{2}|20[0-3]\d)$").unwrap(),
            ep_season: Regex::new(r"(?i)^(?:s\d{1,3}(?:e\d{1,3})?|e(?:p)?\d{1,4}|ova|oad|sp\d{0,2}|special)$").unwrap(),
            res_dim: Regex::new(r"(?i)^(?:\d{3,4}[pi]|4k|8k|\d{3,4}x\d{3,4}|(?:4|8|10|12)bit|fps\d*)$").unwrap(),
            media_kw: Regex::new(r"(?i)\b(?:bluray|blu-ray|bdrip|web-dl|webdl|webrip|hdrip|dvdrip|hdcam|hdtv|remux|hevc|avc|x264|x265|h\.?264|h\.?265|aac|ac3|eac3|dts(?:-hd)?|truehd|atmos|ddp?\d(?:\.\d)?|flac|alac|opus|hdr10\+?|dolby|dv|mkv|mp4|avi|iso|esub|subs?|dual|multi|castellano|french|english|japanese|korean|russian|proper|repack|complete|season|yts\.mx|fansub)\b").unwrap(),
            cjk_ep: Regex::new(r"第\s*\d+\s*[集话期季部回]|全\s*\d+\s*[集话]|国语|粤语|台配|中字|简繁|简体|繁体|双语|字幕|特效字幕|未删减").unwrap(),
            release_suffix: Regex::new(r"(?i)(?:-raws|fansub|subs?)$|字幕组$|压制组$").unwrap(),
            mixed_alnum_id: Regex::new(r"(?i)(?:(?:^|[^a-z0-9])(?:vx|wx|qq|tg)|微|薇|威|裙|群|电报|飞机)[:：\s@_-]{0,3}[a-z0-9_]{5,}|(?:^|[^a-z0-9])v\s*[:：]\s*[a-z0-9_]{4,}|@[a-z0-9_]{4,}").unwrap(),
            contact_handle: Regex::new(r"(?i)^(?:vx|wx|qq|tg|v)[:：_-]\S{3,}$").unwrap(),
        }
    }
}

/// 判断语块是否主要为正规影视/音乐/动漫元信息（供打分保护与自发现降噪使用）。
pub(crate) fn is_protected_media_span(inner: &str) -> bool {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return false;
    }
    let re = media_token_regex();
    if re.year.is_match(trimmed) || re.ep_season.is_match(trimmed) || re.res_dim.is_match(trimmed) {
        return true;
    }
    if re.release_suffix.is_match(trimmed) {
        return true;
    }
    if re.cjk_ep.is_match(trimmed) && !trimmed.contains("推广") {
        return true;
    }
    if re.media_kw.is_match(trimmed) {
        return true;
    }
    false
}

/// 静态特征加权表（含字符 2-gram/3-gram 与语义词素，无重型正则）。
const SOCIAL_OBFUSCATIONS: &[(&str, i32, &str)] = &[
    ("加微", 50, "社交引流"),
    ("加薇", 50, "社交引流"),
    ("加威", 50, "社交引流"),
    ("威信", 50, "社交引流"),
    ("薇芯", 50, "社交引流"),
    ("微芯", 50, "社交引流"),
    ("v信", 50, "社交引流"),
    ("vx:", 50, "社交引流"),
    ("vx：", 50, "社交引流"),
    ("wx:", 50, "社交引流"),
    ("wx：", 50, "社交引流"),
    ("qq:", 48, "QQ引流"),
    ("qq：", 48, "QQ引流"),
    ("tg:", 50, "Telegram引流"),
    ("tg：", 50, "Telegram引流"),
    ("tg群", 50, "Telegram引流"),
    ("进群", 48, "进群引流"),
    ("加群", 48, "加群引流"),
    ("进裙", 52, "谐音裙引流"),
    ("加裙", 52, "谐音裙引流"),
    ("君羊", 48, "拆字群引流"),
    ("抠抠", 48, "谐音QQ引流"),
    ("扣扣", 45, "谐音QQ引流"),
    ("电报", 45, "电报引流"),
    ("纸飞机", 48, "电报引流"),
    ("飞机群", 50, "电报引流"),
    ("公众号", 45, "公众号引流"),
    ("私域", 45, "私域引流"),
    ("扫码", 42, "扫码引流"),
];

const ANTI_LOSS_CUES: &[(&str, i32, &str)] = &[
    ("新址", 42, "防失联域名变体"),
    ("新域", 42, "防失联域名变体"),
    ("备用址", 42, "备用地址变体"),
    ("备用域", 42, "备用域名变体"),
    ("防失联", 45, "防失联引导"),
    ("防迷路", 45, "防迷路引导"),
    ("防走丢", 45, "防走丢引导"),
    ("防和谐", 42, "防和谐引导"),
    ("防封", 38, "防封引导"),
    ("发布页", 42, "发布页"),
    ("导航站", 42, "导航站"),
    ("永久地址", 45, "永久地址"),
    ("永久域名", 45, "永久域名"),
    ("最新地址", 45, "最新地址"),
    ("最新域名", 45, "最新域名"),
    ("地址发布", 42, "地址发布"),
    ("获取最新", 38, "获取最新"),
    ("免翻", 38, "翻墙指引"),
    ("直连", 32, "直连指引"),
    ("不迷路", 45, "防迷路引导"),
    ("发邮件获取", 48, "邮箱回传网址"),
];

const PROMO_VERBS: &[(&str, i32, &str)] = &[
    ("首发", 28, "站点首发标识"),
    ("独家发布", 34, "独家发布水印"),
    ("每日更新", 34, "更新频率水印"),
    ("持续更新", 34, "更新频率水印"),
    ("免费看", 34, "免费观看引导"),
    ("免费观看", 34, "免费观看引导"),
    ("在线看", 32, "在线观看引导"),
    ("在线观看", 32, "在线观看引导"),
    ("无水印", 28, "封装水印声明"),
    ("看完整", 34, "诱导看完整版"),
    ("更多资源", 28, "更多资源引流"),
    ("请访问", 34, "访问域名引导"),
    ("资源群", 42, "资源群引流"),
    ("资源站", 38, "资源站水印"),
    ("影视网", 34, "影视站水印"),
    ("磁力站", 38, "磁力站水印"),
    ("bt站", 38, "BT站水印"),
    ("限时领取", 38, "营销领取"),
    ("扫码领取", 45, "扫码领取"),
    ("点击进入", 38, "点击引导"),
    ("官网", 30, "官网水印"),
    ("入口", 26, "入口引导"),
    ("福利", 26, "福利引流"),
    ("完整版", 22, "完整版营销"),
];

fn is_release_part_label(label_lower: &str) -> bool {
    for prefix in ["cd", "disc", "disk", "part", "vol", "track", "ch", "ep", "ver", "v"] {
        if let Some(rest) = label_lower.strip_prefix(prefix)
            && !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

fn is_media_extension_or_tag(token_lower: &str) -> bool {
    FILE_EXTENSION_COLLISIONS.contains(&token_lower)
        || GENERIC_MEDIA_LABELS_LOWER.contains(&token_lower)
        || is_release_part_label(token_lower)
        || is_protected_media_span(token_lower)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DomainRiskHit {
    None,
    HighConfidence,
    ContextDependent,
}

fn classify_high_risk_domain(inner: &str, context_promo_or_bracket: bool) -> DomainRiskHit {
    for piece in inner.split(|c: char| c.is_whitespace() || matches!(c, '|' | '/' | ',' | '，' | '；' | ';' | '：' | ':' | '[' | ']' | '【' | '】' | '(' | ')' | '（' | '）')) {
        let tok = piece.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-');
        if tok.len() < 4 || !tok.contains('.') {
            continue;
        }
        let parts: Vec<&str> = tok.split('.').collect();
        if parts.len() < 2 {
            continue;
        }
        let tld_raw = parts[parts.len() - 1];
        let tld = tld_raw.to_ascii_lowercase();
        if !HIGH_RISK_TLDS.contains(&tld.as_str()) {
            continue;
        }
        let label_raw = parts[parts.len() - 2];
        if label_raw.len() < 2 || !label_raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        let label = label_raw.to_ascii_lowercase();
        if is_media_extension_or_tag(&label) {
            continue;
        }
        // 避免 Apple.TV / Android.TV 误杀
        if tld == "tv" && matches!(label.as_str(), "apple" | "android" | "google" | "smart") {
            continue;
        }
        // 长点分链（如 Avengers.2024.1080p.WebRip.CN）只要包含媒体词且前标无数字混写，就保守跳过
        let has_media_sibling = parts[..parts.len() - 1]
            .iter()
            .any(|p| is_media_extension_or_tag(&p.to_ascii_lowercase()));

        let has_digit = label.chars().any(|c| c.is_ascii_digit());
        let has_alpha = label.chars().any(|c| c.is_ascii_alphabetic());
        let is_mixed_alnum = has_digit && has_alpha;
        let is_long_digits = has_digit && !has_alpha && label.len() >= 5;
        let all_upper_short = label_raw.len() <= 4 && label_raw.chars().all(|c| !c.is_ascii_lowercase());

        if (is_mixed_alnum || is_long_digits) && !has_media_sibling {
            return DomainRiskHit::HighConfidence;
        }
        if context_promo_or_bracket && !has_media_sibling && !all_upper_short && label.len() >= 3 {
            return DomainRiskHit::ContextDependent;
        }
    }
    DomainRiskHit::None
}

fn contains_high_risk_domain(inner: &str, context_promo_or_bracket: bool) -> bool {
    classify_high_risk_domain(inner, context_promo_or_bracket) != DomainRiskHit::None
}

/// 片段级线性特征 + 字符 n-gram 打分器。
pub(crate) fn score_span(
    inner: &str,
    is_bracketed: bool,
    is_edge: bool,
    has_clause_sep_before: bool,
) -> (i32, Vec<&'static str>) {
    let normalized = inner
        .nfkc()
        .collect::<String>()
        .trim()
        .to_string();
    if normalized.is_empty() {
        return (0, Vec::new());
    }
    let lower = normalized.to_lowercase();
    let re = media_token_regex();

    let mut score: i32 = 0;
    let mut reasons: Vec<&'static str> = Vec::new();
    let mut push_reason = |r: &'static str| {
        if !reasons.contains(&r) {
            reasons.push(r);
        }
    };

    let mut social_hits = 0;
    for &(cue, w, reason) in SOCIAL_OBFUSCATIONS {
        if lower.contains(cue) {
            score += w;
            push_reason(reason);
            social_hits += 1;
            if social_hits >= 2 {
                break;
            }
        }
    }

    if re.mixed_alnum_id.is_match(&normalized) || re.contact_handle.is_match(&normalized) {
        score += 45;
        push_reason("联系方式ID");
    }

    let mut anti_loss_hits = 0;
    for &(cue, w, reason) in ANTI_LOSS_CUES {
        if lower.contains(cue) {
            score += w;
            push_reason(reason);
            anti_loss_hits += 1;
            if anti_loss_hits >= 2 {
                break;
            }
        }
    }

    let mut promo_hits = 0;
    for &(cue, w, reason) in PROMO_VERBS {
        if lower.contains(cue) {
            score += w;
            push_reason(reason);
            promo_hits += 1;
            if promo_hits >= 2 {
                break;
            }
        }
    }

    let has_cjk = normalized.chars().any(is_cjk_han);
    let context_promo_or_bracket = is_bracketed && (has_cjk || score > 0) || score > 0;
    let domain_risk = classify_high_risk_domain(&normalized, context_promo_or_bracket);
    let has_risk_domain = domain_risk != DomainRiskHit::None;
    match domain_risk {
        DomainRiskHit::HighConfidence => {
            score += 68;
            push_reason("高风险TLD域名水印");
        }
        DomainRiskHit::ContextDependent => {
            score += if is_bracketed || is_edge || promo_hits > 0 || anti_loss_hits > 0 {
                58
            } else {
                45
            };
            push_reason("高风险TLD域名水印");
        }
        DomainRiskHit::None => {}
    }

    // 位置先验：仅当存在任意风险词素/域名信号时叠加，避免伤及 [Beatrice-Raws] / 【重度BDSM】 等纯中性括号
    if score > 0 {
        if is_bracketed && is_edge {
            score += 12;
        }
        if has_clause_sep_before {
            score += 10;
        }
    }

    // 媒体保护降权：无社交引流、无高风险域名且无反失联信号时保护影视标签
    if is_protected_media_span(&normalized) && social_hits == 0 && !has_risk_domain && anti_loss_hits == 0 {
        score -= 55;
    }

    (score.clamp(0, 100), reasons)
}

fn reconstruct_body(spans: &[TitleSpan<'_>], stripped: &[bool]) -> String {
    let mut out = String::new();
    let mut any_kept = false;
    for (idx, sp) in spans.iter().enumerate() {
        if stripped[idx] {
            continue;
        }
        if any_kept {
            if sp.has_clause_sep_before && !sp.leading_sep.is_empty() && !stripped[idx.saturating_sub(1)] {
                if matches!(sp.leading_sep, "," | "，" | "/" | "|") {
                    out.push_str(sp.leading_sep);
                } else {
                    out.push(' ');
                    out.push_str(sp.leading_sep);
                    out.push(' ');
                }
            } else {
                out.push(' ');
            }
        }
        out.push_str(sp.raw);
        any_kept = true;
    }
    out
}

fn is_ascii_contact_payload(raw: &str) -> bool {
    let t = raw.trim();
    t.len() >= 4
        && t.len() <= 36
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@'))
        && !is_protected_media_span(t)
}

/// L2a 阶段：语块分割 + 线性特征打分 + 极简 CRF 相邻传播剥离。
pub fn strip_scored_ad_spans(text: &str, threshold: u8) -> (String, Vec<ScoredSpanHit>) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let threshold_i32 = i32::from(threshold.clamp(20, 95));
    let (body, trailing_ext) = split_trailing_known_ext(trimmed);
    let spans = segment_spans(body);
    if spans.is_empty() {
        return (text.to_string(), Vec::new());
    }

    let n = spans.len();
    let mut scores = vec![0_i32; n];
    let mut reasons_vec: Vec<Vec<&'static str>> = Vec::with_capacity(n);
    let mut protected = vec![false; n];

    for (i, sp) in spans.iter().enumerate() {
        let is_edge = i == 0 || i + 1 == n;
        let (sc, rs) = score_span(sp.inner, sp.is_bracketed, is_edge, sp.has_clause_sep_before);
        scores[i] = sc;
        reasons_vec.push(rs);
        protected[i] = is_protected_media_span(sp.inner) && sc < 50;
    }

    // 极简 CRF 相邻传播：沿首部与尾部高风险广告链向外/向内传播（含 "加微: payload" 冒号负载连带）
    let mut final_scores = scores.clone();
    let mut lead_active = n > 0 && final_scores[0] >= threshold_i32;
    for i in 1..n {
        if !lead_active {
            break;
        }
        let prev_colon = spans[i - 1].raw.ends_with(':') || spans[i - 1].raw.ends_with('：');
        if prev_colon && !spans[i].is_bracketed && is_ascii_contact_payload(spans[i].raw) {
            final_scores[i] = final_scores[i].max(72);
            if !reasons_vec[i].contains(&"紧随联系方式冒号负载") {
                reasons_vec[i].push("紧随联系方式冒号负载");
            }
        } else if !protected[i] && final_scores[i] >= 25 {
            final_scores[i] = (final_scores[i] + 30).clamp(0, 100);
        }
        lead_active = final_scores[i] >= threshold_i32;
    }

    let mut trail_active = n > 1 && final_scores[n - 1] >= threshold_i32;
    for i in (0..n.saturating_sub(1)).rev() {
        if !trail_active {
            break;
        }
        if !protected[i] && final_scores[i] >= 25 {
            final_scores[i] = (final_scores[i] + 30).clamp(0, 100);
        }
        trail_active = final_scores[i] >= threshold_i32;
    }

    let mut stripped = vec![false; n];
    let mut hits = Vec::new();
    for i in 0..n {
        if final_scores[i] >= threshold_i32 {
            stripped[i] = true;
            hits.push(ScoredSpanHit {
                text: spans[i].raw.to_string(),
                score: u8::try_from(final_scores[i]).unwrap_or(100),
                reasons: reasons_vec[i].iter().map(|s| (*s).to_string()).collect(),
            });
        }
    }

    if hits.is_empty() {
        return (text.to_string(), Vec::new());
    }

    // 安全兜底：如果全部语块都要被删光，仅当全部分数 >= 80（纯广告标题）才允许置空；否则保留得分最低的一个语块防误杀
    if stripped.iter().all(|&s| s) {
        let all_extreme = final_scores.iter().all(|&s| s >= 80);
        if all_extreme {
            return (String::new(), hits);
        }
        if let Some((min_idx, _)) = final_scores.iter().enumerate().min_by_key(|(_, s)| *s) {
            stripped[min_idx] = false;
            let min_text = spans[min_idx].raw;
            hits.retain(|h| h.text != min_text);
        }
    }

    let mut cleaned = reconstruct_body(&spans, &stripped);
    if cleaned.trim().is_empty() {
        return (String::new(), hits);
    }
    if let Some(ext) = trailing_ext {
        cleaned.push_str(ext);
    }
    (cleaned, hits)
}

fn is_system_junk_file(path: &str, basename: &str) -> bool {
    let lower_path = path.to_ascii_lowercase();
    let lower_base = basename.to_ascii_lowercase();
    lower_base == ".ds_store"
        || lower_base == "thumbs.db"
        || lower_base == "desktop.ini"
        || lower_base.starts_with("._")
        || lower_path.contains("__macosx/")
        || lower_path.starts_with("__macosx/")
}

fn extract_anchor_basename_and_stem(files: &[FileInfo]) -> Option<(&str, &str)> {
    let anchor = files
        .iter()
        .filter(|f| f.size > 0)
        .filter(|f| {
            let basename = f.path.rsplit('/').next().unwrap_or(&f.path).trim();
            if basename.is_empty() || is_system_junk_file(&f.path, basename) {
                return false;
            }
            if let Some((_, ext)) = basename.rsplit_once('.') {
                !JUNK_OR_SIDECAR_EXTS.contains(&ext.to_ascii_lowercase().as_str())
            } else {
                true
            }
        })
        .max_by_key(|f| f.size)?;

    let basename = anchor.path.rsplit('/').next().unwrap_or(&anchor.path).trim();
    let stem = match basename.rsplit_once('.') {
        Some((s, ext)) if ext.len() <= 6 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => s,
        _ => basename,
    };
    let stem_lower = stem.trim().to_ascii_lowercase();
    if stem_lower.chars().filter(|c| c.is_alphanumeric()).count() < 4 {
        return None;
    }
    let stem_prefix = stem_lower
        .split(['.', '-', '_', ' '])
        .find(|p| !p.is_empty())
        .unwrap_or(&stem_lower);
    if GENERIC_FILE_STEMS.contains(&stem_lower.as_str())
        || (GENERIC_FILE_STEMS.contains(&stem_prefix)
            && stem_lower
                .trim_start_matches(stem_prefix)
                .trim_matches(|c: char| matches!(c, '-' | '_' | '.' | ' '))
                .chars()
                .all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some((basename, stem))
}

fn extract_comparison_tokens(text: &str) -> HashSet<String> {
    let normalized: String = text.nfkc().collect::<String>().to_lowercase();
    let mut tokens = HashSet::new();

    // ASCII 单词（长度 >= 2）
    let mut ascii_buf = String::new();
    let mut cjk_buf: Vec<char> = Vec::new();

    let flush_ascii = |buf: &mut String, out: &mut HashSet<String>| {
        if buf.len() >= 2 {
            out.insert(buf.clone());
        }
        buf.clear();
    };
    let flush_cjk = |buf: &mut Vec<char>, out: &mut HashSet<String>| {
        for &c in buf.iter() {
            let _ = c;
        }
        if buf.len() >= 2 {
            for win in buf.windows(2) {
                let mut s = String::with_capacity(8);
                s.push(win[0]);
                s.push(win[1]);
                out.insert(s);
            }
        }
        buf.clear();
    };

    for c in normalized.chars() {
        if c.is_ascii_alphanumeric() {
            flush_cjk(&mut cjk_buf, &mut tokens);
            ascii_buf.push(c);
        } else if is_cjk_han(c) {
            flush_ascii(&mut ascii_buf, &mut tokens);
            cjk_buf.push(c);
        } else {
            flush_ascii(&mut ascii_buf, &mut tokens);
            flush_cjk(&mut cjk_buf, &mut tokens);
        }
    }
    flush_ascii(&mut ascii_buf, &mut tokens);
    flush_cjk(&mut cjk_buf, &mut tokens);
    tokens
}

fn has_token_overlap(span_inner: &str, anchor_tokens: &HashSet<String>) -> bool {
    let span_tokens = extract_comparison_tokens(span_inner);
    span_tokens.iter().any(|t| anchor_tokens.contains(t))
}

/// L2b 阶段：结合种子内最大实体主文件名交叉校验，剥离外侧无重合广告包装块。
///
/// 安全约束：
/// - 当当前清洗后文本已为空时直接返回，绝不将“纯广告标题”借主文件名复活；
/// - 占位文件名（如 `sample/video.mkv`、`file-1.bin`）自动忽略；
/// - 仅当标题内部至少有一个语块与主文件名存在词元锚定重合时，才剥离外侧无重合的非保护包装块。
pub fn cross_clean_with_files(text: &str, files: &[FileInfo]) -> (String, Option<String>) {
    let trimmed = text.trim();
    if trimmed.is_empty() || files.is_empty() {
        return (text.to_string(), None);
    }

    let Some((anchor_basename, anchor_stem)) = extract_anchor_basename_and_stem(files) else {
        return (text.to_string(), None);
    };

    let anchor_tokens = extract_comparison_tokens(anchor_stem);
    if anchor_tokens.is_empty() {
        return (text.to_string(), None);
    }

    let (body, trailing_ext) = split_trailing_known_ext(trimmed);
    let spans = segment_spans(body);
    if spans.len() < 2 {
        return (text.to_string(), None);
    }

    let mut overlaps = vec![false; spans.len()];
    let mut first_overlap = None;
    let mut last_overlap = None;
    for (idx, sp) in spans.iter().enumerate() {
        let ov = has_token_overlap(sp.inner, &anchor_tokens);
        overlaps[idx] = ov;
        if ov {
            if first_overlap.is_none() {
                first_overlap = Some(idx);
            }
            last_overlap = Some(idx);
        }
    }

    let (Some(first_ov), Some(last_ov)) = (first_overlap, last_overlap) else {
        return (text.to_string(), None);
    };

    let mut stripped = vec![false; spans.len()];
    let mut peeled_parts: Vec<String> = Vec::new();

    // 从左侧向内剥离外侧无重合包装块（最多剥离到首个锚定重合块之前）
    for idx in 0..first_ov {
        let sp = &spans[idx];
        if overlaps[idx] || is_protected_media_span(sp.inner) {
            break;
        }
        let (sc, _) = score_span(sp.inner, sp.is_bracketed, idx == 0, sp.has_clause_sep_before);
        if sp.is_bracketed || sc >= 25 {
            stripped[idx] = true;
            peeled_parts.push(sp.raw.to_string());
        } else {
            break;
        }
    }

    // 从右侧向内剥离外侧无重合包装块（最多剥离到末个锚定重合块之后）
    for idx in ((last_ov + 1)..spans.len()).rev() {
        let sp = &spans[idx];
        if overlaps[idx] || is_protected_media_span(sp.inner) {
            break;
        }
        let (sc, _) = score_span(
            sp.inner,
            sp.is_bracketed,
            idx + 1 == spans.len(),
            sp.has_clause_sep_before,
        );
        if sp.is_bracketed || sc >= 25 {
            stripped[idx] = true;
            peeled_parts.push(sp.raw.to_string());
        } else {
            break;
        }
    }

    if peeled_parts.is_empty() {
        return (text.to_string(), None);
    }

    let mut cleaned = reconstruct_body(&spans, &stripped);
    if cleaned.trim().is_empty() {
        return (text.to_string(), None);
    }
    if let Some(ext) = trailing_ext {
        cleaned.push_str(ext);
    }
    let detail = format!(
        "主文件对齐({anchor_basename})，剥离首尾无重合水印: {}",
        peeled_parts.join(" ")
    );
    (cleaned, Some(detail))
}

/// 供旁路高频广告模板自发现提取首/尾候选语块及主体签名文本。
pub(crate) fn extract_edge_discovery_candidates<'a>(raw_name: &'a str) -> Vec<(String, String, u8)> {
    let trimmed = raw_name.trim();
    if trimmed.chars().count() < 6 {
        return Vec::new();
    }
    let (body, _) = split_trailing_known_ext(trimmed);
    let spans = segment_spans(body);
    if spans.len() < 2 {
        return Vec::new();
    }

    let mut candidate_indices = Vec::new();
    candidate_indices.push(0);
    if spans.len() > 2 && spans[0].is_bracketed {
        candidate_indices.push(1);
    }
    let last_idx = spans.len() - 1;
    if last_idx > 0 {
        candidate_indices.push(last_idx);
        if spans.len() > 2 && spans[last_idx].is_bracketed {
            candidate_indices.push(last_idx - 1);
        }
    }
    candidate_indices.sort_unstable();
    candidate_indices.dedup();

    let mut results = Vec::new();
    for idx in candidate_indices {
        let sp = &spans[idx];
        let cand_text = sp.inner.trim();
        let char_len = cand_text.chars().count();
        if !(3..=36).contains(&char_len) {
            continue;
        }
        // 只保留有足够特征边界的候选：括号块，或具备域名/风险信号的高辨识度词块
        let (sc, _) = score_span(cand_text, sp.is_bracketed, true, sp.has_clause_sep_before);
        let looks_like_domain = contains_high_risk_domain(cand_text, true);
        if !sp.is_bracketed && !looks_like_domain && sc < 25 {
            continue;
        }
        if is_protected_media_span(cand_text) && sc < 45 && !looks_like_domain {
            continue;
        }
        if cand_text.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '_' | '.')) {
            continue;
        }

        // 构造去掉该语块后的剩余主体，供计算不同电影/种子的文档频次
        let mut stripped = vec![false; spans.len()];
        stripped[idx] = true;
        let remainder = reconstruct_body(&spans, &stripped);
        if remainder.trim().chars().count() < 3 {
            continue;
        }

        results.push((
            sp.raw.trim().to_string(),
            remainder,
            u8::try_from(sc.clamp(0, 100)).unwrap_or(100),
        ));
    }

    results
}
