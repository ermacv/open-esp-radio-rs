//! Validated names: scenario and instance identifiers, and the dotted
//! names of bounds, spans, counts, operations, chips and fixtures.

use core::fmt;
use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

/// A name's text: borrowed for compile-time names, owned for parsed ones.
type Text = Cow<'static, str>;

/// The grammars a name can follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Grammar {
    /// Lowercase ASCII letters, digits and single hyphens, starting with a
    /// letter and not ending with a hyphen, at most 64 bytes:
    /// `lower-mac-tx-ownership`.
    Kebab,
    /// Kebab segments joined by single dots, at most 96 bytes:
    /// `lower-mac.drain`, `boot-to-hello`.
    Dotted,
}

const fn kebab(bytes: &[u8], start: usize, end: usize) -> bool {
    if start >= end || !bytes[start].is_ascii_lowercase() || bytes[end - 1] == b'-' {
        return false;
    }
    let mut index = start;
    while index < end {
        let byte = bytes[index];
        let allowed = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if !allowed || (byte == b'-' && index > start && bytes[index - 1] == b'-') {
            return false;
        }
        index += 1;
    }
    true
}

impl Grammar {
    pub const fn accepts(self, name: &str) -> bool {
        let bytes = name.as_bytes();
        match self {
            Self::Kebab => bytes.len() <= 64 && kebab(bytes, 0, bytes.len()),
            Self::Dotted => {
                if bytes.len() > 96 {
                    return false;
                }
                let mut start = 0;
                let mut index = 0;
                while index <= bytes.len() {
                    if index == bytes.len() || bytes[index] == b'.' {
                        if !kebab(bytes, start, index) {
                            return false;
                        }
                        start = index + 1;
                    }
                    index += 1;
                }
                true
            }
        }
    }
}

/// A name that does not follow its grammar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidName {
    pub name: String,
    pub grammar: Grammar,
}

impl fmt::Display for InvalidName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a {:?} name", self.name, self.grammar)
    }
}

impl std::error::Error for InvalidName {}

macro_rules! names {
    ($($(#[$doc:meta])* $name:ident: $grammar:ident;)*) => {$(
        $(#[$doc])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Text);

        impl $name {
            /// A name known at compile time, or `None` when it breaks its
            /// grammar. [`name!`](crate::name) turns `None` into a
            /// compile error.
            pub const fn checked(name: &'static str) -> Option<Self> {
                if Grammar::$grammar.accepts(name) {
                    Some(Self(Text::Borrowed(name)))
                } else {
                    None
                }
            }

            /// A name read at run time: a requirement, a record, a command
            /// line.
            pub fn parse(name: &str) -> Result<Self, InvalidName> {
                if Grammar::$grammar.accepts(name) {
                    Ok(Self(Text::Owned(name.to_owned())))
                } else {
                    Err(InvalidName { name: name.to_owned(), grammar: Grammar::$grammar })
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Self::parse(&text).map_err(D::Error::custom)
            }
        }
    )*};
}

names! {
    /// The stable identity of a scenario type.
    ScenarioId: Kebab;
    /// One parameterised instance of a scenario.
    InstanceName: Kebab;
    /// A contract bound or backend parameter an image reports, such as
    /// `lower-mac.drain`.
    BoundName: Dotted;
    /// A span measured on the stand, such as `boot-to-hello`.
    SpanName: Dotted;
    /// A count a run records, such as `connect.attempts`.
    CountName: Dotted;
    /// An infrastructure operation, such as `link.sync`.
    Operation: Dotted;
    /// A chip of `platform/<chip>/chip.toml`, such as `esp32s31`.
    ChipId: Kebab;
    /// A fixture of the stand file, such as `hostapd`.
    FixtureName: Kebab;
    /// A product composition crate, such as `oer-esp32s31-ieee80211-system`.
    CompositionName: Kebab;
    /// A capability the HIL agent serves, by its `hil/protocol` module and
    /// name, such as `conformance.lower-mac`; the image keys derive from
    /// these through `oer-hil-image-keys`.
    CapabilityKey: Dotted;
}

/// One instance of one scenario: `scenario/instance`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ScenarioInstance {
    pub scenario: ScenarioId,
    pub instance: InstanceName,
}

impl fmt::Display for ScenarioInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.scenario, self.instance)
    }
}

/// A name checked at compile time: `name!(ScenarioId, "lower-mac-tx")`.
/// A name that breaks its grammar does not compile.
#[macro_export]
macro_rules! name {
    ($type:ident, $text:literal) => {
        const {
            $crate::$type::checked($text).expect(concat!(
                "`",
                $text,
                "` breaks the grammar of ",
                stringify!($type)
            ))
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_names_follow_their_grammar() {
        assert_eq!(
            crate::name!(ScenarioId, "lower-mac-tx-ownership").as_str(),
            "lower-mac-tx-ownership"
        );
        assert!(InstanceName::parse("p2-step-bounds").is_ok());
        for name in [
            "",
            "Upper",
            "1st",
            "trailing-",
            "double--hyphen",
            "under_score",
            "dot.ted",
            &"a".repeat(65),
        ] {
            assert!(InstanceName::parse(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn dotted_names_join_kebab_segments() {
        assert!(BoundName::parse("lower-mac.drain").is_ok());
        assert!(SpanName::parse("boot-to-hello").is_ok());
        for name in ["", ".a", "a.", "a..b", "a.B", "a.-b"] {
            assert!(BoundName::parse(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_parsed_name_reads_back_and_a_wrong_one_is_refused() {
        let name: CountName = serde_json::from_str("\"connect.attempts\"").unwrap();
        assert_eq!(name, crate::name!(CountName, "connect.attempts"));
        assert!(serde_json::from_str::<CountName>("\"Connect\"").is_err());
    }
}
