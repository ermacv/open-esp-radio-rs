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
    mismatched: usize,
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
        "branches={} control-flow={} MMIO={}",
        coverage.uncovered_branch_outcomes(),
        coverage.uncovered_control_flow(),
        coverage.unmapped_mmio.len(),
    )
}

fn case_row(profile: &str, case: &CaseReport) -> [String; 4] {
    match case {
        CaseReport::Match {
            name,
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
        CaseReport::Mismatch { name, vendor, rust } => [
            profile.to_owned(),
            name.clone(),
            "MISMATCH".to_owned(),
            format!(
                "vendor: events={} memory={} return={:#x}; rust: events={} memory={} return={:#x}",
                vendor.events.len(),
                vendor.memory_changes.len(),
                vendor.return_value,
                rust.events.len(),
                rust.memory_changes.len(),
                rust.return_value,
            ),
        ],
        CaseReport::Incomplete {
            name,
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
                "Mismatch",
                "Incomplete",
                "Vendor gaps",
                "Rust gaps",
            ],
            report.profiles.iter().map(|profile| [
                profile.name.clone(),
                profile.comparison.verdict.label().to_owned(),
                profile.comparison.summary.cases.to_string(),
                profile.comparison.summary.matched.to_string(),
                profile.comparison.summary.mismatched.to_string(),
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
        "Summary: profiles={} matched={} mismatched={} incomplete={}",
        report.summary.profiles,
        report.summary.matched,
        report.summary.mismatched,
        report.summary.incomplete,
    );
}

fn print_tsv(report: &ProfileVerificationReport) {
    for profile in &report.profiles {
        outputln!("PROFILE\t{}\tBEGIN", profile.name);
        print_execution_comparison(&profile.comparison);
        outputln!(
            "PROFILE\t{}\t{}",
            profile.name,
            profile.comparison.verdict.label()
        );
    }
    outputln!(
        "PROFILE-SUMMARY\tprofiles={}\tmatch={}\tmismatch={}\tincomplete={}",
        report.summary.profiles,
        report.summary.matched,
        report.summary.mismatched,
        report.summary.incomplete,
    );
}

pub(super) fn run(arguments: VerifyProfilesArgs, svd: &MmioRegisterMap) -> Result<bool> {
    let profile_path = arguments.profiles.ok_or("missing --profiles")?;
    let vendor_artifact = arguments
        .vendor_artifact
        .ok_or("missing --vendor-artifact")?;
    let rust_artifact = arguments.rust_artifact.ok_or("missing --rust-artifact")?;
    let loaded_profiles = profiles::load(&profile_path)?;
    let mut matched = 0_usize;
    let mut mismatched = 0_usize;
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
            ComparisonVerdict::Match => matched += 1,
            ComparisonVerdict::Mismatch => mismatched += 1,
            ComparisonVerdict::Incomplete => {}
        }
        reports.push(ProfileComparisonReport {
            name: profile.name.clone(),
            comparison,
        });
    }
    let report = ProfileVerificationReport {
        schema_version: 1,
        command: "verify profiles",
        summary: ProfileVerificationSummary {
            profiles: loaded_profiles.len(),
            matched,
            mismatched,
            incomplete: loaded_profiles.len() - matched - mismatched,
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
