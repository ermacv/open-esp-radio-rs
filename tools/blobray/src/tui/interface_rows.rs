//! Borrowed interface rows over the shared query and optional reviewed slots.

use crate::{
    InterfaceAssignmentFact, InterfaceCallFact, InterfaceDecodeBlockerFact,
    InterfaceDecodeFailureFact, InterfaceGapFact, InterfaceSlotSummary, InterfaceTableFact,
    InterfaceWorkspaceReport,
};

#[derive(Clone, Copy, serde::Serialize)]
#[serde(tag = "kind", content = "observation", rename_all = "kebab-case")]
pub(super) enum InterfaceRow<'a> {
    Slot(&'a InterfaceSlotSummary),
    Table(&'a InterfaceTableFact),
    Call(&'a InterfaceCallFact),
    Assignment(&'a InterfaceAssignmentFact),
    Gap(&'a InterfaceGapFact),
    DecodeBlocker(&'a InterfaceDecodeBlockerFact),
    AnalysisFailure(&'a InterfaceDecodeFailureFact),
}

pub(super) fn count(report: &InterfaceWorkspaceReport) -> usize {
    report.slots.len()
        + report.observations.as_ref().map_or(0, |facts| {
            facts.tables.len()
                + facts.calls.len()
                + facts.assignments.len()
                + facts.gaps.len()
                + facts.decode_blockers.len()
                + facts.analysis_failures.len()
        })
}

/// Resolve a display index without cloning observations or scanning preceding rows.
pub(super) fn at(report: &InterfaceWorkspaceReport, mut index: usize) -> Option<InterfaceRow<'_>> {
    if index < report.slots.len() {
        return report.slots.get(index).map(InterfaceRow::Slot);
    }
    index -= report.slots.len();
    let facts = report.observations.as_ref()?;
    macro_rules! group {
        ($field:ident, $variant:ident) => {
            if index < facts.$field.len() {
                return facts.$field.get(index).map(InterfaceRow::$variant);
            }
            index -= facts.$field.len();
        };
    }
    group!(tables, Table);
    group!(calls, Call);
    group!(assignments, Assignment);
    group!(gaps, Gap);
    group!(decode_blockers, DecodeBlocker);
    facts
        .analysis_failures
        .get(index)
        .map(InterfaceRow::AnalysisFailure)
}

impl InterfaceRow<'_> {
    pub(super) fn columns(self) -> [String; 3] {
        match self {
            Self::Slot(slot) => [
                slot.name.clone(),
                format!("{:+#x}", slot.offset),
                slot.review_state.label().into(),
            ],
            Self::Table(table) => [
                format!("table: {}", table.root.kind()),
                "unknown".into(),
                "candidate".into(),
            ],
            Self::Call(call) => [
                format!("call: {}", call.function),
                format!("{:#x}", call.site),
                "candidate".into(),
            ],
            Self::Assignment(assignment) => [
                format!("store: {}", assignment.function),
                format!("{:#x}", assignment.site),
                "candidate".into(),
            ],
            Self::Gap(gap) => [
                format!("gap: {}", gap.evidence.function),
                format!("{:#x}", gap.evidence.site),
                "unknown".into(),
            ],
            Self::DecodeBlocker(blocker) => [
                format!("decode: {}", blocker.function),
                format!("{:#x}", blocker.address),
                "unknown".into(),
            ],
            Self::AnalysisFailure(failure) => [
                format!("failure: {}", failure.function),
                "unknown".into(),
                "unknown".into(),
            ],
        }
    }

    pub(super) fn matches(self, query: &str) -> bool {
        (!matches!(self, Self::Slot(_)) && "unknown behavior".contains(query))
            || self
                .columns()
                .iter()
                .any(|value| value.to_ascii_lowercase().contains(query))
            || serde_json::to_string(&self)
                .expect("typed observation is serializable")
                .to_ascii_lowercase()
                .contains(query)
    }

    pub(super) fn evidence(self) -> String {
        serde_json::to_string_pretty(&self).expect("typed observation is serializable")
    }
}
