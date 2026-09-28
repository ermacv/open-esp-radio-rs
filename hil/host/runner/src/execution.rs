//! Dispatch from validated scenario workloads to their concrete host owners.

use std::path::Path;

use crate::scenario::Scenario;
use hil_core::{evidence::run::Failure, evidence::run::FailureKind, evidence::run::Outcome};

pub(crate) mod doctor;
pub(crate) mod firmware;
pub(crate) mod fixture_check;
pub(crate) mod orchestration;
pub(crate) mod preflight;
#[cfg(test)]
mod tests;
pub(crate) use hil_core::failure::classify;

#[derive(Default)]
pub(crate) struct ExecutionEvidence {
    pub(crate) measurements: Vec<hil_core::evidence::run::Measurement>,
    pub(crate) failure: Option<Failure>,
    pub(crate) interrupted: bool,
}

impl ExecutionEvidence {
    pub(crate) fn outcome(&self) -> Outcome {
        if self.interrupted {
            return Outcome::Interrupted;
        }
        match self.failure.as_ref().map(|failure| failure.kind) {
            None => Outcome::Passed,
            Some(FailureKind::Infrastructure) => Outcome::Broken,
            Some(_) => Outcome::Failed,
        }
    }
}

pub(crate) fn execute_workload(
    lab: &hil_core::lab::config::LabConfig,
    selected: &Scenario,
    output: &Path,
    fixture: &hil_wifi::fixture::prepared::Prepared,
) -> ExecutionEvidence {
    let context = hil_core::context::Context::new(lab, selected.plan().settings, output);
    let result = selected.family.run(output, &context, fixture);
    // A failure may be the target ending: ask it how, without resetting it.
    let post_mortem = result
        .as_ref()
        .err()
        .filter(|error| !oer_process::is_cancelled(&***error))
        .and_then(|_| {
            let elf = runtime_elf(output, selected.image().id());
            hil_core::post_mortem::inspect(&lab.device.serial, output, elf.as_deref())
        });
    let mut evidence = ExecutionEvidence {
        measurements: context.measurements.snapshot(),
        interrupted: result
            .as_ref()
            .err()
            .is_some_and(|error| oer_process::is_cancelled(&**error)),
        failure: result.err().map(|error| classify(&*error)),
    };
    if let Some(finding) = post_mortem {
        let _ = hil_core::durable::atomic_json(
            &output.join("post-mortem/post-mortem.json"),
            &serde_json::json!({
                "schema": 1,
                "boot": finding.boot,
                "checkpoints": finding.checkpoints,
                "failure": finding.failure,
                "workload_failure": evidence.failure,
            }),
        );
        if let Some(failure) = finding.failure {
            // The workload's own error stays in the recorded report above.
            evidence.failure = Some(failure);
        }
    }
    if oer_process::cancellation_requested() {
        evidence.interrupted = true;
        evidence.failure.get_or_insert_with(|| {
            Failure::new(FailureKind::Infrastructure, "run cancelled by signal")
        });
    }
    evidence
}

/// The runtime ELF the run archived for `image`, found from a repetition's
/// output directory inside the run bundle.
fn runtime_elf(output: &Path, image: &str) -> Option<std::path::PathBuf> {
    output
        .ancestors()
        .map(|directory| directory.join("firmware").join(image).join("runtime.elf"))
        .find(|elf| elf.is_file())
}
