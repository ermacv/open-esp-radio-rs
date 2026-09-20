//! Explanations derive from explicit declarations and the existing evaluator.

use super::*;

fn action(
    entry: &Entry,
    kind: WorkKind,
    subject: &str,
    reason: &str,
    origin: &'static str,
) -> Action {
    Action {
        entry: entry.id.clone(),
        entry_kind: entry.kind,
        kind,
        subject: subject.into(),
        reason: reason.into(),
        origin,
    }
}

pub(super) fn capability(entry: &Entry, document: &CapabilityDocument) -> Vec<Action> {
    let mut actions = Vec::new();
    for gap in &document.gaps {
        let (kind, reason, origin) = if let Some(work) = document
            .development
            .gap_work
            .iter()
            .find(|work| work.gap == gap.id)
        {
            (work.kind, work.reason.as_str(), "gap-work")
        } else {
            match gap.axis {
                Axis::Implementation | Axis::Async => (
                    WorkKind::Implement,
                    "The reviewed scope retains this implementation or lifetime gap.",
                    "declared-gap",
                ),
                Axis::Host => (
                    WorkKind::HostTest,
                    "The reviewed host coverage retains this gap; a test selector alone cannot close it.",
                    "declared-gap",
                ),
                Axis::Vendor => (
                    WorkKind::InspectVendor,
                    "Inspect the existing research/comparison gap before selecting analysis or comparison work.",
                    "declared-gap",
                ),
                Axis::Hil => (
                    WorkKind::ReviewGap,
                    "Classify this HIL gap: existing experiment, new experiment, or missing measurement method. Its identifier is not parsed as a work instruction.",
                    "declared-gap",
                ),
            }
        };
        actions.push(action(entry, kind, &gap.id, reason, origin));
    }
    if entry.owners.is_empty() {
        actions.push(action(entry, WorkKind::LinkOwners, &entry.id, "No source contract identifies this capability's implementation owners; review and link the relevant scope.", "missing-link"));
    }
    if entry.knowledge.is_empty() {
        actions.push(action(
            entry,
            WorkKind::LinkKnowledge,
            &entry.id,
            "No canonical knowledge file is linked. This does not mean the hardware is unexplored.",
            "missing-link",
        ));
    }
    if document.development.host_tests.is_empty() {
        actions.push(action(
            entry,
            WorkKind::LinkHostTests,
            &entry.id,
            "No focused host test selector is linked. The declared coverage state is preserved.",
            "missing-link",
        ));
    }
    if let Some(evidence) = &entry.evidence {
        for decision in &evidence.hil_decisions {
            if let Some((kind, reason)) = decision.next_work() {
                actions.push(action(
                    entry,
                    kind,
                    &decision.scenario,
                    reason,
                    "evidence-decision",
                ));
            }
        }
        if !matches!(evidence.vendor, "qualified" | "not-applicable")
            && !document.gaps.iter().any(|g| g.axis == Axis::Vendor)
        {
            actions.push(action(entry, WorkKind::InspectVendor, &entry.id, "No eligible current vendor evidence closes the declared comparison; inspect the selected project's status and evidence inputs.", "evidence-decision"));
        }
    } else if !document.hil_requirements.is_empty() || document.vendor_not_applicable.is_none() {
        actions.push(action(entry, WorkKind::InspectEvidence, &entry.id, "This declarations-only map has not loaded results. Evaluate a selected program before deciding that another experiment is needed.", "not-evaluated"));
    }
    actions
}

pub(super) fn source(entry: &Entry) -> Vec<Action> {
    if matches!(
        entry.implementation.as_str(),
        "PARTIAL" | "ABSENT" | "DIAGNOSTIC" | "FAIL-CLOSED"
    ) {
        vec![action(
            entry,
            WorkKind::ReviewGap,
            &entry.id,
            "Review this source scope and its limits to choose research or implementation work; source status does not determine hardware feasibility.",
            "source-declaration",
        )]
    } else {
        Vec::new()
    }
}
