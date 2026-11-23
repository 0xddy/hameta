use crate::error::CommonError;

pub const DEFAULT_SQL_LIMIT: usize = 200;
pub const MAX_SQL_LIMIT: usize = 1000;

const ALLOWED_LEADING_VERBS: &[&str] = &["SELECT", "SHOW", "DESCRIBE", "DESC", "EXPLAIN"];
const FORBIDDEN_KEYWORDS: &[&str] = &[
    "INSERT", "REPLACE", "UPDATE", "DELETE", "DROP", "ALTER", "TRUNCATE", "CREATE", "ATTACH",
    "FLUSH", "OPTIMIZE", "FREEZE", "UNFREEZE", "KILL", "CALL",
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Word(String),
    StringOrIdent,
    Comma,
    OpenParen,
    CloseParen,
    Semicolon,
    Other,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    start: usize,
    paren_depth: usize,
}

/// 校验并规范化单条 Manticore SQL 控制台语句（放行 SELECT / SHOW / DESCRIBE / ALTER / INSERT / UPDATE / DELETE 等）：
/// 1. 禁止空语句与分号分隔的多语句（单次执行单条 SQL，允许末尾带一个 `;`）。
/// 2. 校验引号与 `/* ... */` 注释闭合。
/// 3. 仅对 `SELECT` 语句检查 `LIMIT <= {max_limit}`，未显式指定顶层 `LIMIT` 时自动附加 `LIMIT {default_limit}`，防止浏览器渲染爆内存；
///    其他语句（如 `ALTER TABLE`、`UPDATE`、`DELETE`、`SHOW`、`DESCRIBE`）原样放行。
pub fn prepare_console_sql(
    raw: &str,
    default_limit: usize,
    max_limit: usize,
) -> Result<String, CommonError> {
    if default_limit == 0 || default_limit > max_limit {
        return Err(CommonError::InvalidSql(
            "SQL 默认 LIMIT 参数配置无效".to_string(),
        ));
    }

    let (tokens, ends_with_line_comment) = tokenize_sql(raw)?;
    if tokens.is_empty() {
        return Err(CommonError::InvalidSql("SQL 语句不能为空".to_string()));
    }

    let mut trimmed_end = raw.len();
    let active_tokens: &[Token] = if let Some(last) = tokens.last()
        && last.kind == TokenKind::Semicolon
    {
        trimmed_end = last.start;
        &tokens[..tokens.len() - 1]
    } else {
        &tokens
    };

    if active_tokens.is_empty() {
        return Err(CommonError::InvalidSql("SQL 语句不能为空".to_string()));
    }

    if active_tokens
        .iter()
        .any(|token| token.kind == TokenKind::Semicolon)
    {
        return Err(CommonError::InvalidSql(
            "SQL 控制台每次仅允许执行单条语句，请勿使用分号拼接多条 SQL".to_string(),
        ));
    }

    let leading_verb = match &active_tokens[0].kind {
        TokenKind::Word(word) => word.as_str(),
        _ => "",
    };

    let cleaned = raw[..trimmed_end].trim();
    if leading_verb == "SELECT" {
        let has_outer_limit = validate_limits(active_tokens, max_limit)?;
        if !has_outer_limit {
            let separator = if ends_with_line_comment && trimmed_end == raw.len() {
                "\n"
            } else {
                " "
            };
            return Ok(format!("{cleaned}{separator}LIMIT {default_limit}"));
        }
    }

    Ok(cleaned.to_string())
}

/// 校验并规范化只读 SQL 语句：
/// 1. 仅放行单条 `SELECT` / `SHOW` / `DESCRIBE` / `DESC` / `EXPLAIN`。
/// 2. 禁止分号分隔的多语句与写操作/DDL 关键字（字符串字面量与注释之外）。
/// 3. 对未显式指定顶层 `LIMIT` 的 `SELECT` 自动附加 `LIMIT {default_limit}`，并禁止超过 `{max_limit}`。
pub fn validate_readonly_sql(
    raw: &str,
    default_limit: usize,
    max_limit: usize,
) -> Result<String, CommonError> {
    if default_limit == 0 || default_limit > max_limit {
        return Err(CommonError::InvalidSql(
            "SQL 默认 LIMIT 参数配置无效".to_string(),
        ));
    }

    let (tokens, ends_with_line_comment) = tokenize_sql(raw)?;
    if tokens.is_empty() {
        return Err(CommonError::InvalidSql("SQL 语句不能为空".to_string()));
    }

    let mut trimmed_end = raw.len();
    let active_tokens: &[Token] = if let Some(last) = tokens.last()
        && last.kind == TokenKind::Semicolon
    {
        trimmed_end = last.start;
        &tokens[..tokens.len() - 1]
    } else {
        &tokens
    };

    if active_tokens.is_empty() {
        return Err(CommonError::InvalidSql("SQL 语句不能为空".to_string()));
    }

    if active_tokens
        .iter()
        .any(|token| token.kind == TokenKind::Semicolon)
    {
        return Err(CommonError::InvalidSql(
            "只读 SQL 控制台禁止一次执行多条语句".to_string(),
        ));
    }

    let leading_verb = match &active_tokens[0].kind {
        TokenKind::Word(word) => word.as_str(),
        _ => {
            return Err(CommonError::InvalidSql(
                "仅允许以 SELECT / SHOW / DESCRIBE / DESC / EXPLAIN 开头的只读查询".to_string(),
            ));
        }
    };

    if !ALLOWED_LEADING_VERBS.contains(&leading_verb) {
        return Err(CommonError::InvalidSql(format!(
            "不允许执行写操作或非只读语句: {leading_verb}"
        )));
    }

    for token in active_tokens {
        if let TokenKind::Word(word) = &token.kind
            && FORBIDDEN_KEYWORDS.contains(&word.as_str())
        {
            return Err(CommonError::InvalidSql(format!(
                "只读 SQL 控制台禁止包含关键字: {word}"
            )));
        }
    }

    let has_outer_limit = validate_limits(active_tokens, max_limit)?;

    let cleaned = raw[..trimmed_end].trim();
    if leading_verb == "SELECT" && !has_outer_limit {
        let separator = if ends_with_line_comment && trimmed_end == raw.len() {
            "\n"
        } else {
            " "
        };
        Ok(format!("{cleaned}{separator}LIMIT {default_limit}"))
    } else {
        Ok(cleaned.to_string())
    }
}

