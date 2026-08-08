//! Read-only projection of every configured project workspace.

use std::collections::BTreeSet;

use super::{ResolvedProject, model::*};
use crate::{
    function_workspace::{
        FunctionMemoryObjectFact, FunctionReviewStatus, FunctionWorkspace, ReviewedMemoryObject,
    },
    interfaces::InterfaceWorkspace,
    registers::ProjectRegisterWorkspace,
};

pub(super) fn collect(resolved: &ResolvedProject, generation: u64) -> WorkspaceSnapshot {
    let context = resolved.context();
    let status = crate::cli::commands::project_status::collect(&context);
    let project_status = project_status_snapshot(&status);
    let mut diagnostics = status
        .phases
        .iter()
        .flat_map(|phase| {
            phase.components.iter().filter_map(move |component| {
                component
                    .diagnostic
                    .as_ref()
                    .map(|diagnostic| DiagnosticRecord {
                        severity: if component.status
                            == crate::cli::commands::project_status::model::Readiness::Invalid
                        {
                            DiagnosticSeverity::Error
                        } else {
                            DiagnosticSeverity::Warning
                        },
                        component: format!("{}.{}", phase.name, component.name),
                        message: diagnostic.clone(),
                        path: None,
                    })
            })
        })
        .collect::<Vec<_>>();
    let (functions, logical_types) = functions(resolved, &mut diagnostics);
    let registers = registers(resolved, &mut diagnostics);
    let interfaces = interfaces(resolved, &mut diagnostics);
    let comparisons = comparisons(resolved, &mut diagnostics);
    diagnostics.sort_by(|left, right| {
        (&left.component, &left.message).cmp(&(&right.component, &right.message))
    });
    diagnostics.dedup();
    WorkspaceSnapshot {
        generation,
        project_status,
        functions,
        logical_types,
        registers,
        interfaces,
        comparisons,
        diagnostics,
    }
}

