//! Architecture-neutral semantic verification interfaces.
//!
//! The workbench facade dispatches these requests to an explicitly selected
//! platform harness. Implementations may bind a production driver, while the
//! interface itself contains no chip registry or driver dependency.

mod driver_plan;
mod effect_contract;
mod equivalence;
mod qualification;

use std::path::Path;

pub use driver_plan::*;
pub use effect_contract::*;
pub use equivalence::*;
pub use open_radio_vendor_analysis_model::*;
pub use qualification::*;

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

/// Additional executable image supplied to a platform adapter without adding
/// its symbols to the suite's coverage inventory.
///
/// This is intended for authoritative replay link units which provide
/// fixtures or external-call implementations for one reviewed vendor root.
pub struct DriverAdapterArtifact<'a> {
    pub id: &'a str,
    pub artifact: &'a Path,
}

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
    pub auxiliary_artifacts: &'a [DriverAdapterArtifact<'a>],
    pub rust_artifact: &'a Path,
    pub rust_companion: Option<&'a Path>,
    pub rust_symbol: &'a str,
    pub policy: &'a EffectPolicy,
}

/// Strength of the claim established by a compiled driver adapter.
///
/// A successful projection or Rust-only conformance check is useful evidence,
/// but it must never be promoted to whole-function vendor equivalence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriverAdapterClaim {
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

impl DriverAdapterClaim {
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

/// Origin of the vendor-side behavior used by a driver adapter.
///
/// This is deliberately independent of a friendly claim label. Only concrete
/// replay may establish equivalence with a production implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VendorOracleKind {
    ConcreteReplay,
    CompleteLiftedTrace,
    StaticReviewedFacts,
    ManualAssumption,
}

impl VendorOracleKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ConcreteReplay => "concrete-replay",
            Self::CompleteLiftedTrace => "complete-lifted-trace",
            Self::StaticReviewedFacts => "static-reviewed-facts",
            Self::ManualAssumption => "manual-assumption",
        }
    }
}

/// Relationship between a compiled probe and the production Rust component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RustBindingKind {
    GeneratedTransaction,
    ExactProductionEntry,
    SharedProductionCore,
    VerificationProjection,
}

impl RustBindingKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::GeneratedTransaction => "generated-transaction",
            Self::ExactProductionEntry => "exact-production-entry",
            Self::SharedProductionCore => "shared-production-core",
            Self::VerificationProjection => "verification-projection",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriverAdapterDomain {
    WholeFunction,
    ReviewedDomain,
}

impl DriverAdapterDomain {
    pub const fn label(self) -> &'static str {
        match self {
            Self::WholeFunction => "whole-function",
            Self::ReviewedDomain => "reviewed-domain",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriverAdapterRelation {
    Exact,
    Refinement,
    Projection,
    Conformance,
}

impl DriverAdapterRelation {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Refinement => "refinement",
            Self::Projection => "projection",
            Self::Conformance => "conformance",
        }
    }
}

/// Facts from which the verifier computes the strongest admissible claim.
/// Adapter code cannot select a stronger claim directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct DriverAdapterTrust {
    pub vendor: VendorOracleKind,
    pub rust: RustBindingKind,
    pub domain: DriverAdapterDomain,
    pub relation: DriverAdapterRelation,
}

impl DriverAdapterTrust {
    pub const fn claim(self) -> DriverAdapterClaim {
        use DriverAdapterClaim as Claim;
        use DriverAdapterDomain as Domain;
        use DriverAdapterRelation as Relation;
        use RustBindingKind as Rust;
        use VendorOracleKind as Vendor;

        match (self.vendor, self.rust, self.domain, self.relation) {
            (
                Vendor::ConcreteReplay,
                Rust::GeneratedTransaction | Rust::ExactProductionEntry,
                Domain::WholeFunction,
                Relation::Exact,
            ) => Claim::WholeFunctionEquivalence,
            (
                Vendor::ConcreteReplay,
                Rust::GeneratedTransaction | Rust::ExactProductionEntry,
                Domain::ReviewedDomain,
                Relation::Exact,
            ) => Claim::ReviewedDomainEquivalence,
            (
                Vendor::ConcreteReplay,
                Rust::GeneratedTransaction
                | Rust::ExactProductionEntry
                | Rust::SharedProductionCore,
                _,
                Relation::Exact | Relation::Refinement,
            ) => Claim::ReviewedRefinement,
            (_, _, _, Relation::Conformance) | (Vendor::ManualAssumption, _, _, _) => {
                Claim::RustConformance
            }
            _ => Claim::ReviewedProjection,
        }
    }

    pub fn canonical(self) -> String {
        format!(
            "vendor-oracle {}\nrust-binding {}\ndomain {}\nrelation {}\nclaim {}\n",
            self.vendor.label(),
            self.rust.label(),
            self.domain.label(),
            self.relation.label(),
            self.claim().label(),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct DriverAdapterCase {
    pub name: String,
    pub matched: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverAdapterVerification {
    pub claim: DriverAdapterClaim,
    pub trust: DriverAdapterTrust,
    pub matched: bool,
    pub canonical: String,
    pub cases: Vec<DriverAdapterCase>,
}

impl DriverAdapterVerification {
    pub fn from_trust(trust: DriverAdapterTrust, matched: bool, mut canonical: String) -> Self {
        canonical.push_str(&trust.canonical());
        Self {
            claim: trust.claim(),
            trust,
            matched,
            canonical,
            cases: Vec::new(),
        }
    }

    pub fn with_cases(mut self, cases: Vec<DriverAdapterCase>) -> Self {
        self.cases = cases;
        self
    }
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
    pub trust: DriverAdapterTrust,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticContractEvidenceSources {
    pub common: &'static [EvidenceSource],
    pub contract: EvidenceSource,
}

#[cfg(test)]
mod trust_tests {
    use super::*;

    fn trust(
        vendor: VendorOracleKind,
        rust: RustBindingKind,
        domain: DriverAdapterDomain,
        relation: DriverAdapterRelation,
    ) -> DriverAdapterTrust {
        DriverAdapterTrust {
            vendor,
            rust,
            domain,
            relation,
        }
    }

    #[test]
    fn only_concrete_replay_of_an_exact_production_entry_can_claim_whole_function() {
        assert_eq!(
            trust(
                VendorOracleKind::ConcreteReplay,
                RustBindingKind::ExactProductionEntry,
                DriverAdapterDomain::WholeFunction,
                DriverAdapterRelation::Exact,
            )
            .claim(),
            DriverAdapterClaim::WholeFunctionEquivalence
        );
        assert_eq!(
            trust(
                VendorOracleKind::CompleteLiftedTrace,
                RustBindingKind::ExactProductionEntry,
                DriverAdapterDomain::WholeFunction,
                DriverAdapterRelation::Exact,
            )
            .claim(),
            DriverAdapterClaim::ReviewedProjection
        );
    }

    #[test]
    fn production_refinement_and_verification_projection_have_distinct_ceilings() {
        assert_eq!(
            trust(
                VendorOracleKind::ConcreteReplay,
                RustBindingKind::SharedProductionCore,
                DriverAdapterDomain::ReviewedDomain,
                DriverAdapterRelation::Refinement,
            )
            .claim(),
            DriverAdapterClaim::ReviewedRefinement
        );
        assert_eq!(
            trust(
                VendorOracleKind::ConcreteReplay,
                RustBindingKind::VerificationProjection,
                DriverAdapterDomain::ReviewedDomain,
                DriverAdapterRelation::Exact,
            )
            .claim(),
            DriverAdapterClaim::ReviewedProjection
        );
    }
}
