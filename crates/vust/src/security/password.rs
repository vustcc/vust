//! 密码规则：按配置执行空值与复杂度校验。

/// 密码校验错误。
#[derive(Debug, thiserror::Error)]
pub enum PasswordValidationError {
    #[error("password must not be empty")]
    Empty,
    #[error("password length must be at least {min}")]
    TooShort { min: usize, actual: usize },
    #[error("password length must be between {min} and {max}")]
    InvalidLength {
        min: usize,
        max: usize,
        actual: usize,
    },
    #[error("password must contain at least {required} character classes")]
    InsufficientCharTypes { required: usize, actual: usize },
}

/// 校验密码复杂度。
pub fn validate_password(
    password: &str,
    enforce_complexity: bool,
) -> Result<(), PasswordValidationError> {
    if password.is_empty() {
        return Err(PasswordValidationError::Empty);
    }
    let len = password.len();
    if !enforce_complexity {
        if len < 5 {
            return Err(PasswordValidationError::TooShort {
                min: 5,
                actual: len,
            });
        }
        return Ok(());
    }

    if !(8..=30).contains(&len) {
        return Err(PasswordValidationError::InvalidLength {
            min: 8,
            max: 30,
            actual: len,
        });
    }

    let has_letter = password.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = password.chars().any(|c| c.is_ascii_digit());
    let has_special = password.chars().any(|c| !c.is_ascii_alphanumeric());
    let actual = [has_letter, has_digit, has_special]
        .into_iter()
        .filter(|value| *value)
        .count();
    if actual < 2 {
        return Err(PasswordValidationError::InsufficientCharTypes {
            required: 2,
            actual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PasswordValidationError, validate_password};

    #[test]
    fn rejects_empty_password() {
        assert!(matches!(
            validate_password("", false),
            Err(PasswordValidationError::Empty)
        ));
    }

    #[test]
    fn requires_minimum_length_without_complexity() {
        assert!(matches!(
            validate_password("1234", false),
            Err(PasswordValidationError::TooShort { min: 5, actual: 4 })
        ));
        assert!(validate_password("12345", false).is_ok());
    }

    #[test]
    fn enforces_length_when_complexity_enabled() {
        assert!(matches!(
            validate_password("a1!4567", true),
            Err(PasswordValidationError::InvalidLength {
                min: 8,
                max: 30,
                actual: 7
            })
        ));
        assert!(matches!(
            validate_password("a1!4567890123456789012345678901", true),
            Err(PasswordValidationError::InvalidLength {
                min: 8,
                max: 30,
                actual: 31
            })
        ));
    }

    #[test]
    fn enforces_character_types_when_complexity_enabled() {
        assert!(matches!(
            validate_password("abcdefgh", true),
            Err(PasswordValidationError::InsufficientCharTypes {
                required: 2,
                actual: 1
            })
        ));
        assert!(validate_password("abcdef12", true).is_ok());
        assert!(validate_password("abcdef!!", true).is_ok());
    }
}
