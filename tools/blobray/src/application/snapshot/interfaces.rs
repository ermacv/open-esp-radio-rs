//! Interface-workspace projection for the read-only workspace snapshot.

use super::{ProjectSession, push_error};
use crate::application::model::{
    DiagnosticRecord, InterfaceContractSummary, InterfaceReviewState, InterfaceSlotSummary,
    InterfaceWorkspaceReport,
};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> InterfaceWorkspaceReport {
    let Some(paths) = resolved.project.interfaces.as_ref() else {
        return empty(false, None, None);
    };
    let mut report = empty(true, Some(paths.facts.clone()), paths.pack.clone());
    let facts = match resolved.interface_facts() {
        Ok(Some(facts)) => facts.clone(),
        Ok(None) => return report,
        Err(error) => {
            report.observation_state = crate::InterfaceObservationState::Failed {
                reason: error.to_string(),
            };
            push_error(
                diagnostics,
                "interface-observations",
                error,
                Some(paths.facts.clone()),
            );
            return report;
        }
    };
    report.observed_slots = facts.observed_slots();
    report.unreviewed_slots = report.observed_slots;
    report.observation_state = crate::InterfaceObservationState::Available;
    report.observations = Some(facts);
    let workspace = match resolved.interface_workspace() {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return report,
        Err(error) => {
            push_error(diagnostics, "interface-review", error, paths.pack.clone());
            return report;
        }
    };
    let summary = workspace.summary();
    InterfaceWorkspaceReport {
        observations: report.observations,
        observation_state: report.observation_state,
        configured: true,
        facts: Some(paths.facts.clone()),
        pack: paths.pack.clone(),
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
                review_state: InterfaceReviewState::Reviewed,
                selector: None,
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
                call_sites: slot
                    .calls
                    .iter()
                    .map(|call| call.observation.site)
                    .collect(),
            })
            .chain(
                workspace
                    .unreviewed_observations()
                    .iter()
                    .map(|slot| InterfaceSlotSummary {
                        id: slot.id.clone(),
                        contract: slot.contract.clone(),
                        offset: slot.offset,
                        width: slot.width,
                        name: slot.selector.as_ref().map_or_else(
                            || format!("slot_{:x}", slot.offset),
                            |selector| format!("indexed_{selector}"),
                        ),
                        review_state: InterfaceReviewState::Unreviewed,
                        selector: slot.selector.clone(),
                        arguments: Vec::new(),
                        return_type: "unknown".to_owned(),
                        variadic: false,
                        semantic: None,
                        effects: Vec::new(),
                        replacement: None,
                        execution_model: None,
                        functions: slot.functions.clone(),
                        call_sites: slot.call_sites.clone(),
                    }),
            )
            .collect(),
    }
}

fn empty(
    configured: bool,
    facts: Option<std::path::PathBuf>,
    pack: Option<std::path::PathBuf>,
) -> InterfaceWorkspaceReport {
    InterfaceWorkspaceReport {
        observations: None,
        observation_state: if configured {
            crate::InterfaceObservationState::Missing
        } else {
            crate::InterfaceObservationState::NotConfigured
        },
        configured,
        facts,
        pack,
        observed_slots: 0,
        reviewed_slots: 0,
        unreviewed_slots: 0,
        contracts: Vec::new(),
        slots: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
