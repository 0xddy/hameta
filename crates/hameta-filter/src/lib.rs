pub mod cleaner;
pub mod discovery;
pub mod engine;
pub mod error;
pub mod legacy;
pub mod playground;
pub mod preset;
pub mod runtime;
pub mod schema;
pub mod smart;

/// 过滤引擎接口版本号（v2: Gates + Cleaner + Allow/Reject PolicyRule 链）。
pub const FILTER_ENGINE_API_VERSION: u32 = 2;

pub use cleaner::{CleaningStep, CleaningTrace, TitleCleaner};
pub use discovery::{AdDiscoveryTracker, DiscoveredAdCandidate, merge_discovered_candidates};
pub use engine::{CompiledFilterEngine, FilterCategory, FilterOutcome, RuleHit};
pub use error::FilterError;
pub use legacy::{
    ImportedFilterData, LegacyLimitsConfig, import_any_filter_json, parse_legacy_filter_json,
    parse_stored_policy_json,
};
pub use playground::{
    AbComparisonItem, AbComparisonReport, AbComparisonSummary, AbDiffStatus, PlaygroundInput,
    PlaygroundItemResult, compare_playground_ab, evaluate_playground_batch,
    evaluate_playground_one,
};
pub use preset::{
    ExtensionPackInfo, FilterPresetCatalog, KeywordPackInfo, PolicyPresetInfo, RuleTemplateInfo,
    builtin_ad_phrase_note, builtin_ad_regex_note, builtin_preset_catalog, default_ad_phrases,
    default_ad_regexes, default_online_filter_bundle, default_online_filter_policy,
    policy_preset_bundle, upgrade_legacy_builtin_notes,
};
pub use runtime::{
    DEFAULT_REJECTED_SAMPLE_CAPACITY, FilterRuntime, FilterStatsSnapshot, RejectedSample,
    RuleHitStat,
};
pub use schema::{
    AdPhraseRule, AdRegexRule, CleanerOptions, CompositeLogic, DensityGate, ExtensionScope,
    FILTER_POLICY_SCHEMA_VERSION, FileConstraintMode, FilterBundle, FilterGates, FilterPolicy,
    KeywordMatchMode, PolicyAction, PolicyRule, RuleMatcher, TextTarget, TitlePatternKind,
    ToggleU64, ToggleUsize, normalize_extension,
};
pub use smart::ScoredSpanHit;

#[cfg(test)]
mod tests {
    use super::*;
    use hameta_common::{FileInfo, TorrentMessage};
    use serde_json::json;

    fn make_engine(policy: FilterPolicy) -> CompiledFilterEngine {
        CompiledFilterEngine::compile(&FilterBundle {
            version: 1,
            policy,
            ad_phrases: default_ad_phrases(),
            ad_regexes: default_ad_regexes(),
        })
        .unwrap()
    }

    fn torrent_message(name: &str) -> TorrentMessage {
        TorrentMessage {
            info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
            name: name.to_string(),
            total_size: 1024 * 1024 * 1024,
            file_count: 1,
            files: vec![FileInfo {
                path: format!("{name}.mp4"),
                size: 1024 * 1024 * 1024,
            }],
            timestamp: 0,
        }
    }

    fn torrent_message_with_file(name: &str, path: &str, size: u64) -> TorrentMessage {
        TorrentMessage {
            info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
            name: name.to_string(),
            total_size: size,
            file_count: 1,
            files: vec![FileInfo {
                path: path.to_string(),
                size,
            }],
            timestamp: 0,
        }
    }

    fn torrent_message_with_files(name: &str, files: Vec<(&str, u64)>) -> TorrentMessage {
        let file_infos: Vec<FileInfo> = files
            .iter()
            .map(|(path, size)| FileInfo {
                path: (*path).to_string(),
                size: *size,
            })
            .collect();
        let total_size: u64 = file_infos.iter().map(|f| f.size).sum();
        let file_count = file_infos.len();
        TorrentMessage {
            info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
            name: name.to_string(),
            total_size,
            file_count,
            files: file_infos,
            timestamp: 0,
        }
    }

    // ==================== 标题清洗回归测试（对齐原 text.rs） ====================

    #[test]
    fn cleaner_strips_leading_url_and_torrenting_prefixes() {
        let cleaner = TitleCleaner::default_baseline();
        assert_eq!(
            cleaner.refine("www.UIndex.org    -    Greenland.2.Migration.2026.1080p.x264-SyncUP"),
            "Greenland.2.Migration.2026.1080p.x264-SyncUP"
        );
        assert_eq!(
            cleaner.refine("www.Torrenting.com - A Man Called Horse 1970 1080p BluRay x264-OFT"),
            "A Man Called Horse 1970 1080p BluRay x264-OFT"
        );
        assert_eq!(
            cleaner.refine("madoubt.com 826983.xyz caribbeancom-042817_420"),
            "caribbeancom-042817_420"
        );
    }

    #[test]
    fn cleaner_strips_embedded_domains_and_bracket_labels() {
        let cleaner = TitleCleaner::default_baseline();
        assert_eq!(
            cleaner.refine("Rick.and.Morty.S07E05.1080p.WEB-DL.www.YTS.MX.mp4"),
            "Rick.and.Morty.S07E05.1080p.WEB-DL.mp4"
        );
        assert_eq!(
            cleaner.refine("[电影天堂www.dy2018.com]消失的她.1080p.HD国语中字.mp4"),
            "消失的她.1080p.HD国语中字.mp4"
        );
        assert_eq!(
            cleaner.refine("Service.Animals.5.EnGlisH.XXX.DVDRip.XviD-WwW.Torrentesx.CoM.mpg"),
            "Service.Animals.5.EnGlisH.XXX.DVDRip.XviD.mpg"
        );
        assert_eq!(
            cleaner.refine("Avengers.Endgame.2019.www.torrenting.org.1080p.mkv"),
            "Avengers.Endgame.2019.1080p.mkv"
        );
    }

