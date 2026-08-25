//! Compact human and typed machine rendering for bounded flow investigation.

use super::super::*;
use crate::flow_investigation::{
    EffectFlowRequest, EventRouteFlowRequest, FlowEffectKind, FlowInvestigationReport,
    FlowInvestigationRequest, FlowStatus, FlowTargetRequest, TargetFlowRequest, investigate,
};

pub(super) fn run(arguments: InspectFlowArgs, project: &ProjectSpec) -> Result<bool> {
    let target_count = usize::from(arguments.to_function.is_some())
        + usize::from(arguments.to_register.is_some())
        + usize::from(arguments.to_address.is_some());
    let request = match (
        arguments.selector.as_deref(),
        arguments.effects,
        arguments.event_route.as_deref(),
        target_count,
    ) {
        (None, None, Some(route), 0) => {
            FlowInvestigationRequest::EventRoute(EventRouteFlowRequest {
                route,
                max_depth: arguments.max_depth,
            })
        }
        (Some(selector), Some(kind), None, 0) => {
            let (source, root_symbol) = parse_selector(selector)?;
            FlowInvestigationRequest::Effects(EffectFlowRequest {
                source,
                root_symbol,
                kind: effect_kind(kind),
                max_depth: arguments.max_depth,
            })
        }
        (Some(selector), None, None, 1) => {
            let (source, root_symbol) = parse_selector(selector)?;
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
            FlowInvestigationRequest::Target(TargetFlowRequest {
                source,
                root_symbol,
                target,
                max_depth: arguments.max_depth,
            })
        }
        _ => {
            return Err(crate::Error::invalid(
                "inspect flow accepts exactly one mode: SOURCE:SYMBOL with one --to-*, SOURCE:SYMBOL with --effects, or --event-route without SOURCE:SYMBOL",
            ));
        }
    };
    let report = investigate(request, project)?;
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(report.reached())
}

