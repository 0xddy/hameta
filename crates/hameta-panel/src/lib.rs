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
}
