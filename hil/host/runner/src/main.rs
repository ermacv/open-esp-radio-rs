use std::{
    env,
    error::Error,
    ffi::OsString,
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};

use crate::cli::{Cli, CliCommand, DeviceCommand, ImageCommand, ReportCommand, ScenarioCommand};
use clap::Parser;
use serde::Serialize;
use sha2::{Digest, Sha256};

mod cli;
mod device;
mod error;
mod evidence;
mod execution;
mod fixture;
mod image;
mod lab;
mod reporting;
mod scenario;
mod session;
mod transport;
mod workload;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
static MACHINE_STDOUT: OnceLock<Mutex<Box<dyn std::io::Write + Send>>> = OnceLock::new();

fn reserve_machine_stdout() -> Result<()> {
    #[cfg(unix)]
    let output: Box<dyn std::io::Write + Send> = {
        use std::os::fd::FromRawFd as _;

        // SAFETY: `dup` returns a new owned descriptor or -1. Ownership of a
        // successful descriptor is transferred exactly once to `File`.
        let descriptor = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: the descriptor was just created by `dup` and is uniquely
        // owned here.
        let file = unsafe { File::from_raw_fd(descriptor) };
        // SAFETY: both standard descriptors are process-owned and valid. This
        // redirects inherited child stdout to the diagnostic stderr stream.
        if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Box::new(file)
    };
    #[cfg(not(unix))]
    let output: Box<dyn std::io::Write + Send> = Box::new(std::io::stdout());

    MACHINE_STDOUT
        .set(Mutex::new(output))
        .map_err(|_| "machine stdout was initialized more than once".into())
}

pub(crate) fn emit_json(value: &impl Serialize, pretty: bool) -> Result<()> {
    let output = MACHINE_STDOUT
        .get()
        .ok_or("machine stdout is not initialized")?;
    let mut output = output
        .lock()
        .map_err(|_| "machine stdout lock is poisoned")?;
    if pretty {
        serde_json::to_writer_pretty(&mut **output, value)?;
    } else {
        serde_json::to_writer(&mut **output, value)?;
    }
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(if oer_process::is_cancelled(&*error) {
            130
        } else {
            1
        });
    }
}

