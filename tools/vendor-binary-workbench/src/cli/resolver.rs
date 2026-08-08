//! Resolution of parsed CLI, project and run-spec inputs into executable invocations.

mod defaults;
mod register_catalog;
#[cfg(test)]
mod tests;

use std::{env, path::PathBuf};

use super::args::{Command, CommandArguments, ParsedInvocation};
use crate::{MemoryMap, MmioRegisterMap, ProjectSpec, Result, TargetSpec, run_spec::RunSpec};
use defaults::{apply_project_defaults, apply_run_spec_defaults, apply_target_defaults};

/// The complete result of configuration resolution.
///
/// Project setup commands intentionally do not carry a partially initialized
/// target context. Every ordinary command instead receives the same resolved,
/// owned context regardless of which domain workflow will consume it.
pub(super) enum ResolvedInvocation {
    ProjectInit {
        arguments: CommandArguments,
    },
    ProjectConfigure {
        arguments: CommandArguments,
        project_path: PathBuf,
    },
    Command(Box<ResolvedCommandInvocation>),
}

pub(super) struct ResolvedCommandInvocation {
    pub(super) command: Command,
    pub(super) arguments: CommandArguments,
    pub(super) project_path: Option<PathBuf>,
    pub(super) project: Option<ProjectSpec>,
    pub(super) target_path: PathBuf,
    pub(super) target: TargetSpec,
    pub(super) run_spec_path: Option<PathBuf>,
    pub(super) run_spec: Option<RunSpec>,
    pub(super) memory_map: Option<MemoryMap>,
    pub(super) svd_paths: Vec<PathBuf>,
    pub(super) svd: MmioRegisterMap,
}

pub(super) fn resolve(invocation: ParsedInvocation) -> Result<ResolvedInvocation> {
    let current_dir = env::current_dir()?;
    resolve_from(invocation, &current_dir)
}

