pub mod log_capture;
pub mod os_guard;

use crate::ingest_hub::IngestHub;
use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use hameta_common::{
    CrawlerWorkerConfig, DesiredState, GeoIpConfig, IndexerWorkerConfig, ManticoreServiceConfig,
    RedisServiceConfig, ServiceKind, WorkerBootstrap, WorkerDrainStatus, WorkerIngestEventsRequest,
    WorkerReadyRequest, WorkerRegisterRequest, WorkerReloadGeoIpRequest, WorkerReloadGeoIpResponse,
    WorkerReloadRulesRequest, WorkerReloadRulesResponse, WorkerRole, WorkerRuntimeState,
    WorkerShutdownRequest, WorkerSnapshotEnvelope, generate_token, verify_bearer_token,
};
use hameta_filter::{FilterBundle, default_online_filter_bundle};
use hameta_storage::Database;
use log_capture::{StderrRingBuffer, spawn_log_pumps};
use os_guard::{ChildJobGuard, configure_child_deathsig, create_job_guard_best_effort};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::{Notify, broadcast, oneshot};
use tokio_util::sync::CancellationToken;

const CRASH_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
}

pub type CommandFactory = Arc<dyn Fn(&WorkerBootstrap) -> Result<SpawnSpec> + Send + Sync>;