fn run() -> Result<()> {
    let root = repository_root()?;
    let invocation = env::args_os().collect::<Vec<_>>();
    let cli = Cli::parse();
    reserve_machine_stdout()?;
    let _signals = oer_process::install_signal_handlers()?;
    let lab_path = cli
        .lab_config
        .unwrap_or(crate::lab::config::LabConfig::default_path()?);
    let catalog_path = root.join("hil/scenarios");
    match cli.command {
        CliCommand::Doctor(selection) => {
            let catalog = crate::scenario::Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            crate::lab::doctor::run(
                &root,
                &crate::lab::config::LabConfig::load(&lab_path)?,
                &selected,
            )
        }
        CliCommand::Plan(selection) => {
            let catalog = crate::scenario::Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            emit_json(
                &serde_json::json!({
                    "schema": 1,
                    "requirements": crate::lab::requirements::Requirements::union(&selected),
                    "scenarios": selected.iter().map(|scenario| serde_json::json!({
                        "scenario": scenario.id,
                        "image": scenario.image,
                        "repetitions": scenario.repetitions,
                        "requirements": crate::lab::requirements::Requirements::for_scenario(scenario),
                    })).collect::<Vec<_>>(),
                }),
                true,
            )
        }
        CliCommand::Scenario { command } => {
            let catalog = crate::scenario::Catalog::load(&catalog_path)?;
            match command {
                ScenarioCommand::List => {
                    emit_json(
                        &serde_json::json!({
                            "schema": crate::scenario::SCENARIO_SCHEMA,
                            "scenarios": catalog.all(),
                        }),
                        true,
                    )?;
                    Ok(())
                }
                ScenarioCommand::Validate { scenario } => {
                    if let Some(id) = scenario {
                        let _ = catalog.get(&id)?;
                    }
                    emit_json(
                        &serde_json::json!({
                            "schema": crate::scenario::SCENARIO_SCHEMA,
                            "scenarios": catalog.all().len(),
                            "status": "valid"
                        }),
                        false,
                    )?;
                    Ok(())
                }
            }
        }
        CliCommand::Image { command } => match command {
            ImageCommand::Build { class } => {
                let artifacts = image::build(&root, class)?;
                image::print_artifacts(class, &artifacts, false)
            }
            ImageCommand::VerifyRebuild { class, trim_paths } => {
                image::verify_rebuild(&root, class, trim_paths)
            }
            ImageCommand::Flash { class } => {
                let artifacts = image::build(&root, class)?;
                let lab = crate::lab::config::LabConfig::load(&lab_path)?;
                let _fixture = crate::lab::lock::FixtureLock::acquire(&lab)?;
                device::flash(&root, &artifacts, &lab.device.serial)?;
                image::print_artifacts(class, &artifacts, true)
            }
            ImageCommand::Replay { run_id, class } => {
                let firmware =
                    crate::evidence::verify::archived_firmware(&root, "esp32s31", &run_id, class)?;
                let lab = crate::lab::config::LabConfig::load(&lab_path)?;
                let _fixture = crate::lab::lock::FixtureLock::acquire(&lab)?;
                device::flash_archived(&root, &firmware, &lab.device.serial)?;
                emit_json(
                    &serde_json::json!({
                        "schema": crate::evidence::run::RUN_SCHEMA,
                        "run_id": firmware.run_id,
                        "image_class": firmware.image,
                        "application_image": firmware.application_path,
                        "application_sha256": firmware.application_sha256,
                        "flashed": true
                    }),
                    true,
                )
            }
        },
        CliCommand::Device {
            command: DeviceCommand::Status,
        } => {
            let lab = crate::lab::config::LabConfig::load(&lab_path)?;
            let _fixture = crate::lab::lock::FixtureLock::acquire(&lab)?;
            device::status(&root, &lab)
        }
        CliCommand::Report { command } => match command {
            ReportCommand::Rebuild => {
                let completion = reporting::history::rebuild(&root, "esp32s31")?;
                emit_json(&completion, false)
            }
            ReportCommand::Verify { run_id } => {
                let completion =
                    crate::evidence::verify::verify(&root, "esp32s31", run_id.as_deref())?;
                emit_json(&completion, false)
            }
        },
        CliCommand::Run {
            scenario: id,
            firmware_from,
        } => {
            let catalog = crate::scenario::Catalog::load(&catalog_path)?;
            let selected = catalog.get(&id)?.clone();
            let firmware = match firmware_from {
                Some(run_id) => {
                    RunFirmware::Replay(Box::new(crate::evidence::verify::archived_firmware(
                        &root,
                        "esp32s31",
                        &run_id,
                        selected.image,
                    )?))
                }
                None => RunFirmware::BuildCurrent,
            };
            let lab = crate::lab::config::LabConfig::load(&lab_path)?;
            let _fixture = crate::lab::lock::FixtureLock::acquire_for(
                &lab,
                crate::lab::requirements::Requirements::for_scenario(&selected),
            )?;
            run_one(&root, &lab, &catalog, &selected, firmware, invocation)
        }
        CliCommand::RunAll { tag } => {
            let catalog = crate::scenario::Catalog::load(&catalog_path)?;
            let lab = crate::lab::config::LabConfig::load(&lab_path)?;
            let selected = crate::cli::Selection {
                scenario: None,
                tag: tag.clone(),
            }
            .resolve(&catalog)?;
            let _fixture = crate::lab::lock::FixtureLock::acquire_for(
                &lab,
                crate::lab::requirements::Requirements::union(&selected),
            )?;
            run_all(&root, &lab, &catalog, &tag, invocation)
        }
    }
}

enum RunFirmware {
    BuildCurrent,
    Replay(Box<crate::evidence::verify::ArchivedFirmware>),
}

impl RunFirmware {
    fn plan(&self) -> crate::evidence::run::PlannedFirmware {
        match self {
            Self::BuildCurrent => crate::evidence::run::PlannedFirmware::BuildCurrent,
            Self::Replay(firmware) => crate::evidence::run::PlannedFirmware::Replay {
                source_run_id: firmware.run_id.clone(),
                image: firmware.image,
                build_id: firmware.build_id.clone(),
                application_sha256: firmware.application_sha256.clone(),
            },
        }
    }
}

fn repository_root() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find(|path| path.join(".git").exists() && path.join("Cargo.toml").is_file())
        .map(PathBuf::from)
        .ok_or_else(|| "HIL runner must live inside the repository".into())
}