fn validate_limits(tokens: &[Token], max_limit: usize) -> Result<bool, CommonError> {
    let mut has_outer_limit = false;
    let mut i = 0;
    while i < tokens.len() {
        if let TokenKind::Word(word) = &tokens[i].kind
            && word == "LIMIT"
        {
            if tokens[i].paren_depth == 0 {
                has_outer_limit = true;
            }
            let first = tokens
                .get(i + 1)
                .and_then(|t| match &t.kind {
                    TokenKind::Word(w) => w.parse::<usize>().ok(),
                    _ => None,
                })
                .ok_or_else(|| CommonError::InvalidSql("LIMIT 后必须跟正整数数量".to_string()))?;

            let effective_count = if tokens
                .get(i + 2)
                .is_some_and(|t| t.kind == TokenKind::Comma)
            {
                tokens
                    .get(i + 3)
                    .and_then(|t| match &t.kind {
                        TokenKind::Word(w) => w.parse::<usize>().ok(),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        CommonError::InvalidSql("LIMIT offset, count 语法无效".to_string())
                    })?
            } else {
                first
            };

            if effective_count == 0 || effective_count > max_limit {
                return Err(CommonError::InvalidSql(format!(
                    "LIMIT 必须在 1..={max_limit} 范围内，当前为 {effective_count}"
                )));
            }
        }
        i += 1;
    }
    Ok(has_outer_limit)
}

fn tokenize_sql(input: &str) -> Result<(Vec<Token>, bool), CommonError> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0;
    let mut paren_depth = 0_usize;
    let mut ends_with_line_comment = false;

    while i < len {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            if b == b'\n' {
                ends_with_line_comment = false;
            }
            i += 1;
            continue;
        }

        // `-- ...` 行注释
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            ends_with_line_comment = i == len;
            continue;
        }

        // `# ...` MySQL 风格行注释
        if b == b'#' {
            i += 1;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            ends_with_line_comment = i == len;
            continue;
        }

        // `/* ... */` 块注释
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            let mut closed = false;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Err(CommonError::InvalidSql("存在未闭合的 /* 注释".to_string()));
            }
            ends_with_line_comment = false;
            continue;
        }

        // 字符串与反引号标识符
        if b == b'\'' || b == b'"' || b == b'`' {
            let quote = b;
            let start = i;
            i += 1;
            let mut closed = false;
            while i < len {
                let curr = bytes[i];
                if curr == b'\\' && quote != b'`' {
                    i = (i + 2).min(len);
                    continue;
                }
                if curr == quote {
                    if bytes.get(i + 1) == Some(&quote) {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                return Err(CommonError::InvalidSql(
                    "存在未闭合的字符串或标识符引号".to_string(),
                ));
            }
            ends_with_line_comment = false;
            tokens.push(Token {
                kind: TokenKind::StringOrIdent,
                start,
                paren_depth,
            });
            continue;
        }

        // 单词 / 数字标识符
        if b.is_ascii_alphanumeric() || b == b'_' {
            let start = i;
            i += 1;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = input[start..i].to_ascii_uppercase();
            ends_with_line_comment = false;
            tokens.push(Token {
                kind: TokenKind::Word(word),
                start,
                paren_depth,
            });
            continue;
        }

        ends_with_line_comment = false;
        let ch = input[i..].chars().next().unwrap_or(' ');
        let char_len = ch.len_utf8();
        let kind = match ch {
            ';' => TokenKind::Semicolon,
            ',' => TokenKind::Comma,
            '(' => {
                let kind = TokenKind::OpenParen;
                tokens.push(Token {
                    kind,
                    start: i,
                    paren_depth,
                });
                paren_depth += 1;
                i += char_len;
                continue;
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                TokenKind::CloseParen
            }
            _ => TokenKind::Other,
        };
        tokens.push(Token {
            kind,
            start: i,
            paren_depth,
        });
        i += char_len;
    }

    Ok((tokens, ends_with_line_comment))
}
