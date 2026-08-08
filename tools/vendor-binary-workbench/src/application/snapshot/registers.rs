//! Register-workspace projection for the read-only workspace snapshot.

use super::{ProjectSession, push_error};
use crate::{
    application::model::{DiagnosticRecord, RegisterSummary, RegisterWorkspaceReport},
    registers::ProjectRegisterWorkspace,
};

pub(super) fn collect(
    resolved: &ProjectSession,
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
