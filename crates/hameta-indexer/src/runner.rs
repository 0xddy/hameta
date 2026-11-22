use crate::batcher::{AcceptedTorrentItem, TorrentBatcher};
use crate::hot_batcher::{HotBatcher, HotQueueItem};
use crate::ipc::{IndexerIpcState, notify_panel_ready, notify_panel_register, start_ipc_server};
use crate::manticore::ManticoreClient;
use crate::stats::{IndexerAppStats, build_indexer_snapshot};
use crate::streams::{ParsedTorrentEntry, RedisStreamsConsumer};
use anyhow::{Context, Result};
use chrono::Utc;
use hameta_common::{IndexerWorkerConfig, TorrentRecord, WorkerBootstrap};
use hameta_filter::{FilterBundle, FilterOutcome, FilterRuntime, default_online_filter_bundle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;

/// 启动并运行完整的 `hameta-indexer` Worker 直到收到停机信号且刷盘 + XACK 完成。
pub async fn run_indexer_worker(bootstrap: WorkerBootstrap) -> Result<()> {
    bootstrap.redis.validate().context("Redis 服务配置无效")?;
    let manticore_cfg = bootstrap
        .manticore
        .clone()
        .context("Indexer 启动缺少 Manticore 服务配置")?;
    manticore_cfg.validate().context("Manticore 服务配置无效")?;

    let worker_cfg: IndexerWorkerConfig = serde_json::from_value(bootstrap.worker_config.clone())
        .context("解析 IndexerWorkerConfig 失败")?;
    worker_cfg.validate().context("Indexer 配置参数无效")?;

    let filter_bundle: FilterBundle = match &bootstrap.filter_bundle {
        Some(val) => serde_json::from_value(val.clone()).context("解析 FilterBundle JSON 失败")?,
        None => default_online_filter_bundle(),
    };
    let filter_runtime =
        Arc::new(FilterRuntime::new(&filter_bundle).context("初始化 FilterRuntime 失败")?);

    let started_at = Instant::now();
    let app_stats = Arc::new(IndexerAppStats::default());
    let worker_shutdown = CancellationToken::new();
    let ipc_shutdown = CancellationToken::new();
    let draining = Arc::new(AtomicBool::new(false));
    let drain_completed = Arc::new(AtomicBool::new(false));
    let graceful_timeout_secs = Arc::new(AtomicU64::new(15));

    // 启动前先做 Manticore 只读表结构校验，绝不擅自建表或改表
    let manticore = Arc::new(ManticoreClient::new(&manticore_cfg)?);
    let schema = manticore.describe_torrents().await?;
    anyhow::ensure!(
        schema.compatible,
        "Manticore torrents 表未就绪或不兼容: {}",
        schema.message
    );
    tracing::info!("Manticore 只读结构校验通过: {}", schema.message);

    let streams = Arc::new(RedisStreamsConsumer::new(&bootstrap.redis).await?);
    streams.ensure_consumer_groups().await?;

    let ipc_state = Arc::new(IndexerIpcState {
        instance_id: bootstrap.instance_id.clone(),
        ipc_token: bootstrap.ipc_token.clone(),
        filter_runtime: filter_runtime.clone(),
        draining: draining.clone(),
        drain_completed: drain_completed.clone(),
        graceful_timeout_secs: graceful_timeout_secs.clone(),
        queued_items: {
            let s = app_stats.clone();
            Arc::new(move || {
                s.torrent_buffered.load(Ordering::Relaxed) + s.hot_buffered.load(Ordering::Relaxed)
            })
        },
        in_flight_items: {
            let s = app_stats.clone();
            Arc::new(move || s.torrent_in_flight.load(Ordering::Relaxed))
        },
        snapshot_provider: {
            let s = app_stats.clone();
            let f = filter_runtime.clone();
            let st = streams.clone();
            Arc::new(move || {
                let (pool_size, pool_avail) = st.pool_status();
                let snap = build_indexer_snapshot(started_at, pool_size, pool_avail, &s, &f);
                serde_json::to_value(snap).unwrap_or_default()
            })
        },
        shutdown_token: worker_shutdown.clone(),
    });

    let ipc_addr = start_ipc_server(ipc_state, ipc_shutdown.clone()).await?;
    tracing::info!(
        "Indexer Worker IPC 已监听 127.0.0.1:{} (instance={})",
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

    let (torrent_tx, torrent_rx) =
        mpsc::channel::<AcceptedTorrentItem>(worker_cfg.channel_buffer_size.max(16));
    let (hot_tx, hot_rx) =
        mpsc::channel::<HotQueueItem>(worker_cfg.hot_channel_buffer_size.max(16));

    let torrent_batcher = TorrentBatcher::new(
        manticore.clone(),
        streams.clone(),
        worker_cfg.clone(),
        app_stats.clone(),
    );
    let batcher_handle = tokio::spawn(torrent_batcher.run(torrent_rx));

    let hot_batcher = HotBatcher::new(
        manticore.clone(),
        streams.clone(),
        worker_cfg.clone(),
        app_stats.clone(),
    );
    let hot_handle = tokio::spawn(hot_batcher.run(hot_rx));

    let mut consumer_handles = Vec::new();
    let consume_cancel = CancellationToken::new();

    for idx in 0..worker_cfg.torrent_workers.max(1) {
        let consumer_name = format!("{}-new-{idx}", bootstrap.instance_id);
        let st = streams.clone();
        let fr = filter_runtime.clone();
        let tx = torrent_tx.clone();
        let stats = app_stats.clone();
        let cancel = consume_cancel.clone();
        let batch = worker_cfg.read_batch_size.max(1);
        let block_ms = worker_cfg.read_block_ms.max(100);

        consumer_handles.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                let res = tokio::select! {
                    _ = cancel.cancelled() => break,
                    r = st.read_torrent_batch(&consumer_name, batch, block_ms) => r,
                };
                match res {
                    Ok(read_batch) => {
                        let read_count = read_batch.valid.len() + read_batch.malformed.len();
                        if read_count == 0 {
                            continue;
                        }
                        stats
                            .torrent_messages_read
                            .fetch_add(u64::try_from(read_count).unwrap_or(0), Ordering::Relaxed);

                        let mut immediate_acks = Vec::new();
                        for mal in read_batch.malformed {
                            tracing::warn!(
                                "新种 Stream 消息损坏，记录后立即确认丢弃: entry_id={}, reason={}",
                                mal.entry_id,
                                mal.reason
                            );
                            stats
                                .malformed_messages_acked
                                .fetch_add(1, Ordering::Relaxed);
                            immediate_acks.push(mal.entry_id);
                        }

                        process_parsed_torrents(
                            read_batch.valid,
                            &fr,
                            &tx,
                            &stats,
                            &mut immediate_acks,
                            &cancel,
                        )
                        .await;

                        if !immediate_acks.is_empty()
                            && let Err(err) =
                                st.xack_batch(st.torrent_stream(), &immediate_acks).await
                        {
                            tracing::warn!("批量 XACK 拦截/损坏消息失败: {err}");
                        } else if !immediate_acks.is_empty() {
                            stats.stream_xacked_new.fetch_add(
                                u64::try_from(immediate_acks.len()).unwrap_or(0),
                                Ordering::Relaxed,
                            );
                        }
                    }
                    Err(err) => {
                        tracing::warn!("读取新种 Stream 失败: {err}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }));
    }

    for idx in 0..worker_cfg.hot_workers.max(1) {
        let consumer_name = format!("{}-hot-{idx}", bootstrap.instance_id);
        let st = streams.clone();
        let tx = hot_tx.clone();
        let stats = app_stats.clone();
        let cancel = consume_cancel.clone();
        let batch = worker_cfg.read_batch_size.max(1);
        let block_ms = worker_cfg.read_block_ms.max(100);

        consumer_handles.push(tokio::spawn(async move {
            while !cancel.is_cancelled() {
                let res = tokio::select! {
                    _ = cancel.cancelled() => break,
                    r = st.read_hot_batch(&consumer_name, batch, block_ms) => r,
                };
                match res {
                    Ok(read_batch) => {
                        let read_count = read_batch.valid.len() + read_batch.malformed.len();
                        if read_count == 0 {
                            continue;
                        }
                        stats
                            .hot_messages_read
                            .fetch_add(u64::try_from(read_count).unwrap_or(0), Ordering::Relaxed);

                        let mut mal_acks = Vec::new();
                        for mal in read_batch.malformed {
                            stats
                                .malformed_messages_acked
                                .fetch_add(1, Ordering::Relaxed);
                            mal_acks.push(mal.entry_id);
                        }
                        if !mal_acks.is_empty() {
                            let _ = st.xack_batch(st.hot_stream(), &mal_acks).await;
                        }
                        for item in read_batch.valid {
                            if cancel.is_cancelled() {
                                break;
                            }
                            let _ = tx
                                .send(HotQueueItem {
                                    entry_id: item.entry_id,
                                    info_hash: item.info_hash,
                                })
                                .await;
                        }
                    }
                    Err(err) => {
                        tracing::warn!("读取热点 Stream 失败: {err}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }));
    }

    // 后台 PEL 崩溃接管巡检任务
    {
        let st = streams.clone();
        let fr = filter_runtime.clone();
        let t_tx = torrent_tx.clone();
        let h_tx = hot_tx.clone();
        let stats = app_stats.clone();
        let cancel = consume_cancel.clone();
        let reclaim_consumer = format!("{}-reclaim", bootstrap.instance_id);
        let interval_secs = worker_cfg.xautoclaim_interval_secs.max(1);
        let min_idle_ms = worker_cfg.xautoclaim_min_idle_ms.max(1_000);
        let max_attempts = worker_cfg.max_delivery_attempts.max(1);

        consumer_handles.push(tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(interval_secs));
            ticker.tick().await; // 跳过启动即刻空跑
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        match st.reclaim_stale_torrents(&reclaim_consumer, min_idle_ms, 256, max_attempts).await {
                            Ok(reclaimed) => {
                                stats.poison_messages_acked.fetch_add(
                                    u64::try_from(reclaimed.poisoned_count).unwrap_or(0),
                                    Ordering::Relaxed,
                                );
                                stats.reclaimed_new.fetch_add(
                                    u64::try_from(reclaimed.valid.len()).unwrap_or(0),
                                    Ordering::Relaxed,
                                );
                                let mut acks = reclaimed.ack_immediately;
                                process_parsed_torrents(
                                    reclaimed.valid,
                                    &fr,
                                    &t_tx,
                                    &stats,
                                    &mut acks,
                                    &cancel,
                                ).await;
                                if !acks.is_empty() {
                                    let _ = st.xack_batch(st.torrent_stream(), &acks).await;
                                }
                            }
                            Err(err) => tracing::debug!("新种 PEL 巡检失败: {err}"),
                        }

                        match st.reclaim_stale_hot(&reclaim_consumer, min_idle_ms, 256, max_attempts).await {
                            Ok(reclaimed) => {
                                stats.poison_messages_acked.fetch_add(
                                    u64::try_from(reclaimed.poisoned_count).unwrap_or(0),
                                    Ordering::Relaxed,
                                );
                                stats.reclaimed_hot.fetch_add(
                                    u64::try_from(reclaimed.valid.len()).unwrap_or(0),
                                    Ordering::Relaxed,
                                );
                                if !reclaimed.ack_immediately.is_empty() {
                                    let _ = st.xack_batch(st.hot_stream(), &reclaimed.ack_immediately).await;
                                }
                                for item in reclaimed.valid {
                                    if cancel.is_cancelled() {
                                        break;
                                    }
                                    let _ = h_tx.send(HotQueueItem {
                                        entry_id: item.entry_id,
                                        info_hash: item.info_hash,
                                    }).await;
                                }
                            }
                            Err(err) => tracing::debug!("热点 PEL 巡检失败: {err}"),
                        }
                    }
                }
            }
        }));
    }

    if bootstrap.panel_ipc_port > 0 {
        notify_panel_ready(
            bootstrap.panel_ipc_port,
            &bootstrap.ipc_token,
            &bootstrap.instance_id,
            filter_runtime.active_version(),
        )
        .await?;
    }

    tracing::info!(
        "hameta indexer worker 运行中: instance={}, rule_version=v{}, torrent_workers={}, hot_workers={}",
        bootstrap.instance_id,
        filter_runtime.active_version(),
        worker_cfg.torrent_workers,
        worker_cfg.hot_workers
    );

    tokio::select! {
        _ = worker_shutdown.cancelled() => {
            tracing::info!("Indexer Worker 收到 IPC 停机请求，开始优雅排空...");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Indexer Worker 收到中断信号，开始优雅排空...");
        }
    }

    draining.store(true, Ordering::Relaxed);
    consume_cancel.cancel();

    for handle in consumer_handles {
        let _ = tokio::time::timeout(Duration::from_secs(3), handle).await;
    }

    drop(torrent_tx);
    drop(hot_tx);

    let deadline = Duration::from_secs(graceful_timeout_secs.load(Ordering::Relaxed).clamp(1, 300));
    let drain_res = tokio::time::timeout(deadline, async {
        let _ = batcher_handle.await;
        let _ = hot_handle.await;
    })
    .await;
    if drain_res.is_err() {
        tracing::warn!("Indexer Worker 排空超时（{}s）", deadline.as_secs());
    }

    drain_completed.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(100)).await;
    ipc_shutdown.cancel();

    tracing::info!(
        "hameta indexer worker 已优雅退出: {}",
        bootstrap.instance_id
    );
    Ok(())
}

async fn process_parsed_torrents(
    items: Vec<ParsedTorrentEntry>,
    filter_runtime: &Arc<FilterRuntime>,
    tx: &mpsc::Sender<AcceptedTorrentItem>,
    stats: &Arc<IndexerAppStats>,
    immediate_acks: &mut Vec<String>,
    cancel: &CancellationToken,
) {
    for entry in items {
        if cancel.is_cancelled() {
            break;
        }
        match filter_runtime.evaluate_and_record(&entry.message) {
            FilterOutcome::Rejected { .. } => {
                stats.filter_rejected.fetch_add(1, Ordering::Relaxed);
                immediate_acks.push(entry.entry_id);
            }
            FilterOutcome::Accepted { refined_name, .. } => {
                stats.filter_accepted.fetch_add(1, Ordering::Relaxed);
                let now = Utc::now();
                let record = TorrentRecord {
                    info_hash: entry.message.info_hash,
                    raw_name: entry.message.name,
                    refine_name: Some(refined_name),
                    file_count: i32::try_from(entry.message.file_count).unwrap_or(i32::MAX),
                    file_info: entry.message.files,
                    hot: 1,
                    created_at: now,
                    updated_at: now,
                };
                if tx
                    .send(AcceptedTorrentItem {
                        entry_id: entry.entry_id,
                        record,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}
