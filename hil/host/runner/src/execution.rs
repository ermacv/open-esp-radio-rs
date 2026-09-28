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
    /// The stand quarantined the board after this repetition.
    pub(crate) quarantined: bool,
}

impl ExecutionEvidence {
    pub(crate) fn outcome(&self) -> Outcome {
        if self.interrupted {
            return Outcome::Interrupted;
        }
        if self.quarantined {
            return Outcome::BoardQuarantined;
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
    // The board's MAC outlives its port name, which a reset can change.
    let mac = hil_core::post_mortem::board_mac(&lab.device.serial);
    let context = hil_core::context::Context::new(lab, selected.plan().settings, output);
    let result = selected.family.run(output, &context, fixture);
    let elf = runtime_elf(output, selected.image().id());
    // A failure may be the target ending: ask it how, without resetting it;
    // a target that does not answer climbs the recovery ladder.
    let failed = result
        .as_ref()
        .err()
        .is_some_and(|error| !oer_process::is_cancelled(&**error));
    let mut post_mortem = failed
        .then(|| {
            hil_core::post_mortem::inspect(
                &lab.device.serial,
                mac.as_deref(),
                output,
                elf.as_deref(),
            )
        })
        .flatten();
    let recovery = (failed && post_mortem.is_none())
        .then(|| {
            let origin = output
                .ancestors()
                .find(|directory| directory.join("manifest.json").is_file())
                .and_then(|run| run.file_name())
                .map_or_else(String::new, |run| run.to_string_lossy().into_owned());
            hil_core::recovery::recover(
                &lab.device.serial,
                mac.as_deref(),
                output,
                elf.as_deref(),
                &origin,
            )
        })
        .flatten();
    if let Some(hil_core::recovery::Recovery::Recovered { finding, .. }) = &recovery {
        post_mortem = Some((**finding).clone());
    }
    if let Some(hil_core::recovery::Recovery::BootedSilent { .. }) = &recovery {
        hil_core::recovery::mark_image_silent(selected.image().id());
    }
    let mut evidence = ExecutionEvidence {
        quarantined: recovery
            .as_ref()
            .is_some_and(hil_core::recovery::Recovery::quarantined),
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
    if let Some(recovery) = &recovery {
        let failure = evidence.failure.get_or_insert_with(|| {
            Failure::new(FailureKind::Infrastructure, "the target stopped answering")
        });
        failure.message = format!("{}; {}", failure.message, recovery.describe());
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
