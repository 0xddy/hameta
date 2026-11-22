use crate::db::{Database, now_unix};
use crate::error::StorageError;
use hameta_common::ServiceKind;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct ServiceConfigRow {
    pub id: i64,
    pub kind: String,
    pub version: i64,
    pub config_json: String,
    pub is_active: i64,
    pub last_test_ok: Option<i64>,
    pub last_test_message: Option<String>,
    pub last_tested_at: Option<i64>,
    pub created_by: String,
    pub note: String,
    pub created_at: i64,
}

impl Database {
    pub async fn get_active_service_config(
        &self,
        kind: ServiceKind,
    ) -> Result<Option<ServiceConfigRow>, StorageError> {
        let row = sqlx::query_as::<_, ServiceConfigRow>(
            "SELECT id, kind, version, config_json, is_active, last_test_ok, last_test_message,
                    last_tested_at, created_by, note, created_at
             FROM service_configs
             WHERE kind = ?1 AND is_active = 1
             ORDER BY version DESC LIMIT 1",
        )
        .bind(kind.as_str())
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn save_service_config(
        &self,
        kind: ServiceKind,
        config_json: &str,
        created_by: &str,
        note: &str,
    ) -> Result<ServiceConfigRow, StorageError> {
        let now = now_unix();
        let kind_str = kind.as_str();
        let mut tx = self.pool().begin().await?;

        let (max_ver,): (Option<i64>,) =
            sqlx::query_as("SELECT MAX(version) FROM service_configs WHERE kind = ?1")
                .bind(kind_str)
                .fetch_one(&mut *tx)
                .await?;
        let next_version = max_ver.unwrap_or(0) + 1;

        sqlx::query("UPDATE service_configs SET is_active = 0 WHERE kind = ?1 AND is_active = 1")
            .bind(kind_str)
            .execute(&mut *tx)
            .await?;

        let id = sqlx::query(
            "INSERT INTO service_configs
             (kind, version, config_json, is_active, created_by, note, created_at)
             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6)",
        )
        .bind(kind_str)
        .bind(next_version)
        .bind(config_json)
        .bind(created_by)
        .bind(note)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();

        tx.commit().await?;

        Ok(ServiceConfigRow {
            id,
            kind: kind_str.to_string(),
            version: next_version,
            config_json: config_json.to_string(),
            is_active: 1,
            last_test_ok: None,
            last_test_message: None,
            last_tested_at: None,
            created_by: created_by.to_string(),
            note: note.to_string(),
            created_at: now,
        })
    }

    pub async fn update_service_test_result(
        &self,
        kind: ServiceKind,
        version: i64,
        ok: bool,
        message: &str,
    ) -> Result<(), StorageError> {
        let now = now_unix();
        sqlx::query(
            "UPDATE service_configs
             SET last_test_ok = ?1, last_test_message = ?2, last_tested_at = ?3
             WHERE kind = ?4 AND version = ?5",
        )
        .bind(i64::from(ok))
        .bind(message)
        .bind(now)
        .bind(kind.as_str())
        .bind(version)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn list_service_config_versions(
        &self,
        kind: ServiceKind,
        limit: i64,
    ) -> Result<Vec<ServiceConfigRow>, StorageError> {
        let rows = sqlx::query_as::<_, ServiceConfigRow>(
            "SELECT id, kind, version, config_json, is_active, last_test_ok, last_test_message,
                    last_tested_at, created_by, note, created_at
             FROM service_configs
             WHERE kind = ?1
             ORDER BY version DESC
             LIMIT ?2",
        )
        .bind(kind.as_str())
        .bind(limit.max(1))
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    pub async fn rollback_service_config(
        &self,
        kind: ServiceKind,
        target_version: i64,
        created_by: &str,
    ) -> Result<ServiceConfigRow, StorageError> {
        let kind_str = kind.as_str();
        let target = sqlx::query_as::<_, ServiceConfigRow>(
            "SELECT id, kind, version, config_json, is_active, last_test_ok, last_test_message,
                    last_tested_at, created_by, note, created_at
             FROM service_configs
             WHERE kind = ?1 AND version = ?2
             LIMIT 1",
        )
        .bind(kind_str)
        .bind(target_version)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| {
            StorageError::NotFound(format!("服务配置版本 {kind_str} v{target_version} 不存在"))
        })?;

        let note = format!("回滚自 v{target_version}");
        self.save_service_config(kind, &target.config_json, created_by, &note)
            .await
    }
}
