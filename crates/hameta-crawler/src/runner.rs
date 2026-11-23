use crate::cache::MetadataCache;
use crate::dht::build_dht_options;
use crate::geoip::GeoIpEngine;
use crate::ipc::{CrawlerIpcState, notify_panel_ready, notify_panel_register, start_ipc_server};
use crate::metadata::MetadataHandler;
use crate::redis_streams::RedisStreamsClient;
use crate::stats::{CrawlerAppStats, CrawlerSnapshotInput, build_crawler_snapshot};
use anyhow::{Context, Result};
use chrono::Utc;
use dht_crawler::DHTServer;
use hameta_common::{CrawlerWorkerConfig, WorkerBootstrap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct InFlightGuard<'a> {
    counter: &'a std::sync::atomic::AtomicUsize,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 启动并运行完整的 `hameta-crawler` 采集 Worker 直到收到停机信号且完成排空。
pub async fn run_crawler_worker(bootstrap: WorkerBootstrap) -> Result<()> {
    bootstrap.redis.validate().context("Redis 服务配置无效")?;
    let worker_cfg: CrawlerWorkerConfig = serde_json::from_value(bootstrap.worker_config.clone())
        .context("解析 CrawlerWorkerConfig 失败")?;
    worker_cfg.validate().context("Crawler 配置参数无效")?;

    let started_at = Instant::now();
    let app_stats = Arc::new(CrawlerAppStats::default());
    let worker_shutdown = CancellationToken::new();
    let ipc_shutdown = CancellationToken::new();
    let draining = Arc::new(AtomicBool::new(false));
    let drain_completed = Arc::new(AtomicBool::new(false));
    let graceful_timeout_secs = Arc::new(AtomicU64::new(15));

    let sink_capacity = worker_cfg.push_concurrency.saturating_mul(16).max(256);
    let redis_client = Arc::new(
        RedisStreamsClient::new(
            &bootstrap.redis,
            worker_cfg.metadata_pending_ttl_secs,
            worker_cfg.metadata_failure_backoff_secs,
        )
        .await?,
    );

    let cache = Arc::new(MetadataCache::new(
        worker_cfg.metadata_cache_capacity,
        worker_cfg.metadata_pending_ttl_secs,
        worker_cfg.metadata_failure_backoff_secs,
    ));

    let (hot_tx, hot_rx) = async_channel::bounded::<String>(4_096);
    let mut hot_tasks = Vec::with_capacity(2);
    for _ in 0..2 {
        let rx = hot_rx.clone();
        let redis = redis_client.clone();
        let stats = app_stats.clone();
        hot_tasks.push(tokio::spawn(async move {
            while let Ok(info_hash) = rx.recv().await {
                let ts = u64::try_from(Utc::now().timestamp()).unwrap_or_default();
                match redis.push_hot(&info_hash, ts).await {
                    Ok(()) => {
                        stats.hot_pushed.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(err) => {
                        stats.hot_push_errors.fetch_add(1, Ordering::Relaxed);
                        tracing::debug!("推送热点种子失败 info_hash={info_hash}: {err}");
                    }
                }
            }
        }));
    }

    let metadata_handler = Arc::new(MetadataHandler::new(
        redis_client.clone(),
        cache.clone(),
        app_stats.clone(),
        hot_tx.clone(),
        sink_capacity,
    ));

    let reserved_fds = bootstrap.redis.crawler_pool_size.saturating_add(64);
    let dht_options = build_dht_options(&worker_cfg, reserved_fds)?;
    let effective_metadata_workers = dht_options.metadata.max_worker_count;
    let dht_server = DHTServer::new(dht_options)
        .await
        .context("初始化 DHTServer 失败（请检查 UDP 端口是否被占用）")?;
    let dht_stats = dht_server.runtime_stats();

    let data_dir_path = PathBuf::from(&bootstrap.data_dir);
    let geo_cfg = bootstrap.geoip.clone().unwrap_or_default();
    let geoip = GeoIpEngine::new(&geo_cfg, &data_dir_path);
    let geoip_handle = geoip.spawn_worker(worker_shutdown.clone());

    let stats_for_snap = app_stats.clone();
    let dht_stats_for_snap = dht_stats.clone();
    let cache_for_snap = cache.clone();
    let redis_for_snap = redis_client.clone();
    let profile_for_snap = worker_cfg.crawl_profile.clone();
    let geoip_for_snap = geoip.clone();
    let geoip_for_reload = geoip.clone();
    let reload_data_dir = data_dir_path.clone();

    let ipc_state = Arc::new(CrawlerIpcState {
        instance_id: bootstrap.instance_id.clone(),
        ipc_token: bootstrap.ipc_token.clone(),
        draining: draining.clone(),
        drain_completed: drain_completed.clone(),
        graceful_timeout_secs: graceful_timeout_secs.clone(),
        queued_items: {
            let s = app_stats.clone();
            Arc::new(move || s.sink_queue_depth.load(Ordering::Relaxed))
        },
        in_flight_items: {
            let s = app_stats.clone();
            Arc::new(move || s.sink_in_flight.load(Ordering::Relaxed))
        },
        snapshot_provider: Arc::new(move || {
            let (pool_size, pool_avail) = redis_for_snap.pool_status();
            let snap = build_crawler_snapshot(CrawlerSnapshotInput {
                started_at,
                crawl_profile: &profile_for_snap,
                effective_metadata_workers,
                local_cache_entries: cache_for_snap.entry_count(),
                redis_pool_size: pool_size,
                redis_pool_available: pool_avail,
                app_stats: &stats_for_snap,
                dht_stats: &dht_stats_for_snap,
                geo_snapshot: geoip_for_snap.snapshot(),
            });
            serde_json::to_value(snap).unwrap_or_default()
        }),
        geoip_reloader: Some(Arc::new(move |cfg| {
            geoip_for_reload.reload(&cfg, &reload_data_dir)
        })),
        shutdown_token: worker_shutdown.clone(),
    });

    let ipc_addr = start_ipc_server(ipc_state, ipc_shutdown.clone()).await?;
    tracing::info!(
        "Crawler Worker IPC 已监听 127.0.0.1:{} (instance={})",
        ipc_addr.port(),
        bootstrap.instance_id
    );

    if bootstrap.panel_ipc_port > 0 {
        notify_panel_register(
            bootstrap.panel_ipc_port,
            &bootstrap.ipc_token,
            &bootstrap.instance_id,
            ipc_addr.port(),
        )
        .await?;
    }

    // 挂载 DHT 回调管线
    let early_handler = metadata_handler.clone();
    dht_server.filter(move |info_hash| early_handler.should_accept_early(info_hash));

    let admission_handler = metadata_handler.clone();
    dht_server.on_metadata_admission(move |info_hash| {
        let h = admission_handler.clone();
        async move { h.admit_metadata(&info_hash).await }
    });

    let (torrent_tx, torrent_rx) =
        async_channel::bounded::<dht_crawler::prelude::TorrentInfo>(sink_capacity);
    let drain_notify = Arc::new(Notify::new());

    let push_concurrency = worker_cfg.push_concurrency.max(1);
    let mut sink_tasks = Vec::with_capacity(push_concurrency);
    for _ in 0..push_concurrency {
        let rx = torrent_rx.clone();
        let stats = app_stats.clone();
        let handler = metadata_handler.clone();
        let notify = drain_notify.clone();
        sink_tasks.push(tokio::spawn(async move {
            while let Ok(torrent) = rx.recv().await {
                stats.sink_queue_depth.fetch_sub(1, Ordering::Relaxed);
                stats.sink_in_flight.fetch_add(1, Ordering::Relaxed);
                let _guard = InFlightGuard {
                    counter: &stats.sink_in_flight,
                };
                let _ = handler.deliver_torrent(torrent).await;
                drop(_guard);
                notify.notify_waiters();
            }
        }));
    }

    let ack_stats = app_stats.clone();
    let ack_tx = torrent_tx.clone();
    dht_server.on_torrent_with_ack(move |torrent| {
        if ack_tx.is_closed() {
            return false;
        }
        ack_stats
            .torrents_downloaded
            .fetch_add(1, Ordering::Relaxed);
        ack_stats.sink_queue_depth.fetch_add(1, Ordering::Relaxed);
        match ack_tx.try_send(torrent) {
            Ok(()) => true,
            Err(_) => {
                ack_stats.sink_queue_depth.fetch_sub(1, Ordering::Relaxed);
                ack_stats
                    .torrents_sink_dropped
                    .fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    });

    let completion_handler = metadata_handler.clone();
    dht_server.on_metadata_fetch_complete(move |completion| {
        let h = completion_handler.clone();
        tokio::spawn(async move {
            h.handle_fetch_completion(&completion.info_hash, completion.status)
                .await;
        });
    });

    let err_stats = app_stats.clone();
    dht_server.on_error(move |err| {
        err_stats.dht_runtime_errors.fetch_add(1, Ordering::Relaxed);
        tracing::warn!("DHT 内核运行异常: {err}");
    });

    let geoip_hook = geoip.clone();
    dht_server.on_inbound_dht_packet(move |sample| {
        geoip_hook.observe_packet(sample);
    });

    let mut dht_handle = {
        let srv = dht_server.clone();
        tokio::spawn(async move { srv.start().await })
    };

    if bootstrap.panel_ipc_port > 0 {
        notify_panel_ready(
            bootstrap.panel_ipc_port,
            &bootstrap.ipc_token,
            &bootstrap.instance_id,
        )
        .await?;
    }
    tracing::info!(
        "hameta crawler worker 运行中: instance={}, port={}, profile={}, workers={}",
        bootstrap.instance_id,
        worker_cfg.port,
        worker_cfg.crawl_profile,
        effective_metadata_workers
    );

    tokio::select! {
        _ = worker_shutdown.cancelled() => {
            tracing::info!("Crawler Worker 收到 IPC 停机请求，开始优雅排空...");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Crawler Worker 收到中断信号，开始优雅排空...");
        }
        res = &mut dht_handle => {
            match res {
                Ok(Ok(())) => tracing::info!("DHTServer 主循环已结束"),
                Ok(Err(err)) => tracing::error!("DHTServer 异常退出: {err}"),
                Err(err) => tracing::error!("DHTServer 任务异常终止: {err}"),
            }
        }
    }

    // 优雅停机与排空流程
    draining.store(true, Ordering::Relaxed);
    worker_shutdown.cancel();
    dht_server.clear_inbound_dht_packet_observer();
    dht_server.shutdown();
    if !dht_handle.is_finished() {
        let _ = tokio::time::timeout(Duration::from_secs(5), dht_handle).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), geoip_handle).await;

    torrent_tx.close();
    let deadline = Duration::from_secs(graceful_timeout_secs.load(Ordering::Relaxed).clamp(1, 300));
    let drain_result = tokio::time::timeout(deadline, async {
        for task in sink_tasks {
            let _ = task.await;
        }
    })
    .await;
    if drain_result.is_err() {
        tracing::warn!(
            "Crawler Worker 排空超时（{}s），剩余未推送条目={}",
            deadline.as_secs(),
            app_stats.sink_queue_depth.load(Ordering::Relaxed)
        );
    }

    hot_tx.close();
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        for task in hot_tasks {
            let _ = task.await;
        }
    })
    .await;

    if let Err(err) = metadata_handler.release_remaining_claims().await {
        tracing::warn!("停机释放剩余 Redis Pending 租约失败: {err}");
    }

    drain_completed.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(100)).await;
    ipc_shutdown.cancel();

    tracing::info!(
        "hameta crawler worker 已优雅退出: {}",
        bootstrap.instance_id
    );
    Ok(())
}
