//! Suite, scenario and repetition lifecycle orchestration.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    Result, emit_json,
    evidence::run::{
        Failure, FailureKind, Outcome, PlanDisposition, PlanEntry, PlannedFirmware, RUN_SCHEMA,
        RepetitionResult, RunPlan, RunSession, ScenarioResult,
    },
    fixture,
    image::{ImageClass, Integration},
    lab::{config::LabConfig, requirements::Requirements},
    scenario::{Catalog, Scenario},
};

use super::{
    firmware::{self, RunFirmware},
    preflight,
};

pub(crate) fn selection_description(tags: &[String]) -> String {
    if tags.is_empty() {
        String::from("all scenarios")
    } else {
        format!("all scenarios with tags: {}", tags.join(", "))
    }
}

pub(crate) fn run_all(
    root: &Path,
    lab: &LabConfig,
    catalog: &Catalog,
    selected: &[&Scenario],
    selection: String,
    network: Integration,
    invocation: Vec<OsString>,
) -> Result<()> {
    let mut session = start_run(
        root,
        lab,
        catalog,
        selected,
        selection,
        Some(PlannedFirmware::BuildCurrent),
        invocation,
    )?;
    let results = {
        let mut operations = LiveSuite {
            root,
            lab,
            firmware: FirmwarePreparation::BuildCurrent(network),
        };
        execute_selected(&mut session, &mut operations, selected)?
    };
    finish_run(session, results)
}

pub(crate) fn run_one(
    root: &Path,
    lab: &LabConfig,
    catalog: &Catalog,
    selected: &Scenario,
    firmware: RunFirmware,
    invocation: Vec<OsString>,
) -> Result<()> {
    let selected_entries = [selected];
    let mut session = start_run(
        root,
        lab,
        catalog,
        &selected_entries,
        format!("scenario: {}", selected.id),
        Some(firmware.plan()),
        invocation,
    )?;
    let results = {
        let mut operations = LiveSuite {
            root,
            lab,
            firmware: FirmwarePreparation::Selected(&firmware),
        };
        execute_one(&mut session, &mut operations, selected)?
    };
    finish_run(session, results)
}

enum FirmwarePreparation<'a> {
    BuildCurrent(Integration),
    Selected(&'a RunFirmware),
}

struct LiveSuite<'a> {
    root: &'a Path,
    lab: &'a LabConfig,
    firmware: FirmwarePreparation<'a>,
}

trait SuiteEffects {
    fn check_cancelled(&mut self) -> Result<()>;
    fn preflight(&mut self, scenario: &Scenario) -> Option<Failure>;
    fn prepare_image(
        &mut self,
        class: ImageClass,
        session: &mut RunSession,
    ) -> Result<Option<Failure>>;
    fn execute_scenario(
        &mut self,
        scenario: &Scenario,
        session: &RunSession,
    ) -> Result<ScenarioResult>;
}

impl SuiteEffects for LiveSuite<'_> {
    fn check_cancelled(&mut self) -> Result<()> {
        oer_process::check_cancelled()
    }

    fn preflight(&mut self, scenario: &Scenario) -> Option<Failure> {
        preflight::scenario_failure(self.lab, scenario)
    }

    fn prepare_image(
        &mut self,
        class: ImageClass,
        session: &mut RunSession,
    ) -> Result<Option<Failure>> {
        match self.firmware {
            FirmwarePreparation::BuildCurrent(network) => {
                firmware::prepare_image(self.root, self.lab, class, network, session)
            }
            FirmwarePreparation::Selected(firmware) => {
                firmware::prepare_run_image(self.root, self.lab, class, firmware, session)
            }
        }
    }

    fn execute_scenario(
        &mut self,
        scenario: &Scenario,
        session: &RunSession,
    ) -> Result<ScenarioResult> {
        run_scenario(self.lab, scenario, session)
    }
}

fn execute_selected(
    session: &mut RunSession,
    effects: &mut impl SuiteEffects,
    selected: &[&Scenario],
) -> Result<Vec<ScenarioResult>> {
    let mut results = Vec::with_capacity(selected.len());
    for (class, class_scenarios) in group_selected_scenarios(selected) {
        effects.check_cancelled()?;
        let mut executable = Vec::with_capacity(class_scenarios.len());
        for scenario in class_scenarios {
            if let Some(failure) = effects.preflight(scenario) {
                session.record_event(
                    "scenario-blocked",
                    Some(&scenario.id),
                    Some(class),
                    Some(Outcome::Blocked),
                )?;
                results.push(write_blocked_scenario(session, scenario, failure)?);
            } else {
                executable.push(scenario);
            }
        }
        if executable.is_empty() {
            continue;
        }
        if let Some(failure) = effects.prepare_image(class, session)? {
            for scenario in executable {
                session.record_event(
                    "scenario-blocked",
                    Some(&scenario.id),
                    Some(class),
                    Some(Outcome::Blocked),
                )?;
                results.push(write_blocked_scenario(session, scenario, failure.clone())?);
            }
            continue;
        }
        for scenario in executable {
            effects.check_cancelled()?;
            session.record_event("scenario-started", Some(&scenario.id), Some(class), None)?;
            let result = effects.execute_scenario(scenario, session)?;
            session.record_event(
                "scenario-finished",
                Some(&scenario.id),
                Some(class),
                Some(result.outcome),
            )?;
            results.push(result);
        }
    }
    Ok(results)
}

