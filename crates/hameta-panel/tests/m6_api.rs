use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router as AxumRouter};
use hameta_common::{
    FileInfo, IndexerWorkerConfig, ManticoreServiceConfig, RedisServiceConfig, ServiceKind,
    TorrentMessage, WorkerBootstrap, WorkerDrainStatus, WorkerReadyRequest, WorkerRegisterRequest,
    WorkerReloadRulesRequest, WorkerReloadRulesResponse, WorkerRole, WorkerRuntimeState,
    WorkerShutdownRequest, WorkerSnapshotEnvelope, generate_token, hash_password, hash_token,
    verify_bearer_token,
};
use hameta_filter::{
    AdPhraseRule, ExtensionScope, FilterBundle, FilterRuntime, KeywordMatchMode, PolicyAction,
    PolicyRule, RuleMatcher, TextTarget, default_online_filter_bundle,
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Clone)]
struct MockM6Indexer {
    bootstrap: WorkerBootstrap,
    filter_runtime: Arc<FilterRuntime>,
    draining: Arc<AtomicBool>,
    shutdown: CancellationToken,
}

fn mock_auth(headers: &HeaderMap, expected: &str) -> bool {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    verify_bearer_token(auth, expected)
}

async fn mock_snapshot(State(st): State<Arc<MockM6Indexer>>, headers: HeaderMap) -> Response {
    if !mock_auth(&headers, &st.bootstrap.ipc_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let filter_snap = st.filter_runtime.snapshot_stats(50);
    Json(WorkerSnapshotEnvelope {
        instance_id: st.bootstrap.instance_id.clone(),
        role: WorkerRole::Indexer,
        captured_at_unix: 1_700_000_000,
        payload: json!({
            "uptime_secs": 5,
            "active_rule_version": filter_snap.active_version,
            "rss_bytes": 1024 * 1024,
            "app": {},
            "filter": filter_snap
        }),
    })
    .into_response()
}

async fn mock_reload(
    State(st): State<Arc<MockM6Indexer>>,
    headers: HeaderMap,
    Json(req): Json<WorkerReloadRulesRequest>,
) -> Response {
    if !mock_auth(&headers, &st.bootstrap.ipc_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut bundle: FilterBundle = serde_json::from_value(req.filter_bundle).unwrap();
    bundle.version = req.version;
    match st.filter_runtime.reload(&bundle) {
        Ok(ver) => Json(WorkerReloadRulesResponse {
            active_rule_version: ver,
        })
        .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn mock_drain(State(st): State<Arc<MockM6Indexer>>, headers: HeaderMap) -> Response {
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
    State(st): State<Arc<MockM6Indexer>>,
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
async fn mock_m6_indexer_child() {
    let Ok(flag) = std::env::var("HAMETA_M6_MOCK_INDEXER") else {
        return;
    };
    if flag != "1" {
        return;
    }

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).unwrap();
    let bootstrap: WorkerBootstrap = serde_json::from_str(&raw).unwrap();

    let bundle: FilterBundle = bootstrap
        .filter_bundle
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_else(default_online_filter_bundle);
    let initial_version = bundle.version;
    let runtime = Arc::new(FilterRuntime::new(&bundle).unwrap());

    // 预跑一条放行样本和一条拦截样本以产生真实归因统计与最近拦截样本
    let ok_msg = TorrentMessage {
        info_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        name: "Ubuntu.24.04.LTS.Desktop.amd64".to_string(),
        total_size: 1024 * 1024 * 1024,
        file_count: 1,
        files: vec![FileInfo {
            path: "ubuntu-24.04-live.mkv".to_string(),
            size: 1024 * 1024 * 1024,
        }],
        timestamp: 0,
    };
    let bad_msg = TorrentMessage {
        info_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        name: "Super.Trojan.HackTool.Pack".to_string(),
        total_size: 20 * 1024 * 1024,
        file_count: 1,
        files: vec![FileInfo {
            path: "tool/readme.mkv".to_string(),
            size: 20 * 1024 * 1024,
        }],
        timestamp: 0,
    };
    let _ = runtime.evaluate_and_record(&ok_msg);
    let _ = runtime.evaluate_and_record(&bad_msg);

    let shutdown = CancellationToken::new();
    let st = Arc::new(MockM6Indexer {
        bootstrap: bootstrap.clone(),
        filter_runtime: runtime,
        draining: Arc::new(AtomicBool::new(false)),
        shutdown: shutdown.clone(),
    });
    let app = AxumRouter::new()
        .route("/internal/v1/snapshot", get(mock_snapshot))
        .route("/internal/v1/rules/reload", post(mock_reload))
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
            rule_version: Some(initial_version),
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

async fn setup_panel_with_mock_indexer() -> TestHarness {
    let dir = std::env::temp_dir().join(format!("hameta-m6-api-{}", generate_token()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(&dir.join("hameta.db")).await.unwrap();

    let pw_hash = hash_password("secret-pass-123").unwrap();
    let admin = db.create_admin("admin", &pw_hash).await.unwrap();
    let session_tok = format!("sess-{}", generate_token());
    db.create_session(
        &hash_token(&session_tok),
        admin.id,
        "127.0.0.1",
        "m6-test",
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
    db.publish_filter_draft("admin", Some("初始线上过滤基线"))
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
                "mock_m6_indexer_child".to_string(),
                "--nocapture".to_string(),
            ],
            envs: vec![("HAMETA_M6_MOCK_INDEXER".to_string(), "1".to_string())],
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
    let bytes = to_bytes(resp.into_body(), 4 * 1024 * 1024).await.unwrap();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

#[tokio::test]
async fn m6_filter_workbench_hot_reload_playground_stats_and_rollback() {
    let h = setup_panel_with_mock_indexer().await;

    // 启动 mock indexer 子进程并等待 Running
    let (st, start_res) = json_req(
        &h.app,
        "POST",
        "/api/v1/workers/indexer-1/start",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "start failed: {start_res}");

    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let statuses = h.state.supervisor.list_worker_statuses().await.unwrap();
        if let Some(w) = statuses.into_iter().find(|w| w.id == "indexer-1")
            && w.runtime_state == WorkerRuntimeState::Running
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "indexer-1 failed to reach Running"
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    // 1. 获取内置预设目录 & 初始过滤状态与在线 indexer PID/rule_version
    let (st, presets_res) =
        json_req(&h.app, "GET", "/api/v1/filter/presets", &h.cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(presets_res["policy_presets"].as_array().unwrap().len(), 4);
    assert!(!presets_res["rule_templates"].as_array().unwrap().is_empty());
    assert!(
        !presets_res["extension_packs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!presets_res["keyword_packs"].as_array().unwrap().is_empty());

    let (st, state_v1) = json_req(&h.app, "GET", "/api/v1/filter/state", &h.cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(state_v1["has_draft"], false);
    assert_eq!(state_v1["published"]["version"], 1);
    assert_eq!(
        state_v1["published"]["bundle"]["policy"]["preset_id"],
        "online_standard"
    );
    let pid_before = state_v1["online_indexers"][0]["pid"]
        .as_u64()
        .expect("indexer pid expected");
    assert_eq!(state_v1["online_indexers"][0]["active_rule_version"], 1);

    // 2. 携带非法正则保存草稿 -> 拒绝 400 INVALID_FILTER 且精确返回规则类别
    let mut bad_bundle: FilterBundle =
        serde_json::from_value(state_v1["published"]["bundle"].clone()).unwrap();
    bad_bundle.policy.rules.push(PolicyRule {
        id: "bad-regex-test".to_string(),
        name: "非法自定义正则".to_string(),
        enabled: true,
        action: PolicyAction::Reject,
        tags: vec![],
        note: String::new(),
        matcher: RuleMatcher::CustomRegex {
            target: TextTarget::RawName,
            match_all: false,
            patterns: vec!["[unclosed(".to_string()],
        },
    });
    let (st, bad_resp) = json_req(
        &h.app,
        "PUT",
        "/api/v1/filter/draft",
        &h.cookie,
        Some(json!({ "bundle": bad_bundle, "note": "bad regex" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(bad_resp["code"], "INVALID_FILTER");
    assert!(
        bad_resp["message"]
            .as_str()
            .unwrap()
            .contains("custom_regex"),
        "bad_resp={bad_resp}"
    );

    // 3. 插入高优先级白名单 Allow 规则 + 新增关键词组 + 新广告短语并保存草稿 (v2)
    let mut draft_bundle: FilterBundle =
        serde_json::from_value(state_v1["published"]["bundle"].clone()).unwrap();
    draft_bundle.policy.rules.insert(
        0,
        PolicyRule {
            id: "allow-trusted-iso".to_string(),
            name: "官方 Ubuntu 发行版白名单".to_string(),
            enabled: true,
            action: PolicyAction::Allow,
            tags: vec!["白名单".to_string()],
            note: "豁免 Ubuntu ISO".to_string(),
            matcher: RuleMatcher::KeywordGroup {
                target: TextTarget::RawName,
                match_mode: KeywordMatchMode::WordBoundary,
                case_sensitive: false,
                words: vec!["Ubuntu.Official.Allow".to_string()],
            },
        },
    );
    draft_bundle.policy.rules.push(PolicyRule {
        id: String::new(),
        name: "M6 自定义垃圾词".to_string(),
        enabled: true,
        action: PolicyAction::Reject,
        tags: vec![],
        note: "m6 test keyword".to_string(),
        matcher: RuleMatcher::KeywordGroup {
            target: TextTarget::RawName,
            match_mode: KeywordMatchMode::Contains,
            case_sensitive: false,
            words: vec!["m6_custom_spam".to_string()],
        },
    });
    draft_bundle.ad_phrases.push(AdPhraseRule {
        id: String::new(),
        phrase: "【测试广告】".to_string(),
        enabled: true,
        note: "m6 test phrase".to_string(),
    });

    let (st, saved_draft) = json_req(
        &h.app,
        "PUT",
        "/api/v1/filter/draft",
        &h.cookie,
        Some(json!({ "bundle": draft_bundle, "note": "M6 新增关键词与广告词" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "save draft failed: {saved_draft}");
    assert_eq!(saved_draft["version"], 2);
    assert_eq!(saved_draft["status"], "draft");

    // 4. 在线演练场 A/B 对比（含 Allow 白名单豁免原被 .iso 拦截的样本）
    let (st, pg_res) = json_req(
        &h.app,
        "POST",
        "/api/v1/filter/playground/test",
        &h.cookie,
        Some(json!({
            "mode": "ab_compare",
            "samples": [
                { "name": "Awesome.Release.2026.m6_custom_spam.1080p" },
                { "name": "【测试广告】Clean.Documentary.2026.1080p" },
                { "name": "Ubuntu.24.04.LTS.Server" },
                { "name": "Ubuntu.Official.Allow.24.04.iso" }
            ]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "playground failed: {pg_res}");
    assert_eq!(pg_res["ab_report"]["summary"]["total"], 4);
    assert_eq!(pg_res["ab_report"]["summary"]["newly_rejected"], 1);
    assert_eq!(pg_res["ab_report"]["summary"]["refined_name_changed"], 1);
    assert_eq!(pg_res["ab_report"]["summary"]["unchanged_accepted"], 1);
    assert_eq!(pg_res["ab_report"]["summary"]["newly_accepted"], 1);
    assert_eq!(
        pg_res["results"][3]["allowed_by"]["rule_id"],
        "allow-trusted-iso"
    );
    assert!(
        !pg_res["results"][1]["cleaning_trace"]["steps"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // 5. 发布并触发无停机 IPC 热更新 -> 校验 PID 未变而 active_rule_version 升至 v2
    let (st, pub_res) = json_req(
        &h.app,
        "POST",
        "/api/v1/filter/publish",
        &h.cookie,
        Some(json!({ "note": "发布 v2 热更新" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "publish failed: {pub_res}");
    assert_eq!(pub_res["published"]["version"], 2);
    assert_eq!(pub_res["reload_results"][0]["ok"], true);
    assert_eq!(pub_res["reload_results"][0]["active_rule_version"], 2);

    let (st, state_v2) = json_req(&h.app, "GET", "/api/v1/filter/state", &h.cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(state_v2["has_draft"], false);
    assert_eq!(state_v2["published"]["version"], 2);
    assert_eq!(state_v2["online_indexers"][0]["pid"], pid_before);
    assert_eq!(state_v2["online_indexers"][0]["active_rule_version"], 2);

    // 6. 拉取快照并核验规则归因排行与最近拦截样本及沙盒回采
    h.state.supervisor.poll_snapshots_once().await;
    let (st, stats) = json_req(&h.app, "GET", "/api/v1/filter/stats", &h.cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(stats["total_evaluated"], 2);
    assert_eq!(stats["total_accepted"], 1);
    assert_eq!(stats["total_rejected"], 1);
    assert_eq!(stats["rule_hits"].as_array().unwrap().len(), 1);
    assert_eq!(stats["recent_rejected"].as_array().unwrap().len(), 1);

    let (st, sampled) = json_req(
        &h.app,
        "POST",
        "/api/v1/filter/playground/sample",
        &h.cookie,
        Some(json!({ "source": "rejected_recent", "limit": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sampled["samples"].as_array().unwrap().len(), 1);
    assert!(
        sampled["samples"][0]["name"]
            .as_str()
            .unwrap()
            .contains("Trojan")
    );

    // 7. 旧版配置智能转译导入（带 filter 包裹 + 后缀正则转译为 ExtensionRule）及多预设切换与草稿丢弃
    let wrapped_legacy = json!({
        "redis": { "url": "redis://127.0.0.1:6379" },
        "filter": {
            "blocked_rules": ["wrapped_only_kw"],
            "blocked_file_patterns": ["(?i)\\.(exe|bat)$"],
            "limits": { "min_name_length": 4, "max_name_length": 128 }
        }
    })
    .to_string();
    let (st, imported) = json_req(
        &h.app,
        "POST",
        "/api/v1/filter/import-legacy",
        &h.cookie,
        Some(json!({ "raw_json": wrapped_legacy, "save_as_draft": true })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "import wrapped failed: {imported}");
    assert_eq!(imported["source_format"], "legacy_config");
    let imported_rules = imported["bundle"]["policy"]["rules"].as_array().unwrap();
    assert!(imported_rules.iter().any(|r| {
        r["matcher"]["type"] == "keyword_group" && r["matcher"]["words"][0] == "wrapped_only_kw"
    }));
    assert!(imported_rules.iter().any(|r| {
        r["matcher"]["type"] == "extension_rule"
            && r["matcher"]["scope"] == "any_file"
            && r["matcher"]["extensions"] == json!(["exe", "bat"])
    }));
    // 验证 ExtensionScope 默认序列化对齐
    let _ = ExtensionScope::AnyFile;

    let (st, reset_media) = json_req(
        &h.app,
        "POST",
        "/api/v1/filter/reset-preset",
        &h.cookie,
        Some(json!({ "preset_id": "media_focused", "include_dictionary": true })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "reset media_focused failed: {reset_media}"
    );
    assert_eq!(
        reset_media["bundle"]["policy"]["preset_id"],
        "media_focused"
    );

    let (st, reset_res) = json_req(
        &h.app,
        "POST",
        "/api/v1/filter/reset-preset",
        &h.cookie,
        Some(json!({ "preset_id": "online_standard", "include_dictionary": true })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        reset_res["bundle"]["policy"]["preset_id"],
        "online_standard"
    );
    assert!(
        reset_res["bundle"]["policy"]["rules"]
            .as_array()
            .unwrap()
            .len()
            >= 14
    );

    let (st, disc_res) = json_req(&h.app, "DELETE", "/api/v1/filter/draft", &h.cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(disc_res["discarded"], true);

    // 8. 版本历史与一键回滚至 v1（生成 v3 并无停机热更新生效）
    let (st, versions) = json_req(
        &h.app,
        "GET",
        "/api/v1/filter/versions?limit=10",
        &h.cookie,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(versions.as_array().unwrap().len(), 2);

    let (st, rolled) = json_req(
        &h.app,
        "POST",
        "/api/v1/filter/rollback",
        &h.cookie,
        Some(json!({ "target_version": 1 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "rollback failed: {rolled}");
    assert_eq!(rolled["published"]["version"], 3);
    assert_eq!(rolled["reload_results"][0]["ok"], true);
    assert_eq!(rolled["reload_results"][0]["active_rule_version"], 3);

    let (st, state_v3) = json_req(&h.app, "GET", "/api/v1/filter/state", &h.cookie, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(state_v3["online_indexers"][0]["pid"], pid_before);
    assert_eq!(state_v3["online_indexers"][0]["active_rule_version"], 3);

    h.state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&h.dir);
}