fn resolve_from(
    invocation: ParsedInvocation,
    current_dir: &std::path::Path,
) -> Result<ResolvedInvocation> {
    let ParsedInvocation {
        ui: _,
        command,
        project: requested_project,
        target_spec: requested_target,
        run_spec: requested_run_spec,
        mut svd_paths,
        arguments: mut command_arguments,
    } = invocation;

    if command == Command::ProjectInit {
        if requested_project.is_some()
            || requested_target.is_some()
            || requested_run_spec.is_some()
            || !svd_paths.is_empty()
        {
            return Err(
                "project init does not accept --project, --target-spec, --run-spec or --svd".into(),
            );
        }
        return Ok(ResolvedInvocation::ProjectInit {
            arguments: command_arguments,
        });
    }

    // An explicit target selects standalone target/backend development and
    // therefore deliberately disables implicit project discovery.
    let project_path = if requested_project.is_some() || requested_target.is_some() {
        requested_project
    } else {
        ProjectSpec::discover_from(current_dir)?
    };
    if command == Command::ProjectConfigure {
        if requested_target.is_some() || requested_run_spec.is_some() || !svd_paths.is_empty() {
            return Err(
                "project configure does not accept --target-spec, --run-spec or --svd".into(),
            );
        }
        return Ok(ResolvedInvocation::ProjectConfigure {
            arguments: command_arguments,
            project_path: project_path
                .ok_or("project configure requires --project or a discovered manifest")?,
        });
    }

    let project = project_path.as_deref().map(ProjectSpec::load).transpose()?;
    require_project(command, project.as_ref())?;

    let target_path = requested_target
        .or_else(|| project.as_ref().map(|project| project.target_spec.clone()))
        .ok_or("missing --project or --target-spec, and no vendor-project.toml was found")?;
    let mut target = TargetSpec::load(&target_path)?;
    if let Some(pack) = project
        .as_ref()
        .and_then(|project| project.platform_pack.as_ref())
    {
        pack.apply_to_target(&mut target)?;
    }
    if command.requires_backend() {
        target.require_available_backend()?;
    }
    if command.requires_harness() {
        target.require_available_harness()?;
    }
    trace_resolved_target(command, project_path.as_deref(), project.as_ref(), &target);

    let run_spec_path = if command.uses_run_spec() {
        requested_run_spec.or_else(|| {
            project
                .as_ref()
                .and_then(|project| project.run_spec.clone())
        })
    } else {
        None
    };
    let run_spec = run_spec_path.as_deref().map(RunSpec::load).transpose()?;
    if let Some(run_spec) = &run_spec {
        apply_run_spec_defaults(command, &mut command_arguments, run_spec);
    }

    if svd_paths.is_empty() {
        svd_paths = project
            .as_ref()
            .filter(|project| project.svd_configured)
            .map(|project| project.svd_paths.clone())
            .unwrap_or_else(|| target.svd_paths.clone());
    }
    apply_target_defaults(command, &mut command_arguments, &target);

    let memory_map = if command.uses_memory_map() {
        let project_memory_map = project
            .as_ref()
            .and_then(|project| project.memory_map.as_deref());
        project_memory_map
            .or(target.memory_map.as_deref())
            .map(MemoryMap::load)
            .transpose()?
    } else {
        None
    };
    apply_project_defaults(
        command,
        &mut command_arguments,
        project.as_ref(),
        memory_map.as_ref(),
    )?;

    let mut svd = register_catalog::load(command, &svd_paths, project.as_ref())?;
    if let Some(memory_map) = &memory_map {
        svd.windows.extend(memory_map.mmio_windows()?);
        svd.windows.sort_by_key(|window| (window.start, window.end));
        svd.windows.dedup();
    }
    if command.requires_mmio_map() && svd.windows.is_empty() {
        return Err("command requires an MMIO region; add memory-map to the project".into());
    }

    Ok(ResolvedInvocation::Command(Box::new(
        ResolvedCommandInvocation {
            command,
            arguments: command_arguments,
            project_path,
            project,
            target_path,
            target,
            run_spec_path,
            run_spec,
            memory_map,
            svd_paths,
            svd,
        },
    )))
}

fn require_project(command: Command, project: Option<&ProjectSpec>) -> Result<()> {
    if matches!(
        command,
        Command::ProjectDoctor
            | Command::ProjectStatus
            | Command::ProjectConfigure
            | Command::ProjectAnalyze
            | Command::ProjectPublish
            | Command::FunctionInitPack
            | Command::FunctionValidate
            | Command::FunctionReview
            | Command::InterfaceInitPack
            | Command::InterfaceValidate
            | Command::RegisterInitModel
            | Command::RegisterImportSvd
            | Command::RegisterValidate
            | Command::RegisterReview
            | Command::RegisterExportSvd
            | Command::RegisterGeneratePac
            | Command::RegisterGenerateBindings
            | Command::BuildIr
    ) && project.is_none()
    {
        return Err("project/workspace commands require a project manifest".into());
    }
    if command.requires_harness() && project.is_none() {
        return Err(
            "platform-harness commands require a project manifest and platform pack".into(),
        );
    }
    Ok(())
}

fn trace_resolved_target(
    command: Command,
    project_path: Option<&std::path::Path>,
    project: Option<&ProjectSpec>,
    target: &TargetSpec,
) {
    if matches!(command, Command::ProjectDoctor | Command::ProjectStatus) {
        return;
    }
    if let Some(project) = project {
        tracing::info!(
            project.id = %project.id,
            project.manifest = %project_path
                .expect("a loaded project has a manifest path")
                .display(),
            "loaded project"
        );
    }
    tracing::info!(
        target.id = %target.id,
        target.harness = target.harness.as_deref().unwrap_or("-"),
        target.architecture = target.architecture.label(),
        target.calling_convention = target.calling_convention.label(),
        target.endianness = target.endianness.label(),
        target.pointer_width,
        target.rust_target = %target.rust_target,
        "resolved target"
    );
}