fn default_command_factory(bootstrap: &WorkerBootstrap) -> Result<SpawnSpec> {
    let exe = std::env::current_exe().context("定位当前 hameta 可执行文件路径失败")?;
    Ok(SpawnSpec {
        program: exe,
        args: vec!["worker".to_string(), bootstrap.role.as_str().to_string()],
        envs: Vec::new(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerStatusView {
    pub id: String,
    pub role: WorkerRole,
    pub display_name: String,
    pub desired_state: DesiredState,
    pub runtime_state: WorkerRuntimeState,
    pub active_config_version: i64,
    pub auto_restart: bool,
    pub max_restarts: i64,
    pub restart_backoff_secs: i64,
    pub pid: Option<u32>,
    pub worker_port: Option<u16>,
    pub active_rule_version: Option<i64>,
    pub started_at: Option<i64>,
    pub restart_count: u64,
    pub last_exit_code: Option<i32>,
    pub last_error: Option<String>,
    pub stderr_tail: String,
    pub drain_status: Option<WorkerDrainStatus>,
    pub snapshot: Option<WorkerSnapshotEnvelope>,
    #[serde(skip)]
    pub snapshot_seq: u64,
    #[serde(skip)]
    pub snapshot_fetched_at_ms: i64,
}

pub const META_GEOIP_CONFIG_KEY: &str = "geoip_config";

pub async fn load_geoip_config(db: &Database) -> GeoIpConfig {
    match db.get_meta(META_GEOIP_CONFIG_KEY).await {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
        _ => GeoIpConfig::default(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerReloadResult {
    pub instance_id: String,
    pub ok: bool,
    pub active_rule_version: Option<i64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerGeoIpReloadResult {
    pub instance_id: String,
    pub ok: bool,
    pub mmdb_loaded: bool,
    pub error: Option<String>,
    pub message: String,
}

struct ManagedWorker {
    instance_id: String,
    role: WorkerRole,
    runtime_state: WorkerRuntimeState,
    generation: u64,
    pid: Option<u32>,
    worker_port: Option<u16>,
    ipc_token: String,
    active_rule_version: Option<i64>,
    started_at: Option<i64>,
    last_exit_code: Option<i32>,
    last_error: Option<String>,
    restart_count: u64,
    recent_crashes: VecDeque<Instant>,
    manual_stop: bool,
    latest_snapshot: Option<WorkerSnapshotEnvelope>,
    snapshot_seq: u64,
    snapshot_fetched_at_ms: i64,
    drain_status: Option<WorkerDrainStatus>,
    stderr_ring: Arc<StderrRingBuffer>,
    log_tx: broadcast::Sender<String>,
    kill_tx: Option<oneshot::Sender<()>>,
    backoff_cancel: Option<CancellationToken>,
    exit_notify: Arc<Notify>,
}

impl ManagedWorker {
    fn new(instance_id: &str, role: WorkerRole) -> Self {
        let (log_tx, _) = broadcast::channel(512);
        Self {
            instance_id: instance_id.to_string(),
            role,
            runtime_state: WorkerRuntimeState::Stopped,
            generation: 0,
            pid: None,
            worker_port: None,
            ipc_token: String::new(),
            active_rule_version: None,
            started_at: None,
            last_exit_code: None,
            last_error: None,
            restart_count: 0,
            recent_crashes: VecDeque::new(),
            manual_stop: false,
            latest_snapshot: None,
            snapshot_seq: 0,
            snapshot_fetched_at_ms: 0,
            drain_status: None,
            stderr_ring: Arc::new(StderrRingBuffer::default()),
            log_tx,
            kill_tx: None,
            backoff_cancel: None,
            exit_notify: Arc::new(Notify::new()),
        }
    }
}

/// 进程守护控制器：负责 Worker 拉起、`stdin` 参数注入、Loopback IPC 互认、崩溃自愈熔断及优雅排空停机。
pub struct Supervisor {
    db: Database,
    data_dir: PathBuf,
    panel_ipc_port: u16,
    http_client: Client,
    job_guard: ChildJobGuard,
    command_factory: CommandFactory,
    workers: RwLock<HashMap<String, ManagedWorker>>,
    ingest_hub: Arc<IngestHub>,
    ipc_shutdown: CancellationToken,
    shutting_down: AtomicBool,
}

impl Supervisor {
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub async fn new(db: Database, data_dir: PathBuf) -> Result<Arc<Self>> {
        Self::with_command_factory(db, data_dir, Arc::new(default_command_factory)).await
    }

    pub async fn with_command_factory(
        db: Database,
        data_dir: PathBuf,
        command_factory: CommandFactory,
    ) -> Result<Arc<Self>> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("Panel Supervisor 绑定 Loopback IPC 端口失败")?;
        let panel_ipc_port = listener
            .local_addr()
            .context("获取 Supervisor IPC 地址失败")?
            .port();

        let http_client = Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(2))
            .build()
            .context("创建 Supervisor IPC HTTP 客户端失败")?;

        let ipc_shutdown = CancellationToken::new();
        let supervisor = Arc::new(Self {
            db,
            data_dir,
            panel_ipc_port,
            http_client,
            job_guard: create_job_guard_best_effort(),
            command_factory,
            workers: RwLock::new(HashMap::new()),
            ingest_hub: IngestHub::new(),
            ipc_shutdown: ipc_shutdown.clone(),
            shutting_down: AtomicBool::new(false),
        });

        let router = build_panel_ipc_router(supervisor.clone());
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router)
                .with_graceful_shutdown(async move { ipc_shutdown.cancelled().await })
                .await
            {
                tracing::warn!("Supervisor IPC 服务异常退出: {err}");
            }
        });

        tracing::info!("Supervisor Loopback IPC 已监听 127.0.0.1:{panel_ipc_port}");
        Ok(supervisor)
    }

    pub fn ipc_port(&self) -> u16 {
        self.panel_ipc_port
    }

    pub fn ingest_hub(&self) -> Arc<IngestHub> {
        self.ingest_hub.clone()
    }

    /// 面板启动时自动同步并拉起数据库中所有 `desired_state = 'running'` 的 Worker。
    pub async fn reconcile_desired_workers(self: &Arc<Self>) -> Result<()> {
        let rows = self.db.list_workers().await?;
        for row in rows {
            if row.desired_state == DesiredState::Running.as_str()
                && let Err(err) = self.start_worker(&row.id).await
            {
                tracing::error!("自动拉起期望运行中的 Worker '{}' 失败: {err:#}", row.id);
            }
        }
        Ok(())
    }

    pub async fn start_worker(self: &Arc<Self>, instance_id: &str) -> Result<()> {
        anyhow::ensure!(
            !self.shutting_down.load(Ordering::Relaxed),
            "Panel 正在停机中，禁止启动新 Worker"
        );

        let row = self
            .db
            .get_worker(instance_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Worker 实例 {instance_id} 不存在"))?;
        let role: WorkerRole = row.role.parse().map_err(|e: String| anyhow::anyhow!(e))?;

        {
            let mut guard = self
                .workers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = guard
                .entry(instance_id.to_string())
                .or_insert_with(|| ManagedWorker::new(instance_id, role));
            entry.role = role;
            if let Some(cancel) = entry.backoff_cancel.take() {
                cancel.cancel();
            }
            if matches!(
                entry.runtime_state,
                WorkerRuntimeState::Starting
                    | WorkerRuntimeState::Running
                    | WorkerRuntimeState::Draining
            ) && entry.kill_tx.is_some()
            {
                anyhow::bail!("Worker {instance_id} 当前正在运行或停机排空中");
            }
            // 用户手动重启时清理上次崩溃计数和错误态
            entry.recent_crashes.clear();
            entry.manual_stop = false;
            entry.last_error = None;
            entry.last_exit_code = None;
            entry.stderr_ring.clear();
        }

        self.spawn_instance_process(instance_id, role).await
    }

    fn spawn_instance_process<'a>(
        self: &'a Arc<Self>,
        instance_id: &'a str,
        role: WorkerRole,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let bootstrap = self.assemble_bootstrap(instance_id, role).await?;
            let spec = (self.command_factory)(&bootstrap)?;
            let ipc_token = bootstrap.ipc_token.clone();

            let my_generation = {
                let mut guard = self
                    .workers
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let entry = guard
                    .entry(instance_id.to_string())
                    .or_insert_with(|| ManagedWorker::new(instance_id, role));
                entry.generation += 1;
                entry.runtime_state = WorkerRuntimeState::Starting;
                entry.ipc_token = ipc_token;
                entry.worker_port = None;
                entry.pid = None;
                entry.started_at = Some(Utc::now().timestamp());
                entry.manual_stop = false;
                entry.drain_status = None;
                entry.latest_snapshot = None;
                entry.snapshot_fetched_at_ms = 0;
                entry.generation
            };

            let mut cmd = Command::new(&spec.program);
            cmd.args(&spec.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            for (k, v) in &spec.envs {
                cmd.env(k, v);
            }
            configure_child_deathsig(&mut cmd);

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(err) => {
                    self.mark_spawn_failure(instance_id, my_generation, &err.to_string());
                    return Err(err).with_context(|| {
                        format!(
                            "拉起 Worker 子进程失败: instance={}, program={}",
                            instance_id,
                            spec.program.display()
                        )
                    });
                }
            };

            if let Err(err) = self.job_guard.assign_child(&child) {
                tracing::warn!("绑定 Worker {instance_id} 至 Job Object 失败: {err}");
            }

            let pid = child.id();
            let mut stdin = child.stdin.take();
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let (stderr_ring, log_tx, exit_notify, kill_tx_slot) = {
                let (kill_tx, kill_rx) = oneshot::channel::<()>();
                let mut guard = self
                    .workers
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let entry = guard
                    .entry(instance_id.to_string())
                    .or_insert_with(|| ManagedWorker::new(instance_id, role));
                entry.pid = pid;
                entry.kill_tx = Some(kill_tx);
                (
                    entry.stderr_ring.clone(),
                    entry.log_tx.clone(),
                    entry.exit_notify.clone(),
                    kill_rx,
                )
            };

            let logs_dir = self.data_dir.join("logs");
            spawn_log_pumps(
                instance_id.to_string(),
                logs_dir,
                stdout,
                stderr,
                stderr_ring.clone(),
                log_tx,
            );

            // 通过 stdin 一次性注入 WorkerBootstrap JSON 并立刻关闭管道（EOF）
            if let Some(mut stdin_pipe) = stdin.take() {
                let payload =
                    serde_json::to_vec(&bootstrap).context("序列化 WorkerBootstrap 失败")?;
                if let Err(err) = stdin_pipe.write_all(&payload).await {
                    tracing::warn!("向 Worker {instance_id} 写入 stdin Bootstrap 失败: {err}");
                }
                let _ = stdin_pipe.flush().await;
                let _ = stdin_pipe.shutdown().await;
                drop(stdin_pipe);
            }

            let sup = self.clone();
            let inst_id = instance_id.to_string();
            tokio::spawn(async move {
                let exit_status = tokio::select! {
                    res = child.wait() => res.ok(),
                    _ = kill_tx_slot => {
                        let _ = child.kill().await;
                        child.wait().await.ok()
                    }
                };
                sup.handle_child_exit(
                    &inst_id,
                    role,
                    my_generation,
                    exit_status,
                    &exit_notify,
                    &stderr_ring,
                )
                .await;
            });

            tracing::info!(
                "已启动 Worker 子进程: instance={}, role={}, pid={:?}",
                instance_id,
                role.as_str(),
                pid
            );
            Ok(())
        })
    }

    fn mark_spawn_failure(&self, instance_id: &str, expected_gen: u64, err_msg: &str) {
        let mut guard = self
            .workers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = guard.get_mut(instance_id)
            && entry.generation == expected_gen
        {
            entry.runtime_state = WorkerRuntimeState::Crashed;
            entry.last_error = Some(err_msg.to_string());
            entry.pid = None;
            entry.worker_port = None;
            entry.kill_tx = None;
        }
    }

    async fn handle_child_exit(
        self: &Arc<Self>,
        instance_id: &str,
        role: WorkerRole,
        my_generation: u64,
        exit_status: Option<std::process::ExitStatus>,
        exit_notify: &Arc<Notify>,
        stderr_ring: &Arc<StderrRingBuffer>,
    ) {
        let exit_code = exit_status.and_then(|s| s.code());
        let stderr_tail = stderr_ring.snapshot_text();

        let (manual_stop, was_stale) = {
            let mut guard = self
                .workers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = guard.get_mut(instance_id) else {
                exit_notify.notify_waiters();
                return;
            };
            if entry.generation != my_generation {
                (false, true)
            } else {
                entry.pid = None;
                entry.worker_port = None;
                entry.kill_tx = None;
                entry.drain_status = None;
                entry.last_exit_code = exit_code;
                (entry.manual_stop, false)
            }
        };
        exit_notify.notify_waiters();

        if was_stale {
            return;
        }

        if manual_stop || self.shutting_down.load(Ordering::Relaxed) {
            let mut guard = self
                .workers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = guard.get_mut(instance_id)
                && entry.generation == my_generation
            {
                entry.runtime_state = WorkerRuntimeState::Stopped;
            }
            return;
        }

        let row = self.db.get_worker(instance_id).await.ok().flatten();
        let should_keep_running = row
            .as_ref()
            .is_some_and(|r| r.desired_state == DesiredState::Running.as_str());
        let auto_restart = row.as_ref().is_some_and(|r| r.auto_restart != 0);
        let max_restarts = row
            .as_ref()
            .map_or(5, |r| usize::try_from(r.max_restarts.max(0)).unwrap_or(5));
        let base_backoff_secs = row.as_ref().map_or(3, |r| {
            u64::try_from(r.restart_backoff_secs.max(1)).unwrap_or(3)
        });

        // 若期望已不再是 running（且非异常状态），标记 Stopped
        if !should_keep_running && exit_status.is_some_and(|s| s.success()) {
            let mut guard = self
                .workers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = guard.get_mut(instance_id)
                && entry.generation == my_generation
            {
                entry.runtime_state = WorkerRuntimeState::Stopped;
            }
            return;
        }

        let summary_tail = tail_summary(&stderr_tail, 500);
        let err_desc = format!(
            "Worker 非预期退出 (exit_code={exit_code:?}){}",
            if summary_tail.is_empty() {
                String::new()
            } else {
                format!(": {summary_tail}")
            }
        );

        enum RestartAction {
            Stop,
            CrashFuse,
            Retry {
                delay: Duration,
                cancel: CancellationToken,
            },
        }

        let action = {
            let mut guard = self
                .workers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = guard.get_mut(instance_id) else {
                return;
            };
            if entry.generation != my_generation {
                return;
            }

            if !should_keep_running {
                entry.runtime_state = WorkerRuntimeState::Stopped;
                entry.last_error = Some(err_desc.clone());
                RestartAction::Stop
            } else {
                let now = Instant::now();
                while entry
                    .recent_crashes
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW)
                {
                    entry.recent_crashes.pop_front();
                }
                entry.recent_crashes.push_back(now);
                let streak = entry.recent_crashes.len();

                if !auto_restart || max_restarts == 0 || streak >= max_restarts {
                    entry.runtime_state = WorkerRuntimeState::Crashed;
                    entry.last_error = Some(err_desc.clone());
                    RestartAction::CrashFuse
                } else {
                    let shift = u32::try_from(streak.saturating_sub(1)).unwrap_or(0).min(6);
                    let delay_secs = base_backoff_secs.saturating_mul(1u64 << shift).min(60);
                    let cancel = CancellationToken::new();
                    entry.runtime_state = WorkerRuntimeState::Starting;
                    entry.restart_count += 1;
                    entry.last_error = Some(err_desc.clone());
                    entry.backoff_cancel = Some(cancel.clone());
                    RestartAction::Retry {
                        delay: Duration::from_secs(delay_secs),
                        cancel,
                    }
                }
            }
        };

        match action {
            RestartAction::Stop => {
                tracing::warn!("Worker {instance_id} 退出（期望状态非运行，不重启）: {err_desc}");
            }
            RestartAction::CrashFuse => {
                tracing::error!(
                    "Worker {instance_id} 在 60s 内连续异常退出达到熔断阈值 ({max_restarts} 次)，已停止自愈: {err_desc}"
                );
                let detail = json!({
                    "exit_code": exit_code,
                    "max_restarts": max_restarts,
                    "stderr_tail": tail_summary(&stderr_tail, 2000),
                })
                .to_string();
                let _ = self
                    .db
                    .record_audit(
                        "supervisor",
                        "worker.crashed",
                        instance_id,
                        &detail,
                        "127.0.0.1",
                    )
                    .await;
            }
            RestartAction::Retry { delay, cancel } => {
                tracing::warn!(
                    "Worker {instance_id} 异常退出，将在 {}s 后退避重启: {err_desc}",
                    delay.as_secs()
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = cancel.cancelled() => return,
                }
                if self.shutting_down.load(Ordering::Relaxed) {
                    return;
                }
                let still_running = {
                    let guard = self
                        .workers
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    guard
                        .get(instance_id)
                        .is_some_and(|e| e.generation == my_generation && !e.manual_stop)
                };
                if !still_running {
                    return;
                }
                let db_running = self
                    .db
                    .get_worker(instance_id)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.desired_state == DesiredState::Running.as_str());
                if !db_running {
                    return;
                }
                if let Err(err) = self.spawn_instance_process(instance_id, role).await {
                    tracing::error!("自动重启 Worker {instance_id} 失败: {err:#}");
                }
            }
        }
    }

    async fn assemble_bootstrap(
        &self,
        instance_id: &str,
        role: WorkerRole,
    ) -> Result<WorkerBootstrap> {
        let worker_cfg_row = self
            .db
            .get_active_worker_config(instance_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Worker {instance_id} 缺少激活配置版本"))?;
        let worker_config: Value = serde_json::from_str(&worker_cfg_row.config_json)
            .context("解析 Worker 配置 JSON 失败")?;

        match role {
            WorkerRole::Crawler => {
                let parsed: CrawlerWorkerConfig = serde_json::from_value(worker_config.clone())
                    .context("CrawlerWorkerConfig 结构不合法")?;
                parsed.validate()?;
            }
            WorkerRole::Indexer => {
                let parsed: IndexerWorkerConfig = serde_json::from_value(worker_config.clone())
                    .context("IndexerWorkerConfig 结构不合法")?;
                parsed.validate()?;
            }
        }

        let redis_row = self
            .db
            .get_active_service_config(ServiceKind::Redis)
            .await?
            .ok_or_else(|| anyhow::anyhow!("尚未配置 Redis 连接参数，无法启动 Worker"))?;
        let redis: RedisServiceConfig = serde_json::from_str(&redis_row.config_json)
            .context("解析 Redis 服务配置 JSON 失败")?;
        redis.validate()?;

        let (manticore, filter_bundle) = if role == WorkerRole::Indexer {
            let manticore_row = self
                .db
                .get_active_service_config(ServiceKind::Manticore)
                .await?
                .ok_or_else(|| anyhow::anyhow!("尚未配置 Manticore 连接参数，无法启动 Indexer"))?;
            let m_cfg: ManticoreServiceConfig = serde_json::from_str(&manticore_row.config_json)
                .context("解析 Manticore 服务配置 JSON 失败")?;
            m_cfg.validate()?;

            let bundle = load_published_filter_bundle(&self.db).await?;
            let bundle_val = serde_json::to_value(&bundle).context("序列化 FilterBundle 失败")?;
            (Some(m_cfg), Some(bundle_val))
        } else {
            (None, None)
        };

        let geoip = if role == WorkerRole::Crawler {
            Some(load_geoip_config(&self.db).await)
        } else {
            None
        };

        Ok(WorkerBootstrap {
            instance_id: instance_id.to_string(),
            role,
            panel_ipc_port: self.panel_ipc_port,
            ipc_token: generate_token(),
            data_dir: self.data_dir.to_string_lossy().into_owned(),
            redis,
            manticore,
            worker_config,
            filter_bundle,
            geoip,
        })
    }

    pub async fn stop_worker(&self, instance_id: &str, graceful_timeout_secs: u64) -> Result<()> {
        let timeout_secs = graceful_timeout_secs.clamp(1, 300);

        let (worker_port, ipc_token, exit_notify, has_process) = {
            let mut guard = self
                .workers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = guard.get_mut(instance_id) else {
                return Ok(());
            };
            entry.manual_stop = true;
            if let Some(cancel) = entry.backoff_cancel.take() {
                cancel.cancel();
            }
            if entry.kill_tx.is_none() && entry.pid.is_none() {
                entry.runtime_state = WorkerRuntimeState::Stopped;
                entry.worker_port = None;
                return Ok(());
            }
            entry.runtime_state = WorkerRuntimeState::Draining;
            (
                entry.worker_port,
                entry.ipc_token.clone(),
                entry.exit_notify.clone(),
                true,
            )
        };

        if !has_process {
            return Ok(());
        }

        if let Some(port) = worker_port {
            let url = format!("http://127.0.0.1:{port}/internal/v1/shutdown");
            let req = WorkerShutdownRequest {
                graceful_timeout_secs: timeout_secs,
            };
            if let Err(err) = self
                .http_client
                .post(&url)
                .bearer_auth(&ipc_token)
                .json(&req)
                .send()
                .await
            {
                tracing::warn!("通知 Worker {instance_id} 优雅停机失败，准备兜底清理: {err}");
            }
        }

        let deadline = Instant::now() + Duration::from_secs(timeout_secs.saturating_add(2));
        loop {
            let still_alive = {
                let guard = self
                    .workers
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.get(instance_id).is_some_and(|e| e.kill_tx.is_some())
            };
            if !still_alive || Instant::now() >= deadline {
                break;
            }

            if let Some(port) = worker_port {
                let drain_url = format!("http://127.0.0.1:{port}/internal/v1/drain-status");
                if let Ok(resp) = self
                    .http_client
                    .get(&drain_url)
                    .bearer_auth(&ipc_token)
                    .send()
                    .await
                    && let Ok(status) = resp.json::<WorkerDrainStatus>().await
                {
                    let mut guard = self
                        .workers
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(entry) = guard.get_mut(instance_id) {
                        entry.drain_status = Some(status);
                    }
                }
            }

            let wait_slice = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(150));
            let _ = tokio::time::timeout(wait_slice, exit_notify.notified()).await;
        }

        // 若超时仍未退出，发送强制终止信号
        let kill_tx = {
            let mut guard = self
                .workers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.get_mut(instance_id).and_then(|e| e.kill_tx.take())
        };
        if let Some(tx) = kill_tx {
            tracing::warn!("Worker {instance_id} 优雅停机超时，正在强制终止...");
            let _ = tx.send(());
            let _ = tokio::time::timeout(Duration::from_secs(3), exit_notify.notified()).await;
        }

        let mut guard = self
            .workers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = guard.get_mut(instance_id) {
            entry.runtime_state = WorkerRuntimeState::Stopped;
            entry.worker_port = None;
            entry.pid = None;
            entry.drain_status = None;
        }
        Ok(())
    }

    pub async fn restart_worker(
        self: &Arc<Self>,
        instance_id: &str,
        graceful_timeout_secs: u64,
    ) -> Result<()> {
        self.stop_worker(instance_id, graceful_timeout_secs).await?;
        self.start_worker(instance_id).await
    }

    /// 向所有在线 `indexer` 广播过滤规则与动态清洗词库热更新。
    pub async fn reload_all_indexers(
        &self,
        version: i64,
        bundle: &FilterBundle,
    ) -> Vec<IndexerReloadResult> {
        let targets: Vec<(String, u16, String)> = {
            let guard = self
                .workers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .values()
                .filter(|w| {
                    w.role == WorkerRole::Indexer
                        && w.runtime_state == WorkerRuntimeState::Running
                        && w.worker_port.is_some()
                })
                .filter_map(|w| {
                    w.worker_port
                        .map(|p| (w.instance_id.clone(), p, w.ipc_token.clone()))
                })
                .collect()
        };

        let bundle_value = match serde_json::to_value(bundle) {
            Ok(v) => v,
            Err(err) => {
                return targets
                    .into_iter()
                    .map(|(id, _, _)| IndexerReloadResult {
                        instance_id: id,
                        ok: false,
                        active_rule_version: None,
                        message: format!("序列化规则包失败: {err}"),
                    })
                    .collect();
            }
        };

        let mut results = Vec::with_capacity(targets.len());
        for (instance_id, port, token) in targets {
            let url = format!("http://127.0.0.1:{port}/internal/v1/rules/reload");
            let req = WorkerReloadRulesRequest {
                version,
                filter_bundle: bundle_value.clone(),
            };
            match self
                .http_client
                .post(&url)
                .bearer_auth(&token)
                .json(&req)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<WorkerReloadRulesResponse>().await {
                        Ok(parsed) => {
                            let mut guard = self
                                .workers
                                .write()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if let Some(entry) = guard.get_mut(&instance_id) {
                                entry.active_rule_version = Some(parsed.active_rule_version);
                            }
                            results.push(IndexerReloadResult {
                                instance_id,
                                ok: true,
                                active_rule_version: Some(parsed.active_rule_version),
                                message: format!("已切换至 v{}", parsed.active_rule_version),
                            });
                        }
                        Err(err) => results.push(IndexerReloadResult {
                            instance_id,
                            ok: false,
                            active_rule_version: None,
                            message: format!("解析热更新响应失败: {err}"),
                        }),
                    }
                }
                Ok(resp) => {
                    let st = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    results.push(IndexerReloadResult {
                        instance_id,
                        ok: false,
                        active_rule_version: None,
                        message: format!("Indexer 返回 HTTP {st}: {body}"),
                    });
                }
                Err(err) => results.push(IndexerReloadResult {
                    instance_id,
                    ok: false,
                    active_rule_version: None,
                    message: format!("请求 Indexer 热更新接口失败: {err}"),
                }),
            }
        }
        results
    }

    /// 向所有在线 `crawler` 广播 GeoIP 离线库路径与采样配置热重载。
    pub async fn reload_all_crawlers_geoip(
        &self,
        config: &GeoIpConfig,
    ) -> Vec<CrawlerGeoIpReloadResult> {
        let targets: Vec<(String, u16, String)> = {
            let guard = self
                .workers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .values()
                .filter(|w| {
                    w.role == WorkerRole::Crawler
                        && w.runtime_state == WorkerRuntimeState::Running
                        && w.worker_port.is_some()
                })
                .filter_map(|w| {
                    w.worker_port
                        .map(|p| (w.instance_id.clone(), p, w.ipc_token.clone()))
                })
                .collect()
        };

        let req = WorkerReloadGeoIpRequest {
            config: config.clone(),
        };
        let mut results = Vec::with_capacity(targets.len());
        for (instance_id, port, token) in targets {
            let url = format!("http://127.0.0.1:{port}/internal/v1/geoip/reload");
            match self
                .http_client
                .post(&url)
                .bearer_auth(&token)
                .json(&req)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<WorkerReloadGeoIpResponse>().await {
                        Ok(parsed) => {
                            let message = if parsed.mmdb_loaded {
                                format!(
                                    "已重载 MMDB（{} · {} 字节）",
                                    parsed.database_type, parsed.mmdb_size_bytes
                                )
                            } else if !parsed.enabled {
                                "GeoIP 归因已禁用".to_string()
                            } else {
                                parsed
                                    .error
                                    .clone()
                                    .unwrap_or_else(|| "MMDB 未加载（降级模式）".to_string())
                            };
                            results.push(CrawlerGeoIpReloadResult {
                                instance_id,
                                ok: true,
                                mmdb_loaded: parsed.mmdb_loaded,
                                error: parsed.error,
                                message,
                            });
                        }
                        Err(err) => results.push(CrawlerGeoIpReloadResult {
                            instance_id,
                            ok: false,
                            mmdb_loaded: false,
                            error: Some(err.to_string()),
                            message: format!("解析 Crawler GeoIP 重载响应失败: {err}"),
                        }),
                    }
                }
                Ok(resp) => {
                    let st = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    results.push(CrawlerGeoIpReloadResult {
                        instance_id,
                        ok: false,
                        mmdb_loaded: false,
                        error: Some(body.clone()),
                        message: format!("Crawler 返回 HTTP {st}: {body}"),
                    });
                }
                Err(err) => results.push(CrawlerGeoIpReloadResult {
                    instance_id,
                    ok: false,
                    mmdb_loaded: false,
                    error: Some(err.to_string()),
                    message: format!("请求 Crawler GeoIP 热重载接口失败: {err}"),
                }),
            }
        }
        results
    }

    async fn fetch_and_store_snapshot(&self, instance_id: &str, port: u16, token: &str) {
        let url = format!("http://127.0.0.1:{port}/internal/v1/snapshot");
        if let Ok(resp) = self.http_client.get(&url).bearer_auth(token).send().await
            && resp.status().is_success()
            && let Ok(env) = resp.json::<WorkerSnapshotEnvelope>().await
        {
            let mut guard = self
                .workers
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(entry) = guard.get_mut(instance_id) {
                entry.latest_snapshot = Some(env);
                entry.snapshot_seq = entry.snapshot_seq.wrapping_add(1).max(1);
                entry.snapshot_fetched_at_ms = Utc::now().timestamp_millis();
            }
        }
    }

    pub async fn poll_worker_snapshot_once(&self, instance_id: &str) {
        let target: Option<(u16, String)> = {
            let guard = self
                .workers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.get(instance_id).and_then(|w| {
                if w.runtime_state == WorkerRuntimeState::Running {
                    w.worker_port.map(|p| (p, w.ipc_token.clone()))
                } else {
                    None
                }
            })
        };
        if let Some((port, token)) = target {
            self.fetch_and_store_snapshot(instance_id, port, &token)
                .await;
        }
    }

    pub async fn poll_snapshots_once(&self) {
        let targets: Vec<(String, u16, String)> = {
            let guard = self
                .workers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard
                .values()
                .filter(|w| w.runtime_state == WorkerRuntimeState::Running)
                .filter_map(|w| {
                    w.worker_port
                        .map(|p| (w.instance_id.clone(), p, w.ipc_token.clone()))
                })
                .collect()
        };

        for (instance_id, port, token) in targets {
            self.fetch_and_store_snapshot(&instance_id, port, &token)
                .await;
        }
    }

    pub fn spawn_snapshot_poller(self: &Arc<Self>) {
        let sup = self.clone();
        let cancel = self.ipc_shutdown.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        sup.poll_snapshots_once().await;
                    }
                }
            }
        });
    }

    pub async fn list_worker_statuses(&self) -> Result<Vec<WorkerStatusView>> {
        let rows = self.db.list_workers().await?;
        let guard = self
            .workers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::with_capacity(rows.len());

        for row in rows {
            let role: WorkerRole = row.role.parse().unwrap_or(WorkerRole::Crawler);
            let desired_state: DesiredState =
                row.desired_state.parse().unwrap_or(DesiredState::Stopped);
            let managed = guard.get(&row.id);

            out.push(WorkerStatusView {
                id: row.id,
                role,
                display_name: row.display_name,
                desired_state,
                runtime_state: managed.map_or(WorkerRuntimeState::Stopped, |m| m.runtime_state),
                active_config_version: row.active_config_version,
                auto_restart: row.auto_restart != 0,
                max_restarts: row.max_restarts,
                restart_backoff_secs: row.restart_backoff_secs,
                pid: managed.and_then(|m| m.pid),
                worker_port: managed.and_then(|m| m.worker_port),
                active_rule_version: managed.and_then(|m| m.active_rule_version),
                started_at: managed.and_then(|m| m.started_at),
                restart_count: managed.map_or(0, |m| m.restart_count),
                last_exit_code: managed.and_then(|m| m.last_exit_code),
                last_error: managed.and_then(|m| m.last_error.clone()),
                stderr_tail: managed
                    .map(|m| m.stderr_ring.snapshot_text())
                    .unwrap_or_default(),
                drain_status: managed.and_then(|m| m.drain_status.clone()),
                snapshot: managed.and_then(|m| m.latest_snapshot.clone()),
                snapshot_seq: managed.map_or(0, |m| m.snapshot_seq),
                snapshot_fetched_at_ms: managed.map_or(0, |m| m.snapshot_fetched_at_ms),
            });
        }
        Ok(out)
    }

    pub fn get_worker_snapshot(&self, instance_id: &str) -> Option<WorkerSnapshotEnvelope> {
        let guard = self
            .workers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get(instance_id)
            .and_then(|m| m.latest_snapshot.clone())
    }

    pub fn subscribe_worker_logs(&self, instance_id: &str) -> Option<broadcast::Receiver<String>> {
        let guard = self
            .workers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.get(instance_id).map(|m| m.log_tx.subscribe())
    }

    pub async fn shutdown_all(self: &Arc<Self>) {
        self.shutting_down.store(true, Ordering::Relaxed);
        let ids: Vec<String> = {
            let guard = self
                .workers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.keys().cloned().collect()
        };
        for id in ids {
            let _ = self.stop_worker(&id, 10).await;
        }
        self.ipc_shutdown.cancel();
    }
}

