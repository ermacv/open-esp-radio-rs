//! Command parsing and dispatch for the validator binary.

mod args;
mod commands;
mod generated_output;
mod json;

use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
};

use crate::*;
use args::{Command, Invocation};

pub(crate) fn usage() {
    eprintln!(
        "usage: vendor-code-validator GROUP COMMAND [--project PATH | --target-spec PATH] [--run-spec PATH] [OPTIONS]\n\nworkflows:\n  project    doctor | build | check\n  functions  init-pack | validate | review\n  symbols    inventory\n  interfaces discover | init-pack | validate\n  registers  init-model | init-overlay | import-svd | validate | review | export-svd | generate-pac | generate-bindings\n  inspect    analyze | trace | compare\n  mmio       discover\n  ir         export | build\n  reference  generate | generate-batch\n  driver     generate\n  execute    run | compare\n  verify     profiles | source | inventory | contract channel | contract rf-init\n  image      audit-targets\n\nA project composes a target spec, optional local run bindings, a memory map and SVD catalogs.\nWithout an explicit configuration root, the nearest vendor-validator.toml is used.\nDirect --target-spec/--run-spec invocation remains available for compatibility. Legacy flat command names are temporarily accepted."
    );
}

pub(crate) fn run() -> Result<bool> {
    let Invocation {
        command,
        project,
        target_spec,
        run_spec,
        mut svd_paths,
        arguments: mut filtered,
    } = Invocation::parse(env::args().skip(1))?;
    let project_path = if project.is_some() || target_spec.is_some() {
        project
    } else {
        ProjectSpec::discover_from(&env::current_dir()?)
    };
    let project = project_path.as_deref().map(ProjectSpec::load).transpose()?;
    if matches!(
        command,
        Command::ProjectDoctor
            | Command::ProjectBuild
            | Command::ProjectCheck
            | Command::FunctionInitPack
            | Command::FunctionValidate
            | Command::FunctionReview
            | Command::InterfaceInitPack
            | Command::InterfaceValidate
            | Command::RegisterInitOverlay
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
    let target_spec = target_spec
        .or_else(|| project.as_ref().map(|project| project.target_spec.clone()))
        .ok_or("missing --project or --target-spec, and no vendor-validator.toml was found")?;
    let target = TargetSpec::load(&target_spec)?;
    if command.requires_backend() {
        target.require_available_backend()?;
    }
    if command.requires_harness() {
        target.require_available_harness()?;
    }
    if command != Command::ProjectDoctor {
        if let Some(project) = &project {
            eprintln!(
                "PROJECT\tid={}\tmanifest={}",
                project.id,
                project_path
                    .as_deref()
                    .expect("a loaded project has a manifest path")
                    .display()
            );
        }
        eprintln!(
            "TARGET\tid={}\tharness={}\tarchitecture={}\tcalling-convention={}\tendianness={}\tpointer-width={}\trust-target={}",
            target.id,
            target.harness.as_deref().unwrap_or("-"),
            target.architecture.label(),
            target.calling_convention.label(),
            target.endianness.label(),
            target.pointer_width,
            target.rust_target,
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
        run_spec.append_defaults(
            &mut filtered,
            |role| command.accepts_run_input_role(role),
            |role, arguments| command.input_role_is_overridden(role, arguments),
        );
    }
    if svd_paths.is_empty() {
        svd_paths = project
            .as_ref()
            .filter(|project| project.svd_configured)
            .map(|project| project.svd_paths.clone())
            .unwrap_or_else(|| target.svd_paths.clone());
    }
    if command == Command::VerifyAll {
        append_default_path(&mut filtered, "--profiles", target.profiles.as_deref());
        append_default_path(
            &mut filtered,
            "--dispositions",
            target.dispositions.as_deref(),
        );
        append_default_path(
            &mut filtered,
            "--evidence-baseline",
            target.evidence_baseline.as_deref(),
        );
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
        && !filtered.iter().any(|argument| argument == "--range")
        && let Some(memory_map) = &memory_map
    {
        for (name, start, end) in memory_map.mmio_ranges()? {
            filtered.push("--range".to_owned());
            filtered.push(format!("{name}={start:#010x}..{end:#010x}"));
        }
    }
    if command == Command::DiscoverMmio
        && !filtered.iter().any(|argument| argument == "--json-report")
        && let Some(path) = project
            .as_ref()
            .and_then(|project| project.registers.as_ref())
            .map(|registers| &registers.facts)
    {
        filtered.push("--json-report".to_owned());
        filtered.push(path.display().to_string());
    }
    if command == Command::InterfaceDiscover
        && !filtered.iter().any(|argument| argument == "--json-report")
        && let Some(path) = project
            .as_ref()
            .and_then(|project| project.interfaces.as_ref())
            .map(|interfaces| &interfaces.facts)
    {
        filtered.push("--json-report".to_owned());
        filtered.push(path.display().to_string());
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
        return Err("command requires an MMIO region; add memory-map to the project or an SVD address-window extension".into());
    }
    if command == Command::ProjectDoctor {
        return commands::run_project_doctor(
            filtered,
            commands::ProjectDoctorContext {
                project_path: project_path
                    .as_deref()
                    .expect("project doctor requires a manifest path"),
                project: project
                    .as_ref()
                    .expect("project doctor requires a loaded project"),
                target_path: &target_spec,
                target: &target,
                run_spec_path: run_spec_path.as_deref(),
                run_spec: run_spec.as_ref(),
                memory_map: memory_map.as_ref(),
                svd_paths: &svd_paths,
                svd: &svd,
            },
        );
    }
    if matches!(command, Command::ProjectBuild | Command::ProjectCheck) {
        return commands::run_project_pipeline(
            command,
            filtered,
            project
                .as_ref()
                .expect("project pipeline requires a loaded project"),
            run_spec.as_ref(),
            memory_map.as_ref(),
            &svd,
            &target,
        );
    }
    if matches!(
        command,
        Command::FunctionInitPack | Command::FunctionValidate | Command::FunctionReview
    ) {
        return commands::run_function_pack_command(
            command,
            filtered,
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
        return commands::run_symbol_inventory(filtered, run_spec);
    }
    if command == Command::InterfaceDiscover {
        let run_spec = run_spec
            .as_ref()
            .ok_or("interfaces discover requires a run spec with artifact bindings")?;
        return commands::run_interface_discovery(filtered, run_spec);
    }
    if matches!(
        command,
        Command::InterfaceInitPack | Command::InterfaceValidate
    ) {
        return commands::run_interface_pack_command(
            command,
            filtered,
            project
                .as_ref()
                .expect("interface pack commands require a loaded project"),
            &target,
        );
    }
    if matches!(
        command,
        Command::RegisterInitOverlay
            | Command::RegisterInitModel
            | Command::RegisterImportSvd
            | Command::RegisterValidate
            | Command::RegisterReview
            | Command::RegisterExportSvd
            | Command::RegisterGeneratePac
            | Command::RegisterGenerateBindings
    ) {
        return commands::run_register_command(
            command,
            filtered,
            project
                .as_ref()
                .expect("register commands require a loaded project"),
            memory_map.as_ref(),
        );
    }
    if command == Command::BuildIr {
        return commands::run_ir_build(
            filtered,
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
    commands::run(command, filtered, &svd, &target)
}

fn append_default_path(arguments: &mut Vec<String>, option: &str, path: Option<&std::path::Path>) {
    let disable = format!("--no-{}", option.trim_start_matches("--"));
    if arguments
        .iter()
        .any(|argument| argument == option || argument == &disable)
    {
        return;
    }
    if let Some(path) = path {
        arguments.push(option.to_owned());
        arguments.push(path.display().to_string());
    }
}
