pub mod auth;
pub mod console;
pub mod error;
pub mod filter;
pub mod globe;
pub mod overview;
pub mod redis_monitor;
pub mod settings;
pub mod setup;
pub mod sql_console;
pub mod torrents;
pub mod workers;

use crate::server::PanelState;
use axum::Router;
use axum::routing::{delete, get, patch, post, put};
use std::sync::Arc;

pub fn api_v1_router() -> Router<Arc<PanelState>> {
    Router::new()
        // Setup Wizard
        .route("/setup/status", get(setup::setup_status_handler))
        .route(
            "/setup/detect-services",
            get(setup::setup_detect_services_handler),
        )
        .route("/setup/test-redis", post(setup::setup_test_redis_handler))
        .route(
            "/setup/test-manticore",
            post(setup::setup_test_manticore_handler),
        )
        .route(
            "/setup/init-manticore-table",
            post(setup::setup_init_manticore_table_handler),
        )
        .route("/setup/init", post(setup::setup_init_handler))
        // Auth
        .route("/auth/login", post(auth::login_handler))
        .route("/auth/logout", post(auth::logout_handler))
        .route("/auth/me", get(auth::me_handler))
        .route("/auth/password", post(auth::change_password_handler))
        // Service Settings
        .route("/settings/services", get(settings::get_services_handler))
        .route(
            "/settings/detect-services",
            get(settings::detect_services_handler),
        )
        .route("/settings/redis/test", post(settings::test_redis_handler))
        .route("/settings/redis", put(settings::update_redis_handler))
        .route(
            "/settings/manticore/test",
            post(settings::test_manticore_handler),
        )
        .route(
            "/settings/manticore/init-table",
            post(settings::init_manticore_table_handler),
        )
        .route(
            "/settings/manticore",
            put(settings::update_manticore_handler),
        )
        .route("/settings/history", get(settings::settings_history_handler))
        .route(
            "/settings/rollback",
            post(settings::settings_rollback_handler),
        )
        // Overview & Metrics
        .route(
            "/overview/snapshot",
            get(overview::overview_snapshot_handler),
        )
        .route("/overview/history", get(overview::overview_history_handler))
        .route("/overview/stream", get(overview::overview_stream_handler))
        // Console (SSE Ingest Stream) & Global DHT Traffic Radar
        .route("/console/recent", get(console::console_recent_handler))
        .route(
            "/console/ingest-stream",
            get(console::console_ingest_stream_handler),
        )
        .route("/globe/snapshot", get(globe::globe_snapshot_handler))
        .route("/globe/stream", get(globe::globe_stream_handler))
        .route(
            "/settings/geoip",
            get(settings::get_geoip_settings_handler).put(settings::update_geoip_settings_handler),
        )
        .route(
            "/settings/geoip/reload",
            post(settings::reload_geoip_settings_handler),
        )
        .route(
            "/settings/geoip/download",
            post(settings::download_geoip_mmdb_handler),
        )
        .route(
            "/settings/nofile",
            get(settings::get_nofile_settings_handler)
                .put(settings::update_nofile_settings_handler),
        )
        // Workers & DHT Profiles
        .route("/dht/profiles", get(workers::get_dht_profiles_handler))
        .route("/workers", get(workers::list_workers_handler))
        .route("/workers", post(workers::create_worker_handler))
        .route("/workers/{id}", delete(workers::delete_worker_handler))
        .route("/workers/{id}/start", post(workers::start_worker_handler))
        .route("/workers/{id}/stop", post(workers::stop_worker_handler))
        .route(
            "/workers/{id}/restart",
            post(workers::restart_worker_handler),
        )
        .route(
            "/workers/{id}/policy",
            put(workers::update_worker_policy_handler),
        )
        .route(
            "/workers/{id}/config",
            get(workers::get_worker_config_handler),
        )
        .route(
            "/workers/{id}/config",
            put(workers::update_worker_config_handler),
        )
        .route(
            "/workers/{id}/config/versions",
            get(workers::list_worker_config_versions_handler),
        )
        .route(
            "/workers/{id}/config/rollback",
            post(workers::rollback_worker_config_handler),
        )
        .route(
            "/workers/{id}/diagnostics",
            get(workers::get_worker_diagnostics_handler),
        )
        // Redis Streams Monitor & Hash Inspector
        .route(
            "/redis/overview",
            get(redis_monitor::redis_overview_handler),
        )
        .route(
            "/redis/streams/{name}/preview",
            get(redis_monitor::redis_stream_preview_handler),
        )
        .route(
            "/redis/streams/{name}/trim",
            post(redis_monitor::redis_stream_trim_handler),
        )
        .route(
            "/redis/hash/{info_hash}",
            get(redis_monitor::redis_hash_inspect_handler),
        )
        .route(
            "/redis/hash/{info_hash}/locks",
            delete(redis_monitor::redis_hash_clear_locks_handler),
        )
        // Read-Only SQL Console
        .route(
            "/sql/query",
            post(sql_console::execute_readonly_sql_handler),
        )
        // Filter & Cleaning Workbench
        .route("/filter/state", get(filter::get_filter_state_handler))
        .route("/filter/presets", get(filter::list_filter_presets_handler))
        .route("/filter/draft", put(filter::save_filter_draft_handler))
        .route(
            "/filter/draft",
            delete(filter::discard_filter_draft_handler),
        )
        .route("/filter/validate", post(filter::validate_filter_handler))
        .route(
            "/filter/import-legacy",
            post(filter::import_legacy_filter_handler),
        )
        .route(
            "/filter/reset-preset",
            post(filter::reset_filter_preset_handler),
        )
        .route("/filter/publish", post(filter::publish_filter_handler))
        .route(
            "/filter/versions",
            get(filter::list_filter_versions_handler),
        )
        .route("/filter/rollback", post(filter::rollback_filter_handler))
        .route("/filter/stats", get(filter::get_filter_stats_handler))
        .route(
            "/filter/discover-candidates",
            post(filter::discover_ad_candidates_handler),
        )
        .route(
            "/filter/playground/test",
            post(filter::playground_test_handler),
        )
        .route(
            "/filter/playground/sample",
            post(filter::playground_sample_handler),
        )
        // Torrent Explorer
        .route("/torrents", get(torrents::list_torrents_handler))
        .route("/torrents", post(torrents::create_torrent_handler))
        .route(
            "/torrents/batch-delete",
            post(torrents::batch_delete_torrents_handler),
        )
        .route("/torrents/{id}", get(torrents::get_torrent_detail_handler))
        .route("/torrents/{id}", patch(torrents::update_torrent_handler))
        .route("/torrents/{id}", delete(torrents::delete_torrent_handler))
}
