//! Reviewed executable-code boundary projection for workspace frontends.

use super::{ProjectSession, push_error};
use crate::{
    application::model::{
        CodeBoundaryControlFlowSummary, CodeBoundaryReviewState, CodeBoundarySummary,
        CodeWorkspaceReport, DiagnosticRecord,
    },
    code_workspace::CodeBoundaryStatus,
};

pub(super) fn collect(
    resolved: &ProjectSession,
    diagnostics: &mut Vec<DiagnosticRecord>,
) -> CodeWorkspaceReport {
    let Some(paths) = resolved.project.code.as_ref() else {
        return empty(false, None, None, None);
    };
    let facts_path = resolved
        .project
        .symbol_inventory
        .as_ref()
        .map(|symbols| symbols.output.clone());
    let Some(facts_path) = facts_path else {
        return empty(
            true,
            None,
            Some(paths.pack.clone()),
            paths.review_output.clone(),
        );
    };
    if !facts_path.is_file() || !paths.pack.is_file() {
        return empty(
            true,
            Some(facts_path),
            Some(paths.pack.clone()),
            paths.review_output.clone(),
        );
    }
    let workspace = match resolved.code_workspace() {
        Ok(Some(workspace)) => workspace,
        Ok(None) => {
            return empty(
                true,
                Some(facts_path),
                Some(paths.pack.clone()),
                paths.review_output.clone(),
            );
        }
        Err(error) => {
            push_error(
                diagnostics,
                "code-boundaries",
                error,
                Some(paths.pack.clone()),
            );
            return empty(
                true,
                Some(facts_path),
                Some(paths.pack.clone()),
                paths.review_output.clone(),
            );
        }
    };
    let summary = workspace.summary();
    CodeWorkspaceReport {
        configured: true,
        facts: Some(facts_path),
        pack: Some(paths.pack.clone()),
        review_output: paths.review_output.clone(),
        observed_candidates: summary.observed_candidates,
        accepted: summary.accepted,
        rejected: summary.rejected,
        unreviewed: summary.unreviewed,
        boundaries: workspace
            .entries()
            .map(|entry| CodeBoundarySummary {
                source: entry.review.source.clone(),
                artifact_sha256: entry.review.artifact_sha256.clone(),
                member: entry.review.member.clone(),
                object_kind: entry.fact.object_kind.clone(),
                section: entry.review.section.clone(),
                address: entry.fact.section_address + entry.review.entry_offset,
                entry_offset: entry.review.entry_offset,
                end_exclusive_offset: entry.review.end_exclusive_offset,
                end_limit_offset: entry.fact.end_limit_offset,
                status: match entry.review.status {
                    CodeBoundaryStatus::Unreviewed => CodeBoundaryReviewState::Unreviewed,
                    CodeBoundaryStatus::Accepted => CodeBoundaryReviewState::Accepted,
                    CodeBoundaryStatus::Rejected => CodeBoundaryReviewState::Rejected,
                },
                name: entry.review.name.clone(),
                reason: entry.review.reason.clone(),
                symbol_names: entry.fact.symbol_names.clone(),
                direct_control_flow: entry
                    .fact
                    .direct_control_flow
                    .iter()
                    .map(|edge| CodeBoundaryControlFlowSummary {
                        caller: edge.caller.clone(),
                        site_offset: edge.site_offset,
                        kind: edge.kind.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn empty(
    configured: bool,
    facts: Option<std::path::PathBuf>,
    pack: Option<std::path::PathBuf>,
    review_output: Option<std::path::PathBuf>,
) -> CodeWorkspaceReport {
    CodeWorkspaceReport {
        configured,
        facts,
        pack,
        review_output,
        observed_candidates: 0,
        accepted: 0,
        rejected: 0,
        unreviewed: 0,
        boundaries: Vec::new(),
    }
}
