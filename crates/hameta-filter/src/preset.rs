use crate::schema::{
    AdPhraseRule, AdRegexRule, CleanerOptions, CompositeLogic, DensityGate, ExtensionScope,
    FILTER_POLICY_SCHEMA_VERSION, FileConstraintMode, FilterBundle, FilterGates, FilterPolicy,
    KeywordMatchMode, PolicyAction, PolicyRule, RuleMatcher, TextTarget, TitlePatternKind,
    ToggleU64, ToggleUsize,
};
use serde::{Deserialize, Serialize};

const AD_PHRASES_RAW: &str = include_str!("../data/ad-phrases.txt");
const AD_REGEXES_RAW: &str = include_str!("../data/ad-regexes.txt");

pub(crate) const MALWARE_KEYWORDS: &[&str] = &[
    "hacker",
    "hack",
    "hacktool",
    "trojan",
    "virus",
    "malware",
    "worm",
    "ransom",
    "ransomware",
    "backdoor",
    "spyware",
    "adware",
    "rootkit",
    "botnet",
    "miner",
    "mining",
    "payload",
    "exploit",
    "dropper",
    "勒索",
    "后门",
    "挖矿",
    "攻击",
];

pub(crate) const CRACK_KEYWORDS: &[&str] = &[
    "crack",
    "cracked",
    "patch",
    "keygen",
    "license",
    "licence",
    "activator",
    "activate",
    "activation",
    "loader",
    "kms",
    "破解版",
    "破解",
    "补丁",
    "注册机",
    "注册码",
    "序列号",
    "激活码",
    "激活工具",
    "激活器",
    "免激活",
    "免注册",
    "绿色版",
    "精简版",
    "汉化补丁",
    "外挂",
    "辅助",
];

pub(crate) const BAIT_KEYWORDS: &[&str] = &[
    "readme",
    "read me",
    "read_me",
    "readme_before",
    "password",
    "pass_list",
    "pass list",
    "install_instructions",
    "install instructions",
    "how to install",
    "how_to_install",
    "license_key",
    "license key",
    "网址发布器",
    "解压密码",
];

pub(crate) const PLACEHOLDER_KEYWORDS: &[&str] = &["unknown", "undefined"];

pub(crate) const DANGEROUS_EXEC_FILE_EXTS: &[&str] = &[
    "exe", "msi", "msp", "bat", "cmd", "com", "scr", "pif", "vbs", "vbe", "js", "jse", "wsf",
    "wsh", "hta", "ps1", "psm1", "reg", "sys", "drv", "cpl", "msc", "apk", "xapk", "apks", "ipa",
];

pub(crate) const EXECUTABLE_TITLE_EXTS: &[&str] = &[
    "exe", "dll", "scr", "com", "pif", "msi", "msp", "bat", "bin", "cmd", "vbs", "vbe", "vb", "js",
    "jse", "wsf", "wsh", "hta", "ps1", "psm1", "ps1xml", "ps2", "psc1", "psc2", "sct", "msc",
    "cpl", "reg", "sys", "drv", "ocx", "jar", "class", "sh", "run", "app", "ipa", "apk", "xapk",
    "apks", "deb", "rpm", "pkg", "dmg",
];

pub(crate) const ARCHIVE_EXTS: &[&str] = &[
    "zip", "rar", "7z", "iso", "img", "vhd", "vhdx", "tar", "gz", "tgz", "bz2", "tbz2", "xz",
    "txz", "cab", "ace", "lzh", "lha", "sit", "sitx", "z", "lzma", "bz",
];

pub(crate) const DOC_EXTS: &[&str] = &[
    "doc", "docx", "docm", "dotm", "xls", "xlsx", "xlsm", "xlam", "xll", "ppt", "pptx", "pptm",
    "chm", "one", "slk", "iqy", "mobi",
];

pub(crate) const SHORTCUT_JUNK_EXTS: &[&str] = &["ink", "url", "lnk", "scf", "html", "txt"];

pub(crate) const VIDEO_EXTS: &[&str] = &[
    "mp4", "avi", "mkv", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp", "rm", "rmvb",
    "vob", "asf", "divx", "xvid", "f4v", "m2ts", "ts",
];

pub(crate) const AUDIO_EXTS: &[&str] = &[
    "flac", "ape", "wav", "alac", "dsf", "dff", "mp3", "aac", "m4a", "ogg", "opus",
];

fn to_vec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn parse_dict_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(String::from)
        .collect()
}

const LEGACY_BUILTIN_NOTE: &str = "内置基线";

/// 根据内置广告短语返回面向用户的友好备注说明。
pub fn builtin_ad_phrase_note(phrase: &str) -> Option<&'static str> {
    match phrase.trim() {
        "地址发布页" | "地址发布" => Some("导航站引流词"),
        "收藏不迷路" | "防走丢" => Some("站点收藏引导语"),
        "更多资源访问" | "更多合集访问" | "更多资源" | "更多合集" => Some("站点引流话术"),
        "关注公众号" => Some("公众号关注引流"),
        "扫码观看" => Some("二维码观看引流"),
        "薇信首发" => Some("微信谐音首发推广"),
        "微信首发" => Some("微信首发推广"),
        "免费观看" | "在线观看" | "免费在线观看" | "免费看片" => Some("视频站引流话术"),
        "大神BT最新域名" | "BT最新域名" => Some("BT 站点域名推广"),
        "大神BT最新地址" | "BT最新地址" => Some("BT 站点地址推广"),
        "大神BT最新网址" | "BT最新网址" => Some("BT 站点网址推广"),
        "最新地址发布页" | "永久地址发布页" | "最新域名发布页" | "最新网址发布页" => {
            Some("防屏蔽发布页引流")
        }
        "最新网址" => Some("站点最新入口推广"),
        "永久地址" | "永久域名" => Some("站点永久入口推广"),
        "备用地址" | "备用网址" => Some("备用入口后缀"),
        "回家地址" | "防迷路地址" => Some("防屏蔽入口推广"),
        "高清完整版无删减" => Some("视频站推广后缀"),
        "无水印完整版" => Some("二次封装水印词"),
        "扫码加群" | "扫码进群" => Some("私域加群引流"),
        "欢迎加入电报群" => Some("电报社群引流"),
        "更多精彩资源" | "更多福利资源" => Some("资源站引流话术"),
        _ => None,
    }
}

