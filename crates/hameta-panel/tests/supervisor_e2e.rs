use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hameta_common::{
    CrawlerWorkerConfig, DesiredState, IndexerWorkerConfig, ManticoreServiceConfig,
    RedisServiceConfig, ServiceKind, WorkerBootstrap, WorkerDrainStatus, WorkerReadyRequest,
    WorkerRegisterRequest, WorkerReloadRulesRequest, WorkerReloadRulesResponse, WorkerRole,
    WorkerRuntimeState, WorkerShutdownRequest, WorkerSnapshotEnvelope, generate_token,
    verify_bearer_token,
};
use hameta_filter::default_online_filter_bundle;
use hameta_panel::supervisor::{SpawnSpec, Supervisor};
use hameta_storage::Database;
use serde_json::json;
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct MockChildState {
    bootstrap: WorkerBootstrap,
    rule_version: Arc<AtomicI64>,
    draining: Arc<AtomicBool>,
    drain_completed: Arc<AtomicBool>,
    shutdown_token: CancellationToken,
}

fn is_child_authorized(headers: &HeaderMap, expected: &str) -> bool {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    verify_bearer_token(auth, expected)
}

fn child_unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"error": "unauthorized"})),
    )
        .into_response()
}

async fn mock_snapshot(State(st): State<Arc<MockChildState>>, headers: HeaderMap) -> Response {
    if !is_child_authorized(&headers, &st.bootstrap.ipc_token) {
        return child_unauthorized();
    }
    let ver = st.rule_version.load(Ordering::Relaxed);
    (
        StatusCode::OK,
        Json(WorkerSnapshotEnvelope {
            instance_id: st.bootstrap.instance_id.clone(),
            role: st.bootstrap.role,
            captured_at_unix: 1_700_000_000,
            payload: json!({
                "mock": true,
                "active_rule_version": ver
            }),
        }),
    )
        .into_response()
}

async fn mock_reload(
    State(st): State<Arc<MockChildState>>,
    headers: HeaderMap,
    Json(req): Json<WorkerReloadRulesRequest>,
) -> Response {
    if !is_child_authorized(&headers, &st.bootstrap.ipc_token) {
        return child_unauthorized();
    }
    st.rule_version.store(req.version, Ordering::Relaxed);
    (
        StatusCode::OK,
        Json(WorkerReloadRulesResponse {
            active_rule_version: req.version,
        }),
    )
        .into_response()
}

async fn mock_drain_status(State(st): State<Arc<MockChildState>>, headers: HeaderMap) -> Response {
    if !is_child_authorized(&headers, &st.bootstrap.ipc_token) {
        return child_unauthorized();
    }
    (
        StatusCode::OK,
        Json(WorkerDrainStatus {
            draining: st.draining.load(Ordering::Relaxed),
            queued_items: 0,
            in_flight_items: 0,
            completed: st.drain_completed.load(Ordering::Relaxed),
        }),
    )
        .into_response()
}

async fn mock_shutdown(
    State(st): State<Arc<MockChildState>>,
    headers: HeaderMap,
    Json(_req): Json<WorkerShutdownRequest>,
) -> Response {
    if !is_child_authorized(&headers, &st.bootstrap.ipc_token) {
        return child_unauthorized();
    }
    st.draining.store(true, Ordering::Relaxed);
    st.drain_completed.store(true, Ordering::Relaxed);
    st.shutdown_token.cancel();
    (StatusCode::OK, Json(json!({ "accepted": true }))).into_response()
}

