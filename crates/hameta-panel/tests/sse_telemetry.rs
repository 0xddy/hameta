use axum::body::Body;
use axum::extract::State;
use axum::http::header::COOKIE;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router as AxumRouter};
use hameta_common::{
    CrawlerWorkerConfig, FileInfo, IndexerWorkerConfig, IngestEventItem, IngestEventType,
    ManticoreServiceConfig, RedisServiceConfig, ServiceKind, WorkerBootstrap, WorkerDrainStatus,
    WorkerIngestEventsRequest, WorkerReadyRequest, WorkerRegisterRequest, WorkerReloadGeoIpRequest,
    WorkerReloadGeoIpResponse, WorkerRole, WorkerRuntimeState, WorkerShutdownRequest,
    WorkerSnapshotEnvelope, generate_token, hash_password, hash_token, verify_bearer_token,
};
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Clone)]
struct MockSseWorker {
    bootstrap: WorkerBootstrap,
    draining: Arc<AtomicBool>,
    shutdown: CancellationToken,
    tick: Arc<AtomicU64>,
}

fn mock_auth(headers: &HeaderMap, expected: &str) -> bool {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    verify_bearer_token(auth, expected)
}

async fn mock_snapshot(State(st): State<Arc<MockSseWorker>>, headers: HeaderMap) -> Response {
    if !mock_auth(&headers, &st.bootstrap.ipc_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let n = st.tick.fetch_add(1, Ordering::Relaxed);
    let rx_pkts = 1_000 + n * 1_000;
    let rx_v4 = 700 + n * 700;
    let rx_v6 = 300 + n * 300;
    let tx_pkts = 500 + n * 400;
    let us_pkts = 500 + n * 600;
    let jp_pkts = 200 + n * 300;

    let payload = if st.bootstrap.role == WorkerRole::Crawler {
        json!({
            "uptime_secs": 10 + n,
            "crawl_profile": "P1",
            "effective_metadata_workers": 256,
            "local_cache_entries": 1_000,
            "redis_pool_size": 32,
            "redis_pool_available": 32,
            "rss_bytes": 33_554_432,
            "app": {
                "early_filter_blocked": n * 10,
                "torrents_downloaded": n * 20,
                "torrents_pushed": n * 20,
                "hot_pushed": n * 5
            },
            "dht": {
                "hashes_received": n * 50,
                "queries_new": n * 200,
                "queries_revisit": n * 50,
                "queries_bootstrap": 10,
                "udp_received": rx_pkts,
                "udp_invalid": n * 3,
                "udp_queue_full": n
            },
            "observability": {
                "udp_rx_packets": rx_pkts,
                "udp_rx_ipv4_packets": rx_v4,
                "udp_rx_ipv6_packets": rx_v6,
                "udp_rx_bytes": rx_pkts * 100,
                "udp_tx_packets": tx_pkts,
                "udp_tx_bytes": tx_pkts * 90,
                "inbound_ping": n * 40,
                "inbound_find_node": n * 80,
                "inbound_get_peers": n * 450,
                "inbound_announce_peer": n * 220,
                "inbound_other": n * 10,
                "inbound_response": n * 190,
                "inbound_error": n * 5,
                "inbound_parse_error": n * 4,
                "metadata_bytes_downloaded": n * 8_192
            },
            "geo": {
                "enabled": true,
                "mmdb_loaded": true,
                "mmdb_path": st.bootstrap.geoip.as_ref().map_or_else(|| "GeoLite2-Country.mmdb".to_string(), |g| g.mmdb_path.clone()),
                "mmdb_size_bytes": 7_400_000,
                "mmdb_build_epoch": 1_763_800_000,
                "database_type": "GeoLite2-Country",
                "error": null,
                "home_country": st.bootstrap.geoip.as_ref().map_or_else(|| "CN".to_string(), |g| g.normalized_home_country()),
                "adaptive_sample_threshold_pps": 30_000,
                "sample_rate_divisor": 1,
                "samples_processed": us_pkts + jp_pkts,
                "samples_dropped": n * 2,
                "unknown_packets": n * 8,
                "cache_hits": 900 + n * 800,
                "cache_misses": 100 + n * 100,
                "cache_hit_ratio": 0.89,
                "avg_lookup_ns": 210,
                "countries": {
                    "US": {
                        "packets": us_pkts,
                        "bytes": us_pkts * 100,
                        "get_peers": us_pkts / 2,
                        "announce_peer": us_pkts / 4,
                        "response": us_pkts / 5,
                        "find_node_ping": us_pkts / 20,
                        "other": 0
                    },
                    "JP": {
                        "packets": jp_pkts,
                        "bytes": jp_pkts * 100,
                        "get_peers": jp_pkts / 2,
                        "announce_peer": jp_pkts / 4,
                        "response": jp_pkts / 5,
                        "find_node_ping": jp_pkts / 20,
                        "other": 0
                    }
                }
            }
        })
    } else {
        json!({
            "uptime_secs": 10 + n,
            "active_rule_version": 1,
            "redis_pool_size": 32,
            "redis_pool_available": 32,
            "rss_bytes": 16_777_216,
            "app": {
                "filter_accepted": 0,
                "filter_rejected": 0,
                "records_written": 0,
                "hot_records_updated": 0
            }
        })
    };

    Json(WorkerSnapshotEnvelope {
        instance_id: st.bootstrap.instance_id.clone(),
        role: st.bootstrap.role,
        captured_at_unix: 1_763_800_000 + n,
        payload,
    })
    .into_response()
}

async fn mock_geoip_reload(
    State(st): State<Arc<MockSseWorker>>,
    headers: HeaderMap,
    Json(req): Json<WorkerReloadGeoIpRequest>,
) -> Response {
    if !mock_auth(&headers, &st.bootstrap.ipc_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(WorkerReloadGeoIpResponse {
        enabled: req.config.enabled,
        mmdb_loaded: true,
        mmdb_path: req.config.mmdb_path,
        mmdb_size_bytes: 1234,
        mmdb_build_epoch: 1_763_800_000,
        database_type: "GeoLite2-Country".to_string(),
        error: None,
    })
    .into_response()
}

async fn mock_drain(State(st): State<Arc<MockSseWorker>>, headers: HeaderMap) -> Response {
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
    State(st): State<Arc<MockSseWorker>>,
    headers: HeaderMap,
    Json(_req): Json<WorkerShutdownRequest>,
) -> Response {
    if !mock_auth(&headers, &st.bootstrap.ipc_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    st.draining.store(true, Ordering::Relaxed);
    st.shutdown.cancel();
    Json(json!({ "accepted": true })).into_response()
}

#[tokio::test]
async fn mock_sse_worker_child() {
    let Ok(flag) = std::env::var("HAMETA_SSE_MOCK_CHILD") else {
        return;
    };
    if flag != "1" {
        return;
    }

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).unwrap();
    let bootstrap: WorkerBootstrap = serde_json::from_str(&raw).unwrap();

    let shutdown = CancellationToken::new();
    let st = Arc::new(MockSseWorker {
        bootstrap: bootstrap.clone(),
        draining: Arc::new(AtomicBool::new(false)),
        shutdown: shutdown.clone(),
        tick: Arc::new(AtomicU64::new(0)),
    });
    let app = AxumRouter::new()
        .route("/internal/v1/snapshot", get(mock_snapshot))
        .route("/internal/v1/geoip/reload", post(mock_geoip_reload))
        .route("/internal/v1/drain-status", get(mock_drain))
        .route("/internal/v1/shutdown", post(mock_shut))
        .with_state(st);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let worker_port = listener.local_addr().unwrap().port();
    let cancel = shutdown.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel.cancelled().await })
            .await;
    });

    let client = reqwest::Client::new();
    let _ = client
        .post(format!(
            "http://127.0.0.1:{}/internal/v1/register",
            bootstrap.panel_ipc_port
        ))
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

    let _ = client
        .post(format!(
            "http://127.0.0.1:{}/internal/v1/ready",
            bootstrap.panel_ipc_port
        ))
        .bearer_auth(&bootstrap.ipc_token)
        .json(&WorkerReadyRequest {
            instance_id: bootstrap.instance_id.clone(),
            rule_version: Some(1),
        })
        .send()
        .await
        .unwrap();

    // 若环境变量指示模拟 Indexer 主动上报入库事件，则推送一条样本后等待停机
    if std::env::var("HAMETA_SSE_PUSH_SAMPLE").as_deref() == Ok("1")
        && bootstrap.role == WorkerRole::Indexer
    {
        let _ = client
            .post(format!(
                "http://127.0.0.1:{}/internal/v1/ingest-events",
                bootstrap.panel_ipc_port
            ))
            .bearer_auth(&bootstrap.ipc_token)
            .json(&WorkerIngestEventsRequest {
                instance_id: bootstrap.instance_id.clone(),
                events: vec![IngestEventItem {
                    info_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                    event_type: IngestEventType::Indexed,
                    worker_id: bootstrap.instance_id.clone(),
                    manticore_id: 1001,
                    batch_id: 1,
                    raw_name: "[ad] Ubuntu 24.04".to_string(),
                    refine_name: "Ubuntu 24.04".to_string(),
                    cleaned: true,
                    total_size: 5_000_000,
                    file_count: 1,
                    files_preview: vec![FileInfo {
                        path: "ubuntu.iso".to_string(),
                        size: 5_000_000,
                    }],
                    hot: 1,
                    hot_delta: 1,
                    latency_ms: 12,
                    updated_at: 1_763_800_000,
                }],
            })
            .send()
            .await
            .unwrap();
    }

    shutdown.cancelled().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    std::process::exit(0);
}

