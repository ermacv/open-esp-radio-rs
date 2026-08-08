//! Profile verification command.

use super::super::*;

use serde::Serialize;

use crate::cli::args::OutputFormat;

#[derive(Serialize)]
struct ProfileComparisonReport {
    name: String,
    comparison: ExecutionComparisonReport,
}

#[derive(Serialize)]
struct ProfileVerificationSummary {
    profiles: usize,
    matched: usize,
    different: usize,
    incomplete: usize,
}

#[derive(Serialize)]
struct ProfileVerificationReport {
    schema_version: u32,
    command: &'static str,
    profiles: Vec<ProfileComparisonReport>,
    summary: ProfileVerificationSummary,
}

fn coverage_summary(coverage: &CoverageReport) -> String {
    format!(
        "branches={} control-flow={} unnamed-MMIO={}",
        coverage.uncovered_branch_outcomes(),
        coverage.uncovered_control_flow(),
        coverage.unnamed_mmio.len(),
    )
}

fn case_row(profile: &str, case: &CaseReport) -> [String; 4] {
    match case {
        CaseReport::Match {
            name,
            environment: _,
            events,
            memory_changes,
            return_compared,
        } => [
            profile.to_owned(),
            name.clone(),
            "MATCH".to_owned(),
            format!(
                "events={events} memory-changes={memory_changes} return-compared={return_compared}"
            ),
        ],
        CaseReport::Diff {
            name,
            environment: _,
            difference,
        } => [
            profile.to_owned(),
            name.clone(),
            "DIFF".to_owned(),
            format!(
                "kind={:?} first-difference={}",
                difference.kind, difference.first_difference,
            ),
        ],
        CaseReport::Incomplete {
            name,
            environment: _,
            vendor_error,
            rust_error,
        } => [
            profile.to_owned(),
            name.clone(),
            "INCOMPLETE".to_owned(),
            format!(
                "vendor={}; rust={}",
                vendor_error.as_deref().unwrap_or("ok"),
                rust_error.as_deref().unwrap_or("ok"),
            ),
        ],
    }
}

fn print_human(report: &ProfileVerificationReport) {
    outputln!(
        "Profile verification:\n{}",
        crate::cli::table::render(
            [
                "Profile",
                "Verdict",
                "Cases",
                "Matched",
                "Different",
                "Incomplete",
                "Vendor gaps",
                "Rust gaps",
            ],
            report.profiles.iter().map(|profile| [
                profile.name.clone(),
                profile.comparison.verdict.label().to_owned(),
                profile.comparison.summary.cases.to_string(),
                profile.comparison.summary.matched.to_string(),
                profile.comparison.summary.different.to_string(),
                profile.comparison.summary.incomplete.to_string(),
                coverage_summary(&profile.comparison.vendor_coverage),
                coverage_summary(&profile.comparison.rust_coverage),
            ]),
        )
    );
    let cases = report
        .profiles
        .iter()
        .flat_map(|profile| {
            profile
                .comparison
                .cases
                .iter()
                .map(|case| case_row(&profile.name, case))
        })
        .collect::<Vec<_>>();
    if !cases.is_empty() {
        outputln!(
            "Scenarios:\n{}",
            crate::cli::table::render(["Profile", "Scenario", "Verdict", "Details"], cases)
        );
    }
    outputln!(
        "Summary: profiles={} matched={} different={} incomplete={}",
        report.summary.profiles,
        report.summary.matched,
        report.summary.different,
        report.summary.incomplete,
    );
}

fn print_tsv(report: &ProfileVerificationReport) {
    for profile in &report.profiles {
        outputln!("PROFILE\t{}\tBEGIN", profile.name);
        crate::cli::render::print_execution_comparison(&profile.comparison);
        outputln!(
            "PROFILE\t{}\t{}",
            profile.name,
            profile.comparison.verdict.label()
        );
    }
    outputln!(
        "PROFILE-SUMMARY\tprofiles={}\tmatch={}\tdiff={}\tincomplete={}",
        report.summary.profiles,
        report.summary.matched,
        report.summary.different,
        report.summary.incomplete,
    );
}

pub(super) fn run(arguments: VerifyProfilesArgs, svd: &MmioMap) -> Result<bool> {
    let profile_path = arguments
        .profiles
        .ok_or("missing --profiles")
        .map_err(crate::Error::invalid)?;
    let vendor_artifact = arguments
        .vendor_artifact
        .ok_or("missing --vendor-artifact")
        .map_err(crate::Error::invalid)?;
    let rust_artifact = arguments
        .rust_artifact
        .ok_or("missing --rust-artifact")
        .map_err(crate::Error::invalid)?;
    let loaded_profiles = profiles::load(&profile_path)?;
    let mut matched = 0_usize;
    let mut different = 0_usize;
    let mut reports = Vec::with_capacity(loaded_profiles.len());
    for profile in &loaded_profiles {
        let argument_domain = profile.coverage_argument_constraints();
        let comparison = compare_execution_scenarios(
            svd,
            ExecutionInput {
                artifact: &vendor_artifact,
                companion: arguments.vendor_companion.as_deref(),
                symbol: &profile.vendor_symbol,
            },
            ExecutionInput {
                artifact: &rust_artifact,
                companion: arguments.rust_companion.as_deref(),
                symbol: &profile.rust_symbol,
            },
            profile.compare_return,
            &argument_domain,
            &profile.scenarios,
        )?;
        match comparison.verdict {
            EquivalenceVerdict::Match => matched += 1,
            EquivalenceVerdict::Diff => different += 1,
            EquivalenceVerdict::Incomplete => {}
        }
        reports.push(ProfileComparisonReport {
            name: profile.name.clone(),
            comparison,
        });
    }
    let report = ProfileVerificationReport {
        schema_version: 2,
        command: "verify profiles",
        summary: ProfileVerificationSummary {
            profiles: loaded_profiles.len(),
            matched,
            different,
            incomplete: loaded_profiles.len() - matched - different,
        },
        profiles: reports,
    };
    if !crate::cli::output::structured(&report) {
        match crate::cli::output::format() {
            OutputFormat::Human => print_human(&report),
            OutputFormat::Tsv => print_tsv(&report),
            OutputFormat::Json | OutputFormat::Jsonl => {
                unreachable!("typed profile verification was already emitted")
            }
        }
    }
    Ok(matched == loaded_profiles.len())
}
