//! Inventory matching, verification gates and probe accounting.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
};

use crate::*;

use super::{
    EvidenceClass, FunctionVerificationReport, FunctionVerificationStatus, SourceVerificationReport,
};

mod execution_profile;
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
    knowledge_provider: Option<&str>,
    rust_target: &str,
    source: VerifySource<'_>,
    rust_artifact: &Path,
    rust_companion: Option<&Path>,
    rust_prefix: &str,
    execution_profiles: &[profiles::Profile],
    disposition_manifest: Option<&dispositions::Manifest>,
    evidence: &mut EvidenceSet,
) -> Result<SourceVerificationReport> {
    let diagnostic_contracts = crate::harnesses::diagnostic_contracts_or_empty(knowledge_provider)?;
    let vendor_symbols = vendor_symbols(source)?;
    // A reviewed binding names one exact compiled symbol and is independent
    // of the convention-based probe prefix. Keep the filtered inventory only
    // for convention-based pairing and orphan reporting.
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
        if !all_rust_symbols
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
        let selected_rust: Option<(&ArtifactSymbolIdentity, bool)> = if let Some(profile) =
            execution_profiles
                .iter()
                .find(|profile| profile.vendor_symbol == vendor.name)
        {
            all_rust_symbols
                .iter()
                .find(|symbol| symbol.name == profile.rust_symbol)
                .map(|symbol| (symbol, profile.compare_return))
        } else if let Some(binding) = manifest_entry.and_then(|entry| entry.binding.as_ref()) {
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
                    summary.missing += 1;
                    summary.implemented_unqualified += 1;
                    let release_blockers = entry
                        .release_blockers
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
                    function.release_blockers = release_blockers;
                    function.reason = Some("missing-compiled-production-binding".to_owned());
                    functions.push(function);
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
            member: if source.inventories.is_empty() {
                vendor.member.clone()
            } else {
                None
            },
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
            let evaluation = execution_profile::evaluate(
                svd,
                profile,
                source,
                rust_artifact,
                rust_companion,
                diagnostic_contracts.clone(),
                execution_profile::ReviewedBinding {
                    disposition_label: resolved_disposition
                        .as_ref()
                        .map(|resolved| resolved.disposition.label()),
                    bounded_feature,
                    effect_policy,
                },
            )?;
            let comparison = evaluation.comparison;
            let verdict = comparison.verdict;
            let accepted_match = evaluation.accepted_match;
            let binding_kind = manifest_entry
                .and_then(|entry| entry.binding.as_ref())
                .map(|binding| binding.rust_kind);
            let compiled_component_executed = if accepted_match {
                manifest_entry
                    .map(|entry| executed_production_component(entry, &comparison, rust_artifact))
                    .transpose()?
                    .unwrap_or(false)
            } else {
                false
            };
            let production_trace = compiled_component_executed
                && binding_kind.is_some_and(|kind| {
                    matches!(
                        kind,
                        open_radio_vendor_semantics::RustBindingKind::ExactProductionEntry
                            | open_radio_vendor_semantics::RustBindingKind::ReviewedAbiProjection
                    )
                });
            let shared_core = compiled_component_executed
                && binding_kind
                    == Some(open_radio_vendor_semantics::RustBindingKind::SharedProductionCore);
            if accepted_match && production_trace {
                if evaluation.reviewed_domain {
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
                    profile_evidence(profile, &comparison.diagnostic_contracts),
                )?;
            } else if accepted_match {
                summary.implemented_unqualified += 1;
                record_evidence(
                    evidence,
                    source.name,
                    &vendor.name,
                    profile_evidence(profile, &comparison.diagnostic_contracts),
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
            let status = if accepted_match && production_trace {
                evaluation.matched_status
            } else if accepted_match {
                FunctionVerificationStatus::ImplementedUnqualified
            } else {
                status
            };
            let mut function = FunctionVerificationReport::new(source.name, &vendor.name, status);
            function.evidence_class = if production_trace {
                EvidenceClass::ProductionTrace
            } else if shared_core {
                EvidenceClass::SharedCore
            } else {
                EvidenceClass::StaticAnalysis
            };
            function.rust_symbol = Some(rust.name.clone());
            function.evidence = accepted_match.then(|| profile.contract.evidence().to_owned());
            function.claim = accepted_match.then_some(profile.claim);
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
            if accepted_match && !production_trace {
                function.reason = Some(if !compiled_component_executed {
                    "concrete Rust execution did not reach the reviewed compiled production component"
                        .to_owned()
                } else {
                    format!(
                        "binding {} is supporting evidence, not a production trace",
                        binding_kind.map_or("missing", |kind| kind.label())
                    )
                });
            } else if accepted_match && evaluation.reviewed_domain {
                function.reason = Some(format!(
                    "every declared concrete case matches under reviewed precondition {:?}; static coverage outside that finite domain remains visible and this is not whole-function vendor equivalence",
                    profile
                        .precondition
                        .as_deref()
                        .expect("reviewed domain has a validated precondition")
                ));
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
            let provider = require_knowledge_provider(
                knowledge_provider,
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
                    provider,
                    rust_target,
                    &vendor_input,
                    &generated_companions,
                    &vendor_trace,
                )?;
            tracing::debug!(
                source = source.name,
                symbol = vendor.name,
                identity = %generated_proof.canonical(),
                "validated independent generated-reference trace"
            );
            let extracted_effects = (|| -> core::result::Result<_, String> {
                Ok((
                    effect_contract::effects_from_observable(&vendor_trace.events)
                        .map_err(|error| format!("vendor trace: {error}"))?,
                    effect_contract::effects_from_observable(&generated_proof.trace.events)
                        .map_err(|error| format!("generated reference: {error}"))?,
                    effect_contract::effects_from_observable(&rust_trace.events)
                        .map_err(|error| format!("compiled Rust trace: {error}"))?,
                ))
            })();
            let (vendor_effects, generated_effects, rust_effects) = match extracted_effects {
                Ok(effects) => effects,
                Err(reason) => {
                    // An effect that the current contract schema cannot
                    // represent is an explicit analysis blocker, not a
                    // malformed project and never a reason to discard the
                    // remaining suite observations.
                    summary.incomplete += 1;
                    let mut function = FunctionVerificationReport::new(
                        source.name,
                        &vendor.name,
                        FunctionVerificationStatus::Incomplete,
                    );
                    function.rust_symbol = Some(rust.name.clone());
                    function.contract = Some(policy.comparison.label().to_owned());
                    function.reason = Some(format!(
                        "effect-contract-v2 cannot represent an observed effect: {reason}"
                    ));
                    function.return_compared = Some(compare_return);
                    functions.push(function);
                    continue;
                }
            };
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
                    // This path compares complete lifted traces, not concrete
                    // executions. It remains useful reviewed evidence but may
                    // not mint release equivalence or a reusable accepted
                    // baseline. Concrete execution profiles take the branch
                    // above.
                    summary.implemented_unqualified += 1;
                    (
                        FunctionVerificationStatus::ImplementedUnqualified,
                        Some(
                            "complete lifted/static trace agrees, but release equivalence requires concrete vendor replay"
                                .to_owned(),
                        ),
                    )
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

fn executed_production_component(
    entry: &dispositions::Entry,
    comparison: &ExecutionComparisonReport,
    rust_artifact: &Path,
) -> Result<bool> {
    let Some(component) = entry.rust_component.as_ref() else {
        return Ok(false);
    };
    if comparison.rust_executed_pcs.is_empty() {
        return Ok(false);
    }
    let frames = artifact::inspect_rust_debug_frames(rust_artifact, &comparison.rust_executed_pcs)?;
    Ok(frames_reach_component(component.label(), &frames))
}

fn frames_reach_component(component: &str, frames: &[artifact::ArtifactDebugFrame]) -> bool {
    frames.iter().any(|frame| {
        super::rust_component_index::compiled_matches(component, &frame.demangled_name)
    })
}

fn require_knowledge_provider<'a>(provider: Option<&'a str>, capability: &str) -> Result<&'a str> {
    provider.ok_or_else(|| {
        crate::Error::invalid(format!(
            "{capability} requires a project or chip pack with an executable knowledge provider"
        ))
    })
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    #[test]
    fn production_binding_requires_an_executed_component_frame() {
        let frames = [artifact::ArtifactDebugFrame {
            address: 0x1000,
            demangled_name:
                "open_esp_radio_esp32s31_wifi_mac::tx_runtime::select_ordinary_retry_rate"
                    .to_owned(),
        }];

        assert!(frames_reach_component(
            "open_esp_radio_esp32s31_wifi_mac::tx_runtime::select_ordinary_retry_rate",
            &frames,
        ));
        assert!(!frames_reach_component(
            "open_esp_radio_esp32s31_wifi_mac::tx::TxCompletion::disposition",
            &frames,
        ));
        assert!(!frames_reach_component(
            "open_esp_radio_esp32s31_wifi_mac::tx_runtime::select_ordinary_retry_rate",
            &[],
        ));
    }
}
