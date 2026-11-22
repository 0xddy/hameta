use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("数据库操作失败: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("数据库迁移失败: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("JSON 序列化或解析失败: {0}")]
    Json(#[from] serde_json::Error),

    #[error("文件系统操作失败: {0}")]
    Io(#[from] std::io::Error),

    #[error("记录不存在: {0}")]
    NotFound(String),

    #[error("状态冲突: {0}")]
    Conflict(String),

    #[error("参数无效: {0}")]
    InvalidInput(String),
}
