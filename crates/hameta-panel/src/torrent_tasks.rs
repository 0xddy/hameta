use crate::services::ServiceRuntimePool;
use crate::supervisor::load_published_filter_bundle;
use anyhow::Result;
use hameta_common::{FileInfo, TABLE_TORRENTS};
use hameta_filter::TitleCleaner;
use hameta_indexer::{
    MaintenanceTorrentDoc, ManticoreClient, normalize_file_info_value, sum_file_info_json,
};
use hameta_storage::{Database, TorrentTaskProgressUpdate, TorrentTaskRow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const STOP_NONE: u8 = 0;
const STOP_PAUSE: u8 = 1;
const STOP_CANCEL: u8 = 2;

const DEFAULT_BATCH_SIZE: usize = 500;
const MIN_BATCH_SIZE: usize = 50;
const MAX_BATCH_SIZE: usize = 2_000;
const DEFAULT_BATCH_DELAY_MS: u64 = 100;
const MAX_BATCH_DELAY_MS: u64 = 5_000;
const MAX_SAMPLE_DIFFS: usize = 15;
const BATCH_MAX_RETRIES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TorrentTaskKind {
    SyncTotalSize,
    RecleanTitles,
}

impl TorrentTaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SyncTotalSize => "sync_total_size",
            Self::RecleanTitles => "reclean_titles",
        }
    }

    pub fn from_db_str(raw: &str) -> Self {
        match raw {
            "reclean_titles" => Self::RecleanTitles,
            _ => Self::SyncTotalSize,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecleanScope {
    #[default]
    All,
    EmptyOrUnchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentTaskParams {
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default = "default_batch_delay_ms")]
    pub batch_delay_ms: u64,
    #[serde(default)]
    pub max_docs: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
    /// `sync_total_size` 专用：仅扫描 `total_size = 0` 的存量旧记录（速度快）
    #[serde(default = "default_true")]
    pub only_missing: bool,
    /// `reclean_titles` 专用：清洗范围
    #[serde(default)]
    pub clean_scope: RecleanScope,
    /// `reclean_titles` 专用：清洗结果为空时保留原有 `refine_name`
    #[serde(default = "default_true")]
    pub skip_empty_result: bool,
    /// `reclean_titles` 启动时记录的当前已发布过滤规则版本
    #[serde(default)]
    pub rule_version: Option<i64>,
}

impl Default for TorrentTaskParams {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
            batch_delay_ms: DEFAULT_BATCH_DELAY_MS,
            max_docs: None,
            dry_run: false,
            only_missing: true,
            clean_scope: RecleanScope::All,
            skip_empty_result: true,
            rule_version: None,
        }
    }
}

impl TorrentTaskParams {
    pub fn normalize(&mut self) {
        self.batch_size = self.batch_size.clamp(MIN_BATCH_SIZE, MAX_BATCH_SIZE);
        self.batch_delay_ms = self.batch_delay_ms.min(MAX_BATCH_DELAY_MS);
        if self.max_docs == Some(0) {
            self.max_docs = None;
        }
    }
}

fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}

