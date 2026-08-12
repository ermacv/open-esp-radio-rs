//! Vendor-to-Rust replacement report rendering.

use super::ReplacementInvestigationReport;
use crate::cli::output;
use crate::function_investigation::ReviewedEffectRuleEvidence;

pub(super) fn render(report: &ReplacementInvestigationReport) {
    outputln!("{}", output::heading("Vendor ↔ Rust replacement"));
    outputln!("Function: {}:{}", report.source, report.symbol);
    if report.replacements.is_empty() {
        outputln!(
            "\n{}",
            output::warning("No reviewed replacement edge exists in the current project report.")
        );
        outputln!("Run `project verify` after adding or changing a disposition.");
        return;
    }

    for replacement in &report.replacements {
        outputln!("\nStatus:      {}", replacement.status);
        outputln!(
            "Claim scope: {}",
            replacement.binding_scope.as_deref().unwrap_or("unmapped")
        );
        outputln!(
            "Disposition: {}",
            replacement.disposition.as_deref().unwrap_or("not reviewed")
        );
        outputln!(
            "Rust owner:  {}",
            replacement
                .production_component
                .as_deref()
                .unwrap_or("not assigned")
        );
        if replacement.status == "bounded-match" {
            outputln!(
                "Meaning:     one reviewed production property matches; whole-function equivalence is not claimed"
            );
        }
        if let Some(component) = &replacement.production_component_evidence {
            outputln!(
                "Owner proof: source={}, compiled={}, freshness={}",
                component.source_status,
                component.compiled_status,
                component.freshness_status
            );
            for item in &component.source_items {
                outputln!("  source: {}:{} ({})", item.path, item.line, item.kind);
            }
            if component.source_status == "resolved" && component.compiled_status == "missing" {
                outputln!(
                    "  note: the owner has no standalone ELF symbol; it may be a type or inlined compile-time item"
                );
            }
            if output::details() {
                for symbol in &component.compiled_symbols {
                    outputln!(
                        "  compiled: {} @ {} ({}, {} bytes)",
                        symbol.demangled,
                        symbol.address,
                        symbol.artifact,
                        symbol.size
                    );
                }
            }
        }
        if !replacement.proofs.is_empty() {
            let visible = replacement
                .proofs
                .iter()
                .filter(|proof| output::details() || proof.status != "uncovered")
                .collect::<Vec<_>>();
            outputln!("\nProofs:");
            outputln!(
                "{}",
                crate::cli::table::render(
                    ["Suite", "Status", "Claim", "Contract / evidence"],
                    visible.iter().map(|proof| [
                        proof.suite.clone(),
                        proof.status.clone(),
                        proof
                            .claim
                            .clone()
                            .unwrap_or_else(|| "not declared".to_owned()),
                        proof
                            .contract
                            .clone()
                            .or_else(|| proof.evidence.clone())
                            .unwrap_or_else(|| "none".to_owned()),
                    ]),
                )
            );
            for proof in &visible {
                if proof.effects.is_some() || proof.return_compared.is_some() {
                    outputln!(
                        "- {}: {} reviewed effect(s), return {}",
                        proof.suite,
                        proof.effects.unwrap_or(0),
                        if proof.return_compared.unwrap_or(false) {
                            "compared"
                        } else {
                            "not compared"
                        }
                    );
                }
                for case in &proof.adapter_cases {
                    outputln!(
                        "  case {}: {}{}",
                        case.name,
                        if case.matched { "match" } else { "diff" },
                        case.reason
                            .as_deref()
                            .map(|reason| format!(" — {reason}"))
                            .unwrap_or_default(),
                    );
                }
                for case in &proof.execution_cases {
                    let detail = match (case.events, case.memory_changes) {
                        (Some(events), Some(memory)) => {
                            format!(" — {events} event(s), {memory} RAM change(s)")
                        }
                        _ => case
                            .first_difference
                            .map(|index| {
                                format!(
                                    " — first {} difference at #{index}",
                                    case.difference_kind.as_deref().unwrap_or("trace")
                                )
                            })
                            .unwrap_or_default(),
                    };
                    outputln!("  case {}: {}{detail}", case.name, case.verdict);
                }
                if output::details()
                    && let Some(reason) = &proof.reason
                {
                    outputln!("  reason: {reason}");
                }
            }
            let hidden = replacement.proofs.len().saturating_sub(visible.len());
            if hidden != 0 {
                outputln!(
                    "{hidden} inventory-only uncovered suite row(s) hidden; use --details to show them."
                );
            }
        }
    }

    if !report.reviewed_effects.is_empty() {
        outputln!("\n{}", output::heading("Reviewed effect boundary"));
        outputln!("Policy rows below are not an observed execution trace.");
        let mut suites =
            std::collections::BTreeMap::<&str, Vec<&ReviewedEffectRuleEvidence>>::new();
        for effect in &report.reviewed_effects {
            suites.entry(&effect.suite).or_default().push(effect);
        }
        for (suite, effects) in suites {
            outputln!("\n{suite}");
            for effect in effects {
                outputln!("- {}", effect.selector);
                outputln!("  policy: {}", effect.disposition);
            }
        }
    }

    if !report.feature_qualifications.is_empty() {
        outputln!("\n{}", output::heading("Feature qualification"));
        for feature in &report.feature_qualifications {
            outputln!(
                "- {}: {}{} — {}",
                feature.feature,
                feature.status.as_str(),
                if feature.required { " (required)" } else { "" },
                feature.description,
            );
            for requirement in &feature.requirements {
                outputln!(
                    "  proof {}: suite={}, claim={}",
                    requirement.id,
                    requirement.suite,
                    requirement.claim.label(),
                );
            }
            for blocker in &feature.blockers {
                outputln!("  blocker: {blocker}");
            }
        }
    }
}