async fn setup_panel(
    with_mock_workers: bool,
    push_sample: bool,
) -> (axum::Router, Arc<PanelState>, String, PathBuf) {
    let dir = std::env::temp_dir().join(format!("hameta-sse-test-{}", generate_token()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(&dir.join("hameta.db")).await.unwrap();
    db.mark_initialized().await.unwrap();

    let pass_hash = hash_password("admin-pass-123").unwrap();
    let user = db.create_admin("admin", &pass_hash).await.unwrap();
    let raw_token = generate_token();
    let token_hash = hash_token(&raw_token);
    db.create_session(&token_hash, user.id, "127.0.0.1", "sse-test", 3600)
        .await
        .unwrap();
    let cookie = format!("hameta_session={raw_token}");

    let r_json = serde_json::to_string(&RedisServiceConfig::default()).unwrap();
    db.save_service_config(ServiceKind::Redis, &r_json, "admin", "init")
        .await
        .unwrap();
    let m_json = serde_json::to_string(&ManticoreServiceConfig::default()).unwrap();
    db.save_service_config(ServiceKind::Manticore, &m_json, "admin", "init")
        .await
        .unwrap();

    let supervisor = if with_mock_workers {
        let exe = std::env::current_exe().unwrap();
        let factory = Arc::new(move |_: &WorkerBootstrap| {
            let mut envs = vec![("HAMETA_SSE_MOCK_CHILD".to_string(), "1".to_string())];
            if push_sample {
                envs.push(("HAMETA_SSE_PUSH_SAMPLE".to_string(), "1".to_string()));
            }
            Ok(SpawnSpec {
                program: exe.clone(),
                args: vec![
                    "--exact".to_string(),
                    "mock_sse_worker_child".to_string(),
                    "--nocapture".to_string(),
                ],
                envs,
            })
        });
        Supervisor::with_command_factory(db.clone(), dir.clone(), factory)
            .await
            .unwrap()
    } else {
        Supervisor::new(db.clone(), dir.clone()).await.unwrap()
    };

    let services = Arc::new(ServiceRuntimePool::default());
    let metrics = MetricsCollector::new(db.clone(), supervisor.clone(), services.clone());
    let state = Arc::new(PanelState {
        db,
        config: PanelConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: dir.clone(),
            dev_mode: false,
        },
        supervisor,
        services,
        metrics,
        login_limiter: Arc::new(LoginRateLimiter::default()),
        setup_lock: Arc::new(Mutex::new(())),
    });
    let app = build_router(state.clone());
    (app, state, cookie, dir)
}

async fn read_sse_until(resp: &mut reqwest::Response, needle: &str, timeout_secs: u64) -> String {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut acc = String::new();
    while Instant::now() < deadline {
        if acc.contains(needle) {
            return acc;
        }
        let remain = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remain, resp.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                acc.push_str(&String::from_utf8_lossy(&chunk));
                if acc.contains(needle) {
                    return acc;
                }
            }
            _ => break,
        }
    }
    acc
}

