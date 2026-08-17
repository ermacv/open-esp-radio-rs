//! Safe scaffolding for a new generic vendor-binary project.

use std::{fs, path::Path};

use serde::Serialize;

use super::Result;
use crate::cli::ProjectInitArgs;
use crate::{
    MemoryMap, TargetSpec,
    project::{DEFAULT_PROJECT_MANIFEST, ProjectSpec},
    registers::{
        FactRange, RegisterFacts, RegisterModel, import_svd_model, init_register_model,
        validate_register_model_memory_map,
    },
};

mod options;
mod render;
#[cfg(test)]
mod tests;

use options::{Options, resolve_options};
use render::{
    render_chip, render_ecosystem, render_manifest, render_memory, render_readme,
    render_register_api, render_run_spec, render_target,
};

#[derive(Serialize)]
struct ProjectInitReport {
    schema_version: u32,
    command: &'static str,
    status: &'static str,
    id: String,
    architecture: &'static str,
    sources: usize,
    mmio_regions: usize,
    imported_svd: bool,
    path: String,
    next_command: String,
}

pub(super) fn run(arguments: ProjectInitArgs) -> Result<bool> {
    let options = resolve_options(arguments)?;
    create_project(&options)?;
    let report = ProjectInitReport {
        schema_version: 1,
        command: "project init",
        status: "created",
        id: options.id.clone(),
        architecture: "riscv32",
        sources: options.sources.len(),
        mmio_regions: options.ranges.len(),
        imported_svd: options.import_svd.is_some(),
        path: options.directory.display().to_string(),
        next_command: format!(
            "cargo blobray project doctor --project {}/{}",
            options.directory.display(),
            DEFAULT_PROJECT_MANIFEST
        ),
    };
    crate::cli::output::render_report(&report, || {
        outputln!("{}", crate::cli::output::heading("Project created"));
        outputln!("{}", crate::cli::output::success("READY FOR LOCAL INPUTS"));
        outputln!("\nProject:      {}", report.id);
        outputln!("Directory:    {}", report.path);
        outputln!("Architecture: {}", report.architecture);
        outputln!("Sources:      {}", report.sources);
        outputln!("MMIO regions: {}", report.mmio_regions);
        outputln!("Imported SVD: {}", report.imported_svd);
        outputln!("\n{}", crate::cli::output::heading("Next"));
        outputln!("1. {}", report.next_command);
        outputln!("2. blobray project inputs init --help");
    });
    Ok(true)
}

fn create_project(options: &Options) -> Result<()> {
    if options.directory.exists() {
        return Err(crate::Error::invalid(format!(
            "refusing to overwrite existing project path {}",
            options.directory.display()
        )));
    }
    let parent = options
        .directory
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = options
        .directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("project directory must have a UTF-8 final component")
        .map_err(crate::Error::invalid)?;
    let staging = parent.join(format!(
        ".{name}.vendor-project-init-{}",
        std::process::id()
    ));
    if staging.exists() {
        return Err(crate::Error::invalid(format!(
            "project init staging path exists: {}",
            staging.display()
        )));
    }
    fs::create_dir(&staging)?;
    if let Err(error) = write_project(&staging, options) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = fs::rename(&staging, &options.directory) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error.into());
    }
    Ok(())
}

fn write_project(root: &Path, options: &Options) -> Result<()> {
    fs::write(
        root.join(DEFAULT_PROJECT_MANIFEST),
        render_manifest(options),
    )?;
    fs::write(root.join("target.toml"), render_target(options))?;
    fs::write(root.join("ecosystem.toml"), render_ecosystem(options))?;
    fs::write(root.join("chip.toml"), render_chip(options))?;
    fs::write(root.join("memory.toml"), render_memory(options))?;
    fs::write(root.join("local.example.toml"), render_run_spec(options))?;
    fs::write(root.join("README.md"), render_readme(options))?;
    fs::write(root.join(".gitignore"), "/generated/\n/local.toml\n")?;
    fs::create_dir_all(root.join("registers"))?;
    fs::write(root.join("registers/api.toml"), render_register_api())?;

    let model = root.join("registers/device.toml");
    if let Some(input) = options.import_svd.as_deref() {
        import_svd_model(input, &model, "cpu")?;
    } else {
        let facts = RegisterFacts {
            ranges: options
                .ranges
                .iter()
                .map(|range| FactRange {
                    name: range.name.clone(),
                    start: range.start,
                    end: range.end,
                })
                .collect(),
            registers: Vec::new(),
        };
        init_register_model(&facts, &model, "cpu", &options.id)?;
    }

    validate_project(root)
}

fn validate_project(root: &Path) -> Result<()> {
    let project = ProjectSpec::load(&root.join(DEFAULT_PROJECT_MANIFEST))?;
    let mut target = TargetSpec::load(&root.join("target.toml"))?;
    if let Some(pack) = &project.chip_pack {
        pack.apply_to_target(&mut target)?;
    }
    target.require_available_backend()?;
    let memory = MemoryMap::load(&root.join("memory.toml"))?;
    let model = RegisterModel::load(&project.registers.as_ref().unwrap().model)?;
    let _ = model.render_svd()?;
    let _ = validate_register_model_memory_map(&model, &memory)?;
    let _ = crate::registers::validate_pac_api(project.registers.as_ref().unwrap())?;
    let _ = project.function_ir_reports()?;
    Ok(())
}
