//! Artifact inventory analysis command.

use std::path::{Path, PathBuf};

use serde::Serialize;

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
    direct_unmapped_mmio: Vec<u32>,
    reference_unmapped_mmio: Vec<u32>,
    reference_blockers: Vec<String>,
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

#[derive(Serialize)]
struct ArtifactIdentity {
    path: String,
    sha256: String,
}

impl ArtifactIdentity {
    fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.display().to_string(),
            sha256: crate::artifact_sha256(path)?,
        })
    }
}

#[derive(Serialize)]
struct AnalysisSummary {
    functions: usize,
    direct_trace_exact: usize,
    direct_trace_incomplete: usize,
    reference_codegen_eligible: usize,
    reference_codegen_blocked: usize,
}

#[derive(Serialize)]
struct BlockerImpactDocument<'a> {
    scope: &'a str,
    kind: &'a str,
    occurrences: usize,
    affected_functions: usize,
    functions: &'a BTreeSet<String>,
}

#[derive(Serialize)]
struct CalleeHotspotDocument<'a> {
    callee: &'a str,
    affected_functions: usize,
    functions: &'a BTreeSet<String>,
}

#[derive(Serialize)]
struct UnmappedMmioDocument<'a> {
    address: String,
    affected_functions: usize,
    functions: &'a BTreeSet<String>,
}

#[derive(Serialize)]
struct FunctionDocument<'a> {
    symbol: &'a str,
    owner: &'a Option<String>,
    direct_trace_exact: bool,
    reference_codegen_eligible: bool,
    events: usize,
    indexed_mmio: usize,
    reference_dependencies: &'a [String],
    direct_blockers: &'a [String],
    local_reference_blockers: &'a [String],
    transitive_reference_blockers: &'a [String],
    direct_unmapped_mmio: Vec<String>,
    reference_unmapped_mmio: Vec<String>,
    reference_blockers: &'a [String],
    blocking_callees: &'a [String],
}

impl<'a> From<&'a FunctionReport> for FunctionDocument<'a> {
    fn from(function: &'a FunctionReport) -> Self {
        Self {
            symbol: &function.symbol,
            owner: &function.owner,
            direct_trace_exact: function.direct_trace_exact,
            reference_codegen_eligible: function.reference_codegen_eligible,
            events: function.event_count,
            indexed_mmio: function.indexed_mmio,
            reference_dependencies: &function.reference_dependencies,
            direct_blockers: &function.direct_blockers,
            local_reference_blockers: &function.local_reference_blockers,
            transitive_reference_blockers: &function.transitive_reference_blockers,
            reference_blockers: &function.reference_blockers,
            direct_unmapped_mmio: function
                .direct_unmapped_mmio
                .iter()
                .map(|address| format!("{address:#010x}"))
                .collect(),
            reference_unmapped_mmio: function
                .reference_unmapped_mmio
                .iter()
                .map(|address| format!("{address:#010x}"))
                .collect(),
            blocking_callees: &function.blocking_callees,
        }
    }
}

#[derive(Serialize)]
pub(super) struct AnalysisDocument<'a> {
    schema_version: u32,
    command: &'static str,
    artifact: ArtifactIdentity,
    companions: Vec<ArtifactIdentity>,
    symbol_prefix: &'a str,
    entry_contract: &'a str,
    summary: AnalysisSummary,
    blocker_impact: Vec<BlockerImpactDocument<'a>>,
    callee_hotspots: Vec<CalleeHotspotDocument<'a>>,
    unmapped_mmio: Vec<UnmappedMmioDocument<'a>>,
    functions: Vec<FunctionDocument<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<crate::cli::output::Publication>,
}

struct AnalysisInputs<'a> {
    artifact: &'a Path,
    companions: &'a [PathBuf],
    prefix: &'a str,
    entry_contract: EntryContractRef,
    functions: &'a [FunctionReport],
    impacts: &'a BTreeMap<(String, String), BlockerImpact>,
    callee_callers: &'a BTreeMap<String, BTreeSet<String>>,
    unmapped_users: &'a BTreeMap<u32, BTreeSet<String>>,
    direct_exact: usize,
    reference_eligible: usize,
    publication: Option<crate::cli::output::Publication>,
}

