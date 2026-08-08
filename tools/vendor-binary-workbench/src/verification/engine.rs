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
    svd: &MmioRegisterMap,
    harness: &str,
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
            return Err(format!(
                "profile {} targets {}, but was routed to {}",
                profile.name, profile.vendor_source, source.name
            )
            .into());
        }
        if !profiled_vendor_symbols.insert(profile.vendor_symbol.as_str()) {
            return Err(format!(
                "multiple execution profiles target {} in {}",
                profile.vendor_symbol, source.name
            )
            .into());
        }
        if !vendor_symbols
            .iter()
            .any(|symbol| symbol.name == profile.vendor_symbol)
        {
            return Err(format!(
                "profile {} refers to missing {} vendor symbol {}",
                profile.name, source.name, profile.vendor_symbol
            )
            .into());
        }
        if !rust_symbols
            .iter()
            .any(|symbol| symbol.name == profile.rust_symbol)
        {
            return Err(format!(
                "profile {} refers to missing Rust symbol {}",
                profile.name, profile.rust_symbol
            )
            .into());
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
            return Err(format!(
                "Rust probe suffix {suffix:?} is ambiguous between {} and {}",
                previous.name, symbol.name
            )
            .into());
        }
    }

    let mut summary = VerifySummary {
        vendor_functions: vendor_symbols.len(),
        ..VerifySummary::default()
    };
    let mut functions = Vec::with_capacity(vendor_symbols.len());
    for vendor in &vendor_symbols {
        let suffix = vendor
            .name
            .strip_prefix(source.prefix)
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
            .ok_or_else(|| format!("no harness registered driver adapter {}", adapter.label()))?;
            if proof.matched {
                summary.matched += 1;
                summary.effect_contract_matches += 1;
                record_evidence(
                    evidence,
                    source.name,
                    &vendor.name,
                    driver_adapter_effect_evidence(harness, policy, binding, &proof.canonical),
                )?;
            } else {
                summary.mismatched += 1;
            }
            let mut function = FunctionVerificationReport::new(
                source.name,
                &vendor.name,
                if proof.matched {
                    FunctionVerificationStatus::Match
                } else {
                    FunctionVerificationStatus::Mismatch
                },
            );
            function.rust_symbol = Some(binding.rust_probe.clone());
            function.evidence = proof.matched.then(|| "effect-contract".to_owned());
            function.contract = Some(policy.comparison.label().to_owned());
            function.driver_adapter = Some(adapter.label().to_owned());
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
                        })?;
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
                        function.rust_component = entry.rust_component.clone();
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
                        function.rust_component = entry.rust_component.clone();
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
            let argument_domain = profile.coverage_argument_constraints();
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
                &argument_domain,
                &profile.scenarios,
            )?;
            let verdict = comparison.verdict;
            match verdict {
                ComparisonVerdict::Match => {
                    summary.matched += 1;
                    match profile.contract {
                        profiles::ProfileContract::Scenario => summary.scenario_matches += 1,
                        profiles::ProfileContract::State => summary.state_matches += 1,
                    }
                    record_evidence(
                        evidence,
                        source.name,
                        &vendor.name,
                        profile_evidence(profile),
                    )?;
                }
                ComparisonVerdict::Mismatch => summary.mismatched += 1,
                ComparisonVerdict::Incomplete => summary.incomplete += 1,
            }
            let status = match verdict {
                ComparisonVerdict::Match => FunctionVerificationStatus::Match,
                ComparisonVerdict::Mismatch => FunctionVerificationStatus::Mismatch,
                ComparisonVerdict::Incomplete => FunctionVerificationStatus::Incomplete,
            };
            let mut function = FunctionVerificationReport::new(source.name, &vendor.name, status);
            function.rust_symbol = Some(rust.name.clone());
            function.evidence = (verdict == ComparisonVerdict::Match)
                .then(|| profile.contract.evidence().to_owned());
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
                    effect_contract::EffectComparisonVerdict::Match,
                    effect_contract::EffectComparisonVerdict::Match,
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
                    effect_contract::EffectComparisonVerdict::Match,
                    effect_contract::EffectComparisonVerdict::Match,
                ) => {
                    summary.mismatched += 1;
                    (
                        FunctionVerificationStatus::Mismatch,
                        Some("return".to_owned()),
                    )
                }
                (effect_contract::EffectComparisonVerdict::Mismatch(reason), _) => {
                    summary.mismatched += 1;
                    (FunctionVerificationStatus::Mismatch, Some(reason))
                }
                (_, effect_contract::EffectComparisonVerdict::Mismatch(reason)) => {
                    summary.mismatched += 1;
                    (
                        FunctionVerificationStatus::Mismatch,
                        Some(format!("generated-reference: {reason}")),
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
            record_evidence(evidence, source.name, &vendor.name, "symbolic")?;
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
    Ok(SourceVerificationReport {
        source: source.name.to_owned(),
        summary,
        functions,
    })
}