/// 根据内置广告正则返回面向用户的友好备注说明。
pub fn builtin_ad_regex_note(pattern: &str) -> Option<&'static str> {
    match pattern.trim() {
        r"(?i)(?:\d{4}年\d{1,2}月(?:\d{1,2}日)?\s*)?(?:大神)?BT最新(?:域名|地址|网址)" => {
            Some("日期前缀与 BT 最新域名/地址推广")
        }
        r"(?i)[\[\(【]\s*(?:(?:QQ|Q|微信|VX|V信|TG|Telegram|电报)(?:群|频道|号)\s*[:：]?|(?:QQ|微信|VX|V信|Telegram|电报)\s*[:：])\s*[A-Za-z0-9_@\-]{4,24}\s*[\]\)】]" => {
            Some("QQ / 微信 / 电报群号引流标签")
        }
        r"【[^【】]{0,24}(?:地址发布|最新地址|最新域名|独家首发|影视网|资源网|电影网|字幕网|论坛首发|福利群)】" => {
            Some("中文发布站方括号推广标签")
        }
        r"(?i)[\[\(【]\s*(?:关注)?(?:公众号|微信号|微信|VX|TG频道)\s*[:：]\s*[^\]\)】]{1,24}\s*[\]\)】]" => {
            Some("公众号与社交关注引导括号")
        }
        r"(?:第\d+备用|最新|永久|防屏蔽|防走丢)(?:地址|域名|网址)(?:发布页?)?(?:\s*[:：]\s*\S+)?" => {
            Some("备用 / 防屏蔽跳转地址前后缀")
        }
        r"(?i)[\[\(【][^\]\)】]{0,16}[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,12}[^\]\)】]{0,16}[\]\)】]" => {
            Some("括号内邮箱联系方式引流")
        }
        _ => None,
    }
}

/// 将旧版本持久化数据中仍为“内置基线”的备注自动升级为友好说明（不覆盖用户自定义备注）。
pub fn upgrade_legacy_builtin_notes(
    ad_phrases: &mut [AdPhraseRule],
    ad_regexes: &mut [AdRegexRule],
) {
    for item in ad_phrases {
        if item.note.trim() == LEGACY_BUILTIN_NOTE {
            item.note = builtin_ad_phrase_note(&item.phrase)
                .unwrap_or("默认推广短语")
                .to_string();
        }
    }
    for item in ad_regexes {
        if item.note.trim() == LEGACY_BUILTIN_NOTE {
            item.note = builtin_ad_regex_note(&item.pattern)
                .unwrap_or("默认推广正则")
                .to_string();
        }
    }
}

/// 返回内建默认 `ad-phrases`（中文推广短语词典）。
pub fn default_ad_phrases() -> Vec<AdPhraseRule> {
    parse_dict_lines(AD_PHRASES_RAW)
        .into_iter()
        .enumerate()
        .map(|(idx, phrase)| {
            let note = builtin_ad_phrase_note(&phrase)
                .unwrap_or("默认推广短语")
                .to_string();
            AdPhraseRule {
                id: format!("phrase-{}", idx + 1),
                phrase,
                enabled: true,
                note,
            }
        })
        .collect()
}

/// 返回内建默认 `ad-regexes`（常见推广广告正则规则）。
pub fn default_ad_regexes() -> Vec<AdRegexRule> {
    parse_dict_lines(AD_REGEXES_RAW)
        .into_iter()
        .enumerate()
        .map(|(idx, pattern)| {
            let note = builtin_ad_regex_note(&pattern)
                .unwrap_or("默认推广正则")
                .to_string();
            AdRegexRule {
                id: format!("ad-re-{}", idx + 1),
                pattern,
                replacement: " ".to_string(),
                enabled: true,
                note,
            }
        })
        .collect()
}

