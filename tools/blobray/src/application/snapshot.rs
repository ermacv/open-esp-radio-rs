//! Read-only projection of every configured project workspace.

mod code;
mod comparisons;
mod functions;
mod interfaces;
mod policy;
pub(super) mod registers;
mod review_queue;
mod scopes;

use std::collections::{BTreeMap, BTreeSet};

use super::{ProjectSession, model::*};
use crate::function_workspace::{
    FunctionFact, FunctionMemoryObjectFact, FunctionReviewStatus, ReviewedFunction,
    ReviewedLogicalType, ReviewedMemoryObject,
};

pub(super) fn collect(resolved: &ProjectSession, generation: u64) -> WorkspaceSnapshot {
    let context = resolved.context();
    let status = crate::application::status::collect(&context);
    let analysis_surfaces = status
        .phases
        .iter()
        .find(|phase| phase.name == "analysis")
        .and_then(|phase| {
            phase
                .components
                .iter()
                .find(|component| component.name == "radio_surfaces")
        })
        .and_then(|component| component.details.get("surfaces"))
        .and_then(|value| match value {
            crate::DetailValue::AnalysisSurfaces(surfaces) => Some(surfaces.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let project_status = status.clone();
    let mut diagnostics = status
        .phases
        .iter()
        .flat_map(|phase| {
            phase.components.iter().filter_map(move |component| {
                component
                    .diagnostic
                    .as_ref()
                    .map(|diagnostic| DiagnosticRecord {
                        severity: if component.status == crate::Readiness::Invalid {
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
    let code = self::code::collect(resolved, &mut diagnostics);
    let (functions, logical_types) = self::functions::collect(resolved, &mut diagnostics);
    let registers = self::registers::collect(resolved, &mut diagnostics);
    let interfaces = self::interfaces::collect(resolved, &mut diagnostics);
    let review_scopes = self::scopes::collect(resolved, &mut diagnostics);
    let verification_policy = self::policy::collect(resolved, &mut diagnostics);
    let review_queue = self::review_queue::collect(resolved, &mut diagnostics);
    let comparisons = self::comparisons::collect(resolved, &mut diagnostics);
    diagnostics.sort_by(|left, right| {
        (&left.component, &left.message).cmp(&(&right.component, &right.message))
    });
    diagnostics.dedup();
    WorkspaceSnapshot {
        generation,
        project_status,
        code,
        functions,
        logical_types,
        registers,
        interfaces,
        review_scopes,
        analysis_surfaces,
        verification_policy,
        review_queue,
        comparisons,
        diagnostics,
    }
}

pub(super) fn function_detail(
    resolved: &ProjectSession,
    identity: &str,
) -> crate::Result<Option<FunctionDetailSummary>> {
    let Some(_) = resolved.project.functions.as_ref() else {
        return Ok(None);
    };
    let Some(workspace) = resolved.function_workspace()? else {
        return Ok(None);
    };
    let Some(summary_fact) = workspace
        .facts
        .functions
        .iter()
        .find(|fact| fact.identity == identity)
    else {
        return Ok(None);
    };
    let reports = resolved.project.function_ir_reports()?;
    let report = reports
        .iter()
        .find_map(|(profile, path)| (profile == &summary_fact.profile).then_some(path))
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "function profile {:?} has no linked-IR report",
                summary_fact.profile
            ))
        })?;
    let fact = resolved
        .linked_ir(report)?
        .get_function_by_identity(&summary_fact.identity)?
        .map(|function| {
            crate::function_workspace::function_fact_from_stored(&summary_fact.profile, function)
        })
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "linked-IR report no longer contains function {:?}",
                summary_fact.identity
            ))
        })?;
    let reviewed = workspace.pack.functions.iter().find(|function| {
        function.profile == fact.profile
            && function.source == fact.source
            && function.identity == fact.identity
    });
    let investigation = function_investigation(resolved, &fact)?;
    Ok(Some(function_detail_summary(
        &fact,
        reviewed,
        &workspace.pack.types,
        investigation,
    )))
}

pub(super) fn register_detail(
    resolved: &ProjectSession,
    address: u32,
) -> crate::Result<Option<RegisterDetailSummary>> {
    self::registers::detail(&resolved.project, &resolved.mmio, address)
}

fn function_detail_summary(
    fact: &FunctionFact,
    reviewed: Option<&ReviewedFunction>,
    logical_types: &[ReviewedLogicalType],
    investigation: Option<crate::FunctionInvestigationReport>,
) -> FunctionDetailSummary {
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
                            candidate.offset == field.offset && candidate.width == field.width
                        })
                    });
                    FunctionContextFieldSummary {
                        offset: field.offset,
                        width: field.width,
                        reads: field.reads,
                        writes: field.writes,
                        write_mask: field.write_mask,
                        name: reviewed_field.and_then(|field| field.name.clone()),
                        display_type: reviewed_field.and_then(|field| field.display_type.clone()),
                        description: reviewed_field.and_then(|field| field.description.clone()),
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
    let scenario_suggestions = fact
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
        .collect::<Vec<_>>();
    FunctionDetailSummary {
        identity: fact.identity.clone(),
        registers: fact.mmio_addresses.clone(),
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
        decode_blockers: fact
            .decode_blockers
            .iter()
            .map(|blocker| FunctionDecodeBlockerSummary {
                address: blocker.address,
                width: blocker.width,
                raw: blocker.raw,
                class: blocker.class.clone(),
                operation: crate::artifact::unsupported_instruction_mnemonic(
                    blocker.width,
                    blocker.raw,
                )
                .to_owned(),
                linear_control_flow: blocker.linear_control_flow,
            })
            .collect(),
        profile_draft: profile_draft(fact, &scenario_suggestions),
        scenario_suggestions,
        pseudo_rust: reviewed_pseudo(fact, reviewed, logical_types),
        reviewed_preconditions: reviewed
            .into_iter()
            .flat_map(|function| &function.preconditions)
            .map(|precondition| ReviewedPreconditionSummary {
                id: precondition.id.clone(),
                expression: precondition.expression.clone(),
                rationale: precondition.rationale.clone(),
            })
            .collect(),
        reviewed_paths: reviewed
            .into_iter()
            .flat_map(|function| &function.paths)
            .map(|path| ReviewedPathSummary {
                id: path.id.clone(),
                class: path.class.clone(),
                summary: path.summary.clone(),
                evidence: path.evidence.clone(),
            })
            .collect(),
        investigation,
    }
}

