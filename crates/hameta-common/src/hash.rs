use crate::error::CommonError;

/// Manticore Search 锁定存量表名。
pub const TABLE_TORRENTS: &str = "torrents";

/// 判断是否为 40 位十六进制 InfoHash。
pub fn is_valid_info_hash(info_hash: &str) -> bool {
    info_hash.len() == 40 && info_hash.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 去除首尾空白并校验规范化为小写 40 位十六进制 InfoHash。
pub fn normalize_info_hash(info_hash: &str) -> Result<String, CommonError> {
    let trimmed = info_hash.trim();
    if !is_valid_info_hash(trimmed) {
        return Err(CommonError::InvalidInfoHash(
            "info_hash 必须是 40 位十六进制字符串".to_string(),
        ));
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// 将 InfoHash 确定性映射为 Manticore 63-bit 正整数主键 `i64`。
///
/// 规则锁定自存量系统：取前 16 位 hex 字符（64 bits）解析为 `u64`，
/// 清除最高位（`& 0x7FFF_FFFF_FFFF_FFFF`）以保证落入非负 `i64` 空间。
pub fn hash_to_id(info_hash: &str) -> i64 {
    let Some(prefix) = info_hash.get(..16) else {
        return 0;
    };
    if !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return 0;
    }

    match u64::from_str_radix(prefix, 16) {
        Ok(value) => (value & 0x7FFF_FFFF_FFFF_FFFF) as i64,
        Err(_) => 0,
    }
}