#[tokio::test]
async fn mock_worker_child_entry() {
    let Ok(mode) = std::env::var("HAMETA_E2E_MOCK_MODE") else {
        // 正常测试执行时未注入环境变量，直接跳过
        return;
    };

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).unwrap();
    let bootstrap: WorkerBootstrap = serde_json::from_str(&raw).unwrap();

    if mode == "crash" {
        eprintln!(
            "MOCK_CRASH_SIGNATURE: simulated fatal failure for {}",
            bootstrap.instance_id
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
        std::process::exit(1);
    }

    let initial_version = bootstrap
        .filter_bundle
        .as_ref()
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    let shutdown = CancellationToken::new();
    let st = Arc::new(MockChildState {
        bootstrap: bootstrap.clone(),
        rule_version: Arc::new(AtomicI64::new(initial_version)),
        draining: Arc::new(AtomicBool::new(false)),
        drain_completed: Arc::new(AtomicBool::new(false)),
        shutdown_token: shutdown.clone(),
    });

    let router = Router::new()
        .route("/internal/v1/snapshot", get(mock_snapshot))
        .route("/internal/v1/rules/reload", post(mock_reload))
        .route("/internal/v1/drain-status", get(mock_drain_status))
        .route("/internal/v1/shutdown", post(mock_shutdown))
        .with_state(st);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let worker_port = listener.local_addr().unwrap().port();
    let srv_cancel = shutdown.clone();
    let srv_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move { srv_cancel.cancelled().await })
            .await;
    });

    let client = reqwest::Client::new();
    let reg_url = format!(
        "http://127.0.0.1:{}/internal/v1/register",
        bootstrap.panel_ipc_port
    );
    let reg_resp = client
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
    assert!(reg_resp.status().is_success());

    let ready_url = format!(
        "http://127.0.0.1:{}/internal/v1/ready",
        bootstrap.panel_ipc_port
    );
    let ready_resp = client
        .post(&ready_url)
        .bearer_auth(&bootstrap.ipc_token)
        .json(&WorkerReadyRequest {
            instance_id: bootstrap.instance_id.clone(),
            rule_version: Some(initial_version),
        })
        .send()
        .await
        .unwrap();
    assert!(ready_resp.status().is_success());

    shutdown.cancelled().await;
    let _ = srv_handle.await;
    std::process::exit(0);
}

async fn setup_base_db(data_dir: &std::path::Path) -> Database {
    std::fs::create_dir_all(data_dir).unwrap();
    let db = Database::open(&data_dir.join("hameta.db")).await.unwrap();
    let redis_json = serde_json::to_string(&RedisServiceConfig::default()).unwrap();
    db.save_service_config(ServiceKind::Redis, &redis_json, "test", "init")
        .await
        .unwrap();
    let manticore_json = serde_json::to_string(&ManticoreServiceConfig::default()).unwrap();
    db.save_service_config(ServiceKind::Manticore, &manticore_json, "test", "init")
        .await
        .unwrap();
    db
}

