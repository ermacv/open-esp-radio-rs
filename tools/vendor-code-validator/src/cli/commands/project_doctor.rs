//! Project configuration and local-input readiness diagnostics.

use std::path::{Path, PathBuf};

use super::super::*;
use crate::{
    interfaces::{InterfaceFacts, InterfaceWorkspace},
    memory_map::{MemoryMap, MemoryRegionKind},
    project::ProjectSpec,
    registers::{ProjectRegisterWorkspace, RegisterFacts, RegisterModel},
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

    let project_model = context.project.registers.as_ref().is_some_and(|paths| {
        paths.model.is_file() && RegisterModel::is_model_file(&paths.model).unwrap_or(false)
    });
    if context.svd_paths.is_empty() && !project_model {
        println!(
            "CAPABILITY\tregister-catalog\tnot-configured\tregisters=0\tlegacy-windows={}",
            context.svd.windows.len()
        );
    } else {
        println!(
            "CAPABILITY\tregister-catalog\tavailable\tfiles={}\tproject-model={}\tregisters={}\tcombined-mmio-windows={}",
            context.svd_paths.len(),
            if project_model { "yes" } else { "no" },
            context.svd.registers.len(),
            context.svd.windows.len()
        );
        for path in context.svd_paths {
            println!("SVD\t{}", path.display());
        }
    }

    match &context.project.registers {
        None => println!("CAPABILITY\tregister-workspace\tnot-configured"),
        Some(paths) if paths.model.is_file() => {
            let is_model_v2 = RegisterModel::is_model_file(&paths.model);
            if matches!(is_model_v2, Ok(false)) && !paths.facts.is_file() {
                warnings += 1;
                println!(
                    "CAPABILITY\tregister-workspace\tnot-generated\tformat=legacy-overlay-v1\tfacts={}\tmodel={}",
                    paths.facts.display(),
                    paths.model.display()
                );
            } else {
                match ProjectRegisterWorkspace::load(&paths.facts, &paths.model)
                    .and_then(|workspace| Ok((workspace.summary()?, workspace.format_label())))
                {
                    Ok((summary, format)) => {
                        println!(
                            "CAPABILITY\tregister-workspace\tavailable\tformat={}\tranges={}\tobserved={}\treviewed={}\tignored={}\tmanual={}\tunreviewed={}\tfields={}\tfacts={}\tmodel={}\tsvd-output={}\tpac-output={}",
                            format,
                            summary.ranges,
                            summary.observed,
                            summary.reviewed,
                            summary.ignored,
                            summary.manual,
                            summary.unreviewed,
                            summary.fields,
                            paths.facts.display(),
                            paths.model.display(),
                            paths
                                .svd_output
                                .as_deref()
                                .map_or_else(|| "-".to_owned(), |path| path.display().to_string()),
                            paths.pac.as_ref().map_or_else(
                                || "-".to_owned(),
                                |pac| pac.output.display().to_string()
                            )
                        );
                    }
                    Err(error) => {
                        errors += 1;
                        println!(
                            "CAPABILITY\tregister-workspace\tinvalid\tfacts={}\tmodel={}\terror={error}",
                            paths.facts.display(),
                            paths.model.display()
                        );
                    }
                }
            }
        }
        Some(paths) if !paths.facts.is_file() => {
            warnings += 1;
            println!(
                "CAPABILITY\tregister-workspace\tnot-generated\tfacts={}\tmodel={}",
                paths.facts.display(),
                paths.model.display()
            );
        }
        Some(paths) => match RegisterFacts::load(&paths.facts) {
            Ok(facts) => {
                warnings += 1;
                println!(
                    "CAPABILITY\tregister-workspace\tmodel-not-initialized\tranges={}\tobserved={}\tfacts={}\tmodel={}",
                    facts.ranges.len(),
                    facts.registers.len(),
                    paths.facts.display(),
                    paths.model.display()
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
        Some(paths) => match InterfaceFacts::load(&paths.facts) {
            Err(error) => {
                errors += 1;
                println!(
                    "CAPABILITY\tinterface-workspace\tinvalid-facts\tfacts={}\terror={error}",
                    paths.facts.display()
                );
            }
            Ok(facts) => match paths.pack.as_deref() {
                None => {
                    println!(
                        "CAPABILITY\tinterface-facts\tavailable\ttables={}\tobserved-slots={}\tfacts={}",
                        facts.tables.len(),
                        facts.observed_slots(),
                        paths.facts.display()
                    );
                }
                Some(pack) if !pack.is_file() => {
                    warnings += 1;
                    println!(
                        "CAPABILITY\tinterface-workspace\tpack-not-initialized\ttables={}\tobserved-slots={}\tfacts={}\tpack={}",
                        facts.tables.len(),
                        facts.observed_slots(),
                        paths.facts.display(),
                        pack.display()
                    );
                }
                Some(pack) => match InterfaceWorkspace::load(
                    &paths.facts,
                    pack,
                    &paths.semantic_catalogs,
                    context.target.calling_convention.label(),
                ) {
                    Ok(workspace) => {
                        let summary = workspace.summary();
                        println!(
                            "CAPABILITY\tinterface-workspace\tavailable\tfact-tables={}\tobserved-slots={}\treviewed-anchors={}\tignored-anchors={}\tunreviewed-anchors={}\treviewed-slots={}\tignored-slots={}\tunreviewed-slots={}\tsemantic-links={}\tsemantic-operations={}\tfacts={}\tpack={}",
                            summary.fact_tables,
                            summary.observed_slots,
                            summary.reviewed_anchors,
                            summary.ignored_anchors,
                            summary.unreviewed_anchors,
                            summary.reviewed_slots,
                            summary.ignored_slots,
                            summary.unreviewed_slots,
                            summary.semantic_links,
                            summary.semantic_operations,
                            paths.facts.display(),
                            pack.display()
                        );
                    }
                    Err(error) => {
                        errors += 1;
                        println!(
                            "CAPABILITY\tinterface-workspace\tinvalid\tfacts={}\tpack={}\terror={error}",
                            paths.facts.display(),
                            pack.display()
                        );
                    }
                },
            },
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
