//! Project-profile resolution for execution comparison.

use std::path::PathBuf;

use super::{ApplicationError, ApplicationResult, ComparisonScenario, ProjectSession, operations};

pub(super) fn compare_profile(
    resolved: &ProjectSession,
    name: &str,
) -> ApplicationResult<crate::ExecutionComparisonReport> {
    let workspace =
        resolved.project.verification.as_ref().ok_or_else(|| {
            crate::Error::invalid("project has no [verification] profile workspace")
        })?;
    let mut selected = None;
    for suite in &workspace.suites {
        for path in &suite.profiles {
            for profile in crate::verification::profiles::load(path)? {
                if profile.name != name {
                    continue;
                }
                if selected
                    .replace((
                        profile,
                        suite.rust_artifact_role.clone(),
                        suite.rust_companion_role.clone(),
                    ))
                    .is_some()
                {
                    return Err(ApplicationError::from(crate::Error::invalid(format!(
                        "comparison profile {name:?} is defined more than once"
                    ))));
                }
            }
        }
    }
    let (profile, rust_artifact_role, rust_companion_role) = selected
        .ok_or_else(|| crate::Error::invalid(format!("unknown comparison profile {name:?}")))?;
    let run = resolved.run_spec.as_ref().ok_or_else(|| {
        crate::Error::invalid(
            "comparison requires a project run-spec with vendor and Rust artifacts",
        )
    })?;
    let input = |wanted: crate::run_spec::InputRole| -> Option<PathBuf> {
        run.inputs()
            .iter()
            .find(|input| input.role == wanted)
            .map(|input| input.path.clone())
    };
    let source: crate::source_id::SourceId = profile.vendor_source.parse().map_err(|_| {
        crate::Error::invalid(format!(
            "comparison profile {} has invalid source {:?}",
            profile.name, profile.vendor_source
        ))
    })?;
    let vendor_artifact = input(crate::run_spec::InputRole::SourceArtifact(source.clone()))
        .or_else(|| {
            (profile.vendor_source == "vendor")
                .then(|| input(crate::run_spec::InputRole::VendorArtifact))
                .flatten()
        })
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "run-spec has no artifact for source {}",
                profile.vendor_source
            ))
        })?;
    let vendor_companion =
        input(crate::run_spec::InputRole::SourceCompanion(source)).or_else(|| {
            (profile.vendor_source == "vendor")
                .then(|| input(crate::run_spec::InputRole::VendorCompanion))
                .flatten()
        });
    let rust_artifact = input(rust_artifact_role.clone()).ok_or_else(|| {
        crate::Error::invalid(format!("run-spec has no {rust_artifact_role} input"))
    })?;
    let rust_companion = rust_companion_role.and_then(input);
    let table_scenarios = profile
        .scenarios
        .iter()
        .map(|scenario| ComparisonScenario {
            name: scenario.name.clone(),
            scenario: scenario.scenario.clone(),
            vendor_table_instances: scenario.vendor_table_instances.clone(),
            rust_table_instances: scenario.rust_table_instances.clone(),
        })
        .collect::<Vec<_>>();
    operations::validate_table_instances(resolved, &table_scenarios)?;
    let argument_domain = profile.coverage_argument_constraints();
    Ok(crate::compare_execution_scenarios(
        &resolved.mmio,
        crate::ExecutionInput {
            artifact: &vendor_artifact,
            companion: vendor_companion.as_deref(),
            symbol: &profile.vendor_symbol,
        },
        crate::ExecutionInput {
            artifact: &rust_artifact,
            companion: rust_companion.as_deref(),
            symbol: &profile.rust_symbol,
        },
        profile.compare_return,
        &argument_domain,
        &profile.scenarios,
    )?)
}
