use crate::error::CommonError;
use rand::RngCore;
use sha2::{Digest, Sha256};

const SALT_BYTES: usize = 16;

/// 使用随机 16 字节盐 + SHA-256 生成 `salt$hex` 格式密码摘要。
pub fn hash_password(password: &str) -> Result<String, CommonError> {
    if password.is_empty() {
        return Err(CommonError::InvalidPassword("密码不能为空".to_string()));
    }

    let mut salt_bytes = [0_u8; SALT_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut salt_bytes);
    let salt = hex::encode(salt_bytes);
    let digest = compute_salted_sha256(&salt, password);
    Ok(format!("{salt}${}", hex::encode(digest)))
}

/// 验证明文密码与 `salt$hex` 摘要是否一致（常量时间比较）。
pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    if password.is_empty() {
        return false;
    }

    let Some((salt, expected_hex)) = stored_hash.split_once('$') else {
        return false;
    };
    if salt.len() != SALT_BYTES * 2 || !salt.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }

    let Ok(expected_bytes) = hex::decode(expected_hex) else {
        return false;
    };
    if expected_bytes.len() != 32 {
        return false;
    }

    let actual_bytes = compute_salted_sha256(salt, password);
    constant_time_eq(&actual_bytes, &expected_bytes)
}

/// 生成 32 字节（64 位十六进制）高熵随机令牌（用于 Session 与 IPC Token）。
pub fn generate_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// 计算任意令牌的 SHA-256 小写十六进制摘要。
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex::encode(digest)
}

fn compute_salted_sha256(salt: &str, password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b":");
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (&a, &b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}
