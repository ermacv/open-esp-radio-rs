//! Static trust audit for every reviewed vendor-to-Rust executable binding.

use std::collections::BTreeMap;

use open_radio_vendor_semantics::{
    DriverAdapterClaim, DriverAdapterDomain, DriverAdapterRelation, DriverAdapterTrust,
    RustBindingKind, VendorOracleKind,
};
use serde::Serialize;

use crate::{ProjectSpec, Result, harnesses, qualification};

use super::{
    dispositions::{Disposition, Manifest},
    profiles,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct BindingAuditRow {
    pub(crate) suite: String,
    pub(crate) source: String,
    pub(crate) vendor_symbol: String,
    pub(crate) rust_component: String,
    pub(crate) rust_probe: String,
    pub(crate) rust_binding: RustBindingKind,
    pub(crate) vendor_oracle: VendorOracleKind,
    pub(crate) relation: DriverAdapterRelation,
    pub(crate) domain: DriverAdapterDomain,
    pub(crate) maximum_claim: DriverAdapterClaim,
    pub(crate) disposition: String,
    pub(crate) declared_claims: Vec<DriverAdapterClaim>,
    pub(crate) required_by: Vec<String>,
    pub(crate) release_required: bool,
    pub(crate) claim_valid: bool,
    pub(crate) disposition_valid: bool,
    pub(crate) release_eligible: bool,
    pub(crate) status: &'static str,
    pub(crate) driver_adapter: Option<String>,
    pub(crate) blocker: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct BindingAuditReport {
    pub(crate) schema: u32,
    pub(crate) project: String,
    pub(crate) bindings: Vec<BindingAuditRow>,
    pub(crate) release_eligible: usize,
    pub(crate) release_required: usize,
    pub(crate) release_blocked: usize,
    pub(crate) research_only: usize,
    pub(crate) invalid: usize,
    pub(crate) passed: bool,
}

fn disposition_policy(
    claim: DriverAdapterClaim,
    disposition: Disposition,
    release_required: bool,
) -> (bool, bool) {
    let qualifies = match claim {
        DriverAdapterClaim::WholeFunctionEquivalence => disposition != Disposition::BoundedFeature,
        _ => disposition == Disposition::BoundedFeature,
    };
    let valid = match claim {
        DriverAdapterClaim::WholeFunctionEquivalence => qualifies,
        _ => !release_required || qualifies,
    };
    (valid, qualifies)
}

fn binding_status(
    declaration_valid: bool,
    release_required: bool,
    release_eligible: bool,
) -> &'static str {
    if !declaration_valid {
        "invalid"
    } else if release_required && !release_eligible {
        "release-blocked"
    } else if release_required {
        "release-ready"
    } else if release_eligible {
        "available"
    } else {
        "research-only"
    }
}

fn trust_qualifies_for_release(trust: DriverAdapterTrust, binding: RustBindingKind) -> bool {
    trust.vendor == VendorOracleKind::ConcreteReplay
        && binding != RustBindingKind::VerificationProjection
        && trust.claim() != DriverAdapterClaim::ReviewedProjection
}

pub(crate) fn audit(project: &ProjectSpec, harness: Option<&str>) -> Result<BindingAuditReport> {
    let workspace = project.verification.as_ref().ok_or_else(|| {
        crate::Error::invalid("project audit bindings requires [[verification.suites]]")
    })?;
    let mut feature_requirements = qualification::binding_feature_requirements(project)?;
    let mut rows = BTreeMap::new();

    for suite in &workspace.suites {
        let Some(manifest) = Manifest::load_all(&suite.dispositions)? else {
            continue;
        };
        let mut configured_profiles = Vec::new();
        for path in &suite.profiles {
            configured_profiles.extend(profiles::load(path)?);
        }
        for entry in manifest.entries() {
            let Some(binding) = &entry.binding else {
                continue;
            };
            let key = (suite.id.clone(), entry.source.clone(), entry.symbol.clone());
            let requirements = feature_requirements.remove(&key).unwrap_or_default();
            let release_required = requirements
                .iter()
                .any(|requirement| requirement.release_required);
            let required_by = requirements
                .iter()
                .filter(|requirement| requirement.release_required)
                .map(|requirement| requirement.feature.clone())
                .collect::<Vec<_>>();
            let mut declared_claims = requirements
                .iter()
                .map(|requirement| requirement.claim)
                .collect::<Vec<_>>();
            declared_claims.sort_by_key(|claim| claim.label());
            declared_claims.dedup();
            let component = entry
                .rust_component
                .as_ref()
                .map_or_else(|| "<missing>".to_owned(), |value| value.label().to_owned());

            let (trust, adapter, mut issues) = if let Some(adapter) = &binding.driver_adapter {
                let id = adapter.label().to_owned();
                match harness
                    .and_then(|harness| harnesses::driver_adapter_evidence_sources(harness, &id))
                {
                    Some(sources) => (sources.trust, Some(id), Vec::new()),
                    None => (
                        DriverAdapterTrust {
                            vendor: VendorOracleKind::ManualAssumption,
                            rust: binding.rust_kind,
                            domain: DriverAdapterDomain::ReviewedDomain,
                            relation: DriverAdapterRelation::Conformance,
                        },
                        Some(id.clone()),
                        vec![format!("adapter {id} has no registered trust boundary")],
                    ),
                }
            } else if let Some(profile) = configured_profiles.iter().find(|profile| {
                profile.vendor_source == entry.source && profile.vendor_symbol == entry.symbol
            }) {
                (
                    DriverAdapterTrust {
                        vendor: VendorOracleKind::ConcreteReplay,
                        rust: binding.rust_kind,
                        domain: match profile.claim {
                            DriverAdapterClaim::WholeFunctionEquivalence => {
                                DriverAdapterDomain::WholeFunction
                            }
                            _ => DriverAdapterDomain::ReviewedDomain,
                        },
                        relation: DriverAdapterRelation::Exact,
                    },
                    None,
                    Vec::new(),
                )
            } else {
                (
                    DriverAdapterTrust {
                        vendor: VendorOracleKind::CompleteLiftedTrace,
                        rust: binding.rust_kind,
                        domain: DriverAdapterDomain::WholeFunction,
                        relation: DriverAdapterRelation::Projection,
                    },
                    None,
                    vec![
                        "vendor behavior is a lifted/static trace, not concrete replay".to_owned(),
                    ],
                )
            };

            if trust.rust != binding.rust_kind {
                issues.push(format!(
                    "binding declares {}, adapter proves {}",
                    binding.rust_kind.label(),
                    trust.rust.label()
                ));
            }
            if binding.rust_kind == RustBindingKind::VerificationProjection {
                issues.push(
                    "verification projection is not an executable production boundary".to_owned(),
                );
            }
            let maximum_claim = trust.claim();
            let claim_valid = declared_claims.iter().all(|claim| *claim == maximum_claim);
            if !claim_valid {
                issues.push(format!(
                    "feature policy requests [{}], but the binding proves only {}",
                    declared_claims
                        .iter()
                        .map(|claim| claim.label())
                        .collect::<Vec<_>>()
                        .join(", "),
                    maximum_claim.label(),
                ));
            }
            let (disposition_valid, disposition_qualifies) =
                disposition_policy(maximum_claim, entry.disposition, release_required);
            if !disposition_valid {
                issues.push(format!(
                    "{} claim requires {} disposition{}",
                    maximum_claim.label(),
                    if maximum_claim == DriverAdapterClaim::WholeFunctionEquivalence {
                        "a whole-function"
                    } else {
                        "bounded-feature"
                    },
                    if release_required {
                        " in the required feature closure"
                    } else {
                        ""
                    },
                ));
            }
            let declaration_valid = claim_valid && disposition_valid;
            let release_eligible = declaration_valid
                && issues.is_empty()
                && disposition_qualifies
                && trust_qualifies_for_release(trust, binding.rust_kind);
            let status = binding_status(declaration_valid, release_required, release_eligible);
            let blocker = (!issues.is_empty()).then(|| issues.join("; "));
            rows.insert(
                key,
                BindingAuditRow {
                    suite: suite.id.clone(),
                    source: entry.source.clone(),
                    vendor_symbol: entry.symbol.clone(),
                    rust_component: component,
                    rust_probe: binding.rust_probe.clone(),
                    rust_binding: binding.rust_kind,
                    vendor_oracle: trust.vendor,
                    relation: trust.relation,
                    domain: trust.domain,
                    maximum_claim,
                    disposition: entry.disposition.label().to_owned(),
                    declared_claims,
                    required_by,
                    release_required,
                    claim_valid,
                    disposition_valid,
                    release_eligible,
                    status,
                    driver_adapter: adapter,
                    blocker,
                },
            );
        }
    }

    let bindings = rows.into_values().collect::<Vec<_>>();
    let release_eligible = bindings
        .iter()
        .filter(|binding| binding.release_eligible)
        .count();
    let release_required = bindings
        .iter()
        .filter(|binding| binding.release_required)
        .count();
    let release_blocked = bindings
        .iter()
        .filter(|binding| binding.release_required && !binding.release_eligible)
        .count();
    let research_only = bindings
        .iter()
        .filter(|binding| binding.status == "research-only")
        .count();
    let invalid = bindings
        .iter()
        .filter(|binding| binding.status == "invalid")
        .count();
    Ok(BindingAuditReport {
        schema: 2,
        project: project.id.clone(),
        bindings,
        release_eligible,
        release_required,
        release_blocked,
        research_only,
        invalid,
        passed: release_blocked == 0 && invalid == 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_required_projection_remains_visible_without_blocking_release() {
        let (valid, qualifies) = disposition_policy(
            DriverAdapterClaim::ReviewedProjection,
            Disposition::ReplacedByComposition,
            false,
        );
        assert!(valid);
        assert!(!qualifies);
        assert_eq!(binding_status(valid, false, false), "research-only");
    }

    #[test]
    fn required_projection_needs_a_bounded_disposition() {
        let (valid, qualifies) = disposition_policy(
            DriverAdapterClaim::ReviewedProjection,
            Disposition::ReplacedByComposition,
            true,
        );
        assert!(!valid);
        assert!(!qualifies);
        assert_eq!(binding_status(valid, true, false), "invalid");
    }

    #[test]
    fn required_bounded_proof_can_be_release_ready() {
        let (valid, qualifies) = disposition_policy(
            DriverAdapterClaim::ReviewedRefinement,
            Disposition::BoundedFeature,
            true,
        );
        assert!(valid);
        assert!(qualifies);
        assert_eq!(binding_status(valid, true, true), "release-ready");
    }

    #[test]
    fn concrete_rust_conformance_over_production_code_is_release_eligible() {
        let trust = DriverAdapterTrust {
            vendor: VendorOracleKind::ConcreteReplay,
            rust: RustBindingKind::SharedProductionCore,
            domain: DriverAdapterDomain::ReviewedDomain,
            relation: DriverAdapterRelation::Conformance,
        };
        assert_eq!(trust.claim(), DriverAdapterClaim::RustConformance);
        assert!(trust_qualifies_for_release(trust, trust.rust));

        let projection = DriverAdapterTrust {
            rust: RustBindingKind::VerificationProjection,
            ..trust
        };
        assert!(!trust_qualifies_for_release(projection, projection.rust));
    }
}