fn run_all(
    root: &Path,
    lab: &crate::lab::config::LabConfig,
    catalog: &crate::scenario::Catalog,
    tags: &[String],
    invocation: Vec<OsString>,
) -> Result<()> {
    let selected = catalog
        .all()
        .iter()
        .filter(|entry| tags.iter().all(|tag| entry.tags.contains(tag)))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("no HIL scenarios match the requested tags".into());
    }
    let selection = if tags.is_empty() {
        String::from("all scenarios")
    } else {
        format!("all scenarios with tags: {}", tags.join(", "))
    };
    let mut session = start_run(
        root,
        lab,
        catalog,
        &selected,
        selection,
        Some(crate::evidence::run::PlannedFirmware::BuildCurrent),
        invocation,
    )?;
    let mut results = Vec::with_capacity(selected.len());
    for class in crate::image::ImageClass::ALL {
        oer_process::check_cancelled()?;
        let class_scenarios = selected
            .iter()
            .copied()
            .filter(|entry| entry.image == class)
            .collect::<Vec<_>>();
        if class_scenarios.is_empty() {
            continue;
        }
        let mut executable = Vec::with_capacity(class_scenarios.len());
        for entry in class_scenarios {
            if let Some(failure) = scenario_precondition(lab, entry) {
                session.record_event(
                    "scenario-blocked",
                    Some(&entry.id),
                    Some(class),
                    Some(crate::evidence::run::Outcome::Blocked),
                )?;
                results.push(write_blocked_scenario(&session, entry, failure)?);
            } else {
                executable.push(entry);
            }
        }
        if executable.is_empty() {
            continue;
        }
        if let Some(failure) = prepare_image(root, lab, class, &mut session)? {
            for entry in executable {
                session.record_event(
                    "scenario-blocked",
                    Some(&entry.id),
                    Some(class),
                    Some(crate::evidence::run::Outcome::Blocked),
                )?;
                results.push(write_blocked_scenario(&session, entry, failure.clone())?);
            }
            continue;
        }
        for entry in executable {
            oer_process::check_cancelled()?;
            session.record_event("scenario-started", Some(&entry.id), Some(class), None)?;
            let result = run_scenario(lab, entry, &session)?;
            session.record_event(
                "scenario-finished",
                Some(&entry.id),
                Some(class),
                Some(result.outcome),
            )?;
            results.push(result);
        }
    }
    finish_run(session, results)
}

fn run_one(
    root: &Path,
    lab: &crate::lab::config::LabConfig,
    catalog: &crate::scenario::Catalog,
    selected: &crate::scenario::Scenario,
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
    if let Some(failure) = scenario_precondition(lab, selected) {
        session.record_event(
            "scenario-blocked",
            Some(&selected.id),
            Some(selected.image),
            Some(crate::evidence::run::Outcome::Blocked),
        )?;
        let result = write_blocked_scenario(&session, selected, failure)?;
        return finish_run(session, vec![result]);
    }
    if let Some(failure) = prepare_run_image(root, lab, selected.image, &firmware, &mut session)? {
        session.record_event(
            "scenario-blocked",
            Some(&selected.id),
            Some(selected.image),
            Some(crate::evidence::run::Outcome::Blocked),
        )?;
        let result = write_blocked_scenario(&session, selected, failure)?;
        return finish_run(session, vec![result]);
    }
    session.record_event(
        "scenario-started",
        Some(&selected.id),
        Some(selected.image),
        None,
    )?;
    let result = run_scenario(lab, selected, &session)?;
    session.record_event(
        "scenario-finished",
        Some(&selected.id),
        Some(selected.image),
        Some(result.outcome),
    )?;
    finish_run(session, vec![result])
}

fn prepare_run_image(
    root: &Path,
    lab: &crate::lab::config::LabConfig,
    class: crate::image::ImageClass,
    firmware: &RunFirmware,
    session: &mut crate::evidence::run::RunSession,
) -> Result<Option<crate::evidence::run::Failure>> {
    match firmware {
        RunFirmware::BuildCurrent => prepare_image(root, lab, class, session),
        RunFirmware::Replay(archived) => prepare_replayed_image(root, lab, archived, session),
    }
}

