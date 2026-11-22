pub mod db;
pub mod error;
pub mod repos;

pub use db::Database;
pub use error::StorageError;
pub use repos::admin::{AdminSessionRow, AdminUserRow};
pub use repos::audit::AuditLogRow;
pub use repos::filter::FilterRuleVersionRow;
pub use repos::service_config::ServiceConfigRow;
pub use repos::worker::{WorkerConfigVersionRow, WorkerInstanceRow};

#[cfg(test)]
mod tests {
    use super::*;
    use hameta_common::{
        DesiredState, MetricsHourPoint, MetricsMinutePoint, ServiceKind, WorkerRole, hash_password,
        hash_token, verify_password,
    };

    #[tokio::test]
    async fn file_db_enables_wal_and_creates_all_tables() {
        let dir = std::env::temp_dir().join(format!(
            "hameta-storage-wal-{}",
            hameta_common::generate_token()
        ));
        let db_path = dir.join("hameta.db");
        let db = Database::open(&db_path).await.unwrap();

        let (journal_mode,): (String,) = sqlx::query_as("PRAGMA journal_mode;")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

        let (table_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '_sqlx_%' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(table_count, 10);

        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn meta_and_admin_reset_clears_sessions_and_logs_audit() {
        let db = Database::open_in_memory().await.unwrap();
        assert!(!db.is_initialized().await.unwrap());
        db.mark_initialized().await.unwrap();
        assert!(db.is_initialized().await.unwrap());

        let old_hash = hash_password("old_pass").unwrap();
        let admin = db.create_admin("old_admin", &old_hash).await.unwrap();
        let tok_hash = hash_token("session-token-1");
        db.create_session(&tok_hash, admin.id, "127.0.0.1", "test-agent", 3600)
            .await
            .unwrap();
        assert!(db.find_valid_session(&tok_hash).await.unwrap().is_some());

        let new_hash = hash_password("new_pass").unwrap();
        let reset = db
            .reset_admin_credentials("new_admin", &new_hash, "cli")
            .await
            .unwrap();
        assert_eq!(reset.username, "new_admin");
        assert!(verify_password("new_pass", &reset.password_hash));
        assert!(!verify_password("old_pass", &reset.password_hash));

        // 重置后所有旧会话必须失效
        assert!(db.find_valid_session(&tok_hash).await.unwrap().is_none());

        let audits = db.list_audits(10).await.unwrap();
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].actor, "cli");
        assert_eq!(audits[0].action, "admin.reset");
        assert_eq!(audits[0].target, "new_admin");

        // 校验 find_admin_by_id、update_admin_password 与 touch_session
        let found = db.find_admin_by_id(reset.id).await.unwrap().unwrap();
        assert_eq!(found.username, "new_admin");
        let rotated_hash = hash_password("rotated_pass").unwrap();
        db.update_admin_password(reset.id, &rotated_hash)
            .await
            .unwrap();
        let after = db.find_admin_by_id(reset.id).await.unwrap().unwrap();
        assert!(verify_password("rotated_pass", &after.password_hash));

        let tok2 = hash_token("session-token-2");
        db.create_session(&tok2, reset.id, "127.0.0.1", "ua", 3600)
            .await
            .unwrap();
        db.touch_session(&tok2).await.unwrap();
        assert!(db.find_valid_session(&tok2).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn service_config_versions_and_rollback() {
        let db = Database::open_in_memory().await.unwrap();
        let v1 = db
            .save_service_config(
                ServiceKind::Redis,
                r#"{"host":"127.0.0.1","port":6379}"#,
                "admin",
                "init",
            )
            .await
            .unwrap();
        assert_eq!(v1.version, 1);
        assert_eq!(v1.is_active, 1);

        let v2 = db
            .save_service_config(
                ServiceKind::Redis,
                r#"{"host":"10.0.0.2","port":6380}"#,
                "admin",
                "update",
            )
            .await
            .unwrap();
        assert_eq!(v2.version, 2);

        db.update_service_test_result(ServiceKind::Redis, 2, true, "PONG 1ms")
            .await
            .unwrap();
        let active = db
            .get_active_service_config(ServiceKind::Redis)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.version, 2);
        assert_eq!(active.last_test_ok, Some(1));

        let v3 = db
            .rollback_service_config(ServiceKind::Redis, 1, "admin")
            .await
            .unwrap();
        assert_eq!(v3.version, 3);
        assert!(v3.config_json.contains("127.0.0.1"));
        assert_eq!(
            db.list_service_config_versions(ServiceKind::Redis, 10)
                .await
                .unwrap()
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn worker_crud_config_versions_and_rollback() {
        let db = Database::open_in_memory().await.unwrap();
        let w = db
            .create_worker(
                "crawler-1",
                WorkerRole::Crawler,
                "主采集节点",
                r#"{"port":12313,"crawl_profile":"P1"}"#,
                "admin",
            )
            .await
            .unwrap();
        assert_eq!(w.active_config_version, 1);

        db.set_worker_desired_state("crawler-1", DesiredState::Running)
            .await
            .unwrap();
        let v2 = db
            .save_worker_config_version(
                "crawler-1",
                r#"{"port":12313,"crawl_profile":"P5"}"#,
                "admin",
                "升档 P5",
            )
            .await
            .unwrap();
        assert_eq!(v2.version, 2);

        let v3 = db
            .rollback_worker_config("crawler-1", 1, "admin")
            .await
            .unwrap();
        assert_eq!(v3.version, 3);
        assert!(v3.config_json.contains("\"P1\""));

        let active = db
            .get_active_worker_config("crawler-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.version, 3);

        db.delete_worker("crawler-1").await.unwrap();
        assert!(db.list_workers().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn filter_draft_publish_and_rollback() {
        let db = Database::open_in_memory().await.unwrap();
        let d1 = db
            .save_filter_draft(r#"{"blocked_rules":["a"]}"#, "[]", "[]", "admin", "draft 1")
            .await
            .unwrap();
        assert_eq!(d1.version, 1);
        assert_eq!(d1.status, "draft");

        // 草稿覆盖更新不消耗新版本号
        let d1_updated = db
            .save_filter_draft(
                r#"{"blocked_rules":["a","b"]}"#,
                "[\"ad\"]",
                "[]",
                "admin",
                "draft 1b",
            )
            .await
            .unwrap();
        assert_eq!(d1_updated.version, 1);

        let p1 = db
            .publish_filter_draft("admin", Some("发布首版"))
            .await
            .unwrap();
        assert_eq!(p1.version, 1);
        assert_eq!(p1.status, "published");

        let _d2 = db
            .save_filter_draft(r#"{"blocked_rules":["c"]}"#, "[]", "[]", "admin", "draft 2")
            .await
            .unwrap();
        let p2 = db.publish_filter_draft("admin", None).await.unwrap();
        assert_eq!(p2.version, 2);

        let p3 = db.rollback_filter_version(1, "admin").await.unwrap();
        assert_eq!(p3.version, 3);
        assert_eq!(p3.status, "published");
        assert!(p3.filter_json.contains("\"b\""));

        let versions = db.list_filter_versions(10).await.unwrap();
        assert_eq!(versions.len(), 3);
    }

    #[tokio::test]
    async fn metrics_upsert_query_and_prune() {
        let db = Database::open_in_memory().await.unwrap();
        db.upsert_metrics_minute(&MetricsMinutePoint {
            ts_minute: 1_000,
            hashes_per_min: 100.0,
            metadata_succeeded_per_min: 40.0,
            stream_pushed_per_min: 39.0,
            filtered_per_min: 10.0,
            indexed_written_per_min: 29.0,
            hot_updated_per_min: 5.0,
            actual_qps: 880.0,
            stream_new_len: 12,
            stream_hot_len: 3,
            stream_new_lag: 0,
            udp_rx_bps: 1024.0,
            udp_tx_bps: 2048.0,
            metadata_rx_bps: 4096.0,
            total_rss_bytes: 50_000_000,
            avg_cpu_percent: 12.5,
        })
        .await
        .unwrap();

        db.upsert_metrics_hour(&MetricsHourPoint {
            ts_hour: 3_600,
            hashes_total: 6_000,
            metadata_succeeded_total: 2_400,
            stream_pushed_total: 2_340,
            filtered_total: 600,
            indexed_written_total: 1_740,
            hot_updated_total: 300,
            avg_qps: 880.0,
            max_stream_new_lag: 20,
            avg_rss_bytes: 50_000_000,
        })
        .await
        .unwrap();

        assert_eq!(db.query_metrics_minute(0, 100).await.unwrap().len(), 1);
        assert_eq!(db.query_metrics_hour(0, 100).await.unwrap().len(), 1);

        let (deleted_min, deleted_hour) = db.prune_metrics(2_000, 4_000).await.unwrap();
        assert_eq!(deleted_min, 1);
        assert_eq!(deleted_hour, 1);
    }
}