fn comparisons(
    resolved: &ResolvedProject,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> Vec<ComparisonProfileSummary> {
    let Some(workspace) = resolved.project.verification.as_ref() else {
        return Vec::new();
    };
    let mut output = Vec::new();
    let mut names = BTreeSet::new();
    for path in &workspace.profiles {
        let profiles = match crate::verification::profiles::load(path) {
            Ok(profiles) => profiles,
            Err(error) => {
                push_error(
                    diagnostics,
                    "verification.profiles",
                    error,
                    Some(path.clone()),
                );
                continue;
            }
        };
        for profile in profiles {
            if !names.insert(profile.name.clone()) {
                diagnostics.push(DiagnosticRecord {
                    severity: DiagnosticSeverity::Error,
                    component: "verification.profiles".to_owned(),
                    message: format!("duplicate comparison profile {:?}", profile.name),
                    path: Some(path.clone()),
                });
                continue;
            }
            output.push(ComparisonProfileSummary {
                name: profile.name,
                path: path.clone(),
                vendor_source: profile.vendor_source,
                vendor_symbol: profile.vendor_symbol,
                rust_symbol: profile.rust_symbol,
                scenarios: profile.scenarios.len(),
            });
        }
    }
    output.sort_by(|left, right| left.name.cmp(&right.name));
    output
}

fn project_status_snapshot(
    report: &crate::cli::commands::project_status::model::StatusReport,
) -> ProjectStatusSnapshot {
    ProjectStatusSnapshot {
        project_id: report.project_id.clone(),
        manifest: report.manifest.clone(),
        target_id: report.target.id.clone(),
        architecture: report.target.architecture.clone(),
        calling_convention: report.target.calling_convention.clone(),
        harness: report.target.harness.clone(),
        overall: readiness(report.overall),
        phases: report
            .phases
            .iter()
            .map(|phase| WorkspacePhaseSnapshot {
                name: phase.name.to_owned(),
                status: readiness(phase.status),
                components: phase
                    .components
                    .iter()
                    .map(|component| WorkspaceComponentSnapshot {
                        name: component.name.to_owned(),
                        status: readiness(component.status),
                        details: component
                            .details
                            .iter()
                            .map(|(key, value)| {
                                (
                                    key.clone(),
                                    serde_json::to_value(value)
                                        .expect("status detail values are serializable"),
                                )
                            })
                            .collect(),
                        diagnostic: component.diagnostic.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn readiness(value: crate::cli::commands::project_status::model::Readiness) -> WorkspaceReadiness {
    use crate::cli::commands::project_status::model::Readiness;
    match value {
        Readiness::Ready => WorkspaceReadiness::Ready,
        Readiness::Incomplete => WorkspaceReadiness::Incomplete,
        Readiness::NotConfigured => WorkspaceReadiness::NotConfigured,
        Readiness::Invalid => WorkspaceReadiness::Invalid,
    }
}

fn functions(
    resolved: &ResolvedProject,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> (Vec<FunctionSummary>, Vec<LogicalTypeSummary>) {
    let Some(paths) = resolved.project.functions.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let reports = match resolved.project.function_ir_reports() {
        Ok(reports) => reports,
        Err(error) => {
            push_error(diagnostics, "functions", error, Some(paths.pack.clone()));
            return (Vec::new(), Vec::new());
        }
    };
    if reports.iter().any(|(_, path)| !path.is_file()) || !paths.pack.is_file() {
        return (Vec::new(), Vec::new());
    }
    let workspace = match FunctionWorkspace::load(&reports, &paths.pack) {
        Ok(workspace) => workspace,
        Err(error) => {
            push_error(diagnostics, "functions", error, Some(paths.pack.clone()));
            return (Vec::new(), Vec::new());
        }
    };
    let functions = workspace
        .facts
        .functions
        .iter()
        .map(|fact| {
            let reviewed = workspace.pack.functions.iter().find(|function| {
                function.profile == fact.profile
                    && function.source == fact.source
                    && function.identity == fact.identity
            });
            let arguments = fact
                .context_fields
                .iter()
                .map(|field| field.argument)
                .chain(
                    reviewed
                        .into_iter()
                        .flat_map(|function| &function.contexts)
                        .map(|context| context.argument),
                )
                .collect::<BTreeSet<_>>();
            let contexts = arguments
                .into_iter()
                .map(|argument| {
                    let reviewed_context = reviewed.and_then(|function| {
                        function
                            .contexts
                            .iter()
                            .find(|context| context.argument == argument)
                    });
                    let fields = fact
                        .context_fields
                        .iter()
                        .filter(|field| field.argument == argument)
                        .map(|field| {
                            let reviewed_field = reviewed_context.and_then(|context| {
                                context.fields.iter().find(|candidate| {
                                    candidate.offset == field.offset
                                        && candidate.width == field.width
                                })
                            });
                            FunctionContextFieldSummary {
                                offset: field.offset,
                                width: field.width,
                                reads: field.reads,
                                writes: field.writes,
                                write_mask: field.write_mask,
                                name: reviewed_field.and_then(|field| field.name.clone()),
                                display_type: reviewed_field
                                    .and_then(|field| field.display_type.clone()),
                                description: reviewed_field
                                    .and_then(|field| field.description.clone()),
                            }
                        })
                        .collect();
                    FunctionContextSummary {
                        argument,
                        name: reviewed_context.and_then(|context| context.name.clone()),
                        type_name: reviewed_context.and_then(|context| context.type_name.clone()),
                        fields,
                    }
                })
                .collect();
            let mut blockers = fact.context_projection_blockers.clone();
            if !fact.direct_complete {
                blockers.push("direct structural analysis is incomplete".to_owned());
            }
            if !fact.call_graph_closed {
                blockers.push("call graph is not closed".to_owned());
            }
            FunctionSummary {
                profile: fact.profile.clone(),
                source: fact.source.clone(),
                identity: fact.identity.clone(),
                symbol: fact.symbol.clone(),
                member: fact.member.clone(),
                selection: fact.selection.clone(),
                review_status: reviewed
                    .map_or("unreviewed", |function| function_status(function.status))
                    .to_owned(),
                reviewed_name: reviewed.and_then(|function| function.name.clone()),
                role: reviewed.and_then(|function| function.role.clone()),
                summary: reviewed.and_then(|function| function.summary.clone()),
                complete: fact.review_complete(),
                blockers,
                semantic_operations: fact.semantic_operations.clone(),
                calls: fact.calls.len(),
                contexts,
                memory_fields: fact
                    .memory_fields
                    .iter()
                    .map(|field| FunctionMemoryFieldSummary {
                        object: memory_fact_label(&field.object),
                        offset: field.offset,
                        width: field.width,
                        reads: field.reads,
                        writes: field.writes,
                        write_mask: field.write_mask,
                    })
                    .collect(),
                scenario_suggestions: fact
                    .scenario_suggestions
                    .iter()
                    .map(|suggestion| ScenarioSuggestionSummary {
                        kind: suggestion.kind.clone(),
                        site: suggestion.site,
                        evidence: suggestion.evidence.clone(),
                        variants: suggestion
                            .variants
                            .iter()
                            .map(|variant| ScenarioSuggestionVariantSummary {
                                name: variant.name.clone(),
                                arguments: variant
                                    .arguments
                                    .iter()
                                    .map(|argument| ScenarioArgumentSummary {
                                        index: argument.index,
                                        value: argument.value,
                                    })
                                    .collect(),
                                mmio_reads: variant
                                    .mmio_reads
                                    .iter()
                                    .map(|read| ScenarioMmioReadSummary {
                                        address: read.address,
                                        mask: read.mask,
                                        expected: read.expected,
                                        values: read.values.clone(),
                                    })
                                    .collect(),
                            })
                            .collect(),
                    })
                    .collect(),
                pseudo_rust: fact.pseudo.clone(),
            }
        })
        .collect();
    let logical_types = workspace
        .pack
        .types
        .iter()
        .map(|logical_type| LogicalTypeSummary {
            id: logical_type.id.clone(),
            name: logical_type.name.clone(),
            description: logical_type.description.clone(),
            bindings: logical_type
                .bindings
                .iter()
                .map(|binding| LogicalTypeBindingSummary {
                    profile: binding.profile.clone(),
                    source: binding.source.clone(),
                    name: binding.name.clone(),
                    object: reviewed_memory_label(&binding.object),
                })
                .collect(),
            fields: logical_type
                .fields
                .iter()
                .map(|field| LogicalTypeFieldSummary {
                    offset: field.offset,
                    width: field.width,
                    status: function_status(field.status).to_owned(),
                    name: field.name.clone(),
                    display_type: field.display_type.clone(),
                    description: field.description.clone(),
                })
                .collect(),
        })
        .collect();
    (functions, logical_types)
}

fn memory_fact_label(object: &FunctionMemoryObjectFact) -> String {
    match object {
        FunctionMemoryObjectFact::Argument { index } => format!("argument:{index}"),
        FunctionMemoryObjectFact::Global { member, symbol } => format!(
            "global:{}::{symbol}",
            member.as_deref().unwrap_or("<linked>")
        ),
        FunctionMemoryObjectFact::DereferencedGlobal {
            member,
            symbol,
            pointer_offset,
        } => format!(
            "dereferenced-global:{}::{symbol}{pointer_offset:+#x}",
            member.as_deref().unwrap_or("<linked>")
        ),
        FunctionMemoryObjectFact::Absolute {
            address_space,
            address,
        } => format!("absolute:{address_space}:{address:#010x}"),
    }
}

fn reviewed_memory_label(object: &ReviewedMemoryObject) -> String {
    match object {
        ReviewedMemoryObject::Argument { function, index } => {
            format!("argument:{function}:arg{index}")
        }
        ReviewedMemoryObject::Global { member, symbol } => format!(
            "global:{}::{symbol}",
            member.as_deref().unwrap_or("<linked>")
        ),
        ReviewedMemoryObject::DereferencedGlobal {
            member,
            symbol,
            pointer_offset,
        } => format!(
            "dereferenced-global:{}::{symbol}{pointer_offset:+#x}",
            member.as_deref().unwrap_or("<linked>")
        ),
        ReviewedMemoryObject::Absolute {
            address_space,
            address,
        } => format!("absolute:{address_space}:{address:#010x}"),
    }
}

fn function_status(status: FunctionReviewStatus) -> &'static str {
    match status {
        FunctionReviewStatus::Unreviewed => "unreviewed",
        FunctionReviewStatus::Reviewed => "reviewed",
        FunctionReviewStatus::Ignored => "ignored",
    }
}

fn registers(
    resolved: &ResolvedProject,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> RegisterWorkspaceReport {
    let configured = resolved.project.registers.is_some();
    let model = resolved
        .project
        .registers
        .as_ref()
        .map(|paths| paths.model.clone());
    let summary = resolved.project.registers.as_ref().and_then(|paths| {
        if !paths.model.is_file() {
            return None;
        }
        match ProjectRegisterWorkspace::load(&paths.facts, &paths.model)
            .and_then(|workspace| workspace.summary())
        {
            Ok(summary) => Some(summary),
            Err(error) => {
                push_error(diagnostics, "registers", error, Some(paths.model.clone()));
                None
            }
        }
    });
    RegisterWorkspaceReport {
        configured,
        model,
        ranges: summary.map_or(0, |summary| summary.ranges),
        observed: summary.map_or(0, |summary| summary.observed),
        reviewed: summary.map_or(0, |summary| summary.reviewed),
        manual: summary.map_or(0, |summary| summary.manual),
        unreviewed: summary.map_or(0, |summary| summary.unreviewed),
        fields: summary.map_or(0, |summary| summary.fields),
        registers: resolved
            .mmio
            .registers
            .iter()
            .map(|register| RegisterSummary {
                address: register.address,
                name: register.name.clone(),
            })
            .collect(),
    }
}

fn interfaces(
    resolved: &ResolvedProject,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> InterfaceWorkspaceReport {
    let Some(paths) = resolved.project.interfaces.as_ref() else {
        return InterfaceWorkspaceReport {
            configured: false,
            facts: None,
            pack: None,
            observed_slots: 0,
            reviewed_slots: 0,
            unreviewed_slots: 0,
            contracts: Vec::new(),
            slots: Vec::new(),
        };
    };
    let Some(pack) = paths.pack.as_ref().filter(|pack| pack.is_file()) else {
        return InterfaceWorkspaceReport {
            configured: true,
            facts: Some(paths.facts.clone()),
            pack: paths.pack.clone(),
            observed_slots: 0,
            reviewed_slots: 0,
            unreviewed_slots: 0,
            contracts: Vec::new(),
            slots: Vec::new(),
        };
    };
    if !paths.facts.is_file() {
        return InterfaceWorkspaceReport {
            configured: true,
            facts: Some(paths.facts.clone()),
            pack: Some(pack.clone()),
            observed_slots: 0,
            reviewed_slots: 0,
            unreviewed_slots: 0,
            contracts: Vec::new(),
            slots: Vec::new(),
        };
    }
    let harness = resolved
        .target
        .harness
        .as_deref()
        .and_then(|harness| crate::harnesses::contracts(harness).ok());
    let workspace = match InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        resolved.target.calling_convention.label(),
        harness,
    ) {
        Ok(workspace) => workspace,
        Err(error) => {
            push_error(diagnostics, "interfaces", error, Some(pack.clone()));
            return InterfaceWorkspaceReport {
                configured: true,
                facts: Some(paths.facts.clone()),
                pack: Some(pack.clone()),
                observed_slots: 0,
                reviewed_slots: 0,
                unreviewed_slots: 0,
                contracts: Vec::new(),
                slots: Vec::new(),
            };
        }
    };
    let summary = workspace.summary();
    InterfaceWorkspaceReport {
        configured: true,
        facts: Some(paths.facts.clone()),
        pack: Some(pack.clone()),
        observed_slots: summary.observed_slots,
        reviewed_slots: summary.reviewed_slots,
        unreviewed_slots: summary.unreviewed_slots,
        contracts: workspace
            .contracts()
            .iter()
            .map(|contract| InterfaceContractSummary {
                id: contract.id.clone(),
                source: contract.source.clone(),
                layout_version: contract.layout_version.clone(),
                pointer_width: contract.pointer_width,
                layout_size: contract.layout_size,
                slot_stride: contract.slot_stride,
                guards: contract.guards.len(),
                execution_contract: contract
                    .execution_contract
                    .as_ref()
                    .map(|contract| contract.id.clone()),
                slots: contract.slots.clone(),
            })
            .collect(),
        slots: workspace
            .bindings()
            .iter()
            .map(|slot| InterfaceSlotSummary {
                id: slot.id.clone(),
                contract: slot.contract.clone(),
                offset: slot.offset,
                width: slot.width,
                name: slot.name.clone(),
                arguments: slot.arguments.clone(),
                return_type: slot.return_type.clone(),
                variadic: slot.variadic,
                semantic: slot.semantic.clone(),
                effects: slot
                    .semantic_annotation
                    .as_ref()
                    .map_or_else(Vec::new, |semantic| semantic.effects.clone()),
                replacement: slot
                    .semantic_annotation
                    .as_ref()
                    .and_then(|semantic| semantic.replacement.clone()),
                execution_model: slot.execution_model.as_ref().map(|model| model.id.clone()),
                functions: slot.functions.iter().cloned().collect(),
                call_sites: slot.calls.iter().map(|call| call.site).collect(),
            })
            .collect(),
    }
}

fn push_error(
    diagnostics: &mut Vec<DiagnosticRecord>,
    component: &str,
    error: crate::Error,
    path: Option<std::path::PathBuf>,
) {
    diagnostics.push(DiagnosticRecord {
        severity: DiagnosticSeverity::Error,
        component: component.to_owned(),
        message: error.to_string(),
        path,
    });
}
