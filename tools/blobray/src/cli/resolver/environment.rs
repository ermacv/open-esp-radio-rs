//! Loading and merging project, target, run-spec, memory and register inputs.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::{
    ProjectSpec, Result, TargetSpec,
    application::{ProjectSession, ProjectSessionOptions},
    cli::args::{Command, ParsedInvocation},
    run_spec::RunSpec,
};

use super::{
    command::{ResolvedEnvironment, resolve_command},
    defaults::{apply_project_defaults, apply_run_spec_defaults, apply_target_defaults},
    model::ResolvedInvocation,
    needs::ResolutionNeeds,
    register_catalog,
};

pub(in crate::cli) fn resolve(invocation: ParsedInvocation) -> Result<ResolvedInvocation> {
    let current_dir = env::current_dir()?;
    resolve_from(invocation, &current_dir)
}

pub(super) fn resolve_from(
    invocation: ParsedInvocation,
    current_dir: &Path,
) -> Result<ResolvedInvocation> {
    let ParsedInvocation {
        ui: _,
        command,
        project: requested_project,
        target_spec: requested_target,
        run_spec: requested_run_spec,
        svd_paths,
    } = invocation;

    let mut command = match command {
        Command::GenerateCompletions(arguments) => {
            reject_configuration_roots(
                requested_project.as_ref(),
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "tooling commands do not accept --project, --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::GenerateCompletions(arguments));
        }
        Command::GenerateManpage(arguments) => {
            reject_configuration_roots(
                requested_project.as_ref(),
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "tooling commands do not accept --project, --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::GenerateManpage(arguments));
        }
        Command::ProjectInit(arguments) => {
            reject_configuration_roots(
                requested_project.as_ref(),
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project init does not accept --project, --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectInit(arguments));
        }
        Command::SymbolCorrelate(arguments) => {
            reject_configuration_roots(
                requested_project.as_ref(),
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "symbols correlate consumes only its explicit --from and --to artifacts",
            )?;
            return Ok(ResolvedInvocation::SymbolCorrelate(arguments));
        }
        Command::SymbolLineage(arguments) => {
            reject_configuration_roots(
                requested_project.as_ref(),
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "symbols lineage consumes only its explicit --revision artifacts",
            )?;
            return Ok(ResolvedInvocation::SymbolLineage(arguments));
        }
        command => command,
    };

    // An explicit target without an explicit project selects standalone
    // target/backend development and deliberately disables project discovery.
    // When both are present, the target is a project-scoped override.
    let project_path = if requested_project.is_some() || requested_target.is_some() {
        requested_project
    } else {
        ProjectSpec::discover_from(current_dir)?
    };
    command = match command {
        Command::ProjectConfigure(arguments) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project configure does not accept --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectConfigure {
                arguments,
                project_path: required_project_path(
                    project_path,
                    "project configure requires --project or a discovered manifest",
                )?,
            });
        }
        Command::ProjectInputsInit(arguments) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project inputs init does not accept --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectInputsInit {
                arguments,
                project_path: required_project_path(
                    project_path,
                    "project inputs init requires --project or a discovered manifest",
                )?,
            });
        }
        Command::ProjectCacheStats(_) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project cache stats accepts a project manifest, not --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectCacheStats {
                project_path: required_regular_project_path(
                    project_path,
                    "project cache stats requires --project or a discovered manifest",
                )?,
            });
        }
        Command::ProjectCacheGc(arguments) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project cache gc accepts a project manifest, not --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectCacheGc {
                arguments,
                project_path: required_regular_project_path(
                    project_path,
                    "project cache gc requires --project or a discovered manifest",
                )?,
            });
        }
        Command::ProjectCacheCompact(arguments) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project cache compact accepts a project manifest, not --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectCacheCompact {
                arguments,
                project_path: required_regular_project_path(
                    project_path,
                    "project cache compact requires --project or a discovered manifest",
                )?,
            });
        }
        Command::ProjectBrowse(_) => {
            reject_target_overrides(
                requested_target.as_ref(),
                requested_run_spec.as_ref(),
                &svd_paths,
                "project browse accepts a project manifest, not --target-spec, --run-spec or --svd",
            )?;
            return Ok(ResolvedInvocation::ProjectBrowse {
                project_path: required_project_path(
                    project_path,
                    "project browse requires --project or a discovered manifest",
                )?,
            });
        }
        command => command,
    };

    let needs = ResolutionNeeds::for_command(&command);
    require_project(needs, project_path.as_ref())?;

    let (
        project,
        target_path,
        target,
        run_spec_path,
        run_spec,
        memory_map,
        resolved_svd_paths,
        mut svd,
        explicit_context,
    ) = if let Some(manifest) = project_path.as_deref() {
        let session = ProjectSession::open_with(
            manifest,
            ProjectSessionOptions {
                target_spec: requested_target,
                run_spec: requested_run_spec,
                svd_paths,
                load_run_spec: needs.run_spec,
                authenticate_review_context: needs.review_context,
                load_memory_map: needs.memory_map,
                load_register_catalog: needs.register_catalog,
                invocation_directory: Some(current_dir.to_owned()),
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
            session.explicit_context,
        )
    } else {
        let target_path = requested_target
            .ok_or("missing --project or --target-spec, and no vendor-project.toml was found")
            .map_err(crate::Error::invalid)?;
        let target = TargetSpec::load(&target_path)?;
        let run_spec_path = needs.run_spec.then_some(requested_run_spec).flatten();
        let run_spec = run_spec_path.as_deref().map(RunSpec::load).transpose()?;
        let memory_map = None;
        let svd = register_catalog::load(needs.register_catalog, &svd_paths, None)?;
        (
            None,
            target_path,
            target,
            run_spec_path,
            run_spec,
            memory_map,
            svd_paths,
            svd,
            Default::default(),
        )
    };
    let svd_paths = resolved_svd_paths;
    if needs.backend {
        target.require_available_backend()?;
    }
    if needs.requires_knowledge_provider(target.knowledge_provider.is_some()) {
        target.require_available_knowledge_provider()?;
    }
    trace_resolved_target(&command, project_path.as_deref(), project.as_ref(), &target);

    if let Some(run_spec) = &run_spec {
        apply_run_spec_defaults(&mut command, run_spec);
    }
    apply_target_defaults(&mut command, &target);
    apply_project_defaults(&mut command, project.as_ref(), memory_map.as_ref())?;

    if let Some(memory_map) = &memory_map {
        svd.regions.extend(memory_map.resolved_mmio_regions()?);
        svd.regions
            .sort_by_key(|region| (region.start, region.end, region.name.clone()));
        svd.regions.dedup();
    }
    if needs.mmio_map && svd.regions.is_empty() {
        return Err(crate::Error::invalid(
            "command requires an MMIO region; add memory-map to the project",
        ));
    }

    resolve_command(
        command,
        ResolvedEnvironment {
            invocation_directory: current_dir.to_owned(),
            project_path,
            project,
            target_path,
            target,
            run_spec_path,
            run_spec,
            memory_map,
            svd_paths,
            svd,
            explicit_context,
        },
    )
}

