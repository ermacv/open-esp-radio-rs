//! Project configuration and local-input readiness diagnostics.

use super::super::*;
use crate::{
    interfaces::{InterfaceFacts, InterfaceWorkspace},
    memory_map::MemoryRegionKind,
    registers::{
        ProjectRegisterWorkspace, RegisterFacts, RegisterModel, inspect_register_review_ir,
        validate_pac_api, validate_register_evidence, validate_register_lints,
    },
};

pub(super) fn run(context: super::ProjectContext<'_>) -> Result<bool> {
    let mut errors = 0usize;
    let mut warnings = 0usize;
    outputln!(
        "PROJECT\tid={}\tmanifest={}",
        context.project.id,
        context.project_path.display()
    );
    outputln!(
        "TARGET\tid={}\tspec={}",
        context.target.id,
        context.target_path.display()
    );
    match &context.project.platform_pack {
        Some(pack) => outputln!(
            "CAPABILITY\tplatform-pack\tavailable\tid={}\tharness={}\tsemantic-catalogs={}\tsemantic-operations={}\tpath={}",
            pack.id,
            pack.harness.as_deref().unwrap_or("-"),
            pack.semantic_catalogs.len(),
            pack.semantic_operations,
            pack.path.display(),
        ),
        None => outputln!("CAPABILITY\tplatform-pack\tnot-configured\tgeneric-target-only"),
    }

    match context.target.require_available_backend() {
        Ok(()) => outputln!(
            "CAPABILITY\tbackend\tavailable\tarchitecture={}\tcalling-convention={}",
            context.target.architecture.label(),
            context.target.calling_convention.label()
        ),
        Err(error) => {
            errors += 1;
            outputln!("CAPABILITY\tbackend\tunavailable\t{error}");
        }
    }

    match &context.target.harness {
        None => {
            outputln!("CAPABILITY\tharness\tnot-configured\tgeneric-analysis-only");
        }
        Some(_) => match context.target.require_available_harness() {
            Ok(harness) => outputln!("CAPABILITY\tharness\tavailable\tid={harness}"),
            Err(error) => {
                warnings += 1;
                outputln!("CAPABILITY\tharness\tunavailable\t{error}");
            }
        },
    }

    if let Some(memory_map) = context.memory_map {
        let mmio_regions = memory_map
            .regions
            .iter()
            .filter(|region| region.kind == MemoryRegionKind::Mmio)
            .count();
        outputln!(
            "CAPABILITY\tmemory-map\tavailable\tspaces={}\tregions={}\tmmio-regions={}\tdefault-space={}",
            memory_map.address_spaces.len(),
            memory_map.regions.len(),
            mmio_regions,
            memory_map.default_address_space
        );
        if mmio_regions == 0 {
            warnings += 1;
            outputln!("DIAGNOSTIC\twarning\tmemory map has no MMIO regions");
        }
    } else {
        warnings += 1;
        outputln!("CAPABILITY\tmemory-map\tnot-configured");
    }

    let project_model = context.project.registers.as_ref().is_some_and(|paths| {
        paths.model.is_file() && RegisterModel::is_model_file(&paths.model).unwrap_or(false)
    });
    if context.svd_paths.is_empty() && !project_model {
        outputln!(
            "CAPABILITY\tregister-catalog\tnot-configured\tregisters=0\tmmio-windows={}",
            context.svd.windows.len()
        );
    } else {
        outputln!(
            "CAPABILITY\tregister-catalog\tavailable\tfiles={}\tproject-model={}\tregisters={}\tmmio-windows={}",
            context.svd_paths.len(),
            if project_model { "yes" } else { "no" },
            context.svd.registers.len(),
            context.svd.windows.len()
        );
        for path in context.svd_paths {
            outputln!("SVD\t{}", path.display());
        }
    }

    let (ir_errors, ir_warnings) =
        super::project_ir_doctor::inspect(context.project, context.run_spec, context.target);
    errors += ir_errors;
    warnings += ir_warnings;

    match &context.project.symbol_inventory {
        None => outputln!("CAPABILITY\tsymbol-inventory\tnot-configured"),
        Some(spec) if !spec.output.is_file() => {
            warnings += 1;
            outputln!(
                "CAPABILITY\tsymbol-inventory\tnot-generated\tpath={}",
                spec.output.display()
            );
        }
        Some(spec) => match super::symbol_inventory::inspect_report(&spec.output) {
            Ok(summary) => outputln!(
                "CAPABILITY\tsymbol-inventory\tavailable\tartifacts={}\tsymbol-facts={}\texported-definitions={}\tundefined={}\tunresolved-or-associated={}\tpath={}",
                summary.artifacts,
                summary.symbol_facts,
                summary.exported_definitions,
                summary.undefined,
                summary.unresolved_or_associated,
                spec.output.display(),
            ),
            Err(error) => {
                errors += 1;
                outputln!(
                    "CAPABILITY\tsymbol-inventory\tinvalid\tpath={}\terror={}",
                    spec.output.display(),
                    error
                );
            }
        },
    }

    let (function_errors, function_warnings) =
        super::project_function_doctor::inspect(context.project);
    errors += function_errors;
    warnings += function_warnings;

    match &context.project.registers {
        None => outputln!("CAPABILITY\tregister-workspace\tnot-configured"),
        Some(paths) if paths.model.is_file() => {
            match ProjectRegisterWorkspace::load(&paths.facts, &paths.model)
                .and_then(|workspace| Ok((workspace.summary()?, workspace.format_label())))
            {
                Ok((summary, format)) => {
                    outputln!(
                        "CAPABILITY\tregister-workspace\tavailable\tformat={}\tranges={}\tobserved={}\treviewed={}\tignored={}\tmanual={}\tunreviewed={}\tfields={}\tfacts={}\tmodel={}\treview-output={}\treview-ir-reports={}\tsvd-output={}\tpac-output={}\tbindings-output={}\tbindings-crate={}",
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
                            .review_output
                            .as_deref()
                            .map_or_else(|| "-".to_owned(), |path| path.display().to_string()),
                        paths.review_ir_reports.len(),
                        paths
                            .svd_output
                            .as_deref()
                            .map_or_else(|| "-".to_owned(), |path| path.display().to_string()),
                        paths
                            .pac
                            .as_ref()
                            .map_or_else(|| "-".to_owned(), |pac| pac.output.display().to_string()),
                        paths.bindings.as_ref().map_or_else(
                            || "-".to_owned(),
                            |bindings| bindings.output.display().to_string()
                        ),
                        paths
                            .bindings
                            .as_ref()
                            .map_or("-", |bindings| bindings.crate_name.as_str())
                    );
                    if !paths.review_ir_reports.is_empty() {
                        let missing_project_outputs = paths
                            .review_ir_reports
                            .iter()
                            .filter(|path| !path.is_file())
                            .filter(|path| {
                                context
                                    .project
                                    .ir_profiles
                                    .iter()
                                    .any(|profile| profile.output.as_path() == path.as_path())
                            })
                            .count();
                        let missing_reports = paths
                            .review_ir_reports
                            .iter()
                            .filter(|path| !path.is_file())
                            .count();
                        if missing_reports != 0 && missing_reports == missing_project_outputs {
                            outputln!(
                                "CAPABILITY\tregister-review-ir\tnot-generated\treports={}\tmissing={missing_reports}",
                                paths.review_ir_reports.len()
                            );
                        } else {
                            match inspect_register_review_ir(&paths.review_ir_reports) {
                                Ok(ir) => outputln!(
                                    "CAPABILITY\tregister-review-ir\tavailable\treports={}\tregisters={}\tfield-candidates={}",
                                    ir.reports,
                                    ir.registers,
                                    ir.fields
                                ),
                                Err(error) => {
                                    errors += 1;
                                    outputln!(
                                        "CAPABILITY\tregister-review-ir\tinvalid\treports={}\terror={error}",
                                        paths.review_ir_reports.len()
                                    );
                                }
                            }
                        }
                        for path in &paths.review_ir_reports {
                            outputln!("REGISTER-REVIEW-IR\tpath={}", path.display());
                        }
                    }
                }
                Err(error) => {
                    errors += 1;
                    outputln!(
                        "CAPABILITY\tregister-workspace\tinvalid\tfacts={}\tmodel={}\terror={error}",
                        paths.facts.display(),
                        paths.model.display()
                    );
                }
            }
        }
        Some(paths) if !paths.facts.is_file() => {
            warnings += 1;
            outputln!(
                "CAPABILITY\tregister-workspace\tnot-generated\tfacts={}\tmodel={}",
                paths.facts.display(),
                paths.model.display()
            );
        }
        Some(paths) => match RegisterFacts::load(&paths.facts) {
            Ok(facts) => {
                warnings += 1;
                outputln!(
                    "CAPABILITY\tregister-workspace\tmodel-not-initialized\tranges={}\tobserved={}\tfacts={}\tmodel={}",
                    facts.ranges.len(),
                    facts.registers.len(),
                    paths.facts.display(),
                    paths.model.display()
                );
            }
            Err(error) => {
                errors += 1;
                outputln!(
                    "CAPABILITY\tregister-workspace\tinvalid-facts\tfacts={}\terror={error}",
                    paths.facts.display()
                );
            }
        },
    }

    match context
        .project
        .registers
        .as_ref()
        .and_then(|paths| paths.api_pack.as_deref().map(|pack| (paths, pack)))
    {
        None => outputln!("CAPABILITY\tpac-api\tnot-configured"),
        Some((_, path)) if !path.is_file() => {
            warnings += 1;
            outputln!(
                "CAPABILITY\tpac-api\tnot-initialized\tpack={}",
                path.display()
            );
        }
        Some((paths, path)) => match validate_pac_api(paths) {
            Ok(Some(pack)) => outputln!(
                "CAPABILITY\tpac-api\tavailable\tschema={}\toperations={}\tsources={}\tperipheral-ownership={}\tdevice-access={}\tpack={}",
                pack.schema,
                pack.operation_count(),
                pack.source_ids().len(),
                pack.options.peripheral_ownership,
                pack.options.device_access,
                path.display()
            ),
            Ok(None) => unreachable!("PAC API path was configured before validation"),
            Err(error) => {
                errors += 1;
                outputln!(
                    "CAPABILITY\tpac-api\tinvalid\tpack={}\terror={error}",
                    path.display()
                );
            }
        },
    }

    match context
        .project
        .registers
        .as_ref()
        .and_then(|paths| paths.lint_pack.as_deref().map(|pack| (paths, pack)))
    {
        None => outputln!("CAPABILITY\tregister-lints\tnot-configured"),
        Some((_, path)) if !path.is_file() => {
            warnings += 1;
            outputln!(
                "CAPABILITY\tregister-lints\tnot-initialized\tpack={}",
                path.display()
            );
        }
        Some((paths, path)) => match validate_register_lints(paths) {
            Ok(Some(pack)) => outputln!(
                "CAPABILITY\tregister-lints\tavailable\tschema={}\tforbidden-field-name-substrings={}\tpack={}",
                pack.schema,
                pack.forbidden_field_name_substrings.len(),
                path.display()
            ),
            Ok(None) => unreachable!("register lint path was configured before validation"),
            Err(error) => {
                errors += 1;
                outputln!(
                    "CAPABILITY\tregister-lints\tinvalid\tpack={}\terror={error}",
                    path.display()
                );
            }
        },
    }

    match context.project.registers.as_ref() {
        None => outputln!("CAPABILITY\tregister-evidence\tnot-configured"),
        Some(paths) if paths.evidence_catalogs.is_empty() => {
            outputln!("CAPABILITY\tregister-evidence\tnot-configured")
        }
        Some(paths) => {
            let result = validate_register_evidence(paths, context.memory_map);
            match result {
                Ok(Some(evidence)) => outputln!(
                    "CAPABILITY\tregister-evidence\tavailable\tcatalogs={}\tconfidence-levels={}\tsources={}\tranges={}",
                    paths.evidence_catalogs.len(),
                    evidence.confidence_levels.len(),
                    evidence.sources.len(),
                    evidence.ranges.len()
                ),
                Ok(None) => unreachable!("evidence catalogs were configured before validation"),
                Err(error) => {
                    errors += 1;
                    outputln!(
                        "CAPABILITY\tregister-evidence\tinvalid\tcatalogs={}\terror={error}",
                        paths.evidence_catalogs.len()
                    );
                }
            }
        }
    }

    match &context.project.interfaces {
        None => outputln!("CAPABILITY\tinterface-facts\tnot-configured"),
        Some(paths) if !paths.facts.is_file() => {
            warnings += 1;
            outputln!(
                "CAPABILITY\tinterface-facts\tnot-generated\tfacts={}",
                paths.facts.display()
            );
        }
        Some(paths) => match InterfaceFacts::load(&paths.facts) {
            Err(error) => {
                errors += 1;
                outputln!(
                    "CAPABILITY\tinterface-workspace\tinvalid-facts\tfacts={}\terror={error}",
                    paths.facts.display()
                );
            }
            Ok(facts) => match paths.pack.as_deref() {
                None => {
                    outputln!(
                        "CAPABILITY\tinterface-facts\tavailable\ttables={}\tobserved-slots={}\tobserved-calls={}\tfacts={}",
                        facts.tables.len(),
                        facts.observed_slots(),
                        facts.observed_calls(),
                        paths.facts.display()
                    );
                }
                Some(pack) if !pack.is_file() => {
                    warnings += 1;
                    outputln!(
                        "CAPABILITY\tinterface-workspace\tpack-not-initialized\ttables={}\tobserved-slots={}\tobserved-calls={}\tfacts={}\tpack={}",
                        facts.tables.len(),
                        facts.observed_slots(),
                        facts.observed_calls(),
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
                        outputln!(
                            "CAPABILITY\tinterface-workspace\tavailable\tfact-tables={}\tobserved-slots={}\tobserved-calls={}\tresolved-calls={}\treviewed-anchors={}\tignored-anchors={}\tunreviewed-anchors={}\treviewed-slots={}\tignored-slots={}\tunreviewed-slots={}\tsemantic-links={}\tsemantic-operations={}\tfacts={}\tpack={}",
                            summary.fact_tables,
                            summary.observed_slots,
                            summary.observed_calls,
                            summary.resolved_calls,
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
                        outputln!(
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
            outputln!("RUN-SPEC\t{}", path.display());
            input_count = run_spec.inputs().len();
            for input in run_spec.inputs() {
                if !input.path.is_file() {
                    errors += 1;
                    outputln!(
                        "INPUT\trole={}\tstatus=missing\tpath={}",
                        input.role,
                        input.path.display()
                    );
                    continue;
                }
                match artifact::inspect_artifact(&input.path) {
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
                        outputln!(
                            "INPUT\trole={}\tstatus={}\tcontainer={}\tobjects={}\tskipped-members={}\tsymbol-facts={}\tcode-definitions={}\texported-definitions={}\tundefined={}\tpath={}",
                            input.role,
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
                            input.path.display()
                        );
                    }
                    Err(error) => {
                        errors += 1;
                        outputln!(
                            "INPUT\trole={}\tstatus=invalid\tpath={}\terror={}",
                            input.role,
                            input.path.display(),
                            error
                        );
                    }
                }
            }
        }
        (None, None) => {
            warnings += 1;
            outputln!("RUN-SPEC\tnot-configured\tartifact-bindings-unavailable");
        }
        _ => unreachable!("run-spec path and parsed contents are created together"),
    }

    outputln!(
        "SUMMARY\tstatus={}\terrors={}\twarnings={}\tinputs={}\tvalid-inputs={}",
        if errors == 0 { "ok" } else { "failed" },
        errors,
        warnings,
        input_count,
        valid_inputs
    );
    Ok(errors == 0)
}