#[tokio::test]
async fn supervisor_manages_real_child_lifecycle_reload_and_graceful_stop() {
    let dir = std::env::temp_dir().join(format!("hameta-e2e-ok-{}", generate_token()));
    let db = setup_base_db(&dir).await;

    let idx_cfg = serde_json::to_string(&IndexerWorkerConfig::default()).unwrap();
    db.create_worker(
        "indexer-e2e",
        WorkerRole::Indexer,
        "E2E Indexer",
        &idx_cfg,
        "test",
    )
    .await
    .unwrap();
    db.set_worker_desired_state("indexer-e2e", DesiredState::Running)
        .await
        .unwrap();

    let exe = std::env::current_exe().unwrap();
    let factory = Arc::new(move |_bootstrap: &WorkerBootstrap| {
        Ok(SpawnSpec {
            program: exe.clone(),
            args: vec![
                "--exact".to_string(),
                "mock_worker_child_entry".to_string(),
                "--nocapture".to_string(),
            ],
            envs: vec![("HAMETA_E2E_MOCK_MODE".to_string(), "ok".to_string())],
        })
    });

    let sup = Supervisor::with_command_factory(db.clone(), dir.clone(), factory)
        .await
        .unwrap();
    sup.start_worker("indexer-e2e").await.unwrap();

    // 等待真实子进程读取 stdin Bootstrap、注册并上报 Ready
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ready_view = None;
    while Instant::now() < deadline {
        let list = sup.list_worker_statuses().await.unwrap();
        if let Some(view) = list.into_iter().find(|w| w.id == "indexer-e2e")
            && view.runtime_state == WorkerRuntimeState::Running
        {
            ready_view = Some(view);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let view = ready_view.expect("mock indexer worker did not become Running within timeout");
    assert!(view.pid.is_some());
    assert!(view.worker_port.is_some());

    // 主动拉取一次快照并校验
    sup.poll_snapshots_once().await;
    let snap = sup
        .get_worker_snapshot("indexer-e2e")
        .expect("missing snapshot after poll");
    assert_eq!(snap.instance_id, "indexer-e2e");
    assert_eq!(snap.payload["mock"], true);

    // 广播热更新规则版本到 v7
    let mut bundle = default_online_filter_bundle();
    bundle.version = 7;
    let reload_results = sup.reload_all_indexers(7, &bundle).await;
    assert_eq!(reload_results.len(), 1);
    assert!(reload_results[0].ok);
    assert_eq!(reload_results[0].active_rule_version, Some(7));

    // 触发优雅停机并等待退出
    db.set_worker_desired_state("indexer-e2e", DesiredState::Stopped)
        .await
        .unwrap();
    sup.stop_worker("indexer-e2e", 5).await.unwrap();

    let after = sup.list_worker_statuses().await.unwrap();
    let stopped_view = after.into_iter().find(|w| w.id == "indexer-e2e").unwrap();
    assert_eq!(stopped_view.runtime_state, WorkerRuntimeState::Stopped);
    assert_eq!(stopped_view.pid, None);
    assert_eq!(stopped_view.worker_port, None);

    sup.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn supervisor_auto_restarts_with_backoff_fuses_to_crashed_and_captures_stderr() {
    let dir = std::env::temp_dir().join(format!("hameta-e2e-crash-{}", generate_token()));
    let db = setup_base_db(&dir).await;

    let crawler_cfg = serde_json::to_string(&CrawlerWorkerConfig::default()).unwrap();
    db.create_worker(
        "crawler-crash",
        WorkerRole::Crawler,
        "Crash Crawler",
        &crawler_cfg,
        "test",
    )
    .await
    .unwrap();
    db.update_worker_policy("crawler-crash", "Crash Crawler", true, 2, 1)
        .await
        .unwrap();
    db.set_worker_desired_state("crawler-crash", DesiredState::Running)
        .await
        .unwrap();

    let exe = std::env::current_exe().unwrap();
    let factory = Arc::new(move |_bootstrap: &WorkerBootstrap| {
        Ok(SpawnSpec {
            program: exe.clone(),
            args: vec![
                "--exact".to_string(),
                "mock_worker_child_entry".to_string(),
                "--nocapture".to_string(),
            ],
            envs: vec![("HAMETA_E2E_MOCK_MODE".to_string(), "crash".to_string())],
        })
    });

    let sup = Supervisor::with_command_factory(db.clone(), dir.clone(), factory)
        .await
        .unwrap();
    sup.start_worker("crawler-crash").await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(12);
    let mut crashed_view = None;
    while Instant::now() < deadline {
        let list = sup.list_worker_statuses().await.unwrap();
        if let Some(view) = list.into_iter().find(|w| w.id == "crawler-crash")
            && view.runtime_state == WorkerRuntimeState::Crashed
        {
            crashed_view = Some(view);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let view = crashed_view.expect("worker did not fuse into Crashed state after retries");
    assert!(view.restart_count >= 1);
    assert_eq!(view.last_exit_code, Some(1));
    assert!(
        view.stderr_tail.contains("MOCK_CRASH_SIGNATURE"),
        "expected stderr capture, got: {}",
        view.stderr_tail
    );
    assert!(
        view.last_error
            .as_deref()
            .unwrap_or_default()
            .contains("MOCK_CRASH_SIGNATURE")
    );

    let audits = db.list_audits(5).await.unwrap();
    assert!(
        audits
            .iter()
            .any(|a| a.action == "worker.crashed" && a.target == "crawler-crash"),
        "expected worker.crashed audit log"
    );

    sup.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}
