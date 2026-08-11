//! Human and machine rendering for target-directed value-flow inspection.

use super::super::*;
use crate::flow_investigation::{
    FlowInvestigationReport, FlowInvestigationRequest, FlowTargetRequest, investigate,
};

pub(super) fn run(arguments: InspectFlowArgs, project: &ProjectSpec) -> Result<bool> {
    let (source, root_symbol) = arguments
        .selector
        .split_once(':')
        .ok_or_else(|| crate::Error::invalid("flow root must be SOURCE:SYMBOL"))?;
    if source.is_empty() || root_symbol.is_empty() || root_symbol.contains(':') {
        return Err(crate::Error::invalid(
            "flow root must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    let selected = usize::from(arguments.to_function.is_some())
        + usize::from(arguments.to_register.is_some())
        + usize::from(arguments.to_address.is_some());
    if selected != 1 {
        return Err(crate::Error::invalid(
            "inspect flow requires exactly one of --to-function, --to-register or --to-address",
        ));
    }
    let target = if let Some(target) = arguments.to_function.as_deref() {
        FlowTargetRequest::Function(target)
    } else if let Some(target) = arguments.to_register.as_deref() {
        FlowTargetRequest::Register(target)
    } else {
        FlowTargetRequest::Address(parse_address(
            arguments
                .to_address
                .as_deref()
                .expect("one target was validated"),
        )?)
    };
    let report = investigate(
        FlowInvestigationRequest {
            source,
            root_symbol,
            target,
            max_depth: arguments.max_depth,
        },
        project,
    )?;
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(report.reached)
}

fn parse_address(value: &str) -> Result<u32> {
    value
        .strip_prefix("0x")
        .map_or_else(|| value.parse(), |value| u32::from_str_radix(value, 16))
        .map_err(|_| crate::Error::invalid(format!("invalid MMIO address {value:?}")))
}

fn render_human(report: &FlowInvestigationReport) {
    crate::cli::output::line(format_args!(
        "FLOW {} -> {} {}  profile={}  status={}",
        report.root,
        report.target_kind,
        report.target,
        report.profile,
        if report.complete {
            "complete"
        } else {
            "review-required"
        }
    ));
    for edge in &report.edges {
        crate::cli::output::line(format_args!(
            "\n{}. {} --{}{}{}--> {}",
            edge.ordinal + 1,
            edge.caller,
            edge.kind,
            if edge.tail { "/tail" } else { "" },
            edge.site
                .map(|site| format!("@{site:#010x}"))
                .unwrap_or_default(),
            edge.callee,
        ));
        for argument in &edge.arguments {
            crate::cli::output::line(format_args!(
                "     a{}: {:<34} => {:<18} [{}]",
                argument.position, argument.local, argument.resolved, argument.provenance
            ));
            for pointee in &argument.pointee {
                crate::cli::output::line(format_args!(
                    "         -> [{:+#x}] {} bits: {:<24} => {} [{}]",
                    pointee.offset,
                    pointee.width,
                    pointee.local,
                    pointee.resolved,
                    pointee.provenance,
                ));
            }
        }
        if !edge.guards.is_empty() {
            crate::cli::output::line(format_args!("     when: {}", edge.guards.join(" || ")));
        }
    }
    if !report.sink_effects.is_empty() {
        crate::cli::output::line(format_args!("\nSINK EFFECTS"));
        for effect in &report.sink_effects {
            crate::cli::output::line(format_args!(
                "  {}: {}{} {} ({:#010x}){}",
                effect.function,
                effect.access,
                effect.width,
                effect.register,
                effect.address,
                effect
                    .value
                    .as_deref()
                    .map(|value| format!(" value={value}"))
                    .unwrap_or_default(),
            ));
        }
    }
    if !report.blockers.is_empty() {
        crate::cli::output::line(format_args!("\nCOMPOSITION BLOCKERS"));
        for blocker in &report.blockers {
            crate::cli::output::line(format_args!("  - {blocker}"));
        }
    }
    crate::cli::output::line(format_args!("\nlinked IR: {}", report.linked_ir));
}