/// 生成与原版线上 `filter` JSON 完全语义等价的 v2 规则策略（`online_standard`）。
pub fn default_online_filter_policy() -> FilterPolicy {
    FilterPolicy {
        schema_version: FILTER_POLICY_SCHEMA_VERSION,
        preset_id: Some("online_standard".to_string()),
        gates: FilterGates {
            reject_empty_name: true,
            min_name_length: ToggleUsize::on(4),
            max_name_length: ToggleUsize::on(128),
            reject_empty_files: true,
            reject_all_empty_files: true,
            max_empty_files: ToggleUsize::off(0),
            max_file_count: ToggleUsize::on(2000),
            max_file_name_length: ToggleUsize::off(255),
            min_total_size: ToggleU64::on(1),
            max_total_size: ToggleU64::on(2_199_023_255_552),
            density_check: DensityGate::on(100, 102_400),
        },
        cleaner: CleanerOptions {
            min_refined_name_length: ToggleUsize::on(2),
            ..CleanerOptions::default()
        },
        rules: vec![
            PolicyRule {
                id: "kw-malware".to_string(),
                name: "恶意软件与木马关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全防护".to_string(), "关键词".to_string()],
                note: "拦截常见病毒/木马/挖矿/勒索词汇（原始标题包含匹配）".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: to_vec(MALWARE_KEYWORDS),
                },
            },
            PolicyRule {
                id: "kw-crack".to_string(),
                name: "破解与激活外挂关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["软件破解".to_string(), "关键词".to_string()],
                note: "拦截破解版/注册机/激活工具/外挂词汇".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: to_vec(CRACK_KEYWORDS),
                },
            },
            PolicyRule {
                id: "kw-bait".to_string(),
                name: "密码与安装说明诱饵关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["诱饵推广".to_string(), "关键词".to_string()],
                note: "拦截解压密码/网址发布器/安装指南诱饵".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: to_vec(BAIT_KEYWORDS),
                },
            },
            PolicyRule {
                id: "kw-placeholder".to_string(),
                name: "无效占位标题关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["垃圾占位".to_string(), "关键词".to_string()],
                note: "拦截 unknown/undefined 等无意义占位名称".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: to_vec(PLACEHOLDER_KEYWORDS),
                },
            },
            PolicyRule {
                id: "fc-video-quality".to_string(),
                name: "视频文件最小体积与文件名长度约束".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["视频质检".to_string(), "文件约束".to_string()],
                note: "要求所有视频文件体积 >= 10 MiB 且单文件名长度在 4~128 字符之间".to_string(),
                matcher: RuleMatcher::FileConstraint {
                    extensions: to_vec(VIDEO_EXTS),
                    extra_regexes: Vec::new(),
                    mode: FileConstraintMode::EveryMatched,
                    min_file_size: Some(10_485_760),
                    max_file_size: None,
                    min_file_name_length: Some(4),
                    max_file_name_length: Some(128),
                },
            },
            PolicyRule {
                id: "sub-non-junk".to_string(),
                name: "排除垃圾与快捷方式后的实质文件底线".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["整种质检".to_string()],
                note: "排除系统残留和说明/快捷方式后，至少含 1 个合计 >= 1 MiB 的实质文件"
                    .to_string(),
                matcher: RuleMatcher::SubstantiveContent {
                    junk_extensions: to_vec(SHORTCUT_JUNK_EXTS),
                    include_system_junk: true,
                    extra_junk_regexes: Vec::new(),
                    min_files: 1,
                    min_total_size: 1_048_576,
                },
            },
            PolicyRule {
                id: "ratio-archive".to_string(),
                name: "压缩包体积占比上限（70%）".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["压缩包治理".to_string()],
                note: "压缩包及分卷合计体积占总大小 >= 70% 时丢弃".to_string(),
                matcher: RuleMatcher::SizeRatio {
                    extensions: to_vec(ARCHIVE_EXTS),
                    include_split_archives: true,
                    extra_regexes: Vec::new(),
                    max_ratio: 0.7,
                },
            },
            PolicyRule {
                id: "title-spam-features".to_string(),
                name: "常见垃圾标题结构特征".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["标题规范".to_string()],
                note: "拦截西里尔文字、纯数字/哈希/UUID、磁力/HTTP 链接前缀及分卷标题".to_string(),
                matcher: RuleMatcher::TitlePatternPreset {
                    patterns: vec![
                        TitlePatternKind::Cyrillic,
                        TitlePatternKind::DigitsOnly,
                        TitlePatternKind::Sha1Hex,
                        TitlePatternKind::Md5Hex,
                        TitlePatternKind::Uuid,
                        TitlePatternKind::DecimalCode,
                        TitlePatternKind::MagnetPrefix,
                        TitlePatternKind::HttpUrlPrefix,
                        TitlePatternKind::SplitArchiveSuffix,
                    ],
                    min_cyrillic_chars: 2,
                },
            },
            PolicyRule {
                id: "ext-title-archives".to_string(),
                name: "标题结尾为压缩包/镜像后缀".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["扩展名".to_string(), "标题".to_string()],
                note: "拦截单文件压缩包/镜像标题（如 .zip/.rar/.7z/.iso）".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::TorrentName,
                    extensions: to_vec(ARCHIVE_EXTS),
                },
            },
            PolicyRule {
                id: "ext-title-executables".to_string(),
                name: "标题结尾为可执行/脚本/安装包后缀".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全防护".to_string(), "扩展名".to_string()],
                note: "拦截标题直接以 .exe/.msi/.apk/.dmg/.sh 等结尾的资源".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::TorrentName,
                    extensions: to_vec(EXECUTABLE_TITLE_EXTS),
                },
            },
            PolicyRule {
                id: "ext-title-docs".to_string(),
                name: "标题结尾为办公/宏文档/电子书诱饵后缀".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["扩展名".to_string(), "标题".to_string()],
                note: "拦截以 .docm/.xlsm/.chm/.mobi 等结尾的孤立文档标题".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::TorrentName,
                    extensions: to_vec(DOC_EXTS),
                },
            },
            PolicyRule {
                id: "ext-title-shortcuts".to_string(),
                name: "标题结尾为快捷方式/网页/文本后缀".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["扩展名".to_string(), "标题".to_string()],
                note: "拦截 .lnk/.url/.html/.txt 等诱饵标题".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::TorrentName,
                    extensions: to_vec(SHORTCUT_JUNK_EXTS),
                },
            },
            PolicyRule {
                id: "ext-file-dangerous".to_string(),
                name: "种子内含高危可执行/移动端安装包".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全防护".to_string(), "扩展名".to_string()],
                note: "种子内任意文件命中高危可执行/脚本/APK/IPA 即拦截整种".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::AnyFile,
                    extensions: to_vec(DANGEROUS_EXEC_FILE_EXTS),
                },
            },
            PolicyRule {
                id: "ext-file-nested-torrent".to_string(),
                name: "种子内含套娃 .torrent 文件".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["扩展名".to_string(), "套娃".to_string()],
                note: "禁止套娃分发 .torrent 种子文件".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::AnyFile,
                    extensions: vec!["torrent".to_string()],
                },
            },
        ],
    }
}

/// 返回初始化向导内置的完整默认过滤与清洗数据包（版本号 `1`）。
pub fn default_online_filter_bundle() -> FilterBundle {
    FilterBundle {
        version: 1,
        policy: default_online_filter_policy(),
        ad_phrases: default_ad_phrases(),
        ad_regexes: default_ad_regexes(),
    }
}

