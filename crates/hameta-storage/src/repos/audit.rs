use crate::db::{Database, now_unix};
use crate::error::StorageError;
use hameta_common::AuditLogEntry;
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct AuditLogRow {
    pub id: i64,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail_json: String,
    pub client_ip: String,
    pub created_at: i64,
}

impl From<AuditLogRow> for AuditLogEntry {
    fn from(r: AuditLogRow) -> Self {
        Self {
            id: r.id,
            actor: r.actor,
            action: r.action,
            target: r.target,
            detail_json: r.detail_json,
            client_ip: r.client_ip,
            created_at: r.created_at,
        }
    }
}

impl Database {
    pub async fn record_audit(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        detail_json: &str,
        client_ip: &str,
    ) -> Result<i64, StorageError> {
        let now = now_unix();
        let id = sqlx::query(
            "INSERT INTO audit_logs (actor, action, target, detail_json, client_ip, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(detail_json)
        .bind(client_ip)
        .bind(now)
        .execute(self.pool())
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub async fn list_audits(&self, limit: i64) -> Result<Vec<AuditLogEntry>, StorageError> {
        let rows = sqlx::query_as::<_, AuditLogRow>(
            "SELECT id, actor, action, target, detail_json, client_ip, created_at
             FROM audit_logs
             ORDER BY id DESC
             LIMIT ?1",
        )
        .bind(limit.max(1))
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}
