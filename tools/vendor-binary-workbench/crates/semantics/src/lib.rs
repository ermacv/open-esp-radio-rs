//! Architecture-neutral effect-comparison vocabulary.
//!
//! The generic Workbench owns execution and verdict calculation. This crate
//! contains no chip registry, production driver dependency, or target-owned
//! comparison callback.

mod effect_contract;
mod equivalence;

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

/// Strength of a reviewed verification claim.
///
/// A successful projection or Rust-only conformance check is useful evidence,
/// but it must never be promoted to whole-function vendor equivalence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationClaim {
    WholeFunctionEquivalence,
    /// Every case in an explicit finite input domain matches, while behavior
    /// outside that reviewed precondition remains outside the claim.
    ReviewedDomainEquivalence,
    /// The same reviewed behavior is preserved through an explicit, bounded
    /// production refinement (for example a blocking wait replaced by an
    /// asynchronous scheduling edge).
    ReviewedRefinement,
    ReviewedProjection,
    RustConformance,
}

impl VerificationClaim {
    pub fn label(self) -> &'static str {
        match self {
            Self::WholeFunctionEquivalence => "whole-function-equivalence",
            Self::ReviewedDomainEquivalence => "reviewed-domain-equivalence",
            Self::ReviewedRefinement => "reviewed-refinement",
            Self::ReviewedProjection => "reviewed-projection",
            Self::RustConformance => "rust-conformance",
        }
    }
}

/// Relationship between a compiled probe and the production Rust component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RustBindingKind {
    /// Generated research/static-evidence entry. This is never production
    /// driver code and cannot by itself establish a production binding.
    GeneratedReference,
    /// A reviewed, finite ABI/layout adapter around a production component.
    /// It may support only a bounded-domain claim and becomes production
    /// evidence only when concrete execution reaches that compiled component.
    ReviewedAbiProjection,
    ExactProductionEntry,
    SharedProductionCore,
    VerificationProjection,
}

impl RustBindingKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::GeneratedReference => "generated-reference",
            Self::ReviewedAbiProjection => "reviewed-abi-projection",
            Self::ExactProductionEntry => "exact-production-entry",
            Self::SharedProductionCore => "shared-production-core",
            Self::VerificationProjection => "verification-projection",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvidenceSource {
    pub name: &'static str,
    pub contents: &'static str,
}
