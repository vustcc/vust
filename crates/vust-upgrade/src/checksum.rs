//! SHA256 摘要与 checksum 文本解析。

use ring::digest;
use thiserror::Error;

/// checksum 解析或校验错误。
#[derive(Debug, Error)]
pub enum ChecksumError {
    #[error("sha256 must be a 64-character hex string")]
    InvalidSha256,
    #[error("checksum file does not contain a valid sha256 entry")]
    MissingEntry,
    #[error("sha256 mismatch")]
    Mismatch,
}

/// 计算 SHA256 hex 摘要。
pub fn compute_sha256_hex(bytes: &[u8]) -> String {
    hex::encode(digest::digest(&digest::SHA256, bytes).as_ref())
}

/// 规范化 SHA256 hex 字符串。
pub fn normalize_sha256(value: &str) -> Result<String, ChecksumError> {
    let normalized = value.trim().to_lowercase();
    if normalized.len() != 64 || !normalized.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(ChecksumError::InvalidSha256);
    }
    Ok(normalized)
}

/// 校验 bytes 的 SHA256 是否等于 expected。
pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<String, ChecksumError> {
    let expected = normalize_sha256(expected)?;
    let actual = compute_sha256_hex(bytes);
    if actual != expected {
        return Err(ChecksumError::Mismatch);
    }
    Ok(actual)
}

/// 解析 checksum 文本，支持单独摘要或 `<sha256> <file>` 格式。
pub fn parse_checksum_text(text: &str, artifact_name: &str) -> Result<String, ChecksumError> {
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let mut parts = line.split_whitespace();
        let Some(checksum) = parts.next() else {
            continue;
        };
        let Ok(checksum) = normalize_sha256(checksum) else {
            continue;
        };
        let matches_artifact = parts
            .next()
            .map(|name| name.trim_start_matches('*') == artifact_name)
            .unwrap_or(true);
        if matches_artifact {
            return Ok(checksum);
        }
    }
    Err(ChecksumError::MissingEntry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_single_checksum() {
        let checksum = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(parse_checksum_text(checksum, "a").unwrap(), checksum);
    }

    #[test]
    fn parse_selects_named_entry() {
        let expected = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let raw = format!(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb b\n{expected} *a\n"
        );
        assert_eq!(parse_checksum_text(&raw, "a").unwrap(), expected);
    }
}
