//! Inventory matching, verification gates and probe accounting.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
};

use crate::*;

use super::{FunctionVerificationReport, FunctionVerificationStatus, SourceVerificationReport};

mod model;

pub(crate) use model::*;

#[allow(
    clippy::too_many_arguments,
    reason = "source verification keeps all artifact and policy inputs explicit"
)]
#[tracing::instrument(
    name = "verify_vendor_source",
    skip_all,
    fields(source = source.name, artifact = %source.artifact.display())
)]
pub(crate) fn verify_source(
    svd: &MmioMap,
    harness: Option<&str>,
    rust_target: &str,
    source: VerifySource<'_>,
    rust_artifact: &Path,
    rust_companion: Option<&Path>,
    rust_prefix: &str,
    execution_profiles: &[profiles::Profile],
    disposition_manifest: Option<&dispositions::Manifest>,
    evidence: &mut EvidenceSet,
) -> Result<SourceVerificationReport> {
    let vendor_symbols = vendor_symbols(source)?;
    // Binding v1 names one exact compiled symbol and is independent of the
    // convention-based probe prefix. Keep the filtered inventory only for
    // convention-based pairing and orphan reporting.
    let all_rust_symbols = list_code_symbols(rust_artifact, "")?;
    let rust_symbols = list_code_symbols(rust_artifact, rust_prefix)?;
    if let Some(manifest) = disposition_manifest {
        for entry in manifest
            .entries()
            .filter(|entry| entry.source == source.name)
        {
            if let Some(binding) = &entry.binding {
                binding.validate(&all_rust_symbols)?;
            }
        }
    }
    let mut profiled_vendor_symbols = BTreeSet::new();
    for profile in execution_profiles {
        if profile.vendor_source != source.name && source.name != "vendor" {
            return Err(crate::Error::invalid(format!(
                "profile {} targets {}, but was routed to {}",
                profile.name, profile.vendor_source, source.name
            )));
        }
        if !profiled_vendor_symbols.insert(profile.vendor_symbol.as_str()) {
            return Err(crate::Error::invalid(format!(
                "multiple execution profiles target {} in {}",
                profile.vendor_symbol, source.name
            )));
        }
        if !vendor_symbols
            .iter()
            .any(|symbol| symbol.name == profile.vendor_symbol)
        {
            return Err(crate::Error::invalid(format!(
                "profile {} refers to missing {} vendor symbol {}",
                profile.name, source.name, profile.vendor_symbol
            )));
        }
        if !rust_symbols
            .iter()
            .any(|symbol| symbol.name == profile.rust_symbol)
        {
            return Err(crate::Error::invalid(format!(
                "profile {} refers to missing Rust symbol {}",
                profile.name, profile.rust_symbol
            )));
        }
    }
    let mut rust_by_suffix = HashMap::new();
    for symbol in &rust_symbols {
        let Some(suffix) = symbol.name.strip_prefix(rust_prefix) else {
            continue;
        };
        let (suffix, compare_return) = suffix
            .strip_prefix("ret_")
            .map_or((suffix, false), |suffix| (suffix, true));
        if let Some((previous, _)) = rust_by_suffix.insert(suffix, (symbol, compare_return)) {
            return Err(crate::Error::invalid(format!(
                "Rust probe suffix {suffix:?} is ambiguous between {} and {}",
                previous.name, symbol.name
            )));
        }
    }

    let mut summary = VerifySummary {
        vendor_functions: vendor_symbols.len(),
        ..VerifySummary::default()
    };
    let mut functions = Vec::with_capacity(vendor_symbols.len());
    for vendor in &vendor_symbols {
        let suffix = source
            .selection
            .stripped_name(&vendor.name)
            .expect("symbol was filtered by vendor prefix");
        let source_qualified_suffix = format!("{}_{suffix}", source.name);
        let manifest_entry = disposition_manifest
            .and_then(|manifest| manifest.resolve(source.name, &vendor.name).entry);
        if let Some((entry, binding, adapter)) = manifest_entry.and_then(|entry| {
            entry.binding.as_ref().and_then(|binding| {
                binding
                    .driver_adapter
                    .as_ref()
                    .map(|adapter| (entry, binding, adapter))
            })
        }) {
            let harness = require_platform_harness(harness, "driver-adapter verification")?;
            let policy = entry
                .effect_contract
                .as_ref()
                .expect("driver adapter requires an effect contract");
            let proof = harnesses::verify_driver_adapter(
                harness,
                &harnesses::DriverAdapterRequest {
                    id: adapter.label(),
                    source: source.name,
                    vendor_symbol: &vendor.name,
                    svd,
                    vendor_inventory: source.inventory,
                    vendor_artifact: source.artifact,
                    vendor_companion: source.companion,
                    rust_artifact,
                    rust_companion,
                    rust_symbol: &binding.rust_probe,
                    policy,
                },
            )?
            .ok_or_else(|| format!("no harness registered driver adapter {}", adapter.label()))
            .map_err(crate::Error::invalid)?;
            let whole_function = proof.claim
                == open_radio_vendor_semantics::DriverAdapterClaim::WholeFunctionEquivalence;
            let bounded_feature = entry.disposition.is_bounded_feature();
            if bounded_feature && whole_function {
                return Err(crate::Error::invalid(format!(
                    "bounded-feature {} {} cannot use a whole-function-equivalence adapter claim",
                    source.name, vendor.name
                )));
            }
            if proof.matched && whole_function {
                summary.matched += 1;
                summary.effect_contract_matches += 1;
                record_evidence(
                    evidence,
                    source.name,
                    &vendor.name,
                    driver_adapter_effect_evidence(harness, policy, binding, &proof.canonical),
                )?;
            } else if proof.matched && bounded_feature {
                summary.bounded_matches += 1;
                record_evidence(
                    evidence,
                    source.name,
                    &vendor.name,
                    driver_adapter_limited_claim_evidence(
                        harness,
                        policy,
                        binding,
                        proof.claim,
                        &proof.canonical,
                    ),
                )?;
            } else if proof.matched {
                summary.implemented_unqualified += 1;
                record_evidence(
                    evidence,
                    source.name,
                    &vendor.name,
                    driver_adapter_limited_claim_evidence(
                        harness,
                        policy,
                        binding,
                        proof.claim,
                        &proof.canonical,
                    ),
                )?;
            } else {
                summary.mismatched += 1;
            }
            let status = if !proof.matched {
                FunctionVerificationStatus::Mismatch
            } else if whole_function {
                FunctionVerificationStatus::Match
            } else if bounded_feature {
                FunctionVerificationStatus::BoundedMatch
            } else {
                FunctionVerificationStatus::ImplementedUnqualified
            };
            let mut function = FunctionVerificationReport::new(source.name, &vendor.name, status);
            function.rust_symbol = Some(binding.rust_probe.clone());
            function.evidence = proof.matched.then(|| {
                if whole_function {
                    "effect-contract".to_owned()
                } else {
                    proof.claim.label().to_owned()
                }
            });
            function.contract = Some(policy.comparison.label().to_owned());
            function.driver_adapter = Some(adapter.label().to_owned());
            function.claim = Some(proof.claim);
            function.adapter_cases = proof.cases.clone();
            if !proof.matched {
                function.reason = proof
                    .cases
                    .iter()
                    .find(|case| !case.matched)
                    .and_then(|case| {
                        case.reason
                            .as_ref()
                            .map(|reason| format!("case {:?}: {reason}", case.name))
                    });
            }
            if proof.matched && !whole_function {
                function.reason = Some(format!(
                    "{} evidence establishes only the reviewed bounded feature, not whole-function vendor equivalence",
                    match proof.claim {
                        open_radio_vendor_semantics::DriverAdapterClaim::ReviewedProjection =>
                            "reviewed projection",
                        open_radio_vendor_semantics::DriverAdapterClaim::RustConformance =>
                            "Rust conformance",
                        open_radio_vendor_semantics::DriverAdapterClaim::WholeFunctionEquivalence =>
                            unreachable!(),
                    }
                ));
            }
            functions.push(function);
            continue;
        }
        let selected_rust: Option<(&ArtifactSymbolIdentity, bool)> =
            if let Some(binding) = manifest_entry.and_then(|entry| entry.binding.as_ref()) {
                all_rust_symbols
                    .iter()
                    .find(|symbol| symbol.name == binding.rust_probe)
                    .map(|symbol| (symbol, binding.compare_return))
            } else {
                rust_by_suffix
                    .get(source_qualified_suffix.as_str())
                    .or_else(|| rust_by_suffix.get(suffix))
                    .copied()
            };
        let Some((rust, compare_return)) = selected_rust else {
            if let Some(manifest) = disposition_manifest {
                let resolved = manifest.resolve(source.name, &vendor.name);
                if resolved.disposition.is_implemented() {
                    let entry = resolved
                        .entry
                        .expect("implemented disposition must be an exact function entry");
                    if let Some(contract) = entry.semantic_contract.as_ref() {
                        let harness =
                            require_platform_harness(harness, "semantic-contract verification")?;
                        let matched = harnesses::verify_semantic_contract(
                            harness,
                            &harnesses::SemanticContractRequest {
                                id: contract.label(),
                                source: source.name,
                                vendor_symbol: &vendor.name,
                                svd,
                                vendor_artifact: source.artifact,
                                vendor_companion: source.companion,
                            },
                        )?
                        .ok_or_else(|| {
                            format!(
                                "no harness registered semantic contract {}",
                                contract.label()
                            )
                        })
                        .map_err(crate::Error::invalid)?;
                        if matched {
                            summary.matched += 1;
                            summary.composition_matches += 1;
                            record_evidence(
                                evidence,
                                source.name,
                                &vendor.name,
                                semantic_contract_evidence(harness, contract.label()),
                            )?;
                        } else {
                            summary.mismatched += 1;
                        }
                        let mut function = FunctionVerificationReport::new(
                            source.name,
                            &vendor.name,
                            if matched {
                                FunctionVerificationStatus::Match
                            } else {
                                FunctionVerificationStatus::Mismatch
                            },
                        );
                        function.rust_component = entry
                            .rust_component
                            .as_ref()
                            .map(|component| component.label().to_owned());
                        function.evidence =
                            matched.then(|| "composition-state-scenario".to_owned());
                        function.contract = Some(contract.label().to_owned());
                        function.hil_evidence = entry.hil_evidence.clone();
                        functions.push(function);
                    } else {
                        summary.missing += 1;
                        summary.implemented_unqualified += 1;
                        let qualification_blockers = entry
                            .qualification_blockers
                            .iter()
                            .map(|(source, symbol)| format!("{source}:{symbol}"))
                            .collect::<Vec<_>>();
                        let mut function = FunctionVerificationReport::new(
                            source.name,
                            &vendor.name,
                            FunctionVerificationStatus::ImplementedUnqualified,
                        );
                        function.rust_component = entry
                            .rust_component
                            .as_ref()
                            .map(|component| component.label().to_owned());
                        function.disposition = Some(resolved.disposition.label().to_owned());
                        function.protocol = Some(resolved.protocol.label().to_owned());
                        function.hil_evidence = entry.hil_evidence.clone();
                        function.qualification_blockers = qualification_blockers;
                        function.reason = Some("missing-semantic-contract".to_owned());
                        functions.push(function);
                    }
                } else {
                    summary.missing += 1;
                    summary.not_yet_ported += 1;
                    let mut function = FunctionVerificationReport::new(
                        source.name,
                        &vendor.name,
                        FunctionVerificationStatus::Uncovered,
                    );
                    function.disposition = Some(resolved.disposition.label().to_owned());
                    function.protocol = Some(resolved.protocol.label().to_owned());
                    function.reason = Some(format!(
                        "missing Rust probe {rust_prefix}{suffix} or {rust_prefix}{source_qualified_suffix}"
                    ));
                    functions.push(function);
                }
            } else {
                summary.missing += 1;
                let mut function = FunctionVerificationReport::new(
                    source.name,
                    &vendor.name,
                    FunctionVerificationStatus::Uncovered,
                );
                function.reason = Some(format!(
                    "missing Rust probe {rust_prefix}{suffix} or {rust_prefix}{source_qualified_suffix}"
                ));
                functions.push(function);
            }
            continue;
        };
        let vendor_input = ArtifactSymbolSelector {
            artifact: source.artifact.to_path_buf(),
            member: source
                .inventory
                .map_or_else(|| vendor.member.clone(), |_| None),
            symbol: vendor.name.clone(),
        };
        let vendor_trace = extract(&vendor_input, svd)?;
        let rust_trace = extract(
            &ArtifactSymbolSelector {
                artifact: rust_artifact.to_path_buf(),
                member: rust.member.clone(),
                symbol: rust.name.clone(),
            },
            svd,
        )?;
        let effect_policy = manifest_entry
            .and_then(|entry| entry.binding.as_ref().and(entry.effect_contract.as_ref()));
        if let Some(profile) = execution_profiles
            .iter()
            .find(|profile| profile.vendor_symbol == vendor.name)
        {
            let resolved_disposition =
                disposition_manifest.map(|manifest| manifest.resolve(source.name, &vendor.name));
            let bounded_feature = resolved_disposition
                .as_ref()
                .is_some_and(|resolved| resolved.disposition.is_bounded_feature());
            let coverage_domain = profile.coverage_constraints();
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
                &coverage_domain,
                &profile.scenarios,
            )?;
            let verdict = comparison.verdict;
            let bounded_projection_match = bounded_feature
                && comparison.summary.cases > 0
                && comparison.summary.matched == comparison.summary.cases
                && comparison.summary.different == 0
                && comparison.summary.incomplete == 0;
            let accepted_match = verdict == EquivalenceVerdict::Match || bounded_projection_match;
            if accepted_match {
                if bounded_feature {
                    summary.bounded_matches += 1;
                } else {
                    summary.matched += 1;
                    match profile.contract {
                        profiles::ProfileContract::Scenario => summary.scenario_matches += 1,
                        profiles::ProfileContract::State => summary.state_matches += 1,
                    }
                }
                record_evidence(
                    evidence,
                    source.name,
                    &vendor.name,
                    profile_evidence(profile),
                )?;
            } else {
                match verdict {
                    EquivalenceVerdict::Match => unreachable!("a match is always accepted"),
                    EquivalenceVerdict::Diff => summary.mismatched += 1,
                    EquivalenceVerdict::Incomplete => summary.incomplete += 1,
                }
            }
            let status = match verdict {
                EquivalenceVerdict::Match => FunctionVerificationStatus::Match,
                EquivalenceVerdict::Diff => FunctionVerificationStatus::Mismatch,
                EquivalenceVerdict::Incomplete => FunctionVerificationStatus::Incomplete,
            };
            let status = accepted_match
                .then(|| matched_profile_classification(bounded_feature).0)
                .unwrap_or(status);
            let mut function = FunctionVerificationReport::new(source.name, &vendor.name, status);
            function.rust_symbol = Some(rust.name.clone());
            function.evidence = accepted_match.then(|| profile.contract.evidence().to_owned());
            function.claim =
                accepted_match.then(|| matched_profile_classification(bounded_feature).1);
            if let Some(resolved) = resolved_disposition {
                function.disposition = Some(resolved.disposition.label().to_owned());
                function.protocol = Some(resolved.protocol.label().to_owned());
                function.disposition_reviewed = resolved.entry.is_some();
                function.rust_component = resolved.entry.and_then(|entry| {
                    entry
                        .rust_component
                        .as_ref()
                        .map(|component| component.label().to_owned())
                });
            }
            if accepted_match && bounded_feature {
                function.reason = Some(
                    "every declared concrete case matches inside the reviewed finite input projection; static coverage outside that projection is retained and this is not whole-function vendor equivalence"
                        .to_owned(),
                );
            }
            function.profile = Some(profile.name.clone());
            function.execution = Some(comparison);
            functions.push(function);
            continue;
        }
        if !vendor_trace.is_exact()
            || !rust_trace.is_exact()
            || (compare_return
                && (!vendor_trace.return_value.is_resolved()
                    || !rust_trace.return_value.is_resolved()))
        {
            summary.incomplete += 1;
            let mut uncovered = vendor_trace.blockers.len()
                + vendor_trace
                    .events
                    .iter()
                    .filter_map(ObservableEvent::unmapped_address)
                    .count()
                + rust_trace.blockers.len()
                + rust_trace
                    .events
                    .iter()
                    .filter_map(ObservableEvent::unmapped_address)
                    .count();
            if compare_return && !vendor_trace.return_value.is_resolved() {
                uncovered += 1;
            }
            if compare_return && !rust_trace.return_value.is_resolved() {
                uncovered += 1;
            }
            let mut function = FunctionVerificationReport::new(
                source.name,
                &vendor.name,
                FunctionVerificationStatus::Incomplete,
            );
            function.rust_symbol = Some(rust.name.clone());
            function.uncovered = Some(uncovered);
            function.return_compared = Some(compare_return);
            functions.push(function);
        } else if let Some(policy) = effect_policy {
            let harness = require_platform_harness(
                harness,
                "generated-reference effect-contract verification",
            )?;
            let generated_companions = source
                .companion
                .into_iter()
                .map(Path::to_path_buf)
                .collect::<Vec<_>>();
            let generated_proof =
                crate::generated_reference::generate_compile_and_prove_exact_mmio_leaf(
                    svd,
                    harness,
                    rust_target,
                    &vendor_input,
                    &generated_companions,
                    &vendor_trace,
                )?;
            let vendor_effects = effect_contract::effects_from_observable(&vendor_trace.events)?;
            let generated_effects =
                effect_contract::effects_from_observable(&generated_proof.trace.events)?;
            let rust_effects = effect_contract::effects_from_observable(&rust_trace.events)?;
            let vendor_to_rust =
                effect_contract::compare_effects(&vendor_effects, &rust_effects, policy)?;
            let generated_to_rust =
                effect_contract::compare_effects(&generated_effects, &rust_effects, policy)?;
            let (status, reason) = match (vendor_to_rust, generated_to_rust) {
                (
                    effect_contract::EquivalenceOutcome {
                        verdict: effect_contract::EquivalenceVerdict::Match,
                        ..
                    },
                    effect_contract::EquivalenceOutcome {
                        verdict: effect_contract::EquivalenceVerdict::Match,
                        ..
                    },
                ) if !compare_return || returns_equal(&vendor_trace, &rust_trace) => {
                    summary.matched += 1;
                    summary.effect_contract_matches += 1;
                    record_evidence(
                        evidence,
                        source.name,
                        &vendor.name,
                        effect_contract_evidence(
                            policy,
                            manifest_entry
                                .and_then(|entry| entry.binding.as_ref())
                                .expect("effect contract requires an executable binding"),
                            &generated_proof.canonical(),
                        ),
                    )?;
                    (FunctionVerificationStatus::Match, None)
                }
                (
                    effect_contract::EquivalenceOutcome {
                        verdict: effect_contract::EquivalenceVerdict::Match,
                        ..
                    },
                    effect_contract::EquivalenceOutcome {
                        verdict: effect_contract::EquivalenceVerdict::Match,
                        ..
                    },
                ) => {
                    summary.mismatched += 1;
                    (
                        FunctionVerificationStatus::Mismatch,
                        Some("return".to_owned()),
                    )
                }
                (
                    effect_contract::EquivalenceOutcome {
                        verdict: effect_contract::EquivalenceVerdict::Diff,
                        reason,
                        ..
                    },
                    _,
                ) => {
                    summary.mismatched += 1;
                    (FunctionVerificationStatus::Mismatch, reason)
                }
                (
                    _,
                    effect_contract::EquivalenceOutcome {
                        verdict: effect_contract::EquivalenceVerdict::Diff,
                        reason,
                        ..
                    },
                ) => {
                    summary.mismatched += 1;
                    (
                        FunctionVerificationStatus::Mismatch,
                        Some(format!(
                            "generated-reference: {}",
                            reason.as_deref().unwrap_or("semantic difference")
                        )),
                    )
                }
                (
                    effect_contract::EquivalenceOutcome {
                        verdict: effect_contract::EquivalenceVerdict::Incomplete,
                        reason,
                        ..
                    },
                    _,
                ) => {
                    summary.incomplete += 1;
                    (FunctionVerificationStatus::Incomplete, reason)
                }
                (
                    _,
                    effect_contract::EquivalenceOutcome {
                        verdict: effect_contract::EquivalenceVerdict::Incomplete,
                        reason,
                        ..
                    },
                ) => {
                    summary.incomplete += 1;
                    (
                        FunctionVerificationStatus::Incomplete,
                        Some(format!(
                            "generated-reference: {}",
                            reason.as_deref().unwrap_or("semantic coverage incomplete")
                        )),
                    )
                }
            };
            let mut function = FunctionVerificationReport::new(source.name, &vendor.name, status);
            function.rust_symbol = Some(rust.name.clone());
            function.evidence =
                (status == FunctionVerificationStatus::Match).then(|| "effect-contract".to_owned());
            function.contract = Some(policy.comparison.label().to_owned());
            function.reason = reason;
            function.effects = Some(vendor_effects.len());
            function.return_compared = Some(compare_return);
            functions.push(function);
        } else if traces_equal(&vendor_trace, &rust_trace)
            && (!compare_return || returns_equal(&vendor_trace, &rust_trace))
        {
            summary.matched += 1;
            summary.symbolic_matches += 1;
            record_evidence(
                evidence,
                source.name,
                &vendor.name,
                super::EvidenceIdentity::plain("symbolic"),
            )?;
            let mut function = FunctionVerificationReport::new(
                source.name,
                &vendor.name,
                FunctionVerificationStatus::Match,
            );
            function.rust_symbol = Some(rust.name.clone());
            function.evidence = Some("symbolic".to_owned());
            function.vendor_events = Some(vendor_trace.events.len());
            function.rust_events = Some(rust_trace.events.len());
            function.return_compared = Some(compare_return);
            functions.push(function);
        } else {
            summary.mismatched += 1;
            let mut function = FunctionVerificationReport::new(
                source.name,
                &vendor.name,
                FunctionVerificationStatus::Mismatch,
            );
            function.rust_symbol = Some(rust.name.clone());
            function.vendor_events = Some(vendor_trace.events.len());
            function.rust_events = Some(rust_trace.events.len());
            function.return_compared = Some(compare_return);
            functions.push(function);
        }
    }
    if let Some(manifest) = disposition_manifest {
        for function in &mut functions {
            annotate_replacement(function, manifest);
        }
    }
    Ok(SourceVerificationReport {
        source: source.name.to_owned(),
        summary,
        functions,
    })
}

