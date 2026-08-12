//! Bounded stack-safe decoding for recursively represented IR values.

use serde::de::DeserializeOwned;

use crate::Result;

const MAX_RECORD_BYTES: usize = 64 * 1024 * 1024;
const MAX_JSON_DEPTH: usize = 2048;

pub(super) fn from_slice<T: DeserializeOwned>(input: &[u8]) -> Result<T> {
    validate_shape(input)?;
    let mut deserializer = serde_json::Deserializer::from_slice(input);
    deserializer.disable_recursion_limit();
    let value = T::deserialize(serde_stacker::Deserializer::new(&mut deserializer))?;
    deserializer.end()?;
    Ok(value)
}

pub(super) fn from_str<T: DeserializeOwned>(input: &str) -> Result<T> {
    from_slice(input.as_bytes())
}

fn validate_shape(input: &[u8]) -> Result<()> {
    if input.len() > MAX_RECORD_BYTES {
        return Err(crate::Error::invalid(format!(
            "linked-IR JSON record is {} bytes; limit is {MAX_RECORD_BYTES}",
            input.len()
        )));
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in input {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_JSON_DEPTH {
                    return Err(crate::Error::invalid(format!(
                        "linked-IR JSON record nesting exceeds {MAX_JSON_DEPTH}"
                    )));
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deeply_nested_ir_record_uses_growing_stack() {
        let mut input = String::new();
        input.extend(std::iter::repeat_n('[', 256));
        input.push('0');
        input.extend(std::iter::repeat_n(']', 256));
        let value: serde_json::Value = from_str(&input).unwrap();
        assert!(value.is_array());
    }

    #[test]
    fn excessive_nesting_is_rejected_before_deserialization() {
        let input = "[".repeat(MAX_JSON_DEPTH + 1);
        let error = from_str::<serde_json::Value>(&input).unwrap_err();
        assert!(error.to_string().contains("nesting exceeds"));
    }
}
