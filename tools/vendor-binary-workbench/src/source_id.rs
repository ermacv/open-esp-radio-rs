//! Stable generic IDs used to bind project selections to local input roles.

use std::{fmt, str::FromStr};

use crate::Result;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct SourceId(String);

impl SourceId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SourceId {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        if !is_source_id(value) {
            return Err(format!(
                "invalid source id {value:?}; expected a lowercase identifier"
            ));
        }
        Ok(Self(value.to_owned()))
    }
}

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
            assert_eq!(valid.parse::<SourceId>().unwrap().as_str(), valid);
        }
        for invalid in ["", "2rom", "ROM", "rom.elf", "rom:path"] {
            assert!(validate_source_id(invalid).is_err());
            assert!(invalid.parse::<SourceId>().is_err());
        }
    }
}