fn media_focused_policy() -> FilterPolicy {
    FilterPolicy {
        schema_version: FILTER_POLICY_SCHEMA_VERSION,
        preset_id: Some("media_focused".to_string()),
        gates: FilterGates {
            reject_empty_name: true,
            min_name_length: ToggleUsize::on(4),
            max_name_length: ToggleUsize::on(160),
            reject_empty_files: true,
            reject_all_empty_files: true,
            max_empty_files: ToggleUsize::off(0),
            max_file_count: ToggleUsize::on(1500),
            max_file_name_length: ToggleUsize::off(255),
            min_total_size: ToggleU64::on(10 * 1024 * 1024),
            max_total_size: ToggleU64::on(2_199_023_255_552),
            density_check: DensityGate::on(100, 204_800),
        },
        cleaner: CleanerOptions {
            min_refined_name_length: ToggleUsize::on(2),
            ..CleanerOptions::default()
        },
        rules: vec![
            PolicyRule {
                id: "ext-file-dangerous".to_string(),
                name: "高危可执行/移动端安装包硬拦截".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全底线".to_string()],
                note: "在白名单之前优先执行，绝不放行夹带木马可执行程序的影视包".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::AnyFile,
                    extensions: to_vec(DANGEROUS_EXEC_FILE_EXTS),
                },
            },
            PolicyRule {
                id: "ext-file-nested-torrent".to_string(),
                name: "禁止套娃 .torrent 文件".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["套娃".to_string()],
                note: "拦截种子内嵌套的 .torrent 文件".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::AnyFile,
                    extensions: vec!["torrent".to_string()],
                },
            },
            PolicyRule {
                id: "allow-hd-main-video".to_string(),
                name: "高清主视频文件白名单豁免（>= 300 MB）".to_string(),
                enabled: true,
                action: PolicyAction::Allow,
                tags: vec!["白名单".to_string(), "影视精选".to_string()],
                note: "只要包含至少 1 个 >= 300 MB 的视频主文件（且无木马可执行），直接放行并豁免后续整词/花絮校验".to_string(),
                matcher: RuleMatcher::FileConstraint {
                    extensions: to_vec(VIDEO_EXTS),
                    extra_regexes: Vec::new(),
                    mode: FileConstraintMode::AtLeastOneMatched,
                    min_file_size: Some(300 * 1024 * 1024),
                    max_file_size: None,
                    min_file_name_length: Some(4),
                    max_file_name_length: None,
                },
            },
            PolicyRule {
                id: "kw-malware-boundary".to_string(),
                name: "恶意软件关键词（整词边界保护）".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全防护".to_string(), "整词匹配".to_string()],
                note: "采用整词边界匹配，避免误伤 Shack / LifeHack 等正常片名".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::WordBoundary,
                    case_sensitive: false,
                    words: to_vec(MALWARE_KEYWORDS),
                },
            },
            PolicyRule {
                id: "kw-bait-boundary".to_string(),
                name: "密码与广告诱饵关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["诱饵推广".to_string()],
                note: "拦截解压密码/网址发布器等低质广告种子".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::WordBoundary,
                    case_sensitive: false,
                    words: to_vec(BAIT_KEYWORDS),
                },
            },
            PolicyRule {
                id: "title-spam-features".to_string(),
                name: "常见垃圾标题结构特征".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["标题规范".to_string()],
                note: "过滤纯数字、哈希、UUID、磁力/URL 前缀及西里尔垃圾标题".to_string(),
                matcher: RuleMatcher::TitlePatternPreset {
                    patterns: vec![
                        TitlePatternKind::Cyrillic,
                        TitlePatternKind::DigitsOnly,
                        TitlePatternKind::Sha1Hex,
                        TitlePatternKind::Md5Hex,
                        TitlePatternKind::Uuid,
                        TitlePatternKind::DecimalCode,
                        TitlePatternKind::MagnetPrefix,
                        TitlePatternKind::HttpUrlPrefix,
                        TitlePatternKind::SplitArchiveSuffix,
                    ],
                    min_cyrillic_chars: 2,
                },
            },
            PolicyRule {
                id: "fc-video-main-or-sample".to_string(),
                name: "视频主文件达标校验（容忍小花絮）".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["影视质检".to_string()],
                note: "采用 AtLeastOneMatched 模式，防止蓝光原盘里自带的小体积 Sample/Trailer 预告片误杀整部电影".to_string(),
                matcher: RuleMatcher::FileConstraint {
                    extensions: to_vec(VIDEO_EXTS),
                    extra_regexes: Vec::new(),
                    mode: FileConstraintMode::AtLeastOneMatched,
                    min_file_size: Some(20 * 1024 * 1024),
                    max_file_size: None,
                    min_file_name_length: Some(4),
                    max_file_name_length: None,
                },
            },
            PolicyRule {
                id: "sub-non-junk".to_string(),
                name: "有效实质文件底线（>= 10 MB）".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["整种质检".to_string()],
                note: "排除系统残留及 txt/url 后至少保留 10 MB 有效文件内容".to_string(),
                matcher: RuleMatcher::SubstantiveContent {
                    junk_extensions: to_vec(SHORTCUT_JUNK_EXTS),
                    include_system_junk: true,
                    extra_junk_regexes: Vec::new(),
                    min_files: 1,
                    min_total_size: 10 * 1024 * 1024,
                },
            },
            PolicyRule {
                id: "ratio-archive".to_string(),
                name: "压缩包占比上限（70%）".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["压缩包治理".to_string()],
                note: "丢弃以分卷压缩为主的打包资源".to_string(),
                matcher: RuleMatcher::SizeRatio {
                    extensions: to_vec(ARCHIVE_EXTS),
                    include_split_archives: true,
                    extra_regexes: Vec::new(),
                    max_ratio: 0.7,
                },
            },
        ],
    }
}

