//! Dispatch from validated scenario workloads to their concrete host owners.

use std::path::Path;

use crate::scenario::Scenario;
use oer_hil_evidence::run::{Failure, FailureKind, Outcome};

pub(crate) mod doctor;
pub(crate) mod firmware;
pub(crate) mod fixture_check;
pub(crate) mod interrupt_stack;
pub(crate) mod orchestration;
pub(crate) mod preflight;
#[cfg(test)]
mod tests;
pub(crate) use oer_hil_execution::failure::classify;

#[derive(Default)]
pub(crate) struct ExecutionEvidence {
    pub(crate) measurements: Vec<oer_hil_evidence::run::Measurement>,
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
    lab: &oer_hil_stand::config::LabConfig,
    selected: &Scenario,
    output: &Path,
    fixture: &hil_wifi::fixture::prepared::Prepared,
) -> ExecutionEvidence {
    // The board's MAC outlives its port name, which a reset can change.
    let mac = oer_hil_stand::post_mortem::board_mac(&lab.dut.serial);
    let context = oer_hil_execution::context::Context::new(lab, selected.plan().settings, output)
        .with_profile(selected.header.profile);
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
            oer_hil_stand::post_mortem::inspect(
                &lab.dut.serial,
                mac.as_deref(),
                output,
                elf.as_deref(),
            )
        })
        .flatten();
    // A target that does not answer is read through its JTAG before any
    // reset erases where it stopped.
    if failed && post_mortem.is_none() {
        oer_hil_stand::post_mortem::jtag_snapshot_through_stand_openocd(
            lab.chip(),
            mac.as_deref(),
            output,
            elf.as_deref(),
        );
    }
    let recovery = (failed && post_mortem.is_none())
        .then(|| {
            let origin = output
                .ancestors()
                .find(|directory| directory.join("manifest.json").is_file())
                .and_then(|run| run.file_name())
                .map_or_else(String::new, |run| run.to_string_lossy().into_owned());
            oer_hil_stand::recovery::recover(
                &lab.dut.serial,
                mac.as_deref(),
                output,
                elf.as_deref(),
                &origin,
            )
        })
        .flatten();
    if let Some(oer_hil_stand::recovery::Recovery::Recovered { finding, .. }) = &recovery {
        post_mortem = Some((**finding).clone());
    }
    if let Some(oer_hil_stand::recovery::Recovery::BootedSilent { .. }) = &recovery {
        oer_hil_stand::recovery::mark_image_silent(selected.image().id());
    }
    let mut evidence = ExecutionEvidence {
        quarantined: recovery
            .as_ref()
            .is_some_and(oer_hil_stand::recovery::Recovery::quarantined),
        measurements: context.measurements.snapshot(),
        interrupted: result
            .as_ref()
            .err()
            .is_some_and(|error| oer_process::is_cancelled(&**error)),
        failure: result.err().map(|error| classify(&*error)),
    };
    check_interrupt_stacks(lab.chip(), elf.as_deref(), &mut evidence);
    if let Some(finding) = post_mortem {
        let _ = oer_hil_durable::atomic_json(
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

/// Hold each hart's observed interrupt-stack use of an ESP32-S31 repetition
/// to the image's static bound: a watermark above it fails the repetition,
/// as does an observed hart the bound cannot be computed for.
fn check_interrupt_stacks(chip: &str, elf: Option<&Path>, evidence: &mut ExecutionEvidence) {
    if chip != "esp32s31" {
        return;
    }
    let peaks = interrupt_stack::observed_peaks(&evidence.measurements);
    if peaks.is_empty() {
        return;
    }
    let evaluated = elf
        .ok_or_else(|| "the run archived no runtime ELF to bound the interrupt stacks".to_owned())
        .and_then(interrupt_stack::bounds)
        .and_then(|bounds| interrupt_stack::evaluate(&peaks, &bounds));
    match evaluated {
        Ok(measurements) => {
            let exceeded: Vec<String> = measurements
                .iter()
                .filter(|measurement| {
                    measurement.verdict == Some(oer_hil_evidence::run::MeasurementVerdict::Failed)
                })
                .map(|measurement| {
                    format!(
                        "{} {} > static bound {}",
                        measurement.name,
                        measurement.value,
                        measurement.threshold.map_or(0, |threshold| threshold.value)
                    )
                })
                .collect();
            evidence.measurements.extend(measurements);
            if !exceeded.is_empty() {
                evidence.failure.get_or_insert_with(|| {
                    Failure::new(
                        FailureKind::Scenario,
                        format!(
                            "observed interrupt-stack use exceeds the static bound: {}",
                            exceeded.join(", ")
                        ),
                    )
                });
            }
        }
        Err(error) => {
            evidence
                .failure
                .get_or_insert_with(|| Failure::new(FailureKind::Infrastructure, error));
        }
    }
}

/// The runtime ELF the run archived for `image`, found from a repetition's
/// output directory inside the run bundle.
fn runtime_elf(output: &Path, image: &str) -> Option<std::path::PathBuf> {
    output
        .ancestors()
        .map(|directory| directory.join("firmware").join(image).join("runtime.elf"))
        .find(|elf| elf.is_file())
}
