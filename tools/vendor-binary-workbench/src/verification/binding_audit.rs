//! Static declaration audit for reviewed vendor-to-Rust executable bindings.
//!
//! This module validates manifest consistency only. It never infers execution
//! truth, trust, equivalence, or qualification readiness from declarations.

use std::collections::BTreeMap;

use open_radio_vendor_semantics::{RustBindingKind, VerificationClaim};
use serde::Serialize;

use crate::{ProjectSpec, Result};

use super::dispositions::{Disposition, Manifest};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct BindingAuditRow {
    pub(crate) suite: String,
    pub(crate) source: String,
    pub(crate) vendor_symbol: String,
    pub(crate) rust_component: String,
    pub(crate) rust_probe: String,
    pub(crate) binding_version: &'static str,
    pub(crate) comparison_plan: String,
    pub(crate) rust_binding: RustBindingKind,
    pub(crate) disposition: String,
    pub(crate) declared_claims: Vec<VerificationClaim>,
    pub(crate) required_by: Vec<String>,
    pub(crate) verification_required: bool,
    pub(crate) declaration_valid: bool,
    pub(crate) status: &'static str,
    pub(crate) blocker: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct BindingAuditReport {
    pub(crate) schema: u32,
    pub(crate) scope: &'static str,
    pub(crate) project: String,
    pub(crate) bindings: Vec<BindingAuditRow>,
    pub(crate) declared: usize,
    pub(crate) verification_required: usize,
    pub(crate) invalid: usize,
    pub(crate) unbound_requirements: Vec<UnboundPolicyRequirement>,
    pub(crate) passed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct UnboundPolicyRequirement {
    pub(crate) suite: String,
    pub(crate) source: String,
    pub(crate) vendor_symbol: String,
    pub(crate) required_by: Vec<String>,
}

fn disposition_accepts_claim(claim: VerificationClaim, disposition: Disposition) -> bool {
    match claim {
        VerificationClaim::WholeFunctionEquivalence => disposition != Disposition::BoundedFeature,
        VerificationClaim::ReviewedDomainEquivalence
        | VerificationClaim::ReviewedRefinement
        | VerificationClaim::ReviewedProjection
        | VerificationClaim::RustConformance => disposition == Disposition::BoundedFeature,
    }
}

pub(crate) fn audit(project: &ProjectSpec) -> Result<BindingAuditReport> {
    let workspace = project.verification.as_ref().ok_or_else(|| {
        crate::Error::invalid("project audit bindings requires [[verification.suites]]")
    })?;
    let mut policy_requirements = super::policy::binding_requirements(project)?;
    let mut rows = BTreeMap::new();

    for suite in &workspace.suites {
        let Some(manifest) = Manifest::load_all(&suite.dispositions)? else {
            continue;
        };
        for entry in manifest.entries() {
            let Some(binding) = &entry.binding else {
                continue;
            };
            let key = (suite.id.clone(), entry.source.clone(), entry.symbol.clone());
            let requirements = policy_requirements.remove(&key).unwrap_or_default();
            let verification_required = !requirements.is_empty();
            let required_by = requirements
                .iter()
                .map(|requirement| requirement.surface.clone())
                .collect::<Vec<_>>();
            let mut declared_claims = requirements
                .iter()
                .map(|requirement| requirement.claim)
                .collect::<Vec<_>>();
            declared_claims.sort_by_key(|claim| claim.label());
            declared_claims.dedup();

            let mut issues = Vec::new();
            for claim in &declared_claims {
                if !disposition_accepts_claim(*claim, entry.disposition) {
                    issues.push(format!(
                        "declared {} requires a {} disposition",
                        claim.label(),
                        if *claim == VerificationClaim::WholeFunctionEquivalence {
                            "whole-function"
                        } else {
                            "bounded-feature"
                        }
                    ));
                }
            }
            if binding.rust_kind == RustBindingKind::ReviewedAbiProjection
                && declared_claims
                    .iter()
                    .any(|claim| *claim != VerificationClaim::ReviewedDomainEquivalence)
            {
                issues.push(
                    "a reviewed ABI projection may support only reviewed-domain-equivalence"
                        .to_owned(),
                );
            }
            if verification_required
                && matches!(
                    binding.rust_kind,
                    RustBindingKind::GeneratedReference
                        | RustBindingKind::SharedProductionCore
                        | RustBindingKind::VerificationProjection
                )
            {
                issues.push(format!(
                    "required verification cannot target a {} binding",
                    binding.rust_kind.label()
                ));
            }

            let declaration_valid = issues.is_empty();
            rows.insert(
                key,
                BindingAuditRow {
                    suite: suite.id.clone(),
                    source: entry.source.clone(),
                    vendor_symbol: entry.symbol.clone(),
                    rust_component: entry
                        .rust_component
                        .as_ref()
                        .map_or_else(|| "<missing>".to_owned(), |value| value.label().to_owned()),
                    rust_probe: binding.rust_probe.clone(),
                    binding_version: binding.version.label(),
                    comparison_plan: binding.comparison_plan.label().to_owned(),
                    rust_binding: binding.rust_kind,
                    disposition: entry.disposition.label().to_owned(),
                    declared_claims,
                    required_by,
                    verification_required,
                    declaration_valid,
                    status: if declaration_valid {
                        "declared"
                    } else {
                        "invalid"
                    },
                    blocker: (!issues.is_empty()).then(|| issues.join("; ")),
                },
            );
        }
    }

    let bindings = rows.into_values().collect::<Vec<_>>();
    let unbound_requirements = policy_requirements
        .into_iter()
        .map(
            |((suite, source, vendor_symbol), requirements)| UnboundPolicyRequirement {
                suite,
                source,
                vendor_symbol,
                required_by: requirements
                    .into_iter()
                    .map(|requirement| requirement.surface)
                    .collect(),
            },
        )
        .collect::<Vec<_>>();
    let invalid = bindings
        .iter()
        .filter(|binding| !binding.declaration_valid)
        .count();
    let verification_required = bindings
        .iter()
        .filter(|binding| binding.verification_required)
        .count();
    Ok(BindingAuditReport {
        schema: 5,
        scope: "binding-declarations",
        project: project.id.clone(),
        declared: bindings.len().saturating_sub(invalid),
        verification_required,
        invalid: invalid + unbound_requirements.len(),
        passed: invalid == 0 && unbound_requirements.is_empty(),
        unbound_requirements,
        bindings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_policy_does_not_claim_execution_readiness() {
        assert!(disposition_accepts_claim(
            VerificationClaim::WholeFunctionEquivalence,
            Disposition::Direct,
        ));
        assert!(disposition_accepts_claim(
            VerificationClaim::ReviewedRefinement,
            Disposition::BoundedFeature,
        ));
        assert!(!disposition_accepts_claim(
            VerificationClaim::ReviewedProjection,
            Disposition::ReplacedByComposition,
        ));
    }
}
