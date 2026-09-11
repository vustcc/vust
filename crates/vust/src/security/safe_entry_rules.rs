//! 安全入口值规则：统一长度、字符集与保留前缀校验。

/// 安全入口最小长度。
pub const SAFE_ENTRY_MIN_LEN: usize = 8;
/// 安全入口最大长度。
pub const SAFE_ENTRY_MAX_LEN: usize = 32;
/// 安全入口保留路径前缀。
pub const RESERVED_SAFE_ENTRY_PREFIXES: &[&str] = &[
    "api", "assets", "images", "favicon", "static", "public", "health", "metrics", "ws", "wss",
    "robots",
];

/// 安全入口校验错误。
#[derive(Debug, thiserror::Error)]
pub enum SafeEntryValidationError {
    #[error("safe entry must be 8-32 ASCII letters or digits")]
    InvalidFormat,
    #[error("safe entry uses a reserved path prefix")]
    ReservedPrefix,
}

/// 校验安全入口值。
pub fn validate_safe_entry_value(value: &str) -> Result<(), SafeEntryValidationError> {
    if !(SAFE_ENTRY_MIN_LEN..=SAFE_ENTRY_MAX_LEN).contains(&value.len())
        || !value.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return Err(SafeEntryValidationError::InvalidFormat);
    }

    let lower = value.to_ascii_lowercase();
    if RESERVED_SAFE_ENTRY_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return Err(SafeEntryValidationError::ReservedPrefix);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{SafeEntryValidationError, validate_safe_entry_value};

    #[test]
    fn accepts_valid_safe_entry() {
        assert!(validate_safe_entry_value("Xm9Kp2Qs").is_ok());
    }

    #[test]
    fn rejects_invalid_format() {
        assert!(matches!(
            validate_safe_entry_value("abc1234"),
            Err(SafeEntryValidationError::InvalidFormat)
        ));
        assert!(matches!(
            validate_safe_entry_value("abc1234!"),
            Err(SafeEntryValidationError::InvalidFormat)
        ));
    }

    #[test]
    fn rejects_reserved_prefix_case_insensitively() {
        assert!(matches!(
            validate_safe_entry_value("api123456"),
            Err(SafeEntryValidationError::ReservedPrefix)
        ));
        assert!(matches!(
            validate_safe_entry_value("AssetsLogin"),
            Err(SafeEntryValidationError::ReservedPrefix)
        ));
        assert!(matches!(
            validate_safe_entry_value("robots88"),
            Err(SafeEntryValidationError::ReservedPrefix)
        ));
    }
}