fn encode_ctrl(type_num: usize, size: usize, out: &mut Vec<u8>) {
    if type_num <= 7 {
        out.push(((type_num << 5) | size) as u8);
    } else {
        out.push(size as u8);
        out.push((type_num - 7) as u8);
    }
}

fn encode_str(s: &str, out: &mut Vec<u8>) {
    encode_ctrl(2, s.len(), out);
    out.extend_from_slice(s.as_bytes());
}

fn encode_uint16(val: u16, out: &mut Vec<u8>) {
    let bytes = val.to_be_bytes();
    let trimmed = if bytes[0] == 0 {
        if bytes[1] == 0 { &[][..] } else { &bytes[1..2] }
    } else {
        &bytes[..]
    };
    encode_ctrl(5, trimmed.len(), out);
    out.extend_from_slice(trimmed);
}

fn encode_uint32(val: u32, out: &mut Vec<u8>) {
    let bytes = val.to_be_bytes();
    let first_nz = bytes.iter().position(|&b| b != 0).unwrap_or(4);
    encode_ctrl(6, bytes.len() - first_nz, out);
    out.extend_from_slice(&bytes[first_nz..]);
}

fn encode_uint64(val: u64, out: &mut Vec<u8>) {
    let bytes = val.to_be_bytes();
    let first_nz = bytes.iter().position(|&b| b != 0).unwrap_or(8);
    encode_ctrl(9, bytes.len() - first_nz, out);
    out.extend_from_slice(&bytes[first_nz..]);
}

