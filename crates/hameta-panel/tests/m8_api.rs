use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router as AxumRouter};
use hameta_common::{
    CrawlerWorkerConfig, IndexerWorkerConfig, ManticoreServiceConfig, RedisServiceConfig,
    ServiceKind, WorkerBootstrap, WorkerDrainStatus, WorkerReadyRequest, WorkerRegisterRequest,
    WorkerRole, WorkerRuntimeState, WorkerShutdownRequest, WorkerSnapshotEnvelope, generate_token,
    hash_password, hash_token, verify_bearer_token,
};
use hameta_filter::default_online_filter_bundle;
use hameta_panel::supervisor::SpawnSpec;
use hameta_panel::{
    LoginRateLimiter, MetricsCollector, PanelConfig, PanelState, ServiceRuntimePool, Supervisor,
    build_router,
};
use hameta_storage::Database;
use serde_json::{Value, json};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Clone)]
struct MockM8Crawler {
    bootstrap: WorkerBootstrap,
    draining: Arc<AtomicBool>,
    shutdown: CancellationToken,
}

fn mock_auth(headers: &HeaderMap, expected: &str) -> bool {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    verify_bearer_token(auth, expected)
}

async fn mock_snapshot(State(st): State<Arc<MockM8Crawler>>, headers: HeaderMap) -> Response {
    if !mock_auth(&headers, &st.bootstrap.ipc_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(WorkerSnapshotEnvelope {
        instance_id: st.bootstrap.instance_id.clone(),
        role: WorkerRole::Crawler,
        captured_at_unix: 1_700_000_123,
        payload: json!({
            "uptime_secs": 120,
            "crawl_profile": "P1",
            "effective_metadata_workers": 256,
            "local_cache_entries": 42_000,
            "redis_pool_size": 64,
            "redis_pool_available": 62,
            "rss_bytes": 67_108_864,
            "app": {
                "early_filter_passed": 500,
                "early_filter_blocked": 120,
                "torrents_downloaded": 380,
                "torrents_pushed": 375,
                "hot_pushed": 40
            },
            "dht": {
                "hashes_received": 9_500,
                "metadata_queue_depth": 120,
                "metadata_queue_max": 10_000,
                "metadata_in_flight": 48,
                "find_node_in_flight": 800,
                "find_node_in_flight_max": 4_498,
                "find_node_effective_rate_per_sec": 878,
                "queries_new": 45_000,
                "queries_revisit": 12_000,
                "queries_bootstrap": 300,
                "udp_queue_full": 2,
                "udp_responses_rate_limited": 14
            },
            "observability": {
                "udp_rx_packets": 95_000,
                "udp_rx_bytes": 18_432_000,
                "udp_tx_packets": 88_000,
                "udp_tx_bytes": 9_216_000,
                "metadata_bytes_downloaded": 104_857_600,
                "metadata_timeout_stages": {
                    "connect_handshake": 4,
                    "tcp_connect": 11,
                    "base_handshake": 3,
                    "extension_handshake_send": 1,
                    "extension_handshake_receive": 7,
                    "piece_request_send": 2,
                    "piece_receive": 18,
                    "validation": 0
                },
                "metadata_connection_errors": {
                    "refused": 22,
                    "reset": 15,
                    "aborted": 1,
                    "closed": 9,
                    "timeout": 45,
                    "invalid_handshake": 6,
                    "info_hash_mismatch": 2,
                    "io_other": 3,
                    "other": 1
                },
                "queue_wait_ms": {
                    "bounds": [10, 50, 200, 1000],
                    "counts": [210, 120, 40, 8],
                    "overflow": 2,
                    "count": 380,
                    "sum": 25_000,
                    "p50": 45,
                    "p95": 420
                },
                "fetch_duration_ms": {
                    "bounds": [100, 500, 1500, 4000],
                    "counts": [80, 190, 90, 20],
                    "overflow": 0,
                    "count": 380,
                    "sum": 310_000,
                    "p50": 380,
                    "p95": 1450
                },
                "metadata_size_bytes": {
                    "bounds": [4096, 32768, 131072, 1048576],
                    "counts": [40, 210, 110, 18],
                    "overflow": 2,
                    "count": 380,
                    "sum": 18_000_000,
                    "p50": 24_576,
                    "p95": 110_000
                }
            },
            "yield_counters": {
                "announce_peer": 120,
                "get_peers": 840,
                "sample_infohashes": 310
            },
            "yield_gauges": {
                "active_nodes": 5_120
            }
        }),
    })
    .into_response()
}

async fn mock_drain(State(st): State<Arc<MockM8Crawler>>, headers: HeaderMap) -> Response {
    if !mock_auth(&headers, &st.bootstrap.ipc_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(WorkerDrainStatus {
        draining: st.draining.load(Ordering::Relaxed),
        queued_items: 0,
        in_flight_items: 0,
        completed: true,
    })
    .into_response()
}

async fn mock_shut(
    State(st): State<Arc<MockM8Crawler>>,
    headers: HeaderMap,
    Json(_req): Json<WorkerShutdownRequest>,
) -> Response {
    if !mock_auth(&headers, &st.bootstrap.ipc_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    st.draining.store(true, Ordering::Relaxed);
    st.shutdown.cancel();
    Json(json!({"accepted": true})).into_response()
}

#[tokio::test]
async fn mock_m8_crawler_child() {
    let Ok(flag) = std::env::var("HAMETA_M8_MOCK_CRAWLER") else {
        return;
    };
    if flag != "1" {
        return;
    }

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).unwrap();
    let bootstrap: WorkerBootstrap = serde_json::from_str(&raw).unwrap();

    let shutdown = CancellationToken::new();
    let st = Arc::new(MockM8Crawler {
        bootstrap: bootstrap.clone(),
        draining: Arc::new(AtomicBool::new(false)),
        shutdown: shutdown.clone(),
    });
    let app = AxumRouter::new()
        .route("/internal/v1/snapshot", get(mock_snapshot))
        .route("/internal/v1/drain-status", get(mock_drain))
        .route("/internal/v1/shutdown", post(mock_shut))
        .with_state(st);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let worker_port = listener.local_addr().unwrap().port();
    let c = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { c.cancelled().await })
            .await;
    });

    let client = reqwest::Client::new();
    let reg_url = format!(
        "http://127.0.0.1:{}/internal/v1/register",
        bootstrap.panel_ipc_port
    );
    client
        .post(&reg_url)
        .bearer_auth(&bootstrap.ipc_token)
        .json(&WorkerRegisterRequest {
            instance_id: bootstrap.instance_id.clone(),
            worker_port,
            pid: std::process::id(),
            ipc_token: bootstrap.ipc_token.clone(),
        })
        .send()
        .await
        .unwrap();

    let ready_url = format!(
        "http://127.0.0.1:{}/internal/v1/ready",
        bootstrap.panel_ipc_port
    );
    client
        .post(&ready_url)
        .bearer_auth(&bootstrap.ipc_token)
        .json(&WorkerReadyRequest {
            instance_id: bootstrap.instance_id.clone(),
            rule_version: None,
        })
        .send()
        .await
        .unwrap();

    shutdown.cancelled().await;
    std::process::exit(0);
}

