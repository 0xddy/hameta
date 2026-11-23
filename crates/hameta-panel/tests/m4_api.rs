use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router as AxumRouter};
use hameta_common::{
    DesiredState, ManticoreServiceConfig, RedisServiceConfig, WorkerBootstrap, WorkerDrainStatus,
    WorkerReadyRequest, WorkerRegisterRequest, WorkerRuntimeState, WorkerShutdownRequest,
    generate_token, verify_bearer_token,
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
struct MockChild {
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

async fn mock_drain(State(st): State<Arc<MockChild>>, headers: HeaderMap) -> Response {
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
    State(st): State<Arc<MockChild>>,
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
async fn mock_m4_worker_child() {
    let Ok(flag) = std::env::var("HAMETA_M4_MOCK_CHILD") else {
        return;
    };
    if flag != "1" {
        return;
    }

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).unwrap();
    let bootstrap: WorkerBootstrap = serde_json::from_str(&raw).unwrap();

    let shutdown = CancellationToken::new();
    let st = Arc::new(MockChild {
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

    shutdown.cancelled().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    std::process::exit(0);
}

async fn make_app(with_mock_workers: bool) -> (Router, Arc<PanelState>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("hameta-m4-api-{}", generate_token()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(&dir.join("hameta.db")).await.unwrap();

    let supervisor = if with_mock_workers {
        let exe = std::env::current_exe().unwrap();
        let factory = Arc::new(move |_: &WorkerBootstrap| {
            Ok(SpawnSpec {
                program: exe.clone(),
                args: vec![
                    "--exact".to_string(),
                    "mock_m4_worker_child".to_string(),
                    "--nocapture".to_string(),
                ],
                envs: vec![("HAMETA_M4_MOCK_CHILD".to_string(), "1".to_string())],
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
    let torrent_tasks = hameta_panel::TorrentTaskManager::new(db.clone(), services.clone());
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
        torrent_tasks,
        login_limiter: Arc::new(LoginRateLimiter::default()),
        setup_lock: Arc::new(Mutex::new(())),
    });
    let router = build_router(state.clone());
    (router, state, dir)
}

fn extract_cookie_pair(resp: &axum::response::Response) -> Option<String> {
    let header = resp.headers().get(SET_COOKIE)?.to_str().ok()?;
    let pair = header.split(';').next()?.trim().to_string();
    Some(pair)
}

async fn send_json(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, Option<String>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(COOKIE, c);
    }
    let req_body = if let Some(val) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(serde_json::to_vec(&val).unwrap())
    } else {
        Body::empty()
    };
    let resp = app
        .clone()
        .oneshot(builder.body(req_body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let set_cookie = extract_cookie_pair(&resp);
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, set_cookie)
}

#[tokio::test]
async fn setup_wizard_auth_rate_limit_and_password_rotation_work_end_to_end() {
    let (app, state, dir) = make_app(false).await;

    // 未初始化时 status=false，受保护接口拒绝
    let (st, body, _) = send_json(&app, "GET", "/api/v1/setup/status", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["initialized"], false);

    let (st, _, _) = send_json(&app, "GET", "/api/v1/settings/services", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // 非法旧规则 JSON 拒绝且不会标记为已初始化
    let mut bad_init = json!({
        "admin_username": "admin",
        "admin_password": "secret_pass_1",
        "redis": RedisServiceConfig::default(),
        "manticore": ManticoreServiceConfig::default(),
        "legacy_filter_json": r#"{"filter":{"blocked_name_patterns":["([unclosed"]}}"#,
        "create_default_workers": true
    });
    let (st, err_body, _) = send_json(
        &app,
        "POST",
        "/api/v1/setup/init",
        None,
        Some(bad_init.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err_body["code"], "INVALID_FILTER");

    // 传入合法旧版 config.json 片段，成功初始化并自动签发登录 Cookie
    bad_init["legacy_filter_json"] = Value::String(
        r#"{"filter":{"blocked_rules":["trojan"],"limits":{"min_name_length":4,"max_name_length":128}}}"#.to_string(),
    );
    let (st, ok_body, init_cookie) =
        send_json(&app, "POST", "/api/v1/setup/init", None, Some(bad_init)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ok_body["initialized"], true);
    assert_eq!(ok_body["username"], "admin");
    assert_eq!(ok_body["created_workers"].as_array().unwrap().len(), 2);
    let cookie = init_cookie.expect("setup/init must issue session cookie");

    // 初始化完成后再请求 /setup/* 必须返回 403 锁定
    let (st, lock_body, _) =
        send_json(&app, "POST", "/api/v1/setup/init", None, Some(json!({}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(lock_body["code"], "ALREADY_INITIALIZED");

    // 当前自动签发的会话可直接访问 /auth/me
    let (st, me_body, _) = send_json(&app, "GET", "/api/v1/auth/me", Some(&cookie), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_body["user"]["username"], "admin");

    // 登出后 Cookie 失效
    let (st, _, _) = send_json(&app, "POST", "/api/v1/auth/logout", Some(&cookie), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = send_json(&app, "GET", "/api/v1/auth/me", Some(&cookie), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // 同 IP 连续 5 次错误密码后第 6 次触发 429 限流
    for _ in 0..5 {
        let (st, _, _) = send_json(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({"username": "admin", "password": "wrong_password"})),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    let (st, rate_body, _) = send_json(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({"username": "admin", "password": "secret_pass_1"})),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(rate_body["code"], "RATE_LIMITED");

    // 清除限流窗口状态（或换 X-Forwarded-For IP）验证正确登录与修改密码
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "10.9.9.9")
        .body(Body::from(
            serde_json::to_vec(&json!({"username": "admin", "password": "secret_pass_1"})).unwrap(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let login_cookie = extract_cookie_pair(&resp).unwrap();

    // 在线修改密码后，旧密码失效、新密码可登录
    let (st, _, rotated_cookie) = send_json(
        &app,
        "POST",
        "/api/v1/auth/password",
        Some(&login_cookie),
        Some(json!({"current_password": "secret_pass_1", "new_password": "new_secret_99"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(rotated_cookie.is_some());

    req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header("content-type", "application/json")
        .header("x-forwarded-for", "10.9.9.10")
        .body(Body::from(
            serde_json::to_vec(&json!({"username": "admin", "password": "new_secret_99"})).unwrap(),
        ))
        .unwrap();
    let resp2 = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);

    state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn service_settings_echo_all_fields_rollback_and_restart_affected_workers() {
    let (app, state, dir) = make_app(true).await;

    let init_redis = RedisServiceConfig {
        password: "redis_demo_pw_1".to_string(),
        ..RedisServiceConfig::default()
    };
    let init_manticore = ManticoreServiceConfig {
        username: "mc_user".to_string(),
        password: "mc_demo_pw_1".to_string(),
        ..ManticoreServiceConfig::default()
    };

    let (st, _, cookie_opt) = send_json(
        &app,
        "POST",
        "/api/v1/setup/init",
        None,
        Some(json!({
            "admin_username": "ops",
            "admin_password": "ops_password_123",
            "redis": init_redis,
            "manticore": init_manticore,
            "create_default_workers": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cookie = cookie_opt.unwrap();

    // 验证配置全部字段（含密码）完整回显
    let (st, svc_body, _) = send_json(
        &app,
        "GET",
        "/api/v1/settings/services",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(svc_body["redis"]["config"]["password"], "redis_demo_pw_1");
    assert_eq!(svc_body["manticore"]["config"]["auth_mode"], "basic");
    assert_eq!(svc_body["manticore"]["config"]["username"], "mc_user");
    assert_eq!(svc_body["manticore"]["config"]["password"], "mc_demo_pw_1");

    // 验证禁止修改 Manticore 表名 & 单边 Basic Auth
    let (st, bad_table, _) = send_json(
        &app,
        "PUT",
        "/api/v1/settings/manticore",
        Some(&cookie),
        Some(json!({
            "config": { "url": "http://127.0.0.1:9308", "table_name": "other_table" },
            "note": "bad"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(bad_table["message"].as_str().unwrap().contains("torrents"));

    let (st, _, _) = send_json(
        &app,
        "PUT",
        "/api/v1/settings/manticore",
        Some(&cookie),
        Some(json!({
            "config": { "url": "http://127.0.0.1:9308", "username": "only_user", "password": "" }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 启动两个 mock Worker，验证变更 Manticore 仅重启 indexer-1，变更 Redis 同时重启两者
    state
        .db
        .set_worker_desired_state("crawler-1", DesiredState::Running)
        .await
        .unwrap();
    state
        .db
        .set_worker_desired_state("indexer-1", DesiredState::Running)
        .await
        .unwrap();
    state.supervisor.start_worker("crawler-1").await.unwrap();
    state.supervisor.start_worker("indexer-1").await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let list = state.supervisor.list_worker_statuses().await.unwrap();
        let c_ok = list
            .iter()
            .any(|w| w.id == "crawler-1" && w.runtime_state == WorkerRuntimeState::Running);
        let i_ok = list
            .iter()
            .any(|w| w.id == "indexer-1" && w.runtime_state == WorkerRuntimeState::Running);
        if c_ok && i_ok {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "mock workers did not become Running"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 修改 Manticore 并启用联动滚动重启 -> 仅影响并重启 indexer-1
    let (st, upd_mc, _) = send_json(
        &app,
        "PUT",
        "/api/v1/settings/manticore",
        Some(&cookie),
        Some(json!({
            "config": {
                "url": "http://127.0.0.1:9308",
                "username": "mc_user",
                "password": "mc_demo_pw_2",
                "table_name": "torrents",
                "request_timeout_secs": 15,
                "max_connections": 640
            },
            "note": "调整连接数",
            "restart_affected_workers": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(upd_mc["version"], 2);
    let mc_restarted = upd_mc["restarted_workers"].as_array().unwrap();
    assert_eq!(mc_restarted.len(), 1);
    assert_eq!(mc_restarted[0]["id"], "indexer-1");
    assert_eq!(mc_restarted[0]["ok"], true);

    // 等待 indexer-1 重新成为 Running
    let d2 = Instant::now() + Duration::from_secs(10);
    while Instant::now() < d2 {
        let list = state.supervisor.list_worker_statuses().await.unwrap();
        if list
            .iter()
            .any(|w| w.id == "indexer-1" && w.runtime_state == WorkerRuntimeState::Running)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // 修改 Redis 并启用联动滚动重启 -> 同时影响 crawler-1 和 indexer-1
    let next_redis = RedisServiceConfig {
        password: "redis_demo_pw_2".to_string(),
        ..RedisServiceConfig::default()
    };
    let (st, upd_rd, _) = send_json(
        &app,
        "PUT",
        "/api/v1/settings/redis",
        Some(&cookie),
        Some(json!({
            "config": next_redis,
            "note": "更换 Redis 密码",
            "restart_affected_workers": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(upd_rd["version"], 2);
    let rd_restarted = upd_rd["restarted_workers"].as_array().unwrap();
    assert_eq!(rd_restarted.len(), 2);
    assert!(rd_restarted.iter().all(|w| w["ok"] == true));

    // 回滚 Redis 到 v1，验证版本号变为 v3 且密码回到 v1
    let (st, rb, _) = send_json(
        &app,
        "POST",
        "/api/v1/settings/rollback",
        Some(&cookie),
        Some(json!({
            "kind": "redis",
            "target_version": 1,
            "restart_affected_workers": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rb["version"], 3);
    assert_eq!(rb["config"]["password"], "redis_demo_pw_1");

    // 查询历史列表验证版本完整记录
    let (st, hist, _) = send_json(
        &app,
        "GET",
        "/api/v1/settings/history?kind=redis&limit=10",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hist.as_array().unwrap().len(), 3);

    // 检查审计日志绝不泄露密码明文
    let audits = state.db.list_audits(20).await.unwrap();
    for a in &audits {
        assert!(!a.detail_json.contains("redis_demo_pw"));
        assert!(!a.detail_json.contains("mc_demo_pw"));
    }

    state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn manticore_reverse_proxy_auth_modes_settings_and_audit() {
    let (app, state, dir) = make_app(false).await;

    let init_manticore = ManticoreServiceConfig {
        url: "https://proxy.example.com/manticore".to_string(),
        username: "legacy_user".to_string(),
        password: "legacy_secret_pw".to_string(),
        ..ManticoreServiceConfig::default()
    };

    let (st, _, cookie_opt) = send_json(
        &app,
        "POST",
        "/api/v1/setup/init",
        None,
        Some(json!({
            "admin_username": "ops",
            "admin_password": "ops_password_123",
            "redis": RedisServiceConfig::default(),
            "manticore": init_manticore,
            "create_default_workers": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cookie = cookie_opt.unwrap();

    // 旧版仅填 username/password 应自动归一化为 basic
    let (st, svc_body, _) = send_json(
        &app,
        "GET",
        "/api/v1/settings/services",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(svc_body["manticore"]["config"]["auth_mode"], "basic");
    assert_eq!(svc_body["manticore"]["config"]["username"], "legacy_user");

    // 切换为 Bearer Token 鉴权（含 Nginx Bearer 令牌）
    let (st, upd_bearer, _) = send_json(
        &app,
        "PUT",
        "/api/v1/settings/manticore",
        Some(&cookie),
        Some(json!({
            "config": {
                "url": "https://proxy.example.com/manticore",
                "auth_mode": "bearer",
                "bearer_token": "tok_secret_live_1",
                "table_name": "torrents",
                "request_timeout_secs": 10,
                "max_connections": 1280
            },
            "note": "Nginx Bearer 反向代理"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(upd_bearer["config"]["auth_mode"], "bearer");
    assert_eq!(upd_bearer["config"]["bearer_token"], "tok_secret_live_1");

    // 切换为自定义请求头 API Key（如 X-API-Key）
    let (st, upd_hdr, _) = send_json(
        &app,
        "PUT",
        "/api/v1/settings/manticore",
        Some(&cookie),
        Some(json!({
            "config": {
                "url": "https://proxy.example.com/manticore",
                "auth_mode": "header",
                "header_name": "X-API-Key",
                "header_value": "hdr_secret_value_2",
                "table_name": "torrents",
                "request_timeout_secs": 10,
                "max_connections": 1280
            },
            "note": "Nginx Header 反向代理"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(upd_hdr["config"]["auth_mode"], "header");
    assert_eq!(upd_hdr["config"]["header_name"], "X-API-Key");
    assert_eq!(upd_hdr["config"]["header_value"], "hdr_secret_value_2");

    let (st, svc_after, _) = send_json(
        &app,
        "GET",
        "/api/v1/settings/services",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(svc_after["manticore"]["config"]["auth_mode"], "header");
    assert_eq!(svc_after["manticore"]["config"]["header_name"], "X-API-Key");
    assert_eq!(
        svc_after["manticore"]["config"]["header_value"],
        "hdr_secret_value_2"
    );

    // 拒绝空 Bearer Token 与非法 Header 名
    let (st, _, _) = send_json(
        &app,
        "PUT",
        "/api/v1/settings/manticore",
        Some(&cookie),
        Some(json!({
            "config": {
                "url": "https://proxy.example.com/manticore",
                "auth_mode": "bearer",
                "bearer_token": "   "
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, _, _) = send_json(
        &app,
        "PUT",
        "/api/v1/settings/manticore",
        Some(&cookie),
        Some(json!({
            "config": {
                "url": "https://proxy.example.com/manticore",
                "auth_mode": "header",
                "header_name": "Bad Header",
                "header_value": "v"
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 验证审计日志包含 auth_mode / header_name 且绝不泄露任何密钥明文
    let audits = state.db.list_audits(20).await.unwrap();
    let update_audits: Vec<&str> = audits
        .iter()
        .filter(|a| a.action == "settings.manticore.update")
        .map(|a| a.detail_json.as_str())
        .collect();
    assert!(update_audits.iter().any(|d| d.contains("\"bearer\"")));
    assert!(
        update_audits
            .iter()
            .any(|d| d.contains("\"header\"") && d.contains("X-API-Key"))
    );
    for a in &audits {
        assert!(!a.detail_json.contains("legacy_secret_pw"));
        assert!(!a.detail_json.contains("tok_secret_live_1"));
        assert!(!a.detail_json.contains("hdr_secret_value_2"));
    }

    state.supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}
