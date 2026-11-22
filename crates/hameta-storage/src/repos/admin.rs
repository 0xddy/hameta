use crate::db::{Database, now_unix};
use crate::error::StorageError;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct AdminUserRow {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_login_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct AdminSessionRow {
    pub token_hash: String,
    pub user_id: i64,
    pub client_ip: String,
    pub user_agent: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub last_seen_at: i64,
}

impl Database {
    pub async fn get_primary_admin(&self) -> Result<Option<AdminUserRow>, StorageError> {
        let row = sqlx::query_as::<_, AdminUserRow>(
            "SELECT id, username, password_hash, created_at, updated_at, last_login_at
             FROM admin_users ORDER BY id ASC LIMIT 1",
        )
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn find_admin_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AdminUserRow>, StorageError> {
        let row = sqlx::query_as::<_, AdminUserRow>(
            "SELECT id, username, password_hash, created_at, updated_at, last_login_at
             FROM admin_users WHERE username = ?1 LIMIT 1",
        )
        .bind(username)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn find_admin_by_id(
        &self,
        user_id: i64,
    ) -> Result<Option<AdminUserRow>, StorageError> {
        let row = sqlx::query_as::<_, AdminUserRow>(
            "SELECT id, username, password_hash, created_at, updated_at, last_login_at
             FROM admin_users WHERE id = ?1 LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn update_admin_password(
        &self,
        user_id: i64,
        password_hash: &str,
    ) -> Result<(), StorageError> {
        let now = now_unix();
        let res =
            sqlx::query("UPDATE admin_users SET password_hash = ?1, updated_at = ?2 WHERE id = ?3")
                .bind(password_hash)
                .bind(now)
                .bind(user_id)
                .execute(self.pool())
                .await?;
        if res.rows_affected() == 0 {
            return Err(StorageError::NotFound(format!(
                "管理员 ID={user_id} 不存在"
            )));
        }
        Ok(())
    }

    pub async fn create_admin(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<AdminUserRow, StorageError> {
        let username = username.trim();
        if username.is_empty() {
            return Err(StorageError::InvalidInput(
                "管理员用户名不能为空".to_string(),
            ));
        }

        let now = now_unix();
        let id = sqlx::query(
            "INSERT INTO admin_users (username, password_hash, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(username)
        .bind(password_hash)
        .bind(now)
        .bind(now)
        .execute(self.pool())
        .await?
        .last_insert_rowid();

        Ok(AdminUserRow {
            id,
            username: username.to_string(),
            password_hash: password_hash.to_string(),
            created_at: now,
            updated_at: now,
            last_login_at: None,
        })
    }

    /// 在单事务内重置（或创建）主管理员账号与密码、清空全部现存登录会话并记录审计日志。
    pub async fn reset_admin_credentials(
        &self,
        username: &str,
        password_hash: &str,
        actor: &str,
    ) -> Result<AdminUserRow, StorageError> {
        let username = username.trim();
        if username.is_empty() {
            return Err(StorageError::InvalidInput(
                "管理员用户名不能为空".to_string(),
            ));
        }

        let now = now_unix();
        let mut tx = self.pool().begin().await?;

        let existing = sqlx::query_as::<_, AdminUserRow>(
            "SELECT id, username, password_hash, created_at, updated_at, last_login_at
             FROM admin_users ORDER BY id ASC LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;

        let updated = match existing {
            Some(user) => {
                sqlx::query(
                    "UPDATE admin_users
                     SET username = ?1, password_hash = ?2, updated_at = ?3
                     WHERE id = ?4",
                )
                .bind(username)
                .bind(password_hash)
                .bind(now)
                .bind(user.id)
                .execute(&mut *tx)
                .await?;

                AdminUserRow {
                    id: user.id,
                    username: username.to_string(),
                    password_hash: password_hash.to_string(),
                    created_at: user.created_at,
                    updated_at: now,
                    last_login_at: user.last_login_at,
                }
            }
            None => {
                let id = sqlx::query(
                    "INSERT INTO admin_users (username, password_hash, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .bind(username)
                .bind(password_hash)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await?
                .last_insert_rowid();

                AdminUserRow {
                    id,
                    username: username.to_string(),
                    password_hash: password_hash.to_string(),
                    created_at: now,
                    updated_at: now,
                    last_login_at: None,
                }
            }
        };

        sqlx::query("DELETE FROM admin_sessions")
            .execute(&mut *tx)
            .await?;

        let detail = serde_json::json!({ "username": username }).to_string();
        sqlx::query(
            "INSERT INTO audit_logs (actor, action, target, detail_json, client_ip, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(actor)
        .bind("admin.reset")
        .bind(username)
        .bind(detail)
        .bind("")
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(updated)
    }

    pub async fn update_last_login(&self, user_id: i64) -> Result<(), StorageError> {
        let now = now_unix();
        sqlx::query("UPDATE admin_users SET last_login_at = ?1 WHERE id = ?2")
            .bind(now)
            .bind(user_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn create_session(
        &self,
        token_hash: &str,
        user_id: i64,
        client_ip: &str,
        user_agent: &str,
        ttl_secs: i64,
    ) -> Result<AdminSessionRow, StorageError> {
        let now = now_unix();
        let expires_at = now + ttl_secs.max(60);
        sqlx::query(
            "INSERT INTO admin_sessions (token_hash, user_id, client_ip, user_agent, created_at, expires_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(token_hash)
        .bind(user_id)
        .bind(client_ip)
        .bind(user_agent)
        .bind(now)
        .bind(expires_at)
        .bind(now)
        .execute(self.pool())
        .await?;

        Ok(AdminSessionRow {
            token_hash: token_hash.to_string(),
            user_id,
            client_ip: client_ip.to_string(),
            user_agent: user_agent.to_string(),
            created_at: now,
            expires_at,
            last_seen_at: now,
        })
    }

    pub async fn find_valid_session(
        &self,
        token_hash: &str,
    ) -> Result<Option<AdminSessionRow>, StorageError> {
        let now = now_unix();
        let row = sqlx::query_as::<_, AdminSessionRow>(
            "SELECT token_hash, user_id, client_ip, user_agent, created_at, expires_at, last_seen_at
             FROM admin_sessions
             WHERE token_hash = ?1 AND expires_at > ?2
             LIMIT 1",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(self.pool())
        .await?;
        Ok(row)
    }

    pub async fn delete_session(&self, token_hash: &str) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM admin_sessions WHERE token_hash = ?1")
            .bind(token_hash)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn touch_session(&self, token_hash: &str) -> Result<(), StorageError> {
        let now = now_unix();
        sqlx::query("UPDATE admin_sessions SET last_seen_at = ?1 WHERE token_hash = ?2")
            .bind(now)
            .bind(token_hash)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn delete_all_sessions(&self) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM admin_sessions")
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn cleanup_expired_sessions(&self) -> Result<u64, StorageError> {
        let now = now_unix();
        let res = sqlx::query("DELETE FROM admin_sessions WHERE expires_at <= ?1")
            .bind(now)
            .execute(self.pool())
            .await?;
        Ok(res.rows_affected())
    }

    pub async fn count_active_sessions(&self) -> Result<i64, StorageError> {
        let now = now_unix();
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM admin_sessions WHERE expires_at > ?1")
                .bind(now)
                .fetch_one(self.pool())
                .await?;
        Ok(count)
    }
}
