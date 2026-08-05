//! Project configuration and local-input readiness diagnostics.

use std::path::{Path, PathBuf};

use super::super::*;
use crate::{
    memory_map::{MemoryMap, MemoryRegionKind},
    project::ProjectSpec,
    registers::{RegisterFacts, RegisterWorkspace},
    run_spec::RunSpec,
};

pub(crate) struct ProjectDoctorContext<'a> {
    pub(crate) project_path: &'a Path,
    pub(crate) project: &'a ProjectSpec,
    pub(crate) target_path: &'a Path,
    pub(crate) target: &'a TargetSpec,
    pub(crate) run_spec_path: Option<&'a Path>,
    pub(crate) run_spec: Option<&'a RunSpec>,
    pub(crate) memory_map: Option<&'a MemoryMap>,
    pub(crate) svd_paths: &'a [PathBuf],
    pub(crate) svd: &'a MmioRegisterMap,
}

pub(super) fn run(arguments: Vec<String>, context: ProjectDoctorContext<'_>) -> Result<bool> {
    if !arguments.is_empty() {
        return Err(format!("project doctor takes no command options: {arguments:?}").into());
    }

    let mut errors = 0usize;
    let mut warnings = 0usize;
    println!(
        "PROJECT\tid={}\tmanifest={}",
        context.project.id,
        context.project_path.display()
    );
    println!(
        "TARGET\tid={}\tspec={}",
        context.target.id,
        context.target_path.display()
    );

    match context.target.require_available_backend() {
        Ok(()) => println!(
            "CAPABILITY\tbackend\tavailable\tarchitecture={}\tcalling-convention={}",
            context.target.architecture.label(),
            context.target.calling_convention.label()
        ),
        Err(error) => {
            errors += 1;
            println!("CAPABILITY\tbackend\tunavailable\t{error}");
        }
    }

    match &context.target.harness {
        None => {
            println!("CAPABILITY\tharness\tnot-configured\tgeneric-analysis-only");
        }
        Some(_) => match context.target.require_available_harness() {
            Ok(harness) => println!("CAPABILITY\tharness\tavailable\tid={harness}"),
            Err(error) => {
                warnings += 1;
                println!("CAPABILITY\tharness\tunavailable\t{error}");
            }
        },
    }

    if let Some(memory_map) = context.memory_map {
        let mmio_regions = memory_map
            .regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Mmio)
            .count();
        println!(
            "CAPABILITY\tmemory-map\tavailable\tspaces={}\tregions={}\tmmio-regions={}\tdefault-space={}",
            memory_map.address_spaces.len(),
            memory_map.regions.len(),
            mmio_regions,
            memory_map.default_address_space
        );
        if mmio_regions == 0 {
            warnings += 1;
            println!("DIAGNOSTIC\twarning\tmemory map has no MMIO regions");
        }
    } else {
        warnings += 1;
        println!("CAPABILITY\tmemory-map\tnot-configured");
    }

    if context.svd_paths.is_empty() {
        println!(
            "CAPABILITY\tregister-catalog\tnot-configured\tregisters=0\tlegacy-windows={}",
            context.svd.windows.len()
        );
    } else {
        println!(
            "CAPABILITY\tregister-catalog\tavailable\tfiles={}\tregisters={}\tcombined-mmio-windows={}",
            context.svd_paths.len(),
            context.svd.registers.len(),
            context.svd.windows.len()
        );
        for path in context.svd_paths {
            println!("SVD\t{}", path.display());
        }
    }

    match &context.project.registers {
        None => println!("CAPABILITY\tregister-workspace\tnot-configured"),
        Some(paths) if !paths.facts.is_file() => {
            warnings += 1;
            println!(
                "CAPABILITY\tregister-workspace\tnot-generated\tfacts={}\toverlay={}",
                paths.facts.display(),
                paths.overlay.display()
            );
        }
        Some(paths) if !paths.overlay.is_file() => match RegisterFacts::load(&paths.facts) {
            Ok(facts) => {
                warnings += 1;
                println!(
                    "CAPABILITY\tregister-workspace\toverlay-not-initialized\tranges={}\tobserved={}\tfacts={}\toverlay={}",
                    facts.ranges.len(),
                    facts.registers.len(),
                    paths.facts.display(),
                    paths.overlay.display()
                );
            }
            Err(error) => {
                errors += 1;
                println!(
                    "CAPABILITY\tregister-workspace\tinvalid-facts\tfacts={}\terror={error}",
                    paths.facts.display()
                );
            }
        },
        Some(paths) => match RegisterWorkspace::load(&paths.facts, &paths.overlay) {
            Ok(workspace) => {
                let summary = workspace.summary();
                println!(
                    "CAPABILITY\tregister-workspace\tavailable\tranges={}\tobserved={}\treviewed={}\tignored={}\tmanual={}\tunreviewed={}\tfields={}\tfacts={}\toverlay={}",
                    summary.ranges,
                    summary.observed,
                    summary.reviewed,
                    summary.ignored,
                    summary.manual,
                    summary.unreviewed,
                    summary.fields,
                    paths.facts.display(),
                    paths.overlay.display()
                );
            }
            Err(error) => {
                errors += 1;
                println!(
                    "CAPABILITY\tregister-workspace\tinvalid\tfacts={}\toverlay={}\terror={error}",
                    paths.facts.display(),
                    paths.overlay.display()
                );
            }
        },
    }

    match &context.project.interfaces {
        None => println!("CAPABILITY\tinterface-facts\tnot-configured"),
        Some(paths) if !paths.facts.is_file() => {
            warnings += 1;
            println!(
                "CAPABILITY\tinterface-facts\tnot-generated\tfacts={}",
                paths.facts.display()
            );
        }
        Some(paths) => match load_json(&paths.facts) {
            Ok(document)
                if document
                    .get("schema_version")
                    .and_then(serde_json::Value::as_u64)
                    == Some(1)
                    && document.get("command").and_then(serde_json::Value::as_str)
                        == Some("interfaces-discover") =>
            {
                let calls = document
                    .get("calls")
                    .and_then(serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                let tables = document
                    .get("table_candidates")
                    .and_then(serde_json::Value::as_array)
                    .map_or(0, Vec::len);
                println!(
                    "CAPABILITY\tinterface-facts\tavailable\tcalls={calls}\ttables={tables}\tfacts={}",
                    paths.facts.display()
                );
            }
            Ok(_) => {
                errors += 1;
                println!(
                    "CAPABILITY\tinterface-facts\tinvalid-schema\tfacts={}",
                    paths.facts.display()
                );
            }
            Err(error) => {
                errors += 1;
                println!(
                    "CAPABILITY\tinterface-facts\tinvalid\tfacts={}\terror={error}",
                    paths.facts.display()
                );
            }
        },
    }

    let mut valid_inputs = 0usize;
    let mut input_count = 0usize;
    match (context.run_spec_path, context.run_spec) {
        (Some(path), Some(run_spec)) => {
            println!("RUN-SPEC\t{}", path.display());
            input_count = run_spec.inputs().len();
            for (role, path) in run_spec.inputs() {
                if !path.is_file() {
                    errors += 1;
                    println!(
                        "INPUT\trole={}\tstatus=missing\tpath={}",
                        role,
                        path.display()
                    );
                    continue;
                }
                match artifact::inspect_artifact(path) {
                    Ok(inventory) => {
                        valid_inputs += 1;
                        let symbol_facts = inventory.symbols().count();
                        let code_definitions = inventory
                            .symbols()
                            .filter(|(_, fact)| {
                                fact.kind == artifact::ArtifactSymbolKind::Text
                                    && fact.definition.is_definition()
                            })
                            .count();
                        let exported_definitions = inventory
                            .symbols()
                            .filter(|(_, fact)| fact.is_exported_definition())
                            .count();
                        let undefined = inventory
                            .symbols()
                            .filter(|(_, fact)| {
                                fact.definition
                                    == artifact::ArtifactSymbolDefinitionState::Undefined
                            })
                            .count();
                        if symbol_facts == 0 {
                            warnings += 1;
                        }
                        println!(
                            "INPUT\trole={}\tstatus={}\tcontainer={}\tobjects={}\tskipped-members={}\tsymbol-facts={}\tcode-definitions={}\texported-definitions={}\tundefined={}\tpath={}",
                            role,
                            if symbol_facts == 0 {
                                "readable-no-symbols"
                            } else {
                                "ready"
                            },
                            inventory.container.label(),
                            inventory.objects.len(),
                            inventory.skipped_members,
                            symbol_facts,
                            code_definitions,
                            exported_definitions,
                            undefined,
                            path.display()
                        );
                    }
                    Err(error) => {
                        errors += 1;
                        println!(
                            "INPUT\trole={}\tstatus=invalid\tpath={}\terror={}",
                            role,
                            path.display(),
                            error
                        );
                    }
                }
            }
        }
        (None, None) => {
            warnings += 1;
            println!("RUN-SPEC\tnot-configured\tartifact-bindings-unavailable");
        }
        _ => unreachable!("run-spec path and parsed contents are created together"),
    }

    println!(
        "SUMMARY\tstatus={}\terrors={}\twarnings={}\tinputs={}\tvalid-inputs={}",
        if errors == 0 { "ok" } else { "failed" },
        errors,
        warnings,
        input_count,
        valid_inputs
    );
    Ok(errors == 0)
}

fn load_json(path: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}