pub async fn load_published_filter_bundle(db: &Database) -> Result<FilterBundle> {
    let Some(row) = db.get_published_filter_version().await? else {
        return Ok(default_online_filter_bundle());
    };
    let policy = hameta_filter::parse_stored_policy_json(&row.filter_json)
        .context("解析数据库中已发布 filter_json 失败")?;
    let ad_phrases = serde_json::from_str(&row.ad_phrases_json)
        .context("解析数据库中已发布 ad_phrases_json 失败")?;
    let ad_regexes = serde_json::from_str(&row.ad_regexes_json)
        .context("解析数据库中已发布 ad_regexes_json 失败")?;
    Ok(FilterBundle {
        version: row.version,
        policy,
        ad_phrases,
        ad_regexes,
    })
}

pub fn build_panel_ipc_router(supervisor: Arc<Supervisor>) -> Router {
    Router::new()
        .route("/internal/v1/register", post(register_handler))
        .route("/internal/v1/ready", post(ready_handler))
        .route("/internal/v1/ingest-events", post(ingest_events_handler))
        .with_state(supervisor)
}

async fn register_handler(
    State(sup): State<Arc<Supervisor>>,
    headers: HeaderMap,
    Json(req): Json<WorkerRegisterRequest>,
) -> Response {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let mut guard = sup
        .workers
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = guard.get_mut(&req.instance_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "NOT_FOUND", "message": "未知 Worker 实例" })),
        )
            .into_response();
    };
    if !verify_bearer_token(auth, &entry.ipc_token) || req.ipc_token != entry.ipc_token {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "code": "UNAUTHORIZED", "message": "Worker 注册令牌无效" })),
        )
            .into_response();
    }
    entry.worker_port = Some(req.worker_port);
    entry.pid = Some(req.pid);
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

