use std::path::Path;

use serde::Serialize;

use crate::{Result, model::Qualification};

#[derive(Serialize)]
struct Summary {
    capabilities: usize,
    implementation_complete: usize,
    host_covered: usize,
    vendor_qualified: usize,
    hil_qualified: usize,
    async_terminal: usize,
    proof_ready: usize,
    ready: usize,
}

#[derive(Serialize)]
struct Report<'a> {
    schema: u16,
    target: &'a str,
    repository_commit: &'a str,
    repository_dirty: bool,
    evidence_inputs: EvidenceInputsReport,
    capabilities: Vec<CapabilityReport<'a>>,
    summary: Summary,
}

#[derive(Serialize)]
struct EvidenceInputsReport {
    verification_entries: usize,
    verification_current_release_entries: usize,
    hil: HilInputsReport,
}

#[derive(Serialize)]
struct HilInputsReport {
    observer_configuration_problem: Option<String>,
    directories: usize,
    bundles: usize,
    incomplete: usize,
    completed: usize,
    passing: usize,
    current_source_producer: usize,
    qualifying: usize,
    sealed_attempts: usize,
    shards: usize,
    current_shards: usize,
    evaluator_dirty: bool,
    invalid: Vec<crate::hil::InvalidRun>,
    /// Runs of another schema, by schema.
    unsupported: std::collections::BTreeMap<u64, usize>,
}

#[derive(Serialize)]
struct CapabilityReport<'a> {
    id: &'a str,
    title: &'a str,
    scope: &'a str,
    implementation: &'static str,
    host: &'static str,
    vendor: &'static str,
    hil: &'static str,
    r#async: &'static str,
    proof_ready: bool,
    ready: bool,
    dependencies: &'a [String],
    evidence: &'a [String],
    hil_checks: &'a [crate::model::HilCheckEvidence],
    hil_decisions: &'a [crate::hil::EvidenceDecision],
    gaps: Vec<GapReport<'a>>,
    source_contracts: &'a [crate::model::SourceContract],
}

#[derive(Serialize)]
struct GapReport<'a> {
    axis: &'static str,
    id: &'a str,
}

fn summary(qualification: &Qualification) -> Summary {
    Summary {
        capabilities: qualification.capabilities.len(),
        implementation_complete: qualification
            .capabilities
            .values()
            .filter(|capability| capability.implementation.is_terminal())
            .count(),
        host_covered: qualification
            .capabilities
            .values()
            .filter(|capability| capability.host.is_terminal())
            .count(),
        vendor_qualified: qualification
            .capabilities
            .values()
            .filter(|capability| capability.vendor.is_qualified())
            .count(),
        hil_qualified: qualification
            .capabilities
            .values()
            .filter(|capability| capability.hil.is_qualified())
            .count(),
        async_terminal: qualification
            .capabilities
            .values()
            .filter(|capability| capability.async_proof.is_terminal())
            .count(),
        proof_ready: qualification
            .capabilities
            .values()
            .filter(|capability| capability.proof_ready())
            .count(),
        ready: qualification.ready_count(),
    }
}

fn report(qualification: &Qualification) -> Report<'_> {
    Report {
        schema: 4,
        target: &qualification.target,
        repository_commit: &qualification.repository.commit,
        repository_dirty: qualification.repository.dirty,
        evidence_inputs: EvidenceInputsReport {
            verification_entries: qualification.evidence_inputs.verification_entries,
            verification_current_release_entries: qualification
                .evidence_inputs
                .verification_current_release_entries,
            hil: HilInputsReport {
                directories: qualification.evidence_inputs.hil.directories,
                bundles: qualification.evidence_inputs.hil.bundles,
                incomplete: qualification.evidence_inputs.hil.incomplete,
                completed: qualification.evidence_inputs.hil.completed,
                passing: qualification.evidence_inputs.hil.passing,
                current_source_producer: qualification.evidence_inputs.hil.current_source_producer,
                qualifying: qualification.evidence_inputs.hil.qualifying,
                sealed_attempts: qualification.evidence_inputs.hil.sealed_attempts,
                shards: qualification.evidence_inputs.hil.shards,
                current_shards: qualification.evidence_inputs.hil.current_shards,
                invalid: qualification.evidence_inputs.hil.invalid.clone(),
                unsupported: qualification.evidence_inputs.hil.unsupported.clone(),
                observer_configuration_problem: qualification
                    .evidence_inputs
                    .hil
                    .observer_configuration_problem
                    .clone(),
                evaluator_dirty: qualification.evidence_inputs.hil.evaluator_dirty,
            },
        },
        capabilities: qualification
            .capabilities
            .values()
            .map(|capability| CapabilityReport {
                id: &capability.id,
                title: &capability.title,
                scope: &capability.scope,
                implementation: capability.implementation.label(),
                host: capability.host.label(),
                vendor: capability.vendor.label(),
                hil: capability.hil.label(),
                r#async: capability.async_proof.label(),
                proof_ready: capability.proof_ready(),
                ready: qualification.is_ready(&capability.id),
                dependencies: &capability.dependencies,
                evidence: &capability.evidence,
                hil_checks: &capability.hil_checks,
                hil_decisions: &capability.hil_decisions,
                source_contracts: &capability.source_contracts,
                gaps: capability
                    .gaps
                    .iter()
                    .map(|gap| GapReport {
                        axis: gap.axis.label(),
                        id: &gap.id,
                    })
                    .collect(),
            })
            .collect(),
        summary: summary(qualification),
    }
}

