//! Artifact inventory analysis command.

use std::{fmt::Write as _, path::Path};

use super::super::json::{write_artifact, write_string, write_strings};
use super::super::*;

#[derive(Debug)]
struct FunctionReport {
    symbol: String,
    owner: Option<String>,
    direct_trace_exact: bool,
    reference_codegen_eligible: bool,
    event_count: usize,
    reference_dependencies: Vec<String>,
    indexed_mmio: usize,
    direct_blockers: Vec<String>,
    local_reference_blockers: Vec<String>,
    transitive_reference_blockers: Vec<String>,
    unmapped_mmio: Vec<u32>,
    blocking_callees: Vec<String>,
}

#[derive(Debug, Default)]
struct BlockerImpact {
    occurrences: usize,
    functions: BTreeSet<String>,
}

fn blocker_kind(blocker: &str) -> &str {
    blocker.split_once(' ').map_or(blocker, |(kind, _)| kind)
}

fn blocking_callees(blocker: &str) -> BTreeSet<String> {
    const MARKER: &str = "callee-ineligible at ";

    let mut output = BTreeSet::new();
    let mut remainder = blocker;
    while let Some(marker) = remainder.find(MARKER) {
        let after_marker = &remainder[marker + MARKER.len()..];
        let Some((_, after_site)) = after_marker.split_once(": ") else {
            break;
        };
        let name_length = after_site
            .find(|character: char| {
                character.is_whitespace() || matches!(character, '[' | ';' | '|')
            })
            .unwrap_or(after_site.len());
        if name_length == 0 {
            break;
        }
        output.insert(after_site[..name_length].to_owned());
        remainder = &after_site[name_length..];
    }
    output
}

fn record_impact(
    impacts: &mut BTreeMap<(String, String), BlockerImpact>,
    scope: &str,
    blocker: &str,
    symbol: &str,
) {
    let kind = if scope == "reference-transitive" {
        "callee-ineligible"
    } else {
        blocker_kind(blocker)
    };
    let impact = impacts
        .entry((scope.to_owned(), kind.to_owned()))
        .or_default();
    impact.occurrences += 1;
    impact.functions.insert(symbol.to_owned());
}