    #[test]
    fn cleaner_strips_diverse_tlds_and_bracketed_site_prefixes() {
        let cleaner = TitleCleaner::default_baseline();
        assert_eq!(
            cleaner
                .refine("www.youiv.pw 少女偶像 原创 NMNS-019-B Yuna Otomo 大友優奈 FACE ZONE 2 BD"),
            "少女偶像 原创 NMNS-019-B Yuna Otomo 大友優奈 FACE ZONE 2 BD"
        );
        assert_eq!(
            cleaner.refine(
                "www.1TamilMV.eu - Inside Out 2 (2024) English HDCAM-Rip - 720p - x264 - HQ Clean"
            ),
            "Inside Out 2 (2024) English HDCAM-Rip - 720p - x264 - HQ Clean"
        );
        assert_eq!(
            cleaner.refine("www.5MovieRulz.graphics - Derby (2026) Malayalam TRUE WEB-DL"),
            "Derby (2026) Malayalam TRUE WEB-DL"
        );
        assert_eq!(
            cleaner.refine("www.1TamilMV.cards - Carmeni Selvam (2026) Tamil HQ HDRip - x264 - AAC - 400MB - ESub.mkv"),
            "Carmeni Selvam (2026) Tamil HQ HDRip - x264 - AAC - 400MB - ESub.mkv"
        );
        assert_eq!(
            cleaner.refine("[ www.Torrent9.uno ] Capharnaum.2018.FRENCH.BDRip.XviD-ACOOL.avi"),
            "Capharnaum.2018.FRENCH.BDRip.XviD-ACOOL.avi"
        );
        assert_eq!(
            cleaner.refine("【高清剧集网发布 www.BTHDTV.com】我的王室死对头[第06集][简繁英字幕]"),
            "我的王室死对头[第06集][简繁英字幕]"
        );
        assert_eq!(
            cleaner.refine("Los bicivoladores (1983) [BluRay 720p X264 MKV][AC3 5.1 Castellano][www.atomixHQ.ART].mkv"),
            "Los bicivoladores (1983) [BluRay 720p X264 MKV][AC3 5.1 Castellano].mkv"
        );
        assert_eq!(
            cleaner.refine("www.Channel.tv - Episode One (2026).mkv"),
            "Episode One (2026).mkv"
        );
    }

    #[test]
    fn cleaner_removes_chinese_ads_and_deobfuscates() {
        let cleaner = TitleCleaner::default_baseline();
        assert_eq!(
            cleaner.refine("周处除三害.2023.HD1080P.国语中字.mp4 更多资源访问 www.66ys.co"),
            "周处除三害.2023.HD1080P.国语中字.mp4"
        );
        assert_eq!(
            cleaner.refine("尘兔.6v电影 地址发布页 www.6v123.com 收藏不迷路"),
            "尘兔.6v电影"
        );
        assert_eq!(
            cleaner.refine(
                "2025年04月大神BT最新域名 992393.xyz 【重度BDSM】韩国巅峰女S「Yerin」OF重度调教私拍 男女奴一边做爱一边受刑，大幅超越常规调教"
            ),
            "【重度BDSM】韩国巅峰女S「Yerin」OF重度调教私拍 男女奴一边做爱一边受刑,大幅超越常规调教"
        );
        assert_eq!(
            cleaner.refine("BT最新域名 123456.xyz 正常电影标题 2024"),
            "正常电影标题 2024"
        );
        assert_eq!(
            cleaner.refine("Dune.Part.Two.2024.1080p.WEBRip.x264.AAC-[ Y T S . M X ]"),
            "Dune.Part.Two.2024.1080p.WEBRip.x264.AAC-[YTS.MX]"
        );
        assert_eq!(
            cleaner.refine("Transformers.Rise.of.the.Beasts.2023.[w w w . y t s . m x].1080p"),
            "Transformers.Rise.of.the.Beasts.2023.1080p"
        );
        assert_eq!(cleaner.refine("最 新 位 址 獲 取"), "最新位址獲取");
        assert_eq!(
            cleaner.refine("Dune.Part.Two.2024.1080p．ＷＷＷ．ＹＴＳ．ＭＸ"),
            "Dune.Part.Two.2024.1080p"
        );
        assert_eq!(cleaner.refine("ＡＶＥＮＧＥＲＳ.２０１９"), "AVENGERS.2019");
    }

    #[test]
    fn cleaner_preserves_normal_releases_and_cleans_padding() {
        let cleaner = TitleCleaner::default_baseline();
        let release =
            "Avengers.Endgame.2019.2160p.BluRay.REMUX.HEVC.DTS-HD.MA.TrueHD.7.1.Atmos-FGT";
        assert_eq!(cleaner.refine(release), release);

        let anime = "[Beatrice-Raws] Mushoku Tensei II - 13 [WEB 1920x1080 HEVC AAC].mkv";
        assert_eq!(cleaner.refine(anime), anime);

        let codec = "House.of.the.Dragon.S02E01.2160p.MAX.WEB-DL.DDP5.1.Atmos.DV.HDR10.H.265-FLUX";
        assert_eq!(cleaner.refine(codec), codec);

        let dotted_prefix = "www.voiny.Ohotniki.na.vampirov.(4.serii.iz.4).1997-1998.XviD.DVDRip";
        assert_eq!(cleaner.refine(dotted_prefix), dotted_prefix);

        assert_eq!(cleaner.refine("--- === Movie Title === ---"), "Movie Title");
        assert_eq!(cleaner.refine("==="), "");
        assert_eq!(cleaner.refine("www.example.com"), "");
    }

    #[test]
    fn cleaner_options_stage_toggles_and_custom_replacement() {
        let bundle = FilterBundle {
            version: 1,
            policy: FilterPolicy {
                cleaner: CleanerOptions {
                    enable_nfkc: false,
                    enable_ad_phrases: false,
                    reject_if_empty_after_clean: false,
                    ..CleanerOptions::default()
                },
                ..FilterPolicy::default()
            },
            ad_phrases: default_ad_phrases(),
            ad_regexes: vec![AdRegexRule {
                id: "ad-custom".into(),
                pattern: r"\[AD-[0-9]+\]".into(),
                replacement: " [CLEAN] ".into(),
                enabled: true,
                note: String::new(),
            }],
        };
        let engine = CompiledFilterEngine::compile(&bundle).unwrap();
        let msg = torrent_message("ＡＶＥＮＧＥＲＳ [AD-99] 更多资源");
        let trace = engine.cleaner().refine_with_trace(&msg.name);
        assert!(
            !trace
                .steps
                .iter()
                .find(|s| s.stage == "nfkc")
                .unwrap()
                .enabled
        );
        assert!(
            !trace
                .steps
                .iter()
                .find(|s| s.stage == "remove_ad_phrases")
                .unwrap()
                .enabled
        );
        assert!(trace.refined_name.contains("[CLEAN]"));
        assert!(trace.refined_name.contains("更多资源"));
        assert!(trace.refined_name.contains("ＡＶＥＮＧＥＲＳ"));

        // 当关闭 reject_if_empty_after_clean 时，纯 URL 清洗后为空仍允许放行
        assert!(engine.is_valid(&torrent_message("www.example.com")));
    }

    // ==================== Filter Policy v2 核心机制测试 ====================