async fn ready_handler(
    State(sup): State<Arc<Supervisor>>,
    headers: HeaderMap,
    Json(req): Json<WorkerReadyRequest>,
) -> Response {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let mut guard = sup
        .workers
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(entry) = guard.get_mut(&req.instance_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "code": "NOT_FOUND", "message": "未知 Worker 实例" })),
        )
            .into_response();
    };
    if !verify_bearer_token(auth, &entry.ipc_token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "code": "UNAUTHORIZED", "message": "Worker 就绪令牌无效" })),
        )
            .into_response();
    }
    if entry.runtime_state != WorkerRuntimeState::Draining {
        entry.runtime_state = WorkerRuntimeState::Running;
    }
    if req.rule_version.is_some() {
        entry.active_rule_version = req.rule_version;
    }
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

async fn ingest_events_handler(
    State(sup): State<Arc<Supervisor>>,
    headers: HeaderMap,
    Json(req): Json<WorkerIngestEventsRequest>,
) -> Response {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    {
        let guard = sup
            .workers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = guard.get(&req.instance_id) else {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "code": "NOT_FOUND", "message": "未知 Worker 实例" })),
            )
                .into_response();
        };
        if !verify_bearer_token(auth, &entry.ipc_token) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "code": "UNAUTHORIZED", "message": "Worker 上报令牌无效" })),
            )
                .into_response();
        }
    }

    let accepted = sup.ingest_hub.publish_batch(req.events);
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "accepted": accepted })),
    )
        .into_response()
}

