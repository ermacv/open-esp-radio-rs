//! Resolution of parsed CLI, project and run-spec inputs into executable invocations.

mod defaults;
mod register_catalog;
#[cfg(test)]
mod tests;

use std::{env, path::PathBuf};

use super::args::{Command, CommandArguments, ParsedInvocation};
use crate::{
    MemoryMap, MmioMap, ProjectSpec, Result, TargetSpec,
    application::{ProjectSession, ProjectSessionOptions},
    run_spec::RunSpec,
};
use defaults::{apply_project_defaults, apply_run_spec_defaults, apply_target_defaults};

/// The complete result of configuration resolution.
///
/// Project setup commands intentionally do not carry a partially initialized
/// target context. Every ordinary command instead receives the same resolved,
/// owned context regardless of which domain workflow will consume it.
pub(super) enum ResolvedInvocation {
    Tooling {
        command: Command,
        arguments: CommandArguments,
    },
    ProjectInit {
        arguments: CommandArguments,
    },
    ProjectConfigure {
        arguments: CommandArguments,
        project_path: PathBuf,
    },
    ProjectInputsInit {
        arguments: CommandArguments,
        project_path: PathBuf,
    },
    ProjectBrowse {
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
    pub(super) svd: MmioMap,
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

    if command.is_tooling() {
        if requested_project.is_some()
            || requested_target.is_some()
            || requested_run_spec.is_some()
            || !svd_paths.is_empty()
        {
            return Err(crate::Error::invalid(
                "tooling commands do not accept --project, --target-spec, --run-spec or --svd",
            ));
        }
        return Ok(ResolvedInvocation::Tooling {
            command,
            arguments: command_arguments,
        });
    }

    if command == Command::ProjectInit {
        if requested_project.is_some()
            || requested_target.is_some()
            || requested_run_spec.is_some()
            || !svd_paths.is_empty()
        {
            return Err(crate::Error::invalid(
                "project init does not accept --project, --target-spec, --run-spec or --svd",
            ));
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
            return Err(crate::Error::invalid(
                "project configure does not accept --target-spec, --run-spec or --svd",
            ));
        }
        return Ok(ResolvedInvocation::ProjectConfigure {
            arguments: command_arguments,
            project_path: project_path
                .ok_or("project configure requires --project or a discovered manifest")
                .map_err(crate::Error::invalid)?,
        });
    }
    if command == Command::ProjectInputsInit {
        if requested_target.is_some() || requested_run_spec.is_some() || !svd_paths.is_empty() {
            return Err(crate::Error::invalid(
                "project inputs init does not accept --target-spec, --run-spec or --svd",
            ));
        }
        return Ok(ResolvedInvocation::ProjectInputsInit {
            arguments: command_arguments,
            project_path: project_path
                .ok_or("project inputs init requires --project or a discovered manifest")
                .map_err(crate::Error::invalid)?,
        });
    }
    if command == Command::ProjectBrowse {
        if requested_target.is_some() || requested_run_spec.is_some() || !svd_paths.is_empty() {
            return Err(crate::Error::invalid(
                "project browse accepts a project manifest, not --target-spec, --run-spec or --svd",
            ));
        }
        return Ok(ResolvedInvocation::ProjectBrowse {
            arguments: command_arguments,
            project_path: project_path
                .ok_or("project browse requires --project or a discovered manifest")
                .map_err(crate::Error::invalid)?,
        });
    }

    require_project(command, project_path.as_ref())?;

    let (
        project,
        target_path,
        target,
        run_spec_path,
        run_spec,
        memory_map,
        resolved_svd_paths,
        mut svd,
    ) = if let Some(manifest) = project_path.as_deref() {
        let session = ProjectSession::open_with(
            manifest,
            ProjectSessionOptions {
                target_spec: requested_target,
                run_spec: requested_run_spec,
                svd_paths,
                load_run_spec: command.uses_run_spec(),
                load_memory_map: command.uses_memory_map(),
                load_register_catalog: command.uses_register_catalog(),
            },
        )?;
        (
            Some(session.project),
            session.target_path,
            session.target,
            session.run_spec_path,
            session.run_spec,
            session.memory_map,
            session.svd_paths,
            session.mmio,
        )
    } else {
        let target_path = requested_target
            .ok_or("missing --project or --target-spec, and no vendor-project.toml was found")
            .map_err(crate::Error::invalid)?;
        let target = TargetSpec::load(&target_path)?;
        let run_spec_path = command
            .uses_run_spec()
            .then_some(requested_run_spec)
            .flatten();
        let run_spec = run_spec_path.as_deref().map(RunSpec::load).transpose()?;
        if svd_paths.is_empty() {
            svd_paths.clone_from(&target.svd_paths);
        }
        let memory_map = if command.uses_memory_map() {
            target
                .memory_map
                .as_deref()
                .map(MemoryMap::load)
                .transpose()?
        } else {
            None
        };
        let svd = register_catalog::load(command, &svd_paths, None)?;
        (
            None,
            target_path,
            target,
            run_spec_path,
            run_spec,
            memory_map,
            svd_paths,
            svd,
        )
    };
    let svd_paths = resolved_svd_paths;
    if command.requires_backend() {
        target.require_available_backend()?;
    }
    if command.requires_harness() {
        target.require_available_harness()?;
    }
    trace_resolved_target(command, project_path.as_deref(), project.as_ref(), &target);

    if let Some(run_spec) = &run_spec {
        apply_run_spec_defaults(command, &mut command_arguments, run_spec);
    }
    apply_target_defaults(command, &mut command_arguments, &target);
    apply_project_defaults(
        command,
        &mut command_arguments,
        project.as_ref(),
        memory_map.as_ref(),
    )?;

    if let Some(memory_map) = &memory_map {
        svd.regions.extend(memory_map.resolved_mmio_regions()?);
        svd.regions
            .sort_by_key(|region| (region.start, region.end, region.name.clone()));
        svd.regions.dedup();
    }
    if command.requires_mmio_map() && svd.regions.is_empty() {
        return Err(crate::Error::invalid(
            "command requires an MMIO region; add memory-map to the project",
        ));
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

fn require_project(command: Command, project: Option<&PathBuf>) -> Result<()> {
    if matches!(
        command,
        Command::ProjectDoctor
            | Command::ProjectStatus
            | Command::ProjectBrowse
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
        return Err(crate::Error::invalid(
            "project/workspace commands require a project manifest",
        ));
    }
    if command.requires_harness() && project.is_none() {
        return Err(crate::Error::invalid(
            "platform-harness commands require a project manifest and platform pack",
        ));
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