    #[test]
    fn filter_v2_blocks_junk_archives_and_all_empty_gates() {
        let policy = FilterPolicy {
            gates: FilterGates {
                reject_all_empty_files: true,
                min_total_size: ToggleU64::on(1),
                ..FilterGates::default()
            },
            rules: vec![
                PolicyRule {
                    id: "sub".into(),
                    name: "substantive".into(),
                    enabled: true,
                    action: PolicyAction::Reject,
                    tags: vec![],
                    note: String::new(),
                    matcher: RuleMatcher::SubstantiveContent {
                        junk_extensions: vec![
                            "ink".into(),
                            "url".into(),
                            "lnk".into(),
                            "scf".into(),
                            "html".into(),
                            "txt".into(),
                        ],
                        include_system_junk: true,
                        extra_junk_regexes: vec![],
                        min_files: 1,
                        min_total_size: 1024,
                    },
                },
                PolicyRule {
                    id: "ratio".into(),
                    name: "archive ratio".into(),
                    enabled: true,
                    action: PolicyAction::Reject,
                    tags: vec![],
                    note: String::new(),
                    matcher: RuleMatcher::SizeRatio {
                        extensions: vec!["zip".into(), "rar".into(), "7z".into()],
                        include_split_archives: true,
                        extra_regexes: vec![],
                        max_ratio: 0.5,
                    },
                },
            ],
            ..FilterPolicy::default()
        };
        let engine = make_engine(policy);

        assert!(!engine.is_valid(&torrent_message_with_files(
            "发错空种",
            vec![("empty.mkv", 0), ("also.empty.mp4", 0)],
        )));
        assert!(!engine.is_valid(&torrent_message_with_files(
            "广告包",
            vec![("visit.url", 128), ("readme.txt", 256), ("site.ink", 64)],
        )));
        assert!(!engine.is_valid(&torrent_message_with_files(
            "mac 打包残留",
            vec![("__MACOSX/._movie.mkv", 4096), ("__MACOSX/movie.mkv", 120)],
        )));
        assert!(!engine.is_valid(&torrent_message_with_files(
            "本质压缩包",
            vec![
                ("movie/sample.mkv", 10 * 1024 * 1024),
                ("pack.rar", 15 * 1024 * 1024),
            ],
        )));
        assert!(engine.is_valid(&torrent_message_with_files(
            "正常电影带小附件和系统残留",
            vec![
                ("movie/sample.mkv", 90 * 1024 * 1024),
                ("extras/sample.zip", 10 * 1024 * 1024),
                (".DS_Store", 6148),
            ],
        )));
    }

    #[test]
    fn filter_v2_word_boundary_prevents_false_positives() {
        let engine = make_engine(FilterPolicy {
            rules: vec![PolicyRule {
                id: "kw-wb".into(),
                name: "整词拦截".into(),
                enabled: true,
                action: PolicyAction::Reject,
                tags: vec![],
                note: String::new(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::WordBoundary,
                    case_sensitive: false,
                    words: vec!["hack".into(), "patch".into(), "破解版".into()],
                },
            }],
            ..FilterPolicy::default()
        });

        assert!(!engine.is_valid(&torrent_message("Windows.Hack.2026.Edition")));
        assert!(!engine.is_valid(&torrent_message("Game_Patch_v1.2")));
        assert!(!engine.is_valid(&torrent_message("超强工具破解版下载")));
        assert!(engine.is_valid(&torrent_message("The.Shack.2017.1080p.BluRay")));
        assert!(engine.is_valid(&torrent_message("Love.LifeHack.Documentary.2026")));
        assert!(engine.is_valid(&torrent_message("Dispatch.Season.01.1080p")));
    }

    #[test]
    fn filter_v2_allow_priority_and_composite_and_at_least_one_file_constraint() {
        let policy = FilterPolicy {
            rules: vec![
                // 优先级 1：含 .exe 硬拦截（在白名单之前）
                PolicyRule {
                    id: "ext-exe".into(),
                    name: "exe hard block".into(),
                    enabled: true,
                    action: PolicyAction::Reject,
                    tags: vec![],
                    note: String::new(),
                    matcher: RuleMatcher::ExtensionRule {
                        scope: ExtensionScope::AnyFile,
                        extensions: vec!["exe".into()],
                    },
                },
                // 优先级 2：组合白名单（标题含 4K/REMUX 且存在 >= 500MB 视频文件）
                PolicyRule {
                    id: "allow-4k".into(),
                    name: "4K REMUX 白名单".into(),
                    enabled: true,
                    action: PolicyAction::Allow,
                    tags: vec![],
                    note: String::new(),
                    matcher: RuleMatcher::Composite {
                        logic: CompositeLogic::All,
                        conditions: vec![
                            RuleMatcher::KeywordGroup {
                                target: TextTarget::RefinedName,
                                match_mode: KeywordMatchMode::WordBoundary,
                                case_sensitive: false,
                                words: vec!["4K".into(), "REMUX".into()],
                            },
                            RuleMatcher::FileConstraint {
                                extensions: vec!["mkv".into(), "mp4".into()],
                                extra_regexes: vec![],
                                mode: FileConstraintMode::AtLeastOneMatched,
                                min_file_size: Some(500 * 1024 * 1024),
                                max_file_size: None,
                                min_file_name_length: None,
                                max_file_name_length: None,
                            },
                        ],
                    },
                },
                // 优先级 3：普通严格视频体积校验（若未被白名单放行，则小花絮也会命中）
                PolicyRule {
                    id: "fc-strict".into(),
                    name: "所有视频 >= 100MB".into(),
                    enabled: true,
                    action: PolicyAction::Reject,
                    tags: vec![],
                    note: String::new(),
                    matcher: RuleMatcher::FileConstraint {
                        extensions: vec!["mkv".into(), "mp4".into()],
                        extra_regexes: vec![],
                        mode: FileConstraintMode::EveryMatched,
                        min_file_size: Some(100 * 1024 * 1024),
                        max_file_size: None,
                        min_file_name_length: None,
                        max_file_name_length: None,
                    },
                },
            ],
            ..FilterPolicy::default()
        };
        let engine = make_engine(policy);

        // 带 .exe 即使标题有 4K REMUX 且有大视频也必须先被优先级 1 拦截
        let exe_with_remux = torrent_message_with_files(
            "Movie.2026.4K.REMUX",
            vec![("movie.mkv", 2 * 1024 * 1024 * 1024), ("payload.exe", 1024)],
        );
        assert_eq!(
            engine.evaluate(&exe_with_remux).hit().unwrap().rule_id,
            "ext-exe"
        );

        // 带 2GB 正片 + 2MB 预告花絮的 4K REMUX -> 由白名单 Allow 放行（豁免 fc-strict！）
        let remux_with_sample = torrent_message_with_files(
            "Dune.2024.4K.REMUX.HEVC",
            vec![
                ("Dune.2024.mkv", 2 * 1024 * 1024 * 1024),
                ("trailer.mkv", 2 * 1024 * 1024),
            ],
        );
        let outcome = engine.evaluate(&remux_with_sample);
        assert!(outcome.is_accepted());
        assert_eq!(outcome.allowed_by().unwrap().rule_id, "allow-4k");

        // 不含 4K/REMUX 的普通电影带小花絮 -> 不命中白名单，最终被 fc-strict 拦截
        let normal_with_sample = torrent_message_with_files(
            "Dune.2024.1080p",
            vec![
                ("Dune.2024.mkv", 2 * 1024 * 1024 * 1024),
                ("trailer.mkv", 2 * 1024 * 1024),
            ],
        );
        assert_eq!(
            engine.evaluate(&normal_with_sample).hit().unwrap().rule_id,
            "fc-strict"
        );
    }

