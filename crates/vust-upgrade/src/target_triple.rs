//! VUST 升级包平台标识。

use thiserror::Error;

/// targetTriple 解析错误。
#[derive(Debug, Error)]
pub enum TargetTripleError {
    #[error("targetTriple must use lowercase os-arch form")]
    Invalid,
}

/// 返回当前运行平台的 VUST targetTriple。
pub fn current_target_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        _ => "linux-x86_64",
    }
}

/// 规范化并校验 targetTriple。
pub fn normalize_target_triple(value: &str) -> Result<String, TargetTripleError> {
    let normalized = value.trim().to_lowercase();
    let valid_chars = normalized
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-');
    if !valid_chars || !matches!(normalized.as_str(), "linux-x86_64" | "linux-aarch64") {
        return Err(TargetTripleError::Invalid);
    }
    Ok(normalized)
}
