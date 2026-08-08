//! Read-only projection of every configured project workspace.

mod comparisons;
mod functions;
mod interfaces;
mod registers;

use std::collections::{BTreeMap, BTreeSet};

use super::{ProjectSession, model::*};
use crate::function_workspace::{
    FunctionFact, FunctionMemoryObjectFact, FunctionReviewStatus, FunctionWorkspace,
    ReviewedFunction, ReviewedLogicalType, ReviewedMemoryObject,
};

pub(super) fn collect(resolved: &ProjectSession, generation: u64) -> WorkspaceSnapshot {
    let context = resolved.context();
    let status = crate::application::status::collect(&context);
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
    let (functions, logical_types) = self::functions::collect(resolved, &mut diagnostics);
    let registers = self::registers::collect(resolved, &mut diagnostics);
    let interfaces = self::interfaces::collect(resolved, &mut diagnostics);
    let comparisons = self::comparisons::collect(resolved, &mut diagnostics);
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

pub(super) fn function_detail(
    resolved: &ProjectSession,
    identity: &str,
) -> crate::Result<Option<FunctionDetailSummary>> {
    let Some(paths) = resolved.project.functions.as_ref() else {
        return Ok(None);
    };
    let reports = resolved.project.function_ir_reports()?;
    if reports.iter().any(|(_, path)| !path.is_file()) || !paths.pack.is_file() {
        return Ok(None);
    }
    let workspace = FunctionWorkspace::load(&reports, &paths.pack)?;
    let Some(fact) = workspace
        .facts
        .functions
        .iter()
        .find(|fact| fact.identity == identity)
    else {
        return Ok(None);
    };
    let reviewed = workspace.pack.functions.iter().find(|function| {
        function.profile == fact.profile
            && function.source == fact.source
            && function.identity == fact.identity
    });
    Ok(Some(function_detail_summary(
        fact,
        reviewed,
        &workspace.pack.types,
    )))
}

pub(super) fn register_detail(
    resolved: &ProjectSession,
    address: u32,
) -> crate::Result<Option<RegisterDetailSummary>> {
    self::registers::detail(resolved, address)
}

fn function_detail_summary(
    fact: &FunctionFact,
    reviewed: Option<&ReviewedFunction>,
    logical_types: &[ReviewedLogicalType],
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
        profile_draft: profile_draft(fact, &scenario_suggestions),
        scenario_suggestions,
        pseudo_rust: reviewed_pseudo(fact, reviewed, logical_types),
    }
}

fn profile_draft(fact: &FunctionFact, suggestions: &[ScenarioSuggestionSummary]) -> Option<String> {
    if suggestions.is_empty() {
        return None;
    }
    let mut output = format!(
        "# Generated coverage draft; replace TODO values and replay every case.\nprofile draft-{}\nvendor-source {}\nvendor-symbol {}\nrust-symbol TODO_RUST_SYMBOL\n",
        fact.symbol
            .replace(|character: char| !character.is_ascii_alphanumeric(), "-"),
        fact.source,
        fact.symbol,
    );
    for suggestion in suggestions {
        for variant in &suggestion.variants {
            output.push_str(&format!(
                "\n# {}: {}\ncase {}-{}\n",
                suggestion.kind, suggestion.evidence, suggestion.kind, variant.name
            ));
            let arguments = variant
                .arguments
                .iter()
                .map(|argument| (argument.index, argument.value))
                .collect::<BTreeMap<_, _>>();
            if let Some(maximum) = arguments.keys().max().copied() {
                for index in 0..=maximum {
                    if let Some(value) = arguments.get(&index) {
                        output.push_str(&format!("arg {value:#010x}\n"));
                    } else {
                        output.push_str(&format!(
                            "arg 0x00000000 # TODO: supply unconstrained argument a{index}\n"
                        ));
                    }
                }
            }
            for read in &variant.mmio_reads {
                for value in &read.values {
                    output.push_str(&format!("read {:#010x}={value:#010x}\n", read.address));
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
            selection: "symbol-prefix-root".to_owned(),
            direct_complete: true,
            call_graph_closed: true,
            context_projection_complete: true,
            context_projection_blockers: Vec::new(),
            reachable_functions: Vec::new(),
            calls: Vec::new(),
            mmio_addresses: vec![0x4000],
            context_fields: Vec::new(),
            memory_fields: Vec::new(),
            semantic_operations: Vec::new(),
            trampoline_calls: 0,
            event_dispatches: 0,
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
        assert!(draft.contains("rust-symbol TODO_RUST_SYMBOL"));
        assert!(draft.contains("TODO: supply unconstrained argument a0"));
        assert!(draft.contains("arg 0x00000001"));
        assert!(draft.contains("read 0x00004000=0x00000000"));
        assert!(draft.contains("read 0x00004000=0x00000001"));
    }
}
