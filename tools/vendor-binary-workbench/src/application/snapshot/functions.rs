//! Function index and reviewed logical-type projection for workspace browsing.

use std::collections::{BTreeMap, BTreeSet};

use super::{ProjectSession, push_error};
use crate::{
    application::model::{
        DiagnosticRecord, FunctionMmioSiteSummary, FunctionReviewState, FunctionSelection,
        FunctionSummary, LogicalTypeBindingSummary, LogicalTypeFieldSummary, LogicalTypeSummary,
    },
    function_workspace::{FunctionReviewStatus, ReviewedMemoryObject},
    registers::RegisterFacts,
};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> (Vec<FunctionSummary>, Vec<LogicalTypeSummary>) {
    let Some(paths) = resolved.project.functions.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let workspace = match resolved.function_workspace() {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return (Vec::new(), Vec::new()),
        Err(error) => {
            push_error(diagnostics, "functions", error, Some(paths.pack.clone()));
            return (Vec::new(), Vec::new());
        }
    };
    let static_mmio = resolved
        .project
        .registers
        .as_ref()
        .filter(|paths| paths.facts.is_file())
        .and_then(|paths| match RegisterFacts::load(&paths.facts) {
            Ok(facts) => Some(static_mmio_by_function(&facts)),
            Err(error) => {
                push_error(
                    diagnostics,
                    "function-static-mmio",
                    error,
                    Some(paths.facts.clone()),
                );
                None
            }
        })
        .unwrap_or_default();
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
            let static_sites = static_mmio
                .get(&fact_function_key(fact))
                .cloned()
                .unwrap_or_default();
            let registers = fact
                .mmio_addresses
                .iter()
                .copied()
                .chain(static_sites.iter().map(|site| site.address))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
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
                registers,
                mmio_sites: static_sites,
                calls: fact.direct_calls,
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

pub(super) fn static_mmio_by_function(
    facts: &RegisterFacts,
) -> BTreeMap<String, Vec<FunctionMmioSiteSummary>> {
    let mut output = BTreeMap::<String, Vec<FunctionMmioSiteSummary>>::new();
    for register in &facts.registers {
        for site in &register.read_sites {
            output
                .entry(site.function.clone())
                .or_default()
                .push(FunctionMmioSiteSummary {
                    address: register.address,
                    width: register.width,
                    access: "read".to_owned(),
                    pc: site.pc,
                });
        }
        for site in &register.write_sites {
            output
                .entry(site.function.clone())
                .or_default()
                .push(FunctionMmioSiteSummary {
                    address: register.address,
                    width: register.width,
                    access: "write".to_owned(),
                    pc: site.pc,
                });
        }
    }
    for sites in output.values_mut() {
        sites.sort_by_key(|site| (site.pc, site.address, site.width, site.access.clone()));
        sites.dedup();
    }
    output
}

pub(super) fn fact_function_key(fact: &crate::function_workspace::FunctionFact) -> String {
    fact.member.as_deref().map_or_else(
        || format!("{}:{}", fact.source, fact.symbol),
        |member| format!("{}:{}:{}", fact.source, member, fact.symbol),
    )
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
        ReviewedMemoryObject::Dereferenced {
            pointer,
            pointer_offset,
        } => format!("*({}{pointer_offset:+#x})", reviewed_memory_label(pointer)),
        ReviewedMemoryObject::Absolute {
            address_space,
            address,
        } => format!("absolute:{address_space}:{address:#010x}"),
    }
}

fn review_status(status: FunctionReviewStatus) -> FunctionReviewState {
    match status {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::{FactRange, RegisterAccessSiteFact, RegisterFact};

    #[test]
    fn artifact_wide_mmio_sites_survive_incomplete_linked_ir() {
        let facts = RegisterFacts {
            ranges: vec![FactRange {
                name: "radio".to_owned(),
                start: 0x2010_0000,
                end: 0x2011_0000,
            }],
            registers: vec![RegisterFact {
                address: 0x2010_4090,
                width: 32,
                catalog_name: "radio.REG_20104090".to_owned(),
                reads: 1,
                writes: 0,
                read_functions: ["libpp:wdev_record_rx_linked_list".to_owned()].into(),
                write_functions: BTreeSet::new(),
                read_sites: [RegisterAccessSiteFact {
                    function: "libpp:wdev_record_rx_linked_list".to_owned(),
                    pc: 0x1002_3562,
                }]
                .into(),
                write_sites: BTreeSet::new(),
                write_patterns: Vec::new(),
                candidate_masks: Vec::new(),
            }],
        };

        let sites = static_mmio_by_function(&facts);
        assert_eq!(
            sites["libpp:wdev_record_rx_linked_list"],
            [FunctionMmioSiteSummary {
                address: 0x2010_4090,
                width: 32,
                access: "read".to_owned(),
                pc: 0x1002_3562,
            }]
        );
    }
}