    #[test]
    fn composite_depth_exceeding_limit_and_invalid_ratio_fail_compile() {
        let deep = RuleMatcher::Composite {
            logic: CompositeLogic::All,
            conditions: vec![RuleMatcher::Composite {
                logic: CompositeLogic::Any,
                conditions: vec![RuleMatcher::Composite {
                    logic: CompositeLogic::All,
                    conditions: vec![RuleMatcher::Composite {
                        logic: CompositeLogic::All,
                        conditions: vec![RuleMatcher::ExtensionRule {
                            scope: ExtensionScope::AnyFile,
                            extensions: vec!["exe".into()],
                        }],
                    }],
                }],
            }],
        };
        let err = CompiledFilterEngine::compile(&FilterBundle {
            version: 1,
            policy: FilterPolicy {
                rules: vec![PolicyRule {
                    id: "too-deep".into(),
                    matcher: deep,
                    ..PolicyRule::default()
                }],
                ..FilterPolicy::default()
            },
            ..FilterBundle::default()
        })
        .err()
        .unwrap();
        assert!(matches!(err, FilterError::InvalidRule(_)));

        let bad_ratio = CompiledFilterEngine::compile(&FilterBundle {
            version: 1,
            policy: FilterPolicy {
                rules: vec![PolicyRule {
                    id: "bad-ratio".into(),
                    matcher: RuleMatcher::SizeRatio {
                        extensions: vec!["zip".into()],
                        include_split_archives: false,
                        extra_regexes: vec![],
                        max_ratio: 1.5,
                    },
                    ..PolicyRule::default()
                }],
                ..FilterPolicy::default()
            },
            ..FilterBundle::default()
        })
        .err()
        .unwrap();
        assert!(matches!(bad_ratio, FilterError::InvalidRule(_)));
    }

    // ==================== v1 预设 vs 旧版 JSON 智能转换等价性矩阵测试 ====================

