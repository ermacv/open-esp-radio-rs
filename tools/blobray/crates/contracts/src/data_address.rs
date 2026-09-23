//! Candidate associations between numeric addresses and captured data symbols.
//! The original address expression remains independent of these hypotheses.

use serde::{Deserialize, Serialize};

use crate::DataIdentity;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DataAddressBasis {
    AccessAddress,
    IndexedBase,
    ArgumentOffsetHint,
    /// A read location is known, but its access width is absent from this expression.
    ReadLocationHint,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DataAddressGap {
    NotAnalyzed,
    NonConcreteAddress,
    InvalidAccessWidth,
    AddressOverflow,
    NoContainingDefinition,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataAddressCandidate {
    pub identity: DataIdentity,
    pub member: Option<String>,
    pub symbol: String,
    pub symbol_address: u32,
    pub symbol_size: u32,
    pub exported: bool,
    pub offset: i64,
}

/// A single range candidate is not proof of object boundaries or link selection.
/// Ambiguity includes aliases, overlapping ranges and distinct artifact owners.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DataAddressResolution {
    Unknown {
        reason: DataAddressGap,
    },
    Candidate {
        basis: DataAddressBasis,
        address: u32,
        width: Option<u8>,
        candidate: Box<DataAddressCandidate>,
    },
    Ambiguous {
        basis: DataAddressBasis,
        address: u32,
        width: Option<u8>,
        candidates: Vec<DataAddressCandidate>,
    },
}

impl DataAddressResolution {
    pub fn from_candidates(
        address: u32,
        width: Option<u8>,
        mut candidates: Vec<DataAddressCandidate>,
    ) -> Self {
        candidates.sort();
        match candidates.len() {
            0 => Self::Unknown {
                reason: DataAddressGap::NoContainingDefinition,
            },
            1 => Self::Candidate {
                basis: DataAddressBasis::AccessAddress,
                address,
                width,
                candidate: Box::new(candidates.pop().expect("one candidate")),
            },
            _ => Self::Ambiguous {
                basis: DataAddressBasis::AccessAddress,
                address,
                width,
                candidates,
            },
        }
    }

    /// Address and width used for range matching, separate from the observed expression.
    pub fn access(&self) -> Option<(u32, Option<u8>)> {
        match self {
            Self::Unknown { .. } => None,
            Self::Candidate { address, width, .. } | Self::Ambiguous { address, width, .. } => {
                Some((*address, *width))
            }
        }
    }

    /// Check that serialized candidates describe the claimed range without
    /// promoting a range association to verified object ownership.
    pub fn validate(&self) -> Result<(), String> {
        let Some((address, width)) = self.access() else {
            return Ok(());
        };
        if matches!(self, Self::Ambiguous { candidates, .. } if candidates.len() < 2) {
            return Err("ambiguous data address requires at least two candidates".to_owned());
        }
        if width.is_some_and(|width| width == 0 || !width.is_multiple_of(8)) {
            return Err("invalid data-address access width".to_owned());
        }
        let access_end = address
            .checked_add(u32::from(width.map_or(1, |width| width / 8)))
            .ok_or("data-address access overflows")?;
        for candidate in self.candidates() {
            let end = candidate
                .symbol_address
                .checked_add(candidate.symbol_size)
                .ok_or("data-symbol range overflows")?;
            if candidate.symbol_size == 0 || address < candidate.symbol_address || access_end > end
            {
                return Err("data-address access lies outside its data-symbol range".to_owned());
            }
            if candidate.offset != i64::from(address - candidate.symbol_address) {
                return Err("data-address candidate offset does not match its range".to_owned());
            }
            match &candidate.identity {
                DataIdentity::Symbol {
                    artifact_sha256, ..
                } if artifact_sha256.len() != 64
                    || !artifact_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) =>
                {
                    return Err("invalid data-address artifact identity".to_owned());
                }
                DataIdentity::Synthetic { namespace, key }
                    if namespace.is_empty() || key.is_empty() =>
                {
                    return Err("empty synthetic data-address identity".to_owned());
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn candidates(&self) -> &[DataAddressCandidate] {
        match self {
            Self::Unknown { .. } => &[],
            Self::Candidate { candidate, .. } => std::slice::from_ref(candidate.as_ref()),
            Self::Ambiguous { candidates, .. } => candidates,
        }
    }

    pub fn with_basis(mut self, value: DataAddressBasis) -> Self {
        match &mut self {
            Self::Candidate { basis, .. } | Self::Ambiguous { basis, .. } => *basis = value,
            Self::Unknown { .. } => {}
        }
        self
    }
}