fn build_minimal_mmdb_fixture() -> Vec<u8> {
    let node_count: u32 = 1;
    let mut data = Vec::new();
    encode_ctrl(7, 1, &mut data);
    encode_str("country", &mut data);
    encode_ctrl(7, 1, &mut data);
    encode_str("iso_code", &mut data);
    encode_str("US", &mut data);

    let val = node_count + 16;
    let mut out = Vec::new();
    out.extend_from_slice(&val.to_be_bytes()[1..4]);
    out.extend_from_slice(&val.to_be_bytes()[1..4]);
    out.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\xab\xcd\xefMaxMind.com");

    encode_ctrl(7, 9, &mut out);
    encode_str("node_count", &mut out);
    encode_uint32(node_count, &mut out);
    encode_str("record_size", &mut out);
    encode_uint16(24, &mut out);
    encode_str("ip_version", &mut out);
    encode_uint16(6, &mut out);
    encode_str("binary_format_major_version", &mut out);
    encode_uint16(2, &mut out);
    encode_str("binary_format_minor_version", &mut out);
    encode_uint16(0, &mut out);
    encode_str("build_epoch", &mut out);
    encode_uint64(1_763_800_000, &mut out);
    encode_str("database_type", &mut out);
    encode_str("GeoLite2-Country", &mut out);
    encode_str("description", &mut out);
    encode_ctrl(7, 1, &mut out);
    encode_str("en", &mut out);
    encode_str("Test", &mut out);
    encode_str("languages", &mut out);
    encode_ctrl(11, 1, &mut out);
    encode_str("en", &mut out);
    out
}

