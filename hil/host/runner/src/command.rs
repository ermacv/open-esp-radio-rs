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
            // The stand's lease takes the fixture software once granted.
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
            include_untracked,
            layout_seed,
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let plan: hil_core::campaign::Plan = serde_json::from_slice(&std::fs::read(plan)?)?;
            let plan = plan.refresh(&root, &catalog)?;
            let (selected, network) = plan.resolve(&catalog)?;
            if check || selected.is_empty() {
                return emit_json(&plan, true);
            }
            let snapshot = image::snapshot::capture(&root, &source_include, include_untracked)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let required = requirements(&selected);
            hil_wifi::fixture::local::network_helper::require_for(&lab, required)?;
            // Orchestration builds the images, then leases the stand and
            // the fixture software.
            orchestration::run_all(
                &root,
                &lab,
                &catalog,
                &selected,
                orchestration::SuiteSelection::Campaign(&plan),
                hil_core::image::CurrentBuild {
                    network,
                    layout_seed,
                },
                orchestration::Invocation {
                    arguments: invocation,
                    snapshot: Some(snapshot),
                    then: None,
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
            ImageCommand::Snapshot {
                source_include,
                include_untracked,
            } => {
                let snapshot = image::snapshot::capture(&root, &source_include, include_untracked)?;
                emit_json(&snapshot, true)
            }
            ImageCommand::Mono { class } => image::mono::capture(&root, class),
            ImageCommand::Build {
                class,
                network,
                source_snapshot,
                layout_seed,
            } => {
                let artifacts = match source_snapshot {
                    Some(snapshot) => {
                        let artifacts =
                            image::snapshot::build(&root, &snapshot, class, network, layout_seed)?;
                        let record = hil_core::evidence::build_record::publish(
                            &root, &snapshot, class, &artifacts,
                        )?;
                        eprintln!("build_record={}", record.display());
                        artifacts
                    }
                    None => image::build(&root, class, network, layout_seed)?,
                };
                image::print_artifacts(class, &artifacts, false)
            }
            ImageCommand::VerifyRebuild { class, trim_paths } => {
                image::verify_rebuild(&root, class, trim_paths)
            }
            ImageCommand::Flash {
                class,
                network,
                layout_seed,
            } => {
                let artifacts = image::build(&root, class, network, layout_seed)?;
                let lab = lab::config::LabConfig::load(&lab_path)?;
                let _fixture = lab::lock::FixtureLock::acquire(&lab)?;
                device::flash(&root, &artifacts, &lab.device.serial)?;
                lab::lock::record_flash(
                    &lab.device.serial,
                    class.id(),
                    &artifacts.application_image,
                    None,
                    None,
                    String::from("image flash of the live checkout"),
                );
                image::print_artifacts(class, &artifacts, true)
            }
            ImageCommand::Replay { run_id, class } => {
                let firmware = hil_core::evidence::verify::archived_firmware(
                    &root, "esp32s31", &run_id, class,
                )?;
                let lab = lab::config::LabConfig::load(&lab_path)?;
                let _fixture = lab::lock::FixtureLock::acquire(&lab)?;
                device::flash_archived(&root, &firmware, &lab.device.serial)?;
                lab::lock::record_flash(
                    &lab.device.serial,
                    firmware.image.id(),
                    &firmware.application_path,
                    None,
                    None,
                    format!("image replay of run {}", firmware.run_id),
                );
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
            ReportCommand::Rebuild { target } => {
                let completion = hil_core::evidence::reporting::history::rebuild(&root, &target)?;
                emit_json(&completion, false)
            }
            ReportCommand::Verify { run_id, target } => {
                let completion =
                    hil_core::evidence::verify::verify(&root, &target, run_id.as_deref())?;
                emit_json(&completion, false)
            }
        },
        CliCommand::Run {
            scenarios,
            source_include,
            include_untracked,
            source_snapshot,
            ap_scheduler,
            firmware_from,
            network,
            layout_seed,
            then,
            target,
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let mut selected = orchestration::named_scenarios(&catalog, &scenarios)?;
            let target = orchestration::select_target(
                &selected.iter().collect::<Vec<_>>(),
                target.as_deref(),
            )?;
            for scenario in &mut selected {
                preflight::configure_run_selection(scenario, ap_scheduler.map(Into::into))?;
            }
            let snapshot = match (&firmware_from, source_snapshot) {
                (Some(_), _) => None,
                (None, Some(directory)) => Some(image::snapshot::Snapshot::load(&directory)?),
                (None, None) => Some(image::snapshot::capture(
                    &root,
                    &source_include,
                    include_untracked,
                )?),
            };
            let firmware = match firmware_from {
                Some(run_id) => {
                    let class = orchestration::single_image_class(&selected)?;
                    RunFirmware::Replay(Box::new(hil_core::evidence::verify::archived_firmware(
                        &root, &target, &run_id, class,
                    )?))
                }
                None => RunFirmware::BuildCurrent(hil_core::image::CurrentBuild {
                    network,
                    layout_seed,
                }),
            };
            let lab = lab::config::LabConfig::load(&lab_path)?.for_target(&target)?;
            orchestration::require_image_pipeline(&target)?;
            let selected = selected.iter().collect::<Vec<_>>();
            let required = requirements(&selected);
            hil_wifi::fixture::local::network_helper::require_for(&lab, required)?;
            let invocation = orchestration::Invocation {
                arguments: invocation,
                snapshot,
                then,
            };
            // Orchestration builds the images, then leases the stand and
            // the fixture software.
            match selected.as_slice() {
                [single] => {
                    orchestration::run_one(&root, &lab, &catalog, single, firmware, invocation)
                }
                _ => {
                    orchestration::run_many(&root, &lab, &catalog, &selected, firmware, invocation)
                }
            }
        }
        CliCommand::RunAll {
            tag,
            network,
            layout_seed,
            source_include,
            include_untracked,
            all: _,
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let lab = lab::config::LabConfig::load(&lab_path)?;
            let selected = crate::cli::Selection {
                scenario: None,
                tag: tag.clone(),
            }
            .resolve(&catalog)?;
            let snapshot = image::snapshot::capture(&root, &source_include, include_untracked)?;
            let required = requirements(&selected);
            hil_wifi::fixture::local::network_helper::require_for(&lab, required)?;
            // Orchestration builds the images, then leases the stand and
            // the fixture software.
            orchestration::run_all(
                &root,
                &lab,
                &catalog,
                &selected,
                orchestration::SuiteSelection::Catalog(orchestration::selection_description(&tag)),
                hil_core::image::CurrentBuild {
                    network,
                    layout_seed,
                },
                orchestration::Invocation {
                    arguments: invocation,
                    snapshot: Some(snapshot),
                    then: None,
                },
            )
        }
    }
}
