//! Lossless, project-aware function investigation command.

use std::collections::BTreeMap;

use serde::Serialize;

use super::super::*;
use crate::cli::output;
use crate::function_investigation::{
    CallKnowledgeEvidence, FunctionInvestigationReport, FunctionInvestigationRequest,
    ReplacementEvidence, ReviewedEffectRuleEvidence, investigate, replacement_evidence,
    reviewed_effect_rules,
};

#[derive(Serialize)]
struct CallsiteInvestigationReport<'a> {
    schema_version: u32,
    command: &'static str,
    source: &'a str,
    symbol: &'a str,
    filter: Option<&'a str>,
    calls: Vec<ProfiledCallsite<'a>>,
}

#[derive(Serialize)]
struct ProfiledCallsite<'a> {
    profile: &'a str,
    #[serde(flatten)]
    call: &'a CallKnowledgeEvidence,
}

#[derive(Serialize)]
struct ReplacementInvestigationReport {
    schema_version: u32,
    command: &'static str,
    source: String,
    symbol: String,
    replacements: Vec<ReplacementEvidence>,
    reviewed_effects: Vec<ReviewedEffectRuleEvidence>,
    feature_qualifications: Vec<crate::qualification::FunctionQualificationEvidence>,
}

pub(super) fn run(arguments: InspectFunctionArgs, project: &ProjectSpec) -> Result<bool> {
    let full = arguments.full;
    let focused_calls = arguments.calls || arguments.call.is_some();
    let call_filter = arguments.call.clone();
    let (source, symbol) = arguments
        .selector
        .split_once(':')
        .ok_or_else(|| crate::Error::invalid("function selector must be SOURCE:SYMBOL"))?;
    if source.is_empty() || symbol.is_empty() || symbol.contains(':') {
        return Err(crate::Error::invalid(
            "function selector must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    let (symbol, runtime_address) = parse_exact_symbol(symbol)?;
    if arguments.replacement {
        let report = ReplacementInvestigationReport {
            schema_version: 2,
            command: "inspect function replacement",
            source: source.to_owned(),
            symbol: symbol.to_owned(),
            replacements: replacement_evidence(source, &symbol, project)?,
            reviewed_effects: reviewed_effect_rules(source, &symbol, project)?,
            feature_qualifications: crate::qualification::evidence_for_function(
                project, source, &symbol,
            )?,
        };
        let found = !report.replacements.is_empty();
        crate::cli::output::render_report(&report, || render_replacement(&report));
        return Ok(found);
    }
    let artifact = arguments.artifact.as_deref().ok_or_else(|| {
        crate::Error::invalid(format!(
            "run spec does not define source-artifact:{source}; pass --artifact"
        ))
    })?;
    let report = investigate(
        FunctionInvestigationRequest {
            source,
            symbol,
            runtime_address,
            artifact,
            inventory: arguments.inventory.as_deref(),
            member: arguments.member.as_deref(),
            origin_member: arguments.origin_member.as_deref(),
            graph_depth: arguments.depth,
            include_callers: arguments.callers,
            cfg_path: arguments.path.as_deref(),
        },
        project,
    )?;
    if focused_calls {
        let callsites = callsite_report(&report, call_filter.as_deref());
        crate::cli::output::render_report(&callsites, || {
            render_calls(&report, call_filter.as_deref());
        });
    } else {
        crate::cli::output::render_report(&report, || {
            render_human(&report, full);
        });
    }
    Ok(report.runtime.accounted_bytes == report.runtime.size)
}

fn render_replacement(report: &ReplacementInvestigationReport) {
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

fn call_matches(call: &CallKnowledgeEvidence, filter: Option<&str>) -> bool {
    filter.is_none_or(|filter| {
        call.target.contains(filter)
            || call.kind.contains(filter)
            || call
                .semantic_operation
                .as_deref()
                .is_some_and(|operation| operation.contains(filter))
            || call
                .provenance
                .iter()
                .any(|evidence| evidence.contains(filter))
    })
}

fn callsite_report<'a>(
    report: &'a FunctionInvestigationReport,
    filter: Option<&'a str>,
) -> CallsiteInvestigationReport<'a> {
    CallsiteInvestigationReport {
        schema_version: 1,
        command: "inspect function calls",
        source: &report.source,
        symbol: &report.symbol,
        filter,
        calls: report
            .semantics
            .iter()
            .flat_map(|semantic| {
                semantic
                    .calls
                    .iter()
                    .filter(move |call| call_matches(call, filter))
                    .map(move |call| ProfiledCallsite {
                        profile: &semantic.profile,
                        call,
                    })
            })
            .collect(),
    }
}

fn render_calls(report: &FunctionInvestigationReport, filter: Option<&str>) {
    outputln!("{}", output::heading("Callsite investigation"));
    outputln!("Function: {}:{}", report.source, report.symbol);
    if let Some(filter) = filter {
        outputln!("Filter:   {filter}");
    }

    let calls = report
        .semantics
        .iter()
        .flat_map(|semantic| {
            semantic.calls.iter().filter_map(move |call| {
                call_matches(call, filter).then_some((semantic.profile.as_str(), call))
            })
        })
        .collect::<Vec<_>>();
    if calls.is_empty() {
        outputln!(
            "\n{}",
            output::warning("No matching callsites were recovered.")
        );
        outputln!(
            "Use --full to inspect the lossless body when semantic execution stops before a call."
        );
        return;
    }

    if filter.is_none() || calls.len() > 1 {
        if output::human_width() < 120 {
            outputln!("\nCalls:");
            for (_, call) in &calls {
                outputln!(
                    "  {}  target={}  arguments={}",
                    call.site
                        .map(|site| format!("{site:#010x}"))
                        .unwrap_or_else(|| "composed".to_owned()),
                    call.target_status,
                    argument_summary(call).replace('\n', ", ")
                );
                outputln!("    {}", call.target);
                if let Some(operation) = &call.semantic_operation {
                    outputln!("    → {operation}");
                }
            }
        } else {
            outputln!(
                "\n{}",
                crate::cli::table::render(
                    ["Site", "Target", "Target proof", "Arguments"],
                    calls.iter().map(|(_, call)| [
                        call.site
                            .map(|site| format!("{site:#010x}"))
                            .unwrap_or_else(|| "composed".to_owned()),
                        call.semantic_operation.as_ref().map_or_else(
                            || call.target.clone(),
                            |operation| { format!("{}\n→ {operation}", call.target) }
                        ),
                        call.target_status.to_owned(),
                        argument_summary(call),
                    ]),
                )
            );
        }
    }

    if filter.is_none() && !output::details() {
        outputln!(
            "\nUse --call TARGET to inspect one boundary, or --details to expand every callsite."
        );
        return;
    }

    for (profile, call) in calls {
        outputln!(
            "\n{}",
            output::heading(format!(
                "{} @ {}",
                call.target,
                call.site
                    .map(|site| format!("{site:#010x}"))
                    .unwrap_or_else(|| "composed call".to_owned())
            ))
        );
        outputln!("Profile: {profile}");
        outputln!("Target:  {} ({})", call.target, call.target_status);
        if !call.target_candidates.is_empty() {
            outputln!("Candidates: {}", call.target_candidates.join(", "));
        }
        if let Some(blocker) = &call.target_blocker {
            outputln!("Target blocker: {blocker}");
        }
        if call.argument_evidence.is_empty() {
            outputln!(
                "Arguments: {}",
                if call.argument_shapes == 0 {
                    "not recovered"
                } else {
                    "ABI has no arguments"
                }
            );
        } else {
            outputln!("Arguments:");
            for argument in &call.argument_evidence {
                outputln!(
                    "  a{} = {}  [{}]",
                    argument.position,
                    argument.value,
                    argument.status
                );
                if output::details() {
                    outputln!("       {}", argument.provenance);
                }
            }
        }
        if !call.guards.is_empty() {
            outputln!("Paths: {} guarded expression(s)", call.guards.len());
            if output::details() {
                for (index, guard) in call.guards.iter().enumerate() {
                    let abbreviated = abbreviate(guard, 240);
                    outputln!("  {}. {abbreviated}", index + 1);
                }
                if call.guards.iter().any(|guard| guard.chars().count() > 240) {
                    outputln!("  Human view abbreviated; --format json preserves exact guards.");
                }
            }
        }
        if !call.provenance.is_empty() {
            outputln!("Evidence:");
            for evidence in &call.provenance {
                outputln!("  - {evidence}");
            }
        }
    }
}

fn argument_summary(call: &crate::function_investigation::CallKnowledgeEvidence) -> String {
    if call.argument_evidence.is_empty() {
        return if call.argument_shapes == 0 {
            "not recovered".to_owned()
        } else {
            "none".to_owned()
        };
    }
    let exact = call
        .argument_evidence
        .iter()
        .filter(|argument| argument.status == "exact")
        .count();
    let partial = call
        .argument_evidence
        .iter()
        .filter(|argument| argument.status == "partial")
        .count();
    let unresolved = call.argument_evidence.len() - exact - partial;
    if call.semantic_operation.is_none() && call.execution_model.is_none() {
        let mut parts = Vec::new();
        if exact != 0 {
            parts.push(format!("{exact} exact"));
        }
        if partial != 0 {
            parts.push(format!("{partial} partial"));
        }
        parts.push("arity unknown".to_owned());
        if call.argument_shapes > 1 {
            parts.push(format!("{} paths", call.argument_shapes));
        }
        return parts.join("\n");
    }
    let mut parts = vec![format!("{exact}/{} exact", call.argument_evidence.len())];
    if partial != 0 {
        parts.push(format!("{partial} partial"));
    }
    if unresolved != 0 {
        parts.push(format!("{unresolved} unresolved"));
    }
    if call.argument_shapes > 1 {
        parts.push(format!("{} paths", call.argument_shapes));
    }
    parts.join("\n")
}

fn abbreviate(value: &str, maximum_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(maximum_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn parse_exact_symbol(input: &str) -> Result<(&str, Option<u64>)> {
    let Some((symbol, address)) = input.rsplit_once("@0x") else {
        return Ok((input, None));
    };
    if symbol.is_empty() || address.is_empty() {
        return Err(crate::Error::invalid(
            "exact function identity must be SYMBOL@0xADDRESS",
        ));
    }
    let address = u64::from_str_radix(address, 16).map_err(|_| {
        crate::Error::invalid(format!("invalid linked function address in {input:?}"))
    })?;
    Ok((symbol, Some(address)))
}

fn render_human(report: &FunctionInvestigationReport, full: bool) {
    if !full {
        render_summary(report);
        return;
    }
    crate::cli::output::line(format_args!(
        "FUNCTION {}:{}  runtime={}{}  bytes={}/{}",
        report.source,
        report.symbol,
        report.runtime.artifact,
        report
            .runtime
            .member
            .as_deref()
            .map(|member| format!("({member})"))
            .unwrap_or_default(),
        report.runtime.accounted_bytes,
        report.runtime.size
    ));
    crate::cli::output::line(format_args!("\nPROOF LEDGER"));
    for entry in &report.proof_ledger {
        crate::cli::output::line(format_args!(
            "  {:<12} {:<20} {}",
            entry.layer, entry.status, entry.detail
        ));
    }
    if !report.reviewed_preconditions.is_empty() || !report.reviewed_paths.is_empty() {
        crate::cli::output::line(format_args!(
            "\nREVIEWED PATH KNOWLEDGE (assumptions, not execution proof)"
        ));
        for precondition in &report.reviewed_preconditions {
            crate::cli::output::line(format_args!(
                "  precondition {}: {} — {}",
                precondition.id, precondition.expression, precondition.rationale
            ));
        }
        for path in &report.reviewed_paths {
            crate::cli::output::line(format_args!(
                "  path {} [{}]: {} — {}",
                path.id, path.class, path.summary, path.evidence
            ));
        }
    }
    if !report.replacements.is_empty() {
        crate::cli::output::line(format_args!("\nVENDOR ↔ RUST REPLACEMENT"));
        for replacement in &report.replacements {
            crate::cli::output::line(format_args!(
                "  {}:{} status={} reviewed={} association={}",
                replacement.vendor_source,
                replacement.vendor_symbol,
                replacement.status,
                replacement.reviewed,
                replacement.association,
            ));
            crate::cli::output::line(format_args!(
                "    production={} probes={} proofs={} report={}{}",
                replacement
                    .production_component
                    .as_deref()
                    .unwrap_or("none"),
                replacement.verification_probes.join(", "),
                replacement.proofs.len(),
                replacement.report,
                if replacement.freshness_claim {
                    ""
                } else {
                    " (stored evidence; freshness not claimed)"
                }
            ));
        }
    }
    if let Some(path) = &report.cfg_path {
        crate::cli::output::line(format_args!("\nSTRUCTURAL CFG PATH"));
        crate::cli::output::line(format_args!(
            "  {:#010x} (bb{}) -> {:#010x} (bb{}): {}",
            path.from_address,
            path.from_block,
            path.to_address,
            path.to_block,
            if path.structurally_reachable {
                "reachable"
            } else {
                "no directed path"
            }
        ));
        crate::cli::output::line(format_args!(
            "  feasibility: not claimed; branch conditions and runtime state are not solved"
        ));
        if path.structurally_reachable {
            crate::cli::output::line(format_args!(
                "  blocks: {}",
                path.blocks
                    .iter()
                    .map(|block| format!("bb{block}"))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ));
            for block_id in &path.blocks {
                let block = report
                    .runtime
                    .basic_blocks
                    .iter()
                    .find(|block| block.id == *block_id)
                    .expect("reported CFG path refers to an existing block");
                crate::cli::output::line(format_args!(
                    "  bb{} +{:#06x}..+{:#06x}",
                    block.id, block.start_offset, block.end_offset
                ));
                for instruction in report.runtime.instructions.iter().filter(|instruction| {
                    instruction.offset >= block.start_offset
                        && instruction.offset < block.end_offset
                }) {
                    crate::cli::output::line(format_args!(
                        "    {:#010x}  {:<10} {}",
                        instruction.address, instruction.raw, instruction.text
                    ));
                }
            }
        }
    }
    if full {
        crate::cli::output::line(format_args!("\nCFG"));
        for block in &report.runtime.basic_blocks {
            let successors = block
                .successors
                .iter()
                .map(|successor| match successor.block {
                    Some(target) => format!("{} -> bb{target}", successor.kind),
                    None => format!(
                        "{} -> {}",
                        successor.kind,
                        successor
                            .target
                            .map(|target| format!("{target:#x}"))
                            .unwrap_or_else(|| "unknown".to_owned())
                    ),
                })
                .collect::<Vec<_>>()
                .join(", ");
            crate::cli::output::line(format_args!(
                "  bb{}  +{:#06x}..+{:#06x}  {}  [{}]",
                block.id,
                block.start_offset,
                block.end_offset,
                if block.reachable {
                    "reachable"
                } else {
                    "unreachable"
                },
                successors
            ));
        }
        crate::cli::output::line(format_args!("\nINSTRUCTIONS"));
        for instruction in &report.runtime.instructions {
            for label in report
                .runtime
                .labels
                .iter()
                .filter(|label| label.offset == instruction.offset)
            {
                crate::cli::output::line(format_args!("{}:", label.name));
            }
            crate::cli::output::line(format_args!(
                "  +{:#06x}  {:#010x}  {:<10} {:<28} {}",
                instruction.offset,
                instruction.address,
                instruction.raw,
                instruction.text,
                instruction.control_flow.kind.label()
            ));
            if let Some(class) = &instruction.blocker_class {
                crate::cli::output::line(format_args!("              ! decode blocker: {class}"));
            }
            for semantic in &report.semantics {
                if let Some(evidence) = semantic
                    .instruction_evidence
                    .iter()
                    .find(|evidence| evidence.address == instruction.address)
                {
                    for effect in &evidence.effects {
                        crate::cli::output::line(format_args!(
                            "              = {} {}{} {}{}",
                            effect.kind,
                            effect.access,
                            effect.width,
                            effect.target,
                            effect
                                .value
                                .as_deref()
                                .map(|value| format!(" value={value}"))
                                .unwrap_or_default(),
                        ));
                        if !effect.guards.is_empty() {
                            crate::cli::output::line(format_args!(
                                "                guards: {}",
                                effect.guards.join("; ")
                            ));
                        }
                    }
                    if !evidence.call_targets.is_empty() {
                        crate::cli::output::line(format_args!(
                            "              = calls {}",
                            evidence.call_targets.join(", ")
                        ));
                    }
                    if !evidence.semantic_operations.is_empty() {
                        crate::cli::output::line(format_args!(
                            "              = semantics {}",
                            evidence.semantic_operations.join(", ")
                        ));
                    }
                    for blocker in &evidence.blocker_ids {
                        crate::cli::output::line(format_args!("              ! {blocker}"));
                    }
                }
            }
            for relocation in &instruction.relocations {
                crate::cli::output::line(format_args!(
                    "              @ {} {} {:+}",
                    relocation.kind, relocation.symbol, relocation.addend
                ));
            }
        }
    }
    for semantic in &report.semantics {
        crate::cli::output::line(format_args!(
            "\nSEMANTICS profile={} complete={} exact={} report={}",
            semantic.profile, semantic.complete, semantic.exact, semantic.report
        ));
        if !semantic.pseudo.is_empty() {
            crate::cli::output::text(format!("{}\n", semantic.pseudo));
        }
        if !semantic.calls.is_empty() {
            crate::cli::output::line(format_args!("CALL BOUNDARIES"));
            for call in &semantic.calls {
                crate::cli::output::line(format_args!(
                    "  - {} {}{} knowledge={}{}{}",
                    call.kind,
                    call.target,
                    call.site
                        .map(|site| format!(" @ {site:#010x}"))
                        .unwrap_or_default(),
                    call.knowledge,
                    call.semantic_operation
                        .as_deref()
                        .map(|operation| format!(" operation={operation}"))
                        .unwrap_or_default(),
                    call.execution_model
                        .as_deref()
                        .map(|model| format!(" model={model}"))
                        .unwrap_or_default(),
                ));
                if !call.arguments.is_empty() {
                    crate::cli::output::line(format_args!(
                        "      args[{} shape{}]: {}",
                        call.argument_shapes,
                        if call.argument_shapes == 1 { "" } else { "s" },
                        call.arguments
                            .iter()
                            .enumerate()
                            .map(|(position, value)| format!("a{position}={value}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if !call.guards.is_empty() {
                    crate::cli::output::line(format_args!(
                        "      when: {}",
                        call.guards.join(" || ")
                    ));
                }
            }
        }
        if !semantic.call_graph_edges.is_empty() {
            crate::cli::output::line(format_args!(
                "CALL GRAPH depth={} reachable={} edges={} visited={} examined={}{}",
                semantic.graph_limits.max_depth,
                semantic.reachable_functions.len(),
                semantic.call_graph_edges.len(),
                semantic.graph_limits.visited_nodes,
                semantic.graph_limits.examined_edges,
                semantic
                    .graph_limits
                    .reached
                    .as_deref()
                    .map(|limit| format!(" boundary={limit}"))
                    .unwrap_or_default(),
            ));
            for edge in semantic.call_graph_edges.iter().take(100) {
                crate::cli::output::line(format_args!(
                    "  - {} --{}{}--> {}",
                    edge.caller,
                    edge.kind,
                    edge.site
                        .map(|site| format!("@{site:#010x}"))
                        .unwrap_or_default(),
                    edge.callee,
                ));
            }
            if semantic.call_graph_edges.len() > 100 {
                crate::cli::output::line(format_args!(
                    "  ... {} additional edges are available in JSON output",
                    semantic.call_graph_edges.len() - 100
                ));
            }
        }
        if !semantic.event_dispatches.is_empty() {
            crate::cli::output::line(format_args!("EVENT DISPATCHES"));
            for dispatch in &semantic.event_dispatches {
                let bindings = dispatch
                    .bindings
                    .iter()
                    .map(|binding| format!("{}={}", binding.role, binding.value))
                    .collect::<Vec<_>>()
                    .join(", ");
                crate::cli::output::line(format_args!(
                    "  - {} context={} receiver={} interface={} [{}]",
                    dispatch.mechanism,
                    dispatch.execution_context,
                    dispatch.receiver.as_deref().unwrap_or("unknown"),
                    if dispatch.interface_complete {
                        "complete"
                    } else {
                        "incomplete"
                    },
                    bindings,
                ));
                for blocker in &dispatch.blockers {
                    crate::cli::output::line(format_args!("      ! {blocker}"));
                }
            }
        }
        if !semantic.reviewed_event_routes.is_empty() {
            crate::cli::output::line(format_args!("REVIEWED EVENT ROUTES"));
            for route in &semantic.reviewed_event_routes {
                crate::cli::output::line(format_args!(
                    "  - {}: {} {}={:#010x} -> {} via {} [{}]",
                    route.id,
                    route.mechanism,
                    route.selector_role,
                    route.selector_value,
                    route
                        .case_handler
                        .as_deref()
                        .unwrap_or("unmapped case handler"),
                    route.consumer_entry,
                    route.execution_context,
                ));
                crate::cli::output::line(format_args!("      {}", route.rationale));
                crate::cli::output::line(format_args!(
                    "      constraint={} consumer-analysis={} case-analysis={}",
                    if route.dispatch_constraint_matched {
                        "matched"
                    } else {
                        "blocked"
                    },
                    if route.consumer_analysis.is_some() {
                        "available"
                    } else {
                        "unavailable"
                    },
                    if route.case_handler_analysis.is_some() {
                        "available"
                    } else {
                        "unavailable"
                    }
                ));
                if let Some(handler) = &route.consumer_analysis {
                    crate::cli::output::line(format_args!(
                        "      consumer complete={} exact={} direct-effects={} calls={} reachable={} depth={}{}",
                        handler.complete,
                        handler.exact,
                        handler.direct_instruction_effects,
                        handler.direct_calls,
                        handler.reachable_functions,
                        handler.reachability_depth,
                        handler
                            .reachability_limit
                            .as_deref()
                            .map(|limit| format!(" boundary={limit}"))
                            .unwrap_or_default(),
                    ));
                    for blocker in &handler.blockers {
                        crate::cli::output::line(format_args!("        ! {blocker}"));
                    }
                }
                if let Some(handler) = &route.case_handler_analysis {
                    crate::cli::output::line(format_args!(
                        "      case-handler complete={} exact={} direct-effects={} calls={} reachable={} depth={}{}",
                        handler.complete,
                        handler.exact,
                        handler.direct_instruction_effects,
                        handler.direct_calls,
                        handler.reachable_functions,
                        handler.reachability_depth,
                        handler
                            .reachability_limit
                            .as_deref()
                            .map(|limit| format!(" boundary={limit}"))
                            .unwrap_or_default(),
                    ));
                    for blocker in &handler.blockers {
                        crate::cli::output::line(format_args!("        ! {blocker}"));
                    }
                }
                for blocker in &route.blockers {
                    crate::cli::output::line(format_args!("      ! {blocker}"));
                }
            }
        }
        if !semantic.blockers.is_empty() {
            crate::cli::output::line(format_args!("BLOCKERS"));
            for blocker in &semantic.blockers {
                crate::cli::output::line(format_args!(
                    "  - {} [{}]{}: {}",
                    blocker.root_id,
                    blocker.kind,
                    blocker
                        .site
                        .map(|site| format!(" @ {site:#010x}"))
                        .unwrap_or_default(),
                    blocker.message
                ));
                crate::cli::output::line(format_args!("      needs: {}", blocker.required_model));
                if !blocker.relocation_candidates.is_empty() {
                    crate::cli::output::line(format_args!(
                        "      relocation candidates: {}",
                        blocker.relocation_candidates.join(", ")
                    ));
                }
            }
        }
    }
    if let Some(origin) = &report.origin {
        crate::cli::output::line(format_args!(
            "\nORIGIN {}{} bytes={}/{} association={}",
            origin.body.artifact,
            origin
                .body
                .member
                .as_deref()
                .map(|member| format!("({member})"))
                .unwrap_or_default(),
            origin.body.accounted_bytes,
            origin.body.size,
            origin.association
        ));
        if !origin.relocation_dependencies.is_empty() {
            crate::cli::output::line(format_args!("ORIGIN RELOCATION DEPENDENCIES"));
            for dependency in &origin.relocation_dependencies {
                crate::cli::output::line(format_args!(
                    "  - {} refs={} offsets={} kinds={}",
                    dependency.symbol,
                    dependency.references,
                    dependency
                        .instruction_offsets
                        .iter()
                        .map(|offset| format!("+{offset:#06x}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    dependency.kinds.join(","),
                ));
            }
            crate::cli::output::line(format_args!(
                "  note: raw archive offsets are never projected by arithmetic; see bounded structural correspondence below"
            ));
        }
        if !origin.instruction_correspondence.is_empty() {
            crate::cli::output::line(format_args!("ORIGIN ↔ LINKED CORRESPONDENCE"));
            for correspondence in &origin.instruction_correspondence {
                crate::cli::output::line(format_args!(
                    "  - origin {} -> linked +{:#06x} ({:#010x}) kind={} symbols={}",
                    correspondence
                        .origin_offsets
                        .iter()
                        .map(|offset| format!("+{offset:#06x}"))
                        .collect::<Vec<_>>()
                        .join(","),
                    correspondence.runtime_offset,
                    correspondence.runtime_address,
                    correspondence.kind,
                    correspondence.relocation_symbols.join(","),
                ));
            }
            crate::cli::output::line(format_args!(
                "  note: structural navigation evidence only; semantic equivalence is not claimed"
            ));
        }
        if full {
            crate::cli::output::line(format_args!("\nORIGIN INSTRUCTIONS"));
            for instruction in &origin.body.instructions {
                for label in origin
                    .body
                    .labels
                    .iter()
                    .filter(|label| label.offset == instruction.offset)
                {
                    crate::cli::output::line(format_args!("{}:", label.name));
                }
                crate::cli::output::line(format_args!(
                    "  +{:#06x}  {:<10} {:<28} {}",
                    instruction.offset,
                    instruction.raw,
                    instruction.text,
                    instruction.control_flow.kind.label()
                ));
                if let Some(class) = &instruction.blocker_class {
                    crate::cli::output::line(format_args!(
                        "              ! decode blocker: {class}"
                    ));
                }
                for relocation in &instruction.relocations {
                    crate::cli::output::line(format_args!(
                        "              @ {} {} {:+}",
                        relocation.kind, relocation.symbol, relocation.addend
                    ));
                }
            }
        }
    }
    if !full {
        crate::cli::output::line(format_args!(
            "\nUse --full for the complete CFG and lossless instruction listing."
        ));
    }
}

fn render_summary(report: &FunctionInvestigationReport) {
    outputln!("{}", output::heading("Function investigation"));
    outputln!("Function: {}:{}", report.source, report.symbol);
    outputln!("Artifact: {}", report.runtime.artifact);
    if let Some(member) = &report.runtime.member {
        outputln!("Member:   {member}");
    }
    outputln!(
        "Body:     {} byte(s), {} instruction(s), {} basic block(s)",
        report.runtime.size,
        report.runtime.instructions.len(),
        report.runtime.basic_blocks.len()
    );

    let complete_body = report.runtime.accounted_bytes == report.runtime.size;
    let complete_semantics =
        !report.semantics.is_empty() && report.semantics.iter().all(|semantic| semantic.complete);
    let blocker_count = report
        .semantics
        .iter()
        .map(|semantic| semantic.blockers.len())
        .sum::<usize>();
    let outcome = if complete_body && complete_semantics && blocker_count == 0 {
        output::success("COMPLETE — binary body and semantic analysis are complete")
    } else if complete_body && report.semantics.is_empty() {
        output::warning("PARTIAL — lossless body retained; semantic analysis is unavailable")
    } else if complete_body {
        output::warning(format!(
            "PARTIAL — lossless body retained; {blocker_count} semantic blocker(s)"
        ))
    } else {
        output::failure(format!(
            "INCOMPLETE — decoded {}/{} body bytes",
            report.runtime.accounted_bytes, report.runtime.size
        ))
    };
    outputln!("\n{outcome}");

    outputln!("\n{}", output::heading("Proof ledger"));
    outputln!(
        "{}",
        crate::cli::table::render(
            ["Layer", "Status", "Meaning"],
            report.proof_ledger.iter().map(|entry| [
                entry.layer.to_owned(),
                entry.status.to_owned(),
                entry.detail.clone(),
            ]),
        )
    );

    for semantic in &report.semantics {
        outputln!(
            "\n{}",
            output::heading(format!("Recovered pseudo-Rust — {}", semantic.profile))
        );
        if semantic.pseudo.is_empty() {
            outputln!("No structured pseudo-Rust is available for this profile.");
        } else {
            // The persistent pseudo document deliberately carries a complete
            // blocker/call preamble. The focused human view renders those as
            // structured sections below, so repeating them here only turns
            // the useful code into an objdump-like wall of text.
            let all_lines = semantic.pseudo.lines().collect::<Vec<_>>();
            let start = all_lines
                .iter()
                .position(|line| line.starts_with("fn "))
                .unwrap_or(0);
            let lines = &all_lines[start..];
            let limit = if output::details() { lines.len() } else { 80 };
            for line in lines.iter().take(limit) {
                outputln!("{line}");
            }
            if lines.len() > limit {
                outputln!(
                    "… {} more line(s); use --details or --full.",
                    lines.len() - limit
                );
            }
        }

        if !semantic.calls.is_empty() {
            outputln!("\n{}", output::heading("Calls and relationships"));
            outputln!(
                "{}",
                crate::cli::table::render(
                    ["Kind", "Target / meaning"],
                    semantic.calls.iter().take(30).map(|call| {
                        let meaning = call
                            .semantic_operation
                            .as_deref()
                            .or(call.execution_model.as_deref())
                            .unwrap_or(call.knowledge);
                        [
                            call.kind.to_owned(),
                            format!("{}\n→ {meaning}", call.target),
                        ]
                    }),
                )
            );
            if semantic.calls.len() > 30 {
                outputln!(
                    "{} more call(s); use --details or JSON.",
                    semantic.calls.len() - 30
                );
            }
        }

        if !semantic.reviewed_event_routes.is_empty() {
            outputln!("\n{}", output::heading("Reviewed event routes"));
            for route in &semantic.reviewed_event_routes {
                outputln!(
                    "- {}: {} {}={:#x} → {} via {} ({})",
                    route.id,
                    route.mechanism,
                    route.selector_role,
                    route.selector_value,
                    route
                        .case_handler
                        .as_deref()
                        .unwrap_or("unmapped case handler"),
                    route.consumer_entry,
                    route.execution_context
                );
            }
        }
    }

    if !report.replacements.is_empty() {
        outputln!("\n{}", output::heading("Vendor ↔ Rust replacement"));
        outputln!(
            "{}",
            crate::cli::table::render(
                ["Vendor", "Status", "Rust component"],
                report.replacements.iter().map(|replacement| [
                    format!(
                        "{}:{}",
                        replacement.vendor_source, replacement.vendor_symbol
                    ),
                    replacement.status.clone(),
                    replacement
                        .production_component
                        .clone()
                        .unwrap_or_else(|| "not assigned".to_owned()),
                ]),
            )
        );
        if output::details() {
            let proofs = report
                .replacements
                .iter()
                .map(|replacement| replacement.proofs.len())
                .sum::<usize>();
            outputln!("Reviewed proof records: {proofs}");
        }
    }

    let blockers = report
        .semantics
        .iter()
        .flat_map(|semantic| &semantic.blockers)
        .collect::<Vec<_>>();
    if !blockers.is_empty() {
        outputln!("\n{}", output::heading("Problems"));
        let mut grouped = BTreeMap::<(&str, &str), Vec<_>>::new();
        for blocker in &blockers {
            grouped
                .entry((&blocker.kind, &blocker.required_model))
                .or_default()
                .push(*blocker);
        }
        let mut grouped = grouped.into_iter().collect::<Vec<_>>();
        grouped.sort_by_key(|((kind, _), _)| blocker_priority(kind));
        for (index, ((kind, required_model), occurrences)) in grouped.iter().enumerate() {
            outputln!(
                "{}. {} — {} occurrence(s)",
                index + 1,
                kind,
                occurrences.len()
            );
            outputln!("   First: {}", occurrences[0].message);
            outputln!("   Needs: {required_model}");
            if output::details() {
                for blocker in occurrences.iter().skip(1) {
                    outputln!("   - {}", blocker.message);
                }
            }
        }
    }

    outputln!("\n{}", output::heading("Next"));
    outputln!("1. add --full for the lossless instruction/CFG view");
    if !blockers.is_empty() {
        outputln!("2. review the named model requirements, then rerun project analyze");
    } else {
        outputln!("2. use inspect flow to follow arguments and constants across callees");
    }
}

fn blocker_priority(kind: &str) -> u8 {
    match kind {
        "memory-load" | "memory-store" => 0,
        "indirect-control-flow" | "call-shape" | "unresolved-call" => 1,
        "control-flow" | "poll-model" => 2,
        "call-boundary" | "call-result-model" => 3,
        "analysis-budget" | "aggregate" => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_exact_symbol;

    #[test]
    fn exact_identity_keeps_the_symbol_and_selects_the_linked_address() {
        assert_eq!(
            parse_exact_symbol("ppTask@0x10067fa0").unwrap(),
            ("ppTask", Some(0x1006_7fa0))
        );
        assert_eq!(parse_exact_symbol("ppTask").unwrap(), ("ppTask", None));
        assert!(parse_exact_symbol("ppTask@0xnot-hex").is_err());
    }
}
