//! Vendor-to-Rust replacement report rendering.

use super::{ReplacementInvestigationReport, VendorEffectEvidence};
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
                let traces = proof
                    .execution_cases
                    .iter()
                    .filter_map(|case| case.trace.as_ref().map(|trace| (&case.name, trace)))
                    .collect::<Vec<_>>();
                let visible_traces = if report.requested_case.is_some() || output::details() {
                    traces.len()
                } else {
                    usize::from(!traces.is_empty())
                };
                for (name, trace) in traces.into_iter().take(visible_traces) {
                    render_matched_trace(&proof.suite, name, trace);
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

    if !report.vendor_effects.is_empty() {
        outputln!("\n{}", output::heading("Ordered vendor effects"));
        outputln!(
            "Direct static effects ordered by instruction PC; proof cases above decide execution equivalence."
        );
        let mut profiles = std::collections::BTreeMap::<&str, Vec<_>>::new();
        for effect in &report.vendor_effects {
            profiles.entry(&effect.profile).or_default().push(effect);
        }
        for (profile, effects) in profiles {
            outputln!("\nProfile: {profile}");
            const HEAD_EFFECTS: usize = 8;
            const TAIL_EFFECTS: usize = 8;
            if output::details() || effects.len() <= HEAD_EFFECTS + TAIL_EFFECTS {
                for (index, evidence) in effects.iter().enumerate() {
                    render_vendor_effect(index, evidence);
                }
            } else {
                for (index, evidence) in effects.iter().take(HEAD_EFFECTS).enumerate() {
                    render_vendor_effect(index, evidence);
                }
                let omitted = effects.len() - HEAD_EFFECTS - TAIL_EFFECTS;
                outputln!(
                    "… {omitted} middle effect(s) omitted; use --details for the complete ordered list."
                );
                for (index, evidence) in effects
                    .iter()
                    .enumerate()
                    .skip(effects.len() - TAIL_EFFECTS)
                {
                    render_vendor_effect(index, evidence);
                }
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
}

fn render_vendor_effect(index: usize, evidence: &VendorEffectEvidence) {
    outputln!(
        "{}. {:#010x}{} {}{} {} {}{}",
        index + 1,
        evidence.address,
        evidence
            .block
            .map(|block| format!(" bb{block}"))
            .unwrap_or_default(),
        evidence.access,
        evidence.width,
        evidence.kind,
        compact_targets(&evidence.targets),
        evidence
            .value
            .as_deref()
            .map(|value| format!(" ← {}", compact_value(value, output::details())))
            .unwrap_or_default(),
    );
    if output::details() && evidence.targets.len() > 2 {
        outputln!("   targets: {}", evidence.targets.join(", "));
    }
    if !evidence.guards.is_empty() {
        outputln!("   when: {}", evidence.guards.join(" || "));
    }
}

fn render_matched_trace(suite: &str, case: &str, trace: &crate::verification::MatchedTraceReport) {
    outputln!(
        "\n{}",
        output::heading(format!("Matched trace — {suite}/{case}"))
    );
    if trace.events.is_empty() {
        outputln!("No observable MMIO, delay or fence events.");
    } else {
        for item in &trace.events {
            outputln!("{}. {}", item.index, event_text(&item.event));
            outputln!(
                "   vendor: {}",
                producer_text(item.vendor_producer.as_ref())
            );
            outputln!("   Rust:   {}", producer_text(item.rust_producer.as_ref()));
        }
    }
    if !trace.memory_changes.is_empty() {
        outputln!("RAM transitions:");
        for change in &trace.memory_changes {
            outputln!(
                "  {:#010x}: {:#04x} → {:#04x}",
                change.address,
                change.before,
                change.after
            );
        }
    }
    if let Some(value) = trace.return_value {
        outputln!("Compared return: {value:#010x}");
    }
}

fn event_text(event: &crate::verification::ExecutionEventReport) -> String {
    use crate::verification::ExecutionEventReport;
    match event {
        ExecutionEventReport::Read {
            width,
            address,
            region,
            register,
            value,
        } => format!(
            "READ/{width} {region}/{} ({address:#010x}) → {value:#010x}",
            register.as_deref().unwrap_or("unknown")
        ),
        ExecutionEventReport::Write {
            width,
            address,
            region,
            register,
            value,
        } => format!(
            "WRITE/{width} {region}/{} ({address:#010x}) ← {value:#010x}",
            register.as_deref().unwrap_or("unknown")
        ),
        ExecutionEventReport::DelayMicros { micros } => format!("DELAY {micros} us"),
        ExecutionEventReport::Fence {
            fm,
            predecessor,
            successor,
        } => format!("FENCE fm={fm:#x} pred={predecessor:#x} succ={successor:#x}"),
    }
}

fn producer_text(producer: Option<&crate::verification::EventProducerReport>) -> String {
    let Some(producer) = producer else {
        return "producer metadata unavailable".to_owned();
    };
    match (&producer.symbol, producer.symbol_offset) {
        (Some(symbol), Some(offset)) => format!("{symbol}+{offset:#x} @ {:#010x}", producer.pc),
        (Some(symbol), None) => format!("{symbol} @ {:#010x}", producer.pc),
        (None, _) => format!("{:#010x}", producer.pc),
    }
}

fn compact_targets(targets: &[String]) -> String {
    match targets {
        [] => "unknown target".to_owned(),
        [target] => target.clone(),
        [first, second] => format!("{first}, {second}"),
        [first, .., last] => format!("{first} … {last} ({} targets)", targets.len()),
    }
}

fn compact_value(value: &str, details: bool) -> String {
    const MAX_INLINE_CHARS: usize = 96;
    if details || value.chars().count() <= MAX_INLINE_CHARS {
        return value.to_owned();
    }
    if value.starts_with("symbolic(\"bits:") {
        return format!(
            "symbolic bit projection ({} assignments; use --details)",
            value.matches('=').count()
        );
    }
    let prefix = value.chars().take(MAX_INLINE_CHARS).collect::<String>();
    format!("{prefix}… (use --details)")
}

#[cfg(test)]
mod tests {
    use super::{compact_targets, compact_value};

    #[test]
    fn indexed_target_domains_are_bounded_in_human_output() {
        let targets = (0..5)
            .map(|index| format!("TIMER{index}"))
            .collect::<Vec<_>>();

        assert_eq!(compact_targets(&targets), "TIMER0 … TIMER4 (5 targets)");
    }

    #[test]
    fn long_symbolic_values_are_summarized_without_losing_machine_evidence() {
        let value = format!(
            "symbolic(\"bits:{}\")",
            (0..32)
                .map(|bit| format!("{bit}=ramread0.{bit}"))
                .collect::<Vec<_>>()
                .join(",")
        );

        assert_eq!(
            compact_value(&value, false),
            "symbolic bit projection (32 assignments; use --details)"
        );
        assert_eq!(compact_value(&value, true), value);
    }
}
