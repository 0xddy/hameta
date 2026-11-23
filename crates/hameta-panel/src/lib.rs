pub mod api;
pub mod assets;
pub mod auth;
pub mod discovery;
pub mod ingest_hub;
pub mod metrics_collector;
pub mod nofile;
pub mod server;
pub mod services;
pub mod supervisor;
pub mod torrent_tasks;
pub mod tuning_advisor;

pub use auth::LoginRateLimiter;
pub use ingest_hub::{ConsoleIngestEvent, IngestHub, IngestHubStats};
pub use metrics_collector::{
    GlobeChannelRates, GlobeCountryTraffic, GlobeGeoIpStatus, GlobeTrafficSnapshot,
    MetricsCollector, OverviewSnapshot,
};
pub use server::{PanelConfig, PanelState, build_router, run_panel};
pub use services::{
    ManticoreTestReport, RedisTestReport, ServiceRuntimePool, test_manticore_connection,
    test_redis_connection,
};
pub use supervisor::{
    CrawlerGeoIpReloadResult, IndexerReloadResult, META_GEOIP_CONFIG_KEY, Supervisor,
    WorkerStatusView, load_geoip_config,
};
pub use torrent_tasks::TorrentTaskManager;
pub use tuning_advisor::TuningAdvisor;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use hameta_common::generate_token;
    use hameta_storage::Database;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    async fn test_state() -> (Arc<PanelState>, PathBuf) {
        let db = Database::open_in_memory().await.unwrap();
        let data_dir = std::env::temp_dir().join(format!("hameta-panel-test-{}", generate_token()));
        let supervisor = Supervisor::new(db.clone(), data_dir.clone()).await.unwrap();
        let services = Arc::new(ServiceRuntimePool::default());
        let metrics = MetricsCollector::new(db.clone(), supervisor.clone(), services.clone());
        let torrent_tasks = TorrentTaskManager::new(db.clone(), services.clone());
        let tuning_advisor = TuningAdvisor::new(db.clone(), supervisor.clone());
        let state = Arc::new(PanelState {
            db,
            config: PanelConfig {
                bind_addr: "127.0.0.1:0".to_string(),
                data_dir: data_dir.clone(),
                dev_mode: false,
            },
            supervisor,
            services,
            metrics,
            torrent_tasks,
            tuning_advisor,
            login_limiter: Arc::new(LoginRateLimiter::default()),
            setup_lock: Arc::new(Mutex::new(())),
        });
        (state, data_dir)
    }

    #[tokio::test]
    async fn healthz_returns_ok_and_initialization_flag() {
        let (state, dir) = test_state().await;
        let app = build_router(state.clone());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["initialized"], false);

        state.db.mark_initialized().await.unwrap();
        let resp2 = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body2 = to_bytes(resp2.into_body(), usize::MAX).await.unwrap();
        let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
        assert_eq!(json2["initialized"], true);

        state.supervisor.shutdown_all().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unknown_api_route_returns_404_json() {
        let (state, dir) = test_state().await;
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/not-found")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "NOT_FOUND");

        state.supervisor.shutdown_all().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn spa_serves_embedded_index_html() {
        let (state, dir) = test_state().await;
        let app = build_router(state.clone());
        for uri in ["/", "/some/spa/route"] {
            let resp = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "uri {uri}");
            let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
            let html = String::from_utf8(body.to_vec()).unwrap();
            assert!(html.contains("<div id=\"app\"></div>"), "uri {uri}");
        }

        state.supervisor.shutdown_all().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tuning_advisor_http_status_and_apply_e2e() {
        use crate::tuning_advisor::CrawlerMetricSample;
        use hameta_common::{CrawlerWorkerConfig, WorkerRole, hash_password, hash_token};

        let (state, dir) = test_state().await;
        state.db.mark_initialized().await.unwrap();
        let pw = hash_password("admin-secret-123").unwrap();
        let admin = state.db.create_admin("admin", &pw).await.unwrap();
        let raw_tok = generate_token();
        state
            .db
            .create_session(&hash_token(&raw_tok), admin.id, "127.0.0.1", "test", 3600)
            .await
            .unwrap();
        let cookie = format!("hameta_session={raw_tok}");

        let mut cfg = CrawlerWorkerConfig {
            crawl_profile: "P17".to_string(),
            ..Default::default()
        };
        cfg.metadata.max_worker_count = 2_000;
        cfg.metadata.timeout_secs = 6;
        cfg.crawl.rate_limit.max_find_node_rate_per_sec = Some(200_000);
        cfg.crawl.rate_limit.metadata_pressure_floor_percent = Some(5);
        let cfg_json = serde_json::to_string(&cfg).unwrap();
        state
            .db
            .create_worker(
                "crawler-1",
                WorkerRole::Crawler,
                "Crawler #1",
                &cfg_json,
                "admin",
            )
            .await
            .unwrap();

        state
            .tuning_advisor
            .inject_rolling_samples(
                "crawler-1",
                vec![
                    CrawlerMetricSample {
                        captured_at: 1_700_000_000,
                        uptime_secs: 100,
                        config_version: 1,
                        effective_metadata_workers: 2_000,
                        metadata_queue_depth: 9_800,
                        metadata_queue_max: 10_000,
                        metadata_in_flight: 1_990,
                        find_node_effective_rate_per_sec: 10_000,
                        queries_new: 10_000,
                        ..Default::default()
                    },
                    CrawlerMetricSample {
                        captured_at: 1_700_003_600,
                        uptime_secs: 3_700,
                        config_version: 1,
                        effective_metadata_workers: 2_000,
                        metadata_queue_depth: 9_900,
                        metadata_queue_max: 10_000,
                        metadata_in_flight: 1_995,
                        find_node_effective_rate_per_sec: 10_000,
                        queries_new: 36_010_000,
                        ..Default::default()
                    },
                ],
            )
            .await;

        let app = build_router(state.clone());
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/workers/crawler-1/tuning/status?source=ring&up_mbps=500&down_mbps=500")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let overview: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let recs = overview["report"]["recommendations"].as_array().unwrap();
        assert!(!recs.is_empty());

        let ids: Vec<String> = recs
            .iter()
            .filter(|r| r["apply_supported"] == true)
            .filter_map(|r| r["id"].as_str().map(ToString::to_string))
            .collect();
        assert!(!ids.is_empty());

        let apply_body = serde_json::json!({
            "recommendation_ids": ids,
            "source": "ring",
            "up_mbps": 500,
            "down_mbps": 500,
            "apply_restart": false
        });
        let resp2 = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/workers/crawler-1/tuning/apply")
                    .header("cookie", &cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(apply_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let applied_json: serde_json::Value =
            serde_json::from_slice(&to_bytes(resp2.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(applied_json["version"], 2);
        assert!(
            applied_json["config"]["crawl"]["rate_limit"]["max_find_node_rate_per_sec"]
                .as_u64()
                .unwrap()
                < 20_000
        );

        state.supervisor.shutdown_all().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
