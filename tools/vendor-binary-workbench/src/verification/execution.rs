//! Concrete vendor/Rust execution comparison.

use std::collections::BTreeSet;

use crate::*;

mod scenario;

pub(crate) use scenario::*;

fn artifact_report(input: ExecutionInput<'_>) -> Result<ArtifactReport> {
    Ok(ArtifactReport {
        path: input.artifact.display().to_string(),
        sha256: artifact_sha256(input.artifact)?,
        companion: input
            .companion
            .map(|path| -> Result<ArtifactIdentity> {
                Ok(ArtifactIdentity {
                    path: path.display().to_string(),
                    sha256: artifact_sha256(path)?,
                })
            })
            .transpose()?,
        symbol: input.symbol.to_owned(),
    })
}

fn coverage_report(
    image: &execution::ExecutableImage,
    inventory: &execution::CoverageInventory,
    covered: &BTreeSet<(u32, bool)>,
    calls: BTreeSet<String>,
    indirect_calls: &BTreeSet<execution::IndirectCall>,
    unmapped_mmio: BTreeSet<u32>,
) -> CoverageReport {
    CoverageReport {
        covered_calls: calls.into_iter().collect(),
        branch_outcomes: inventory
            .branch_outcomes
            .iter()
            .map(|(site, taken)| BranchOutcomeReport {
                site: *site,
                location: image.location(*site),
                taken: *taken,
                covered: covered.contains(&(*site, *taken)),
            })
            .collect(),
        unresolved_control_flow: inventory
            .unresolved_edges
            .iter()
            .map(|(site, edge)| {
                let targets = indirect_calls
                    .iter()
                    .filter(|call| call.site == *site)
                    .map(|call| call.symbol.clone())
                    .collect::<Vec<_>>();
                ControlFlowReport {
                    site: *site,
                    location: image.location(*site),
                    edge: edge.clone(),
                    covered: !targets.is_empty(),
                    targets,
                }
            })
            .collect(),
        unmapped_mmio: unmapped_mmio.into_iter().collect(),
    }
}