fn analysis_document(inputs: AnalysisInputs<'_>) -> Result<AnalysisDocument<'_>> {
    let AnalysisInputs {
        artifact,
        companions,
        prefix,
        entry_contract,
        functions,
        impacts,
        callee_callers,
        unmapped_users,
        direct_exact,
        reference_eligible,
        publication,
    } = inputs;
    let mut hotspots = callee_callers.iter().collect::<Vec<_>>();
    hotspots.sort_by(|(left_name, left_callers), (right_name, right_callers)| {
        right_callers
            .len()
            .cmp(&left_callers.len())
            .then_with(|| left_name.cmp(right_name))
    });
    Ok(AnalysisDocument {
        schema_version: 3,
        command: "inspect analyze",
        artifact: ArtifactIdentity::load(artifact)?,
        companions: companions
            .iter()
            .map(|path| ArtifactIdentity::load(path))
            .collect::<Result<Vec<_>>>()?,
        symbol_prefix: prefix,
        entry_contract: entry_contract.id(),
        summary: AnalysisSummary {
            functions: functions.len(),
            direct_trace_exact: direct_exact,
            direct_trace_incomplete: functions.len() - direct_exact,
            reference_codegen_eligible: reference_eligible,
            reference_codegen_blocked: functions.len() - reference_eligible,
        },
        blocker_impact: impacts
            .iter()
            .map(|((scope, kind), impact)| BlockerImpactDocument {
                scope,
                kind,
                occurrences: impact.occurrences,
                affected_functions: impact.functions.len(),
                functions: &impact.functions,
            })
            .collect(),
        callee_hotspots: hotspots
            .into_iter()
            .map(|(callee, callers)| CalleeHotspotDocument {
                callee,
                affected_functions: callers.len(),
                functions: callers,
            })
            .collect(),
        unmapped_mmio: unmapped_users
            .iter()
            .map(|(address, users)| UnmappedMmioDocument {
                address: format!("{address:#010x}"),
                affected_functions: users.len(),
                functions: users,
            })
            .collect(),
        functions: functions.iter().map(Into::into).collect(),
        publication,
    })
}

fn write_analysis_output(path: &Path, document: &AnalysisDocument<'_>) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(&document)? + "\n")?;
    Ok(())
}

fn print_analysis_report(
    functions: &[FunctionReport],
    reasons: &BTreeMap<String, usize>,
    reference_reasons: &BTreeMap<String, usize>,
) {
    let exact = functions
        .iter()
        .filter(|function| function.direct_trace_exact)
        .count();
    let reference_eligible = functions
        .iter()
        .filter(|function| function.reference_codegen_eligible)
        .count();
    outputln!("{}", crate::cli::output::heading("Artifact analysis"));
    outputln!(
        "{} function(s): {exact} exact, {} incomplete; {reference_eligible} reference-ready, {} blocked",
        functions.len(),
        functions.len() - exact,
        functions.len() - reference_eligible,
    );
    outputln!("\n{}", crate::cli::output::heading("Functions"));
    outputln!(
        "{}",
        crate::cli::table::render(
            ["Function", "Owner", "Trace", "Events", "Reference"],
            functions.iter().map(|function| [
                function.symbol.clone(),
                function
                    .owner
                    .clone()
                    .unwrap_or_else(|| "unknown".to_owned()),
                if function.direct_trace_exact {
                    "exact"
                } else {
                    "incomplete"
                }
                .to_owned(),
                function.event_count.to_string(),
                if function.reference_codegen_eligible {
                    "eligible"
                } else {
                    "blocked"
                }
                .to_owned(),
            ]),
        )
    );
    if !reasons.is_empty() || !reference_reasons.is_empty() {
        outputln!("\n{}", crate::cli::output::heading("Blocker summary"));
        for (reason, count) in reasons {
            outputln!("- Trace: {reason} ({count})");
        }
        for (reason, count) in reference_reasons {
            outputln!("- Reference: {reason} ({count})");
        }
    }
    if crate::cli::output::details() {
        outputln!("\n{}", crate::cli::output::heading("Function blockers"));
        for function in functions {
            for blocker in &function.direct_blockers {
                outputln!("- {}: {blocker}", function.symbol);
            }
            for address in &function.direct_unmapped_mmio {
                outputln!("- {}: unmapped register {address:#010x}", function.symbol);
            }
            for blocker in &function.reference_blockers {
                outputln!("- {} reference: {blocker}", function.symbol);
            }
        }
    }
}

pub(super) fn run(
    arguments: InspectAnalyzeArgs,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    let harness = target.require_available_knowledge_provider()?;
    let riscv_harness = providers::riscv(harness)?;
    let entry_contract = providers::entry_contract(harness, &arguments.entry_contract)?;
    let artifact = arguments
        .artifact
        .ok_or("missing --artifact")
        .map_err(crate::Error::invalid)?;
    let symbols = list_code_symbols(&artifact, &arguments.symbol_prefix)?;
    if symbols.is_empty() {
        return Err(crate::Error::invalid(format!(
            "no external code symbols start with {:?}",
            arguments.symbol_prefix
        )));
    }
    let reference_catalog = ReferenceResolver::load_with_entry_contract(
        &artifact,
        &arguments.companion,
        riscv_harness,
        entry_contract,
    )?;

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
        let reference_eligible = reference_trace.is_reference_eligible();
        if reference_eligible {
            reference_codegen_eligible += 1;
        }
        let direct_unmapped_mmio = trace
            .events
            .iter()
            .filter_map(ObservableEvent::unmapped_address)
            .collect::<Vec<_>>();
        if trace.is_exact() {
            exact += 1;
        } else {
            incomplete += 1;
            for blocker in &trace.blockers {
                *reasons.entry(blocker_kind(blocker).to_owned()).or_default() += 1;
                record_impact(&mut impacts, "direct", blocker, &symbol.name);
            }
            for _address in &direct_unmapped_mmio {
                *reasons.entry("unmapped-register".to_owned()).or_default() += 1;
                record_impact(&mut impacts, "direct", "unmapped-register", &symbol.name);
            }
        }
        for blocker in &reference_trace.reference_blockers {
            *reference_reasons
                .entry(blocker_kind(blocker).to_owned())
                .or_default() += 1;
        }

        let reference_unmapped_mmio = reference_trace
            .reference_unmapped_addresses()
            .into_iter()
            .collect::<Vec<_>>();
        for address in &reference_unmapped_mmio {
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
            direct_unmapped_mmio,
            reference_unmapped_mmio,
            reference_blockers: reference_trace.reference_blockers.clone(),
            blocking_callees: callees.into_iter().collect(),
        });
    }
    let publication = arguments
        .output
        .as_deref()
        .map(|path| crate::cli::output::Publication::new(path, "written"));
    let document = analysis_document(AnalysisInputs {
        artifact: &artifact,
        companions: &arguments.companion,
        prefix: &arguments.symbol_prefix,
        entry_contract,
        functions: &function_reports,
        impacts: &impacts,
        callee_callers: &callee_callers,
        unmapped_users: &unmapped_users,
        direct_exact: exact,
        reference_eligible: reference_codegen_eligible,
        publication: publication.clone(),
    })?;
    if let Some(path) = arguments.output.as_deref() {
        write_analysis_output(path, &document)?;
    }
    let render = || {
        print_analysis_report(&function_reports, &reasons, &reference_reasons);
        if let Some(publication) = &publication {
            outputln!("\nReport {}: {}", publication.status, publication.path);
        }
    };
    crate::cli::output::render_report(&document, render);
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
