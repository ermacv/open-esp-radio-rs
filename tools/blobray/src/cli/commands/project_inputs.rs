//! Creation and verification of caller-owned project input bindings.

use std::{fs, path::Path};

use serde::Serialize;

use super::Result;
use crate::application::project_inputs::ResolvedBinding;
use crate::{
    application::{ExecutableAction, FollowUpStep, ProjectContextRequirement},
    cli::ProjectInputsInitArgs,
    project::ProjectSpec,
    run_spec::RunSpec,
};

#[derive(Serialize)]
struct ProjectInputsReport {
    schema: u32,
    command: &'static str,
    status: &'static str,
    project: String,
    output: String,
    required_roles: Vec<String>,
    bindings: Vec<InputBindingReport>,
    next_steps: Vec<FollowUpStep>,
}

#[derive(Serialize)]
struct InputBindingReport {
    role: String,
    path: String,
    container: &'static str,
}

pub(super) fn run(arguments: ProjectInputsInitArgs, manifest: &Path) -> Result<bool> {
    let project = ProjectSpec::load(manifest)?;
    let output = arguments.output.unwrap_or_else(|| {
        manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("local.toml")
    });
    let known_sources = crate::application::project_inputs::known_sources(&project);
    let required_roles = crate::application::project_inputs::required_roles(&project);
    let bindings = crate::application::project_inputs::resolve_bindings(
        arguments
            .bind
            .into_iter()
            .map(|binding| crate::run_spec::RunInput {
                role: binding.role,
                path: binding.path,
            })
            .collect(),
        &known_sources,
        &required_roles,
    )?;
    for binding in &bindings {
        validate_renderable_path(&binding.path)?;
    }
    reject_artifact_output_alias(&output, &bindings)?;
    let next_steps = vec![FollowUpStep::command(
        "Validate the resolved project, explicit input bindings, and reviewed workspaces.",
        ExecutableAction::new(
            vec![
                "blobray".to_owned(),
                "project".to_owned(),
                "doctor".to_owned(),
                "--project".to_owned(),
                executable_path(manifest, "project manifest")?,
                "--run-spec".to_owned(),
                executable_path(&output, "run spec")?,
            ],
            std::env::current_dir()?,
            ProjectContextRequirement::RunSpec,
        )?,
    )];
    let rendered = render_run_spec(&bindings);
    let (status, succeeded) = if arguments.check {
        let current = match fs::read_to_string(&output) {
            Ok(current) => Some(current),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let matches = current.as_deref() == Some(rendered.as_str());
        (if matches { "verified" } else { "stale" }, matches)
    } else {
        if output.exists() && !arguments.force {
            return Err(crate::Error::invalid(format!(
                "refusing to overwrite existing local run spec {}; use --force or --check",
                output.display()
            )));
        }
        write_atomic(&output, &rendered)?;
        ("written", true)
    };

    let report = ProjectInputsReport {
        schema: 2,
        command: "project inputs init",
        status,
        project: project.id,
        output: output.display().to_string(),
        required_roles: required_roles
            .into_iter()
            .map(|role| role.to_string())
            .collect(),
        bindings: bindings
            .iter()
            .map(|binding| InputBindingReport {
                role: binding.role.to_string(),
                path: binding.path.display().to_string(),
                container: binding.container.label(),
            })
            .collect(),
        next_steps,
    };
    crate::cli::output::render_report(&report, || print_human(&report));
    Ok(succeeded)
}

fn reject_artifact_output_alias(output: &Path, bindings: &[ResolvedBinding]) -> Result<()> {
    if !output.exists() {
        return Ok(());
    }
    let output = fs::canonicalize(output)?;
    if let Some(binding) = bindings.iter().find(|binding| binding.path == output) {
        return Err(crate::Error::invalid(format!(
            "local run-spec output aliases input artifact {} ({})",
            binding.path.display(),
            binding.role
        )));
    }
    Ok(())
}

fn validate_renderable_path(path: &Path) -> Result<()> {
    let value = path.to_str().ok_or_else(|| {
        crate::Error::invalid(format!(
            "project input path is not valid UTF-8: {}",
            path.display()
        ))
    })?;
    if value.contains(['\n', '\r', '#']) {
        return Err(crate::Error::invalid(format!(
            "project input path cannot contain a newline or '#': {}",
            path.display()
        )));
    }
    Ok(())
}

fn render_run_spec(bindings: &[ResolvedBinding]) -> String {
    let mut output = String::from(
        "# Caller-owned artifact bindings. Keep this file untracked.\n\
         schema = 1\n",
    );
    for binding in bindings {
        output.push_str(&format!(
            "\n[[inputs]]\nrole = {:?}\npath = {:?}\n",
            binding.role.to_string(),
            binding.path.display().to_string()
        ));
    }
    output
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("local run-spec output must have a UTF-8 file name")
        .map_err(crate::Error::invalid)?;
    let staging = parent.join(format!(".{name}.blobray-inputs-{}", std::process::id()));
    if staging.exists() {
        return Err(crate::Error::invalid(format!(
            "project input staging path exists: {}",
            staging.display()
        )));
    }
    fs::write(&staging, contents)?;
    if let Err(error) = RunSpec::load(&staging) {
        let _ = fs::remove_file(&staging);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&staging, path) {
        let _ = fs::remove_file(&staging);
        return Err(error.into());
    }
    Ok(())
}

fn print_human(report: &ProjectInputsReport) {
    use crate::cli::{output, table};

    outputln!("{}", output::heading("Project inputs"));
    outputln!("Run spec: {}", report.output);
    outputln!(
        "\n{}",
        output::success(format!("READY — {}", report.status))
    );
    outputln!("\n{}", output::heading("Bindings"));
    outputln!(
        "{}",
        table::render(
            ["Role", "Container", "Path"],
            report.bindings.iter().map(|binding| [
                binding.role.clone(),
                binding.container.to_owned(),
                binding.path.clone(),
            ])
        )
    );
    outputln!("\n{}", output::heading("Next"));
    for (index, step) in report.next_steps.iter().enumerate() {
        outputln!("{}. {}", index + 1, step.instruction);
        for action in &step.commands {
            outputln!("   {}", action.render_posix());
        }
    }
}

fn executable_path(path: &Path, role: &str) -> Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        crate::Error::invalid(format!(
            "{role} path is not valid UTF-8 and cannot be represented in an executable action: {}",
            path.display()
        ))
    })
}