fn parse_selector(value: &str) -> Result<(&str, &str)> {
    let (source, root_symbol) = value
        .split_once(':')
        .ok_or_else(|| crate::Error::invalid("flow root must be SOURCE:SYMBOL"))?;
    if source.is_empty() || root_symbol.is_empty() || root_symbol.contains(':') {
        return Err(crate::Error::invalid(
            "flow root must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    Ok((source, root_symbol))
}

fn effect_kind(value: InspectFlowEffectKind) -> FlowEffectKind {
    match value {
        InspectFlowEffectKind::Delay => FlowEffectKind::Delay,
        InspectFlowEffectKind::Timer => FlowEffectKind::Timer,
        InspectFlowEffectKind::Event => FlowEffectKind::Event,
        InspectFlowEffectKind::Call => FlowEffectKind::Call,
        InspectFlowEffectKind::Queue => FlowEffectKind::Queue,
        InspectFlowEffectKind::Mmio => FlowEffectKind::Mmio,
        InspectFlowEffectKind::Memory => FlowEffectKind::Memory,
        InspectFlowEffectKind::All => FlowEffectKind::All,
    }
}

fn parse_address(value: &str) -> Result<u32> {
    value
        .strip_prefix("0x")
        .map_or_else(|| value.parse(), |value| u32::from_str_radix(value, 16))
        .map_err(|_| crate::Error::invalid(format!("invalid MMIO address {value:?}")))
}

fn render_human(report: &FlowInvestigationReport) {
    let status = match report.status {
        FlowStatus::Complete => crate::cli::output::success("COMPLETE"),
        FlowStatus::Incomplete => crate::cli::output::warning("INCOMPLETE"),
        FlowStatus::NotReached => crate::cli::output::failure("NOT REACHED"),
    };
    outputln!(
        "{}  {}",
        crate::cli::output::heading(format!("Flow · {}", report.mode)),
        status
    );
    outputln!("Root:    {}", report.root);
    if let (Some(kind), Some(target)) = (&report.target_kind, &report.target) {
        outputln!("Target:  {kind} {target}");
    }
    if let Some(route) = &report.route {
        outputln!("Route:   {route}");
    }
    outputln!("Profile: {}", report.profile);

    if !report.steps.is_empty() {
        outputln!("\n{}", crate::cli::output::heading("Evidence path"));
        outputln!(
            "{}",
            crate::cli::table::render(
                ["#", "Level", "Site", "Transition", "Exact values"],
                report.steps.iter().map(|step| [
                    (step.ordinal + 1).to_string(),
                    format!("{:?}", step.evidence).to_lowercase(),
                    step.site
                        .map(|site| format!("{site:#010x}"))
                        .unwrap_or_else(|| "—".to_owned()),
                    format!(
                        "{} → {}",
                        compact_identity(&step.caller),
                        compact_identity(&step.callee)
                    ),
                    exact_values(step),
                ])
            )
        );
        if crate::cli::output::details() {
            for step in &report.steps {
                if !step.guards.is_empty() {
                    outputln!(
                        "  #{} guards: {}",
                        step.ordinal + 1,
                        step.guards.join(" || ")
                    );
                }
                for argument in &step.arguments {
                    outputln!(
                        "  #{} {} {} => {} [{}]",
                        step.ordinal + 1,
                        argument_location(argument.position),
                        argument.local,
                        argument.resolved,
                        argument.provenance
                    );
                    for pointee in &argument.pointee {
                        outputln!(
                            "       {}{:+#x}:u{} {} => {} [{}]",
                            argument_location(argument.position),
                            pointee.offset,
                            pointee.width,
                            pointee.local,
                            pointee.resolved,
                            pointee.provenance
                        );
                    }
                }
            }
        }
    }

    if !report.effects.is_empty() {
        outputln!("\n{}", crate::cli::output::heading("Effects"));
        outputln!(
            "{}",
            crate::cli::table::render(
                ["Kind", "Level", "Site", "Function", "Evidence"],
                report.effects.iter().map(|effect| [
                    effect.kind.clone(),
                    format!("{:?}", effect.evidence).to_lowercase(),
                    effect
                        .site
                        .map(|site| format!("{site:#010x}"))
                        .unwrap_or_else(|| "—".to_owned()),
                    compact_identity(&effect.function).to_owned(),
                    effect.operation.as_ref().map_or_else(
                        || effect.detail.clone(),
                        |operation| { format!("{operation}: {}", effect.detail) }
                    ),
                ])
            )
        );
    }

    if !report.rust_boundaries.is_empty() {
        outputln!(
            "\n{}",
            crate::cli::output::heading("Vendor → Rust boundary")
        );
        for (index, boundary) in report.rust_boundaries.iter().enumerate() {
            if index != 0 {
                outputln!("");
            }
            outputln!(
                "{}. {}:{}",
                index + 1,
                boundary.vendor_source,
                boundary.vendor_symbol
            );
            outputln!(
                "   Mapping:    {} / {}",
                if boundary.reviewed {
                    "reviewed"
                } else {
                    "generated"
                },
                boundary.association
            );
            outputln!(
                "   Production: {}",
                boundary
                    .production_component
                    .as_deref()
                    .unwrap_or("unassigned")
            );
            outputln!(
                "   Verification: {}",
                if boundary.report_complete_project_run
                    && boundary.report_passed
                    && boundary.freshness_claim
                {
                    "verified and fresh"
                } else {
                    "not established by this route"
                }
            );
        }
    }

    if !report.blockers.is_empty() {
        outputln!("\n{}", crate::cli::output::heading("Blockers"));
        for (index, blocker) in report.blockers.iter().enumerate() {
            outputln!("{}. {}: {}", index + 1, blocker.kind, blocker.message);
            outputln!("   Next: {}", blocker.next_step.instruction);
            for command in &blocker.next_step.commands {
                outputln!("   Run:  {}", command.render_posix());
            }
        }
    }
    outputln!(
        "\nLimits: visited={} edges={} loaded={}{}",
        report.limits.visited_nodes,
        report.limits.examined_edges,
        report.limits.loaded_functions,
        report
            .limits
            .reached
            .as_deref()
            .map(|limit| format!(" reached={limit}"))
            .unwrap_or_default()
    );
    outputln!(
        "Claims: navigation={} path-feasibility={} event-delivery={} executable-equivalence={}",
        yes_no(report.claims.structural_navigation),
        yes_no(report.claims.path_feasibility),
        yes_no(report.claims.event_delivery),
        yes_no(report.claims.executable_equivalence)
    );
    if crate::cli::output::details() {
        outputln!("Linked IR: {}", report.linked_ir);
    }
}

fn compact_identity(identity: &str) -> &str {
    identity.rsplit("::").next().unwrap_or(identity)
}

const fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn argument_location(position: usize) -> String {
    if position < 8 {
        format!("a{position}")
    } else {
        format!("stack[{}]", position - 8)
    }
}

fn exact_values(step: &crate::flow_investigation::FlowStepEvidence) -> String {
    let mut values = Vec::new();
    for argument in &step.arguments {
        let location = argument_location(argument.position);
        if !argument.constants.is_empty() {
            values.push(format!(
                "{location}={}",
                render_constants(&argument.constants)
            ));
        }
        for pointee in &argument.pointee {
            if pointee.constants.is_empty() {
                continue;
            }
            values.push(format!(
                "{location}{:+#x}:u{}={}",
                pointee.offset,
                pointee.width,
                render_constants(&pointee.constants)
            ));
        }
    }
    if values.is_empty() {
        "—".to_owned()
    } else {
        values.join(", ")
    }
}

fn render_constants(values: &[u32]) -> String {
    values
        .iter()
        .map(|value| format!("{value:#x}"))
        .collect::<Vec<_>>()
        .join("|")
}

#[cfg(test)]
mod tests {
    use crate::flow_investigation::{
        EvidenceLevel, FlowArgumentEvidence, FlowPointeeEvidence, FlowStepEvidence,
    };

    use super::{argument_location, exact_values};

    #[test]
    fn rv32_argument_locations_distinguish_registers_from_stack_arguments() {
        assert_eq!(argument_location(0), "a0");
        assert_eq!(argument_location(7), "a7");
        assert_eq!(argument_location(8), "stack[0]");
        assert_eq!(argument_location(15), "stack[7]");
    }

    #[test]
    fn exact_values_summarize_direct_and_pointee_constants() {
        let step = FlowStepEvidence {
            ordinal: 0,
            evidence: EvidenceLevel::Observed,
            context: "synchronous".to_owned(),
            caller: "source::caller".to_owned(),
            callee: "source::callee".to_owned(),
            site: Some(0x1000),
            kind: "direct".to_owned(),
            tail: false,
            argument_shapes: 1,
            arguments: vec![FlowArgumentEvidence {
                position: 3,
                local: "private-stack:+0x20".to_owned(),
                resolved: "private-stack:+0x20".to_owned(),
                constants: Vec::new(),
                provenance: "uncomposed-symbolic-expression",
                pointee: vec![FlowPointeeEvidence {
                    offset: 0,
                    width: 8,
                    local: "arg1".to_owned(),
                    resolved: "0x00000001".to_owned(),
                    constants: vec![1],
                    provenance: "exact-constant-domain",
                }],
            }],
            guards: Vec::new(),
            origin: "fixture".to_owned(),
        };

        assert_eq!(exact_values(&step), "a3+0x0:u8=0x1");
    }
}