fn prepare_replayed_image(
    root: &Path,
    lab: &crate::lab::config::LabConfig,
    archived: &crate::evidence::verify::ArchivedFirmware,
    session: &mut crate::evidence::run::RunSession,
) -> Result<Option<crate::evidence::run::Failure>> {
    session.record_event(
        "image-replay-import-started",
        None,
        Some(archived.image),
        None,
    )?;
    let application = match session.record_replayed_firmware(archived) {
        Ok(application) => application,
        Err(error) => {
            oer_process::check_cancelled()?;
            session.record_event(
                "image-replay-import-failed",
                None,
                Some(archived.image),
                Some(crate::evidence::run::Outcome::Broken),
            )?;
            return Err(error);
        }
    };
    session.record_event(
        "image-replay-import-finished",
        None,
        Some(archived.image),
        Some(crate::evidence::run::Outcome::Passed),
    )?;
    session.record_event("image-flash-started", None, Some(archived.image), None)?;
    if let Err(error) = device::flash_replayed(
        root,
        &application,
        session.id(),
        archived.image,
        &lab.device.serial,
    ) {
        oer_process::check_cancelled()?;
        session.record_event(
            "image-flash-failed",
            None,
            Some(archived.image),
            Some(crate::evidence::run::Outcome::Broken),
        )?;
        return Ok(Some(crate::evidence::run::Failure::new(
            crate::evidence::run::FailureKind::ImageFlash,
            error.to_string(),
        )));
    }
    session.record_event(
        "image-flash-finished",
        None,
        Some(archived.image),
        Some(crate::evidence::run::Outcome::Passed),
    )?;
    Ok(None)
}

fn scenario_precondition(
    lab: &crate::lab::config::LabConfig,
    selected: &crate::scenario::Scenario,
) -> Option<crate::evidence::run::Failure> {
    selected.link.and_then(|link| {
        lab.station_fixture
            .require_phy(link.phy)
            .err()
            .map(|error| {
                crate::evidence::run::Failure::new(
                    crate::evidence::run::FailureKind::Precondition,
                    error.to_string(),
                )
            })
    })
}

fn prepare_image(
    root: &Path,
    lab: &crate::lab::config::LabConfig,
    class: crate::image::ImageClass,
    session: &mut crate::evidence::run::RunSession,
) -> Result<Option<crate::evidence::run::Failure>> {
    session.record_event("image-build-started", None, Some(class), None)?;
    let mut artifacts = match image::build(root, class) {
        Ok(artifacts) => artifacts,
        Err(error) => {
            oer_process::check_cancelled()?;
            session.record_event(
                "image-build-failed",
                None,
                Some(class),
                Some(crate::evidence::run::Outcome::Broken),
            )?;
            return Ok(Some(crate::evidence::run::Failure::new(
                crate::evidence::run::FailureKind::ImageBuild,
                error.to_string(),
            )));
        }
    };
    artifacts.application_image = session.record_firmware(
        class,
        &artifacts.application_image,
        &artifacts.runtime_elf,
        &artifacts.runtime_bin,
        &artifacts.bootstrap_elf,
        (
            &artifacts.effective_embedded_lock,
            &artifacts.effective_bootstrap_lock,
        ),
    )?;
    session.record_event(
        "image-build-finished",
        None,
        Some(class),
        Some(crate::evidence::run::Outcome::Passed),
    )?;
    session.record_event("image-flash-started", None, Some(class), None)?;
    if let Err(error) = device::flash(root, &artifacts, &lab.device.serial) {
        oer_process::check_cancelled()?;
        session.record_event(
            "image-flash-failed",
            None,
            Some(class),
            Some(crate::evidence::run::Outcome::Broken),
        )?;
        return Ok(Some(crate::evidence::run::Failure::new(
            crate::evidence::run::FailureKind::ImageFlash,
            error.to_string(),
        )));
    }
    session.record_event(
        "image-flash-finished",
        None,
        Some(class),
        Some(crate::evidence::run::Outcome::Passed),
    )?;
    Ok(None)
}

