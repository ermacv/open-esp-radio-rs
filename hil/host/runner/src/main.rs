use std::{
    collections::BTreeMap,
    env,
    error::Error,
    ffi::OsString,
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};

use clap::{Parser, Subcommand};
use serde::Serialize;
use sha2::{Digest, Sha256};

mod device;
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

type Result<T> = std::result::Result<T, Box<dyn Error>>;
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
        std::process::exit(1);
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "cargo hil",
    about = "Open ESP radio hardware-in-the-loop runner"
)]
struct Cli {
    /// Complete local fixture configuration. Secrets never belong to scenarios.
    #[arg(long, global = true)]
    lab_config: Option<PathBuf>,
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Validate host tools, target device and the configured fixture.
    Doctor,
    /// Inspect and validate the host-owned scenario catalog.
    Scenario {
        #[command(subcommand)]
        command: ScenarioCommand,
    },
    /// Build or flash one reproducible firmware class.
    Image {
        #[command(subcommand)]
        command: ImageCommand,
    },
    /// Inspect the currently flashed target without starting a workload.
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Rebuild derived views from immutable run bundles.
    Report {
        #[command(subcommand)]
        command: ReportCommand,
    },
    /// Build, flash and execute one catalog scenario.
    Run {
        scenario: String,
        /// Use the scenario's image class from a sealed earlier HIL run.
        #[arg(long, value_name = "RUN_ID")]
        firmware_from: Option<String>,
    },
    /// Execute catalog scenarios, flashing once per selected image class.
    RunAll {
        /// Select only scenarios carrying this tag. May be repeated.
        #[arg(long)]
        tag: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ScenarioCommand {
    List,
    Validate { scenario: Option<String> },
}

#[derive(Debug, Subcommand)]
enum ImageCommand {
    Build {
        class: crate::image::ImageClass,
    },
    /// Build one clean commit in two different checkout roots and compare every firmware subject.
    VerifyRebuild {
        class: crate::image::ImageClass,
        /// Diagnose Cargo's experimental object-path sanitization without changing normal builds.
        #[arg(long)]
        trim_paths: bool,
    },
    Flash {
        class: crate::image::ImageClass,
    },
    /// Verify and flash an exact application archived by an earlier HIL run.
    Replay {
        run_id: String,
        class: crate::image::ImageClass,
    },
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    Status,
}

#[derive(Debug, Subcommand)]
enum ReportCommand {
    /// Rebuild history.json and history.html without attached hardware.
    Rebuild,
    /// Verify one run bundle, or every bundle when RUN_ID is omitted.
    Verify { run_id: Option<String> },
}

fn run() -> Result<()> {
    let root = repository_root()?;
    let invocation = env::args_os().collect::<Vec<_>>();
    let cli = Cli::parse();
    reserve_machine_stdout()?;
    let lab_path = cli
        .lab_config
        .unwrap_or(crate::lab::config::LabConfig::default_path()?);
    let catalog_path = root.join("hil/scenarios");
    match cli.command {
        CliCommand::Doctor => doctor(&root, &crate::lab::config::LabConfig::load(&lab_path)?),
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
                let _fixture = crate::lab::lock::FixtureLock::acquire(&root)?;
                device::flash(&root, &artifacts, &lab.device.serial)?;
                image::print_artifacts(class, &artifacts, true)
            }
            ImageCommand::Replay { run_id, class } => {
                let firmware =
                    crate::evidence::verify::archived_firmware(&root, "esp32s31", &run_id, class)?;
                let lab = crate::lab::config::LabConfig::load(&lab_path)?;
                let _fixture = crate::lab::lock::FixtureLock::acquire(&root)?;
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
            let _fixture = crate::lab::lock::FixtureLock::acquire(&root)?;
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
            let _fixture = crate::lab::lock::FixtureLock::acquire(&root)?;
            run_one(&root, &lab, &catalog, &selected, firmware, invocation)
        }
        CliCommand::RunAll { tag } => {
            let catalog = crate::scenario::Catalog::load(&catalog_path)?;
            let lab = crate::lab::config::LabConfig::load(&lab_path)?;
            let _fixture = crate::lab::lock::FixtureLock::acquire(&root)?;
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

fn doctor(root: &std::path::Path, lab: &crate::lab::config::LabConfig) -> Result<()> {
    let firmware = root.join("hil/targets/esp32s31/Cargo.toml");
    if !firmware.is_file() {
        return Err(format!("missing embedded HIL workspace: {}", firmware.display()).into());
    }
    eprintln!("repository={}", root.display());
    eprintln!("firmware_workspace={}", firmware.display());
    eprintln!("target={}", image::TARGET);
    eprintln!("qualified_profile={}", image::QUALIFIED_PROFILE);
    eprintln!("lab_config={}", lab.path().display());
    if !lab.device.serial.exists() {
        return Err(format!(
            "serial device does not exist: {}",
            lab.device.serial.display()
        )
        .into());
    }
    eprintln!("serial_device=PASS");
    let _lab_provenance = crate::lab::provenance::LabProvenance::capture(lab)?;
    eprintln!("lab_provenance=PASS");
    match &lab.station_fixture {
        crate::lab::config::StationFixtureConfig::LocalLinux(_) => {
            crate::fixture::controlled_ap::doctor_local()?;
            eprintln!("station_fixture=local-linux status=PASS");
        }
        crate::lab::config::StationFixtureConfig::OpenWrt(config) => {
            crate::fixture::openwrt_fixture::doctor(config)?;
            crate::fixture::openwrt_tx_monitor::doctor(config)?;
            crate::fixture::local_air_monitor::doctor(config)?;
            crate::fixture::controlled_openwrt_client::doctor(&lab.access_point, config)?;
            eprintln!("station_fixture=openwrt status=PASS");
        }
        crate::lab::config::StationFixtureConfig::External(_) => {
            eprintln!("station_fixture=external status=UNMANAGED");
        }
    }
    crate::fixture::controlled_client::doctor()?;
    eprintln!("controlled_client=PASS");
    for program in [
        "cargo",
        "llvm-objcopy",
        "llvm-objdump",
        "llvm-nm",
        "espflash",
    ] {
        image::require_program(program)?;
        eprintln!("tool_{program}=PASS");
    }
    image::ensure_no_old_application_dependency(root)?;
    eprintln!("old_application_dependency=ABSENT");
    image::ensure_vendor_dependencies_absent(root)?;
    eprintln!("vendor_dependencies=ABSENT");
    emit_json(
        &serde_json::json!({
            "schema": crate::evidence::run::RUN_SCHEMA,
            "status": "passed",
            "target": "esp32s31",
            "cell_id": lab.cell_id(),
            "device_id": lab.device.id.as_str(),
            "lab_config": lab.path(),
        }),
        false,
    )?;
    Ok(())
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
        &artifacts.effective_embedded_lock,
    )?;
    session.record_event(
        "image-build-finished",
        None,
        Some(class),
        Some(crate::evidence::run::Outcome::Passed),
    )?;
    session.record_event("image-flash-started", None, Some(class), None)?;
    if let Err(error) = device::flash(root, &artifacts, &lab.device.serial) {
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
    let lab_provenance = crate::lab::provenance::LabProvenance::capture(lab)?;
    session.record_lab_provenance(&lab_provenance)?;
    session.record_event("lab-provenance-captured", None, None, None)?;
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
    Ok(session)
}

fn finish_run(
    session: crate::evidence::run::RunSession,
    results: Vec<crate::evidence::run::ScenarioResult>,
) -> Result<()> {
    let (suite, completion) = session.finish(results)?;
    emit_json(&completion, false)?;
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
        let relative = PathBuf::from("scenarios")
            .join(&selected.id)
            .join(format!("repetition-{number:03}"));
        let output = session.directory().join(&relative);
        fs::create_dir_all(&output)?;
        repetitions.push(run_scenario_repetition(
            lab, selected, number, &relative, &output,
        )?);
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
    let (outcome, failure, measurements) = match validate_flashed_image(lab, selected.image, output)
    {
        Err(error) => (
            crate::evidence::run::Outcome::Blocked,
            Some(crate::evidence::run::Failure::new(
                crate::evidence::run::FailureKind::Precondition,
                error.to_string(),
            )),
            Vec::new(),
        ),
        Ok(()) => {
            let evidence = execution::execute_workload(lab, selected, output);
            let failure = evidence.failure.map(|message| {
                crate::evidence::run::Failure::new(
                    crate::evidence::run::FailureKind::Scenario,
                    message,
                )
            });
            (
                if failure.is_some() {
                    crate::evidence::run::Outcome::Failed
                } else {
                    crate::evidence::run::Outcome::Passed
                },
                failure,
                evidence.measurements,
            )
        }
    };
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
    let capture = crate::session::SerialCapture::start_with_reset(&lab.device.serial);
    let capabilities = capture.request_capabilities(std::time::Duration::from_secs(10));
    let capture_result = capture.finish_to(&output.join("image-preflight"));
    let capabilities = capabilities?;
    capture_result?;

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

#[cfg(test)]
mod cli_tests;
