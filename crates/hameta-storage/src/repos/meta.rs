use crate::db::{Database, now_unix};
use crate::error::StorageError;

pub const META_INITIALIZED_KEY: &str = "initialized";

impl Database {
    pub async fn get_meta(&self, key: &str) -> Result<Option<String>, StorageError> {
        let row: Option<(String,)> = sqlx::query_as("SELECT value FROM system_meta WHERE key = ?1")
            .bind(key)
            .fetch_optional(self.pool())
            .await?;
        Ok(row.map(|(v,)| v))
    }

    pub async fn set_meta(&self, key: &str, value: &str) -> Result<(), StorageError> {
        let now = now_unix();
        sqlx::query(
            "INSERT INTO system_meta (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(now)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn is_initialized(&self) -> Result<bool, StorageError> {
        Ok(self.get_meta(META_INITIALIZED_KEY).await?.as_deref() == Some("true"))
    }

    pub async fn mark_initialized(&self) -> Result<(), StorageError> {
        self.set_meta(META_INITIALIZED_KEY, "true").await
    }
}