struct TestHarness {
    app: axum::Router,
    state: Arc<PanelState>,
    cookie: String,
    dir: PathBuf,
}

async fn setup_test_panel() -> TestHarness {
    let dir = std::env::temp_dir().join(format!("hameta-m8-api-{}", generate_token()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(&dir.join("hameta.db")).await.unwrap();

    let pw_hash = hash_password("secret-pass-123").unwrap();
    let admin = db.create_admin("admin", &pw_hash).await.unwrap();
    let session_tok = format!("sess-{}", generate_token());
    db.create_session(
        &hash_token(&session_tok),
        admin.id,
        "127.0.0.1",
        "m8-test",
        3600,
    )
    .await
    .unwrap();

    let redis_cfg = RedisServiceConfig::default();
    let redis_json = serde_json::to_string(&redis_cfg).unwrap();
    db.save_service_config(ServiceKind::Redis, &redis_json, "admin", "init")
        .await
        .unwrap();
    let manticore_json = serde_json::to_string(&ManticoreServiceConfig::default()).unwrap();
    db.save_service_config(ServiceKind::Manticore, &manticore_json, "admin", "init")
        .await
        .unwrap();

    let bundle = default_online_filter_bundle();
    db.save_filter_draft(
        &serde_json::to_string(&bundle.policy).unwrap(),
        &serde_json::to_string(&bundle.ad_phrases).unwrap(),
        &serde_json::to_string(&bundle.ad_regexes).unwrap(),
        "admin",
        "init",
    )
    .await
    .unwrap();
    db.publish_filter_draft("admin", Some("init"))
        .await
        .unwrap();

    let crawler_json = serde_json::to_string(&CrawlerWorkerConfig::default()).unwrap();
    db.create_worker(
        "crawler-1",
        WorkerRole::Crawler,
        "主采集节点",
        &crawler_json,
        "admin",
    )
    .await
    .unwrap();
    let indexer_json = serde_json::to_string(&IndexerWorkerConfig::default()).unwrap();
    db.create_worker(
        "indexer-1",
        WorkerRole::Indexer,
        "主入库节点",
        &indexer_json,
        "admin",
    )
    .await
    .unwrap();
    db.mark_initialized().await.unwrap();

    let services = Arc::new(ServiceRuntimePool::default());
    services.reload_redis(&redis_cfg).await.unwrap();

    let exe = std::env::current_exe().unwrap();
    let factory = Arc::new(move |_bootstrap: &WorkerBootstrap| {
        Ok(SpawnSpec {
            program: exe.clone(),
            args: vec![
                "--exact".to_string(),
                "mock_m8_crawler_child".to_string(),
                "--nocapture".to_string(),
            ],
            envs: vec![("HAMETA_M8_MOCK_CRAWLER".to_string(), "1".to_string())],
        })
    });
    let supervisor = Supervisor::with_command_factory(db.clone(), dir.clone(), factory)
        .await
        .unwrap();
    let metrics = MetricsCollector::new(db.clone(), supervisor.clone(), services.clone());
    let torrent_tasks = hameta_panel::TorrentTaskManager::new(db.clone(), services.clone());
    let tuning_advisor = hameta_panel::TuningAdvisor::new(db.clone(), supervisor.clone());

    let state = Arc::new(PanelState {
        config: PanelConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: dir.clone(),
            dev_mode: false,
        },
        db,
        services,
        supervisor,
        metrics,
        torrent_tasks,
        tuning_advisor,
        login_limiter: Arc::new(LoginRateLimiter::default()),
        setup_lock: Arc::new(Mutex::new(())),
    });
    let app = build_router(state.clone());

    TestHarness {
        app,
        state,
        cookie: format!("hameta_session={session_tok}"),
        dir,
    }
}

async fn json_req(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(COOKIE, cookie);
    let req_body = match body {
        Some(v) => {
            builder = builder.header(CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app
        .clone()
        .oneshot(builder.body(req_body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 2 * 1024 * 1024).await.unwrap();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

#[tokio::test]
async fn m8_worker_diagnostics_exposes_crawler_observability_and_profile_defaults() {
    let h = setup_test_panel().await;

    let (st, _) = json_req(
        &h.app,
        "POST",
        "/api/v1/workers/crawler-1/start",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let statuses = h.state.supervisor.list_worker_statuses().await.unwrap();
        if let Some(w) = statuses.into_iter().find(|w| w.id == "crawler-1")
            && w.runtime_state == WorkerRuntimeState::Running
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "crawler-1 failed to reach Running"
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    let (st, diag) = json_req(
        &h.app,
        "GET",
        "/api/v1/workers/crawler-1/diagnostics",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "diag={diag}");
    assert_eq!(diag["id"], "crawler-1");
    assert_eq!(diag["role"], "crawler");
    assert_eq!(diag["runtime_state"], "running");
    assert_eq!(
        diag["snapshot"]["observability"]["metadata_timeout_stages"]["piece_receive"],
        18
    );
    assert_eq!(
        diag["snapshot"]["observability"]["metadata_connection_errors"]["timeout"],
        45
    );
    assert_eq!(
        diag["snapshot"]["observability"]["queue_wait_ms"]["p95"],
        420
    );
    assert_eq!(diag["snapshot"]["yield_counters"]["sample_infohashes"], 310);
    assert_eq!(diag["profile_defaults"]["find_node_rate_per_sec"], 878);

    let (st, _) = json_req(
        &h.app,
        "GET",
        "/api/v1/workers/does-not-exist/diagnostics",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    h.state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn m8_sql_console_allows_ddl_and_blocks_multi_statement_and_excessive_limit() {
    let h = setup_test_panel().await;

    let invalid = [
        "SELECT * FROM torrents; SELECT 1",
        "SELECT * FROM torrents LIMIT 5000",
        "   ",
    ];

    for sql in invalid {
        let (st, body) = json_req(
            &h.app,
            "POST",
            "/api/v1/sql/query",
            &h.cookie,
            Some(json!({ "sql": sql })),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "SQL `{sql}` should fail: {body}"
        );
        assert_eq!(body["code"], "INVALID_CONFIG", "body={body}");
    }

    // ALTER / DDL / DML 不再被只读校验拦截（应放行至 Manticore 执行层）
    let (st, body) = json_req(
        &h.app,
        "POST",
        "/api/v1/sql/query",
        &h.cookie,
        Some(json!({ "sql": "ALTER TABLE torrents ADD COLUMN total_size bigint;" })),
    )
    .await;
    assert_ne!(
        body["code"], "INVALID_CONFIG",
        "ALTER TABLE 不应再被只读校验拦截: status={st}, body={body}"
    );

    h.state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn m8_redis_routes_enforce_stream_whitelist_and_hash_validation() {
    let h = setup_test_panel().await;

    let (st, err_prev) = json_req(
        &h.app,
        "GET",
        "/api/v1/redis/streams/arbitrary_key/preview",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err_prev["code"], "INVALID_STREAM");

    let (st, err_trim) = json_req(
        &h.app,
        "POST",
        "/api/v1/redis/streams/other_stream/trim",
        &h.cookie,
        Some(json!({
            "max_len": 1000,
            "approximate": true,
            "confirm_stream_name": "other_stream"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err_trim["code"], "INVALID_STREAM");

    let (st, err_hash) = json_req(
        &h.app,
        "GET",
        "/api/v1/redis/hash/not-a-valid-hash",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err_hash["code"], "INVALID_CONFIG");

    let (st, err_del) = json_req(
        &h.app,
        "DELETE",
        "/api/v1/redis/hash/too_short/locks",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err_del["code"], "INVALID_CONFIG");

    h.state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn m8_torrent_async_tasks_backfill_total_size_and_reclean_titles_end_to_end() {
    let h = setup_test_panel().await;

    // 启动内存 Mock Manticore 服务，模拟 3 条存量记录（含 63 位 ID、0 体积与脏广告标题）
    #[derive(Clone)]
    struct MockDoc {
        id: i64,
        info_hash: String,
        raw_name: String,
        refine_name: String,
        file_count: i32,
        total_size: i64,
        file_info: String,
        hot: i32,
        created_at: i64,
        updated_at: i64,
    }

    let docs = Arc::new(std::sync::Mutex::new(vec![
        MockDoc {
            id: 1_745_280_108_080_464_101,
            info_hash: "1111111111111111111111111111111111111111".to_string(),
            raw_name: "www.ad99.com - 流浪地球2 收藏不迷路 4K".to_string(),
            refine_name: "www.ad99.com - 流浪地球2 收藏不迷路 4K".to_string(),
            file_count: 2,
            total_size: 0,
            file_info:
                r#"[{"path":"movie.mkv","size":3000000000},{"path":"sub.srt","size":12345}]"#
                    .to_string(),
            hot: 5,
            created_at: 1_700_000_001,
            updated_at: 1_700_000_001,
        },
        MockDoc {
            id: 1_745_280_108_080_464_202,
            info_hash: "2222222222222222222222222222222222222222".to_string(),
            raw_name: "Ubuntu.24.04.LTS".to_string(),
            refine_name: "Ubuntu.24.04.LTS".to_string(),
            file_count: 1,
            total_size: 2_000_000,
            file_info: r#"[{"path":"ubuntu.bin","size":2000000}]"#.to_string(),
            hot: 2,
            created_at: 1_700_000_002,
            updated_at: 1_700_000_002,
        },
        MockDoc {
            id: 1_745_280_108_080_464_303,
            info_hash: "3333333333333333333333333333333333333333".to_string(),
            raw_name: "【最新地址发布页】阿凡达2.2022.1080p".to_string(),
            refine_name: "".to_string(),
            file_count: 1,
            total_size: 0,
            file_info: r#"[{"path":"avatar2.mkv","size":500000000}]"#.to_string(),
            hot: 1,
            created_at: 1_700_000_003,
            updated_at: 1_700_000_003,
        },
    ]));

    let sql_docs = docs.clone();
    let search_docs = docs.clone();
    let bulk_docs = docs.clone();

    let mock_manticore = AxumRouter::new()
        .route(
            "/sql",
            post(move |body: String| {
                let store = sql_docs.clone();
                async move {
                    let upper = body.to_ascii_uppercase();
                    if upper.contains("DESCRIBE") {
                        Json(json!([{
                            "data": [
                                {"Field": "id", "Type": "bigint", "Properties": ""},
                                {"Field": "info_hash", "Type": "string", "Properties": ""},
                                {"Field": "raw_name", "Type": "text", "Properties": ""},
                                {"Field": "refine_name", "Type": "text", "Properties": ""},
                                {"Field": "file_count", "Type": "uint", "Properties": ""},
                                {"Field": "total_size", "Type": "bigint", "Properties": ""},
                                {"Field": "file_info", "Type": "json", "Properties": ""},
                                {"Field": "hot", "Type": "uint", "Properties": ""},
                                {"Field": "created_at", "Type": "timestamp", "Properties": ""},
                                {"Field": "updated_at", "Type": "timestamp", "Properties": ""}
                            ],
                            "error": ""
                        }]))
                    } else {
                        let guard = store.lock().unwrap();
                        let cnt = if upper.contains("TOTAL_SIZE") {
                            guard.iter().filter(|d| d.total_size == 0).count()
                        } else {
                            guard.len()
                        };
                        Json(json!([{
                            "data": [{ "cnt": cnt }],
                            "error": ""
                        }]))
                    }
                }
            }),
        )
        .route(
            "/search",
            post(move |Json(req): Json<Value>| {
                let store = search_docs.clone();
                async move {
                    let limit = req["limit"].as_u64().unwrap_or(500) as usize;
                    let q = &req["query"];
                    let (gt_id, only_zero) = if let Some(must) = q["bool"]["must"].as_array() {
                        let mut gt = 0_i64;
                        let mut zero = false;
                        for clause in must {
                            if let Some(v) = clause["range"]["id"]["gt"].as_i64() {
                                gt = v;
                            }
                            if clause["equals"]["total_size"].as_i64() == Some(0) {
                                zero = true;
                            }
                        }
                        (gt, zero)
                    } else {
                        (q["range"]["id"]["gt"].as_i64().unwrap_or(0), false)
                    };

                    let guard = store.lock().unwrap();
                    let mut matched: Vec<&MockDoc> = guard
                        .iter()
                        .filter(|d| d.id > gt_id && (!only_zero || d.total_size == 0))
                        .collect();
                    matched.sort_by_key(|d| d.id);
                    let hits: Vec<Value> = matched
                        .into_iter()
                        .take(limit)
                        .map(|d| {
                            json!({
                                "_id": d.id,
                                "_source": {
                                    "info_hash": d.info_hash,
                                    "raw_name": d.raw_name,
                                    "refine_name": d.refine_name,
                                    "file_count": d.file_count,
                                    "total_size": d.total_size,
                                    "file_info": d.file_info,
                                    "hot": d.hot,
                                    "created_at": d.created_at,
                                    "updated_at": d.updated_at,
                                }
                            })
                        })
                        .collect();
                    Json(json!({ "hits": { "total": hits.len(), "hits": hits } }))
                }
            }),
        )
        .route(
            "/bulk",
            post(move |body: String| {
                let store = bulk_docs.clone();
                async move {
                    let mut guard = store.lock().unwrap();
                    let mut updated = 0_u64;
                    for line in body.lines().filter(|l| !l.trim().is_empty()) {
                        let val: Value = serde_json::from_str(line).unwrap();
                        let rep = &val["replace"];
                        let id = rep["id"].as_i64().unwrap();
                        let doc = &rep["doc"];
                        if let Some(target) = guard.iter_mut().find(|d| d.id == id) {
                            if let Some(s) = doc["refine_name"].as_str() {
                                target.refine_name = s.to_string();
                            }
                            if let Some(sz) = doc["total_size"].as_i64() {
                                target.total_size = sz;
                            }
                            updated += 1;
                        }
                    }
                    Json(json!({
                        "items": [{ "bulk": { "table": "torrents", "created": 0, "updated": updated, "deleted": 0, "status": 200 } }],
                        "current_line": updated + 1,
                        "skipped_lines": 0,
                        "errors": false,
                        "error": ""
                    }))
                }
            }),
        );

    let mc_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mc_addr = mc_listener.local_addr().unwrap();
    let mc_task = tokio::spawn(async move {
        let _ = axum::serve(mc_listener, mock_manticore).await;
    });

    h.state
        .services
        .reload_manticore(&ManticoreServiceConfig {
            url: format!("http://{mc_addr}"),
            ..ManticoreServiceConfig::default()
        })
        .await
        .unwrap();

    // 1) 启动「同步文件大小」任务（仅回填 total_size = 0 的 2 条旧记录）
    let (st, task1) = json_req(
        &h.app,
        "POST",
        "/api/v1/torrents/tasks",
        &h.cookie,
        Some(json!({
            "kind": "sync_total_size",
            "batch_size": 50,
            "batch_delay_ms": 0,
            "only_missing": true,
            "dry_run": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "start sync_total_size failed: {task1}");
    let t1_id = task1["id"].as_i64().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut final_t1 = task1;
    while Instant::now() < deadline {
        let (st, cur) = json_req(
            &h.app,
            "GET",
            &format!("/api/v1/torrents/tasks/{t1_id}"),
            &h.cookie,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        final_t1 = cur;
        if final_t1["status"] == "completed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(final_t1["status"], "completed", "t1={final_t1}");
    assert_eq!(final_t1["scanned_count"], 2);
    assert_eq!(final_t1["updated_count"], 2);
    assert_eq!(final_t1["cursor_id"], "1745280108080464303");
    assert_eq!(final_t1["sample_diffs"].as_array().unwrap().len(), 2);

    {
        let guard = docs.lock().unwrap();
        assert_eq!(guard[0].total_size, 3_000_012_345);
        assert_eq!(guard[1].total_size, 2_000_000);
        assert_eq!(guard[2].total_size, 500_000_000);
    }

    // 2) 启动「清洗标题」任务（全量重洗，2 条脏标题应被洗净并写回，1 条干净标题跳过）
    let (st, task2) = json_req(
        &h.app,
        "POST",
        "/api/v1/torrents/tasks",
        &h.cookie,
        Some(json!({
            "kind": "reclean_titles",
            "batch_size": 50,
            "batch_delay_ms": 0,
            "clean_scope": "all",
            "skip_empty_result": true,
            "dry_run": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "start reclean_titles failed: {task2}");
    let t2_id = task2["id"].as_i64().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut final_t2 = task2;
    while Instant::now() < deadline {
        let (st, cur) = json_req(
            &h.app,
            "GET",
            &format!("/api/v1/torrents/tasks/{t2_id}"),
            &h.cookie,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        final_t2 = cur;
        if final_t2["status"] == "completed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(final_t2["status"], "completed", "t2={final_t2}");
    assert_eq!(final_t2["scanned_count"], 3);
    assert_eq!(final_t2["updated_count"], 2);
    assert_eq!(final_t2["skipped_count"], 1);

    {
        let guard = docs.lock().unwrap();
        assert_eq!(guard[0].refine_name, "流浪地球2 4K");
        assert_eq!(guard[1].refine_name, "Ubuntu.24.04.LTS");
        assert_eq!(guard[2].refine_name, "阿凡达2.2022.1080p");
    }

    // 3) 列出任务概览 + 删除历史记录
    let (st, ov) = json_req(&h.app, "GET", "/api/v1/torrents/tasks", &h.cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ov["has_total_size_column"], true);
    assert_eq!(ov["tasks"].as_array().unwrap().len(), 2);

    let (st, del) = json_req(
        &h.app,
        "DELETE",
        &format!("/api/v1/torrents/tasks/{t1_id}"),
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(del["deleted"], true);

    mc_task.abort();
    h.state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}