fn execute_one(
    session: &mut RunSession,
    effects: &mut impl SuiteEffects,
    selected: &Scenario,
) -> Result<Vec<ScenarioResult>> {
    if let Some(failure) = effects.preflight(selected) {
        session.record_event(
            "scenario-blocked",
            Some(&selected.id),
            Some(selected.image),
            Some(Outcome::Blocked),
        )?;
        return Ok(vec![write_blocked_scenario(session, selected, failure)?]);
    }
    if let Some(failure) = effects.prepare_image(selected.image, session)? {
        session.record_event(
            "scenario-blocked",
            Some(&selected.id),
            Some(selected.image),
            Some(Outcome::Blocked),
        )?;
        return Ok(vec![write_blocked_scenario(session, selected, failure)?]);
    }
    session.record_event(
        "scenario-started",
        Some(&selected.id),
        Some(selected.image),
        None,
    )?;
    let result = effects.execute_scenario(selected, session)?;
    session.record_event(
        "scenario-finished",
        Some(&selected.id),
        Some(selected.image),
        Some(result.outcome),
    )?;
    Ok(vec![result])
}

fn group_selected_scenarios<'a>(selected: &[&'a Scenario]) -> Vec<(ImageClass, Vec<&'a Scenario>)> {
    ImageClass::ALL
        .into_iter()
        .filter_map(|class| {
            let scenarios = selected
                .iter()
                .copied()
                .filter(|entry| entry.image == class)
                .collect::<Vec<_>>();
            (!scenarios.is_empty()).then_some((class, scenarios))
        })
        .collect()
}

fn start_run(
    root: &Path,
    lab: &LabConfig,
    catalog: &Catalog,
    selected: &[&Scenario],
    selection: String,
    firmware: Option<PlannedFirmware>,
    invocation: Vec<OsString>,
) -> Result<RunSession> {
    let mut session = RunSession::create(
        root,
        "esp32s31",
        lab.cell_id(),
        &lab.device.id,
        &lab.device.serial,
        invocation,
    )?;
    let entries = catalog
        .all()
        .iter()
        .map(|scenario| {
            let is_selected = selected.iter().any(|entry| entry.id == scenario.id);
            PlanEntry {
                scenario: scenario.id.clone(),
                image: scenario.image,
                repetitions: scenario.repetitions,
                disposition: if is_selected {
                    PlanDisposition::Selected
                } else {
                    PlanDisposition::Filtered
                },
                reason: (!is_selected).then(|| format!("excluded by `{selection}`")),
                requirements: Some(Requirements::for_scenario(scenario)),
            }
        })
        .collect();
    session.write_plan(&RunPlan {
        schema: RUN_SCHEMA,
        run_id: session.id().to_owned(),
        selection,
        firmware,
        entries,
    })?;
    session.record_event("plan-resolved", None, None, None)?;
    for scenario in selected {
        let directory = session.scenario_directory(&scenario.id);
        fs::create_dir_all(&directory)?;
        crate::evidence::run::atomic_json(&directory.join("scenario.json"), scenario)?;
    }
    let required = Requirements::union(selected);
    let lab_provenance = crate::lab::provenance::LabProvenance::capture(lab, required)?;
    session.record_lab_provenance(&lab_provenance)?;
    session.record_event("lab-provenance-captured", None, None, None)?;
    Ok(session)
}

fn finish_run(session: RunSession, results: Vec<ScenarioResult>) -> Result<()> {
    oer_process::check_cancelled()?;
    let (suite, completion) = session.finish(results)?;
    emit_json(&completion, false)?;
    // Cancellation of a derived history update occurs after the run was
    // sealed. Publish completion first, then preserve the CLI signal status.
    oer_process::check_cancelled()?;
    if suite.outcome.is_passed() {
        Ok(())
    } else {
        Err(format!(
            "HIL run `{}` failed: {} passed, {} failed, {} blocked, {} broken",
            suite.run_id,
            suite.counts.passed,
            suite.counts.failed,
            suite.counts.blocked,
            suite.counts.broken,
        )
        .into())
    }
}

