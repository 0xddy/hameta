use crate::db::{Database, now_unix};
use crate::error::StorageError;
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct TorrentTaskRow {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub params_json: String,
    pub cursor_id: i64,
    pub estimated_total: i64,
    pub scanned_count: i64,
    pub updated_count: i64,
    pub skipped_count: i64,
    pub error_count: i64,
    pub batches_done: i64,
    pub rate_per_sec: f64,
    pub last_error: String,
    pub sample_diffs_json: String,
    pub created_by: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TorrentTaskProgressUpdate {
    pub cursor_id: i64,
    pub estimated_total: Option<i64>,
    pub scanned_count: i64,
    pub updated_count: i64,
    pub skipped_count: i64,
    pub error_count: i64,
    pub batches_done: i64,
    pub rate_per_sec: f64,
    pub last_error: String,
    pub sample_diffs_json: String,
}

impl Database {
    pub async fn create_torrent_task(
        &self,
        kind: &str,
        params_json: &str,
        estimated_total: i64,
        created_by: &str,
    ) -> Result<TorrentTaskRow, StorageError> {
        let now = now_unix();
        let res = sqlx::query(
            "INSERT INTO torrent_tasks (
                kind, status, params_json, cursor_id, estimated_total,
                scanned_count, updated_count, skipped_count, error_count, batches_done,
                rate_per_sec, last_error, sample_diffs_json, created_by,
                created_at, started_at, updated_at, finished_at
             ) VALUES (
                ?1, 'running', ?2, 0, ?3,
                0, 0, 0, 0, 0,
                0.0, '', '[]', ?4,
                ?5, ?5, ?5, NULL
             )",
        )
        .bind(kind)
        .bind(params_json)
        .bind(estimated_total.max(0))
        .bind(created_by)
        .bind(now)
        .execute(self.pool())
        .await?;

        let id = res.last_insert_rowid();
        self.get_torrent_task(id)
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("未找到刚创建的任务: {id}")))
    }

    pub async fn get_torrent_task(&self, id: i64) -> Result<Option<TorrentTaskRow>, StorageError> {
        let row = sqlx::query_as::<_, TorrentTaskRow>(
            "SELECT id, kind, status, params_json, cursor_id, estimated_total,
                    scanned_count, updated_count, skipped_count, error_count, batches_done,
                    rate_per_sec, last_error, sample_diffs_json, created_by,
                    created_at, started_at, updated_at, finished_at
             FROM torrent_tasks
             WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn list_torrent_tasks(
        &self,
        limit: i64,
    ) -> Result<Vec<TorrentTaskRow>, StorageError> {
        let cap = limit.clamp(1, 200);
        let rows = sqlx::query_as::<_, TorrentTaskRow>(
            "SELECT id, kind, status, params_json, cursor_id, estimated_total,
                    scanned_count, updated_count, skipped_count, error_count, batches_done,
                    rate_per_sec, last_error, sample_diffs_json, created_by,
                    created_at, started_at, updated_at, finished_at
             FROM torrent_tasks
             ORDER BY id DESC
             LIMIT ?1",
        )
        .bind(cap)
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    pub async fn find_running_torrent_task(&self) -> Result<Option<TorrentTaskRow>, StorageError> {
        let row = sqlx::query_as::<_, TorrentTaskRow>(
            "SELECT id, kind, status, params_json, cursor_id, estimated_total,
                    scanned_count, updated_count, skipped_count, error_count, batches_done,
                    rate_per_sec, last_error, sample_diffs_json, created_by,
                    created_at, started_at, updated_at, finished_at
             FROM torrent_tasks
             WHERE status = 'running'
             ORDER BY id DESC
             LIMIT 1",
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn update_torrent_task_progress(
        &self,
        id: i64,
        progress: &TorrentTaskProgressUpdate,
    ) -> Result<(), StorageError> {
        let now = now_unix();
        if let Some(est) = progress.estimated_total {
            sqlx::query(
                "UPDATE torrent_tasks
                 SET cursor_id = ?1,
                     estimated_total = ?2,
                     scanned_count = ?3,
                     updated_count = ?4,
                     skipped_count = ?5,
                     error_count = ?6,
                     batches_done = ?7,
                     rate_per_sec = ?8,
                     last_error = ?9,
                     sample_diffs_json = ?10,
                     updated_at = ?11
                 WHERE id = ?12",
            )
            .bind(progress.cursor_id)
            .bind(est.max(0))
            .bind(progress.scanned_count)
            .bind(progress.updated_count)
            .bind(progress.skipped_count)
            .bind(progress.error_count)
            .bind(progress.batches_done)
            .bind(progress.rate_per_sec)
            .bind(&progress.last_error)
            .bind(&progress.sample_diffs_json)
            .bind(now)
            .bind(id)
            .execute(self.pool())
            .await?;
        } else {
            sqlx::query(
                "UPDATE torrent_tasks
                 SET cursor_id = ?1,
                     scanned_count = ?2,
                     updated_count = ?3,
                     skipped_count = ?4,
                     error_count = ?5,
                     batches_done = ?6,
                     rate_per_sec = ?7,
                     last_error = ?8,
                     sample_diffs_json = ?9,
                     updated_at = ?10
                 WHERE id = ?11",
            )
            .bind(progress.cursor_id)
            .bind(progress.scanned_count)
            .bind(progress.updated_count)
            .bind(progress.skipped_count)
            .bind(progress.error_count)
            .bind(progress.batches_done)
            .bind(progress.rate_per_sec)
            .bind(&progress.last_error)
            .bind(&progress.sample_diffs_json)
            .bind(now)
            .bind(id)
            .execute(self.pool())
            .await?;
        }
        Ok(())
    }

    pub async fn set_torrent_task_status(
        &self,
        id: i64,
        status: &str,
        last_error: Option<&str>,
    ) -> Result<TorrentTaskRow, StorageError> {
        let now = now_unix();
        let finished_at = if matches!(status, "completed" | "cancelled" | "failed") {
            Some(now)
        } else {
            None
        };
        let started_at_bump = if status == "running" { Some(now) } else { None };

        sqlx::query(
            "UPDATE torrent_tasks
             SET status = ?1,
                 last_error = COALESCE(?2, last_error),
                 started_at = COALESCE(started_at, ?3),
                 updated_at = ?4,
                 finished_at = ?5
             WHERE id = ?6",
        )
        .bind(status)
        .bind(last_error)
        .bind(started_at_bump)
        .bind(now)
        .bind(finished_at)
        .bind(id)
        .execute(self.pool())
        .await?;

        self.get_torrent_task(id)
            .await?
            .ok_or_else(|| StorageError::NotFound(format!("未找到任务: {id}")))
    }

    /// 面板重启恢复：将上次异常退出残留的 `running` 任务标记为 `paused`，保留游标便于断点续传。
    pub async fn pause_stale_running_torrent_tasks(&self) -> Result<u64, StorageError> {
        let now = now_unix();
        let res = sqlx::query(
            "UPDATE torrent_tasks
             SET status = 'paused',
                 last_error = CASE
                     WHEN last_error = '' THEN '面板重启已自动暂停，可点击恢复从断点继续'
                     ELSE last_error
                 END,
                 updated_at = ?1
             WHERE status = 'running'",
        )
        .bind(now)
        .execute(self.pool())
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn delete_torrent_task(&self, id: i64) -> Result<bool, StorageError> {
        let res = sqlx::query("DELETE FROM torrent_tasks WHERE id = ?1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
