//! Compact human and typed machine rendering for bounded flow investigation.

use super::super::*;
use crate::flow_investigation::{
    EffectFlowRequest, EventRouteFlowRequest, FlowEffectKind, FlowInvestigationReport,
    FlowInvestigationRequest, FlowStatus, FlowTargetRequest, PublicationFlowRequest,
    PublicationSelectorRequest, TargetFlowRequest, investigate,
};

pub(super) fn run(arguments: InspectFlowArgs, project: &ProjectSpec) -> Result<bool> {
    let target_count = usize::from(arguments.to_function.is_some())
        + usize::from(arguments.to_register.is_some())
        + usize::from(arguments.to_address.is_some());
    let request = match (
        arguments.selector.as_deref(),
        arguments.effects,
        arguments.publication.as_deref(),
        arguments.event_route.as_deref(),
        target_count,
    ) {
        (None, None, None, Some(route), 0) => {
            FlowInvestigationRequest::EventRoute(EventRouteFlowRequest {
                route,
                max_depth: arguments.max_depth,
            })
        }
        (Some(selector), Some(kind), None, None, 0) => {
            let (source, root_symbol) = parse_selector(selector)?;
            FlowInvestigationRequest::Effects(EffectFlowRequest {
                source,
                root_symbol,
                kind: effect_kind(kind),
                max_depth: arguments.max_depth,
            })
        }
        (Some(selector), None, Some(publication), None, 0) => {
            let (source, root_symbol) = parse_selector(selector)?;
            FlowInvestigationRequest::Publication(PublicationFlowRequest {
                source,
                root_symbol,
                selector: parse_publication_selector(publication)?,
                max_depth: arguments.max_depth,
            })
        }
        (Some(selector), None, None, None, 1) => {
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
                max_loaded_functions: crate::flow_investigation::MAX_LOADED_FUNCTIONS,
            })
        }
        _ => {
            return Err(crate::Error::invalid(
                "inspect flow accepts exactly one mode: SOURCE:SYMBOL with one --to-*, --effects, or --publication; or --event-route without SOURCE:SYMBOL",
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
    if source.is_empty() || root_symbol.is_empty() {
        return Err(crate::Error::invalid(
            "flow root must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    if root_symbol.contains(':') {
        let semantic = root_symbol
            .parse::<open_radio_vendor_contracts::SemanticEntityId>()
            .map_err(|error| {
                crate::Error::invalid(format!(
                    "flow root after SOURCE is neither a raw symbol nor a canonical semantic identity: {error}"
                ))
            })?;
        if semantic.domain() != open_radio_vendor_contracts::EntityDomain::Function {
            return Err(crate::Error::invalid(format!(
                "flow root requires a function semantic identity, got {semantic}"
            )));
        }
    }
    Ok((source, root_symbol))
}

fn parse_publication_selector(value: &str) -> Result<PublicationSelectorRequest<'_>> {
    let (kind, target) = value.split_once(':').ok_or_else(|| {
        crate::Error::invalid(
            "publication selector must be operation:NAME, call:TARGET, register:NAME, or address:ADDRESS",
        )
    })?;
    if target.is_empty() || target.trim() != target {
        return Err(crate::Error::invalid(
            "publication selector target must be non-empty and contain no surrounding whitespace",
        ));
    }
    match kind {
        "operation" => Ok(PublicationSelectorRequest::Operation(target)),
        "call" => Ok(PublicationSelectorRequest::Call(target)),
        "register" => Ok(PublicationSelectorRequest::Register(target)),
        "address" => Ok(PublicationSelectorRequest::Address(parse_address(target)?)),
        _ => Err(crate::Error::invalid(
            "publication selector kind must be exactly operation, call, register, or address",
        )),
    }
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

    if !report.publications.is_empty() {
        outputln!("\n{}", crate::cli::output::heading("Publication boundary"));
        for (index, publication) in report.publications.iter().enumerate() {
            outputln!(
                "{}. {:?} {} {} @ {} in {} · selector-exact={} · site-authenticated={}",
                index + 1,
                publication.evidence,
                publication.kind,
                publication.target,
                publication
                    .site
                    .map(|site| format!("{site:#010x}"))
                    .unwrap_or_else(|| "—".to_owned()),
                compact_identity(&publication.function),
                yes_no(publication.selector_exact),
                yes_no(publication.site_authenticated),
            );
            outputln!("   {}", publication_resolution(publication));
        }
    }

    if !report.memory_slice.is_empty() {
        outputln!(
            "\n{}",
            crate::cli::output::heading("Memory reaching publication")
        );
        for (index, slice) in report.memory_slice.iter().enumerate() {
            if index != 0 {
                outputln!("");
            }
            outputln!(
                "{}. {} @ {:#010x} · {}{:+#x}:u{} · aliases={} · definitions={} · publication={} · entry={}",
                index + 1,
                compact_identity(&slice.publication_function),
                slice.publication_site,
                slice.object,
                slice.offset,
                slice.width,
                if slice.alias_complete {
                    "complete"
                } else {
                    "incomplete"
                },
                if slice.definition_set_complete {
                    "complete"
                } else {
                    "incomplete"
                },
                if slice.publication_exact {
                    "exact"
                } else {
                    "candidate"
                },
                if slice.incoming_definition_possible {
                    "incoming-definition-possible"
                } else {
                    "local-definition-on-all-structural-paths"
                }
            );
            if slice.definitions.is_empty() {
                outputln!("   No reaching definitions retained.");
                continue;
            }
            for definition in &slice.definitions {
                outputln!(
                    "   - {} · {:?} write {:#010x} in {}",
                    memory_definition_class(definition.classification),
                    definition.evidence,
                    definition.site,
                    compact_identity(&definition.function),
                );
                outputln!(
                    "     memory={}{:+#x}:u{} · value={} · value-complete={}",
                    definition.object,
                    definition.offset,
                    definition.width,
                    render_memory_values(&definition.values),
                    yes_no(definition.value_complete),
                );
                outputln!("     cfg={}", memory_witness(definition.witness.as_ref()));
            }
        }
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

fn publication_resolution(
    publication: &crate::flow_investigation::FlowPublicationEvidence,
) -> String {
    let mut values = Vec::new();
    if let Some(operation) = &publication.operation {
        values.push(format!("operation={operation}"));
    }
    if !publication.registers.is_empty() {
        values.push(format!("registers={}", publication.registers.join("|")));
    }
    if let Some(address) = publication.address {
        values.push(format!("address={address:#010x}"));
    }
    if !publication.modes.is_empty() {
        values.push(format!("modes={}", publication.modes.join("|")));
    }
    if !publication.paths.is_empty() {
        values.push(format!("paths={}", publication.paths.join("|")));
    }
    if !publication.guards.is_empty() {
        values.push(format!("guards={}", publication.guards.join("|")));
    }
    if !publication.persisted_blocks.is_empty() {
        values.push(format!(
            "persisted-blocks={:?}",
            publication.persisted_blocks
        ));
    }
    values.push(format!(
        "persisted-block-metadata-complete={}",
        yes_no(publication.persisted_block_complete)
    ));
    if !publication.tails.is_empty() {
        values.push(format!("tails={:?}", publication.tails));
    }
    if values.is_empty() {
        "—".to_owned()
    } else {
        values.join(", ")
    }
}

const fn memory_definition_class(
    classification: crate::flow_investigation::FlowMemoryDefinitionClassification,
) -> &'static str {
    match classification {
        crate::flow_investigation::FlowMemoryDefinitionClassification::Must => "must-last-write",
        crate::flow_investigation::FlowMemoryDefinitionClassification::Alternative => {
            "alternative-last-write"
        }
        crate::flow_investigation::FlowMemoryDefinitionClassification::Candidate => {
            "candidate-last-write"
        }
    }
}

fn memory_witness(witness: Option<&crate::flow_investigation::FlowCfgPathWitness>) -> String {
    witness.map_or_else(
        || "unavailable".to_owned(),
        |witness| {
            format!(
                "{} blocks={:?} path-feasibility={}",
                witness.proof,
                witness.blocks,
                yes_no(witness.path_feasibility_claim)
            )
        },
    )
}

fn render_memory_values(values: &[String]) -> String {
    if values.is_empty() {
        "—".to_owned()
    } else {
        values.join("|")
    }
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
        PublicationSelectorRequest,
    };

    use super::{
        argument_location, exact_values, parse_publication_selector, parse_selector,
        render_memory_values,
    };

    #[test]
    fn root_selector_accepts_raw_and_reviewed_function_identities() {
        assert_eq!(
            parse_selector("ble:raw_symbol").unwrap(),
            ("ble", "raw_symbol")
        );
        assert_eq!(
            parse_selector("ble:function:esp-idf/ble/controller/start").unwrap(),
            ("ble", "function:esp-idf/ble/controller/start")
        );
        assert!(parse_selector("ble:memory-object:esp-idf/ble/state").is_err());
        assert!(parse_selector("ble:function:").is_err());
    }

    #[test]
    fn publication_selector_parses_each_exact_kind() {
        assert!(matches!(
            parse_publication_selector("operation:radio.publish").unwrap(),
            PublicationSelectorRequest::Operation("radio.publish")
        ));
        assert!(matches!(
            parse_publication_selector("call:ble::worker").unwrap(),
            PublicationSelectorRequest::Call("ble::worker")
        ));
        assert!(matches!(
            parse_publication_selector("register:MODEM.HEAD").unwrap(),
            PublicationSelectorRequest::Register("MODEM.HEAD")
        ));
        assert!(matches!(
            parse_publication_selector("address:0x10").unwrap(),
            PublicationSelectorRequest::Address(0x10)
        ));
        assert!(matches!(
            parse_publication_selector("address:16").unwrap(),
            PublicationSelectorRequest::Address(16)
        ));
    }

    #[test]
    fn publication_selector_rejects_fuzzy_or_malformed_kinds() {
        for selector in [
            "publish:radio.publish",
            "Operation:radio.publish",
            "operation",
            "operation:",
            "operation: radio.publish",
            "operation:radio.publish ",
            "address:not-an-address",
        ] {
            assert!(
                parse_publication_selector(selector).is_err(),
                "accepted malformed publication selector {selector:?}"
            );
        }
    }

    #[test]
    fn memory_values_preserve_same_site_alternatives() {
        assert_eq!(render_memory_values(&[]), "—");
        assert_eq!(
            render_memory_values(&["arg0".to_owned(), "0x1".to_owned()]),
            "arg0|0x1"
        );
    }

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
            context_evidence: EvidenceLevel::Observed,
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