fn run_scenario(
    lab: &LabConfig,
    selected: &Scenario,
    session: &RunSession,
) -> Result<ScenarioResult> {
    let scenario_output = session.scenario_directory(&selected.id);
    fs::create_dir_all(&scenario_output)?;
    crate::evidence::run::atomic_json(&scenario_output.join("scenario.json"), selected)?;
    let mut repetitions = Vec::with_capacity(usize::from(selected.repetitions));
    for number in 1..=selected.repetitions {
        oer_process::check_cancelled()?;
        let relative = PathBuf::from("scenarios")
            .join(&selected.id)
            .join(format!("repetition-{number:03}"));
        let output = session.directory().join(&relative);
        fs::create_dir_all(&output)?;
        repetitions.push(run_scenario_repetition(
            lab, selected, number, &relative, &output,
        )?);
        oer_process::check_cancelled()?;
    }
    let result = ScenarioResult::from_repetitions(
        selected.id.clone(),
        selected.image,
        selected.repetitions,
        repetitions,
    );
    crate::evidence::run::atomic_json(&scenario_output.join("result.json"), &result)?;
    Ok(result)
}

fn run_scenario_repetition(
    lab: &LabConfig,
    selected: &Scenario,
    repetition: u8,
    artifacts: &Path,
    output: &Path,
) -> Result<RepetitionResult> {
    let resolved = lab.resolve_scenario(selected);
    let lab = &resolved;
    let started_unix_millis = crate::evidence::run::unix_millis()?;
    let started = std::time::Instant::now();
    let cleanup = fixture::cleanup::Scope::new(output);
    let (outcome, failure, measurements) =
        match fixture::prepared::Prepared::start(lab, selected, output).and_then(|fixture| {
            preflight::validate_flashed_image(lab, selected.image, output)?;
            Ok(fixture)
        }) {
            Err(error) => {
                let mut failure = super::classify(&*error);
                let outcome = if oer_process::is_cancelled(&*error)
                    || oer_process::cancellation_requested()
                {
                    Outcome::Interrupted
                } else if failure.kind == FailureKind::Infrastructure {
                    Outcome::Broken
                } else {
                    failure.kind = FailureKind::Precondition;
                    Outcome::Blocked
                };
                (outcome, Some(failure), Vec::new())
            }
            Ok(fixture) => {
                let evidence = super::execute_workload(lab, selected, output, &fixture);
                (evidence.outcome(), evidence.failure, evidence.measurements)
            }
        };
    finalize_repetition(
        repetition,
        artifacts,
        output,
        started_unix_millis,
        started,
        cleanup,
        outcome,
        failure,
        measurements,
    )
}

#[allow(clippy::too_many_arguments)]
fn finalize_repetition(
    repetition: u8,
    artifacts: &Path,
    output: &Path,
    started_unix_millis: u64,
    started: std::time::Instant,
    cleanup: fixture::cleanup::Scope,
    mut outcome: Outcome,
    mut failure: Option<Failure>,
    measurements: Vec<crate::evidence::run::Measurement>,
) -> Result<RepetitionResult> {
    let cleanup = cleanup.finish()?;
    let cleanup_failures = cleanup
        .iter()
        .filter_map(|record| record.failure.as_deref())
        .collect::<Vec<_>>();
    apply_cleanup_failures(&mut outcome, &mut failure, &cleanup_failures);
    let attachments = crate::evidence::run::collect_attachments(output, artifacts)?;
    let result = RepetitionResult {
        schema: RUN_SCHEMA,
        repetition,
        outcome,
        started_unix_millis,
        duration_millis: crate::evidence::run::duration_millis(started.elapsed()),
        artifact_directory: artifacts.to_owned(),
        attachments,
        measurements,
        failure,
    };
    crate::evidence::run::atomic_json(&output.join("result.json"), &result)?;
    Ok(result)
}

fn apply_cleanup_failures(
    outcome: &mut Outcome,
    failure: &mut Option<Failure>,
    cleanup_failures: &[&str],
) {
    if cleanup_failures.is_empty() {
        return;
    }
    let message = format!("fixture cleanup failed: {}", cleanup_failures.join("; "));
    if let Some(failure) = failure {
        failure.message.push_str(&format!("; {message}"));
    } else {
        *outcome = Outcome::Broken;
        *failure = Some(Failure::new(FailureKind::Infrastructure, message));
    }
}

fn write_blocked_scenario(
    session: &RunSession,
    selected: &Scenario,
    failure: Failure,
) -> Result<ScenarioResult> {
    let output = session.scenario_directory(&selected.id);
    fs::create_dir_all(&output)?;
    crate::evidence::run::atomic_json(&output.join("scenario.json"), selected)?;
    let result = ScenarioResult::blocked(
        selected.id.clone(),
        selected.image,
        selected.repetitions,
        failure,
    );
    crate::evidence::run::atomic_json(&output.join("result.json"), &result)?;
    Ok(result)
}

#[cfg(test)]
mod tests;
