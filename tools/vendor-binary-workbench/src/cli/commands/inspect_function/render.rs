//! Human rendering for complete and compact function investigations.

use std::collections::BTreeMap;

use crate::{cli::output, function_investigation::FunctionInvestigationReport};

pub(super) fn human(report: &FunctionInvestigationReport, full: bool) {
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