fn relaxed_archive_policy() -> FilterPolicy {
    FilterPolicy {
        schema_version: FILTER_POLICY_SCHEMA_VERSION,
        preset_id: Some("relaxed_archive".to_string()),
        gates: FilterGates {
            reject_empty_name: true,
            min_name_length: ToggleUsize::on(2),
            max_name_length: ToggleUsize::on(256),
            reject_empty_files: true,
            reject_all_empty_files: true,
            max_empty_files: ToggleUsize::off(0),
            max_file_count: ToggleUsize::on(5000),
            max_file_name_length: ToggleUsize::off(255),
            min_total_size: ToggleU64::on(1),
            max_total_size: ToggleU64::off(2_199_023_255_552),
            density_check: DensityGate::off(100, 102_400),
        },
        cleaner: CleanerOptions::default(),
        rules: vec![
            PolicyRule {
                id: "ext-file-nested-torrent".to_string(),
                name: "禁止套娃 .torrent 文件".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["套娃".to_string()],
                note: "拦截套娃种子文件".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::AnyFile,
                    extensions: vec!["torrent".to_string()],
                },
            },
            PolicyRule {
                id: "ext-file-dangerous".to_string(),
                name: "拦截高危脚本与勒索可执行程序".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全防护".to_string()],
                note: "允许镜像与文档归档，仅拦截极高危直接可执行与批处理脚本".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::AnyFile,
                    extensions: vec![
                        "scr", "pif", "vbs", "vbe", "jse", "wsf", "wsh", "hta", "ps1", "psm1",
                    ]
                    .into_iter()
                    .map(String::from)
                    .collect(),
                },
            },
            PolicyRule {
                id: "kw-malware".to_string(),
                name: "恶意软件与勒索词库（整词）".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全防护".to_string()],
                note: "全量归档模式下仅剔除明显木马病毒词汇".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::WordBoundary,
                    case_sensitive: false,
                    words: to_vec(MALWARE_KEYWORDS),
                },
            },
            PolicyRule {
                id: "title-spam-features".to_string(),
                name: "哈希/磁力/数字乱码标题拦截".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["标题规范".to_string()],
                note: "清理无可读标题的纯哈希与 URL 垃圾条目".to_string(),
                matcher: RuleMatcher::TitlePatternPreset {
                    patterns: vec![
                        TitlePatternKind::DigitsOnly,
                        TitlePatternKind::Sha1Hex,
                        TitlePatternKind::Md5Hex,
                        TitlePatternKind::Uuid,
                        TitlePatternKind::MagnetPrefix,
                        TitlePatternKind::HttpUrlPrefix,
                    ],
                    min_cyrillic_chars: 2,
                },
            },
            PolicyRule {
                id: "sub-non-junk".to_string(),
                name: "排除纯快捷方式与空文本包".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["整种质检".to_string()],
                note: "保留 .iso/.zip/.pdf 等正常内容，只拦截全部为 .url/.lnk/.txt 的垃圾诱饵包"
                    .to_string(),
                matcher: RuleMatcher::SubstantiveContent {
                    junk_extensions: to_vec(SHORTCUT_JUNK_EXTS),
                    include_system_junk: true,
                    extra_junk_regexes: Vec::new(),
                    min_files: 1,
                    min_total_size: 1024,
                },
            },
        ],
    }
}

