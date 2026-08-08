//! Command parsing and dispatch for the Vendor Binary Workbench.

mod args;
mod arguments;
mod commands;
mod generated_output;
mod output;
mod resolver;
mod ui;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
};

use crate::*;
use args::{Command, CommandArguments, Invocation};
pub(crate) use arguments::*;
use resolver::apply_run_spec_defaults;

pub(crate) fn output_line(arguments: std::fmt::Arguments<'_>) {
    output::line(arguments);
}

pub(crate) fn run() -> Result<bool> {
    let Invocation {
        ui,
        command,
        project,
        target_spec,
        run_spec,
        mut svd_paths,
        arguments: mut command_arguments,
    } = Invocation::parse(env::args().skip(1))?;
    ui::init(&ui)?;
    output::init(ui.format);
    if command == Command::ProjectInit {
        if project.is_some() || target_spec.is_some() || run_spec.is_some() || !svd_paths.is_empty()
        {
            return Err(
                "project init does not accept --project, --target-spec, --run-spec or --svd".into(),
            );
        }
        return commands::run_project_init(command_arguments);
    }
    let project_path = if project.is_some() || target_spec.is_some() {
        project
    } else {
        ProjectSpec::discover_from(&env::current_dir()?)?
    };
    if command == Command::ProjectConfigure {
        if target_spec.is_some() || run_spec.is_some() || !svd_paths.is_empty() {
            return Err(
                "project configure does not accept --target-spec, --run-spec or --svd".into(),
            );
        }
        return commands::run_project_configure(
            command_arguments,
            project_path
                .as_deref()
                .ok_or("project configure requires --project or a discovered manifest")?,
        );
    }
    let project = project_path.as_deref().map(ProjectSpec::load).transpose()?;
    if matches!(
        command,
        Command::ProjectDoctor
            | Command::ProjectStatus
            | Command::ProjectConfigure
            | Command::ProjectBuild
            | Command::ProjectCheck
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
    let target_spec = target_spec
        .or_else(|| project.as_ref().map(|project| project.target_spec.clone()))
        .ok_or("missing --project or --target-spec, and no vendor-project.toml was found")?;
    let mut target = TargetSpec::load(&target_spec)?;
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
    if !matches!(command, Command::ProjectDoctor | Command::ProjectStatus) {
        if let Some(project) = &project {
            tracing::info!(
                project.id = %project.id,
                project.manifest = %project_path
                    .as_deref()
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
    let run_spec_path = if command.uses_run_spec() {
        run_spec.or_else(|| {
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
    if command == Command::VerifyInventory
        && let CommandArguments::VerifyInventory(arguments) = &mut command_arguments
    {
        if !arguments.no_profiles && arguments.profiles.is_none() {
            arguments.profiles.clone_from(&target.profiles);
        }
        if !arguments.no_dispositions && arguments.dispositions.is_none() {
            arguments.dispositions.clone_from(&target.dispositions);
        }
    }
    if matches!(command, Command::VerifyInventory | Command::VerifyEvidence) {
        match &mut command_arguments {
            CommandArguments::VerifyInventory(arguments)
                if !arguments.no_evidence_baseline && arguments.evidence_baseline.is_none() =>
            {
                arguments
                    .evidence_baseline
                    .clone_from(&target.evidence_baseline);
            }
            CommandArguments::VerifyEvidence(arguments)
                if !arguments.no_evidence_baseline && arguments.evidence_baseline.is_none() =>
            {
                arguments
                    .evidence_baseline
                    .clone_from(&target.evidence_baseline);
            }
            _ => {}
        }
    }
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
    if command == Command::DiscoverMmio
        && let Some(memory_map) = &memory_map
        && let CommandArguments::MmioDiscover(arguments) = &mut command_arguments
        && arguments.range.is_empty()
    {
        for (name, start, end) in memory_map.mmio_ranges()? {
            arguments
                .range
                .push(format!("{name}={start:#010x}..{end:#010x}"));
        }
    }
    if command == Command::DiscoverMmio
        && let CommandArguments::MmioDiscover(arguments) = &mut command_arguments
        && arguments.json_report.is_none()
        && let Some(path) = project
            .as_ref()
            .and_then(|project| project.registers.as_ref())
            .map(|registers| &registers.facts)
    {
        arguments.json_report = Some(path.clone());
    }
    if command == Command::InterfaceDiscover
        && let CommandArguments::InterfaceDiscover(arguments) = &mut command_arguments
        && arguments.json_report.is_none()
        && let Some(path) = project
            .as_ref()
            .and_then(|project| project.interfaces.as_ref())
            .map(|interfaces| &interfaces.facts)
    {
        arguments.json_report = Some(path.clone());
    }
    let mut svd = if command.uses_register_catalog() {
        MmioRegisterMap::load_all(&svd_paths)?
    } else {
        MmioRegisterMap::load_all(&[])?
    };
    if command.uses_register_catalog()
        && let Some(paths) = project
            .as_ref()
            .and_then(|project| project.registers.as_ref())
        && paths.model.is_file()
        && crate::registers::RegisterModel::is_model_file(&paths.model)?
    {
        let model = crate::registers::RegisterModel::load(&paths.model)?;
        let (model_svd, _) = model.render_svd()?;
        svd.merge(MmioRegisterMap::parse(&model_svd)?)?;
    }
    if let Some(memory_map) = &memory_map {
        svd.windows.extend(memory_map.mmio_windows()?);
        svd.windows.sort_by_key(|window| (window.start, window.end));
        svd.windows.dedup();
    }
    if command.requires_mmio_map() && svd.windows.is_empty() {
        return Err("command requires an MMIO region; add memory-map to the project".into());
    }
    if matches!(command, Command::ProjectDoctor | Command::ProjectStatus) {
        let context = commands::ProjectContext {
            project_path: project_path
                .as_deref()
                .expect("project inspection requires a manifest path"),
            project: project
                .as_ref()
                .expect("project inspection requires a loaded project"),
            target_path: &target_spec,
            target: &target,
            run_spec_path: run_spec_path.as_deref(),
            run_spec: run_spec.as_ref(),
            memory_map: memory_map.as_ref(),
            svd_paths: &svd_paths,
            svd: &svd,
        };
        return if command == Command::ProjectDoctor {
            commands::run_project_doctor(command_arguments, context)
        } else {
            commands::run_project_status(command_arguments, context)
        };
    }
    if matches!(command, Command::ProjectBuild | Command::ProjectCheck) {
        return commands::run_project_pipeline(
            command,
            command_arguments,
            project
                .as_ref()
                .expect("project pipeline requires a loaded project"),
            run_spec.as_ref(),
            memory_map.as_ref(),
            &svd,
            &target,
        );
    }
    if command == Command::ProjectPublish {
        return commands::run_project_publication(
            command_arguments,
            project
                .as_ref()
                .expect("project publication requires a loaded project"),
            memory_map.as_ref(),
        );
    }
    if matches!(
        command,
        Command::FunctionInitPack | Command::FunctionValidate | Command::FunctionReview
    ) {
        return commands::run_function_pack_command(
            command,
            command_arguments,
            project
                .as_ref()
                .expect("function pack commands require a loaded project"),
            &target,
        );
    }
    if command == Command::SymbolInventory {
        let run_spec = run_spec
            .as_ref()
            .ok_or("symbols inventory requires a run spec with artifact bindings")?;
        return commands::run_symbol_inventory(command_arguments, run_spec);
    }
    if command == Command::InterfaceDiscover {
        let run_spec = run_spec
            .as_ref()
            .ok_or("interfaces discover requires a run spec with artifact bindings")?;
        return commands::run_interface_discovery(command_arguments, run_spec);
    }
    if matches!(
        command,
        Command::InterfaceInitPack | Command::InterfaceValidate
    ) {
        return commands::run_interface_pack_command(
            command,
            command_arguments,
            project
                .as_ref()
                .expect("interface pack commands require a loaded project"),
            &target,
        );
    }
    if matches!(
        command,
        Command::RegisterInitModel
            | Command::RegisterImportSvd
            | Command::RegisterValidate
            | Command::RegisterReview
            | Command::RegisterExportSvd
            | Command::RegisterGeneratePac
            | Command::RegisterGenerateBindings
    ) {
        return commands::run_register_command(
            command,
            command_arguments,
            project
                .as_ref()
                .expect("register commands require a loaded project"),
            memory_map.as_ref(),
        );
    }
    if command == Command::BuildIr {
        return commands::run_ir_build(
            command_arguments,
            project
                .as_ref()
                .expect("project IR build requires a loaded project"),
            run_spec
                .as_ref()
                .ok_or("ir build requires a run spec with source artifact bindings")?,
            &svd,
            &target,
        );
    }
    commands::run(command, command_arguments, &svd, &target)
}

pub(crate) fn finish_output() -> Result<()> {
    output::finish()
}
