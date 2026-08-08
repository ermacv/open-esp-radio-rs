//! Profile verification command.

use super::super::*;

use serde::Serialize;

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
    if !crate::cli::output::structured("profile-verification", &report) {
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
            "PROFILE-SUMMARY\tprofiles={}\tmatch={matched}\tmismatch={mismatched}\tincomplete={}",
            loaded_profiles.len(),
            loaded_profiles.len() - matched - mismatched,
        );
    }
    Ok(matched == loaded_profiles.len())
}
