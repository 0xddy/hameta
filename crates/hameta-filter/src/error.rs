use thiserror::Error;

#[derive(Debug, Error)]
pub enum FilterError {
    #[error("正则表达式编译失败 [{category}:{rule_id}] {pattern:?}: {message}")]
    InvalidRegex {
        category: String,
        rule_id: String,
        pattern: String,
        message: String,
    },

    #[error("旧版配置 JSON 解析失败: {0}")]
    InvalidLegacyJson(String),

    #[error("过滤规则配置无效: {0}")]
    InvalidRule(String),
}
