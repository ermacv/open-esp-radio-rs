use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPolicy {
    pub schema: u32,
    pub regions: Vec<RegionPolicy>,
    #[serde(default)]
    pub reserves: Vec<ReservePolicy>,
    #[serde(default)]
    pub rules: Vec<ConsumerRule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionPolicy {
    pub id: String,
    pub kind: RegionKind,
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum RegionKind {
    Sram,
    Psram,
    Flash,
    Rtc,
    Other,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservePolicy {
    pub id: String,
    pub region: String,
    pub start_symbol: String,
    pub end_symbol: String,
    pub reason: PlacementReason,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumerRule {
    /// `*` is the only special character. Matching is attempted against both
    /// the raw ELF symbol and its Rust-demangled spelling.
    pub symbol: String,
    pub owner: String,
    pub scope: ConsumerScope,
    pub reason: PlacementReason,
    pub placement: PlacementRequirement,
    pub region: String,
    #[serde(default)]
    pub optional: bool,
    pub count: Option<u64>,
    pub element_capacity: Option<u64>,
    pub optimization: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ConsumerScope {
    Driver,
    Adapter,
    Runtime,
    Hil,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PlacementReason {
    HardwareDma,
    InterruptCritical,
    LatencyCritical,
    Stack,
    CpuOnly,
    ColdPath,
    QualificationWindow,
    GeneratedTaskState,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PlacementRequirement {
    RequiredSram,
    PreferredSram,
    RequiredPsram,
    PreferredPsram,
    Neutral,
}

impl MemoryPolicy {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })?;
        let policy: Self = toml_edit::de::from_str(&source).map_err(|source| Error::Policy {
            path: path.to_owned(),
            source,
        })?;
        policy.validate()?;
        Ok(policy)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != 1 {
            return Err(Error::InvalidPolicy(format!(
                "unsupported memory policy schema {}; expected 1",
                self.schema
            )));
        }
        if self.regions.is_empty() {
            return Err(Error::InvalidPolicy(
                "policy defines no memory regions".into(),
            ));
        }
        for (index, region) in self.regions.iter().enumerate() {
            if region.id.is_empty() || region.start >= region.end {
                return Err(Error::InvalidPolicy(format!(
                    "invalid region at index {index}: {:?}",
                    region.id
                )));
            }
            if self.regions[..index].iter().any(|other| {
                other.id == region.id || (other.start < region.end && region.start < other.end)
            }) {
                return Err(Error::InvalidPolicy(format!(
                    "duplicate or overlapping region {:?}",
                    region.id
                )));
            }
        }
        for reserve in &self.reserves {
            self.require_region(&reserve.region)?;
        }
        for rule in &self.rules {
            self.require_region(&rule.region)?;
            if rule.symbol.is_empty() || rule.owner.is_empty() {
                return Err(Error::InvalidPolicy(
                    "consumer rules require non-empty symbol and owner".into(),
                ));
            }
        }
        Ok(())
    }

    fn require_region(&self, id: &str) -> Result<()> {
        if self.regions.iter().any(|region| region.id == id) {
            Ok(())
        } else {
            Err(Error::InvalidPolicy(format!(
                "unknown memory region {id:?}"
            )))
        }
    }
}

pub(crate) fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return value == pattern;
    }
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let Some(found) = value[offset..].find(part) else {
            return false;
        };
        if index == 0 && !pattern.starts_with('*') && found != 0 {
            return false;
        }
        offset += found + part.len();
    }
    pattern.ends_with('*') || parts.last().is_some_and(|part| value.ends_with(part))
}

#[cfg(test)]
mod tests {
    use super::wildcard_matches;

    #[test]
    fn wildcard_matching_is_anchored_without_edge_stars() {
        assert!(wildcard_matches("OPEN_*_BUFFER", "OPEN_RX_BUFFER"));
        assert!(!wildcard_matches("OPEN_*_BUFFER", "X_OPEN_RX_BUFFER"));
        assert!(wildcard_matches("*task4POOL", "mangled_task4POOL"));
        assert!(!wildcard_matches("*task4POOL", "mangled_task4POOL_suffix"));
    }
}
