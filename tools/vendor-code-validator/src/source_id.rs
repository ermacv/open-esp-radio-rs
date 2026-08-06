//! Stable generic IDs used to bind project selections to local input roles.

use crate::Result;

pub(crate) fn validate_source_id(value: &str) -> Result<&str> {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
        || !value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
    {
        return Err(format!("invalid source id {value:?}").into());
    }
    Ok(value)
}

pub(crate) fn is_source_id(value: &str) -> bool {
    validate_source_id(value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_ids_are_stable_option_components() {
        for valid in ["rom", "libpp", "wifi-archive", "source_2"] {
            assert_eq!(validate_source_id(valid).unwrap(), valid);
        }
        for invalid in ["", "2rom", "ROM", "rom.elf", "rom:path"] {
            assert!(validate_source_id(invalid).is_err());
        }
    }
}