fn start_run(
    root: &Path,
    lab: &crate::lab::config::LabConfig,
    catalog: &crate::scenario::Catalog,
    selected: &[&crate::scenario::Scenario],
    selection: String,
    firmware: Option<crate::evidence::run::PlannedFirmware>,
    invocation: Vec<OsString>,
) -> Result<crate::evidence::run::RunSession> {
    let mut session = crate::evidence::run::RunSession::create(
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
            crate::evidence::run::PlanEntry {
                scenario: scenario.id.clone(),
                image: scenario.image,
                repetitions: scenario.repetitions,
                disposition: if is_selected {
                    crate::evidence::run::PlanDisposition::Selected
                } else {
                    crate::evidence::run::PlanDisposition::Filtered
                },
                reason: (!is_selected).then(|| format!("excluded by `{selection}`")),
                requirements: Some(crate::lab::requirements::Requirements::for_scenario(
                    scenario,
                )),
            }
        })
        .collect();
    session.write_plan(&crate::evidence::run::RunPlan {
        schema: crate::evidence::run::RUN_SCHEMA,
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
    let required = crate::lab::requirements::Requirements::union(selected);
    let lab_provenance = crate::lab::provenance::LabProvenance::capture(lab, required)?;
    session.record_lab_provenance(&lab_provenance)?;
    session.record_event("lab-provenance-captured", None, None, None)?;
    Ok(session)
}

fn finish_run(
    session: crate::evidence::run::RunSession,
    results: Vec<crate::evidence::run::ScenarioResult>,
) -> Result<()> {
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
    lab: &crate::lab::config::LabConfig,
    selected: &crate::scenario::Scenario,
    session: &crate::evidence::run::RunSession,
) -> Result<crate::evidence::run::ScenarioResult> {
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
    let result = crate::evidence::run::ScenarioResult::from_repetitions(
        selected.id.clone(),
        selected.image,
        selected.repetitions,
        repetitions,
    );
    crate::evidence::run::atomic_json(&scenario_output.join("result.json"), &result)?;
    Ok(result)
}

fn run_scenario_repetition(
    lab: &crate::lab::config::LabConfig,
    selected: &crate::scenario::Scenario,
    repetition: u8,
    artifacts: &Path,
    output: &Path,
) -> Result<crate::evidence::run::RepetitionResult> {
    let started_unix_millis = crate::evidence::run::unix_millis()?;
    let started = std::time::Instant::now();
    let cleanup = crate::fixture::cleanup::Scope::new(output);
    let (mut outcome, mut failure, measurements) =
        match validate_flashed_image(lab, selected.image, output) {
            Err(error) => {
                use crate::evidence::run::{FailureKind, Outcome};
                let mut failure = execution::classify(&*error);
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
            Ok(()) => {
                let evidence = execution::execute_workload(lab, selected, output);
                (evidence.outcome(), evidence.failure, evidence.measurements)
            }
        };
    let cleanup = cleanup.finish()?;
    let cleanup_failures = cleanup
        .iter()
        .filter_map(|record| record.failure.as_deref())
        .collect::<Vec<_>>();
    if !cleanup_failures.is_empty() {
        let message = format!("fixture cleanup failed: {}", cleanup_failures.join("; "));
        if let Some(failure) = &mut failure {
            failure.message.push_str(&format!("; {message}"));
        } else {
            outcome = crate::evidence::run::Outcome::Broken;
            failure = Some(crate::evidence::run::Failure::new(
                crate::evidence::run::FailureKind::Infrastructure,
                message,
            ));
        }
    }
    let attachments = crate::evidence::run::collect_attachments(output, artifacts)?;
    let result = crate::evidence::run::RepetitionResult {
        schema: crate::evidence::run::RUN_SCHEMA,
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

fn write_blocked_scenario(
    session: &crate::evidence::run::RunSession,
    selected: &crate::scenario::Scenario,
    failure: crate::evidence::run::Failure,
) -> Result<crate::evidence::run::ScenarioResult> {
    let output = session.scenario_directory(&selected.id);
    fs::create_dir_all(&output)?;
    crate::evidence::run::atomic_json(&output.join("scenario.json"), selected)?;
    let result = crate::evidence::run::ScenarioResult::blocked(
        selected.id.clone(),
        selected.image,
        selected.repetitions,
        failure,
    );
    crate::evidence::run::atomic_json(&output.join("result.json"), &result)?;
    Ok(result)
}

fn validate_flashed_image(
    lab: &crate::lab::config::LabConfig,
    expected: crate::image::ImageClass,
    output: &Path,
) -> Result<()> {
    if expected == crate::image::ImageClass::BootSmoke {
        return Ok(());
    }
    let capture = crate::session::SerialCapture::start_with_reset(
        &lab.device.serial,
        &output.join("image-preflight"),
    )?;
    let capabilities = capture.request_capabilities(std::time::Duration::from_secs(10));
    let capabilities = capture.finish_with(capabilities)?;

    let observed = image::classify_flashed_capabilities(&capabilities.features)
        .ok_or("flashed image advertises mutually exclusive diagnostic capabilities")?;
    if observed != expected {
        return Err(format!(
            "scenario requires `{}` image but flashed target advertises `{}` capabilities",
            expected.id(),
            observed.id()
        )
        .into());
    }
    Ok(())
}
