use crate::db::{Database, now_unix};
use crate::error::StorageError;
use hameta_common::FilterVersionStatus;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct FilterRuleVersionRow {
    pub id: i64,
    pub version: i64,
    pub status: String,
    pub filter_json: String,
    pub ad_phrases_json: String,
    pub ad_regexes_json: String,
    pub created_by: String,
    pub note: String,
    pub created_at: i64,
    pub published_at: Option<i64>,
}

impl Database {
    pub async fn get_published_filter_version(
        &self,
    ) -> Result<Option<FilterRuleVersionRow>, StorageError> {
        let row = sqlx::query_as::<_, FilterRuleVersionRow>(
            "SELECT id, version, status, filter_json, ad_phrases_json, ad_regexes_json,
                    created_by, note, created_at, published_at
             FROM filter_rule_versions
             WHERE status = 'published'
             ORDER BY version DESC LIMIT 1",
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn get_latest_draft_filter_version(
        &self,
    ) -> Result<Option<FilterRuleVersionRow>, StorageError> {
        let row = sqlx::query_as::<_, FilterRuleVersionRow>(
            "SELECT id, version, status, filter_json, ad_phrases_json, ad_regexes_json,
                    created_by, note, created_at, published_at
             FROM filter_rule_versions
             WHERE status = 'draft'
             ORDER BY version DESC LIMIT 1",
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn save_filter_draft(
        &self,
        filter_json: &str,
        ad_phrases_json: &str,
        ad_regexes_json: &str,
        created_by: &str,
        note: &str,
    ) -> Result<FilterRuleVersionRow, StorageError> {
        let now = now_unix();
        let mut tx = self.pool().begin().await?;

        let existing_draft = sqlx::query_as::<_, FilterRuleVersionRow>(
            "SELECT id, version, status, filter_json, ad_phrases_json, ad_regexes_json,
                    created_by, note, created_at, published_at
             FROM filter_rule_versions
             WHERE status = 'draft'
             ORDER BY version DESC LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;

        let saved = if let Some(draft) = existing_draft {
            sqlx::query(
                "UPDATE filter_rule_versions
                 SET filter_json = ?1, ad_phrases_json = ?2, ad_regexes_json = ?3,
                     created_by = ?4, note = ?5, created_at = ?6
                 WHERE id = ?7",
            )
            .bind(filter_json)
            .bind(ad_phrases_json)
            .bind(ad_regexes_json)
            .bind(created_by)
            .bind(note)
            .bind(now)
            .bind(draft.id)
            .execute(&mut *tx)
            .await?;

            FilterRuleVersionRow {
                id: draft.id,
                version: draft.version,
                status: FilterVersionStatus::Draft.as_str().to_string(),
                filter_json: filter_json.to_string(),
                ad_phrases_json: ad_phrases_json.to_string(),
                ad_regexes_json: ad_regexes_json.to_string(),
                created_by: created_by.to_string(),
                note: note.to_string(),
                created_at: now,
                published_at: None,
            }
        } else {
            let (max_ver,): (Option<i64>,) =
                sqlx::query_as("SELECT MAX(version) FROM filter_rule_versions")
                    .fetch_one(&mut *tx)
                    .await?;
            let next_version = max_ver.unwrap_or(0) + 1;

            let id = sqlx::query(
                "INSERT INTO filter_rule_versions
                 (version, status, filter_json, ad_phrases_json, ad_regexes_json, created_by, note, created_at, published_at)
                 VALUES (?1, 'draft', ?2, ?3, ?4, ?5, ?6, ?7, NULL)",
            )
            .bind(next_version)
            .bind(filter_json)
            .bind(ad_phrases_json)
            .bind(ad_regexes_json)
            .bind(created_by)
            .bind(note)
            .bind(now)
            .execute(&mut *tx)
            .await?
            .last_insert_rowid();

            FilterRuleVersionRow {
                id,
                version: next_version,
                status: FilterVersionStatus::Draft.as_str().to_string(),
                filter_json: filter_json.to_string(),
                ad_phrases_json: ad_phrases_json.to_string(),
                ad_regexes_json: ad_regexes_json.to_string(),
                created_by: created_by.to_string(),
                note: note.to_string(),
                created_at: now,
                published_at: None,
            }
        };

        tx.commit().await?;
        Ok(saved)
    }

    pub async fn publish_filter_draft(
        &self,
        published_by: &str,
        note: Option<&str>,
    ) -> Result<FilterRuleVersionRow, StorageError> {
        let now = now_unix();
        let mut tx = self.pool().begin().await?;

        let draft = sqlx::query_as::<_, FilterRuleVersionRow>(
            "SELECT id, version, status, filter_json, ad_phrases_json, ad_regexes_json,
                    created_by, note, created_at, published_at
             FROM filter_rule_versions
             WHERE status = 'draft'
             ORDER BY version DESC LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound("当前没有待发布的过滤规则草稿".to_string()))?;

        sqlx::query(
            "UPDATE filter_rule_versions SET status = 'archived' WHERE status = 'published'",
        )
        .execute(&mut *tx)
        .await?;

        let final_note = note.unwrap_or(&draft.note);
        sqlx::query(
            "UPDATE filter_rule_versions
             SET status = 'published', created_by = ?1, note = ?2, published_at = ?3
             WHERE id = ?4",
        )
        .bind(published_by)
        .bind(final_note)
        .bind(now)
        .bind(draft.id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(FilterRuleVersionRow {
            id: draft.id,
            version: draft.version,
            status: FilterVersionStatus::Published.as_str().to_string(),
            filter_json: draft.filter_json,
            ad_phrases_json: draft.ad_phrases_json,
            ad_regexes_json: draft.ad_regexes_json,
            created_by: published_by.to_string(),
            note: final_note.to_string(),
            created_at: draft.created_at,
            published_at: Some(now),
        })
    }

    pub async fn list_filter_versions(
        &self,
        limit: i64,
    ) -> Result<Vec<FilterRuleVersionRow>, StorageError> {
        let rows = sqlx::query_as::<_, FilterRuleVersionRow>(
            "SELECT id, version, status, filter_json, ad_phrases_json, ad_regexes_json,
                    created_by, note, created_at, published_at
             FROM filter_rule_versions
             ORDER BY version DESC
             LIMIT ?1",
        )
        .bind(limit.max(1))
        .fetch_all(self.pool())
        .await?;
        Ok(rows)
    }

    pub async fn rollback_filter_version(
        &self,
        target_version: i64,
        created_by: &str,
    ) -> Result<FilterRuleVersionRow, StorageError> {
        let now = now_unix();
        let mut tx = self.pool().begin().await?;

        let target = sqlx::query_as::<_, FilterRuleVersionRow>(
            "SELECT id, version, status, filter_json, ad_phrases_json, ad_regexes_json,
                    created_by, note, created_at, published_at
             FROM filter_rule_versions
             WHERE version = ?1
             LIMIT 1",
        )
        .bind(target_version)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::NotFound(format!("过滤规则版本 v{target_version} 不存在")))?;

        let (max_ver,): (Option<i64>,) =
            sqlx::query_as("SELECT MAX(version) FROM filter_rule_versions")
                .fetch_one(&mut *tx)
                .await?;
        let next_version = max_ver.unwrap_or(0) + 1;

        sqlx::query(
            "UPDATE filter_rule_versions SET status = 'archived' WHERE status = 'published'",
        )
        .execute(&mut *tx)
        .await?;

        let note = format!("回滚自 v{target_version}");
        let id = sqlx::query(
            "INSERT INTO filter_rule_versions
             (version, status, filter_json, ad_phrases_json, ad_regexes_json, created_by, note, created_at, published_at)
             VALUES (?1, 'published', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(next_version)
        .bind(&target.filter_json)
        .bind(&target.ad_phrases_json)
        .bind(&target.ad_regexes_json)
        .bind(created_by)
        .bind(&note)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();

        tx.commit().await?;

        Ok(FilterRuleVersionRow {
            id,
            version: next_version,
            status: FilterVersionStatus::Published.as_str().to_string(),
            filter_json: target.filter_json,
            ad_phrases_json: target.ad_phrases_json,
            ad_regexes_json: target.ad_regexes_json,
            created_by: created_by.to_string(),
            note,
            created_at: now,
            published_at: Some(now),
        })
    }

    pub async fn discard_filter_draft(&self) -> Result<bool, StorageError> {
        let res = sqlx::query("DELETE FROM filter_rule_versions WHERE status = 'draft'")
            .execute(self.pool())
            .await?;
        Ok(res.rows_affected() > 0)
    }
}
