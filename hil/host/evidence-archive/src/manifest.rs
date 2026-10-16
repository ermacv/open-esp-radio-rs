//! A version's manifest and the version's name.
//!
//! The manifest's bytes are its compact JSON, so the [`Pin`](crate::Pin),
//! which repeats its fields, recomputes them and their digest without the
//! archive. The version is named by that digest (rule 3): two versions with
//! the same name hold the same runs and the same payload bytes.

use std::fmt;

use oer_hil_model::{RunRef, Sha256};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use crate::{Result, refused};

/// The manifest format.
pub const SCHEMA: u32 = 1;

/// The file name of a version's manifest.
pub const MANIFEST: &str = "manifest.json";

/// The file name of a version's payload.
pub const ARCHIVE: &str = "evidence.tar.gz";

/// The payload, `evidence.tar.gz`: its SHA-256 and size.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub sha256: Sha256,
    pub bytes: u64,
}

/// What one version holds: its runs, each by the digest of its seal, sorted
/// by run ID, and the payload that carries them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub archive: Payload,
    pub runs: Vec<RunRef>,
}

impl Manifest {
    /// The manifest of `runs` in `archive`; refuses no run or a run twice.
    pub fn new(archive: Payload, mut runs: Vec<RunRef>) -> Result<Self> {
        runs.sort();
        let manifest = Self {
            schema: SCHEMA,
            archive,
            runs,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Its schema, at least one run, and each run once in run ID order.
    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            return refused(format!(
                "evidence archive manifest schema {} is not {SCHEMA}",
                self.schema
            ));
        }
        if self.runs.is_empty() {
            return refused("an evidence archive version holds at least one run");
        }
        if let Some(pair) = self.runs.windows(2).find(|pair| pair[0].run >= pair[1].run) {
            return refused(format!(
                "evidence archive runs are not in run ID order or name `{}` twice",
                pair[1].run
            ));
        }
        Ok(())
    }

    /// The manifest's bytes: its compact JSON.
    pub fn bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a manifest serializes")
    }

    pub fn digest(&self) -> Sha256 {
        Sha256::of(&self.bytes())
    }

    pub fn version(&self) -> Version {
        Version(self.digest())
    }

    /// The manifest whose bytes are `bytes`; refuses anything but the
    /// canonical bytes of a valid manifest.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| crate::Refused(format!("invalid evidence manifest: {error}")))?;
        manifest.validate()?;
        if manifest.bytes() != bytes {
            return refused("the evidence manifest is not in its canonical form");
        }
        Ok(manifest)
    }

    /// The reference of `run` this version holds.
    pub fn find(&self, run: &oer_hil_model::RunId) -> Option<&RunRef> {
        self.runs
            .binary_search_by(|held| held.run.cmp(run))
            .ok()
            .map(|index| &self.runs[index])
    }
}

/// A version's name, `evidence-<manifest SHA-256>`: the release tag.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Version(pub Sha256);

impl Version {
    const PREFIX: &str = "evidence-";

    pub fn parse(text: &str) -> Result<Self> {
        text.strip_prefix(Self::PREFIX)
            .and_then(|digest| Sha256::parse(digest).ok())
            .map(Self)
            .ok_or_else(|| {
                crate::Refused(format!(
                    "evidence archive version `{text}` is not `evidence-<SHA-256>`"
                ))
                .into()
            })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.0)
    }
}

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(D::Error::custom)
    }
}
