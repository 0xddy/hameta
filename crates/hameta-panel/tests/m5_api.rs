use axum::body::Body;
use axum::body::to_bytes;
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router as AxumRouter};
use chrono::Utc;
use futures::StreamExt;
use hameta_common::{
    CrawlerWorkerConfig, IndexerWorkerConfig, ManticoreServiceConfig, MetricsHourPoint,
    MetricsMinutePoint, RedisServiceConfig, ServiceKind, WorkerBootstrap, WorkerDrainStatus,
    WorkerReadyRequest, WorkerRegisterRequest, WorkerRole, WorkerRuntimeState,
    WorkerShutdownRequest, generate_token, hash_password, hash_token, verify_bearer_token,
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
struct MockM5Child {
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

async fn mock_drain(State(st): State<Arc<MockM5Child>>, headers: HeaderMap) -> Response {
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
    State(st): State<Arc<MockM5Child>>,
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
async fn mock_m5_worker_child() {
    let Ok(flag) = std::env::var("HAMETA_M5_MOCK_CHILD") else {
        return;
    };
    if flag != "1" {
        return;
    }

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).unwrap();
    let bootstrap: WorkerBootstrap = serde_json::from_str(&raw).unwrap();

    let shutdown = CancellationToken::new();
    let st = Arc::new(MockM5Child {
        bootstrap: bootstrap.clone(),
        draining: Arc::new(AtomicBool::new(false)),
        shutdown: shutdown.clone(),
    });
    let app = AxumRouter::new()
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
            rule_version: Some(1),
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

async fn setup_initialized_panel() -> TestHarness {
    let dir = std::env::temp_dir().join(format!("hameta-m5-api-{}", generate_token()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(&dir.join("hameta.db")).await.unwrap();

    let pw_hash = hash_password("secret-pass-123").unwrap();
    let admin = db.create_admin("admin", &pw_hash).await.unwrap();
    let session_tok = format!("sess-{}", generate_token());
    db.create_session(
        &hash_token(&session_tok),
        admin.id,
        "127.0.0.1",
        "m5-test",
        3600,
    )
    .await
    .unwrap();

    let redis_json = serde_json::to_string(&RedisServiceConfig::default()).unwrap();
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
    let exe = std::env::current_exe().unwrap();
    let factory = Arc::new(move |_bootstrap: &WorkerBootstrap| {
        Ok(SpawnSpec {
            program: exe.clone(),
            args: vec![
                "--exact".to_string(),
                "mock_m5_worker_child".to_string(),
                "--nocapture".to_string(),
            ],
            envs: vec![("HAMETA_M5_MOCK_CHILD".to_string(), "1".to_string())],
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
async fn m5_workers_profiles_config_tuning_and_port_conflict() {
    let h = setup_initialized_panel().await;

    // 1. 17 个内置 DHT 档位接口
    let (status, profiles) = json_req(&h.app, "GET", "/api/v1/dht/profiles", &h.cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    let arr = profiles["profiles"].as_array().unwrap();
    assert_eq!(arr.len(), 17);
    assert_eq!(arr[0]["name"], "P1");
    assert_eq!(arr[16]["name"], "P17");

    // 2. 创建 crawler-2（使用不同 UDP 端口 12314）
    let c2_cfg = CrawlerWorkerConfig {
        port: 12314,
        crawl_profile: "P6".to_string(),
        ..Default::default()
    };
    let (status, created) = json_req(
        &h.app,
        "POST",
        "/api/v1/workers",
        &h.cookie,
        Some(json!({
            "id": "crawler-2",
            "role": "crawler",
            "display_name": "辅助采集节点",
            "config": c2_cfg,
            "auto_restart": true,
            "max_restarts": 4,
            "restart_backoff_secs": 2
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created}");
    assert_eq!(created["id"], "crawler-2");

    // 3. 相同 UDP 端口 12314 创建 crawler-3 -> 409 PORT_CONFLICT
    let c3_cfg = CrawlerWorkerConfig {
        port: 12314,
        ..Default::default()
    };
    let (status, conflict) = json_req(
        &h.app,
        "POST",
        "/api/v1/workers",
        &h.cookie,
        Some(json!({
            "id": "crawler-3",
            "role": "crawler",
            "display_name": "端口冲突节点",
            "config": c3_cfg
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["code"], "PORT_CONFLICT");

    // 4. 启动 crawler-2 -> 运行态禁止删除 (409 WORKER_RUNNING)
    let (status, start_res) = json_req(
        &h.app,
        "POST",
        "/api/v1/workers/crawler-2/start",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "start failed: {start_res}");

    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let statuses = h.state.supervisor.list_worker_statuses().await.unwrap();
        if let Some(st) = statuses.into_iter().find(|w| w.id == "crawler-2")
            && st.runtime_state == WorkerRuntimeState::Running
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "crawler-2 failed to reach Running"
        );
        tokio::time::sleep(Duration::from_millis(80)).await;
    }

    let (status, del_err) = json_req(
        &h.app,
        "DELETE",
        "/api/v1/workers/crawler-2",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(del_err["code"], "WORKER_RUNNING");

    // 5. 停止后再删除 crawler-2 成功
    let (status, _) = json_req(
        &h.app,
        "POST",
        "/api/v1/workers/crawler-2/stop",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, del_ok) = json_req(
        &h.app,
        "DELETE",
        "/api/v1/workers/crawler-2",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "delete failed: {del_ok}");

    // 6. L1 / L2 / L3 参数调优 + deny_unknown_fields + 版本回滚
    let (status, cfg_v1) = json_req(
        &h.app,
        "GET",
        "/api/v1/workers/crawler-1/config",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cfg_v1["version"], 1);
    assert_eq!(cfg_v1["config"]["crawl_profile"], "P1");
    assert_eq!(cfg_v1["profile_defaults"]["find_node_rate_per_sec"], 878);

    // 未知字段应被拒绝 (400 INVALID_WORKER_CONFIG)
    let mut bad_cfg = cfg_v1["config"].clone();
    bad_cfg["unknown_phantom_field"] = json!(true);
    let (status, bad_resp) = json_req(
        &h.app,
        "PUT",
        "/api/v1/workers/crawler-1/config",
        &h.cookie,
        Some(json!({
            "config": bad_cfg,
            "note": "invalid field test",
            "apply_restart": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(bad_resp["code"], "INVALID_WORKER_CONFIG");

    // 更新 L1 + L2 + L3 -> version 2
    let mut next_cfg: CrawlerWorkerConfig =
        serde_json::from_value(cfg_v1["config"].clone()).unwrap();
    next_cfg.crawl_profile = "P8".to_string();
    next_cfg.metadata.max_worker_count = 512;
    next_cfg.discovery.sample_queries_per_second = 12;
    next_cfg.peer_lookup.max_lookups_per_second = Some(40);
    next_cfg.crawl.rate_limit.max_find_node_rate_per_sec = Some(4500);
    next_cfg.crawl.target.random_walk_percent = Some(55);

    let (status, updated) = json_req(
        &h.app,
        "PUT",
        "/api/v1/workers/crawler-1/config",
        &h.cookie,
        Some(json!({
            "config": next_cfg,
            "note": "升级 P8 + L2/L3 覆写",
            "apply_restart": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update failed: {updated}");
    assert_eq!(updated["version"], 2);
    assert_eq!(updated["config"]["crawl_profile"], "P8");
    assert_eq!(updated["config"]["metadata"]["max_worker_count"], 512);
    assert_eq!(
        updated["config"]["crawl"]["rate_limit"]["max_find_node_rate_per_sec"],
        4500
    );

    let (status, versions) = json_req(
        &h.app,
        "GET",
        "/api/v1/workers/crawler-1/config/versions?limit=10",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(versions.as_array().unwrap().len(), 2);

    let (status, rolled) = json_req(
        &h.app,
        "POST",
        "/api/v1/workers/crawler-1/config/rollback",
        &h.cookie,
        Some(json!({
            "target_version": 1,
            "apply_restart": false
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rollback failed: {rolled}");
    assert_eq!(rolled["version"], 3);
    assert_eq!(rolled["config"]["crawl_profile"], "P1");

    h.state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}

#[tokio::test]
async fn m5_overview_snapshot_history_and_sse_stream() {
    let h = setup_initialized_panel().await;

    // 1. 快照端点
    let (status, snap) =
        json_req(&h.app, "GET", "/api/v1/overview/snapshot", &h.cookie, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snap["health"]["panel_ok"], true);
    assert_eq!(snap["workers"].as_array().unwrap().len(), 2);

    // 2. 注入分钟级与小时级历史点并校验 1h / 7d 窗口
    let now = Utc::now().timestamp();
    let now_min = now - (now % 60);
    let now_hour = now - (now % 3600);

    h.state
        .db
        .upsert_metrics_minute(&MetricsMinutePoint {
            ts_minute: now_min - 120,
            hashes_per_min: 1200.0,
            metadata_succeeded_per_min: 300.0,
            stream_pushed_per_min: 280.0,
            filtered_per_min: 50.0,
            indexed_written_per_min: 230.0,
            hot_updated_per_min: 18.0,
            actual_qps: 450.0,
            stream_new_len: 12,
            stream_hot_len: 3,
            stream_new_lag: 4,
            udp_rx_bps: 10240.0,
            udp_tx_bps: 20480.0,
            metadata_rx_bps: 40960.0,
            total_rss_bytes: 128 * 1024 * 1024,
            avg_cpu_percent: 12.0,
        })
        .await
        .unwrap();
    h.state
        .db
        .upsert_metrics_minute(&MetricsMinutePoint {
            ts_minute: now_min - 60,
            hashes_per_min: 1500.0,
            metadata_succeeded_per_min: 350.0,
            stream_pushed_per_min: 340.0,
            filtered_per_min: 60.0,
            indexed_written_per_min: 280.0,
            hot_updated_per_min: 22.0,
            actual_qps: 520.0,
            stream_new_len: 10,
            stream_hot_len: 2,
            stream_new_lag: 2,
            udp_rx_bps: 12240.0,
            udp_tx_bps: 22480.0,
            metadata_rx_bps: 50960.0,
            total_rss_bytes: 130 * 1024 * 1024,
            avg_cpu_percent: 15.0,
        })
        .await
        .unwrap();

    h.state
        .db
        .upsert_metrics_hour(&MetricsHourPoint {
            ts_hour: now_hour - 7200,
            hashes_total: 60_000,
            metadata_succeeded_total: 18_000,
            stream_pushed_total: 15_000,
            filtered_total: 3_000,
            indexed_written_total: 12_000,
            hot_updated_total: 600,
            avg_qps: 400.0,
            max_stream_new_lag: 15,
            avg_rss_bytes: 120 * 1024 * 1024,
        })
        .await
        .unwrap();

    let (status, hist_1h) = json_req(
        &h.app,
        "GET",
        "/api/v1/overview/history?range=1h",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hist_1h["granularity"], "minute");
    assert_eq!(hist_1h["points"].as_array().unwrap().len(), 2);
    assert_eq!(hist_1h["points"][1]["indexed_written_per_min"], 280.0);

    let (status, hist_7d) = json_req(
        &h.app,
        "GET",
        "/api/v1/overview/history?range=7d",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hist_7d["granularity"], "hour");
    assert_eq!(hist_7d["points"].as_array().unwrap().len(), 1);
    assert_eq!(hist_7d["points"][0]["indexed_written_per_min"], 200.0);

    let (status, bad_range) = json_req(
        &h.app,
        "GET",
        "/api/v1/overview/history?range=99y",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(bad_range["code"], "INVALID_RANGE");

    // 3. SSE 实时推送流首帧验证
    let sse_req = Request::builder()
        .method("GET")
        .uri("/api/v1/overview/stream")
        .header(COOKIE, &h.cookie)
        .body(Body::empty())
        .unwrap();
    let sse_resp = h.app.clone().oneshot(sse_req).await.unwrap();
    assert_eq!(sse_resp.status(), StatusCode::OK);
    let ctype = sse_resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ctype.starts_with("text/event-stream"), "ctype={ctype}");

    let mut data_stream = sse_resp.into_body().into_data_stream();
    let mut accumulated = String::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !accumulated.contains("\"panel_ok\":true") {
        let next = tokio::time::timeout(Duration::from_secs(2), data_stream.next())
            .await
            .expect("SSE 首帧超时");
        let chunk = next.expect("SSE 流意外结束").expect("SSE 读取错误");
        accumulated.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(
        accumulated.contains("event: snapshot"),
        "missing snapshot event: {accumulated}"
    );
    assert!(
        accumulated.contains("\"panel_ok\":true"),
        "missing payload: {accumulated}"
    );

    h.state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}