/// One line per declared evidence directory that does not exist, and for a
/// vendor evidence index stale for the checkout: neither holds evidence.
pub(crate) fn absent_lines(absent: &[crate::model::AbsentDirectory]) -> Vec<String> {
    absent
        .iter()
        .map(|directory| {
            format!(
                "EVIDENCE-DIR\t{}\tkind={}\tpath={}\t{}=0",
                directory.state,
                directory.kind,
                directory.path.display(),
                if directory.kind == "hil-runs" {
                    "bundles"
                } else {
                    "shards"
                }
            )
        })
        .collect()
}

pub(crate) fn print(qualification: &Qualification) {
    println!(
        "INPUT\tverification-entries={}\tverification-current-release={}\thil-directories={}\thil-bundles={}\thil-incomplete={}\thil-completed={}\thil-passing={}\thil-current-source-producer={}\thil-qualifying={}\thil-sealed-attempts={}\thil-shards={}\thil-current-shards={}\thil-invalid-unsupported={}\tevaluator-dirty={}",
        qualification.evidence_inputs.verification_entries,
        qualification
            .evidence_inputs
            .verification_current_release_entries,
        qualification.evidence_inputs.hil.directories,
        qualification.evidence_inputs.hil.bundles,
        qualification.evidence_inputs.hil.incomplete,
        qualification.evidence_inputs.hil.completed,
        qualification.evidence_inputs.hil.passing,
        qualification.evidence_inputs.hil.current_source_producer,
        qualification.evidence_inputs.hil.qualifying,
        qualification.evidence_inputs.hil.sealed_attempts,
        qualification.evidence_inputs.hil.shards,
        qualification.evidence_inputs.hil.current_shards,
        qualification
            .evidence_inputs
            .hil
            .unsupported
            .values()
            .sum::<usize>(),
        qualification.evidence_inputs.hil.evaluator_dirty,
    );
    for line in absent_lines(&qualification.evidence_inputs.absent) {
        println!("{line}");
    }
    if let Some(problem) = &qualification
        .evidence_inputs
        .hil
        .observer_configuration_problem
    {
        println!("NOTICE\tcurrent-observer-configuration-unavailable\treason={problem}");
    }
    for (schema, runs) in &qualification.evidence_inputs.hil.unsupported {
        println!(
            "HIL-UNSUPPORTED\tschema={schema}\truns={runs}\tdisposition=excluded-from-evidence"
        );
    }
    for invalid in &qualification.evidence_inputs.hil.invalid {
        println!(
            "HIL-INVALID\t{}\tdisposition=excluded-from-evidence\treason={}",
            invalid.run, invalid.reason
        );
    }
    if qualification.evidence_inputs.hil.incomplete != 0 {
        println!(
            "NOTICE\thil-input-incomplete={}\treason=manifest-missing\tdisposition=ignored-as-evidence",
            qualification.evidence_inputs.hil.incomplete
        );
    }
    for capability in qualification.capabilities.values() {
        println!(
            "CAPABILITY\t{}\timplementation={}\thost={}\tvendor={}\thil={}\tasync={}\tproof-ready={}\tready={}",
            capability.id,
            capability.implementation.label(),
            capability.host.label(),
            capability.vendor.label(),
            capability.hil.label(),
            capability.async_proof.label(),
            capability.proof_ready(),
            qualification.is_ready(&capability.id),
        );
        for gap in &capability.gaps {
            println!(
                "GAP\t{}\taxis={}\tid={}",
                capability.id,
                gap.axis.label(),
                gap.id
            );
        }
        for decision in &capability.hil_decisions {
            println!(
                "HIL-OBLIGATION\t{}\tscenario={}\tstatus={}\tevidence={}",
                capability.id,
                decision.scenario,
                decision.status.label(),
                decision.evidence.as_deref().unwrap_or("none")
            );
        }
        for check in &capability.hil_checks {
            println!(
                "HIL-CHECK\t{}\tscenario={}\tcheck={}\tminimum-repetitions={}\tevidence={}",
                capability.id,
                check.scenario,
                check.check,
                check.minimum_repetitions,
                check.evidence.as_deref().unwrap_or("no-eligible-evidence")
            );
        }
    }
    let summary = summary(qualification);
    println!(
        "SUMMARY\ttarget={}\tcapabilities={}\timplementation-complete={}\thost-covered={}\tvendor-qualified={}\thil-qualified={}\tasync-terminal={}\tproof-ready={}\tready={}",
        qualification.target,
        summary.capabilities,
        summary.implementation_complete,
        summary.host_covered,
        summary.vendor_qualified,
        summary.hil_qualified,
        summary.async_terminal,
        summary.proof_ready,
        summary.ready,
    );
}

pub(crate) fn write_json(qualification: &Qualification, path: &Path) -> Result<()> {
    write_serialized(&report(qualification), path)
}

pub(crate) fn write_serialized(value: &impl Serialize, path: &Path) -> Result<()> {
    Ok(oer_durable::atomic_json(path, value).map_err(|error| error.to_string())?)
}
