//! Project-wide vendor-to-Rust replacement and qualification projection.

use std::collections::{BTreeMap, BTreeSet};

use open_radio_vendor_semantics::DriverAdapterClaim;
use serde::Serialize;

use super::{
    FunctionVerificationReport, FunctionVerificationStatus, ProjectVerificationSuiteReport,
};
use crate::Result;

pub(crate) const REPLACEMENT_GRAPH_SCHEMA: u32 = 3;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct VendorFunctionId {
    pub(crate) source: String,
    pub(crate) symbol: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RustBindingScope {
    Production,
    ProductionFeature,
    VerificationProbeOnly,
}

#[derive(Debug, Serialize)]
pub(crate) struct RustReplacementTarget {
    pub(crate) binding_scope: RustBindingScope,
    /// Reviewed production owner. Verification probe names can never populate
    /// this field implicitly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) production_component: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) verification_probes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReplacementProof {
    pub(crate) suite: String,
    pub(crate) status: FunctionVerificationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) claim: Option<DriverAdapterClaim>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) probe_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) contract: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hil_evidence: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) blockers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReplacementEdge {
    pub(crate) vendor: VendorFunctionId,
    pub(crate) reviewed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) disposition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rust: Option<RustReplacementTarget>,
    pub(crate) status: FunctionVerificationStatus,
    pub(crate) proofs: Vec<ReplacementProof>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RustComponentCoverage {
    pub(crate) component_id: String,
    pub(crate) module_path: String,
    pub(crate) vendor_functions: Vec<VendorFunctionId>,
    pub(crate) verification_probes: Vec<String>,
    pub(crate) matched: usize,
    pub(crate) mismatched: usize,
    pub(crate) incomplete: usize,
    pub(crate) unqualified: usize,
    pub(crate) bounded: usize,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct ReplacementGraphSummary {
    pub(crate) vendor_functions: usize,
    pub(crate) reviewed_dispositions: usize,
    pub(crate) production_components: usize,
    pub(crate) production_replacements: usize,
    pub(crate) production_feature_bindings: usize,
    pub(crate) verification_probe_bindings: usize,
    pub(crate) behavioral_matches: usize,
    pub(crate) production_matches: usize,
    pub(crate) bounded_matches: usize,
    pub(crate) probe_only_matches: usize,
    pub(crate) unmapped_matches: usize,
    pub(crate) mismatches: usize,
    pub(crate) incomplete: usize,
    pub(crate) implemented_unqualified: usize,
    pub(crate) uncovered: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReplacementGraph {
    pub(crate) schema_version: u32,
    pub(crate) summary: ReplacementGraphSummary,
    pub(crate) components: Vec<RustComponentCoverage>,
    pub(crate) replacements: Vec<ReplacementEdge>,
}

#[derive(Default)]
struct ReplacementBuilder {
    reviewed: bool,
    disposition: Option<String>,
    protocol: Option<String>,
    component: Option<String>,
    probes: BTreeSet<String>,
    proofs: Vec<ReplacementProof>,
}

#[derive(Default)]
struct ComponentBuilder {
    vendor_functions: BTreeSet<VendorFunctionId>,
    probes: BTreeSet<String>,
    matched: usize,
    mismatched: usize,
    incomplete: usize,
    unqualified: usize,
    bounded: usize,
}

impl ReplacementGraph {
    pub(crate) fn from_suites(suites: &[ProjectVerificationSuiteReport]) -> Result<Self> {
        let mut builders = BTreeMap::<VendorFunctionId, ReplacementBuilder>::new();
        for suite in suites {
            for function in suite
                .verification
                .sources
                .iter()
                .flat_map(|source| &source.functions)
            {
                let vendor = VendorFunctionId {
                    source: function.source.clone(),
                    symbol: function.vendor_symbol.clone(),
                };
                builders
                    .entry(vendor)
                    .or_default()
                    .merge(&suite.id, function)?;
            }
        }

        let mut summary = ReplacementGraphSummary::default();
        let mut components = BTreeMap::<String, ComponentBuilder>::new();
        let replacements = builders
            .into_iter()
            .map(|(vendor, builder)| {
                summary.vendor_functions += 1;
                summary.reviewed_dispositions += usize::from(builder.reviewed);
                let bounded_feature = builder.disposition.as_deref() == Some("bounded-feature");
                summary.production_replacements +=
                    usize::from(builder.component.is_some() && !bounded_feature);
                summary.production_feature_bindings +=
                    usize::from(builder.component.is_some() && bounded_feature);
                summary.verification_probe_bindings += usize::from(!builder.probes.is_empty());
                let status = aggregate_status(&builder.proofs);
                update_status_summary(&mut summary, status);
                if status == FunctionVerificationStatus::Match {
                    classify_match(&mut summary, &builder);
                }

                if let Some(component) = &builder.component {
                    let entry = components.entry(component.clone()).or_default();
                    entry.vendor_functions.insert(vendor.clone());
                    entry.probes.extend(builder.probes.iter().cloned());
                    match status {
                        FunctionVerificationStatus::Match => entry.matched += 1,
                        FunctionVerificationStatus::BoundedMatch => entry.bounded += 1,
                        FunctionVerificationStatus::Mismatch => entry.mismatched += 1,
                        FunctionVerificationStatus::Incomplete => entry.incomplete += 1,
                        FunctionVerificationStatus::ImplementedUnqualified => {
                            entry.unqualified += 1;
                        }
                        FunctionVerificationStatus::Uncovered => {}
                    }
                }
                let rust = (builder.component.is_some() || !builder.probes.is_empty()).then(|| {
                    RustReplacementTarget {
                        binding_scope: if builder.component.is_some() && bounded_feature {
                            RustBindingScope::ProductionFeature
                        } else if builder.component.is_some() {
                            RustBindingScope::Production
                        } else {
                            RustBindingScope::VerificationProbeOnly
                        },
                        production_component: builder.component.clone(),
                        verification_probes: builder.probes.into_iter().collect(),
                    }
                });
                ReplacementEdge {
                    vendor,
                    reviewed: builder.reviewed,
                    disposition: builder.disposition,
                    protocol: builder.protocol,
                    rust,
                    status,
                    proofs: builder.proofs,
                }
            })
            .collect();

        summary.production_components = components.len();
        let components = components
            .into_iter()
            .map(|(component_id, builder)| RustComponentCoverage {
                module_path: component_id.clone(),
                component_id,
                vendor_functions: builder.vendor_functions.into_iter().collect(),
                verification_probes: builder.probes.into_iter().collect(),
                matched: builder.matched,
                mismatched: builder.mismatched,
                incomplete: builder.incomplete,
                unqualified: builder.unqualified,
                bounded: builder.bounded,
            })
            .collect();

        Ok(Self {
            schema_version: REPLACEMENT_GRAPH_SCHEMA,
            summary,
            components,
            replacements,
        })
    }
}

impl ReplacementBuilder {
    fn merge(&mut self, suite: &str, function: &FunctionVerificationReport) -> Result<()> {
        if function.disposition_reviewed {
            merge_reviewed_value(
                &mut self.disposition,
                function.disposition.as_deref(),
                "disposition",
                function,
            )?;
            merge_reviewed_value(
                &mut self.protocol,
                function.protocol.as_deref(),
                "protocol",
                function,
            )?;
            merge_reviewed_value(
                &mut self.component,
                function.rust_component.as_deref(),
                "Rust component",
                function,
            )?;
            self.reviewed = true;
        }
        if let Some(probe) = &function.rust_symbol {
            self.probes.insert(probe.clone());
        }
        self.proofs.push(proof(suite, function));
        Ok(())
    }
}

fn merge_reviewed_value(
    stored: &mut Option<String>,
    candidate: Option<&str>,
    kind: &str,
    function: &FunctionVerificationReport,
) -> Result<()> {
    let Some(candidate) = candidate else {
        return Ok(());
    };
    match stored {
        Some(stored) if stored != candidate => Err(crate::Error::invalid(format!(
            "conflicting reviewed {kind} for {} {}: {stored:?} versus {candidate:?}",
            function.source, function.vendor_symbol
        ))),
        Some(_) => Ok(()),
        None => {
            *stored = Some(candidate.to_owned());
            Ok(())
        }
    }
}

fn aggregate_status(proofs: &[ReplacementProof]) -> FunctionVerificationStatus {
    const PRIORITY: [FunctionVerificationStatus; 6] = [
        FunctionVerificationStatus::Mismatch,
        FunctionVerificationStatus::Incomplete,
        FunctionVerificationStatus::Match,
        FunctionVerificationStatus::BoundedMatch,
        FunctionVerificationStatus::ImplementedUnqualified,
        FunctionVerificationStatus::Uncovered,
    ];
    PRIORITY
        .into_iter()
        .find(|status| proofs.iter().any(|proof| proof.status == *status))
        .unwrap_or(FunctionVerificationStatus::Uncovered)
}

fn update_status_summary(
    summary: &mut ReplacementGraphSummary,
    status: FunctionVerificationStatus,
) {
    match status {
        FunctionVerificationStatus::Match => summary.behavioral_matches += 1,
        FunctionVerificationStatus::BoundedMatch => summary.bounded_matches += 1,
        FunctionVerificationStatus::Mismatch => summary.mismatches += 1,
        FunctionVerificationStatus::Incomplete => summary.incomplete += 1,
        FunctionVerificationStatus::ImplementedUnqualified => {
            summary.implemented_unqualified += 1;
        }
        FunctionVerificationStatus::Uncovered => summary.uncovered += 1,
    }
}

fn classify_match(summary: &mut ReplacementGraphSummary, builder: &ReplacementBuilder) {
    if builder.component.is_some() {
        summary.production_matches += 1;
    } else if !builder.probes.is_empty() {
        summary.probe_only_matches += 1;
    } else {
        summary.unmapped_matches += 1;
    }
}

fn proof(suite: &str, function: &FunctionVerificationReport) -> ReplacementProof {
    ReplacementProof {
        suite: suite.to_owned(),
        status: function.status,
        claim: function.claim,
        probe_symbol: function.rust_symbol.clone(),
        evidence: function.evidence.clone(),
        contract: function.contract.clone(),
        profile: function.profile.clone(),
        hil_evidence: function.hil_evidence.clone(),
        blockers: function.qualification_blockers.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_status_ignores_uncovered_duplicates_but_never_hides_failures() {
        let proof = |status| ReplacementProof {
            suite: "suite".to_owned(),
            status,
            claim: None,
            probe_symbol: None,
            evidence: None,
            contract: None,
            profile: None,
            hil_evidence: None,
            blockers: Vec::new(),
        };
        assert_eq!(
            aggregate_status(&[
                proof(FunctionVerificationStatus::Uncovered),
                proof(FunctionVerificationStatus::Match),
            ]),
            FunctionVerificationStatus::Match
        );
        assert_eq!(
            aggregate_status(&[
                proof(FunctionVerificationStatus::Match),
                proof(FunctionVerificationStatus::Mismatch),
            ]),
            FunctionVerificationStatus::Mismatch
        );
    }

    #[test]
    fn conflicting_reviewed_component_identity_fails_closed() {
        let function =
            FunctionVerificationReport::new("lib", "vendor_fn", FunctionVerificationStatus::Match);
        let mut stored = Some("crate_a::replacement".to_owned());
        let error = merge_reviewed_value(
            &mut stored,
            Some("crate_b::replacement"),
            "Rust component",
            &function,
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("conflicting reviewed Rust component")
        );
    }

    #[test]
    fn match_classification_never_promotes_a_probe_to_production() {
        let mut summary = ReplacementGraphSummary::default();
        let mut probe_only = ReplacementBuilder::default();
        probe_only.probes.insert("open_trace_leaf".to_owned());
        classify_match(&mut summary, &probe_only);
        assert_eq!(summary.probe_only_matches, 1);
        assert_eq!(summary.production_matches, 0);

        let production = ReplacementBuilder {
            component: Some("crate_name::module::leaf".to_owned()),
            probes: BTreeSet::from(["open_trace_leaf".to_owned()]),
            ..ReplacementBuilder::default()
        };
        classify_match(&mut summary, &production);
        assert_eq!(summary.production_matches, 1);
        assert_eq!(summary.probe_only_matches, 1);
        assert_eq!(summary.unmapped_matches, 0);

        classify_match(&mut summary, &ReplacementBuilder::default());
        assert_eq!(summary.unmapped_matches, 1);
    }
}