#[tracing::instrument(
    name = "compare_execution_scenarios",
    skip_all,
    fields(
        vendor = %vendor.artifact.display(),
        vendor_symbol = vendor.symbol,
        rust = %rust.artifact.display(),
        rust_symbol = rust.symbol,
        scenarios = scenarios.len()
    )
)]
pub(crate) fn compare_execution_scenarios(
    svd: &MmioRegisterMap,
    vendor: ExecutionInput<'_>,
    rust: ExecutionInput<'_>,
    compare_return: bool,
    argument_domain: &[[Option<u32>; 8]],
    scenarios: &[NamedScenario],
) -> Result<ExecutionComparisonReport> {
    let vendor_report = artifact_report(vendor)?;
    let rust_report = artifact_report(rust)?;
    let mut vendor_image = execution::ExecutableImage::load(vendor.artifact)?;
    if let Some(companion) = vendor.companion {
        vendor_image.add_companion(companion)?;
    }
    let mut rust_image = execution::ExecutableImage::load(rust.artifact)?;
    if let Some(companion) = rust.companion {
        rust_image.add_companion(companion)?;
    }
    let mut vendor_inventory =
        static_inventory_for_argument_domain(&vendor_image, vendor.symbol, argument_domain)?;
    let mut rust_inventory =
        static_inventory_for_argument_domain(&rust_image, rust.symbol, argument_domain)?;
    let mut vendor_covered = BTreeSet::new();
    let mut rust_covered = BTreeSet::new();
    let mut vendor_calls = BTreeSet::new();
    let mut rust_calls = BTreeSet::new();
    let mut vendor_indirect_calls = BTreeSet::new();
    let mut rust_indirect_calls = BTreeSet::new();
    let mut vendor_unmapped = BTreeSet::new();
    let mut rust_unmapped = BTreeSet::new();
    let mut matched_cases = 0_usize;
    let mut mismatched_cases = 0_usize;
    let mut incomplete_cases = 0_usize;
    let mut case_reports = Vec::with_capacity(scenarios.len());

    for named in scenarios {
        let vendor_lengths: Vec<_> = named
            .vendor_observations
            .iter()
            .map(MemoryObservation::length)
            .collect();
        let rust_lengths: Vec<_> = named
            .rust_observations
            .iter()
            .map(MemoryObservation::length)
            .collect();
        if vendor_lengths != rust_lengths {
            return Err(format!(
                "scenario {} has different vendor/Rust observation layouts",
                named.name
            )
            .into());
        }
        let vendor_result = execution::execute(
            &vendor_image,
            svd,
            vendor.symbol,
            resolved_scenario(named, &vendor_image, true)?,
        );
        let rust_result = execution::execute(
            &rust_image,
            svd,
            rust.symbol,
            resolved_scenario(named, &rust_image, false)?,
        );
        let (vendor_result, rust_result) = match (vendor_result, rust_result) {
            (Ok(vendor_result), Ok(rust_result)) => (vendor_result, rust_result),
            (vendor_result, rust_result) => {
                incomplete_cases += 1;
                case_reports.push(CaseReport::Incomplete {
                    name: named.name.clone(),
                    vendor_error: vendor_result.err().map(|error| error.to_string()),
                    rust_error: rust_result.err().map(|error| error.to_string()),
                });
                continue;
            }
        };
        vendor_covered.extend(vendor_result.branches.iter().copied());
        rust_covered.extend(rust_result.branches.iter().copied());
        vendor_calls.extend(vendor_result.calls.iter().cloned());
        rust_calls.extend(rust_result.calls.iter().cloned());
        vendor_indirect_calls.extend(vendor_result.indirect_calls.iter().cloned());
        rust_indirect_calls.extend(rust_result.indirect_calls.iter().cloned());
        vendor_unmapped.extend(
            vendor_result
                .events
                .iter()
                .filter_map(unmapped_execution_address),
        );
        rust_unmapped.extend(
            rust_result
                .events
                .iter()
                .filter_map(unmapped_execution_address),
        );

        let events_equal = vendor_result.events == rust_result.events;
        let memory_equal = vendor_result.memory_changes == rust_result.memory_changes;
        let returns_equal =
            !compare_return || vendor_result.return_value == rust_result.return_value;
        if events_equal && memory_equal && returns_equal {
            matched_cases += 1;
            case_reports.push(CaseReport::Match {
                name: named.name.clone(),
                events: vendor_result.events.len(),
                memory_changes: vendor_result.memory_changes.len(),
                return_compared: compare_return,
            });
        } else {
            mismatched_cases += 1;
            case_reports.push(CaseReport::Mismatch {
                name: named.name.clone(),
                vendor: (&vendor_result).into(),
                rust: (&rust_result).into(),
            });
        }
    }

    extend_dynamic_inventory(&vendor_image, &mut vendor_inventory, &vendor_indirect_calls)?;
    extend_dynamic_inventory(&rust_image, &mut rust_inventory, &rust_indirect_calls)?;
    let vendor_coverage = coverage_report(
        &vendor_image,
        &vendor_inventory,
        &vendor_covered,
        vendor_calls,
        &vendor_indirect_calls,
        vendor_unmapped,
    );
    let rust_coverage = coverage_report(
        &rust_image,
        &rust_inventory,
        &rust_covered,
        rust_calls,
        &rust_indirect_calls,
        rust_unmapped,
    );
    let vendor_uncovered = vendor_coverage.uncovered_branch_outcomes();
    let rust_uncovered = rust_coverage.uncovered_branch_outcomes();
    let vendor_unresolved = vendor_coverage.uncovered_control_flow();
    let rust_unresolved = rust_coverage.uncovered_control_flow();
    let cases_match = matched_cases == scenarios.len();
    let coverage_complete = vendor_uncovered == 0
        && rust_uncovered == 0
        && vendor_unresolved == 0
        && rust_unresolved == 0
        && vendor_coverage.unmapped_mmio.is_empty()
        && rust_coverage.unmapped_mmio.is_empty();
    let verdict = if mismatched_cases != 0 {
        ComparisonVerdict::Mismatch
    } else if incomplete_cases != 0 || !coverage_complete || !cases_match {
        ComparisonVerdict::Incomplete
    } else {
        ComparisonVerdict::Match
    };
    Ok(ExecutionComparisonReport {
        schema_version: 1,
        command: "execute compare",
        vendor: vendor_report,
        rust: rust_report,
        compare_return,
        cases: case_reports,
        summary: ComparisonSummary {
            cases: scenarios.len(),
            matched: matched_cases,
            mismatched: mismatched_cases,
            incomplete: incomplete_cases,
            vendor_uncovered_branch_outcomes: vendor_uncovered,
            rust_uncovered_branch_outcomes: rust_uncovered,
            vendor_unresolved_control_flow: vendor_unresolved,
            rust_unresolved_control_flow: rust_unresolved,
            vendor_unmapped_mmio: vendor_coverage.unmapped_mmio.len(),
            rust_unmapped_mmio: rust_coverage.unmapped_mmio.len(),
        },
        vendor_coverage,
        rust_coverage,
        verdict,
    })
}