    #[test]
    fn online_standard_and_legacy_converted_match_on_equivalence_matrix() {
        let online_bundle = default_online_filter_bundle();
        let online_engine = CompiledFilterEngine::compile(&online_bundle).unwrap();

        // 构造完整的原版 torrent-indexer config.json 等价 JSON 并走智能转换
        let legacy_full_json = json!({
            "filter": {
                "blocked_rules": [
                    "hacker", "hack", "crack", "cracked", "patch", "keygen", "license", "licence",
                    "activator", "activate", "activation", "loader", "kms", "hacktool", "trojan",
                    "virus", "malware", "worm", "ransom", "ransomware", "backdoor", "spyware",
                    "adware", "rootkit", "botnet", "miner", "mining", "payload", "exploit", "dropper",
                    "readme", "read me", "read_me", "readme_before", "password", "pass_list",
                    "pass list", "install_instructions", "install instructions", "how to install",
                    "how_to_install", "license_key", "license key", "网址发布器", "解压密码",
                    "破解版", "破解", "补丁", "注册机", "注册码", "序列号", "激活码", "激活工具",
                    "激活器", "免激活", "免注册", "绿色版", "精简版", "汉化补丁", "外挂", "辅助",
                    "勒索", "后门", "挖矿", "攻击", "unknown", "undefined"
                ],
                "blocked_file_patterns": [
                    r"(?i)\.(exe|msi|msp|bat|cmd|com|scr|pif|vbs|vbe|js|jse|wsf|wsh|hta|ps1|psm1|reg|sys|drv|cpl|msc|apk|xapk|apks|ipa)$",
                    r"(?i)\.torrent$"
                ],
                "blocked_name_patterns": [
                    r"\p{Cyrillic}{2,}",
                    r"^\d+$",
                    r"^[0-9a-fA-F]{40}$",
                    r"^[0-9a-fA-F]{32}$",
                    r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
                    r"^\d+\.\d{3}$",
                    r"(?i)^magnet:\?",
                    r"(?i)^https?://",
                    r"(?i)\.(zip|rar|7z|iso|img|vhd|vhdx|tar|gz|tgz|bz2|tbz2|xz|txz|cab|ace|lzh|lha|sit|sitx|z|lzma|bz)$",
                    r"(?i)\.(zip|rar|7z|iso|img|tar|gz|tgz|bz2|tbz2|xz|txz|cab|ace)\.\d+$",
                    r"(?i)\.part\d+\.rar$",
                    r"(?i)\.[rz]\d{2}$",
                    r"(?i)\.(exe|dll|scr|com|pif|msi|msp|bat|bin|cmd|vbs|vbe|vb|js|jse|wsf|wsh|hta|ps1|psm1|ps1xml|ps2|psc1|psc2|sct|msc|cpl|reg|sys|drv|ocx|jar|class|sh|run|app|ipa|apk|xapk|apks|deb|rpm|pkg|dmg)$",
                    r"(?i)\.(doc|docx|docm|dotm|xls|xlsx|xlsm|xlam|xll|ppt|pptx|pptm|chm|one|slk|iqy|mobi)$",
                    r"(?i)\.(ink|url|lnk|scf|html|txt)$"
                ],
                "custom_rules": [
                    {
                        "patterns": [r"(?i)\.(mp4|avi|mkv|mov|wmv|flv|webm|m4v|mpg|mpeg|3gp|rm|rmvb|vob|asf|divx|xvid|f4v|m2ts|ts)$"],
                        "min_name_length": 4,
                        "max_name_length": 128,
                        "min_file_size": 10_485_760_u64,
                        "max_file_size": 0
                    }
                ],
                "torrent_rules": [
                    {
                        "junk_patterns": [
                            r"(?i)\.(ink|url|lnk|scf|html|txt)$",
                            r"(?i)\.ds_store$",
                            r"(?i)thumbs\.db$",
                            r"(?i)desktop\.ini$",
                            r"(?i)(^|/)__macosx/",
                            r"(?i)(^|/)\._"
                        ],
                        "min_substantive_files": 1,
                        "min_substantive_total_size": 1_048_576
                    },
                    {
                        "reject_if_all_empty": true,
                        "min_total_size": 1
                    },
                    {
                        "archive_patterns": [
                            r"(?i)\.(zip|rar|7z|iso|img|vhd|vhdx|tar|gz|tgz|bz2|tbz2|xz|txz|cab|ace|lzh|lha|sit|sitx|z|lzma|bz)$",
                            r"(?i)\.(zip|rar|7z|iso|img|tar|gz|tgz|bz2|tbz2|xz|txz|cab|ace)\.\d+$"
                        ],
                        "max_archive_size_ratio": 0.7
                    }
                ],
                "limits": {
                    "min_name_length": 4,
                    "max_name_length": 128,
                    "max_total_size": 2_199_023_255_552_u64,
                    "min_file_count_for_density_check": 100,
                    "min_avg_file_size": 102_400,
                    "max_file_name_length": 0,
                    "max_file_count": 2000
                }
            }
        })
        .to_string();

        let imported = import_any_filter_json(&legacy_full_json).unwrap();
        assert_eq!(imported.source_format, "legacy_config");
        // 验证旧版 15 条标题正则和 2 条文件正则全部转译为可视化规则（0 条 CustomRegex！）
        assert!(
            !imported
                .policy
                .rules
                .iter()
                .any(|r| matches!(r.matcher, RuleMatcher::CustomRegex { .. })),
            "标准旧版规则不应残留任何手写 CustomRegex"
        );
        let converted_engine = CompiledFilterEngine::compile(&FilterBundle {
            version: 1,
            policy: imported.policy,
            ad_phrases: default_ad_phrases(),
            ad_regexes: default_ad_regexes(),
        })
        .unwrap();

        let dense_files: Vec<(&str, u64)> = (0..105).map(|_| ("chunk.bin", 1024)).collect();
        let matrix: Vec<(bool, TorrentMessage)> = vec![
            (
                true,
                torrent_message_with_file(
                    "www.UIndex.org - Avengers.Endgame.2019.1080p.mkv",
                    "Avengers.Endgame.2019.1080p.mkv",
                    2 * 1024 * 1024 * 1024,
                ),
            ),
            (
                true,
                torrent_message_with_file(
                    "Ubuntu.24.04.LTS.Server.amd64",
                    "ubuntu-24.04-live-server-amd64.bin",
                    1_500_000_000,
                ),
            ),
            (
                false,
                torrent_message_with_file("abc", "abc.mkv", 20_000_000),
            ),
            (
                false,
                torrent_message_with_file("   ", "empty_name.mkv", 20_000_000),
            ),
            (
                false,
                torrent_message_with_file("Adobe.Photoshop.Cracked.2026", "setup.bin", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("解压密码在说明里", "archive.bin", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("русский фильм", "movie.mkv", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("1234567890", "video.mkv", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file(
                    "0123456789abcdef0123456789abcdef01234567",
                    "video.mkv",
                    50_000_000,
                ),
            ),
            (
                false,
                torrent_message_with_file("https://spam.example.com/file", "video.mkv", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("Game.Setup.Pack.zip", "data.bin", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("Movie.Part1.rar", "movie.mkv", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("Driver_Tool.exe", "driver.bin", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("Document_Macro.docm", "data.bin", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("Nested.Torrent.Pack", "inside.torrent", 2048),
            ),
            (
                false,
                torrent_message_with_file("Trojan.Installer.Hidden", "setup.exe", 50_000_000),
            ),
            (
                false,
                torrent_message_with_file("Tiny.Bait.Video", "sample.mp4", 2 * 1024 * 1024),
            ),
            (
                false,
                torrent_message_with_files(
                    "Only.System.Junk.Inside",
                    vec![(".DS_Store", 2 * 1024 * 1024), ("readme.txt", 1024)],
                ),
            ),
            (
                false,
                torrent_message_with_files(
                    "Heavy.Archive.Ratio",
                    vec![
                        ("movie.mkv", 20 * 1024 * 1024),
                        ("pack.7z", 80 * 1024 * 1024),
                    ],
                ),
            ),
            (
                false,
                torrent_message_with_files(
                    "All.Zero.Bytes.Files",
                    vec![("empty1.mkv", 0), ("empty2.mkv", 0)],
                ),
            ),
            (
                false,
                torrent_message_with_files("Too.Many.Tiny.Chunks", dense_files),
            ),
            (
                false,
                torrent_message_with_file("www.example.com", "movie.mkv", 200 * 1024 * 1024),
            ),
        ];

        for (idx, (expected_accept, msg)) in matrix.iter().enumerate() {
            let r1 = online_engine.is_valid(msg);
            let r2 = converted_engine.is_valid(msg);
            assert_eq!(
                r1,
                *expected_accept,
                "online_standard sample #{idx} ({}) expected {expected_accept}, got {:?}",
                msg.name,
                online_engine.evaluate(msg)
            );
            assert_eq!(
                r2,
                *expected_accept,
                "legacy_converted sample #{idx} ({}) expected {expected_accept}, got {:?}",
                msg.name,
                converted_engine.evaluate(msg)
            );
        }
    }

    #[test]
    fn stored_v1_policy_json_upgrades_cleanly_preserving_disabled_flags() {
        let v1_stored = json!({
            "blocked_rules": [
                { "id": "kw-1", "value": "hacker", "enabled": false, "note": "temporarily off" },
                { "id": "kw-2", "value": "trojan", "enabled": true, "note": "" }
            ],
            "blocked_name_patterns": [
                { "id": "name-re-1", "pattern": "(?i)^spam_tag_", "enabled": true, "note": "" }
            ],
            "blocked_file_patterns": [
                { "id": "file-re-1", "pattern": "(?i)\\.torrent$", "enabled": true, "note": "" }
            ],
            "custom_rules": [],
            "torrent_rules": [],
            "limits": {
                "min_name_length": 4,
                "max_name_length": 128,
                "max_total_size": 0,
                "min_file_count_for_density_check": 0,
                "min_avg_file_size": 0,
                "max_file_name_length": 0,
                "max_file_count": 0
            }
        })
        .to_string();

        let policy = parse_stored_policy_json(&v1_stored).unwrap();
        let engine = make_engine(policy);

        // disabled keyword `hacker` must remain disabled!
        assert!(engine.is_valid(&torrent_message("Documentary about Hacker culture")));
        // enabled keyword `trojan` still blocks
        assert!(!engine.is_valid(&torrent_message("Super Trojan Pack")));
        // custom regex preserved cleanly
        assert!(!engine.is_valid(&torrent_message("spam_tag_hello")));
        // extension regex upgraded to ExtensionRule
        assert!(!engine.is_valid(&torrent_message_with_file(
            "Valid Title",
            "inner.torrent",
            1024
        )));
    }

    #[test]
    fn legacy_importer_rejects_broken_regex_when_compiled() {
        let wrapped = r#"{"filter":{"blocked_name_patterns":["([unclosed"]}}"#;
        let policy = parse_legacy_filter_json(wrapped).unwrap();
        let err = CompiledFilterEngine::compile(&FilterBundle {
            version: 1,
            policy,
            ad_phrases: vec![],
            ad_regexes: vec![],
        })
        .err()
        .unwrap();
        assert!(matches!(err, FilterError::InvalidRegex { .. }));
    }

    #[test]
    fn preset_catalog_provides_multiple_valid_presets_and_templates() {
        let catalog = builtin_preset_catalog();
        assert_eq!(catalog.policy_presets.len(), 4);
        assert!(catalog.rule_templates.len() >= 10);
        assert!(catalog.extension_packs.len() >= 6);
        assert!(catalog.keyword_packs.len() >= 4);

        for preset in &catalog.policy_presets {
            CompiledFilterEngine::compile(&preset.bundle)
                .unwrap_or_else(|e| panic!("preset {} compile failed: {e}", preset.id));
        }

        let media = policy_preset_bundle("media_focused").unwrap();
        let media_engine = CompiledFilterEngine::compile(&media).unwrap();
        // 含 500MB 视频 + 1MB 预告花絮 + "Shack" 标题 -> 在高清影视预设置下应当放行且由白名单豁免
        let ok = media_engine.evaluate(&torrent_message_with_files(
            "The.Shack.2017.1080p.BluRay",
            vec![
                ("The.Shack.2017.mkv", 600 * 1024 * 1024),
                ("sample.mkv", 1024 * 1024),
            ],
        ));
        assert!(ok.is_accepted());
        assert!(ok.allowed_by().is_some());
    }

    #[test]
    fn runtime_hot_reload_is_atomic_and_tracks_ring_buffer_and_allow_hits() {
        let mut bundle = default_online_filter_bundle();
        bundle.version = 1;
        let runtime = FilterRuntime::with_sample_capacity(&bundle, 3).unwrap();
        assert_eq!(runtime.active_version(), 1);

        // 触发 4 条拦截以验证容量 3 的环形缓冲淘汰
        for i in 1..=4 {
            let msg =
                torrent_message_with_file(&format!("Hacker.Pack.{i}"), "pack.mkv", 20_000_000);
            assert!(!runtime.evaluate_and_record(&msg).is_accepted());
        }

        let stats = runtime.snapshot_stats(10);
        assert_eq!(stats.total_evaluated, 4);
        assert_eq!(stats.total_rejected, 4);
        assert_eq!(stats.rule_hits[0].hits, 4);
        assert_eq!(stats.rule_hits[0].action, PolicyAction::Reject);
        assert_eq!(stats.recent_rejected.len(), 3);
        assert_eq!(stats.recent_rejected[0].raw_name, "Hacker.Pack.4");
        assert_eq!(stats.recent_rejected[2].raw_name, "Hacker.Pack.2");

        // 非法正则热更新失败时不得破坏线上版本
        let mut bad_bundle = bundle.clone();
        bad_bundle.version = 2;
        bad_bundle.ad_regexes.push(AdRegexRule {
            id: "broken-ad".into(),
            pattern: "[broken".into(),
            replacement: " ".into(),
            enabled: true,
            note: String::new(),
        });
        assert!(runtime.reload(&bad_bundle).is_err());
        assert_eq!(runtime.active_version(), 1);

        // 合法版本 v2（含新增 Allow 规则）热切换后立即生效并统计 Allow 命中
        let mut v2 = bundle;
        v2.version = 2;
        v2.policy.rules.insert(
            0,
            PolicyRule {
                id: "allow-whitelist-test".into(),
                name: "测试白名单".into(),
                enabled: true,
                action: PolicyAction::Allow,
                tags: vec![],
                note: String::new(),
                matcher: RuleMatcher::KeywordGroup {
                    target: TextTarget::RawName,
                    match_mode: KeywordMatchMode::WordBoundary,
                    case_sensitive: false,
                    words: vec!["baitword_whitelist".into()],
                },
            },
        );
        v2.policy.rules.push(PolicyRule {
            id: "kw-custom".into(),
            name: "自定义黑名单".into(),
            enabled: true,
            action: PolicyAction::Reject,
            tags: vec![],
            note: String::new(),
            matcher: RuleMatcher::KeywordGroup {
                target: TextTarget::RawName,
                match_mode: KeywordMatchMode::Contains,
                case_sensitive: false,
                words: vec!["baitword".into()],
            },
        });
        assert_eq!(runtime.reload(&v2).unwrap(), 2);
        assert_eq!(runtime.active_version(), 2);

        let allowed =
            torrent_message_with_file("Movie baitword_whitelist edition", "movie.mkv", 500_000_000);
        let allow_res = runtime.evaluate_and_record(&allowed);
        assert!(allow_res.is_accepted());
        assert_eq!(
            allow_res.allowed_by().unwrap().rule_id,
            "allow-whitelist-test"
        );

        let blocked_by_v2 =
            torrent_message_with_file("Movie baitword edition", "movie.mkv", 500_000_000);
        assert!(!runtime.evaluate_and_record(&blocked_by_v2).is_accepted());

        let stats_v2 = runtime.snapshot_stats(10);
        assert!(
            stats_v2
                .rule_hits
                .iter()
                .any(|h| h.rule_id == "allow-whitelist-test"
                    && h.action == PolicyAction::Allow
                    && h.hits == 1)
        );
    }

    #[test]
    fn playground_cleaning_trace_and_ab_comparison() {
        let published_bundle = default_online_filter_bundle();
        let published = CompiledFilterEngine::compile(&published_bundle).unwrap();

        let mut draft_bundle = published_bundle.clone();
        draft_bundle.version = 2;
        draft_bundle.policy.rules.push(PolicyRule {
            id: "kw-extra".into(),
            name: "测试新增词".into(),
            enabled: true,
            action: PolicyAction::Reject,
            tags: vec![],
            note: String::new(),
            matcher: RuleMatcher::KeywordGroup {
                target: TextTarget::RawName,
                match_mode: KeywordMatchMode::Contains,
                case_sensitive: false,
                words: vec!["badtag".into()],
            },
        });
        draft_bundle.ad_phrases.push(AdPhraseRule {
            id: "phrase-extra".into(),
            phrase: "独家水印".into(),
            enabled: true,
            note: String::new(),
        });
        let draft = CompiledFilterEngine::compile(&draft_bundle).unwrap();

        let samples = vec![
            PlaygroundInput::from_title_only("正常电影.2026.1080p").to_torrent_message(),
            PlaygroundInput::from_title_only("好片子 独家水印 2026").to_torrent_message(),
            PlaygroundInput::from_title_only("电影 badtag 合集").to_torrent_message(),
            PlaygroundInput::from_title_only("WinRAR.Cracked.2026").to_torrent_message(),
        ];

        let batch = evaluate_playground_batch(&draft, &samples);
        assert_eq!(batch[1].refined_name, "好片子 2026");
        assert!(
            batch[1]
                .cleaning_trace
                .steps
                .iter()
                .any(|s| s.stage == "remove_ad_phrases" && s.changed)
        );

        let report = compare_playground_ab(&published, &draft, &samples);
        assert_eq!(report.summary.total, 4);
        assert_eq!(report.summary.unchanged_accepted, 1);
        assert_eq!(report.summary.refined_name_changed, 1);
        assert_eq!(report.summary.newly_rejected, 1);
        assert_eq!(report.summary.unchanged_rejected, 1);
        assert_eq!(
            report.items[1].diff_status,
            AbDiffStatus::RefinedNameChanged
        );
        assert_eq!(report.items[2].diff_status, AbDiffStatus::NewlyRejected);
    }

    #[test]
    fn cleaner_preserves_dot_separated_releases_with_ambiguous_tlds() {
        let cleaner = TitleCleaner::default_baseline();
        for intact in [
            "Avengers.2024.1080p.WebRip.CN.mkv",
            "Concert.2025.Live.1080p.mkv",
            "Show.S01E01.WEB-DL.CC.mkv",
            "Album.VIP.2025.flac",
            "Movie.2025.TOP.1080p.mkv",
        ] {
            assert_eq!(cleaner.refine(intact), intact);
        }
        assert_eq!(
            cleaner.refine("Dune.Part.Two.2024.ad99.com.1080p.mkv"),
            "Dune.Part.Two.2024.1080p.mkv"
        );
    }

    #[test]
    fn cleaner_preserves_file_ext_prefixes_and_strips_multi_layer_site_prefixes() {
        let cleaner = TitleCleaner::default_baseline();
        for intact in [
            "Clip01.mov - Raw Footage",
            "README.md - Project Guide",
            "Assets.zip - Part 2",
        ] {
            assert_eq!(cleaner.refine(intact), intact);
        }
        assert_eq!(
            cleaner.refine("[www.siteA.com][www.siteB.net] Movie.2026"),
            "Movie.2026"
        );
        assert_eq!(
            cleaner.refine("www.siteA.com [www.siteB.org] Movie.2026"),
            "Movie.2026"
        );
    }

    #[test]
    fn cleaner_handles_zero_width_chars_spaced_domains_and_preserves_english() {
        let cleaner = TitleCleaner::default_baseline();
        assert_eq!(
            cleaner.refine("最\u{200b}新\u{200b}地\u{200b}址\u{feff}发布页 Movie 2026"),
            "Movie 2026"
        );
        assert_eq!(
            cleaner.refine("w\u{200b}w\u{200b}w.ad99.com - Movie.2026"),
            "Movie.2026"
        );
        assert_eq!(
            cleaner.refine("w w w . a d 9 9 . c o m - Movie 2026"),
            "Movie 2026"
        );
        assert_eq!(cleaner.refine("[ www . yts . mx ] Movie 2026"), "Movie 2026");
        assert_eq!(cleaner.refine("A B C 1 2 3 Movie"), "A B C 1 2 3 Movie");
        assert_eq!(cleaner.refine("电影 发 行 测 试 电影"), "电影 发行测试 电影");
        assert_eq!(cleaner.refine("电影 最 新 地 址 电影"), "电影 电影");
        assert_eq!(cleaner.refine("大神bt最新域名 正常标题"), "正常标题");
    }

    #[test]
    fn cleaner_strips_url_paths_nested_empty_brackets_and_ext_spaces() {
        let cleaner = TitleCleaner::default_baseline();
        assert_eq!(
            cleaner.refine("www.ad99.com/go/123.html: Movie — "),
            "Movie"
        );
        assert_eq!(
            cleaner.refine("【 - 】[()] 周处除三害 更多资源 .mp4"),
            "周处除三害.mp4"
        );
        assert_eq!(
            cleaner.refine("[QQ群:88421099] Movie (公众号:影视君)"),
            "Movie"
        );
    }

    #[test]
    fn engine_rejects_short_refined_name_and_short_circuits_empty() {
        let engine = make_engine(default_online_filter_policy());
        let too_short = torrent_message_with_file("www.ad99.com - 1", "movie.mkv", 200 * 1024 * 1024);
        let outcome = engine.evaluate(&too_short);
        assert!(!outcome.is_accepted());
        assert_eq!(
            outcome.hit().unwrap().rule_id,
            "cleaner:min_refined_name_length"
        );

        let ok_msg = torrent_message_with_file("www.ad99.com - AB", "movie.mkv", 200 * 1024 * 1024);
        assert!(engine.evaluate(&ok_msg).is_accepted());

        let empty_msg = torrent_message_with_file("www.ad99.com", "bad.exe", 1024);
        let empty_out = engine.evaluate(&empty_msg);
        assert_eq!(
            empty_out.hit().unwrap().rule_id,
            "cleaner:empty_refined_name"
        );
    }

    #[test]
    fn builtin_ad_rules_have_friendly_notes_and_upgrade_legacy_baseline() {
        let phrases = default_ad_phrases();
        assert!(!phrases.is_empty());
        for item in &phrases {
            assert!(
                builtin_ad_phrase_note(&item.phrase).is_some(),
                "missing friendly note for phrase: {}",
                item.phrase
            );
            assert!(!item.note.trim().is_empty());
            assert_ne!(item.note, "内置基线");
        }

        let regexes = default_ad_regexes();
        assert!(!regexes.is_empty());
        for item in &regexes {
            assert!(
                builtin_ad_regex_note(&item.pattern).is_some(),
                "missing friendly note for regex: {}",
                item.pattern
            );
            assert!(!item.note.trim().is_empty());
            assert_ne!(item.note, "内置基线");
        }

        let catalog = builtin_preset_catalog();
        for tpl in &catalog.rule_templates {
            assert!(
                !tpl.rule.note.trim().is_empty(),
                "template rule {} should carry a non-empty friendly note",
                tpl.id
            );
        }

        let mut legacy_phrases = vec![
            AdPhraseRule {
                id: "p1".to_string(),
                phrase: "收藏不迷路".to_string(),
                enabled: true,
                note: "内置基线".to_string(),
            },
            AdPhraseRule {
                id: "p2".to_string(),
                phrase: "自定义广告".to_string(),
                enabled: true,
                note: "我的自定义备注".to_string(),
            },
            AdPhraseRule {
                id: "p3".to_string(),
                phrase: "故意留空".to_string(),
                enabled: true,
                note: String::new(),
            },
        ];
        let mut legacy_regexes = vec![
            AdRegexRule {
                id: "r1".to_string(),
                pattern: regexes[0].pattern.clone(),
                replacement: " ".to_string(),
                enabled: true,
                note: "内置基线".to_string(),
            },
            AdRegexRule {
                id: "r2".to_string(),
                pattern: "foo.*bar".to_string(),
                replacement: String::new(),
                enabled: true,
                note: "手动备注".to_string(),
            },
        ];

        upgrade_legacy_builtin_notes(&mut legacy_phrases, &mut legacy_regexes);
        assert_eq!(legacy_phrases[0].note, "站点收藏引导语");
        assert_eq!(legacy_phrases[1].note, "我的自定义备注");
        assert_eq!(legacy_phrases[2].note, "");
        assert_eq!(legacy_regexes[0].note, "日期前缀与 BT 最新域名/地址推广");
        assert_eq!(legacy_regexes[1].note, "手动备注");
    }

    #[test]
    fn cleaner_smart_l0_normalizes_confusables_dingbats_emojis_safely() {
        let cleaner = TitleCleaner::default_baseline();

        // 西里尔同形字伪装域名（a/o/c/m 为西里尔字母）被折叠后交由站点/域名阶段剥离
        assert_eq!(cleaner.refine("аd99.соm - 流浪地球2"), "流浪地球2");
        assert_eq!(cleaner.refine("ооо.соm - Movie.2026"), "Movie.2026");

        // 纯正俄文标题保持原样，不误折成 ASCII
        assert_eq!(
            smart::normalize_confusables_and_noise("русский фильм"),
            "русский фильм"
        );

        // 非 NFKC 带圈/黑底数字符号与营销 Emoji 降噪
        assert_eq!(
            cleaner.refine("🔥⭐ ❶号 🅰片 Movie.2026 ✅👇"),
            "1号 A片 Movie.2026"
        );

        // 关闭 enable_nfkc 但保留 enable_confusable_norm 时，不破坏原有全角关闭行为
        let no_nfkc = TitleCleaner::compile_with_options(
            &[],
            &[],
            &CleanerOptions {
                enable_nfkc: false,
                enable_confusable_norm: true,
                enable_span_ad_scoring: false,
                enable_filename_cross_clean: false,
                ..CleanerOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            no_nfkc.refine("ＡＶＥＮＧＥＲＳ 🔥 ❶"),
            "ＡＶＥＮＧＥＲＳ 1"
        );
    }

    #[test]
    fn cleaner_smart_l2a_span_scoring_strips_obfuscated_ads_and_preserves_media() {
        let cleaner = TitleCleaner::default_baseline();

        assert_eq!(
            cleaner.refine(
                "进裙加薇: avbot888 最新地址防失联 新址看完整版 99av88.cc 流浪地球2.2023.1080p.mkv"
            ),
            "流浪地球2.2023.1080p.mkv"
        );
        assert_eq!(
            cleaner.refine("流浪地球2.2023.1080p 【新址 9988bt.vip 每日更新 防迷路】.mkv"),
            "流浪地球2.2023.1080p.mkv"
        );

        let trace = cleaner.refine_with_trace("Movie.2026 【加薇: avbot888 防迷路】.mkv");
        assert_eq!(trace.refined_name, "Movie.2026.mkv");
        let scoring_step = trace
            .steps
            .iter()
            .find(|s| s.stage == "span_ad_scoring")
            .unwrap();
        assert!(scoring_step.changed);
        assert!(scoring_step.detail.as_deref().unwrap_or("").contains("剥离"));

        // 零误杀媒体保护列表
        for intact in [
            "Show.S01E01.WEB-DL.CC.mkv",
            "Album.VIP.2025.flac",
            "Movie.2025.TOP.1080p.mkv",
            "Concert.2025.Live.1080p.mkv",
            "Apple.TV.Original.2025.1080p.mkv",
            "Avengers.2024.1080p.WebRip.CN.mkv",
            "[YTS.MX] Movie.2024.1080p.mkv",
            "[Beatrice-Raws] Mushoku Tensei II - 13 [WEB 1920x1080 HEVC AAC].mkv",
            "【重度BDSM】韩国巅峰女S「Yerin」OF重度调教私拍",
        ] {
            assert_eq!(cleaner.refine(intact), intact, "should keep intact: {intact}");
        }

        // 关闭 span_ad_scoring 开关后保留原块
        let no_span = TitleCleaner::compile_with_options(
            &[],
            &[],
            &CleanerOptions {
                enable_span_ad_scoring: false,
                ..CleanerOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            no_span.refine("Movie.2026 [进裙加薇: avbot888]"),
            "Movie.2026 [进裙加薇: avbot888]"
        );
    }

    #[test]
    fn cleaner_smart_l2b_filename_cross_clean_peels_unseen_wrapper_safely() {
        let cleaner = TitleCleaner::default_baseline();
        let files = vec![FileInfo {
            path: "movies/Oppenheimer.2023.2160p.BluRay.x265.mkv".to_string(),
            size: 4_000_000_000,
        }];
        let trace = cleaner.refine_with_files_trace(
            "🔥 [未知新站水印] 奥本海默.Oppenheimer.2023.2160p.BluRay.mkv",
            &files,
        );
        assert_eq!(
            trace.refined_name,
            "奥本海默.Oppenheimer.2023.2160p.BluRay.mkv"
        );
        let cross_step = trace
            .steps
            .iter()
            .find(|s| s.stage == "filename_cross_clean")
            .unwrap();
        assert!(cross_step.changed);
        assert!(
            cross_step
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("Oppenheimer")
        );

        // 占位文件路径（sample/video.mkv）不会引发误剥离
        let placeholder = vec![FileInfo {
            path: "sample/video.mkv".to_string(),
            size: 100 * 1024 * 1024,
        }];
        assert_eq!(
            cleaner.refine_with_files("[Sakurato] Frieren - 01 [1080p].mkv", &placeholder),
            "[Sakurato] Frieren - 01 [1080p].mkv"
        );

        // 已彻底洗空的纯域名标题绝不会被文件名“复活”
        assert_eq!(cleaner.refine_with_files("www.example.com", &files), "");
    }

    #[test]
    fn discovery_tracker_surfaces_high_df_ads_and_runtime_excludes_known_phrases() {
        let mut tracker = AdDiscoveryTracker::new();
        let now = 1_795_400_000;
        for movie in ["流浪地球2", "消失的她", "奥本海默", "沙丘2"] {
            tracker.observe(&format!("[神秘新站888] {movie}.2025.1080p.mkv"), now);
            tracker.observe(&format!("[1080p] {movie}.mkv"), now);
        }
        // 单标题重复不构成高文档频次水印
        for _ in 0..5 {
            tracker.observe("[单标题重复] SameMovie.2025.mkv", now);
        }

        let empty_exclude = std::collections::HashSet::new();
        let cands = tracker.snapshot_candidates(20, &empty_exclude);
        assert!(
            cands
                .iter()
                .any(|c| c.phrase == "[神秘新站888]" && c.distinct_titles == 4 && c.occurrences == 4)
        );
        assert!(!cands.iter().any(|c| c.phrase == "[1080p]"));
        assert!(!cands.iter().any(|c| c.phrase == "[单标题重复]"));

        // 通过 FilterRuntime 接入时，已录入 ad_phrases 后应自动从候选中排除
        let mut bundle = default_online_filter_bundle();
        bundle.version = 1;
        let runtime = FilterRuntime::new(&bundle).unwrap();
        for movie in ["流浪地球2", "消失的她", "奥本海默"] {
            runtime.evaluate_and_record(&torrent_message(&format!(
                "【自发现新水】 {movie}.2025.1080p"
            )));
        }
        let snap1 = runtime.snapshot_stats(10);
        assert!(
            snap1
                .discovered_ad_candidates
                .iter()
                .any(|c| c.phrase == "【自发现新水】")
        );

        bundle.version = 2;
        bundle.ad_phrases.push(AdPhraseRule {
            id: "ad-discovered-1".into(),
            phrase: "【自发现新水】".into(),
            enabled: true,
            note: "智能统计自发现".into(),
        });
        runtime.reload(&bundle).unwrap();
        let snap2 = runtime.snapshot_stats(10);
        assert!(
            !snap2
                .discovered_ad_candidates
                .iter()
                .any(|c| c.phrase == "【自发现新水】")
        );
    }

    #[test]
    fn cleaner_options_legacy_json_deserializes_with_smart_defaults() {
        let legacy_json = json!({
            "enable_nfkc": true,
            "enable_deobfuscate_brackets": true,
            "enable_collapse_spaced_chars": true,
            "enable_strip_leading_site_prefix": true,
            "enable_strip_urls_and_domains": true,
            "enable_ad_regexes": true,
            "enable_ad_phrases": true,
            "enable_cleanup": true,
            "reject_if_empty_after_clean": true,
            "min_refined_name_length": { "enabled": true, "value": 2 }
        });
        let opts: CleanerOptions = serde_json::from_value(legacy_json).unwrap();
        assert!(opts.enable_confusable_norm);
        assert!(opts.enable_span_ad_scoring);
        assert_eq!(opts.span_ad_threshold, 65);
        assert!(opts.enable_filename_cross_clean);
    }
}
