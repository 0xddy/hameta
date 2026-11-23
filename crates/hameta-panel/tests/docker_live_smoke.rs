use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use dht_crawler::prelude::{FileInfo as DhtFileInfo, TorrentInfo};
use hameta_common::{ManticoreServiceConfig, RedisServiceConfig, TorrentRecord, generate_token};
use hameta_crawler::redis_streams::{
    ClaimDecision, MetadataClaim, PublishOutcome, RedisStreamsClient,
};
use hameta_filter::{FilterOutcome, FilterRuntime, default_online_filter_bundle};
use hameta_indexer::{ManticoreClient, RedisStreamsConsumer};
use hameta_panel::{
    LoginRateLimiter, MetricsCollector, PanelConfig, PanelState, ServiceRuntimePool, Supervisor,
    build_router,
};
use hameta_storage::Database;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower::ServiceExt;

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
    let cookie_out = resp
        .headers()
        .get(SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .map(|s| s.trim().to_string());
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, cookie_out)
}

#[tokio::test]
#[ignore = "需要通过环境变量 HAMETA_TEST_REDIS_URL 与 HAMETA_TEST_MANTICORE_URL 指定临时测试容器"]
async fn docker_live_setup_settings_and_crawler_to_manticore_pipeline() {
    let redis_url = std::env::var("HAMETA_TEST_REDIS_URL").expect("missing HAMETA_TEST_REDIS_URL");
    let manticore_url =
        std::env::var("HAMETA_TEST_MANTICORE_URL").expect("missing HAMETA_TEST_MANTICORE_URL");

    let dir = std::env::temp_dir().join(format!("hameta-docker-smoke-{}", generate_token()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(&dir.join("hameta.db")).await.unwrap();
    let supervisor = Supervisor::new(db.clone(), dir.clone()).await.unwrap();
    let services = Arc::new(ServiceRuntimePool::default());
    let metrics = MetricsCollector::new(db.clone(), supervisor.clone(), services.clone());
    let torrent_tasks = hameta_panel::TorrentTaskManager::new(db.clone(), services.clone());
    let state = Arc::new(PanelState {
        db: db.clone(),
        config: PanelConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: dir.clone(),
            dev_mode: false,
        },
        supervisor: supervisor.clone(),
        services,
        metrics,
        torrent_tasks,
        login_limiter: Arc::new(LoginRateLimiter::default()),
        setup_lock: Arc::new(Mutex::new(())),
    });
    let app = build_router(state.clone());

    let redis_cfg = RedisServiceConfig {
        url: redis_url.clone(),
        indexed_key_prefix: "metadata:indexed:".to_string(),
        torrent_stream: "hameta:stream:torrent:new".to_string(),
        hot_stream: "hameta:stream:torrent:hot".to_string(),
        consumer_group: "hameta:cg:indexer".to_string(),
        ..RedisServiceConfig::default()
    };
    let manticore_cfg = ManticoreServiceConfig {
        url: manticore_url,
        ..ManticoreServiceConfig::default()
    };

    // 1. 测试真实 Redis 连通 + 版本 + 读写探测
    let (st, rd_report, _) = send_json(
        &app,
        "POST",
        "/api/v1/setup/test-redis",
        None,
        Some(json!({ "redis": redis_cfg })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rd_report["ok"], true, "redis test failed: {rd_report}");
    assert_eq!(rd_report["streams_supported"], true);
    assert_eq!(rd_report["xautoclaim_supported"], true);
    assert_eq!(rd_report["rw_probe_ok"], true);

    // 2. 测试真实 Manticore 空库状态 -> 显式建表 -> 重复建表返回 409 -> DESCRIBE 9 列兼容通过
    let (st, mc_before, _) = send_json(
        &app,
        "POST",
        "/api/v1/setup/test-manticore",
        None,
        Some(json!({ "manticore": manticore_cfg })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mc_before["ok"], false);
    assert_eq!(mc_before["schema"]["table_exists"], false);

    let (st, init_tbl, _) = send_json(
        &app,
        "POST",
        "/api/v1/setup/init-manticore-table",
        None,
        Some(json!({ "manticore": manticore_cfg })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "init table failed: {init_tbl}");

    let (st, conflict_tbl, _) = send_json(
        &app,
        "POST",
        "/api/v1/setup/init-manticore-table",
        None,
        Some(json!({ "manticore": manticore_cfg })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "expected 409: {conflict_tbl}");

    let (st, mc_after, _) = send_json(
        &app,
        "POST",
        "/api/v1/setup/test-manticore",
        None,
        Some(json!({ "manticore": manticore_cfg })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mc_after["ok"], true, "manticore test failed: {mc_after}");
    assert_eq!(mc_after["schema"]["compatible"], true);
    assert_eq!(mc_after["schema"]["columns"].as_array().unwrap().len(), 9);

    // 3. 完成初始化向导 & 登录后复测 Settings 接口
    let (st, _, cookie_opt) = send_json(
        &app,
        "POST",
        "/api/v1/setup/init",
        None,
        Some(json!({
            "admin_username": "admin",
            "admin_password": "docker_test_password",
            "redis": redis_cfg,
            "manticore": manticore_cfg,
            "create_default_workers": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let cookie = cookie_opt.unwrap();

    let (st, rd_settings_test, _) = send_json(
        &app,
        "POST",
        "/api/v1/settings/redis/test",
        Some(&cookie),
        Some(json!({ "config": null })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(rd_settings_test["ok"], true);

    let (st, mc_settings_test, _) = send_json(
        &app,
        "POST",
        "/api/v1/settings/manticore/test",
        Some(&cookie),
        Some(json!({ "config": null })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(mc_settings_test["ok"], true);

    // 4. 真实贯通 Crawler -> Redis Streams -> FilterRuntime -> Manticore /bulk -> metadata:indexed -> Hot update
    let crawler_redis = RedisStreamsClient::new(&redis_cfg, 3600, 60).await.unwrap();
    let valid_hash = "0123456789abcdef0123456789abcdef01234567";
    let blocked_hash = "fedcba9876543210fedcba9876543210fedcba98";

    let c1 = match crawler_redis
        .claim_metadata(valid_hash, MetadataClaim::new(false))
        .await
        .unwrap()
    {
        ClaimDecision::Acquired(c) => c,
        other => panic!("unexpected claim decision: {other:?}"),
    };
    let c2 = match crawler_redis
        .claim_metadata(blocked_hash, MetadataClaim::new(false))
        .await
        .unwrap()
    {
        ClaimDecision::Acquired(c) => c,
        other => panic!("unexpected claim decision: {other:?}"),
    };

    let t_valid = TorrentInfo {
        info_hash: valid_hash.to_string(),
        magnet_link: format!("magnet:?xt=urn:btih:{valid_hash}"),
        name: "【高清首发】Ubuntu.24.04.LTS.Desktop.amd64[example.com]".to_string(),
        total_size: 4_500_000_000,
        files: vec![DhtFileInfo {
            path: "ubuntu-24.04-desktop-amd64.mkv".to_string(),
            size: 4_500_000_000,
        }],
        piece_length: 262_144,
        peers: vec![],
        timestamp: 1_784_428_800,
    };
    let t_blocked = TorrentInfo {
        info_hash: blocked_hash.to_string(),
        magnet_link: format!("magnet:?xt=urn:btih:{blocked_hash}"),
        name: "Windows.11.Crack.Keygen.Activator.exe".to_string(),
        total_size: 20_000_000,
        files: vec![DhtFileInfo {
            path: "activator.exe".to_string(),
            size: 20_000_000,
        }],
        piece_length: 262_144,
        peers: vec![],
        timestamp: 1_784_428_801,
    };

    assert_eq!(
        crawler_redis.push_torrent(&t_valid, &c1).await.unwrap(),
        PublishOutcome::Submitted
    );
    assert_eq!(
        crawler_redis.push_torrent(&t_blocked, &c2).await.unwrap(),
        PublishOutcome::Submitted
    );

    let indexer_streams = RedisStreamsConsumer::new(&redis_cfg).await.unwrap();
    indexer_streams.ensure_consumer_groups().await.unwrap();

    let manticore_client = ManticoreClient::new(&manticore_cfg).unwrap();
    let filter_runtime = FilterRuntime::new(&default_online_filter_bundle()).unwrap();

    let batch = indexer_streams
        .read_torrent_batch("smoke-consumer-1", 10, 1000)
        .await
        .unwrap();
    assert_eq!(batch.valid.len(), 2);

    let mut ack_ids = Vec::new();
    let mut records_to_write = Vec::new();
    let mut hashes_to_mark = Vec::new();

    for entry in batch.valid {
        match filter_runtime.evaluate_and_record(&entry.message) {
            FilterOutcome::Rejected { .. } => {
                assert_eq!(entry.message.info_hash, blocked_hash);
                ack_ids.push(entry.entry_id);
            }
            FilterOutcome::Accepted { refined_name, .. } => {
                assert_eq!(entry.message.info_hash, valid_hash);
                assert!(!refined_name.contains("example.com"));
                let now = chrono::Utc::now();
                hashes_to_mark.push(entry.message.info_hash.clone());
                ack_ids.push(entry.entry_id);
                records_to_write.push(TorrentRecord {
                    info_hash: entry.message.info_hash,
                    raw_name: entry.message.name,
                    refine_name: Some(refined_name),
                    file_count: i32::try_from(entry.message.file_count).unwrap(),
                    file_info: entry.message.files,
                    hot: 1,
                    created_at: now,
                    updated_at: now,
                });
            }
        }
    }
    assert_eq!(records_to_write.len(), 1);
    assert_eq!(
        manticore_client
            .batch_insert(&records_to_write)
            .await
            .unwrap(),
        1
    );
    indexer_streams
        .mark_indexed_and_clear_pending(&hashes_to_mark)
        .await
        .unwrap();
    indexer_streams
        .xack_batch(indexer_streams.torrent_stream(), &ack_ids)
        .await
        .unwrap();

    // 验证 metadata:indexed:<hash> 已持久化且无 TTL，pending 锁已释放
    let raw_client = redis::Client::open(redis_url).unwrap();
    let mut raw_conn = raw_client.get_multiplexed_async_connection().await.unwrap();
    let indexed_val: String = redis::cmd("GET")
        .arg(format!("metadata:indexed:{valid_hash}"))
        .query_async(&mut raw_conn)
        .await
        .unwrap();
    let indexed_ttl: i64 = redis::cmd("TTL")
        .arg(format!("metadata:indexed:{valid_hash}"))
        .query_async(&mut raw_conn)
        .await
        .unwrap();
    let pending_exists: i64 = redis::cmd("EXISTS")
        .arg(format!("hameta:lock:pending:{valid_hash}"))
        .query_async(&mut raw_conn)
        .await
        .unwrap();
    assert_eq!(indexed_val, "1");
    assert_eq!(indexed_ttl, -1);
    assert_eq!(pending_exists, 0);

    // 验证 Hot 增量流更新 Manticore
    crawler_redis
        .push_hot(valid_hash, 1_784_428_900)
        .await
        .unwrap();
    let hot_batch = indexer_streams
        .read_hot_batch("smoke-hot-1", 10, 1000)
        .await
        .unwrap();
    assert_eq!(hot_batch.valid.len(), 1);
    let mut hot_updates = HashMap::new();
    hot_updates.insert(hot_batch.valid[0].info_hash.clone(), 1);
    assert_eq!(
        manticore_client
            .batch_update_hot(&hot_updates)
            .await
            .unwrap(),
        1
    );
    indexer_streams
        .xack_batch(
            indexer_streams.hot_stream(),
            &[hot_batch.valid[0].entry_id.clone()],
        )
        .await
        .unwrap();

    let sql_res = manticore_client
        .execute_raw_sql(&format!(
            "SELECT id, info_hash, raw_name, refine_name, file_count, file_info, hot FROM torrents WHERE info_hash = '{valid_hash}'"
        ))
        .await
        .unwrap();
    let row = &sql_res[0]["data"][0];
    assert_eq!(row["info_hash"], valid_hash);
    assert_eq!(row["raw_name"], t_valid.name);
    assert!(row["refine_name"].as_str().unwrap().contains("Ubuntu"));
    assert_eq!(row["file_count"], 1);
    assert!(
        row["file_info"]
            .to_string()
            .contains("ubuntu-24.04-desktop-amd64.mkv")
    );
    assert_eq!(row["hot"], 2);

    // 5. 验证 M5 实时总览快照（真实连通 Redis & Manticore）
    let (st, snap, _) = send_json(
        &app,
        "GET",
        "/api/v1/overview/snapshot",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(snap["health"]["panel_ok"], true);
    assert_eq!(snap["health"]["redis_ok"], true, "snap={snap}");
    assert_eq!(snap["health"]["manticore_ok"], true, "snap={snap}");
    assert!(
        snap["kpi"]["torrents_total_docs"].as_u64().is_some(),
        "expected torrents_total_docs: {snap}"
    );

    // 6. 验证 M5 磁力 CRUD、全行替换、Redis indexed 同步删除与批量删除
    let h1 = "111122223333444455556666777788889999aaaa";
    let (st, created1, _) = send_json(
        &app,
        "POST",
        "/api/v1/torrents",
        Some(&cookie),
        Some(json!({
            "info_hash": h1,
            "raw_name": "M5.Live.Movie.1080p.x264",
            "refine_name": "M5 Live Movie",
            "hot": 3,
            "sync_redis_indexed": true,
            "files": [
                { "path": "movie/main.mkv", "size": 2_147_483_648_u64 },
                { "path": "movie/subs.srt", "size": 65_536_u64 }
            ]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create torrent failed: {created1}");
    let id1 = created1["id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| created1["id"].as_i64())
        .unwrap();
    assert!(id1 > 0);
    assert_eq!(created1["file_count"], 2);
    assert_eq!(created1["total_size"], 2_147_549_184_u64);

    let h1_redis_exists: i64 = redis::cmd("EXISTS")
        .arg(format!("metadata:indexed:{h1}"))
        .query_async(&mut raw_conn)
        .await
        .unwrap();
    assert_eq!(h1_redis_exists, 1);

    // 精确 InfoHash 查询与关键词检索
    let (st, list_hash, _) = send_json(
        &app,
        "GET",
        &format!("/api/v1/torrents?q={h1}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list_hash["items"].as_array().unwrap().len(), 1);
    assert_eq!(list_hash["items"][0]["info_hash"], h1);

    let (st, list_kw, _) = send_json(
        &app,
        "GET",
        "/api/v1/torrents?q=M5%20Live%20Movie",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "keyword search failed: {list_kw}");
    assert!(
        list_kw["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|it| it["info_hash"] == h1)
    );

    // 详情与整行 PATCH 更新（验证 columnar 兼容）
    let (st, detail1, _) = send_json(
        &app,
        "GET",
        &format!("/api/v1/torrents/{id1}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(detail1["files"].as_array().unwrap().len(), 2);

    let (st, patched1, _) = send_json(
        &app,
        "PATCH",
        &format!("/api/v1/torrents/{id1}"),
        Some(&cookie),
        Some(json!({
            "refine_name": "M5 Live Movie Updated",
            "hot": 9
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "patch failed: {patched1}");
    assert_eq!(patched1["refine_name"], "M5 Live Movie Updated");
    assert_eq!(patched1["hot"], 9);
    assert_eq!(patched1["file_count"], 2);

    // 单条删除：keep_redis_indexed=true 保留去重标记
    let (st, del1, _) = send_json(
        &app,
        "DELETE",
        &format!("/api/v1/torrents/{id1}?keep_redis_indexed=true"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "delete keep failed: {del1}");
    assert_eq!(del1["keep_redis_indexed"], true);

    let (st, _, _) = send_json(
        &app,
        "GET",
        &format!("/api/v1/torrents/{id1}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let h1_redis_after: i64 = redis::cmd("EXISTS")
        .arg(format!("metadata:indexed:{h1}"))
        .query_async(&mut raw_conn)
        .await
        .unwrap();
    assert_eq!(h1_redis_after, 1);

    // 单条删除：keep_redis_indexed=false 同时清除去重标记
    let h2 = "22223333444455556666777788889999aaaabbbb";
    let (st, created2, _) = send_json(
        &app,
        "POST",
        "/api/v1/torrents",
        Some(&cookie),
        Some(json!({
            "info_hash": h2,
            "raw_name": "M5.Delete.ClearRedis",
            "hot": 1,
            "sync_redis_indexed": true,
            "files": [{ "path": "clear.mkv", "size": 1024 }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id2 = created2["id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| created2["id"].as_i64())
        .unwrap();

    let (st, del2, _) = send_json(
        &app,
        "DELETE",
        &format!("/api/v1/torrents/{id2}?keep_redis_indexed=false"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "delete clear failed: {del2}");
    let h2_redis_after: i64 = redis::cmd("EXISTS")
        .arg(format!("metadata:indexed:{h2}"))
        .query_async(&mut raw_conn)
        .await
        .unwrap();
    assert_eq!(h2_redis_after, 0);

    // 批量删除：keep_redis_indexed=false 同时清理多个 Manticore 文档与 Redis 标记
    let h3 = "3333444455556666777788889999aaaabbbbcccc";
    let h4 = "444455556666777788889999aaaabbbbccccdddd";
    let (st, c3, _) = send_json(
        &app,
        "POST",
        "/api/v1/torrents",
        Some(&cookie),
        Some(json!({
            "info_hash": h3,
            "raw_name": "M5.BatchDelete.One",
            "hot": 1,
            "sync_redis_indexed": true,
            "files": [{ "path": "b1.mkv", "size": 2048 }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id3 = c3["id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| c3["id"].as_i64())
        .unwrap();

    let (st, c4, _) = send_json(
        &app,
        "POST",
        "/api/v1/torrents",
        Some(&cookie),
        Some(json!({
            "info_hash": h4,
            "raw_name": "M5.BatchDelete.Two",
            "hot": 1,
            "sync_redis_indexed": true,
            "files": [{ "path": "b2.mkv", "size": 4096 }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let id4 = c4["id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| c4["id"].as_i64())
        .unwrap();

    let (st, batch_del, _) = send_json(
        &app,
        "POST",
        "/api/v1/torrents/batch-delete",
        Some(&cookie),
        Some(json!({
            "ids": [id3, id4],
            "keep_redis_indexed": false
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "batch delete failed: {batch_del}");
    assert_eq!(batch_del["deleted"], 2);
    assert_eq!(batch_del["redis_synced"], 2);

    let (st3, _, _) = send_json(
        &app,
        "GET",
        &format!("/api/v1/torrents/{id3}"),
        Some(&cookie),
        None,
    )
    .await;
    let (st4, _, _) = send_json(
        &app,
        "GET",
        &format!("/api/v1/torrents/{id4}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st3, StatusCode::NOT_FOUND);
    assert_eq!(st4, StatusCode::NOT_FOUND);

    let h3_redis_after: i64 = redis::cmd("EXISTS")
        .arg(format!("metadata:indexed:{h3}"))
        .query_async(&mut raw_conn)
        .await
        .unwrap();
    let h4_redis_after: i64 = redis::cmd("EXISTS")
        .arg(format!("metadata:indexed:{h4}"))
        .query_async(&mut raw_conn)
        .await
        .unwrap();
    assert_eq!(h3_redis_after, 0);
    assert_eq!(h4_redis_after, 0);

    // 7. 验证 M6 从 Manticore 真实抽样并在演练场执行 A/B 对比
    let (st, sample_res, _) = send_json(
        &app,
        "POST",
        "/api/v1/filter/playground/sample",
        Some(&cookie),
        Some(json!({ "source": "manticore_latest", "limit": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "sample failed: {sample_res}");
    let samples = sample_res["samples"].as_array().unwrap();
    assert!(!samples.is_empty(), "expected >= 1 sampled Manticore row");
    assert!(
        !samples[0]["files"].as_array().unwrap().is_empty(),
        "expected real file_info in sampled row: {}",
        samples[0]
    );

    let (st, ab_res, _) = send_json(
        &app,
        "POST",
        "/api/v1/filter/playground/test",
        Some(&cookie),
        Some(json!({
            "mode": "ab_compare",
            "samples": samples
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "playground ab failed: {ab_res}");
    assert!(
        ab_res["ab_report"]["summary"]["total"].as_u64().unwrap() >= 1,
        "ab_res={ab_res}"
    );

    // 8. 验证 M8 Redis Streams 监控 / 修剪 / Hash 诊断 & 只读 SQL 控制台
    let (st, rd_ov, _) =
        send_json(&app, "GET", "/api/v1/redis/overview", Some(&cookie), None).await;
    assert_eq!(st, StatusCode::OK, "redis overview failed: {rd_ov}");
    assert_eq!(rd_ov["ok"], true);
    assert_eq!(rd_ov["streams"].as_array().unwrap().len(), 2);
    assert_eq!(rd_ov["streams"][0]["role"], "new");
    assert_eq!(rd_ov["streams"][0]["exists"], true);

    let (st, rd_prev, _) = send_json(
        &app,
        "GET",
        "/api/v1/redis/streams/new/preview?count=10",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "stream preview failed: {rd_prev}");
    assert!(!rd_prev["items"].as_array().unwrap().is_empty());

    let (st, trim_err, _) = send_json(
        &app,
        "POST",
        "/api/v1/redis/streams/new/trim",
        Some(&cookie),
        Some(json!({
            "max_len": 1,
            "approximate": false,
            "confirm_stream_name": "wrong-name"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(trim_err["code"], "CONFIRM_MISMATCH");

    let (st, trim_ok, _) = send_json(
        &app,
        "POST",
        "/api/v1/redis/streams/new/trim",
        Some(&cookie),
        Some(json!({
            "max_len": 1,
            "approximate": false,
            "confirm_stream_name": "hameta:stream:torrent:new"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "trim failed: {trim_ok}");
    assert_eq!(trim_ok["remaining_len"], 1);

    let (st, hash_info, _) = send_json(
        &app,
        "GET",
        &format!("/api/v1/redis/hash/{valid_hash}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "hash inspect failed: {hash_info}");
    assert_eq!(hash_info["indexed"]["exists"], true);
    assert_eq!(hash_info["manticore_exists"], true);
    assert_eq!(hash_info["manticore_hot"], 2);

    let _: () = redis::cmd("SET")
        .arg(format!("hameta:lock:pending:{valid_hash}"))
        .arg("tok-m8-test")
        .arg("EX")
        .arg(60)
        .query_async(&mut raw_conn)
        .await
        .unwrap();

    let (st, hash_del, _) = send_json(
        &app,
        "DELETE",
        &format!(
            "/api/v1/redis/hash/{valid_hash}/locks?clear_pending=true&clear_rejected=true&clear_indexed=false"
        ),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "clear locks failed: {hash_del}");
    assert_eq!(hash_del["deleted_count"], 1);

    let (st, hash_after, _) = send_json(
        &app,
        "GET",
        &format!("/api/v1/redis/hash/{valid_hash}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hash_after["pending"]["exists"], false);
    assert_eq!(hash_after["indexed"]["exists"], true);

    let (st, sql_ok, _) = send_json(
        &app,
        "POST",
        "/api/v1/sql/query",
        Some(&cookie),
        Some(json!({ "sql": "SELECT id, info_hash, raw_name, hot FROM torrents" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "sql query failed: {sql_ok}");
    assert!(
        sql_ok["executed_sql"]
            .as_str()
            .unwrap()
            .contains("LIMIT 200")
    );
    assert!(!sql_ok["rows"].as_array().unwrap().is_empty());

    let (st, sql_status, _) = send_json(
        &app,
        "POST",
        "/api/v1/sql/query",
        Some(&cookie),
        Some(json!({ "sql": "SHOW TABLE torrents STATUS" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "sql show status failed: {sql_status}");
    assert!(!sql_status["rows"].as_array().unwrap().is_empty());

    let (st, sql_forbid, _) = send_json(
        &app,
        "POST",
        "/api/v1/sql/query",
        Some(&cookie),
        Some(json!({ "sql": "DELETE FROM torrents WHERE id = 1" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(sql_forbid["code"], "INVALID_CONFIG");

    supervisor.shutdown_all().await;
    let _ = std::fs::remove_dir_all(&dir);
}