fn tail_summary(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    trimmed.chars().skip(count - max_chars).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn panel_ipc_authenticates_worker_register_and_ready() {
        let db = Database::open_in_memory().await.unwrap();
        let dir = std::env::temp_dir().join(format!("hameta-sup-ipc-{}", generate_token()));
        let sup = Supervisor::new(db, dir.clone()).await.unwrap();

        {
            let mut guard = sup.workers.write().unwrap();
            let mut entry = ManagedWorker::new("crawler-1", WorkerRole::Crawler);
            entry.ipc_token = "secret-ipc-999".to_string();
            entry.runtime_state = WorkerRuntimeState::Starting;
            guard.insert("crawler-1".to_string(), entry);
        }

        let app = build_panel_ipc_router(sup.clone());
        let reg_body = serde_json::to_vec(&WorkerRegisterRequest {
            instance_id: "crawler-1".to_string(),
            worker_port: 45123,
            pid: 9876,
            ipc_token: "secret-ipc-999".to_string(),
        })
        .unwrap();

        let bad = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/v1/register")
                    .header("authorization", "Bearer wrong")
                    .header("content-type", "application/json")
                    .body(Body::from(reg_body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);

        let ok_reg = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/v1/register")
                    .header("authorization", "Bearer secret-ipc-999")
                    .header("content-type", "application/json")
                    .body(Body::from(reg_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok_reg.status(), StatusCode::OK);

        let ready_body = serde_json::to_vec(&WorkerReadyRequest {
            instance_id: "crawler-1".to_string(),
            rule_version: Some(3),
        })
        .unwrap();
        let ok_ready = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/internal/v1/ready")
                    .header("authorization", "Bearer secret-ipc-999")
                    .header("content-type", "application/json")
                    .body(Body::from(ready_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok_ready.status(), StatusCode::OK);

        {
            let guard = sup.workers.read().unwrap();
            let entry = guard.get("crawler-1").unwrap();
            assert_eq!(entry.runtime_state, WorkerRuntimeState::Running);
            assert_eq!(entry.worker_port, Some(45123));
            assert_eq!(entry.pid, Some(9876));
            assert_eq!(entry.active_rule_version, Some(3));
        }

        sup.shutdown_all().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