fn function_investigation(
    resolved: &ProjectSession,
    fact: &FunctionFact,
) -> crate::Result<Option<crate::FunctionInvestigationReport>> {
    let Some(run_spec) = resolved.run_spec.as_ref() else {
        return Ok(None);
    };
    let artifact = run_spec
        .inputs()
        .iter()
        .find_map(|input| match &input.role {
            crate::run_spec::InputRole::SourceArtifact(source)
                if source.as_str() == fact.source =>
            {
                Some(input.path.as_path())
            }
            _ => None,
        });
    let Some(artifact) = artifact else {
        return Ok(None);
    };
    let inventories = run_spec
        .inputs()
        .iter()
        .filter_map(|input| match &input.role {
            crate::run_spec::InputRole::SourceInventory(source)
                if source.as_str() == fact.source =>
            {
                Some(input.path.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    crate::function_investigation::investigate(
        crate::function_investigation::FunctionInvestigationRequest {
            source: &fact.source,
            symbol: &fact.symbol,
            runtime_address: fact
                .identity
                .rsplit_once("@0x")
                .and_then(|(_, address)| u64::from_str_radix(address, 16).ok()),
            artifact,
            inventories: &inventories,
            member: fact.member.as_deref(),
            origin_member: None,
            graph_depth: 1,
            include_callers: false,
            cfg_path: None,
            include_linked_ir_record: false,
        },
        &resolved.project,
    )
    .map(Some)
}

fn profile_draft(fact: &FunctionFact, suggestions: &[ScenarioSuggestionSummary]) -> Option<String> {
    if suggestions.is_empty() {
        return None;
    }
    let mut output = format!(
        "# Generated coverage draft; replace REVIEW_REQUIRED values and replay every case.\nschema = 5\ncase-execution = \"independent\"\ntransaction-comparison = \"observables\"\n\n[[profiles]]\nname = {}\nvendor-source = {}\nvendor-symbol = {}\nrust-symbol = \"REVIEW_REQUIRED_RUST_SYMBOL\"\nclaim = \"whole-function-equivalence\"\n",
        toml_edit::Value::from(format!(
            "draft-{}",
            fact.symbol
                .replace(|character: char| !character.is_ascii_alphanumeric(), "-")
        )),
        toml_edit::Value::from(fact.source.as_str()),
        toml_edit::Value::from(fact.symbol.as_str()),
    );
    for suggestion in suggestions {
        for variant in &suggestion.variants {
            output.push_str(&format!(
                "\n# {}: {}\n[[profiles.cases]]\nname = {}\n",
                suggestion.kind,
                suggestion.evidence,
                toml_edit::Value::from(format!("{}-{}", suggestion.kind, variant.name)),
            ));
            let arguments = variant
                .arguments
                .iter()
                .map(|argument| (argument.index, argument.value))
                .collect::<BTreeMap<_, _>>();
            if let Some(maximum) = arguments.keys().max().copied() {
                output.push_str("arguments = [\n");
                for index in 0..=maximum {
                    if let Some(value) = arguments.get(&index) {
                        output.push_str(&format!("  {value:#010x},\n"));
                    } else {
                        output.push_str(&format!(
                            "  0x00000000, # REVIEW_REQUIRED: supply unconstrained argument a{index}\n"
                        ));
                    }
                }
                output.push_str("]\n");
            }
            for read in &variant.mmio_reads {
                for value in &read.values {
                    output.push_str(&format!(
                        "[[profiles.cases.mmio-reads]]\naddress = {:#010x}\nvalue = {value:#010x}\n",
                        read.address
                    ));
                }
            }
        }
    }
    Some(output)
}

#[derive(Clone)]
struct PseudoContextAnnotation {
    name: String,
    type_name: String,
    fields: Vec<(i64, u8, String, String)>,
}

fn reviewed_pseudo(
    fact: &FunctionFact,
    reviewed: Option<&ReviewedFunction>,
    logical_types: &[ReviewedLogicalType],
) -> String {
    let mut contexts = BTreeMap::<u8, PseudoContextAnnotation>::new();
    for logical_type in logical_types {
        for binding in &logical_type.bindings {
            let ReviewedMemoryObject::Argument { function, index } = &binding.object else {
                continue;
            };
            if binding.profile != fact.profile
                || binding.source != fact.source
                || function != &fact.identity
            {
                continue;
            }
            contexts.insert(
                *index,
                PseudoContextAnnotation {
                    name: binding.name.clone(),
                    type_name: logical_type.name.clone(),
                    fields: logical_type
                        .fields
                        .iter()
                        .filter(|field| field.status == FunctionReviewStatus::Reviewed)
                        .filter_map(|field| {
                            Some((
                                field.offset,
                                field.width,
                                field.name.clone()?,
                                field
                                    .display_type
                                    .clone()
                                    .unwrap_or_else(|| "u32".to_owned()),
                            ))
                        })
                        .collect(),
                },
            );
        }
    }
    if let Some(reviewed) = reviewed {
        for context in &reviewed.contexts {
            let entry =
                contexts
                    .entry(context.argument)
                    .or_insert_with(|| PseudoContextAnnotation {
                        name: format!("ctx{}", context.argument),
                        type_name: "opaque context".to_owned(),
                        fields: Vec::new(),
                    });
            if let Some(name) = &context.name {
                entry.name.clone_from(name);
            }
            if let Some(type_name) = &context.type_name {
                entry.type_name.clone_from(type_name);
            }
            for field in &context.fields {
                if field.status != FunctionReviewStatus::Reviewed {
                    continue;
                }
                let Some(name) = &field.name else { continue };
                entry.fields.retain(|(offset, width, _, _)| {
                    *offset != i64::from(field.offset) || *width != field.width
                });
                entry.fields.push((
                    i64::from(field.offset),
                    field.width,
                    name.clone(),
                    field
                        .display_type
                        .clone()
                        .unwrap_or_else(|| "u32".to_owned()),
                ));
            }
        }
    }
    if contexts.is_empty() {
        return fact.pseudo.clone();
    }

    let mut output = fact.pseudo.clone();
    let mut annotations = String::new();
    for (argument, context) in &contexts {
        annotations.push_str(&format!(
            "// reviewed context: ctx{argument} = {}: {}\n",
            context.name, context.type_name
        ));
        for (offset, width, field, display_type) in &context.fields {
            let read = format!("ctx{argument}.read{width}({offset:+#x})");
            let reviewed_read = format!(
                "{}.{}.read{width}() /* {display_type} */",
                context.name, field
            );
            output = output.replace(&read, &reviewed_read);
            let write = format!("ctx{argument}.write{width}({offset:+#x}, ");
            let reviewed_write = format!(
                "{}.{}.write{width}(/* {display_type} */ ",
                context.name, field
            );
            output = output.replace(&write, &reviewed_write);
        }
    }
    if let Some(line_end) = output.find('\n') {
        output.insert_str(line_end + 1, &annotations);
    } else {
        output.insert_str(0, &annotations);
    }
    output
}

fn memory_fact_label(object: &FunctionMemoryObjectFact) -> String {
    match object {
        FunctionMemoryObjectFact::Argument { index } => format!("argument:{index}"),
        FunctionMemoryObjectFact::Global { member, symbol } => format!(
            "global:{}::{symbol}",
            member.as_deref().unwrap_or("<linked>")
        ),
        FunctionMemoryObjectFact::Dereferenced {
            pointer,
            pointer_offset,
        } => format!("*({}{pointer_offset:+#x})", memory_fact_label(pointer)),
        FunctionMemoryObjectFact::Absolute {
            address_space,
            address,
        } => format!("absolute:{address_space}:{address:#010x}"),
        FunctionMemoryObjectFact::Indexed {
            object,
            argument,
            stride,
        } => format!("{}[arg{argument} * {stride:#x}]", memory_fact_label(object)),
        FunctionMemoryObjectFact::Allocation { call_token } => {
            format!("allocation:{call_token}")
        }
        FunctionMemoryObjectFact::ZeroedAllocation { call_token } => {
            format!("zeroed-allocation:{call_token}")
        }
        FunctionMemoryObjectFact::OpaqueExternalObject { call_token } => {
            format!("opaque-external-object:{call_token}")
        }
    }
}

pub(super) fn push_error(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function_workspace::{
        ReviewedTypeBinding, ReviewedTypeField, ScenarioArgumentFact, ScenarioMmioReadFact,
        ScenarioSuggestionFact, ScenarioSuggestionVariantFact,
    };

    fn fact() -> FunctionFact {
        FunctionFact {
            profile: "radio".to_owned(),
            source: "rom".to_owned(),
            identity: "rom::init".to_owned(),
            member: None,
            symbol: "init".to_owned(),
            address: Some(0x1000),
            selection: "symbol-prefix-root".to_owned(),
            body_complete: true,
            call_targets_complete: true,
            transitive_effects_complete: true,
            executable_complete: true,
            transitive_effects_materialized: true,
            call_graph_closed: true,
            context_projection_materialized: true,
            context_projection_complete: true,
            context_projection_blockers: Vec::new(),
            decode_blockers: Vec::new(),
            direct_calls: 0,
            calls: Vec::new(),
            memory_writes: Vec::new(),
            mmio_addresses: vec![0x4000],
            context_fields: Vec::new(),
            memory_fields: Vec::new(),
            semantic_operations: Vec::new(),
            trampoline_calls: 0,
            event_dispatches: Vec::new(),
            scenario_suggestions: Vec::new(),
            pseudo: "// vendor symbol: rom::init\nlet ramread0 = ctx0.read32(+0x4);\nctx0.write32(+0x4, 1);\n".to_owned(),
        }
    }

    #[test]
    fn reviewed_logical_type_names_are_applied_to_pseudo_rust() {
        let logical_type = ReviewedLogicalType {
            id: "phy-state".to_owned(),
            name: "VendorPhyState".to_owned(),
            description: None,
            bindings: vec![ReviewedTypeBinding {
                profile: "radio".to_owned(),
                source: "rom".to_owned(),
                name: "state".to_owned(),
                object: ReviewedMemoryObject::Argument {
                    function: "rom::init".to_owned(),
                    index: 0,
                },
            }],
            fields: vec![ReviewedTypeField {
                offset: 4,
                width: 32,
                status: FunctionReviewStatus::Reviewed,
                name: Some("pending_events".to_owned()),
                display_type: Some("u32".to_owned()),
                description: None,
            }],
        };

        let pseudo = reviewed_pseudo(&fact(), None, &[logical_type]);
        assert!(pseudo.contains("ctx0 = state: VendorPhyState"));
        assert!(pseudo.contains("state.pending_events.read32()"));
        assert!(pseudo.contains("state.pending_events.write32("));
        assert!(!pseudo.contains("ctx0.read32(+0x4)"));
    }

    #[test]
    fn scenario_suggestions_become_an_explicit_editable_profile_draft() {
        let mut fact = fact();
        fact.scenario_suggestions = vec![ScenarioSuggestionFact {
            kind: "argument-branch".to_owned(),
            site: Some(0x1000),
            evidence: "a1 == 1".to_owned(),
            variants: vec![ScenarioSuggestionVariantFact {
                name: "taken".to_owned(),
                arguments: vec![ScenarioArgumentFact { index: 1, value: 1 }],
                mmio_reads: vec![ScenarioMmioReadFact {
                    address: 0x4000,
                    mask: 1,
                    expected: 1,
                    values: vec![0, 1],
                }],
            }],
        }];
        let suggestions = vec![ScenarioSuggestionSummary {
            kind: "argument-branch".to_owned(),
            site: Some(0x1000),
            evidence: "a1 == 1".to_owned(),
            variants: vec![ScenarioSuggestionVariantSummary {
                name: "taken".to_owned(),
                arguments: vec![ScenarioArgumentSummary { index: 1, value: 1 }],
                mmio_reads: vec![ScenarioMmioReadSummary {
                    address: 0x4000,
                    mask: 1,
                    expected: 1,
                    values: vec![0, 1],
                }],
            }],
        }];

        let draft = profile_draft(&fact, &suggestions).unwrap();
        assert!(draft.contains("rust-symbol = \"REVIEW_REQUIRED_RUST_SYMBOL\""));
        assert!(draft.contains("REVIEW_REQUIRED: supply unconstrained argument a0"));
        assert!(draft.contains("  0x00000001,"));
        assert!(draft.contains("address = 0x00004000\nvalue = 0x00000000"));
        assert!(draft.contains("address = 0x00004000\nvalue = 0x00000001"));
        assert!(draft.parse::<toml_edit::DocumentMut>().is_ok());
    }
}