fn minimal_clean_policy() -> FilterPolicy {
    FilterPolicy {
        schema_version: FILTER_POLICY_SCHEMA_VERSION,
        preset_id: Some("minimal_clean".to_string()),
        gates: FilterGates {
            reject_empty_name: true,
            min_name_length: ToggleUsize::on(2),
            max_name_length: ToggleUsize::off(256),
            reject_empty_files: true,
            reject_all_empty_files: true,
            max_empty_files: ToggleUsize::off(0),
            max_file_count: ToggleUsize::on(5000),
            max_file_name_length: ToggleUsize::off(255),
            min_total_size: ToggleU64::on(1),
            max_total_size: ToggleU64::off(2_199_023_255_552),
            density_check: DensityGate::off(100, 102_400),
        },
        cleaner: CleanerOptions::default(),
        rules: vec![PolicyRule {
            id: "ext-file-nested-torrent".to_string(),
            name: "禁止套娃 .torrent 文件".to_string(),
            enabled: true,
            action: PolicyAction::Reject,
            tags: vec!["套娃".to_string()],
            note: "最小化规则集：仅保留基础门禁、标题清洗及禁止套娃 .torrent".to_string(),
            matcher: RuleMatcher::ExtensionRule {
                scope: ExtensionScope::AnyFile,
                extensions: vec!["torrent".to_string()],
            },
        }],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyPresetInfo {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub badge: String,
    pub rule_count: usize,
    pub allow_count: usize,
    pub reject_count: usize,
    pub bundle: FilterBundle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleTemplateInfo {
    pub id: String,
    pub name: String,
    pub group: String,
    pub description: String,
    pub rule: PolicyRule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionPackInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordPackInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub words: Vec<String>,
}

/// 面板内置场景预设、单条规则模板以及快捷扩展名包 / 词库目录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterPresetCatalog {
    pub policy_presets: Vec<PolicyPresetInfo>,
    pub rule_templates: Vec<RuleTemplateInfo>,
    pub extension_packs: Vec<ExtensionPackInfo>,
    pub keyword_packs: Vec<KeywordPackInfo>,
}

fn preset_info(
    id: &str,
    name: &str,
    summary: &str,
    badge: &str,
    policy: FilterPolicy,
) -> PolicyPresetInfo {
    let allow_count = policy
        .rules
        .iter()
        .filter(|r| r.action == PolicyAction::Allow)
        .count();
    let reject_count = policy
        .rules
        .iter()
        .filter(|r| r.action == PolicyAction::Reject)
        .count();
    let rule_count = policy.rules.len();
    PolicyPresetInfo {
        id: id.to_string(),
        name: name.to_string(),
        summary: summary.to_string(),
        badge: badge.to_string(),
        rule_count,
        allow_count,
        reject_count,
        bundle: FilterBundle {
            version: 1,
            policy,
            ad_phrases: default_ad_phrases(),
            ad_regexes: default_ad_regexes(),
        },
    }
}

/// 根据预设 ID 获取完整可发布数据包（用于切换场景预设或恢复默认）。
pub fn policy_preset_bundle(preset_id: &str) -> Option<FilterBundle> {
    let policy = match preset_id {
        "online_standard" => default_online_filter_policy(),
        "media_focused" => media_focused_policy(),
        "relaxed_archive" => relaxed_archive_policy(),
        "minimal_clean" => minimal_clean_policy(),
        _ => return None,
    };
    Some(FilterBundle {
        version: 1,
        policy,
        ad_phrases: default_ad_phrases(),
        ad_regexes: default_ad_regexes(),
    })
}

/// 返回前端工作台使用的全部内置场景预设、单条规则预设模板、扩展名包与关键词包。
pub fn builtin_preset_catalog() -> FilterPresetCatalog {
    let policy_presets = vec![
        preset_info(
            "online_standard",
            "线上标准预设（原配置等价版）",
            "由原 torrent-indexer 线上配置智能转换而来：67 个安全/破解/诱饵关键词、扩展名多级拦阻、10MB 视频校验与 70% 压缩包占比拦截。",
            "推荐基线",
            default_online_filter_policy(),
        ),
        preset_info(
            "media_focused",
            "高清影视精选预设",
            "安全底座 + >=300MB 主视频白名单豁免 + 整词匹配防误杀 + 容忍原盘内含预告花絮小视频，最适合影视索引站点。",
            "影视推荐",
            media_focused_policy(),
        ),
        preset_info(
            "relaxed_archive",
            "宽松全量归档预设",
            "保留压缩包、ISO 镜像、文档与大型数据集，仅拦截高危木马脚本、纯乱码哈希、套娃种子及纯快捷方式诱饵。",
            "高召回",
            relaxed_archive_policy(),
        ),
        preset_info(
            "minimal_clean",
            "极简清洗与基础门禁",
            "仅保留标题长度、非空校验、套娃种子拦截以及完整 8 阶段标题去广告清洗，适合上游已有过滤或最大化探索 DHT 网络。",
            "最宽松",
            minimal_clean_policy(),
        ),
    ];

    let rule_templates = vec![
        RuleTemplateInfo {
            id: "tpl-ext-file-dangerous".to_string(),
            name: "高危可执行/安装包后缀拦截".to_string(),
            group: "后缀拦截".to_string(),
            description: "种子内任意文件含 .exe/.msi/.bat/.apk/.ipa 等即整种拦截".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "高危可执行与安装包后缀拦截".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["安全防护".to_string(), "后缀拦截".to_string()],
                note: "任意文件命中高危扩展名即拦截".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::AnyFile,
                    extensions: to_vec(DANGEROUS_EXEC_FILE_EXTS),
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-ext-file-torrent".to_string(),
            name: "套娃 .torrent 后缀拦截".to_string(),
            group: "后缀拦截".to_string(),
            description: "禁止种子文件内部再包含 .torrent 二级种子".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "禁止套娃 .torrent 文件".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["后缀拦截".to_string(), "套娃".to_string()],
                note: "拦截套娃种子".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::AnyFile,
                    extensions: vec!["torrent".to_string()],
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-ext-title-archive".to_string(),
            name: "标题压缩包/镜像后缀拦截".to_string(),
            group: "后缀拦截".to_string(),
            description: "拦截标题直接以 .zip/.rar/.7z/.iso 等结尾的打包种子".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "标题压缩包与镜像后缀拦截".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["后缀拦截".to_string(), "标题".to_string()],
                note: "拦截单文件压缩包/镜像标题（如 .zip/.rar/.7z/.iso）".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::TorrentName,
                    extensions: to_vec(ARCHIVE_EXTS),
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-ext-title-doc".to_string(),
            name: "标题办公与宏文档后缀拦截".to_string(),
            group: "后缀拦截".to_string(),
            description: "拦截标题以 .docm/.xlsm/.chm/.mobi 等结尾的孤立文档".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "标题办公文档后缀拦截".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["后缀拦截".to_string(), "标题".to_string()],
                note: "拦截以办公文档/宏文档/电子书后缀结尾的孤立诱饵标题".to_string(),
                matcher: RuleMatcher::ExtensionRule {
                    scope: ExtensionScope::TorrentName,
                    extensions: to_vec(DOC_EXTS),
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-kw-malware".to_string(),
            name: "恶意软件/木马/挖矿关键词组".to_string(),
            group: "关键词拦截".to_string(),
            description: "含 hacker/trojan/ransomware/挖矿/后门等 23 个高危关键词（推荐整词模式）".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "恶意软件与木马关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["关键词拦截".to_string(), "安全防护".to_string()],
                note: "拦截病毒、木马、勒索与挖矿词汇（整词边界避免误杀正常片名）".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::WordBoundary,
                    case_sensitive: false,
                    words: to_vec(MALWARE_KEYWORDS),
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-kw-crack".to_string(),
            name: "软件破解/激活器/外挂关键词组".to_string(),
            group: "关键词拦截".to_string(),
            description: "含 crack/keygen/kms/破解版/注册机等 27 个破解外挂词汇".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "破解与激活外挂关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["关键词拦截".to_string()],
                note: "拦截破解版、注册机、激活器与外挂类词汇".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::WordBoundary,
                    case_sensitive: false,
                    words: to_vec(CRACK_KEYWORDS),
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-kw-bait".to_string(),
            name: "解压密码/网址发布器诱饵词组".to_string(),
            group: "关键词拦截".to_string(),
            description: "含 password/readme/解压密码/网址发布器等 15 个引流诱饵词".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "密码与说明诱饵关键词".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["关键词拦截".to_string()],
                note: "拦截解压密码、网址发布器与安装说明类引流诱饵".to_string(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::Contains,
                    case_sensitive: false,
                    words: to_vec(BAIT_KEYWORDS),
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-title-spam".to_string(),
            name: "垃圾标题特征开关集".to_string(),
            group: "标题结构特征".to_string(),
            description: "一键开启纯数字、MD5/SHA1 哈希、UUID、URL 前缀、西里尔与分卷标题检查".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "常见垃圾标题特征".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["标题特征".to_string()],
                note: "拦截纯数字、哈希、UUID、URL/磁力前缀、西里尔与分卷垃圾标题".to_string(),
                matcher: RuleMatcher::TitlePatternPreset {
                    patterns: vec![
                        TitlePatternKind::Cyrillic,
                        TitlePatternKind::DigitsOnly,
                        TitlePatternKind::Sha1Hex,
                        TitlePatternKind::Md5Hex,
                        TitlePatternKind::Uuid,
                        TitlePatternKind::DecimalCode,
                        TitlePatternKind::MagnetPrefix,
                        TitlePatternKind::HttpUrlPrefix,
                        TitlePatternKind::SplitArchiveSuffix,
                    ],
                    min_cyrillic_chars: 2,
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-fc-video-main".to_string(),
            name: "影视正片体积下限（容忍花絮）".to_string(),
            group: "文件体积与长度约束".to_string(),
            description: "包含视频文件时至少有 1 个主文件 >= 50 MB，避免 trailer/sample 误杀".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "影视正片最小体积校验（>= 50 MB）".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["文件约束".to_string(), "影视".to_string()],
                note: "至少有 1 个视频主文件 >= 50 MB，兼容原盘预告片/花絮".to_string(),
                matcher: RuleMatcher::FileConstraint {
                    extensions: to_vec(VIDEO_EXTS),
                    extra_regexes: Vec::new(),
                    mode: FileConstraintMode::AtLeastOneMatched,
                    min_file_size: Some(50 * 1024 * 1024),
                    max_file_size: None,
                    min_file_name_length: Some(4),
                    max_file_name_length: None,
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-substantive".to_string(),
            name: "去噪后实质内容底线检查".to_string(),
            group: "整种结构检查".to_string(),
            description: "剔除 .DS_Store/__MACOSX/txt/url 等垃圾后至少含 1 个 >= 1 MB 有效文件".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "实质有效文件底线".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["整种质检".to_string()],
                note: "剔除系统残留与说明/快捷方式后，至少保留 1 个 >= 1 MiB 的实质文件".to_string(),
                matcher: RuleMatcher::SubstantiveContent {
                    junk_extensions: to_vec(SHORTCUT_JUNK_EXTS),
                    include_system_junk: true,
                    extra_junk_regexes: Vec::new(),
                    min_files: 1,
                    min_total_size: 1_048_576,
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-size-ratio-archive".to_string(),
            name: "压缩包体积占比上限（70%）".to_string(),
            group: "整种结构检查".to_string(),
            description: "压缩包及分卷累计占比 >= 70% 时拦截".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "压缩包体积占比上限 70%".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["整种质检".to_string()],
                note: "压缩包及分卷体积累计占总大小 >= 70% 时拦截".to_string(),
                matcher: RuleMatcher::SizeRatio {
                    extensions: to_vec(ARCHIVE_EXTS),
                    include_split_archives: true,
                    extra_regexes: Vec::new(),
                    max_ratio: 0.7,
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-allow-hd-video".to_string(),
            name: "白名单：高清大体积视频直接放行".to_string(),
            group: "白名单豁免".to_string(),
            description: "含任意 >= 500 MB 视频文件直接 Allow 豁免后续普通关键词/比例规则（建议置于安全后缀规则之后）".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "高清视频大文件白名单放行".to_string(),
                enabled: true,
                action: PolicyAction::Allow,
                tags: vec!["白名单".to_string()],
                note: "至少含 1 个 >= 500 MB 视频即豁免".to_string(),
                matcher: RuleMatcher::FileConstraint {
                    extensions: to_vec(VIDEO_EXTS),
                    extra_regexes: Vec::new(),
                    mode: FileConstraintMode::AtLeastOneMatched,
                    min_file_size: Some(500 * 1024 * 1024),
                    max_file_size: None,
                    min_file_name_length: None,
                    max_file_name_length: None,
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-allow-4k-remux".to_string(),
            name: "白名单：4K/REMUX/BluRay 组合放行".to_string(),
            group: "白名单豁免".to_string(),
            description: "多条件复合（All）：标题含高清压制标识 且 包含 >= 1 GB 视频文件".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "4K / REMUX 优质压制组白名单".to_string(),
                enabled: true,
                action: PolicyAction::Allow,
                tags: vec!["白名单".to_string(), "组合规则".to_string()],
                note: "示范 Composite ALL 组合放行规则".to_string(),
                matcher: RuleMatcher::Composite {
                    logic: CompositeLogic::All,
                    conditions: vec![
                        RuleMatcher::KeywordGroup {
                            target: TextTarget::RefinedName,
                            match_mode: KeywordMatchMode::WordBoundary,
                            case_sensitive: false,
                            words: vec![
                                "2160p".to_string(),
                                "4K".to_string(),
                                "REMUX".to_string(),
                                "BluRay".to_string(),
                            ],
                        },
                        RuleMatcher::FileConstraint {
                            extensions: vec!["mkv".to_string(), "mp4".to_string(), "ts".to_string(), "m2ts".to_string()],
                            extra_regexes: Vec::new(),
                            mode: FileConstraintMode::AtLeastOneMatched,
                            min_file_size: Some(1024 * 1024 * 1024),
                            max_file_size: None,
                            min_file_name_length: None,
                            max_file_name_length: None,
                        },
                    ],
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-allow-lossless-audio".to_string(),
            name: "白名单：整张无损音乐专辑放行".to_string(),
            group: "白名单豁免".to_string(),
            description: "全部文件为无损音频且 >= 100 MB 时放行".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "无损音乐专辑白名单".to_string(),
                enabled: true,
                action: PolicyAction::Allow,
                tags: vec!["白名单".to_string(), "音乐".to_string()],
                note: "每首无损音频文件均 >= 10 MB 时豁免后续普通拦截规则".to_string(),
                matcher: RuleMatcher::FileConstraint {
                    extensions: vec![
                        "flac".to_string(),
                        "ape".to_string(),
                        "wav".to_string(),
                        "alac".to_string(),
                        "dsf".to_string(),
                        "dff".to_string(),
                    ],
                    extra_regexes: Vec::new(),
                    mode: FileConstraintMode::EveryMatched,
                    min_file_size: Some(10 * 1024 * 1024),
                    max_file_size: None,
                    min_file_name_length: None,
                    max_file_name_length: None,
                },
            },
        },
        RuleTemplateInfo {
            id: "tpl-custom-regex".to_string(),
            name: "专家自定义正则规则".to_string(),
            group: "高级兜底".to_string(),
            description: "支持针对原始标题、清洗后标题或文件路径编写 Rust Regex".to_string(),
            rule: PolicyRule {
                id: String::new(),
                name: "自定义正则拦截".to_string(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec!["正则".to_string()],
                note: "按自定义 Rust 正则匹配清洗后标题并拦截 sample/trailer 合集".to_string(),
                matcher: RuleMatcher::CustomRegex {
                    target: TextTarget::RefinedName,
                    match_all: false,
                    patterns: vec![r"(?i)\b(sample|trailer)[._-]?pack\b".to_string()],
                },
            },
        },
    ];

    let extension_packs = vec![
        ExtensionPackInfo {
            id: "pack-dangerous-exec".to_string(),
            name: "高危可执行与手机安装包".to_string(),
            description: "exe, msi, bat, ps1, apk, ipa 等高风险包".to_string(),
            extensions: to_vec(DANGEROUS_EXEC_FILE_EXTS),
        },
        ExtensionPackInfo {
            id: "pack-archive".to_string(),
            name: "压缩包与磁盘镜像".to_string(),
            description: "zip, rar, 7z, iso, img, tar, gz 等".to_string(),
            extensions: to_vec(ARCHIVE_EXTS),
        },
        ExtensionPackInfo {
            id: "pack-docs".to_string(),
            name: "办公文档与电子书诱饵".to_string(),
            description: "doc, docm, xls, xlsm, chm, mobi 等".to_string(),
            extensions: to_vec(DOC_EXTS),
        },
        ExtensionPackInfo {
            id: "pack-shortcut-junk".to_string(),
            name: "快捷方式与网页文本残留".to_string(),
            description: "ink, url, lnk, scf, html, txt".to_string(),
            extensions: to_vec(SHORTCUT_JUNK_EXTS),
        },
        ExtensionPackInfo {
            id: "pack-video".to_string(),
            name: "常见高清视频格式".to_string(),
            description: "mp4, mkv, avi, mov, m2ts, ts, webm 等 20 种".to_string(),
            extensions: to_vec(VIDEO_EXTS),
        },
        ExtensionPackInfo {
            id: "pack-audio".to_string(),
            name: "音频与无损音乐格式".to_string(),
            description: "flac, ape, wav, dsf, mp3, aac 等".to_string(),
            extensions: to_vec(AUDIO_EXTS),
        },
        ExtensionPackInfo {
            id: "pack-torrent".to_string(),
            name: "套娃种子文件".to_string(),
            description: "torrent".to_string(),
            extensions: vec!["torrent".to_string()],
        },
    ];

    let keyword_packs = vec![
        KeywordPackInfo {
            id: "kwpack-malware".to_string(),
            name: "恶意软件与病毒黑名单".to_string(),
            description: "hacker, trojan, virus, ransomware, 挖矿, 后门 等 23 词".to_string(),
            words: to_vec(MALWARE_KEYWORDS),
        },
        KeywordPackInfo {
            id: "kwpack-crack".to_string(),
            name: "软件破解与激活外挂".to_string(),
            description: "crack, keygen, kms, 破解版, 注册机 等 27 词".to_string(),
            words: to_vec(CRACK_KEYWORDS),
        },
        KeywordPackInfo {
            id: "kwpack-bait".to_string(),
            name: "解压密码与安装诱饵".to_string(),
            description: "password, readme, 解压密码, 网址发布器 等 15 词".to_string(),
            words: to_vec(BAIT_KEYWORDS),
        },
        KeywordPackInfo {
            id: "kwpack-placeholder".to_string(),
            name: "无意义占位词".to_string(),
            description: "unknown, undefined".to_string(),
            words: to_vec(PLACEHOLDER_KEYWORDS),
        },
        KeywordPackInfo {
            id: "kwpack-hd-allow".to_string(),
            name: "常见高清影视标识（适合白名单）".to_string(),
            description: "2160p, 1080p, BluRay, REMUX, HEVC, x265, Atmos".to_string(),
            words: vec![
                "2160p".to_string(),
                "1080p".to_string(),
                "BluRay".to_string(),
                "REMUX".to_string(),
                "HEVC".to_string(),
                "x265".to_string(),
                "Atmos".to_string(),
            ],
        },
    ];

    FilterPresetCatalog {
        policy_presets,
        rule_templates,
        extension_packs,
        keyword_packs,
    }
}
