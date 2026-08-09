//! Function index and reviewed logical-type projection for workspace browsing.

use super::{ProjectSession, push_error};
use crate::{
    application::model::{
        DiagnosticRecord, FunctionReviewState, FunctionSelection, FunctionSummary,
        LogicalTypeBindingSummary, LogicalTypeFieldSummary, LogicalTypeSummary,
    },
    function_workspace::{FunctionReviewStatus, FunctionWorkspace, ReviewedMemoryObject},
};

pub(super) fn collect(
    resolved: &ProjectSession,
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
                selection: selection(&fact.selection),
                review_status: reviewed.map_or(FunctionReviewState::Unreviewed, |function| {
                    review_status(function.status)
                }),
                reviewed_name: reviewed.and_then(|function| function.name.clone()),
                role: reviewed.and_then(|function| function.role.clone()),
                summary: reviewed.and_then(|function| function.summary.clone()),
                complete: fact.review_complete(),
                blockers,
                decode_blockers: fact.decode_blockers.len(),
                decode_blocker_classes: fact
                    .decode_blockers
                    .iter()
                    .map(|blocker| blocker.class.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                decode_blocker_operations: fact
                    .decode_blockers
                    .iter()
                    .map(|blocker| {
                        crate::artifact::unsupported_instruction_mnemonic(
                            blocker.width,
                            blocker.raw,
                        )
                        .to_owned()
                    })
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                semantic_operations: fact.semantic_operations.clone(),
                registers: fact.mmio_addresses.clone(),
                calls: fact.calls.len(),
            }
        })
        .collect::<Vec<_>>();
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
                    status: review_status(field.status),
                    name: field.name.clone(),
                    display_type: field.display_type.clone(),
                    description: field.description.clone(),
                })
                .collect(),
        })
        .collect();
    (functions, logical_types)
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

fn review_status(status: FunctionReviewStatus) -> FunctionReviewState {
    match status {
        FunctionReviewStatus::Unreviewed => FunctionReviewState::Unreviewed,
        FunctionReviewStatus::Reviewed => FunctionReviewState::Reviewed,
        FunctionReviewStatus::Ignored => FunctionReviewState::Ignored,
    }
}

fn selection(selection: &str) -> FunctionSelection {
    match selection {
        "symbol-prefix-root" => FunctionSelection::SymbolPrefixRoot,
        "reachable-internal" => FunctionSelection::ReachableInternal,
        _ => unreachable!("validated linked-IR function selection"),
    }
}
