pub mod config;
pub mod error;
pub mod hash;
pub mod ipc;
pub mod models;
pub mod password;
pub mod profiles;
pub mod sql_guard;

pub use config::{
    CrawlerBootstrapConfig, CrawlerCrawlConfig, CrawlerDiscoveryConfig, CrawlerMetadataConfig,
    CrawlerPeerLookupConfig, CrawlerPoolConfig, CrawlerRateLimitConfig, CrawlerSchedulerConfig,
    CrawlerTargetConfig, CrawlerWorkerConfig, DEFAULT_GEOIP_DOWNLOAD_URL,
    DEFAULT_GEOIP_RELEASES_URL, GeoIpConfig, IndexerWorkerConfig, ManticoreAuthMode,
    ManticoreServiceConfig, NetModeConfig, RedisServiceConfig,
};
pub use error::CommonError;
pub use hash::{TABLE_TORRENTS, hash_to_id, is_valid_info_hash, normalize_info_hash};
pub use ipc::{
    IngestEventItem, IngestEventType, WorkerBootstrap, WorkerDrainStatus,
    WorkerIngestEventsRequest, WorkerReadyRequest, WorkerRegisterRequest, WorkerReloadGeoIpRequest,
    WorkerReloadGeoIpResponse, WorkerReloadRulesRequest, WorkerReloadRulesResponse,
    WorkerShutdownRequest, WorkerSnapshotEnvelope, current_process_rss_bytes,
    current_soft_open_file_limit, parse_worker_bootstrap_str, read_worker_bootstrap,
    verify_bearer_token,
};
pub use models::{
    AuditLogEntry, DesiredState, FileInfo, FilterVersionStatus, MetricsHourPoint,
    MetricsMinutePoint, ServiceKind, TorrentMessage, TorrentRecord, WorkerRole, WorkerRuntimeState,
};
pub use password::{generate_token, hash_password, hash_token, verify_password};
pub use profiles::{
    CrawlProfileDefaults, CrawlProfileInfo, all_crawl_profiles, crawl_profile_defaults,
};
pub use sql_guard::{DEFAULT_SQL_LIMIT, MAX_SQL_LIMIT, prepare_console_sql, validate_readonly_sql};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_to_id_matches_reference_63_bit_mapping() {
        let hash = "5bbf5a52328d304c8091e6015b6b08a68026131c";
        let expected =
            (u64::from_str_radix("5bbf5a52328d304c", 16).unwrap() & 0x7FFF_FFFF_FFFF_FFFF) as i64;
        assert_eq!(hash_to_id(hash), expected);
        assert!(hash_to_id(hash) > 0);
        // 最高 bit 为 1 的前缀必须清零保证非负
        let high_bit = "ffffffffffffffff000000000000000000000000";
        assert_eq!(hash_to_id(high_bit), i64::MAX);
        // 短串与非十六进制返回 0
        assert_eq!(hash_to_id("5bbf5a52"), 0);
        assert_eq!(hash_to_id("zzzzzzzzzzzzzzzz000000000000000000000000"), 0);
        assert_eq!(hash_to_id("中文中文中文中文中文中文"), 0);
    }

    #[test]
    fn info_hash_validation_and_normalization() {
        assert!(is_valid_info_hash(
            "0123456789abcdef0123456789ABCDEF01234567"
        ));
        assert!(!is_valid_info_hash("0123"));
        assert!(!is_valid_info_hash(
            "g123456789abcdef0123456789abcdef01234567"
        ));
        let normalized =
            normalize_info_hash("  0123456789ABCDEF0123456789abcdef01234567 \n").unwrap();
        assert_eq!(normalized, "0123456789abcdef0123456789abcdef01234567");
    }

    #[test]
    fn password_salted_sha256_roundtrip() {
        let h1 = hash_password("secret-123").unwrap();
        let h2 = hash_password("secret-123").unwrap();
        assert_ne!(h1, h2);
        assert!(verify_password("secret-123", &h1));
        assert!(verify_password("secret-123", &h2));
        assert!(!verify_password("wrong", &h1));
        assert!(!verify_password("", &h1));
        assert!(hash_password("").is_err());

        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert_eq!(hash_token(&token).len(), 64);
        assert_eq!(hash_token("abc"), hash_token("abc"));
    }

    #[test]
    fn sql_guard_allows_readonly_and_injects_default_limit() {
        assert_eq!(
            validate_readonly_sql("SELECT * FROM torrents;", 200, 1000).unwrap(),
            "SELECT * FROM torrents LIMIT 200"
        );
        assert_eq!(
            validate_readonly_sql("  select id, info_hash from torrents limit 50  ", 200, 1000)
                .unwrap(),
            "select id, info_hash from torrents limit 50"
        );
        assert_eq!(
            validate_readonly_sql("SELECT * FROM torrents LIMIT 100, 500", 200, 1000).unwrap(),
            "SELECT * FROM torrents LIMIT 100, 500"
        );
        assert_eq!(
            validate_readonly_sql("DESCRIBE torrents;", 200, 1000).unwrap(),
            "DESCRIBE torrents"
        );
        assert_eq!(
            validate_readonly_sql("SHOW TABLE torrents STATUS", 200, 1000).unwrap(),
            "SHOW TABLE torrents STATUS"
        );
        // 字符串中包含关键字不应误拦截
        assert_eq!(
            validate_readonly_sql(
                "SELECT * FROM torrents WHERE raw_name = 'delete drop; insert'",
                200,
                1000
            )
            .unwrap(),
            "SELECT * FROM torrents WHERE raw_name = 'delete drop; insert' LIMIT 200"
        );
        // 末尾行注释不会吞掉自动附加的 LIMIT
        assert_eq!(
            validate_readonly_sql("SELECT * FROM torrents -- trailing", 200, 1000).unwrap(),
            "SELECT * FROM torrents -- trailing\nLIMIT 200"
        );
        // 仅子查询有 LIMIT 时，外层仍需自动补 LIMIT
        assert_eq!(
            validate_readonly_sql(
                "SELECT * FROM (SELECT * FROM torrents LIMIT 10) t",
                200,
                1000
            )
            .unwrap(),
            "SELECT * FROM (SELECT * FROM torrents LIMIT 10) t LIMIT 200"
        );
    }

    #[test]
    fn sql_guard_rejects_writes_multi_statement_and_excessive_limit() {
        for bad in [
            "",
            "   -- only comment",
            "DELETE FROM torrents WHERE id = 1",
            "DROP TABLE torrents",
            "ALTER TABLE torrents ADD COLUMN x int",
            "INSERT INTO torrents VALUES (1)",
            "SELECT * FROM torrents; DROP TABLE torrents",
            "SELECT * FROM torrents LIMIT 1001",
            "SELECT * FROM torrents LIMIT 0",
            "SELECT * FROM torrents LIMIT 10, 1500",
            "SELECT * FROM torrents WHERE raw_name = 'unclosed",
        ] {
            assert!(
                validate_readonly_sql(bad, 200, 1000).is_err(),
                "accepted: {bad}"
            );
        }
        // 带 LIMIT 的外层子查询应被允许
        assert!(
            validate_readonly_sql(
                "SELECT * FROM (SELECT * FROM torrents LIMIT 10) t LIMIT 5",
                200,
                1000
            )
            .is_ok()
        );
    }

    #[test]
    fn console_sql_allows_alter_and_dml_while_enforcing_single_statement_and_select_limit() {
        assert_eq!(
            prepare_console_sql(
                "  ALTER TABLE torrents ADD COLUMN total_size bigint;  ",
                200,
                1000
            )
            .unwrap(),
            "ALTER TABLE torrents ADD COLUMN total_size bigint"
        );
        assert_eq!(
            prepare_console_sql("DELETE FROM torrents WHERE id = 1;", 200, 1000).unwrap(),
            "DELETE FROM torrents WHERE id = 1"
        );
        assert_eq!(
            prepare_console_sql("SELECT * FROM torrents;", 200, 1000).unwrap(),
            "SELECT * FROM torrents LIMIT 200"
        );
        assert!(prepare_console_sql("SELECT * FROM torrents LIMIT 5000", 200, 1000).is_err());
        assert!(
            prepare_console_sql("ALTER TABLE torrents ADD COLUMN x int; SELECT 1", 200, 1000)
                .is_err()
        );
        assert!(prepare_console_sql("   ", 200, 1000).is_err());
    }

    #[test]
    fn crawl_profiles_match_reference_budgets() {
        let p1 = crawl_profile_defaults("P1").unwrap();
        assert_eq!(p1.find_node_rate_per_sec, 878);
        assert_eq!(p1.find_node_burst, 176);
        assert_eq!(p1.find_node_max_in_flight, 4_498);
        assert_eq!(p1.peer_lookup_rate_per_sec, 32);

        let p15 = crawl_profile_defaults("p15").unwrap();
        assert_eq!(p15.find_node_rate_per_sec, 3_840);
        assert_eq!(p15.find_node_burst, 768);
        assert_eq!(p15.find_node_max_in_flight, 19_661);

        let p16 = crawl_profile_defaults("P16").unwrap();
        assert_eq!(p16.find_node_rate_per_sec, 5_800);
        assert_eq!(p16.find_node_max_in_flight, 24_576);

        let p17 = crawl_profile_defaults("P17").unwrap();
        assert_eq!(p17.find_node_rate_per_sec, 10_000);
        assert_eq!(p17.find_node_burst, 1_280);
        assert_eq!(p17.find_node_max_in_flight, 32_768);
        assert_eq!(p17.peer_lookup_rate_per_sec, 64);
        assert_eq!(p17.pool_capacity, 200_000);

        assert!(crawl_profile_defaults("safe").is_some());
        assert!(crawl_profile_defaults("P0").is_none());
        assert!(crawl_profile_defaults("P18").is_none());
        assert_eq!(all_crawl_profiles().len(), 17);
    }

    #[test]
    fn config_validation_and_strict_deserialization() {
        let redis = RedisServiceConfig::default();
        assert_eq!(redis.effective_url(), "redis://127.0.0.1:6379");
        assert!(redis.validate().is_ok());

        let redis_auth = RedisServiceConfig {
            host: "10.0.0.9".to_string(),
            port: 6380,
            password: "demo".to_string(),
            db: 2,
            tls: true,
            ..RedisServiceConfig::default()
        };
        assert_eq!(redis_auth.effective_url(), "rediss://:demo@10.0.0.9:6380/2");
        assert!(redis_auth.validate().is_ok());

        let manticore = ManticoreServiceConfig::default();
        assert!(manticore.validate().is_ok());
        let half_auth = ManticoreServiceConfig {
            username: "admin".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert!(half_auth.validate().is_err());

        let wrong_table = ManticoreServiceConfig {
            table_name: "other_table".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert!(wrong_table.validate().is_err());

        let crawler = CrawlerWorkerConfig::default();
        assert!(crawler.validate().is_ok());

        let bad_crawler_json = serde_json::json!({
            "discovery": { "unknown_field": 123 }
        });
        assert!(serde_json::from_value::<CrawlerWorkerConfig>(bad_crawler_json).is_err());

        let indexer = IndexerWorkerConfig::default();
        assert!(indexer.validate().is_ok());
        assert_eq!(indexer.torrent_workers, 64);
        assert_eq!(indexer.batch_size, 1000);
    }

    #[test]
    fn bootstrap_parsing_and_bearer_verification() {
        let std_json = serde_json::json!({
            "instance_id": "crawler-1",
            "role": "crawler",
            "panel_ipc_port": 18080,
            "ipc_token": "tok-abc",
            "data_dir": "./data",
            "redis": RedisServiceConfig::default(),
            "manticore": null,
            "worker_config": {},
            "filter_bundle": null
        })
        .to_string();

        let parsed = parse_worker_bootstrap_str(&std_json, WorkerRole::Crawler, false).unwrap();
        assert_eq!(parsed.instance_id, "crawler-1");
        assert_eq!(parsed.panel_ipc_port, 18080);

        // 角色不匹配被拒绝
        assert!(parse_worker_bootstrap_str(&std_json, WorkerRole::Indexer, false).is_err());
        // 空输入被拒绝
        assert!(parse_worker_bootstrap_str("   ", WorkerRole::Crawler, false).is_err());

        // standalone 默认补齐
        let minimal_standalone = serde_json::json!({
            "redis": RedisServiceConfig::default()
        })
        .to_string();
        assert!(
            parse_worker_bootstrap_str(&minimal_standalone, WorkerRole::Crawler, false).is_err()
        );
        let standalone =
            parse_worker_bootstrap_str(&minimal_standalone, WorkerRole::Crawler, true).unwrap();
        assert_eq!(standalone.instance_id, "standalone-crawler");
        assert_eq!(standalone.panel_ipc_port, 0);

        // Bearer 校验
        assert!(verify_bearer_token(Some("Bearer secret-1"), "secret-1"));
        assert!(verify_bearer_token(Some("bearer   secret-1  "), "secret-1"));
        assert!(!verify_bearer_token(Some("Bearer wrong"), "secret-1"));
        assert!(!verify_bearer_token(Some("Basic secret-1"), "secret-1"));
        assert!(!verify_bearer_token(None, "secret-1"));
        assert!(!verify_bearer_token(Some("Bearer "), ""));
    }

    #[test]
    fn geoip_config_validation_and_path_resolution() {
        let cfg = GeoIpConfig::default();
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.normalized_home_country(), "CN");
        assert_eq!(cfg.normalized_prefix_cache_slots(), 65_536);
        assert_eq!(cfg.effective_download_url(), DEFAULT_GEOIP_DOWNLOAD_URL);
        assert!(DEFAULT_GEOIP_DOWNLOAD_URL.starts_with(DEFAULT_GEOIP_RELEASES_URL));

        // 兼容未持久化 download_url 的历史配置 JSON
        let legacy_geo: GeoIpConfig =
            serde_json::from_str(r#"{"enabled":true,"home_country":"jp"}"#).unwrap();
        assert_eq!(legacy_geo.normalized_home_country(), "JP");
        assert_eq!(
            legacy_geo.effective_download_url(),
            DEFAULT_GEOIP_DOWNLOAD_URL
        );

        let custom_url_cfg = GeoIpConfig {
            download_url: "https://mirror.example.com/GeoLite2-Country.mmdb".to_string(),
            ..GeoIpConfig::default()
        };
        assert!(custom_url_cfg.validate().is_ok());

        let bad_url_cfg = GeoIpConfig {
            download_url: "file:///etc/passwd".to_string(),
            ..GeoIpConfig::default()
        };
        assert!(bad_url_cfg.validate().is_err());

        let data_dir = std::path::Path::new("/tmp/hameta-data");
        assert_eq!(
            cfg.resolve_mmdb_path(data_dir),
            std::path::PathBuf::from("/tmp/hameta-data/GeoLite2-Country.mmdb")
        );

        let non_pow2 = GeoIpConfig {
            home_country: "us".to_string(),
            prefix_cache_slots: 50_000,
            ..GeoIpConfig::default()
        };
        assert_eq!(non_pow2.normalized_home_country(), "US");
        assert_eq!(non_pow2.normalized_prefix_cache_slots(), 65_536);

        let bad_country = GeoIpConfig {
            home_country: "CHN".to_string(),
            ..GeoIpConfig::default()
        };
        assert!(bad_country.validate().is_err());

        let bad_threshold = GeoIpConfig {
            adaptive_sample_threshold_pps: 100,
            ..GeoIpConfig::default()
        };
        assert!(bad_threshold.validate().is_err());
    }

    #[test]
    fn manticore_reverse_proxy_auth_modes_and_legacy_compat() {
        // 1. 兼容旧版未带 auth_mode 的持久化 JSON（含 username/password 时自动识别为 Basic）
        let legacy_json = r#"{
            "url": "https://proxy.example.com/manticore",
            "username": "ops_user",
            "password": "ops_password"
        }"#;
        let legacy_cfg: ManticoreServiceConfig = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy_cfg.effective_auth_mode(), ManticoreAuthMode::Basic);
        assert!(legacy_cfg.has_auth());
        assert_eq!(legacy_cfg.basic_auth(), Some(("ops_user", "ops_password")));
        assert_eq!(legacy_cfg.bearer_token(), None);
        assert_eq!(legacy_cfg.custom_header_auth(), None);
        assert!(legacy_cfg.validate().is_ok());

        let normalized = legacy_cfg.normalized();
        assert_eq!(normalized.auth_mode, ManticoreAuthMode::Basic);

        // 2. 显式 Basic Auth 校验（空字段 / 含冒号 / 含换行）
        let empty_basic = ManticoreServiceConfig {
            auth_mode: ManticoreAuthMode::Basic,
            ..ManticoreServiceConfig::default()
        };
        assert!(empty_basic.validate().is_err());

        let colon_user = ManticoreServiceConfig {
            auth_mode: ManticoreAuthMode::Basic,
            username: "bad:user".to_string(),
            password: "secret".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert!(colon_user.validate().is_err());

        // 3. Bearer Token（自动剥离 Bearer 前缀，防多字节切片 panic，拒绝空值与换行）
        let bearer_prefixed = ManticoreServiceConfig {
            auth_mode: ManticoreAuthMode::Bearer,
            username: "stale_user".to_string(),
            password: "stale_password".to_string(),
            bearer_token: "  Bearer   tok_live_123  ".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert_eq!(
            bearer_prefixed.effective_auth_mode(),
            ManticoreAuthMode::Bearer
        );
        assert_eq!(bearer_prefixed.basic_auth(), None);
        assert_eq!(bearer_prefixed.bearer_token(), Some("tok_live_123"));
        assert!(bearer_prefixed.validate().is_ok());

        let bad_bearer_utf8 = ManticoreServiceConfig {
            auth_mode: ManticoreAuthMode::Bearer,
            bearer_token: "中文令牌".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert!(bad_bearer_utf8.validate().is_err());

        let bad_bearer_crlf = ManticoreServiceConfig {
            auth_mode: ManticoreAuthMode::Bearer,
            bearer_token: "tok\r\nX-Injected: 1".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert!(bad_bearer_crlf.validate().is_err());

        // 4. 自定义请求头 API Key（如 X-API-Key / X-Auth-Token）
        let header_cfg = ManticoreServiceConfig {
            auth_mode: ManticoreAuthMode::Header,
            header_name: " X-API-Key ".to_string(),
            header_value: " secret_key_99 ".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert!(header_cfg.validate().is_ok());
        assert_eq!(
            header_cfg.custom_header_auth(),
            Some(("X-API-Key", "secret_key_99"))
        );

        let bad_header_name = ManticoreServiceConfig {
            auth_mode: ManticoreAuthMode::Header,
            header_name: "Invalid Header".to_string(),
            header_value: "secret".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert!(bad_header_name.validate().is_err());

        let reserved_header = ManticoreServiceConfig {
            auth_mode: ManticoreAuthMode::Header,
            header_name: "Host".to_string(),
            header_value: "evil.internal".to_string(),
            ..ManticoreServiceConfig::default()
        };
        assert!(reserved_header.validate().is_err());
    }
}
