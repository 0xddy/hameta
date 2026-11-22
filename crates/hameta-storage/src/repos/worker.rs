use crate::db::{Database, now_unix};
use crate::error::StorageError;
use hameta_common::{DesiredState, WorkerRole};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct WorkerInstanceRow {
    pub id: String,
    pub role: String,
    pub display_name: String,
    pub desired_state: String,
    pub active_config_version: i64,
    pub auto_restart: i64,
    pub max_restarts: i64,
    pub restart_backoff_secs: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct WorkerConfigVersionRow {
    pub id: i64,
    pub instance_id: String,
    pub version: i64,
    pub config_json: String,
    pub created_by: String,
    pub note: String,
    pub created_at: i64,
}

impl Database {
    pub async fn list_workers(&self) -> Result<Vec<WorkerInstanceRow>, StorageError> {
        let rows = sqlx::query_as::<_, WorkerInstanceRow>(
            "SELECT id, role, display_name, desired_state, active_config_version,
                    auto_restart, max_restarts, restart_backoff_secs, created_at, updated_at
             FROM worker_instances
             ORDER BY role ASC, created_at ASC",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    pub async fn get_worker(
        &self,
        instance_id: &str,
    ) -> Result<Option<WorkerInstanceRow>, StorageError> {
        let row = sqlx::query_as::<_, WorkerInstanceRow>(
            "SELECT id, role, display_name, desired_state, active_config_version,
                    auto_restart, max_restarts, restart_backoff_secs, created_at, updated_at
             FROM worker_instances
             WHERE id = ?1
             LIMIT 1",
        )
        .bind(instance_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn create_worker(
        &self,
        instance_id: &str,
        role: WorkerRole,
        display_name: &str,
        config_json: &str,
        created_by: &str,
    ) -> Result<WorkerInstanceRow, StorageError> {
        let instance_id = instance_id.trim();
        let display_name = display_name.trim();
        if instance_id.is_empty() || display_name.is_empty() {
            return Err(StorageError::InvalidInput(
                "Worker 实例 ID 与显示名称不能为空".to_string(),
            ));
        }

        let now = now_unix();
        let role_str = role.as_str();
        let mut tx = self.pool().begin().await?;

        sqlx::query(
            "INSERT INTO worker_instances
             (id, role, display_name, desired_state, active_config_version, auto_restart, max_restarts, restart_backoff_secs, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'stopped', 1, 1, 5, 3, ?4, ?5)",
        )
        .bind(instance_id)
        .bind(role_str)
        .bind(display_name)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO worker_config_versions
             (instance_id, version, config_json, created_by, note, created_at)
             VALUES (?1, 1, ?2, ?3, '初始配置', ?4)",
        )
        .bind(instance_id)
        .bind(config_json)
        .bind(created_by)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(WorkerInstanceRow {
            id: instance_id.to_string(),
            role: role_str.to_string(),
            display_name: display_name.to_string(),
            desired_state: DesiredState::Stopped.as_str().to_string(),
            active_config_version: 1,
            auto_restart: 1,
            max_restarts: 5,
            restart_backoff_secs: 3,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn update_worker_policy(
        &self,
        instance_id: &str,
        display_name: &str,
        auto_restart: bool,
        max_restarts: i64,
        restart_backoff_secs: i64,
    ) -> Result<(), StorageError> {
        let now = now_unix();
        let res = sqlx::query(
            "UPDATE worker_instances
             SET display_name = ?1, auto_restart = ?2, max_restarts = ?3, restart_backoff_secs = ?4, updated_at = ?5
             WHERE id = ?6",
        )
        .bind(display_name.trim())
        .bind(i64::from(auto_restart))
        .bind(max_restarts.max(0))
        .bind(restart_backoff_secs.max(1))
        .bind(now)
        .bind(instance_id)
        .execute(self.pool())
        .await?;

        if res.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "Worker 实例 {instance_id} 不存在"
            )));
        }
        Ok(())
    }

    pub async fn set_worker_desired_state(
        &self,
        instance_id: &str,
        state: DesiredState,
    ) -> Result<(), StorageError> {
        let now = now_unix();
        let res = sqlx::query(
            "UPDATE worker_instances SET desired_state = ?1, updated_at = ?2 WHERE id = ?3",
        )
        .bind(state.as_str())
        .bind(now)
        .bind(instance_id)
        .execute(self.pool())
        .await?;

        if res.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "Worker 实例 {instance_id} 不存在"
            )));
        }
        Ok(())
    }

    pub async fn delete_worker(&self, instance_id: &str) -> Result<(), StorageError> {
        let res = sqlx::query("DELETE FROM worker_instances WHERE id = ?1")
            .bind(instance_id)
            .execute(self.pool())
            .await?;
        if res.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "Worker 实例 {instance_id} 不存在"
            )));
        }
        Ok(())
    }

    pub async fn get_active_worker_config(
        &self,
        instance_id: &str,
    ) -> Result<Option<WorkerConfigVersionRow>, StorageError> {
        let row = sqlx::query_as::<_, WorkerConfigVersionRow>(
            "SELECT v.id, v.instance_id, v.version, v.config_json, v.created_by, v.note, v.created_at
             FROM worker_config_versions v
             JOIN worker_instances w
               ON w.id = v.instance_id AND w.active_config_version = v.version
             WHERE w.id = ?1
             LIMIT 1",
        )
        .bind(instance_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn save_worker_config_version(
        &self,
        instance_id: &str,
        config_json: &str,
        created_by: &str,
        note: &str,
    ) -> Result<WorkerConfigVersionRow, StorageError> {
        let now = now_unix();
        let mut tx = self.pool().begin().await?;

        let exists: Option<(String,)> =
            sqlx::query_as("SELECT id FROM worker_instances WHERE id = ?1 LIMIT 1")
                .bind(instance_id)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(StorageError::NotFound(format!(
                "Worker 实例 {instance_id} 不存在"
            )));
        }

        let (max_ver,): (Option<i64>,) = sqlx::query_as(
            "SELECT MAX(version) FROM worker_config_versions WHERE instance_id = ?1",
        )
        .bind(instance_id)
        .fetch_one(&mut *tx)
        .await?;
        let next_version = max_ver.unwrap_or(0) + 1;

        let id = sqlx::query(
            "INSERT INTO worker_config_versions
             (instance_id, version, config_json, created_by, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(instance_id)
        .bind(next_version)
        .bind(config_json)
        .bind(created_by)
        .bind(note)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();

        sqlx::query(
            "UPDATE worker_instances
             SET active_config_version = ?1, updated_at = ?2
             WHERE id = ?3",
        )
        .bind(next_version)
        .bind(now)
        .bind(instance_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(WorkerConfigVersionRow {
            id,
            instance_id: instance_id.to_string(),
            version: next_version,
            config_json: config_json.to_string(),
            created_by: created_by.to_string(),
            note: note.to_string(),
            created_at: now,
        })
    }

    pub async fn list_worker_config_versions(
        &self,
        instance_id: &str,
        limit: i64,
    ) -> Result<Vec<WorkerConfigVersionRow>, StorageError> {
        let rows = sqlx::query_as::<_, WorkerConfigVersionRow>(
            "SELECT id, instance_id, version, config_json, created_by, note, created_at
             FROM worker_config_versions
             WHERE instance_id = ?1
             ORDER BY version DESC
             LIMIT ?2",
        )
        .bind(instance_id)
        .bind(limit.max(1))
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    pub async fn rollback_worker_config(
        &self,
        instance_id: &str,
        target_version: i64,
        created_by: &str,
    ) -> Result<WorkerConfigVersionRow, StorageError> {
        let target = sqlx::query_as::<_, WorkerConfigVersionRow>(
            "SELECT id, instance_id, version, config_json, created_by, note, created_at
             FROM worker_config_versions
             WHERE instance_id = ?1 AND version = ?2
             LIMIT 1",
        )
        .bind(instance_id)
        .bind(target_version)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| {
            StorageError::NotFound(format!(
                "Worker {instance_id} 的配置版本 v{target_version} 不存在"
            ))
        })?;

        let note = format!("回滚自 v{target_version}");
        self.save_worker_config_version(instance_id, &target.config_json, created_by, &note)
            .await
    }
}
