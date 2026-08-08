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
    render_manifest, render_memory, render_platform, render_readme, render_run_spec, render_target,
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
            "cargo vendor-binary-workbench project doctor --project {}/{}",
            options.directory.display(),
            DEFAULT_PROJECT_MANIFEST
        ),
    };
    if !crate::cli::output::structured("project-init", &report) {
        outputln!(
            "PROJECT-INIT\tstatus=created\tid={}\tarchitecture=riscv32\tsources={}\tmmio-regions={}\timported-svd={}\tpath={}",
            report.id,
            report.sources,
            report.mmio_regions,
            report.imported_svd,
            report.path
        );
        outputln!("PROJECT-NEXT\tcommand={}", report.next_command);
    }
    Ok(true)
}

fn create_project(options: &Options) -> Result<()> {
    if options.directory.exists() {
        return Err(format!(
            "refusing to overwrite existing project path {}",
            options.directory.display()
        )
        .into());
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
        .ok_or("project directory must have a UTF-8 final component")?;
    let staging = parent.join(format!(
        ".{name}.vendor-project-init-{}",
        std::process::id()
    ));
    if staging.exists() {
        return Err(format!("project init staging path exists: {}", staging.display()).into());
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
    fs::write(root.join("target.spec"), render_target(options))?;
    fs::write(root.join("platform.toml"), render_platform(options))?;
    fs::write(root.join("memory.toml"), render_memory(options))?;
    fs::write(root.join("run.spec.example"), render_run_spec(options))?;
    fs::write(root.join("README.md"), render_readme(options))?;
    fs::write(root.join(".gitignore"), "/generated/\n/local.run\n")?;

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
    let mut target = TargetSpec::load(&root.join("target.spec"))?;
    if let Some(pack) = &project.platform_pack {
        pack.apply_to_target(&mut target)?;
    }
    target.require_available_backend()?;
    let memory = MemoryMap::load(&root.join("memory.toml"))?;
    let model = RegisterModel::load(&project.registers.as_ref().unwrap().model)?;
    let _ = model.render_svd()?;
    let _ = validate_register_model_memory_map(&model, &memory)?;
    let _ = project.function_ir_reports()?;
    Ok(())
}
