use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommonError {
    #[error("无效的 info_hash: {0}")]
    InvalidInfoHash(String),

    #[error("无效的密码格式: {0}")]
    InvalidPassword(String),

    #[error("只读 SQL 校验失败: {0}")]
    InvalidSql(String),

    #[error("无效的采集档位: {0}")]
    InvalidProfile(String),

    #[error("配置校验失败: {0}")]
    InvalidConfig(String),
}
