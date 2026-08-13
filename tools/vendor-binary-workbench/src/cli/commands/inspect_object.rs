//! Random-access inspection of one recovered global/data object.

use serde::Serialize;

use super::super::*;

#[derive(Serialize)]
struct ObjectInvestigationReport {
    schema_version: u32,
    command: &'static str,
    source: String,
    symbol: String,
    offset: Option<i64>,
    constants_only: bool,
    flows_to: Option<String>,
    observations: Vec<ObjectObservation>,
}

#[derive(Serialize)]
struct ObjectObservation {
    profile: String,
    report: String,
    object: crate::artifacts::StoredDataObject,
    accesses: Vec<ObjectAccessEvidence>,
}

#[derive(Serialize)]
struct ObjectAccessEvidence {
    function: String,
    site: u32,
    block: Option<usize>,
    access: String,
    width: u8,
    offset: i64,
    paths: Vec<String>,
    value: Option<String>,
    constant: Option<u32>,
    flow: Vec<crate::artifacts::StoredGraphEdge>,
    flow_calls: Vec<ObjectFlowCall>,
}

#[derive(Clone, Serialize)]
struct ObjectFlowCall {
    caller: String,
    target: String,
    site: Option<u32>,
    arguments: Vec<String>,
}

pub(super) fn run(arguments: InspectObjectArgs, project: &ProjectSpec) -> Result<bool> {
    let (source, symbol) = arguments
        .selector
        .split_once(':')
        .ok_or_else(|| crate::Error::invalid("object selector must be SOURCE:SYMBOL"))?;
    if source.is_empty() || symbol.is_empty() || symbol.contains(':') {
        return Err(crate::Error::invalid(
            "object selector must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    let offset = arguments.offset.as_deref().map(parse_offset).transpose()?;
    let mut observations = Vec::new();
    for profile in project
        .ir_profiles
        .iter()
        .filter(|profile| profile.sources.iter().any(|candidate| candidate == source))
        .filter(|profile| profile.output.is_dir())
    {
        let reader = crate::artifacts::LinkedIrReader::open(&profile.output)?;
        for object in reader.get_data_object(source, symbol)? {
            let targets = arguments
                .flows_to
                .as_deref()
                .map(|selector| reader.matching_function_identities(selector))
                .unwrap_or_default();
            let mut accesses = Vec::new();
            for xref in &object.xrefs {
                let Some(function) = reader.get_function_by_identity(&xref.function)? else {
                    continue;
                };
                let flow = if arguments.flows_to.is_some() {
                    let search = reader.shortest_path_to_any(
                        &xref.function,
                        &targets,
                        crate::artifacts::GraphSearchLimits {
                            max_depth: 12,
                            max_visited_nodes: 2_048,
                            max_examined_edges: 16_384,
                        },
                    )?;
                    let Some(path) = search.path else {
                        continue;
                    };
                    path
                } else {
                    Vec::new()
                };
                let flow_calls = flow_calls(&reader, &xref.function, &function, &flow)?;
                for effect in &function.instruction_effects {
                    let crate::artifacts::StoredInstructionEffect::Memory {
                        site,
                        block,
                        access,
                        width,
                        object: accessed_object,
                        offset: accessed_offset,
                        paths,
                        value,
                        value_pseudo,
                        ..
                    } = effect
                    else {
                        continue;
                    };
                    if !matches_object(
                        accessed_object,
                        object.member.as_deref(),
                        &object.symbol,
                        &object.aliases,
                    ) || offset.is_some_and(|expected| expected != *accessed_offset)
                    {
                        continue;
                    }
                    let rendered_value = value_pseudo.clone().or_else(|| value.clone());
                    let constant = rendered_value.as_deref().and_then(parse_exact_u32);
                    if arguments.constants
                        && constant.is_none()
                        && !flow_calls.iter().any(|call| {
                            call.arguments
                                .iter()
                                .any(|argument| exact_argument(argument).is_some())
                        })
                    {
                        continue;
                    }
                    accesses.push(ObjectAccessEvidence {
                        function: function.identity.clone(),
                        site: *site,
                        block: *block,
                        access: access.clone(),
                        width: *width,
                        offset: *accessed_offset,
                        paths: paths.clone(),
                        value: rendered_value,
                        constant,
                        flow: flow.clone(),
                        flow_calls: flow_calls.clone(),
                    });
                }
            }
            accesses.sort_by_key(|access| (access.function.clone(), access.site));
            accesses.dedup_by(|left, right| {
                left.function == right.function
                    && left.site == right.site
                    && left.access == right.access
                    && left.offset == right.offset
                    && left.value == right.value
            });
            observations.push(ObjectObservation {
                profile: profile.id.clone(),
                report: profile.output.display().to_string(),
                object,
                accesses,
            });
        }
    }
    let report = ObjectInvestigationReport {
        schema_version: 1,
        command: "inspect object",
        source: source.to_owned(),
        symbol: symbol.to_owned(),
        offset,
        constants_only: arguments.constants,
        flows_to: arguments.flows_to,
        observations,
    };
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(!report.observations.is_empty())
}

fn render_human(report: &ObjectInvestigationReport) {
    outputln!("{}", crate::cli::output::heading("Memory object"));
    outputln!("Object:       {}:{}", report.source, report.symbol);
    outputln!("Observations: {}", report.observations.len());
    if let Some(offset) = report.offset {
        outputln!("Offset:       {offset:+#x}");
    }
    if report.constants_only {
        outputln!("Values:       exact constants only");
    }
    if let Some(target) = &report.flows_to {
        outputln!("Flows to:     {target}");
    }
    for observation in &report.observations {
        let object = &observation.object;
        outputln!("\n{}", crate::cli::output::heading(&observation.profile));
        outputln!(
            "Member:  {}",
            object.member.as_deref().unwrap_or("<linked-image>")
        );
        outputln!(
            "Address: {}",
            object.address.as_deref().unwrap_or("unresolved")
        );
        outputln!("Size:    {} byte(s)", object.size);
        outputln!("Uses:    {}", object.xrefs.len());
        outputln!("Access evidence: {}", observation.accesses.len());
        if crate::cli::output::details() {
            outputln!("Report:  {}", observation.report);
        }
        if report.offset.is_none()
            && !report.constants_only
            && report.flows_to.is_none()
            && !object.xrefs.is_empty()
        {
            outputln!(
                "{}",
                crate::cli::table::render(
                    ["Function", "Reads", "Writes", "Offsets"],
                    object.xrefs.iter().take(50).map(|xref| [
                        xref.function.clone(),
                        xref.reads.to_string(),
                        xref.writes.to_string(),
                        xref.offsets.join(", "),
                    ]),
                )
            );
        }
        if !observation.accesses.is_empty() {
            outputln!("\n{}", crate::cli::output::heading("Selected accesses"));
            for (index, access) in observation.accesses.iter().take(100).enumerate() {
                outputln!(
                    "{}. {} u{} {:+#x} at {:#010x}",
                    index + 1,
                    access.access.to_ascii_uppercase(),
                    access.width,
                    access.offset,
                    access.site
                );
                outputln!("   Function: {}", access.function);
                if let Some(value) = &access.value {
                    outputln!("   Value:    {value}");
                }
                if !access.flow.is_empty() {
                    outputln!(
                        "   Route:    {}",
                        access
                            .flow
                            .iter()
                            .map(|edge| short_identity(&edge.callee))
                            .collect::<Vec<_>>()
                            .join(" → ")
                    );
                }
                for call in &access.flow_calls {
                    let arguments = compact_arguments(&call.arguments);
                    outputln!(
                        "   Call:     {}({}){}",
                        short_identity(&call.target),
                        arguments.join(", "),
                        call.site
                            .map_or_else(String::new, |site| format!(" at {site:#010x}"))
                    );
                }
            }
        }
    }
}

fn flow_calls(
    reader: &crate::artifacts::LinkedIrReader,
    root: &str,
    root_function: &crate::artifacts::StoredFunction,
    flow: &[crate::artifacts::StoredGraphEdge],
) -> Result<Vec<ObjectFlowCall>> {
    let mut calls = Vec::new();
    for edge in flow {
        let function = if edge.caller == root {
            Some(root_function)
        } else {
            None
        };
        let owned;
        let function = if let Some(function) = function {
            function
        } else {
            owned = reader.get_function_by_identity(&edge.caller)?;
            let Some(function) = owned.as_ref() else {
                continue;
            };
            function
        };
        if let Some(call) = function.calls.iter().find(|call| {
            edge.site.is_none_or(|site| call.site == Some(site))
                && (call.target == edge.callee
                    || call
                        .project_symbol()
                        .is_some_and(|target| edge.callee.ends_with(target)))
        }) {
            calls.push(ObjectFlowCall {
                caller: edge.caller.clone(),
                target: edge.callee.clone(),
                site: call.site,
                arguments: call.arguments.clone(),
            });
        }
    }
    Ok(calls)
}

fn parse_offset(value: &str) -> Result<i64> {
    let (negative, digits) = value
        .strip_prefix('-')
        .map_or((false, value), |digits| (true, digits));
    let magnitude = digits
        .strip_prefix("0x")
        .map_or_else(
            || digits.parse::<i64>(),
            |digits| i64::from_str_radix(digits, 16),
        )
        .map_err(|_| crate::Error::invalid(format!("invalid object offset {value:?}")))?;
    Ok(if negative { -magnitude } else { magnitude })
}

fn parse_exact_u32(value: &str) -> Option<u32> {
    let value = value.trim().trim_matches(['(', ')']);
    value.strip_prefix("0x").map_or_else(
        || value.parse().ok(),
        |digits| u32::from_str_radix(digits, 16).ok(),
    )
}

fn exact_argument(value: &str) -> Option<u32> {
    parse_exact_u32(value.strip_prefix("const:").unwrap_or(value))
}

fn compact_arguments(arguments: &[String]) -> Vec<String> {
    arguments
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            if let Some(value) = exact_argument(value) {
                return Some(format!("a{index}={value:#x}"));
            }
            value.contains("g_ic").then(|| format!("a{index}={value}"))
        })
        .collect()
}

fn short_identity(identity: &str) -> &str {
    identity.rsplit("::").next().unwrap_or(identity)
}

fn matches_object(
    object: &crate::artifacts::StoredMemoryObject,
    member: Option<&str>,
    symbol: &str,
    aliases: &[String],
) -> bool {
    match object {
        crate::artifacts::StoredMemoryObject::Global {
            member: candidate_member,
            symbol: candidate_symbol,
        } => {
            member.is_none_or(|member| candidate_member.as_deref() == Some(member))
                && (candidate_symbol == symbol
                    || aliases.iter().any(|alias| alias == candidate_symbol))
        }
        crate::artifacts::StoredMemoryObject::Indexed { object, .. }
        | crate::artifacts::StoredMemoryObject::Dereferenced {
            pointer: object, ..
        } => matches_object(object, member, symbol, aliases),
        _ => false,
    }
}
