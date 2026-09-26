//! CLI command dispatch with command-specific ordering and side-effect boundaries.

use std::env;

use clap::Parser as _;

use crate::scenario::{Catalog, requirements};
use crate::{
    Result, cli::Cli, cli::CliCommand, cli::DeviceCommand, cli::ImageCommand, cli::ReportCommand,
    cli::ScenarioCommand, emit_json, execution::firmware::RunFirmware, execution::orchestration,
    execution::preflight, fixture, repository_root,
};
use hil_core::{device, image, lab, output, scenario::SCENARIO_SCHEMA};

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
        CliCommand::Fixture {
            command: crate::cli::FixtureCommand::BuildHostapd,
        } => return fixture::hostapd::build(&root),
        command => command,
    };
    let lab_path = cli
        .lab_config
        .unwrap_or(lab::config::LabConfig::default_path()?);
    let catalog_path = root.join("hil/scenarios");

    match command {
        CliCommand::Fixture {
            command:
                crate::cli::FixtureCommand::Install { .. } | crate::cli::FixtureCommand::BuildHostapd,
        } => unreachable!("install and build commands return before lab configuration is resolved"),
        CliCommand::Fixture {
            command: crate::cli::FixtureCommand::ProbePlan,
        } => {
            let plan: Vec<_> = (0..hil_wifi::fixture::probe_load::model::REQUESTS)
                .filter_map(hil_wifi::fixture::probe_load::model::request)
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
            let _software = hil_core::fixture::software::SoftwareLease::acquire_one(
                oer_hil_fixture_install::Provider::LinuxBluetooth,
            )?;
            hil_bluetooth::fixture::bluetooth::check(&root, adapter, dtm_version)
        }
        CliCommand::Fixture {
            command:
                crate::cli::FixtureCommand::BluetoothConnectReset {
                    adapter,
                    peer,
                    hold_ms,
                },
        } => {
            let _software = hil_core::fixture::software::SoftwareLease::acquire_one(
                oer_hil_fixture_install::Provider::LinuxBluetooth,
            )?;
            hil_bluetooth::fixture::bluetooth::connect_reset(&root, adapter, peer, hold_ms)
        }
        CliCommand::Archive { command } => hil_core::archive::run(&root, command),
        CliCommand::Fixture {
            command: crate::cli::FixtureCommand::Check { scenario: id },
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = catalog.get(&id)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = selected.requirements();
            let _software =
                hil_core::fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            crate::execution::fixture_check::check_without_device(&root, &lab, selected)
        }
        CliCommand::Doctor(selection) => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = requirements(&selected);
            let _software =
                hil_core::fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            crate::execution::doctor::run(&root, &lab, &selected)
        }
        CliCommand::Plan {
            selection,
            out,
            network,
            proofs,
            qualification,
            capability,
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let plan = if let Some(manifest) = qualification {
                hil_core::campaign::Plan::from_qualification(
                    &root, &catalog, manifest, capability, network,
                )?
            } else {
                let selected = selection.resolve(&catalog)?;
                hil_core::campaign::Plan::create_for_checks(&catalog, &selected, network, &proofs)?
            };
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
            let catalog = Catalog::load(&catalog_path)?;
            let plan: hil_core::campaign::Plan = serde_json::from_slice(&std::fs::read(plan)?)?;
            let plan = plan.refresh(&root, &catalog)?;
            let (selected, network) = plan.resolve(&catalog)?;
            if check || selected.is_empty() {
                return emit_json(&plan, true);
            }
            let snapshot = image::snapshot::capture(&root, &source_include)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = requirements(&selected);
            let _software =
                hil_core::fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            hil_wifi::fixture::local::network_helper::require_for(&lab, required)?;
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
            let catalog = Catalog::load(&catalog_path)?;
            match command {
                ScenarioCommand::List => emit_json(
                    &serde_json::json!({
                        "schema": SCENARIO_SCHEMA,
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
                            "schema": SCENARIO_SCHEMA,
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
                    Some(snapshot) => {
                        let artifacts = image::snapshot::build(&root, &snapshot, class, network)?;
                        let record = hil_core::evidence::build_record::publish(
                            &root, &snapshot, class, &artifacts,
                        )?;
                        eprintln!("build_record={}", record.display());
                        artifacts
                    }
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
                let firmware = hil_core::evidence::verify::archived_firmware(
                    &root, "esp32s31", &run_id, class,
                )?;
                let lab = lab::config::LabConfig::load(&lab_path)?;
                let _fixture = lab::lock::FixtureLock::acquire(&lab)?;
                device::flash_archived(&root, &firmware, &lab.device.serial)?;
                emit_json(
                    &serde_json::json!({
                        "schema": hil_core::evidence::run::RUN_SCHEMA,
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
                let completion =
                    hil_core::evidence::reporting::history::rebuild(&root, "esp32s31")?;
                emit_json(&completion, false)
            }
            ReportCommand::Verify { run_id } => {
                let completion =
                    hil_core::evidence::verify::verify(&root, "esp32s31", run_id.as_deref())?;
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
            let catalog = Catalog::load(&catalog_path)?;
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
                    RunFirmware::Replay(Box::new(hil_core::evidence::verify::archived_firmware(
                        &root,
                        "esp32s31",
                        &run_id,
                        selected.image(),
                    )?))
                }
                None => RunFirmware::BuildCurrent(network),
            };
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = selected.requirements();
            let _software =
                hil_core::fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            hil_wifi::fixture::local::network_helper::require_for(&lab, required)?;
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
            let catalog = Catalog::load(&catalog_path)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let selected = crate::cli::Selection {
                scenario: None,
                tag: tag.clone(),
            }
            .resolve(&catalog)?;
            let snapshot = image::snapshot::capture(&root, &source_include)?;
            let required = requirements(&selected);
            let _software =
                hil_core::fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            hil_wifi::fixture::local::network_helper::require_for(&lab, required)?;
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
