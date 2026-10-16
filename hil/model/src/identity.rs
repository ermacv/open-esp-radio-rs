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
        if text.len() != 64 {
            return Err(D::Error::custom("an identity is 64 hexadecimal digits"));
        }
        let mut bytes = [0; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[2 * index..2 * index + 2], 16)
                .map_err(|_| D::Error::custom("an identity is 64 hexadecimal digits"))?;
        }
        Ok(bytes)
    }
}
