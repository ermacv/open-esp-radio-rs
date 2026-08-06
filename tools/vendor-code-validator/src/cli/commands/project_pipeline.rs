//! Project-owned evidence generation and read-only consistency checks.

use std::path::Path;

use super::{Command, MmioRegisterMap, Result, TargetSpec};
use crate::{MemoryMap, project::ProjectSpec, run_spec::RunSpec};

mod status;

use status::{Mode, PipelineSummary, StageOutcome, StageSuccess, execute, report};

#[derive(Debug, Default, Eq, PartialEq)]
struct Options {
    deny_unreviewed: bool,
}

pub(super) fn run(
    command: Command,
    arguments: Vec<String>,
    project: &ProjectSpec,
    run_spec: Option<&RunSpec>,
    memory_map: Option<&MemoryMap>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mode = Mode::parse(command);
    let options = parse_options(arguments)?;
    let mut summary = PipelineSummary::default();

    let mmio = if let Some(registers) = project.registers.as_ref() {
        match (run_spec, memory_map) {
            (None, _) => StageOutcome::Blocked("run-spec is not configured".to_owned()),
            (_, None) => StageOutcome::Blocked("memory-map is not configured".to_owned()),
            (Some(run_spec), Some(memory_map)) => {
                let mut arguments = mmio_arguments(run_spec, memory_map, &registers.facts)?;
                arguments.extend(mode.check_argument());
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
                let mut arguments = vec![
                    "--json-report".to_owned(),
                    paths.facts.display().to_string(),
                ];
                arguments.extend(mode.check_argument());
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
                super::ir_build::run(mode.check_argument(), project, run_spec, svd, target)
            }),
            None => StageOutcome::Blocked("run-spec is not configured".to_owned()),
        }
    };
    report("linked-ir", &ir, &mut summary);

    let register_validation = match project.registers.as_ref() {
        None => StageOutcome::NotConfigured("[registers] is absent".to_owned()),
        Some(_) if mmio.blocks_dependants() => {
            StageOutcome::Blocked("mmio-discovery did not complete".to_owned())
        }
        Some(_) => {
            let arguments = if options.deny_unreviewed {
                vec!["--deny-unreviewed".to_owned()]
            } else {
                Vec::new()
            };
            execute("register-validation", StageSuccess::Verified, || {
                super::registers::run(Command::RegisterValidate, arguments, project, memory_map)
            })
        }
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
                mode.check_argument(),
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
        Some(_) => {
            let arguments = if options.deny_unreviewed {
                vec!["--deny-unreviewed".to_owned()]
            } else {
                Vec::new()
            };
            execute("function-validation", StageSuccess::Verified, || {
                super::function_pack::run(Command::FunctionValidate, arguments, project, target)
            })
        }
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
                mode.check_argument(),
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
        Some(_) => {
            let arguments = if options.deny_unreviewed {
                vec!["--deny-unreviewed".to_owned()]
            } else {
                Vec::new()
            };
            execute("interface-validation", StageSuccess::Verified, || {
                super::interface_pack::run(Command::InterfaceValidate, arguments, project, target)
            })
        }
    };
    report("interface-validation", &interface_validation, &mut summary);

    println!(
        "PROJECT-PIPELINE\tmode={}\tstatus={}\twritten={}\tverified={}\tfailed={}\tblocked={}\tnot-configured={}",
        mode.label(),
        if summary.succeeded() { "ok" } else { "failed" },
        summary.written,
        summary.verified,
        summary.failed,
        summary.blocked,
        summary.not_configured,
    );
    Ok(summary.succeeded())
}

fn parse_options(arguments: Vec<String>) -> Result<Options> {
    let mut options = Options::default();
    for argument in arguments {
        match argument.as_str() {
            "--deny-unreviewed" => {
                if options.deny_unreviewed {
                    return Err("duplicate --deny-unreviewed".into());
                }
                options.deny_unreviewed = true;
            }
            _ => return Err(format!("unknown project pipeline option: {argument}").into()),
        }
    }
    Ok(options)
}

fn mmio_arguments(
    run_spec: &RunSpec,
    memory_map: &MemoryMap,
    output: &Path,
) -> Result<Vec<String>> {
    let mut arguments = Vec::new();
    for (role, path) in run_spec.inputs() {
        let Some(source) = role.strip_prefix("source-artifact:") else {
            continue;
        };
        arguments.push(format!("--source-artifact:{source}"));
        arguments.push(path.display().to_string());
    }
    for (name, start, end) in memory_map.mmio_ranges()? {
        arguments.push("--range".to_owned());
        arguments.push(format!("{name}={start:#010x}..{end:#010x}"));
    }
    arguments.push("--json-report".to_owned());
    arguments.push(output.display().to_string());
    Ok(arguments)
}

fn review_depends_on_project_ir(project: &ProjectSpec, reports: &[std::path::PathBuf]) -> bool {
    project
        .ir_profiles
        .iter()
        .any(|profile| reports.iter().any(|report| report == &profile.output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_options_are_intentionally_small() {
        assert_eq!(
            parse_options(vec!["--deny-unreviewed".to_owned()]).unwrap(),
            Options {
                deny_unreviewed: true
            }
        );
        assert!(parse_options(vec!["--release".to_owned()]).is_err());
    }
}
