//! Creation and verification of caller-owned project input bindings.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use open_radio_vendor_backend_riscv::artifact::ArtifactContainerKind;
use serde::Serialize;

use super::Result;
use crate::{
    artifact,
    cli::{ProjectInputBinding, ProjectInputsInitArgs},
    project::ProjectSpec,
    run_spec::{InputRole, RunSpec},
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
    next_command: String,
}

#[derive(Serialize)]
struct InputBindingReport {
    role: String,
    path: String,
    container: &'static str,
}

struct ResolvedBinding {
    role: InputRole,
    path: PathBuf,
    container: ArtifactContainerKind,
}

pub(super) fn run(arguments: ProjectInputsInitArgs, manifest: &Path) -> Result<bool> {
    let project = ProjectSpec::load(manifest)?;
    let output = arguments.output.unwrap_or_else(|| {
        manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("local.toml")
    });
    let known_sources = known_sources(&project);
    let required_roles = required_roles(&project, &known_sources);
    let bindings = resolve_bindings(arguments.bind, &known_sources, &required_roles)?;
    reject_artifact_output_alias(&output, &bindings)?;
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
        schema: 1,
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
        next_command: format!(
            "cargo vendor-binary-workbench project doctor --project {}",
            manifest.display()
        ),
    };
    crate::cli::output::render_report(&report, || print_human(&report));
    Ok(succeeded)
}

fn known_sources(project: &ProjectSpec) -> BTreeSet<String> {
    let mut sources = project
        .ir_profiles
        .iter()
        .flat_map(|profile| profile.sources.iter().cloned())
        .collect::<BTreeSet<_>>();
    if let Some(workspace) = &project.verification {
        sources.extend(
            workspace
                .suites
                .iter()
                .flat_map(|suite| suite.vendor.iter().map(|vendor| vendor.source.to_string())),
        );
        sources.extend(
            workspace
                .suites
                .iter()
                .flat_map(|suite| suite.auxiliary_sources.iter().map(ToString::to_string)),
        );
    }
    sources
}

fn required_roles(project: &ProjectSpec, known_sources: &BTreeSet<String>) -> BTreeSet<InputRole> {
    let mut roles = known_sources
        .iter()
        .map(|source| InputRole::SourceArtifact(source.parse().expect("validated project source")))
        .collect::<BTreeSet<_>>();
    if let Some(workspace) = &project.verification {
        for suite in &workspace.suites {
            roles.insert(suite.rust_artifact_role.clone());
            if let Some(role) = &suite.rust_companion_role {
                roles.insert(role.clone());
            }
        }
    }
    roles
}

fn resolve_bindings(
    bindings: Vec<ProjectInputBinding>,
    known_sources: &BTreeSet<String>,
    required_roles: &BTreeSet<InputRole>,
) -> Result<Vec<ResolvedBinding>> {
    let mut roles = BTreeSet::new();
    let mut resolved = Vec::with_capacity(bindings.len());
    for binding in bindings {
        if binding.role != InputRole::Companion && !roles.insert(binding.role.clone()) {
            return Err(crate::Error::invalid(format!(
                "duplicate project input role {}",
                binding.role
            )));
        }
        if let Some(source) = binding.role.qualified_source_id()
            && !known_sources.is_empty()
            && !known_sources.contains(source)
        {
            return Err(crate::Error::invalid(format!(
                "input role {} references source {source:?}, which is absent from project analysis and verification suites",
                binding.role
            )));
        }
        let path = fs::canonicalize(&binding.path).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot resolve project input {}: {error}",
                binding.path.display()
            ))
        })?;
        validate_renderable_path(&path)?;
        let inventory = artifact::inspect_artifact(&path)?;
        let expected = if binding.role.expects_archive() {
            ArtifactContainerKind::Archive
        } else {
            ArtifactContainerKind::Elf32
        };
        if inventory.container != expected {
            return Err(crate::Error::invalid(format!(
                "project input {} requires {}, but {} is {}",
                binding.role,
                expected.label(),
                path.display(),
                inventory.container.label()
            )));
        }
        resolved.push(ResolvedBinding {
            role: binding.role,
            path,
            container: inventory.container,
        });
    }
    for role in required_roles {
        if !resolved.iter().any(|binding| &binding.role == role) {
            return Err(crate::Error::invalid(format!(
                "project requires --bind {role}=PATH"
            )));
        }
    }
    resolved.sort_by(|left, right| {
        left.role
            .cmp(&right.role)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(resolved)
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
    let staging = parent.join(format!(
        ".{name}.vendor-workbench-inputs-{}",
        std::process::id()
    ));
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
    outputln!("1. {}", report.next_command);
}
