//! A scenario instance's identity in evidence.

use core::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{InstanceName, Spec};

/// SHA-256 over the scenario ID, the instance name and the canonical JSON
/// of its parameters and criteria. The verdict code's identity is the
/// source closure the run's source snapshot already records; together they
/// pin what a recorded verdict was computed by.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Identity(#[serde(with = "hex")] pub [u8; 32]);

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

pub fn identity<S: Spec>(
    instance: InstanceName,
    params: &S::Params,
    criteria: &S::Criteria,
) -> Result<Identity, serde_json::Error> {
    let canonical = serde_json::to_vec(&serde_json::json!({
        "scenario": S::ID,
        "instance": instance,
        "params": params,
        "criteria": criteria,
    }))?;
    Ok(Identity(Sha256::digest(&canonical).into()))
}

mod hex {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&super::Identity(*bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        super::parse(&text)
            .ok_or_else(|| D::Error::custom("an identity is 64 lowercase hexadecimal digits"))
    }
}

/// The canonical form `Display` writes: exactly 64 lowercase hexadecimal
/// digits; anything else, a sign or a multi-byte character included, is
/// refused.
fn parse(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let value = |digit: u8| match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    };
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
        *byte = (value(pair[0])? << 4) | value(pair[1])?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_canonical_form_reads_back() {
        let identity = Identity([0xa5; 32]);
        let json = serde_json::to_string(&identity).unwrap();
        assert_eq!(serde_json::from_str::<Identity>(&json).unwrap(), identity);
        for text in [
            format!("\"€{}\"", "a".repeat(61)),
            format!("\"+f{}\"", "0".repeat(62)),
            format!("\"A{}\"", "0".repeat(63)),
            format!("\"{}\"", "0".repeat(63)),
        ] {
            assert!(serde_json::from_str::<Identity>(&text).is_err(), "{text}");
        }
    }
}
