//! Profile verification command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut profile_path = None;
    let mut vendor_artifact = None;
    let mut vendor_companion = None;
    let mut rust_artifact = None;
    let mut rust_companion = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--profiles" => {
                profile_path = Some(PathBuf::from(take_value(&mut arguments, "--profiles")?));
            }
            "--vendor-artifact" => {
                vendor_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-artifact",
                )?));
            }
            "--vendor-companion" => {
                vendor_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--vendor-companion",
                )?));
            }
            "--rust-artifact" => {
                rust_artifact = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--rust-artifact",
                )?));
            }
            "--rust-companion" => {
                rust_companion = Some(PathBuf::from(take_value(
                    &mut arguments,
                    "--rust-companion",
                )?));
            }
            _ => return Err(format!("unknown verify-profiles option: {argument}").into()),
        }
    }
    let profile_path = profile_path.ok_or("missing --profiles")?;
    let vendor_artifact = vendor_artifact.ok_or("missing --vendor-artifact")?;
    let rust_artifact = rust_artifact.ok_or("missing --rust-artifact")?;
    let loaded_profiles = profiles::load(&profile_path)?;
    let mut matched = 0_usize;
    let mut mismatched = 0_usize;
    for profile in &loaded_profiles {
        println!("PROFILE\t{}\tBEGIN", profile.name);
        let result = compare_execution_scenarios(
            svd,
            ExecutionInput {
                artifact: &vendor_artifact,
                companion: vendor_companion.as_deref(),
                symbol: &profile.vendor_symbol,
            },
            ExecutionInput {
                artifact: &rust_artifact,
                companion: rust_companion.as_deref(),
                symbol: &profile.rust_symbol,
            },
            profile.compare_return,
            &profile.scenarios,
        )?;
        match result {
            ComparisonVerdict::Match => matched += 1,
            ComparisonVerdict::Mismatch => mismatched += 1,
            ComparisonVerdict::Incomplete => {}
        }
        println!("PROFILE\t{}\t{}", profile.name, result.label());
    }
    println!(
        "PROFILE-SUMMARY\tprofiles={}\tmatch={matched}\tmismatch={mismatched}\tincomplete={}",
        loaded_profiles.len(),
        loaded_profiles.len() - matched - mismatched,
    );
    Ok(matched == loaded_profiles.len())
}
