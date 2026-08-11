//! Lossless, project-aware function investigation command.

use super::super::*;
use crate::function_investigation::{
    FunctionInvestigationReport, FunctionInvestigationRequest, investigate,
};

pub(super) fn run(arguments: InspectFunctionArgs, project: &ProjectSpec) -> Result<bool> {
    let full = arguments.full;
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
    crate::cli::output::render_report(&report, || render_human(&report, full));
    Ok(report.runtime.accounted_bytes == report.runtime.size)
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
                replacement.proofs.as_array().map_or(0, Vec::len),
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
                "CALL GRAPH reachable={} edges={}",
                semantic.reachable_functions.len(),
                semantic.call_graph_edges.len(),
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
                    "  - {}: {} {}={:#010x} -> {} [{}]",
                    route.id,
                    route.mechanism,
                    route.selector_role,
                    route.selector_value,
                    route.handler,
                    route.execution_context,
                ));
                crate::cli::output::line(format_args!("      {}", route.rationale));
                crate::cli::output::line(format_args!(
                    "      constraint={} handler-analysis={}",
                    if route.dispatch_constraint_matched {
                        "matched"
                    } else {
                        "blocked"
                    },
                    if route.handler_analysis.is_some() {
                        "available"
                    } else {
                        "unavailable"
                    }
                ));
                if let Some(handler) = &route.handler_analysis {
                    crate::cli::output::line(format_args!(
                        "      handler complete={} exact={} direct-effects={} calls={} reachable={}",
                        handler.complete,
                        handler.exact,
                        handler.direct_instruction_effects,
                        handler.direct_calls,
                        handler.reachable_functions,
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