#[tokio::test]
async fn sse_and_geoip_endpoints_require_authentication() {
    let (app, state, _cookie, dir) = setup_panel(false, false).await;

    for uri in [
        "/api/v1/console/recent",
        "/api/v1/console/ingest-stream",
        "/api/v1/globe/snapshot",
        "/api/v1/globe/stream",
        "/api/v1/settings/geoip",
        "/api/v1/settings/nofile",
    ] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "uri={uri}");
    }

    state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn nofile_settings_get_validate_and_persist_work_end_to_end() {
    let (app, state, cookie, dir) = setup_panel(false, false).await;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_cancel = CancellationToken::new();
    let srv_tok = srv_cancel.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { srv_tok.cancelled().await })
            .await;
    });

    let client = reqwest::Client::new();

    // 1) GET /api/v1/settings/nofile 返回当前平台信息、推荐档位与实时限制
    let get_resp = client
        .get(format!("http://{addr}/api/v1/settings/nofile"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let st_json: Value = get_resp.json().await.unwrap();
    assert_eq!(st_json["platform"], std::env::consts::OS);
    assert!(
        st_json["recommended_presets"]
            .as_array()
            .is_some_and(|arr| arr.len() >= 4)
    );

    // 2) 非法参数（soft > hard）必须被 400 拒绝
    let bad_resp = client
        .put(format!("http://{addr}/api/v1/settings/nofile"))
        .header(COOKIE, &cookie)
        .json(&json!({
            "apply_process": true,
            "process_soft": 524288,
            "process_hard": 65536,
            "apply_to_workers": false,
            "persist_app_config": true,
            "apply_system": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_resp.status(), StatusCode::BAD_REQUEST);

    // 3) 合法参数：在 Unix 上沿用当前 hard 避免破坏测试进程硬上限，在 Windows 上校验持久化保存与友好提示
    let cur_hard = st_json["process_hard"].as_u64().unwrap_or(262_144);
    let cur_soft = st_json["process_soft"]
        .as_u64()
        .unwrap_or(262_144)
        .min(cur_hard)
        .max(1_024);
    let put_resp = client
        .put(format!("http://{addr}/api/v1/settings/nofile"))
        .header(COOKIE, &cookie)
        .json(&json!({
            "apply_process": true,
            "process_soft": cur_soft,
            "process_hard": cur_hard,
            "apply_to_workers": true,
            "persist_app_config": true,
            "apply_system": false,
            "persist_system_files": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), StatusCode::OK);
    let put_json: Value = put_resp.json().await.unwrap();
    assert_eq!(put_json["persisted_app_config"], true);
    assert_eq!(
        put_json["status"]["saved_config"]["process_soft"],
        json!(cur_soft)
    );
    assert_eq!(
        put_json["status"]["saved_config"]["process_hard"],
        json!(cur_hard)
    );
    if cfg!(unix) {
        assert_eq!(put_json["process_applied"], true);
    }

    srv_cancel.cancel();
    state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn console_ingest_stream_replays_resumes_and_filters_live_events() {
    let (app, state, cookie, dir) = setup_panel(true, true).await;

    let idx_cfg = serde_json::to_string(&IndexerWorkerConfig::default()).unwrap();
    state
        .db
        .create_worker(
            "indexer-1",
            WorkerRole::Indexer,
            "Indexer #1",
            &idx_cfg,
            "admin",
        )
        .await
        .unwrap();
    state.supervisor.start_worker("indexer-1").await.unwrap();

    // 等待模拟 Indexer 子进程启动并通过 /internal/v1/ingest-events 推送第 1 条实时入库事件
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        if state.supervisor.ingest_hub().stats().latest_seq >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(state.supervisor.ingest_hub().stats().latest_seq, 1);

    // 补充推送第 2、3 条不同 type / worker 的事件用于校验过滤与断点续传
    state.supervisor.ingest_hub().publish_batch(vec![
        IngestEventItem {
            info_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            event_type: IngestEventType::HotUpdated,
            worker_id: "indexer-1".to_string(),
            manticore_id: 2002,
            batch_id: 2,
            raw_name: String::new(),
            refine_name: String::new(),
            cleaned: false,
            total_size: 0,
            file_count: 0,
            files_preview: Vec::new(),
            hot: 3,
            hot_delta: 3,
            latency_ms: 4,
            updated_at: 1_763_800_001,
        },
        IngestEventItem {
            info_hash: "cccccccccccccccccccccccccccccccccccccccc".to_string(),
            event_type: IngestEventType::Indexed,
            worker_id: "indexer-2".to_string(),
            manticore_id: 3003,
            batch_id: 1,
            raw_name: "Fedora.iso".to_string(),
            refine_name: "Fedora.iso".to_string(),
            cleaned: false,
            total_size: 2048,
            file_count: 1,
            files_preview: Vec::new(),
            hot: 1,
            hot_delta: 1,
            latency_ms: 6,
            updated_at: 1_763_800_002,
        },
    ]);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_cancel = CancellationToken::new();
    let srv_tok = srv_cancel.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { srv_tok.cancelled().await })
            .await;
    });

    let client = reqwest::Client::new();

    // 1) GET /api/v1/console/recent 过滤与结构校验
    let recent_resp = client
        .get(format!("http://{addr}/api/v1/console/recent"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(recent_resp.status(), StatusCode::OK);
    let recent_json: Value = recent_resp.json().await.unwrap();
    assert_eq!(recent_json["stats"]["latest_seq"], 3);
    assert_eq!(recent_json["events"].as_array().unwrap().len(), 3);

    // 2) SSE 连接按 event_type=indexed & worker_id=indexer-1 过滤，仅应回放 seq=1
    let mut filtered_sse = client
        .get(format!(
            "http://{addr}/api/v1/console/ingest-stream?event_type=indexed&worker_id=indexer-1"
        ))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(filtered_sse.status(), StatusCode::OK);
    let filtered_text = read_sse_until(
        &mut filtered_sse,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        3,
    )
    .await;
    assert!(filtered_text.contains("event: ready"));
    assert!(filtered_text.contains("event: ingest"));
    assert!(!filtered_text.contains("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
    assert!(!filtered_text.contains("cccccccccccccccccccccccccccccccccccccccc"));
    drop(filtered_sse);

    // 3) Last-Event-ID=1 断点续传并持续接收实时事件 seq=4
    let mut resume_sse = client
        .get(format!("http://{addr}/api/v1/console/ingest-stream"))
        .header(COOKIE, &cookie)
        .header("Last-Event-ID", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(resume_sse.status(), StatusCode::OK);
    let initial_resume = read_sse_until(
        &mut resume_sse,
        "cccccccccccccccccccccccccccccccccccccccc",
        3,
    )
    .await;
    assert!(initial_resume.contains("\"resumed_from\":1"));
    assert!(!initial_resume.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(initial_resume.contains("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
    assert!(initial_resume.contains("cccccccccccccccccccccccccccccccccccccccc"));

    state
        .supervisor
        .ingest_hub()
        .publish_batch(vec![IngestEventItem {
            info_hash: "dddddddddddddddddddddddddddddddddddddddd".to_string(),
            event_type: IngestEventType::Indexed,
            worker_id: "indexer-1".to_string(),
            manticore_id: 4004,
            batch_id: 3,
            raw_name: "ArchLinux.iso".to_string(),
            refine_name: "ArchLinux.iso".to_string(),
            cleaned: false,
            total_size: 4096,
            file_count: 1,
            files_preview: Vec::new(),
            hot: 1,
            hot_delta: 1,
            latency_ms: 5,
            updated_at: 1_763_800_003,
        }]);
    let live_chunk = read_sse_until(
        &mut resume_sse,
        "dddddddddddddddddddddddddddddddddddddddd",
        3,
    )
    .await;
    assert!(live_chunk.contains("id: 4"));
    assert!(live_chunk.contains("dddddddddddddddddddddddddddddddddddddddd"));
    drop(resume_sse);

    // 4) SSE max_eps=2 服务端限流与 lagged(reason=rate_limit) 上报验证
    let mut throttled_sse = client
        .get(format!(
            "http://{addr}/api/v1/console/ingest-stream?replay=0&event_type=indexed&max_eps=2"
        ))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(throttled_sse.status(), StatusCode::OK);
    let ready_frame = read_sse_until(&mut throttled_sse, "\"max_eps\":2", 3).await;
    assert!(ready_frame.contains("event: ready"));

    let burst: Vec<IngestEventItem> = (1..=5)
        .map(|i| IngestEventItem {
            info_hash: format!("{i:040x}"),
            event_type: IngestEventType::Indexed,
            worker_id: "indexer-1".to_string(),
            manticore_id: 5000 + i,
            batch_id: 10,
            raw_name: format!("Burst-{i}.iso"),
            refine_name: format!("Burst-{i}.iso"),
            cleaned: false,
            total_size: 1024,
            file_count: 1,
            files_preview: Vec::new(),
            hot: 1,
            hot_delta: 1,
            latency_ms: 2,
            updated_at: 1_763_800_010 + i,
        })
        .collect();
    state.supervisor.ingest_hub().publish_batch(burst);

    let first_window = read_sse_until(&mut throttled_sse, &format!("{:040x}", 2), 3).await;
    assert!(first_window.contains(&format!("{:040x}", 1)));
    assert!(first_window.contains(&format!("{:040x}", 2)));

    // 跨过 1 秒限流窗口后再推第 6 条，应先收到 skipped=3 的 lagged 帧，再收到第 6 条，且第 3~5 条被限流丢弃
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    state
        .supervisor
        .ingest_hub()
        .publish_batch(vec![IngestEventItem {
            info_hash: format!("{:040x}", 6),
            event_type: IngestEventType::Indexed,
            worker_id: "indexer-1".to_string(),
            manticore_id: 5006,
            batch_id: 11,
            raw_name: "Burst-6.iso".to_string(),
            refine_name: "Burst-6.iso".to_string(),
            cleaned: false,
            total_size: 1024,
            file_count: 1,
            files_preview: Vec::new(),
            hot: 1,
            hot_delta: 1,
            latency_ms: 2,
            updated_at: 1_763_800_020,
        }]);
    let second_window = read_sse_until(&mut throttled_sse, &format!("{:040x}", 6), 3).await;
    assert!(second_window.contains("event: lagged"));
    assert!(second_window.contains("\"reason\":\"rate_limit\""));
    assert!(second_window.contains("\"skipped\":3"));
    assert!(second_window.contains(&format!("{:040x}", 6)));
    assert!(!second_window.contains(&format!("{:040x}", 3)));
    assert!(!second_window.contains(&format!("{:040x}", 4)));
    assert!(!second_window.contains(&format!("{:040x}", 5)));
    drop(throttled_sse);

    srv_cancel.cancel();
    state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn globe_snapshot_sse_and_geoip_settings_work_end_to_end() {
    let (app, state, cookie, dir) = setup_panel(true, false).await;

    let crawler_cfg = serde_json::to_string(&CrawlerWorkerConfig::default()).unwrap();
    state
        .db
        .create_worker(
            "crawler-1",
            WorkerRole::Crawler,
            "Crawler #1",
            &crawler_cfg,
            "admin",
        )
        .await
        .unwrap();
    state.supervisor.start_worker("crawler-1").await.unwrap();

    // 等待模拟 Crawler 完成注册与就绪
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        let list = state.supervisor.list_worker_statuses().await.unwrap();
        if list
            .iter()
            .any(|w| w.id == "crawler-1" && w.runtime_state == WorkerRuntimeState::Running)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 两次快照轮询 + 采样，确保 dt_secs >= 0.2s 并算出真实 pkt/s 与国家差分
    state.supervisor.poll_snapshots_once().await;
    state.metrics.sample_once().await.unwrap();
    tokio::time::sleep(Duration::from_millis(260)).await;
    state.supervisor.poll_snapshots_once().await;
    state.metrics.sample_once().await.unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_cancel = CancellationToken::new();
    let srv_tok = srv_cancel.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move { srv_tok.cancelled().await })
            .await;
    });

    let client = reqwest::Client::new();

    // 1) 检查 Globe REST 快照与 OverviewKpi 新速率字段
    let globe_resp = client
        .get(format!("http://{addr}/api/v1/globe/snapshot"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(globe_resp.status(), StatusCode::OK);
    let globe_json: Value = globe_resp.json().await.unwrap();
    assert_eq!(globe_json["running_crawlers"], 1);
    assert!(globe_json["udp_rx_pps"].as_f64().unwrap() > 0.0);
    assert!(globe_json["ipv4_pps"].as_f64().unwrap() > 0.0);
    assert!(globe_json["ipv6_pps"].as_f64().unwrap() > 0.0);
    assert!(globe_json["channels"]["get_peers_pps"].as_f64().unwrap() > 0.0);
    assert_eq!(globe_json["geoip"]["mmdb_loaded"], true);
    let countries = globe_json["countries"].as_array().unwrap();
    assert_eq!(countries[0]["iso_code"], "US");
    assert_eq!(countries[1]["iso_code"], "JP");
    assert!(countries[0]["pps"].as_f64().unwrap() > countries[1]["pps"].as_f64().unwrap());

    let ov_resp = client
        .get(format!("http://{addr}/api/v1/overview/snapshot"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    let ov_json: Value = ov_resp.json().await.unwrap();
    assert!(ov_json["kpi"]["udp_rx_pps"].as_f64().unwrap() > 0.0);
    assert!(ov_json["kpi"]["udp_tx_pps"].as_f64().unwrap() > 0.0);

    // 2) 检查 /api/v1/globe/stream SSE 初始帧与增量推送
    let init_seq = globe_json["seq"].as_u64().unwrap();
    let mut globe_sse = client
        .get(format!("http://{addr}/api/v1/globe/stream"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(globe_sse.status(), StatusCode::OK);
    let init_globe = read_sse_until(&mut globe_sse, &format!("\"seq\":{init_seq}"), 3).await;
    assert!(init_globe.contains("event: globe"));
    assert!(init_globe.contains(&format!("id: {init_seq}")));

    tokio::time::sleep(Duration::from_millis(250)).await;
    state.supervisor.poll_snapshots_once().await;
    state.metrics.sample_once().await.unwrap();
    let next_seq = init_seq + 1;
    let next_globe = read_sse_until(&mut globe_sse, &format!("\"seq\":{next_seq}"), 3).await;
    assert!(next_globe.contains("event: globe"));
    assert!(next_globe.contains(&format!("id: {next_seq}")));
    drop(globe_sse);

    // 3) GeoIP 设置接口：默认缺失降级 -> 非法国家校验 -> 写入真实 MMDB fixture 并触发 Crawler 热重载
    let geo_get = client
        .get(format!("http://{addr}/api/v1/settings/geoip"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(geo_get["mmdb_exists"], false);
    assert_eq!(geo_get["mmdb_valid"], false);
    assert_eq!(geo_get["running_crawlers"].as_array().unwrap().len(), 1);

    let bad_put = client
        .put(format!("http://{addr}/api/v1/settings/geoip"))
        .header(COOKIE, &cookie)
        .json(&json!({
            "config": {
                "enabled": true,
                "mmdb_path": "custom.mmdb",
                "home_country": "CHINA",
                "prefix_cache_slots": 65536,
                "adaptive_sample_threshold_pps": 30000,
                "sample_queue_capacity": 8192
            },
            "reload_crawlers": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_put.status(), StatusCode::BAD_REQUEST);

    std::fs::write(dir.join("custom.mmdb"), build_minimal_mmdb_fixture()).unwrap();
    let ok_put = client
        .put(format!("http://{addr}/api/v1/settings/geoip"))
        .header(COOKIE, &cookie)
        .json(&json!({
            "config": {
                "enabled": true,
                "mmdb_path": "custom.mmdb",
                "home_country": "jp",
                "prefix_cache_slots": 50000,
                "adaptive_sample_threshold_pps": 25000,
                "sample_queue_capacity": 4096
            },
            "reload_crawlers": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok_put.status(), StatusCode::OK);
    let ok_put_json: Value = ok_put.json().await.unwrap();
    assert_eq!(ok_put_json["config"]["home_country"], "JP");
    assert_eq!(ok_put_json["config"]["prefix_cache_slots"], 65536);
    assert_eq!(ok_put_json["inspect"]["mmdb_exists"], true);
    assert_eq!(ok_put_json["inspect"]["mmdb_valid"], true);
    assert_eq!(ok_put_json["inspect"]["database_type"], "GeoLite2-Country");
    assert_eq!(ok_put_json["reload_results"].as_array().unwrap().len(), 1);
    assert_eq!(ok_put_json["reload_results"][0]["ok"], true);

    let reload_post = client
        .post(format!("http://{addr}/api/v1/settings/geoip/reload"))
        .header(COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(reload_post.status(), StatusCode::OK);
    let reload_post_json: Value = reload_post.json().await.unwrap();
    assert_eq!(reload_post_json["inspect"]["mmdb_valid"], true);
    assert_eq!(reload_post_json["reload_results"][0]["ok"], true);

    srv_cancel.cancel();
    state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}
