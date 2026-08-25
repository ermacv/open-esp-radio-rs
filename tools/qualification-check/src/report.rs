use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

use crate::{Result, model::Qualification};

static REPORT_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    bundles: usize,
    completed: usize,
    passing: usize,
    current_clean_producer: usize,
    qualifying: usize,
    evaluator_dirty: bool,
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
    gaps: Vec<GapReport<'a>>,
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
        schema: 3,
        target: &qualification.target,
        repository_commit: &qualification.repository.commit,
        repository_dirty: qualification.repository.dirty,
        evidence_inputs: EvidenceInputsReport {
            verification_entries: qualification.evidence_inputs.verification_entries,
            verification_current_release_entries: qualification
                .evidence_inputs
                .verification_current_release_entries,
            hil: HilInputsReport {
                bundles: qualification.evidence_inputs.hil.bundles,
                completed: qualification.evidence_inputs.hil.completed,
                passing: qualification.evidence_inputs.hil.passing,
                current_clean_producer: qualification.evidence_inputs.hil.current_clean_producer,
                qualifying: qualification.evidence_inputs.hil.qualifying,
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

pub(crate) fn print(qualification: &Qualification) {
    println!(
        "INPUT\tverification-entries={}\tverification-current-release={}\thil-bundles={}\thil-completed={}\thil-passing={}\thil-current-clean-producer={}\thil-qualifying={}\tevaluator-dirty={}",
        qualification.evidence_inputs.verification_entries,
        qualification
            .evidence_inputs
            .verification_current_release_entries,
        qualification.evidence_inputs.hil.bundles,
        qualification.evidence_inputs.hil.completed,
        qualification.evidence_inputs.hil.passing,
        qualification.evidence_inputs.hil.current_clean_producer,
        qualification.evidence_inputs.hil.qualifying,
        qualification.evidence_inputs.hil.evaluator_dirty,
    );
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
    let parent = path.parent().ok_or_else(|| {
        format!(
            "qualification report path has no parent: {}",
            path.display()
        )
    })?;
    fs::create_dir_all(parent)?;
    let mut output = serde_json::to_vec_pretty(&report(qualification))?;
    output.push(b'\n');
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            format!(
                "qualification report path has no file name: {}",
                path.display()
            )
        })?
        .to_string_lossy();
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        REPORT_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&output)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
