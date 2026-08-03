//! 全仓共享的小工具函数（变更-23 C-3 收敛）。

use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// 当前 Unix 毫秒时间戳（对应 TS 的 `Date.now()`）。
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

/// 当前 Unix 秒时间戳。
pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

/// 字节序列的 SHA-256 十六进制摘要（小写）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
