//! Strict JSON primitives shared by the linked-IR fact projections.

use serde_json::{Map, Value};

use crate::Result;

pub(super) fn object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| crate::Error::invalid(format!("{context} must be an object")))
}

pub(super) fn array<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a Vec<Value>> {
    object
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires array {key:?}")))
}

pub(super) fn string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::Error::invalid(format!("{context} requires non-empty string {key:?}"))
        })
}

pub(super) fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<String>> {
    object
        .get(key)
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "{context}.{key} must be a non-empty string or null"
                    ))
                })
        })
        .transpose()
}

pub(super) fn strings(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Vec<String>> {
    array(object, key, context)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                crate::Error::invalid(format!("{context}.{key}[{index}] must be a string"))
            })
        })
        .collect()
}

pub(super) fn boolean(object: &Map<String, Value>, key: &str, context: &str) -> Result<bool> {
    object
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires boolean {key:?}")))
}

pub(super) fn integer(object: &Map<String, Value>, key: &str, context: &str) -> Result<u64> {
    object.get(key).and_then(Value::as_u64).ok_or_else(|| {
        crate::Error::invalid(format!("{context} requires non-negative integer {key:?}"))
    })
}

pub(super) fn signed(object: &Map<String, Value>, key: &str, context: &str) -> Result<i64> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires integer {key:?}")))
}

pub(super) fn count(object: &Map<String, Value>, key: &str, context: &str) -> Result<usize> {
    integer(object, key, context)?
        .try_into()
        .map_err(|_| crate::Error::invalid(format!("invalid count {key:?} in {context}")))
}

pub(super) fn sha256<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a str> {
    let value = string(object, key, context)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(crate::Error::invalid(format!(
            "{context} has invalid lowercase SHA-256"
        )));
    }
    Ok(value)
}

pub(super) fn hex_u32(object: &Map<String, Value>, key: &str, context: &str) -> Result<u32> {
    match object.get(key) {
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|value| value.try_into().ok())
            .ok_or_else(|| crate::Error::invalid(format!("{context}.{key} must be a u32"))),
        Some(Value::String(value)) => value
            .strip_prefix("0x")
            .and_then(|value| u32::from_str_radix(value, 16).ok())
            .ok_or_else(|| {
                crate::Error::invalid(format!("{context}.{key} must be a hexadecimal u32 string"))
            }),
        _ => Err(crate::Error::invalid(format!(
            "{context}.{key} must be a u32"
        ))),
    }
}

pub(super) fn optional_hex_u32(
    object: &Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<u32>> {
    if object.get(key).is_some_and(Value::is_null) {
        return Ok(None);
    }
    hex_u32(object, key, context).map(Some)
}