#[allow(
    clippy::too_many_arguments,
    reason = "the report boundary receives a complete immutable analysis record"
)]
fn write_analysis_json_report(
    path: &Path,
    artifact: &Path,
    companions: &[PathBuf],
    prefix: &str,
    entry_contract: entry_contract::EntryContract,
    functions: &[FunctionReport],
    impacts: &BTreeMap<(String, String), BlockerImpact>,
    callee_callers: &BTreeMap<String, BTreeSet<String>>,
    unmapped_users: &BTreeMap<u32, BTreeSet<String>>,
    direct_exact: usize,
    reference_eligible: usize,
) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 1,\n  \"command\": \"analyze\",\n");
    output.push_str("  \"artifact\": ");
    write_artifact(&mut output, artifact)?;
    output.push_str(",\n  \"companions\": [");
    for (index, companion) in companions.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        write_artifact(&mut output, companion)?;
    }
    output.push_str("],\n  \"symbol_prefix\": ");
    write_string(&mut output, prefix);
    output.push_str(",\n  \"entry_contract\": ");
    write_string(&mut output, entry_contract.id());
    writeln!(output, ",\n  \"summary\": {{").expect("writing to String cannot fail");
    writeln!(output, "    \"functions\": {},", functions.len())
        .expect("writing to String cannot fail");
    writeln!(output, "    \"direct_trace_exact\": {direct_exact},")
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "    \"direct_trace_incomplete\": {},",
        functions.len() - direct_exact
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "    \"reference_codegen_eligible\": {reference_eligible},"
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "    \"reference_codegen_blocked\": {}",
        functions.len() - reference_eligible
    )
    .expect("writing to String cannot fail");
    output.push_str("  },\n  \"blocker_impact\": [\n");
    for (index, ((scope, kind), impact)) in impacts.iter().enumerate() {
        output.push_str("    {\"scope\": ");
        write_string(&mut output, scope);
        output.push_str(", \"kind\": ");
        write_string(&mut output, kind);
        write!(
            output,
            ", \"occurrences\": {}, \"affected_functions\": {}, \"functions\": ",
            impact.occurrences,
            impact.functions.len()
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, &impact.functions);
        output.push('}');
        output.push_str(if index + 1 == impacts.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"callee_hotspots\": [\n");
    let mut hotspots = callee_callers.iter().collect::<Vec<_>>();
    hotspots.sort_by(|(left_name, left_callers), (right_name, right_callers)| {
        right_callers
            .len()
            .cmp(&left_callers.len())
            .then_with(|| left_name.cmp(right_name))
    });
    for (index, (callee, callers)) in hotspots.iter().enumerate() {
        output.push_str("    {\"callee\": ");
        write_string(&mut output, callee);
        write!(
            output,
            ", \"affected_functions\": {}, \"functions\": ",
            callers.len()
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, *callers);
        output.push('}');
        output.push_str(if index + 1 == hotspots.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"unmapped_mmio\": [\n");
    for (index, (address, users)) in unmapped_users.iter().enumerate() {
        write!(output, "    {{\"address\": \"{address:#010x}\", ")
            .expect("writing to String cannot fail");
        write!(
            output,
            "\"affected_functions\": {}, \"functions\": ",
            users.len()
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, users);
        output.push('}');
        output.push_str(if index + 1 == unmapped_users.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ],\n  \"functions\": [\n");
    for (index, function) in functions.iter().enumerate() {
        output.push_str("    {\"symbol\": ");
        write_string(&mut output, &function.symbol);
        output.push_str(", \"owner\": ");
        if let Some(owner) = function.owner.as_deref() {
            write_string(&mut output, owner);
        } else {
            output.push_str("null");
        }
        write!(
            output,
            ", \"direct_trace_exact\": {}, \"reference_codegen_eligible\": {}, \"events\": {}, \"indexed_mmio\": {}, \"reference_dependencies\": ",
            function.direct_trace_exact,
            function.reference_codegen_eligible,
            function.event_count,
            function.indexed_mmio,
        )
        .expect("writing to String cannot fail");
        write_strings(&mut output, &function.reference_dependencies);
        output.push_str(", \"direct_blockers\": ");
        write_strings(&mut output, &function.direct_blockers);
        output.push_str(", \"local_reference_blockers\": ");
        write_strings(&mut output, &function.local_reference_blockers);
        output.push_str(", \"transitive_reference_blockers\": ");
        write_strings(&mut output, &function.transitive_reference_blockers);
        output.push_str(", \"unmapped_mmio\": [");
        for (address_index, address) in function.unmapped_mmio.iter().enumerate() {
            if address_index != 0 {
                output.push_str(", ");
            }
            write!(output, "\"{address:#010x}\"").expect("writing to String cannot fail");
        }
        output.push_str("], \"blocking_callees\": ");
        write_strings(&mut output, &function.blocking_callees);
        output.push('}');
        output.push_str(if index + 1 == functions.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ]\n}\n");
    fs::write(path, output)?;
    println!("JSON-REPORT\t{}", path.display());
    Ok(())
}

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut artifact = None;
    let mut companions = Vec::new();
    let mut prefix = "phy_".to_owned();
    let mut entry_contract = entry_contract::EntryContract::None;
    let mut json_report = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
            }
            "--companion" => {
                companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--symbol-prefix" => {
                prefix = take_value(&mut arguments, "--symbol-prefix")?;
            }
            "--json-report" => {
                json_report = Some(PathBuf::from(take_value(&mut arguments, "--json-report")?));
            }
            "--entry-contract" => {
                entry_contract = entry_contract::EntryContract::parse(&take_value(
                    &mut arguments,
                    "--entry-contract",
                )?)?;
            }
            _ => return Err(format!("unknown analyze option: {argument}").into()),
        }
    }
    let artifact = artifact.ok_or("missing --artifact")?;
    let symbols = list_code_symbols(&artifact, &prefix)?;
    if symbols.is_empty() {
        return Err(format!("no external code symbols start with {prefix:?}").into());
    }
    let reference_catalog =
        ReferenceResolver::load_with_entry_contract(&artifact, &companions, entry_contract)?;

    let mut exact = 0usize;
    let mut incomplete = 0usize;
    let mut reference_codegen_eligible = 0usize;
    let mut reasons = BTreeMap::<String, usize>::new();
    let mut reference_reasons = BTreeMap::<String, usize>::new();
    let mut impacts = BTreeMap::<(String, String), BlockerImpact>::new();
    let mut callee_callers = BTreeMap::<String, BTreeSet<String>>::new();
    let mut unmapped_users = BTreeMap::<u32, BTreeSet<String>>::new();
    let mut function_reports = Vec::with_capacity(symbols.len());
    for symbol in &symbols {
        let input = ArtifactSymbolSelector {
            artifact: artifact.clone(),
            member: symbol.member.clone(),
            symbol: symbol.name.clone(),
        };
        let trace = extract(&input, svd)?;
        let reference_trace =
            reference_catalog.trace(symbol.member.as_deref(), &symbol.name, svd)?;
        let owner = symbol.member.as_deref().unwrap_or("-");
        let reference_eligible = reference_trace.is_reference_eligible();
        let reference_status = if reference_eligible {
            reference_codegen_eligible += 1;
            "eligible"
        } else {
            "blocked"
        };
        if trace.is_exact() {
            exact += 1;
            println!(
                "FUNCTION\t{}\t{owner}\tDIRECT-TRACE-EXACT\tevents={}\treference-codegen={reference_status}\treference-dependencies={}\tindexed-mmio={}",
                symbol.name,
                trace.events.len(),
                reference_trace.reference_dependencies.len(),
                reference_trace.reference_indexed_mmio_count(),
            );
        } else {
            incomplete += 1;
            println!(
                "FUNCTION\t{}\t{owner}\tINCOMPLETE\tevents={}\tuncovered={}\treference-codegen={reference_status}\treference-dependencies={}\tindexed-mmio={}",
                symbol.name,
                trace.events.len(),
                trace.blockers.len()
                    + trace
                        .events
                        .iter()
                        .filter_map(ObservableEvent::unmapped_address)
                        .count(),
                reference_trace.reference_dependencies.len(),
                reference_trace.reference_indexed_mmio_count(),
            );
            for blocker in &trace.blockers {
                *reasons.entry(blocker_kind(blocker).to_owned()).or_default() += 1;
                record_impact(&mut impacts, "direct", blocker, &symbol.name);
                println!("UNCOVERED\t{}\t{blocker}", symbol.name);
            }
            for address in trace
                .events
                .iter()
                .filter_map(ObservableEvent::unmapped_address)
            {
                *reasons.entry("unmapped-register".to_owned()).or_default() += 1;
                record_impact(&mut impacts, "direct", "unmapped-register", &symbol.name);
                println!(
                    "UNCOVERED\t{}\tunmapped-register {:#010x}",
                    symbol.name, address
                );
            }
        }
        for blocker in &reference_trace.reference_blockers {
            *reference_reasons
                .entry(blocker_kind(blocker).to_owned())
                .or_default() += 1;
            println!("REFERENCE-BLOCKED\t{}\t{blocker}", symbol.name);
        }

        let unmapped_mmio = reference_trace
            .reference_unmapped_addresses()
            .into_iter()
            .collect::<Vec<_>>();
        for address in &unmapped_mmio {
            unmapped_users
                .entry(*address)
                .or_default()
                .insert(symbol.name.clone());
        }
        let mut local_reference_blockers = Vec::new();
        let mut transitive_reference_blockers = Vec::new();
        let mut callees = BTreeSet::new();
        if !reference_eligible {
            for blocker in reference_trace.reference_failure_reasons() {
                let blocker_callees = blocking_callees(&blocker);
                if blocker_callees.is_empty() {
                    record_impact(&mut impacts, "reference-local", &blocker, &symbol.name);
                    local_reference_blockers.push(blocker);
                } else {
                    record_impact(&mut impacts, "reference-transitive", &blocker, &symbol.name);
                    callees.extend(blocker_callees);
                    transitive_reference_blockers.push(blocker);
                }
            }
        }
        for callee in &callees {
            callee_callers
                .entry(callee.clone())
                .or_default()
                .insert(symbol.name.clone());
        }
        function_reports.push(FunctionReport {
            symbol: symbol.name.clone(),
            owner: symbol.member.clone(),
            direct_trace_exact: trace.is_exact(),
            reference_codegen_eligible: reference_eligible,
            event_count: trace.events.len(),
            reference_dependencies: reference_trace.reference_dependencies.clone(),
            indexed_mmio: reference_trace.reference_indexed_mmio_count(),
            direct_blockers: trace.blockers.clone(),
            local_reference_blockers,
            transitive_reference_blockers,
            unmapped_mmio,
            blocking_callees: callees.into_iter().collect(),
        });
    }
    println!(
        "SUMMARY\tfunctions={}\tdirect_trace_exact={exact}\tincomplete={incomplete}\treference_codegen_eligible={reference_codegen_eligible}\treference_codegen_blocked={}",
        symbols.len(),
        symbols.len() - reference_codegen_eligible,
    );
    for (reason, count) in reasons {
        println!("SUMMARY-UNCOVERED\t{reason}\t{count}");
    }
    for (reason, count) in reference_reasons {
        println!("SUMMARY-REFERENCE-BLOCKED\t{reason}\t{count}");
    }
    if let Some(path) = json_report.as_deref() {
        write_analysis_json_report(
            path,
            &artifact,
            &companions,
            &prefix,
            entry_contract,
            &function_reports,
            &impacts,
            &callee_callers,
            &unmapped_users,
            exact,
            reference_codegen_eligible,
        )?;
    }
    Ok(incomplete == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_nested_callee_blockers() {
        assert_eq!(
            blocking_callees(
                "call-summary-flattening: callee-ineligible at 0x00001000: outer [causes: symbolic-cfg: callee-ineligible at 0x00002000: inner [causes: loop]]"
            ),
            BTreeSet::from(["inner".to_owned(), "outer".to_owned()])
        );
    }
}
