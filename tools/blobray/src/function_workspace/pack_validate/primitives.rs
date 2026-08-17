//! Shared scalar validation rules for reviewed packs.

use super::super::validation::{ValidationError, ValidationResult};

pub(super) fn validate_one_line(value: &str, label: &str) -> ValidationResult<()> {
    if value.trim().is_empty() || value.contains(['\r', '\n']) {
        return Err(ValidationError::pack(
            "types",
            format!("{label} must be one non-empty line"),
        ));
    }
    Ok(())
}
pub(super) fn validate_optional_description(
    value: Option<&str>,
    label: &str,
) -> ValidationResult<()> {
    if let Some(value) = value {
        validate_one_line(value, &format!("{label} description"))?;
    }
    Ok(())
}
pub(super) fn validate_id(value: &str, context: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid {context} {value:?}"));
    }
    Ok(())
}
pub(super) fn validate_identifier(value: &str, context: &str) -> std::result::Result<(), String> {
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(format!("invalid {context} {value:?}"));
    }
    Ok(())
}
pub(super) fn validate_sha256(value: &str, context: &str) -> std::result::Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(format!("{context} has invalid lowercase SHA-256 {value:?}"));
    }
    Ok(())
}
