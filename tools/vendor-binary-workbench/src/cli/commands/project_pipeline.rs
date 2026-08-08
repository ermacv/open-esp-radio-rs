//! Project-owned evidence generation and read-only consistency checks.

use std::path::Path;

use super::{Command, CommandArguments, MmioMap, Result, TargetSpec};
use crate::cli::{
    InterfaceDiscoverArgs, IrBuildArgs, MmioDiscoverArgs, NamedAddressRange, ProjectAnalyzeArgs,
    RegisterReviewArgs, ReviewArgs, SourcePath, SymbolInventoryArgs, ValidationArgs,
};
use crate::{
    MemoryMap,
    project::ProjectSpec,
    run_spec::{InputRole, RunSpec},
};

pub(crate) mod status;

use status::{
    Mode, PipelineSummary, StageOutcome, StageSuccess, execute, record as report, render,
};

#[derive(Debug, Default, Eq, PartialEq)]
struct Options {
    deny_unreviewed: bool,
}

pub(super) fn run(
    arguments: ProjectAnalyzeArgs,
    project: &ProjectSpec,
    run_spec: Option<&RunSpec>,
    memory_map: Option<&MemoryMap>,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mode = Mode::from_check(arguments.check);
    let options = Options {
        deny_unreviewed: arguments.deny_unreviewed,
    };
    let mut summary = PipelineSummary::default();

    let symbols = if let Some(symbols) = project.symbol_inventory.as_ref() {
        match run_spec {
            Some(run_spec) => execute("symbol-inventory", mode.generated_success(), || {
                super::symbol_inventory::run(
                    SymbolInventoryArgs {
                        check: mode.is_check(),
                        json_report: Some(symbols.output.clone()),
                        ..Default::default()
                    },
                    run_spec,
                )
            }),
            None => StageOutcome::Blocked("run-spec is not configured".to_owned()),
        }
    } else {
        StageOutcome::NotConfigured("[analysis.symbols] is absent".to_owned())
    };
    report("symbol-inventory", &symbols, &mut summary);

    let mmio = if let Some(registers) = project.registers.as_ref() {
        match (run_spec, memory_map) {
            (None, _) => StageOutcome::Blocked("run-spec is not configured".to_owned()),
            (_, None) => StageOutcome::Blocked("memory-map is not configured".to_owned()),
            (Some(run_spec), Some(memory_map)) => {
                let mut arguments = mmio_arguments(run_spec, memory_map, &registers.facts)?;
                arguments.check = mode.is_check();
                execute("mmio-discovery", mode.generated_success(), || {
                    super::discover_mmio::run(arguments, svd)
                })
            }
        }
    } else {
        StageOutcome::NotConfigured("[registers] is absent".to_owned())
    };
    report("mmio-discovery", &mmio, &mut summary);

    let interfaces = if let Some(paths) = project.interfaces.as_ref() {
        match run_spec {
            Some(run_spec) => {
                let arguments = InterfaceDiscoverArgs {
                    check: mode.is_check(),
                    json_report: Some(paths.facts.clone()),
                    ..Default::default()
                };
                execute("interface-discovery", mode.generated_success(), || {
                    super::interface_discovery::run(arguments, run_spec)
                })
            }
            None => StageOutcome::Blocked("run-spec is not configured".to_owned()),
        }
    } else {
        StageOutcome::NotConfigured("[interfaces] is absent".to_owned())
    };
    report("interface-discovery", &interfaces, &mut summary);

    let ir = if project.ir_profiles.is_empty() {
        StageOutcome::NotConfigured("[[analysis.ir]] is absent".to_owned())
    } else {
        match run_spec {
            Some(run_spec) => execute("linked-ir", mode.generated_success(), || {
                super::ir_build::run(
                    IrBuildArgs {
                        check: mode.is_check(),
                        ..Default::default()
                    },
                    project,
                    run_spec,
                    svd,
                    target,
                )
            }),
            None => StageOutcome::Blocked("run-spec is not configured".to_owned()),
        }
    };
    report("linked-ir", &ir, &mut summary);

    let navigation = match project.navigation_index.as_ref() {
        None => StageOutcome::NotConfigured("[analysis.navigation] is absent".to_owned()),
        Some(_) if symbols.blocks_dependants() => {
            StageOutcome::Blocked("symbol-inventory did not complete".to_owned())
        }
        Some(_) if !project.ir_profiles.is_empty() && ir.blocks_dependants() => {
            StageOutcome::Blocked("linked-ir did not complete".to_owned())
        }
        Some(_) if project.interfaces.is_some() && interfaces.blocks_dependants() => {
            StageOutcome::Blocked("interface-discovery did not complete".to_owned())
        }
        Some(spec) => execute("navigation-index", mode.generated_success(), || {
            super::project_navigation::run(project, &spec.output, mode.is_check())
        }),
    };
    report("navigation-index", &navigation, &mut summary);

    let register_validation = match project.registers.as_ref() {
        None => StageOutcome::NotConfigured("[registers] is absent".to_owned()),
        Some(_) if mmio.blocks_dependants() => {
            StageOutcome::Blocked("mmio-discovery did not complete".to_owned())
        }
        Some(_) => execute("register-validation", StageSuccess::Verified, || {
            super::registers::run(
                Command::RegisterValidate,
                CommandArguments::Validation(ValidationArgs {
                    deny_unreviewed: options.deny_unreviewed,
                }),
                project,
                memory_map,
            )
        }),
    };
    report("register-validation", &register_validation, &mut summary);

    let register_review = match project.registers.as_ref() {
        None => StageOutcome::NotConfigured("[registers] is absent".to_owned()),
        Some(paths) if paths.review_output.is_none() => {
            StageOutcome::NotConfigured("[registers.review] is absent".to_owned())
        }
        Some(_) if mmio.blocks_dependants() => {
            StageOutcome::Blocked("mmio-discovery did not complete".to_owned())
        }
        Some(paths)
            if review_depends_on_project_ir(project, &paths.review_ir_reports)
                && ir.blocks_dependants() =>
        {
            StageOutcome::Blocked("linked-ir did not complete".to_owned())
        }
        Some(_) => execute("register-review", mode.generated_success(), || {
            super::registers::run(
                Command::RegisterReview,
                CommandArguments::RegisterReview(RegisterReviewArgs {
                    check: mode.is_check(),
                    ..Default::default()
                }),
                project,
                memory_map,
            )
        }),
    };
    report("register-review", &register_review, &mut summary);

    let function_validation = match project.functions.as_ref() {
        None => StageOutcome::NotConfigured("[functions] is absent".to_owned()),
        Some(_) if ir.blocks_dependants() => {
            StageOutcome::Blocked("linked-ir did not complete".to_owned())
        }
        Some(_) => execute("function-validation", StageSuccess::Verified, || {
            super::function_pack::run(
                Command::FunctionValidate,
                CommandArguments::Validation(ValidationArgs {
                    deny_unreviewed: options.deny_unreviewed,
                }),
                project,
                target,
            )
        }),
    };
    report("function-validation", &function_validation, &mut summary);

    let function_review = match project.functions.as_ref() {
        None => StageOutcome::NotConfigured("[functions] is absent".to_owned()),
        Some(paths) if paths.review_output.is_none() => {
            StageOutcome::NotConfigured("[functions.review] is absent".to_owned())
        }
        Some(_) if ir.blocks_dependants() => {
            StageOutcome::Blocked("linked-ir did not complete".to_owned())
        }
        Some(_)
            if project
                .interfaces
                .as_ref()
                .and_then(|paths| paths.pack.as_deref())
                .is_some_and(Path::is_file)
                && interfaces.blocks_dependants() =>
        {
            StageOutcome::Blocked("interface-discovery did not complete".to_owned())
        }
        Some(_) => execute("function-review", mode.generated_success(), || {
            super::function_pack::run(
                Command::FunctionReview,
                CommandArguments::Review(ReviewArgs {
                    check: mode.is_check(),
                    ..Default::default()
                }),
                project,
                target,
            )
        }),
    };
    report("function-review", &function_review, &mut summary);

    let interface_validation = match project.interfaces.as_ref() {
        None => StageOutcome::NotConfigured("[interfaces] is absent".to_owned()),
        Some(paths) if paths.pack.is_none() => {
            StageOutcome::NotConfigured("[interfaces].pack is absent".to_owned())
        }
        Some(_) if interfaces.blocks_dependants() => {
            StageOutcome::Blocked("interface-discovery did not complete".to_owned())
        }
        Some(_) => execute("interface-validation", StageSuccess::Verified, || {
            super::interface_pack::run(
                Command::InterfaceValidate,
                CommandArguments::Validation(ValidationArgs {
                    deny_unreviewed: options.deny_unreviewed,
                }),
                project,
                target,
            )
        }),
    };
    report("interface-validation", &interface_validation, &mut summary);

    render(mode, &summary);
    Ok(summary.succeeded())
}

fn mmio_arguments(
    run_spec: &RunSpec,
    memory_map: &MemoryMap,
    output: &Path,
) -> Result<MmioDiscoverArgs> {
    let mut artifacts = Vec::new();
    for input in run_spec.inputs() {
        let InputRole::SourceArtifact(source) = &input.role else {
            continue;
        };
        artifacts.push(
            SourcePath::new(source.clone(), input.path.clone())
                .map_err(|message| -> crate::Error { crate::Error::invalid(message) })?,
        );
    }
    let mut ranges = Vec::new();
    for (name, start, end) in memory_map.mmio_ranges()? {
        ranges.push(NamedAddressRange { name, start, end });
    }
    Ok(MmioDiscoverArgs {
        artifact: artifacts,
        range: ranges,
        json_report: Some(output.to_owned()),
        ..Default::default()
    })
}

fn review_depends_on_project_ir(project: &ProjectSpec, reports: &[std::path::PathBuf]) -> bool {
    project
        .ir_profiles
        .iter()
        .any(|profile| reports.iter().any(|report| report == &profile.output))
}
