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
    association: crate::artifacts::DataObjectAssociation,
    function: String,
    site: u32,
    block: Option<usize>,
    access: String,
    width: u8,
    offset: i64,
    observed_offset: i64,
    object: crate::artifacts::StoredMemoryObject,
    data_address: open_radio_vendor_contracts::DataAddressResolution,
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
    if source.is_empty() || symbol.is_empty() {
        return Err(crate::Error::invalid(
            "object selector must contain one non-empty SOURCE and SYMBOL",
        ));
    }
    validate_object_selector(symbol)?;
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
            let mut visited_functions = std::collections::BTreeSet::new();
            for xref in &object.xrefs {
                if !visited_functions.insert(&xref.function) {
                    continue;
                }
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
                        data_address,
                        paths,
                        value,
                        value_pseudo,
                        ..
                    } = effect
                    else {
                        continue;
                    };
                    let candidate = data_address
                        .candidates()
                        .iter()
                        .find(|candidate| candidate.identity == object.data_identity);
                    let association = candidate
                        .map(|_| crate::artifacts::DataObjectAssociation::AddressRangeCandidate)
                        .or_else(|| {
                            object_association(
                                accessed_object,
                                &object.data_identity,
                                object.member.as_deref(),
                                &object.symbol,
                                &object.aliases,
                            )
                        });
                    let Some(association) = association else {
                        continue;
                    };
                    let object_offset =
                        candidate.map_or(*accessed_offset, |candidate| candidate.offset);
                    if offset.is_some_and(|expected| expected != object_offset) {
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
                        association,
                        function: function.identity.clone(),
                        site: *site,
                        block: *block,
                        access: access.clone(),
                        width: *width,
                        offset: object_offset,
                        observed_offset: *accessed_offset,
                        object: accessed_object.clone(),
                        data_address: data_address.clone(),
                        paths: paths.clone(),
                        value: rendered_value,
                        constant,
                        flow: flow.clone(),
                        flow_calls: flow_calls.clone(),
                    });
                }
            }
            accesses.sort_by_key(|access| (access.function.clone(), access.site));
            observations.push(ObjectObservation {
                profile: profile.id.clone(),
                report: profile.output.display().to_string(),
                object,
                accesses,
            });
        }
    }
    let report = ObjectInvestigationReport {
        schema_version: 3,
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

fn validate_object_selector(symbol: &str) -> Result<()> {
    if !symbol.contains(':') {
        return Ok(());
    }
    if symbol.starts_with("occurrence:") {
        let occurrence = symbol
            .parse::<open_radio_vendor_contracts::RevisionOccurrenceId>()
            .map_err(|error| crate::Error::invalid(error.to_string()))?;
        if occurrence.domain() != open_radio_vendor_contracts::EntityDomain::MemoryObject {
            return Err(crate::Error::invalid(
                "inspect object requires a memory-object occurrence",
            ));
        }
        return Ok(());
    }
    let semantic = symbol
        .parse::<open_radio_vendor_contracts::SemanticEntityId>()
        .map_err(|error| {
            crate::Error::invalid(format!(
                "object selector after SOURCE is neither a raw symbol nor a canonical semantic identity: {error}"
            ))
        })?;
    if semantic.domain() != open_radio_vendor_contracts::EntityDomain::MemoryObject {
        return Err(crate::Error::invalid(format!(
            "inspect object requires a memory-object semantic identity, got {semantic}"
        )));
    }
    Ok(())
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
        outputln!("Occurrence: {}", object.occurrence);
        outputln!("Data identity: {}", object.data_identity);
        outputln!(
            "Member:  {}",
            object.member.as_deref().unwrap_or("<linked-image>")
        );
        if let Some(semantic) = &object.semantic {
            outputln!("Semantic: {semantic}");
            outputln!("Raw:      {}:{}", object.source, object.symbol);
        }
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
                    [
                        "Function",
                        "Association",
                        "Evidence",
                        "Reads",
                        "Writes",
                        "Offsets"
                    ],
                    object.xrefs.iter().take(50).map(|xref| [
                        xref.function.clone(),
                        format!("{:?}", xref.association),
                        format!("{:?}", xref.evidence),
                        xref.reads.to_string(),
                        xref.writes.to_string(),
                        xref.offsets.join(", "),
                    ]),
                )
            );
        }
        if !observation.accesses.is_empty() {
            let writes = observation
                .accesses
                .iter()
                .filter(|access| access.access == "write")
                .collect::<Vec<_>>();
            let reads = observation
                .accesses
                .iter()
                .filter(|access| access.access == "read")
                .collect::<Vec<_>>();
            let other = observation
                .accesses
                .iter()
                .filter(|access| access.access != "write" && access.access != "read")
                .collect::<Vec<_>>();
            let constants = observation
                .accesses
                .iter()
                .filter_map(|access| access.constant)
                .collect::<std::collections::BTreeSet<_>>();

            outputln!("\n{}", crate::cli::output::heading("Access summary"));
            outputln!("Writes:       {}", writes.len());
            outputln!("Reads:        {}", reads.len());
            if !other.is_empty() {
                outputln!("Other:        {}", other.len());
            }
            if !constants.is_empty() {
                outputln!(
                    "Exact values: {}",
                    constants
                        .iter()
                        .map(|value| format!("{value:#010x}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            render_access_group("Writes", &writes);
            render_access_group("Reads", &reads);
            render_access_group("Other accesses", &other);
        }
    }
}

fn render_access_group(heading: &str, accesses: &[&ObjectAccessEvidence]) {
    if accesses.is_empty() {
        return;
    }
    outputln!("\n{}", crate::cli::output::heading(heading));
    let limit = if crate::cli::output::details() {
        100
    } else {
        20
    };
    for (index, access) in accesses.iter().take(limit).enumerate() {
        outputln!(
            "{}. {} u{} {:+#x} at {:#010x}",
            index + 1,
            access.access.to_ascii_uppercase(),
            access.width,
            access.offset,
            access.site
        );
        outputln!("   Function: {}", access.function);
        outputln!("   Association: {:?}", access.association);
        if !access.data_address.candidates().is_empty() {
            outputln!("   Address evidence: {:?}", access.data_address);
            outputln!(
                "   Observed object: {:?}, offset {:+#x}",
                access.object,
                access.observed_offset
            );
        }
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
    if accesses.len() > limit {
        outputln!(
            "… {} more; pass -v to show up to 100 entries",
            accesses.len() - limit
        );
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

fn object_association(
    object: &crate::artifacts::StoredMemoryObject,
    identity: &crate::artifact::DataIdentity,
    member: Option<&str>,
    symbol: &str,
    aliases: &[String],
) -> Option<crate::artifacts::DataObjectAssociation> {
    match object {
        crate::artifacts::StoredMemoryObject::Global {
            reference,
            member: candidate_member,
            symbol: candidate_symbol,
        } => crate::artifacts::DataObjectAssociation::for_reference(
            reference,
            identity,
            member.is_none_or(|member| candidate_member.as_deref() == Some(member))
                && (candidate_symbol == symbol
                    || aliases.iter().any(|alias| alias == candidate_symbol)),
        ),
        crate::artifacts::StoredMemoryObject::Indexed { object, .. } => {
            object_association(object, identity, member, symbol, aliases)
        }
        // Dereferencing a pointer stored in an object accesses its pointee, not
        // the pointer's storage. The pointer load has its own memory evidence.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::validate_object_selector;

    #[test]
    fn object_association_does_not_treat_pointee_as_pointer_storage() {
        use crate::artifacts::StoredMemoryObject;
        let identity = crate::artifact::DataIdentity::Synthetic {
            namespace: module_path!().to_owned(),
            key: "pointer-cell".to_owned(),
        };
        let pointer = StoredMemoryObject::Global {
            reference: open_radio_vendor_contracts::SymbolReference::Unknown {
                reason: "fixture".to_owned(),
            },
            member: None,
            symbol: "pointer".to_owned(),
        };
        assert!(super::object_association(&pointer, &identity, None, "pointer", &[]).is_some());
        let pointee = StoredMemoryObject::Dereferenced {
            pointer: Box::new(pointer),
            pointer_offset: 0,
        };
        assert!(super::object_association(&pointee, &identity, None, "pointer", &[]).is_none());
    }

    #[test]
    fn object_selector_accepts_only_memory_object_identities() {
        assert!(validate_object_selector("raw_symbol").is_ok());
        assert!(validate_object_selector("memory-object:esp-idf/ble/state").is_ok());
        assert!(validate_object_selector("function:esp-idf/ble/start").is_err());
        assert!(validate_object_selector("memory-object:").is_err());
        assert!(
            validate_object_selector(&format!(
                "occurrence:memory-object:sha256:{}",
                "1".repeat(64)
            ))
            .is_ok()
        );
        assert!(
            validate_object_selector(&format!("occurrence:function:sha256:{}", "1".repeat(64)))
                .is_err()
        );
        assert!(validate_object_selector("occurrence:memory-object:sha256:bad").is_err());
    }
}