fn reject_configuration_roots(
    project: Option<&PathBuf>,
    target: Option<&PathBuf>,
    run_spec: Option<&PathBuf>,
    svd_paths: &[PathBuf],
    message: &str,
) -> Result<()> {
    if project.is_some() || target.is_some() || run_spec.is_some() || !svd_paths.is_empty() {
        return Err(crate::Error::invalid(message));
    }
    Ok(())
}

fn reject_target_overrides(
    target: Option<&PathBuf>,
    run_spec: Option<&PathBuf>,
    svd_paths: &[PathBuf],
    message: &str,
) -> Result<()> {
    if target.is_some() || run_spec.is_some() || !svd_paths.is_empty() {
        return Err(crate::Error::invalid(message));
    }
    Ok(())
}

fn required_project_path(path: Option<PathBuf>, message: &str) -> Result<PathBuf> {
    path.ok_or_else(|| crate::Error::invalid(message))
}

fn required_regular_project_path(path: Option<PathBuf>, message: &str) -> Result<PathBuf> {
    let path = required_project_path(path, message)?;
    let metadata = fs::metadata(&path).map_err(|error| {
        crate::Error::invalid(format!(
            "project manifest {} is unavailable: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(crate::Error::invalid(format!(
            "project manifest {} is not a regular file",
            path.display()
        )));
    }
    Ok(path)
}

fn require_project(needs: ResolutionNeeds, project: Option<&PathBuf>) -> Result<()> {
    if needs.project && project.is_none() {
        let message = if needs.knowledge_provider {
            "knowledge-provider commands require a project manifest and a compatible provider selection"
        } else {
            "project/workspace commands require a project manifest"
        };
        return Err(crate::Error::invalid(message));
    }
    Ok(())
}

fn trace_resolved_target(
    command: &Command,
    project_path: Option<&Path>,
    project: Option<&ProjectSpec>,
    target: &TargetSpec,
) {
    if matches!(
        command,
        Command::ProjectDoctor(_) | Command::ProjectStatus(_)
    ) {
        return;
    }
    if let Some(project) = project {
        tracing::info!(
            project.id = %project.id,
            project.manifest = %project_path.map_or_else(
                || "<unavailable>".to_owned(),
                |path| path.display().to_string(),
            ),
            "loaded project"
        );
    }
    tracing::info!(
        target.id = %target.id,
        target.knowledge_provider = target.knowledge_provider.as_deref().unwrap_or("-"),
        target.architecture = target.architecture.label(),
        target.calling_convention = target.calling_convention.label(),
        target.endianness = target.endianness.label(),
        target.pointer_width,
        target.rust_target = %target.rust_target,
        "resolved target"
    );
}