fn matched_profile_classification(
    bounded_feature: bool,
) -> (
    FunctionVerificationStatus,
    open_radio_vendor_semantics::DriverAdapterClaim,
) {
    if bounded_feature {
        (
            FunctionVerificationStatus::BoundedMatch,
            open_radio_vendor_semantics::DriverAdapterClaim::ReviewedProjection,
        )
    } else {
        (
            FunctionVerificationStatus::Match,
            open_radio_vendor_semantics::DriverAdapterClaim::WholeFunctionEquivalence,
        )
    }
}

fn annotate_replacement(
    function: &mut FunctionVerificationReport,
    manifest: &dispositions::Manifest,
) {
    let resolved = manifest.resolve(&function.source, &function.vendor_symbol);
    function
        .disposition
        .get_or_insert_with(|| resolved.disposition.label().to_owned());
    function
        .protocol
        .get_or_insert_with(|| resolved.protocol.label().to_owned());
    let Some(entry) = resolved.entry else {
        return;
    };
    function.disposition_reviewed = true;
    if let Some(component) = &entry.rust_component {
        function
            .rust_component
            .get_or_insert_with(|| component.label().to_owned());
    }
    if let Some(binding) = &entry.binding {
        function
            .rust_symbol
            .get_or_insert_with(|| binding.rust_probe.clone());
    }
    if let Some(hil_evidence) = &entry.hil_evidence {
        function
            .hil_evidence
            .get_or_insert_with(|| hil_evidence.clone());
    }
}

fn require_platform_harness<'a>(harness: Option<&'a str>, capability: &str) -> Result<&'a str> {
    harness.ok_or_else(|| {
        crate::Error::invalid(format!(
            "{capability} requires a project platform pack with an executable harness"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_bounded_profile_cannot_claim_the_whole_vendor_function() {
        assert_eq!(
            matched_profile_classification(true),
            (
                FunctionVerificationStatus::BoundedMatch,
                open_radio_vendor_semantics::DriverAdapterClaim::ReviewedProjection,
            )
        );
        assert_eq!(
            matched_profile_classification(false),
            (
                FunctionVerificationStatus::Match,
                open_radio_vendor_semantics::DriverAdapterClaim::WholeFunctionEquivalence,
            )
        );
    }
}
