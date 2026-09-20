//! CLI command dispatch with command-specific ordering and side-effect boundaries.

use std::{env, path::PathBuf};

use clap::Parser as _;

use crate::{
    Result,
    cli::{Cli, CliCommand, DeviceCommand, ImageCommand, ReportCommand, ScenarioCommand},
    device, emit_json,
    execution::{firmware::RunFirmware, orchestration, preflight},
    fixture, image, lab, output, reporting, scenario,
};

pub(crate) fn run() -> Result<()> {
    let root = repository_root()?;
    let invocation = env::args_os().collect::<Vec<_>>();
    let cli = Cli::parse();
    output::reserve_machine_stdout()?;
    let _signals = oer_process::install_signal_handlers()?;
    let command = match cli.command {
        CliCommand::Fixture {
            command:
                crate::cli::FixtureCommand::Install {
                    provider,
                    dry_run,
                    adapter,
                },
        } => return fixture::install::run(&root, provider, dry_run, &adapter),
        command => command,
    };
    let lab_path = cli
        .lab_config
        .unwrap_or(lab::config::LabConfig::default_path()?);
    let catalog_path = root.join("hil/scenarios");

    match command {
        CliCommand::Fixture {
            command: crate::cli::FixtureCommand::Install { .. },
        } => unreachable!("install commands return before lab configuration is resolved"),
        CliCommand::Fixture {
            command: crate::cli::FixtureCommand::ProbePlan,
        } => {
            let plan: Vec<_> = (0..fixture::probe_load::model::REQUESTS)
                .filter_map(fixture::probe_load::model::request)
                .collect();
            emit_json(&plan, true)
        }
        CliCommand::Fixture {
            command:
                crate::cli::FixtureCommand::BluetoothCheck {
                    adapter,
                    dtm_version,
                },
        } => {
            let _software = fixture::software::SoftwareLease::acquire_one(
                open_esp_radio_hil_runner::fixture_install::Provider::LinuxBluetooth,
            )?;
            fixture::bluetooth::check(&root, adapter, dtm_version)
        }
        CliCommand::Fixture {
            command:
                crate::cli::FixtureCommand::BluetoothConnectReset {
                    adapter,
                    peer,
                    hold_ms,
                },
        } => {
            let _software = fixture::software::SoftwareLease::acquire_one(
                open_esp_radio_hil_runner::fixture_install::Provider::LinuxBluetooth,
            )?;
            fixture::bluetooth::connect_reset(&root, adapter, peer, hold_ms)
        }
        CliCommand::Archive { command } => crate::archive::run(&root, command),
        CliCommand::Fixture {
            command: crate::cli::FixtureCommand::Check { scenario: id },
        } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            let selected = catalog.get(&id)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = lab::requirements::Requirements::for_scenario(selected);
            let _software = fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            fixture::prepared::check_without_device(&root, &lab, selected)
        }
        CliCommand::Doctor(selection) => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = lab::requirements::Requirements::union(&selected);
            let _software = fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            lab::doctor::run(&root, &lab, &selected)
        }
        CliCommand::Plan {
            selection,
            out,
            network,
            proofs,
        } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            let plan =
                crate::campaign::Plan::create_for_checks(&catalog, &selected, network, &proofs)?;
            if let Some(path) = out {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                serde_json::to_writer_pretty(&mut file, &plan)?;
            }
            emit_json(&plan, true)
        }
        CliCommand::RunPlan {
            plan,
            check,
            source_include,
        } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            let plan: crate::campaign::Plan = serde_json::from_slice(&std::fs::read(plan)?)?;
            let (selected, network) = plan.resolve(&catalog)?;
            if check {
                return emit_json(&plan, true);
            }
            let snapshot = image::snapshot::capture(&root, &source_include)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = lab::requirements::Requirements::union(&selected);
            let _software = fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            fixture::network_helper::require_for(&lab, required)?;
            let _fixture = lab::lock::FixtureLock::acquire_for(&lab, required)?;
            orchestration::run_all(
                &root,
                &lab,
                &catalog,
                &selected,
                orchestration::SuiteSelection::Campaign(&plan),
                network,
                orchestration::Invocation {
                    arguments: invocation,
                    snapshot: Some(snapshot),
                },
            )
        }
        CliCommand::Scenario { command } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            match command {
                ScenarioCommand::List => emit_json(
                    &serde_json::json!({
                        "schema": scenario::SCENARIO_SCHEMA,
                        "scenarios": catalog.all(),
                    }),
                    true,
                ),
                ScenarioCommand::Validate { scenario: id } => {
                    if let Some(id) = id {
                        let _ = catalog.get(&id)?;
                    }
                    emit_json(
                        &serde_json::json!({
                            "schema": scenario::SCENARIO_SCHEMA,
                            "scenarios": catalog.all().len(),
                            "status": "valid"
                        }),
                        false,
                    )
                }
            }
        }
        CliCommand::Image { command } => match command {
            ImageCommand::Snapshot { source_include } => {
                let snapshot = image::snapshot::capture(&root, &source_include)?;
                emit_json(&snapshot, true)
            }
            ImageCommand::Mono { class } => image::mono::capture(&root, class),
            ImageCommand::Build {
                class,
                network,
                source_snapshot,
            } => {
                let artifacts = match source_snapshot {
                    Some(snapshot) => image::snapshot::build(&root, &snapshot, class, network)?,
                    None => image::build(&root, class, network)?,
                };
                image::print_artifacts(class, &artifacts, false)
            }
            ImageCommand::VerifyRebuild { class, trim_paths } => {
                image::verify_rebuild(&root, class, trim_paths)
            }
            ImageCommand::Flash { class, network } => {
                let artifacts = image::build(&root, class, network)?;
                let lab = lab::config::LabConfig::load(&lab_path)?;
                let _fixture = lab::lock::FixtureLock::acquire(&lab)?;
                device::flash(&root, &artifacts, &lab.device.serial)?;
                image::print_artifacts(class, &artifacts, true)
            }
            ImageCommand::Replay { run_id, class } => {
                let firmware =
                    crate::evidence::verify::archived_firmware(&root, "esp32s31", &run_id, class)?;
                let lab = lab::config::LabConfig::load(&lab_path)?;
                let _fixture = lab::lock::FixtureLock::acquire(&lab)?;
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
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let _fixture = lab::lock::FixtureLock::acquire(&lab)?;
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
            source_include,
            ap_scheduler,
            firmware_from,
            network,
        } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            let mut selected = catalog.get(&id)?.clone();
            preflight::configure_run_selection(
                &mut selected,
                ap_scheduler.map(Into::into),
                firmware_from.is_some(),
                network,
            )?;
            let snapshot = if firmware_from.is_none() {
                Some(image::snapshot::capture(&root, &source_include)?)
            } else {
                None
            };
            let firmware = match firmware_from {
                Some(run_id) => {
                    RunFirmware::Replay(Box::new(crate::evidence::verify::archived_firmware(
                        &root,
                        "esp32s31",
                        &run_id,
                        selected.image,
                    )?))
                }
                None => RunFirmware::BuildCurrent(network),
            };
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = lab::requirements::Requirements::for_scenario(&selected);
            let _software = fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            fixture::network_helper::require_for(&lab, required)?;
            let _fixture = lab::lock::FixtureLock::acquire_for(&lab, required)?;
            orchestration::run_one(
                &root,
                &lab,
                &catalog,
                &selected,
                firmware,
                orchestration::Invocation {
                    arguments: invocation,
                    snapshot,
                },
            )
        }
        CliCommand::RunAll {
            tag,
            network,
            source_include,
        } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let selected = crate::cli::Selection {
                scenario: None,
                tag: tag.clone(),
            }
            .resolve(&catalog)?;
            let snapshot = image::snapshot::capture(&root, &source_include)?;
            let required = lab::requirements::Requirements::union(&selected);
            let _software = fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            fixture::network_helper::require_for(&lab, required)?;
            let _fixture = lab::lock::FixtureLock::acquire_for(&lab, required)?;
            orchestration::run_all(
                &root,
                &lab,
                &catalog,
                &selected,
                orchestration::SuiteSelection::Catalog(orchestration::selection_description(&tag)),
                network,
                orchestration::Invocation {
                    arguments: invocation,
                    snapshot: Some(snapshot),
                },
            )
        }
    }
}

pub(crate) fn repository_root() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find(|path| path.join(".git").exists() && path.join("Cargo.toml").is_file())
        .map(PathBuf::from)
        .ok_or_else(|| "HIL runner must live inside the repository".into())
}