fn default_batch_delay_ms() -> u64 {
    DEFAULT_BATCH_DELAY_MS
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSampleDiff {
    pub id: String,
    pub info_hash: String,
    pub label: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TorrentTaskDto {
    pub id: i64,
    pub kind: TorrentTaskKind,
    pub status: String,
    pub params: TorrentTaskParams,
    /// 63 位 Manticore 主键游标，以十进制字符串输出防 JS 丢精度
    pub cursor_id: String,
    pub estimated_total: i64,
    pub scanned_count: i64,
    pub updated_count: i64,
    pub skipped_count: i64,
    pub error_count: i64,
    pub batches_done: i64,
    pub rate_per_sec: f64,
    pub progress_percent: f64,
    pub eta_secs: Option<u64>,
    pub last_error: String,
    pub sample_diffs: Vec<TaskSampleDiff>,
    pub created_by: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateTorrentTaskRequest {
    pub kind: TorrentTaskKind,
    #[serde(default)]
    pub batch_size: Option<usize>,
    #[serde(default)]
    pub batch_delay_ms: Option<u64>,
    #[serde(default)]
    pub max_docs: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default = "default_true")]
    pub only_missing: bool,
    #[serde(default)]
    pub clean_scope: RecleanScope,
    #[serde(default = "default_true")]
    pub skip_empty_result: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TorrentTasksOverviewDto {
    pub has_total_size_column: bool,
    pub published_rule_version: Option<i64>,
    pub active_task: Option<TorrentTaskDto>,
    pub tasks: Vec<TorrentTaskDto>,
}

struct ActiveTaskHandle {
    task_id: i64,
    cancel_token: CancellationToken,
    stop_mode: Arc<AtomicU8>,
}

pub struct TorrentTaskManager {
    db: Database,
    services: Arc<ServiceRuntimePool>,
    active: Mutex<Option<ActiveTaskHandle>>,
}

impl TorrentTaskManager {
    pub fn new(db: Database, services: Arc<ServiceRuntimePool>) -> Arc<Self> {
        Arc::new(Self {
            db,
            services,
            active: Mutex::new(None),
        })
    }

    /// 面板启动时将上次残留的 `running` 任务恢复为 `paused`，保留断点游标。
    pub async fn recover_stale_tasks_on_startup(&self) {
        match self.db.pause_stale_running_torrent_tasks().await {
            Ok(count) if count > 0 => {
                tracing::info!(
                    "面板启动已将 {count} 条残留运行中磁力维护任务置为暂停（支持断点恢复）"
                );
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!("恢复残留磁力维护任务状态失败: {err}");
            }
        }
    }

    /// 面板停机时优雅暂停当前运行中的异步维护任务，确保断点游标落盘。
    pub async fn pause_active_on_shutdown(&self) {
        let handle = {
            let guard = self.active.lock().await;
            guard
                .as_ref()
                .map(|h| (h.cancel_token.clone(), h.stop_mode.clone()))
        };
        if let Some((token, mode)) = handle {
            mode.store(STOP_PAUSE, Ordering::SeqCst);
            token.cancel();
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    pub async fn overview(&self, limit: i64) -> Result<TorrentTasksOverviewDto, String> {
        let has_total_size_column =
            if let Some((_, client)) = self.services.manticore_client().await {
                client.supports_total_size().await
            } else {
                false
            };
        let published_rule_version = self
            .db
            .get_published_filter_version()
            .await
            .ok()
            .flatten()
            .map(|r| r.version);

        let rows = self
            .db
            .list_torrent_tasks(limit)
            .await
            .map_err(|e| format!("读取任务列表失败: {e}"))?;
        let tasks: Vec<TorrentTaskDto> = rows.into_iter().map(row_to_dto).collect();
        let active_task = tasks
            .iter()
            .find(|t| t.status == "running")
            .or_else(|| tasks.iter().find(|t| t.status == "paused"))
            .cloned();

        Ok(TorrentTasksOverviewDto {
            has_total_size_column,
            published_rule_version,
            active_task,
            tasks,
        })
    }

    pub async fn get_task(&self, id: i64) -> Result<Option<TorrentTaskDto>, String> {
        let row = self
            .db
            .get_torrent_task(id)
            .await
            .map_err(|e| format!("查询任务失败: {e}"))?;
        Ok(row.map(row_to_dto))
    }

    pub async fn start_new_task(
        self: &Arc<Self>,
        req: CreateTorrentTaskRequest,
        operator: &str,
    ) -> Result<TorrentTaskDto, String> {
        let Some((_, client)) = self.services.manticore_client().await else {
            return Err("尚未配置或连接 Manticore 服务，无法启动维护任务".to_string());
        };

        let mut active_guard = self.active.lock().await;
        if let Some(ref cur) = *active_guard {
            return Err(format!(
                "当前已有运行中的批量维护任务 (#{})，请先暂停或等待完成后再启动新任务，避免并发整行写库互相覆盖",
                cur.task_id
            ));
        }

        let has_total_size = client.supports_total_size().await;
        if req.kind == TorrentTaskKind::SyncTotalSize && !has_total_size {
            return Err(
                "当前 torrents 表尚未添加 total_size 列，请先在 SQL 终端执行: ALTER TABLE torrents ADD COLUMN total_size bigint;"
                    .to_string(),
            );
        }

        let mut params = TorrentTaskParams {
            batch_size: req.batch_size.unwrap_or(DEFAULT_BATCH_SIZE),
            batch_delay_ms: req.batch_delay_ms.unwrap_or(DEFAULT_BATCH_DELAY_MS),
            max_docs: req.max_docs,
            dry_run: req.dry_run,
            only_missing: req.only_missing,
            clean_scope: req.clean_scope,
            skip_empty_result: req.skip_empty_result,
            rule_version: None,
        };
        params.normalize();

        // 若是清洗标题任务，预先编译当前已发布过滤规则词库，避免启动后才因规则报错
        let cleaner = if req.kind == TorrentTaskKind::RecleanTitles {
            let bundle = load_published_filter_bundle(&self.db)
                .await
                .map_err(|e| format!("加载当前已发布过滤清洗规则失败: {e}"))?;
            params.rule_version = Some(bundle.version);
            let compiled = TitleCleaner::compile_with_options(
                &bundle.ad_phrases,
                &bundle.ad_regexes,
                &bundle.policy.cleaner,
            )
            .map_err(|e| format!("编译标题清洗器失败: {e}"))?;
            Some(Arc::new(compiled))
        } else {
            None
        };

        // 精确统计预估总行数（不受 /search max_matches=1000 截断）
        let only_zero = req.kind == TorrentTaskKind::SyncTotalSize && params.only_missing;
        let raw_count = client.count_torrents(only_zero).await.unwrap_or(0);
        let estimated_u64 = match params.max_docs {
            Some(limit) if raw_count > 0 => raw_count.min(limit),
            Some(limit) => limit,
            None => raw_count,
        };
        let estimated_total = i64::try_from(estimated_u64).unwrap_or(i64::MAX);

        let params_json =
            serde_json::to_string(&params).map_err(|e| format!("序列化任务参数失败: {e}"))?;
        let row = self
            .db
            .create_torrent_task(req.kind.as_str(), &params_json, estimated_total, operator)
            .await
            .map_err(|e| format!("创建任务记录失败: {e}"))?;

        let cancel_token = CancellationToken::new();
        let stop_mode = Arc::new(AtomicU8::new(STOP_NONE));
        *active_guard = Some(ActiveTaskHandle {
            task_id: row.id,
            cancel_token: cancel_token.clone(),
            stop_mode: stop_mode.clone(),
        });
        drop(active_guard);

        let mgr = Arc::clone(self);
        let row_clone = row.clone();
        tokio::spawn(async move {
            mgr.run_task_loop(
                row_clone,
                params,
                client,
                has_total_size,
                cleaner,
                cancel_token,
                stop_mode,
            )
            .await;
        });

        Ok(row_to_dto(row))
    }

    pub async fn pause_task(&self, id: i64) -> Result<TorrentTaskDto, String> {
        let Some(row) = self
            .db
            .get_torrent_task(id)
            .await
            .map_err(|e| format!("查询任务失败: {e}"))?
        else {
            return Err(format!("未找到任务 #{id}"));
        };
        if row.status != "running" {
            return Err(format!("任务 #{id} 当前状态为 {}，无法暂停", row.status));
        }

        let maybe_signal = {
            let guard = self.active.lock().await;
            guard.as_ref().and_then(|h| {
                if h.task_id == id {
                    Some((h.cancel_token.clone(), h.stop_mode.clone()))
                } else {
                    None
                }
            })
        };

        if let Some((token, mode)) = maybe_signal {
            mode.store(STOP_PAUSE, Ordering::SeqCst);
            token.cancel();
            // 等待当前批次收尾落盘（最多 1.5s），若仍在写库则状态很快变为 paused
            for _ in 0..15 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Ok(Some(cur)) = self.db.get_torrent_task(id).await
                    && cur.status == "paused"
                {
                    return Ok(row_to_dto(cur));
                }
            }
            let latest = self
                .db
                .get_torrent_task(id)
                .await
                .map_err(|e| format!("读取任务状态失败: {e}"))?
                .ok_or_else(|| format!("未找到任务 #{id}"))?;
            Ok(row_to_dto(latest))
        } else {
            // 内存中已无句柄（防御兜底），直接置 paused
            let updated = self
                .db
                .set_torrent_task_status(id, "paused", None)
                .await
                .map_err(|e| format!("暂停任务失败: {e}"))?;
            Ok(row_to_dto(updated))
        }
    }

    pub async fn resume_task(self: &Arc<Self>, id: i64) -> Result<TorrentTaskDto, String> {
        let Some((_, client)) = self.services.manticore_client().await else {
            return Err("尚未配置或连接 Manticore 服务，无法恢复任务".to_string());
        };

        let mut active_guard = self.active.lock().await;
        if let Some(ref cur) = *active_guard {
            return Err(format!(
                "当前已有运行中的批量维护任务 (#{})，请先暂停或等待其完成",
                cur.task_id
            ));
        }

        let Some(row) = self
            .db
            .get_torrent_task(id)
            .await
            .map_err(|e| format!("查询任务失败: {e}"))?
        else {
            return Err(format!("未找到任务 #{id}"));
        };
        if !matches!(row.status.as_str(), "paused" | "failed") {
            return Err(format!(
                "仅支持恢复「已暂停」或「失败」状态的任务（当前为 {}）",
                row.status
            ));
        }

        let kind = TorrentTaskKind::from_db_str(&row.kind);
        let has_total_size = client.supports_total_size().await;
        if kind == TorrentTaskKind::SyncTotalSize && !has_total_size {
            return Err(
                "当前 torrents 表尚未添加 total_size 列，请先在 SQL 终端执行: ALTER TABLE torrents ADD COLUMN total_size bigint;"
                    .to_string(),
            );
        }

        let mut params: TorrentTaskParams =
            serde_json::from_str(&row.params_json).unwrap_or_default();
        params.normalize();

        let cleaner = if kind == TorrentTaskKind::RecleanTitles {
            let bundle = load_published_filter_bundle(&self.db)
                .await
                .map_err(|e| format!("加载过滤清洗规则失败: {e}"))?;
            let compiled = TitleCleaner::compile_with_options(
                &bundle.ad_phrases,
                &bundle.ad_regexes,
                &bundle.policy.cleaner,
            )
            .map_err(|e| format!("编译标题清洗器失败: {e}"))?;
            Some(Arc::new(compiled))
        } else {
            None
        };

        let resumed_row = self
            .db
            .set_torrent_task_status(id, "running", Some(""))
            .await
            .map_err(|e| format!("恢复任务状态失败: {e}"))?;

        let cancel_token = CancellationToken::new();
        let stop_mode = Arc::new(AtomicU8::new(STOP_NONE));
        *active_guard = Some(ActiveTaskHandle {
            task_id: id,
            cancel_token: cancel_token.clone(),
            stop_mode: stop_mode.clone(),
        });
        drop(active_guard);

        let mgr = Arc::clone(self);
        let row_clone = resumed_row.clone();
        tokio::spawn(async move {
            mgr.run_task_loop(
                row_clone,
                params,
                client,
                has_total_size,
                cleaner,
                cancel_token,
                stop_mode,
            )
            .await;
        });

        Ok(row_to_dto(resumed_row))
    }

    pub async fn cancel_task(&self, id: i64) -> Result<TorrentTaskDto, String> {
        let Some(row) = self
            .db
            .get_torrent_task(id)
            .await
            .map_err(|e| format!("查询任务失败: {e}"))?
        else {
            return Err(format!("未找到任务 #{id}"));
        };
        if matches!(row.status.as_str(), "completed" | "cancelled") {
            return Ok(row_to_dto(row));
        }

        let maybe_signal = {
            let guard = self.active.lock().await;
            guard.as_ref().and_then(|h| {
                if h.task_id == id {
                    Some((h.cancel_token.clone(), h.stop_mode.clone()))
                } else {
                    None
                }
            })
        };

        if let Some((token, mode)) = maybe_signal {
            mode.store(STOP_CANCEL, Ordering::SeqCst);
            token.cancel();
            for _ in 0..15 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Ok(Some(cur)) = self.db.get_torrent_task(id).await
                    && cur.status == "cancelled"
                {
                    return Ok(row_to_dto(cur));
                }
            }
            let latest = self
                .db
                .get_torrent_task(id)
                .await
                .map_err(|e| format!("读取任务状态失败: {e}"))?
                .ok_or_else(|| format!("未找到任务 #{id}"))?;
            Ok(row_to_dto(latest))
        } else {
            let updated = self
                .db
                .set_torrent_task_status(id, "cancelled", None)
                .await
                .map_err(|e| format!("取消任务失败: {e}"))?;
            Ok(row_to_dto(updated))
        }
    }

    pub async fn delete_task(&self, id: i64) -> Result<bool, String> {
        let Some(row) = self
            .db
            .get_torrent_task(id)
            .await
            .map_err(|e| format!("查询任务失败: {e}"))?
        else {
            return Err(format!("未找到任务 #{id}"));
        };
        if row.status == "running" {
            return Err("无法删除正在运行中的任务，请先暂停或取消".to_string());
        }
        self.db
            .delete_torrent_task(id)
            .await
            .map_err(|e| format!("删除任务记录失败: {e}"))
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_task_loop(
        self: Arc<Self>,
        initial: TorrentTaskRow,
        params: TorrentTaskParams,
        client: ManticoreClient,
        has_total_size: bool,
        cleaner: Option<Arc<TitleCleaner>>,
        cancel_token: CancellationToken,
        stop_mode: Arc<AtomicU8>,
    ) {
        let task_id = initial.id;
        let kind = TorrentTaskKind::from_db_str(&initial.kind);

        let mut cursor_id = initial.cursor_id;
        let mut scanned_count = initial.scanned_count;
        let mut updated_count = initial.updated_count;
        let mut skipped_count = initial.skipped_count;
        let mut error_count = initial.error_count;
        let mut batches_done = initial.batches_done;
        let mut sample_diffs: Vec<TaskSampleDiff> =
            serde_json::from_str(&initial.sample_diffs_json).unwrap_or_default();

        let session_started = Instant::now();
        let mut session_scanned: u64 = 0;

        let final_status: (&'static str, Option<String>) = loop {
            if cancel_token.is_cancelled() {
                let mode = stop_mode.load(Ordering::SeqCst);
                let st = if mode == STOP_CANCEL {
                    "cancelled"
                } else {
                    "paused"
                };
                break (st, None);
            }

            // 计算本批实际拉取数量（尊重 max_docs 上限）
            let batch_limit = if let Some(max_d) = params.max_docs {
                let scanned_u64 = u64::try_from(scanned_count.max(0)).unwrap_or(0);
                if scanned_u64 >= max_d {
                    break ("completed", None);
                }
                let remaining = max_d.saturating_sub(scanned_u64);
                usize::try_from(remaining)
                    .unwrap_or(params.batch_size)
                    .min(params.batch_size)
            } else {
                params.batch_size
            };

            // 1) 按 63 位主键游标拉取下一批记录（O(batch_size) 稳定扫描，支持千万级表且不受 max_matches 限制）
            let batch_result = fetch_batch_with_retry(
                &client,
                cursor_id,
                batch_limit,
                kind == TorrentTaskKind::SyncTotalSize && params.only_missing,
                has_total_size,
                &cancel_token,
            )
            .await;

            let docs = match batch_result {
                Ok(docs) => docs,
                Err(err) => {
                    if cancel_token.is_cancelled() {
                        let st = if stop_mode.load(Ordering::SeqCst) == STOP_CANCEL {
                            "cancelled"
                        } else {
                            "paused"
                        };
                        break (st, None);
                    }
                    error_count += 1;
                    break ("failed", Some(format!("拉取 Manticore 批次失败: {err:#}")));
                }
            };

            if docs.is_empty() {
                break ("completed", None);
            }

            let mut dirty_docs: Vec<MaintenanceTorrentDoc> = Vec::new();
            let mut batch_max_id = cursor_id;
            let mut batch_skipped: i64 = 0;

            for mut doc in docs {
                if doc.id > batch_max_id {
                    batch_max_id = doc.id;
                }
                scanned_count += 1;
                session_scanned += 1;

                match kind {
                    TorrentTaskKind::SyncTotalSize => {
                        let computed_u64 = sum_file_info_json(&doc.file_info);
                        let computed = i64::try_from(computed_u64).unwrap_or(i64::MAX);
                        if computed != doc.total_size {
                            if sample_diffs.len() < MAX_SAMPLE_DIFFS {
                                sample_diffs.push(TaskSampleDiff {
                                    id: doc.id.to_string(),
                                    info_hash: doc.info_hash.clone(),
                                    label: if doc.refine_name.is_empty() {
                                        doc.raw_name.clone()
                                    } else {
                                        doc.refine_name.clone()
                                    },
                                    before: format_bytes_human(
                                        u64::try_from(doc.total_size.max(0)).unwrap_or(0),
                                    ),
                                    after: format_bytes_human(computed_u64),
                                });
                            }
                            doc.total_size = computed;
                            dirty_docs.push(doc);
                        } else {
                            batch_skipped += 1;
                        }
                    }
                    TorrentTaskKind::RecleanTitles => {
                        let skip_by_scope = params.clean_scope == RecleanScope::EmptyOrUnchanged
                            && !doc.refine_name.trim().is_empty()
                            && doc.refine_name != doc.raw_name;
                        if skip_by_scope {
                            batch_skipped += 1;
                            continue;
                        }
                        let Some(ref cl) = cleaner else {
                            batch_skipped += 1;
                            continue;
                        };
                        let new_refine = if cl.options().enable_filename_cross_clean {
                            let files: Vec<FileInfo> =
                                serde_json::from_str(&doc.file_info).unwrap_or_default();
                            cl.refine_with_files(&doc.raw_name, &files)
                        } else {
                            cl.refine(&doc.raw_name)
                        };
                        if new_refine.trim().is_empty() && params.skip_empty_result {
                            batch_skipped += 1;
                            continue;
                        }
                        if new_refine != doc.refine_name {
                            if sample_diffs.len() < MAX_SAMPLE_DIFFS {
                                sample_diffs.push(TaskSampleDiff {
                                    id: doc.id.to_string(),
                                    info_hash: doc.info_hash.clone(),
                                    label: doc.raw_name.clone(),
                                    before: doc.refine_name.clone(),
                                    after: new_refine.clone(),
                                });
                            }
                            doc.refine_name = new_refine;
                            // 顺带回填 total_size（若列存在且旧记录为 0）
                            if has_total_size && doc.total_size <= 0 {
                                doc.total_size = i64::try_from(sum_file_info_json(&doc.file_info))
                                    .unwrap_or(i64::MAX);
                            }
                            dirty_docs.push(doc);
                        } else {
                            batch_skipped += 1;
                        }
                    }
                }
            }

            // 2) 非预演模式下仅把实际发生变化的文档整批回写 Manticore
            if !dirty_docs.is_empty() {
                if params.dry_run {
                    updated_count += i64::try_from(dirty_docs.len()).unwrap_or(i64::MAX);
                } else {
                    match write_batch_with_retry(
                        &client,
                        &dirty_docs,
                        has_total_size,
                        &cancel_token,
                    )
                    .await
                    {
                        Ok(written) => {
                            updated_count += i64::try_from(written).unwrap_or(i64::MAX);
                        }
                        Err(err) => {
                            if cancel_token.is_cancelled() {
                                let st = if stop_mode.load(Ordering::SeqCst) == STOP_CANCEL {
                                    "cancelled"
                                } else {
                                    "paused"
                                };
                                break (st, None);
                            }
                            error_count += 1;
                            break ("failed", Some(format!("批量回写 Manticore 失败: {err:#}")));
                        }
                    }
                }
            }

            skipped_count += batch_skipped;
            batches_done += 1;
            cursor_id = batch_max_id;

            let elapsed = session_started.elapsed().as_secs_f64();
            let rate_per_sec = if elapsed > 0.05 {
                (session_scanned as f64 / elapsed * 10.0).round() / 10.0
            } else {
                0.0
            };

            let sample_diffs_json =
                serde_json::to_string(&sample_diffs).unwrap_or_else(|_| "[]".to_string());
            let _ = self
                .db
                .update_torrent_task_progress(
                    task_id,
                    &TorrentTaskProgressUpdate {
                        cursor_id,
                        estimated_total: None,
                        scanned_count,
                        updated_count,
                        skipped_count,
                        error_count,
                        batches_done,
                        rate_per_sec,
                        last_error: String::new(),
                        sample_diffs_json,
                    },
                )
                .await;

            // 若满足 max_docs 提前结束
            if let Some(max_d) = params.max_docs
                && u64::try_from(scanned_count.max(0)).unwrap_or(0) >= max_d
            {
                break ("completed", None);
            }

            // 批次流控休眠，避免抢占线上爬虫与索引器 I/O
            if params.batch_delay_ms > 0 {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        let st = if stop_mode.load(Ordering::SeqCst) == STOP_CANCEL {
                            "cancelled"
                        } else {
                            "paused"
                        };
                        break (st, None);
                    }
                    _ = tokio::time::sleep(Duration::from_millis(params.batch_delay_ms)) => {}
                }
            }
        };

        let final_elapsed = session_started.elapsed().as_secs_f64();
        let final_rate = if final_elapsed > 0.05 {
            (session_scanned as f64 / final_elapsed * 10.0).round() / 10.0
        } else {
            initial.rate_per_sec
        };
        let sample_diffs_json =
            serde_json::to_string(&sample_diffs).unwrap_or_else(|_| "[]".to_string());
        let _ = self
            .db
            .update_torrent_task_progress(
                task_id,
                &TorrentTaskProgressUpdate {
                    cursor_id,
                    estimated_total: None,
                    scanned_count,
                    updated_count,
                    skipped_count,
                    error_count,
                    batches_done,
                    rate_per_sec: final_rate,
                    last_error: final_status.1.clone().unwrap_or_default(),
                    sample_diffs_json,
                },
            )
            .await;
        let _ = self
            .db
            .set_torrent_task_status(task_id, final_status.0, final_status.1.as_deref())
            .await;

        let mut guard = self.active.lock().await;
        if guard.as_ref().is_some_and(|h| h.task_id == task_id) {
            *guard = None;
        }
        tracing::info!(
            "磁力库异步维护任务 #{task_id} ({}) 结束: status={}, scanned={scanned_count}, updated={updated_count}, skipped={skipped_count}",
            kind.as_str(),
            final_status.0
        );
    }
}

async fn fetch_batch_with_retry(
    client: &ManticoreClient,
    cursor_id: i64,
    limit: usize,
    only_zero_total_size: bool,
    has_total_size: bool,
    cancel: &CancellationToken,
) -> Result<Vec<MaintenanceTorrentDoc>> {
    let mut last_err = None;
    for attempt in 0..BATCH_MAX_RETRIES {
        match fetch_batch_after_cursor(
            client,
            cursor_id,
            limit,
            only_zero_total_size,
            has_total_size,
        )
        .await
        {
            Ok(docs) => return Ok(docs),
            Err(err) => {
                last_err = Some(err);
                if attempt + 1 < BATCH_MAX_RETRIES {
                    let delay =
                        Duration::from_millis(500 * u64::try_from(attempt + 1).unwrap_or(1));
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("未知 Manticore 查询错误")))
}

async fn write_batch_with_retry(
    client: &ManticoreClient,
    docs: &[MaintenanceTorrentDoc],
    include_total_size: bool,
    cancel: &CancellationToken,
) -> Result<usize> {
    let mut last_err = None;
    for attempt in 0..BATCH_MAX_RETRIES {
        match client
            .batch_replace_maintenance_docs(docs, include_total_size)
            .await
        {
            Ok(n) => return Ok(n),
            Err(err) => {
                last_err = Some(err);
                if attempt + 1 < BATCH_MAX_RETRIES {
                    let delay =
                        Duration::from_millis(500 * u64::try_from(attempt + 1).unwrap_or(1));
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("未知 Manticore 批量写入错误")))
}

async fn fetch_batch_after_cursor(
    client: &ManticoreClient,
    cursor_id: i64,
    limit: usize,
    only_zero_total_size: bool,
    has_total_size: bool,
) -> Result<Vec<MaintenanceTorrentDoc>> {
    let query = if only_zero_total_size && has_total_size {
        json!({
            "bool": {
                "must": [
                    { "range": { "id": { "gt": cursor_id } } },
                    { "equals": { "total_size": 0 } }
                ]
            }
        })
    } else {
        json!({ "range": { "id": { "gt": cursor_id } } })
    };

    let mut source_fields = vec![
        "info_hash",
        "raw_name",
        "refine_name",
        "file_count",
        "file_info",
        "hot",
        "created_at",
        "updated_at",
    ];
    if has_total_size {
        source_fields.push("total_size");
    }

    let body = json!({
        "index": TABLE_TORRENTS,
        "query": query,
        "_source": source_fields,
        "sort": [{ "id": { "order": "asc" } }],
        "limit": limit,
        "options": { "max_matches": limit.max(1000) }
    });

    let resp = client.search_json(&body).await?;
    let mut out = Vec::new();
    if let Some(hits) = resp["hits"]["hits"].as_array() {
        out.reserve(hits.len());
        for hit in hits {
            let id = parse_i64_any(&hit["_id"]).unwrap_or(0);
            if id <= 0 {
                continue;
            }
            let src = &hit["_source"];
            out.push(MaintenanceTorrentDoc {
                id,
                info_hash: src["info_hash"].as_str().unwrap_or_default().to_string(),
                raw_name: src["raw_name"].as_str().unwrap_or_default().to_string(),
                refine_name: src["refine_name"].as_str().unwrap_or_default().to_string(),
                file_count: i32::try_from(parse_i64_any(&src["file_count"]).unwrap_or(0))
                    .unwrap_or(0),
                total_size: src.get("total_size").and_then(parse_i64_any).unwrap_or(0),
                file_info: normalize_file_info_value(src.get("file_info")),
                hot: i32::try_from(parse_i64_any(&src["hot"]).unwrap_or(0)).unwrap_or(0),
                created_at: parse_i64_any(&src["created_at"]).unwrap_or(0),
                updated_at: parse_i64_any(&src["updated_at"]).unwrap_or(0),
            });
        }
    }
    Ok(out)
}

fn parse_i64_any(val: &Value) -> Option<i64> {
    match val {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

pub fn row_to_dto(row: TorrentTaskRow) -> TorrentTaskDto {
    let kind = TorrentTaskKind::from_db_str(&row.kind);
    let params: TorrentTaskParams = serde_json::from_str(&row.params_json).unwrap_or_default();
    let sample_diffs: Vec<TaskSampleDiff> =
        serde_json::from_str(&row.sample_diffs_json).unwrap_or_default();

    let progress_percent = if row.status == "completed" {
        100.0
    } else if row.estimated_total > 0 {
        let pct = (row.scanned_count as f64 / row.estimated_total as f64) * 100.0;
        (pct.clamp(0.0, 99.9) * 10.0).round() / 10.0
    } else {
        0.0
    };

    let eta_secs = if row.status == "running"
        && row.estimated_total > row.scanned_count
        && row.rate_per_sec > 0.1
    {
        let remaining = (row.estimated_total - row.scanned_count) as f64;
        Some((remaining / row.rate_per_sec).ceil() as u64)
    } else {
        None
    };

    TorrentTaskDto {
        id: row.id,
        kind,
        status: row.status,
        params,
        cursor_id: row.cursor_id.to_string(),
        estimated_total: row.estimated_total,
        scanned_count: row.scanned_count,
        updated_count: row.updated_count,
        skipped_count: row.skipped_count,
        error_count: row.error_count,
        batches_done: row.batches_done,
        rate_per_sec: row.rate_per_sec,
        progress_percent,
        eta_secs,
        last_error: row.last_error,
        sample_diffs,
        created_by: row.created_by,
        created_at: row.created_at,
        started_at: row.started_at,
        updated_at: row.updated_at,
        finished_at: row.finished_at,
    }
}

fn format_bytes_human(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= TIB {
        format!("{:.2} TB", b / TIB)
    } else if b >= GIB {
        format!("{:.2} GB", b / GIB)
    } else if b >= MIB {
        format!("{:.2} MB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_normalize_clamps_bounds() {
        let mut p = TorrentTaskParams {
            batch_size: 10,
            batch_delay_ms: 99_999,
            max_docs: Some(0),
            ..TorrentTaskParams::default()
        };
        p.normalize();
        assert_eq!(p.batch_size, MIN_BATCH_SIZE);
        assert_eq!(p.batch_delay_ms, MAX_BATCH_DELAY_MS);
        assert_eq!(p.max_docs, None);
    }

    #[test]
    fn dto_computes_progress_eta_and_string_cursor() {
        let row = TorrentTaskRow {
            id: 7,
            kind: "reclean_titles".to_string(),
            status: "running".to_string(),
            params_json: "{}".to_string(),
            cursor_id: 1_745_280_108_080_464_937,
            estimated_total: 10_000,
            scanned_count: 2_500,
            updated_count: 600,
            skipped_count: 1_900,
            error_count: 0,
            batches_done: 5,
            rate_per_sec: 500.0,
            last_error: String::new(),
            sample_diffs_json: "[]".to_string(),
            created_by: "admin".to_string(),
            created_at: 1_700_000_000,
            started_at: Some(1_700_000_000),
            updated_at: 1_700_000_005,
            finished_at: None,
        };
        let dto = row_to_dto(row);
        assert_eq!(dto.cursor_id, "1745280108080464937");
        assert!((dto.progress_percent - 25.0).abs() < 0.01);
        assert_eq!(dto.eta_secs, Some(15));
    }
}
