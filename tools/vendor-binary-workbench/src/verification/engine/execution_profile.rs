//! Claim-aware execution-profile evaluation.
//!
//! Profile mechanics live here so the source inventory engine does not have
//! to reinterpret the difference between whole-function and finite-domain
//! evidence at every call site.

use std::path::Path;

use crate::*;

use super::{FunctionVerificationStatus, VerifySource};

pub(super) struct Evaluation {
    pub(super) comparison: ExecutionComparisonReport,
    pub(super) accepted_match: bool,
    pub(super) reviewed_domain: bool,
    pub(super) matched_status: FunctionVerificationStatus,
}

pub(super) fn evaluate(
    svd: &MmioMap,
    profile: &profiles::Profile,
    source: VerifySource<'_>,
    rust_artifact: &Path,
    rust_companion: Option<&Path>,
    disposition_label: Option<&str>,
    bounded_feature: bool,
) -> Result<Evaluation> {
    let reviewed_domain =
        profile.claim == open_radio_vendor_semantics::DriverAdapterClaim::ReviewedDomainEquivalence;
    if reviewed_domain != bounded_feature {
        return Err(crate::Error::invalid(format!(
            "profile {} claim {} requires disposition {}, but {}:{} uses {}",
            profile.name,
            profile.claim.label(),
            if reviewed_domain {
                "bounded-feature"
            } else {
                "a production whole-function disposition"
            },
            source.name,
            profile.vendor_symbol,
            disposition_label.unwrap_or("<unreviewed>")
        )));
    }

    let comparison = compare_execution_scenarios(
        svd,
        ExecutionInput {
            artifact: source.artifact,
            companion: source.companion,
            symbol: &profile.vendor_symbol,
        },
        ExecutionInput {
            artifact: rust_artifact,
            companion: rust_companion,
            symbol: &profile.rust_symbol,
        },
        profile.compare_return,
        &profile.coverage_constraints(),
        &profile.scenarios,
    )?;
    let all_declared_cases_match = comparison.summary.cases > 0
        && comparison.summary.matched == comparison.summary.cases
        && comparison.summary.different == 0
        && comparison.summary.incomplete == 0;
    let accepted_match = comparison.verdict == EquivalenceVerdict::Match
        || reviewed_domain && all_declared_cases_match;
    let matched_status = matched_status(profile.claim);

    Ok(Evaluation {
        comparison,
        accepted_match,
        reviewed_domain,
        matched_status,
    })
}

fn matched_status(
    claim: open_radio_vendor_semantics::DriverAdapterClaim,
) -> FunctionVerificationStatus {
    match claim {
        open_radio_vendor_semantics::DriverAdapterClaim::ReviewedDomainEquivalence => {
            FunctionVerificationStatus::BoundedMatch
        }
        open_radio_vendor_semantics::DriverAdapterClaim::WholeFunctionEquivalence => {
            FunctionVerificationStatus::Match
        }
        open_radio_vendor_semantics::DriverAdapterClaim::ReviewedProjection
        | open_radio_vendor_semantics::DriverAdapterClaim::RustConformance => {
            unreachable!("adapter-only claims are rejected by profile validation")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_claims_have_distinct_success_classes() {
        assert_eq!(
            matched_status(
                open_radio_vendor_semantics::DriverAdapterClaim::ReviewedDomainEquivalence
            ),
            FunctionVerificationStatus::BoundedMatch
        );
        assert_eq!(
            matched_status(
                open_radio_vendor_semantics::DriverAdapterClaim::WholeFunctionEquivalence
            ),
            FunctionVerificationStatus::Match
        );
    }
}
