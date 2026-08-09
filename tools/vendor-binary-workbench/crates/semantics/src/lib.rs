//! Architecture-neutral semantic verification interfaces.
//!
//! The workbench facade dispatches these requests to an explicitly selected
//! platform harness. Implementations may bind a production driver, while the
//! interface itself contains no chip registry or driver dependency.

mod driver_plan;
mod effect_contract;
mod equivalence;

use std::path::Path;

pub use driver_plan::*;
pub use effect_contract::*;
pub use equivalence::*;
pub use open_radio_vendor_analysis_model::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Analysis(#[from] open_radio_vendor_analysis_model::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Format(#[from] std::fmt::Error),

    #[error(transparent)]
    Toml(#[from] toml_edit::de::Error),
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.to_owned())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct DriverAdapterRequest<'a> {
    pub id: &'a str,
    pub source: &'a str,
    pub vendor_symbol: &'a str,
    pub svd: &'a MmioMap,
    /// Caller-owned authoritative symbol inventory (for example a raw `.a`).
    pub vendor_inventory: Option<&'a Path>,
    /// Executable linked view used for concrete verification.
    pub vendor_artifact: &'a Path,
    pub vendor_companion: Option<&'a Path>,
    pub rust_artifact: &'a Path,
    pub rust_companion: Option<&'a Path>,
    pub rust_symbol: &'a str,
    pub policy: &'a EffectPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverAdapterVerification {
    pub matched: bool,
    pub canonical: String,
}

pub struct SemanticContractRequest<'a> {
    pub id: &'a str,
    pub source: &'a str,
    pub vendor_symbol: &'a str,
    pub svd: &'a MmioMap,
    pub vendor_artifact: &'a Path,
    pub vendor_companion: Option<&'a Path>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvidenceSource {
    pub name: &'static str,
    pub contents: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverAdapterEvidenceSources {
    pub adapter: &'static [EvidenceSource],
    pub reviewed_summary: EvidenceSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticContractEvidenceSources {
    pub common: &'static [EvidenceSource],
    pub contract: EvidenceSource,
}
