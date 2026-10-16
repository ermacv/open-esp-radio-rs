//! Stable scenario and instance names.

use core::fmt;

use serde::{Serialize, Serializer};

/// The stable identity of a scenario type: lowercase ASCII letters, digits
/// and single hyphens, starting with a letter, at most 64 bytes. A wrong
/// identifier fails to compile, because [`ScenarioId::new`] is evaluated in
/// the `const` that every [`Spec`](crate::Spec) declares.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScenarioId(&'static str);

impl ScenarioId {
    /// # Panics
    ///
    /// At compile time when `id` is not a valid name.
    pub const fn new(id: &'static str) -> Self {
        assert!(
            valid(id),
            "a scenario ID is a lowercase kebab-case name of at most 64 bytes"
        );
        Self(id)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ScenarioId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for ScenarioId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// The name of one parameterised instance of a scenario, with the grammar
/// of [`ScenarioId`]. The instance's full name is `scenario/instance`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InstanceName(&'static str);

impl InstanceName {
    /// # Panics
    ///
    /// At compile time when `name` is not a valid name.
    pub const fn new(name: &'static str) -> Self {
        assert!(
            valid(name),
            "an instance name is a lowercase kebab-case name of at most 64 bytes"
        );
        Self(name)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for InstanceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for InstanceName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// A name read at run time (a requirement, a command line) that is not a
/// valid scenario or instance name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidId(pub String);

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a lowercase kebab-case name of at most 64 bytes",
            self.0
        )
    }
}

impl std::error::Error for InvalidId {}

/// Checks a name read at run time (a requirement, a command line) with
/// the compile-time grammar.
pub fn validate_name(name: &str) -> Result<(), InvalidId> {
    if valid(name) {
        Ok(())
    } else {
        Err(InvalidId(name.to_owned()))
    }
}

const fn valid(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    if bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let allowed = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if !allowed || (byte == b'-' && bytes[index - 1] == b'-') {
            return false;
        }
        index += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_kebab_case_names() {
        assert_eq!(
            ScenarioId::new("lower-mac-tx-ownership").as_str(),
            "lower-mac-tx-ownership"
        );
        assert!(validate_name("p2-step-bounds").is_ok());
    }

    #[test]
    fn rejects_other_names() {
        for name in [
            "",
            "Upper",
            "1st",
            "trailing-",
            "double--hyphen",
            "under_score",
            &"a".repeat(65),
        ] {
            assert!(validate_name(name).is_err(), "{name:?}");
        }
    }
}
