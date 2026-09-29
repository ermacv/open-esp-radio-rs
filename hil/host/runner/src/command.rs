//! CLI command dispatch with command-specific ordering and side-effect boundaries.

use std::env;

use clap::Parser as _;

use crate::scenario::{Catalog, requirements};
use crate::{
    Result, cli::Cli, cli::CliCommand, cli::ImageCommand, cli::ReportCommand, cli::ScenarioCommand,
    emit_json, execution::firmware::RunFirmware, execution::orchestration, fixture,
    repository_root,
};
use hil_core::{image, lab, output, scenario::SCENARIO_SCHEMA};

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
            command: crate::cli::FixtureCommand::Check { scenario: id, chip },
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = catalog.get(&id)?;
            let chip = orchestration::select_chip(&root, &[selected], chip.as_deref())?;
            let lab = lab::config::LabConfig::load(&lab_path, &chip)?;
            // The stand's lease takes the fixture software once granted.
            crate::execution::fixture_check::check_without_device(&root, &lab, selected)
        }
        CliCommand::Doctor(selection) => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            let chip = orchestration::select_chip(&root, &selected, selection.chip.as_deref())?;
            let lab = lab::config::LabConfig::load(&lab_path, &chip)?;
            let required = requirements(&selected);
            let _software =
                hil_core::fixture::software::SoftwareLease::acquire_for(&lab, required)?;
            crate::execution::doctor::run(&root, &lab, &selected)
        }
        CliCommand::Plan { selection, proofs } => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            let plan = hil_core::campaign::Plan::create_for_checks(
                &catalog,
                &selected,
                image::Integration::default(),
                &proofs,
            )?;
            emit_json(&plan, true)
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
            ImageCommand::Build {
                classes,
                source_snapshot,
                layout_seed,
            } => {
                let frozen = source_snapshot
                    .as_deref()
                    .map(|snapshot| {
                        image::snapshot::FrozenSources::open_in_free_workspace(
                            snapshot,
                            &image::snapshot::build_slots("esp32s31")?,
                        )
                    })
                    .transpose()?;
                for class in classes {
                    let artifacts = match (&frozen, &source_snapshot) {
                        (Some(frozen), Some(snapshot)) => {
                            let artifacts = frozen.build(
                                class,
                                image::Integration::default(),
                                layout_seed,
                                &oer_hil_image_class::FeatureDelta::default(),
                            )?;
                            let record = hil_core::evidence::build_record::publish(
                                &root, snapshot, class, &artifacts,
                            )?;
                            eprintln!("build_record={}", record.display());
                            artifacts
                        }
                        _ => image::build(
                            &root,
                            class,
                            image::Integration::default(),
                            layout_seed,
                            &oer_hil_image_class::FeatureDelta::default(),
                        )?,
                    };
                    image::print_artifacts(class, &artifacts, false)?;
                }
                Ok(())
            }
        },
        CliCommand::Report {
            command: ReportCommand::Verify { run_id, chip },
        } => {
            use hil_core::evidence::verify;
            let chips = match (chip, run_id.as_deref()) {
                (Some(chip), _) => vec![chip],
                (None, Some(run)) => vec![verify::chip_of_run(&root, run)?],
                (None, None) => verify::chips_with_runs(&root)?,
            };
            for chip in chips {
                emit_json(&verify::verify(&root, &chip, run_id.as_deref())?, false)?;
            }
            Ok(())
        }
        CliCommand::Run {
            scenarios,
            source_include,
            include_untracked,
            source_snapshot,
            firmware_from,
            layout_seed,
            features,
            repetitions,
            then,
            chip,
            validate_only,
            build_only,
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let mut selected = orchestration::named_scenarios(&catalog, &scenarios)?;
            if let Some(repetitions) = repetitions {
                for scenario in &mut selected {
                    scenario.header.repetitions = repetitions;
                }
            }
            let chip = orchestration::select_chip(
                &root,
                &selected.iter().collect::<Vec<_>>(),
                chip.as_deref(),
            )?;
            // An image with other features is not its class's image: only an
            // experiment, whose runs never qualify, may build one.
            if features.as_ref().is_some_and(|delta| !delta.is_empty())
                && hil_core::experiment::Experiment::from_environment()?.is_none()
            {
                return Err(
                    "--features builds an experiment's image; use `cargo hil ab` with a \
                            `features=` variant"
                        .into(),
                );
            }
            if validate_only {
                println!(
                    "valid: {} on {chip}",
                    selected
                        .iter()
                        .map(|scenario| scenario.id())
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                return Ok(());
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
            if build_only {
                let snapshot = snapshot
                    .as_ref()
                    .ok_or("--build-only builds from the sources, not a replay")?;
                let selected = selected.iter().collect::<Vec<_>>();
                let frozen = image::snapshot::FrozenSources::open_in_free_workspace(
                    snapshot.directory(),
                    &image::snapshot::build_slots(&chip)?,
                )?;
                for class in orchestration::image_classes(&selected) {
                    let artifacts = frozen.build_for_chip(
                        &chip,
                        class,
                        image::Integration::default(),
                        layout_seed,
                        &features.clone().unwrap_or_default(),
                    )?;
                    image::print_artifacts(class, &artifacts, false)?;
                }
                return Ok(());
            }
            let firmware = match firmware_from {
                Some(run_id) => {
                    let class = orchestration::single_image_class(&selected)?;
                    RunFirmware::Replay(Box::new(hil_core::evidence::verify::archived_firmware(
                        &root, &chip, &run_id, class,
                    )?))
                }
                None => RunFirmware::BuildCurrent(hil_core::image::CurrentBuild {
                    network: image::Integration::default(),
                    layout_seed,
                    features: features.clone().unwrap_or_default(),
                }),
            };
            let lab = lab::config::LabConfig::load(&lab_path, &chip)?;
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
            layout_seed,
            source_include,
            include_untracked,
            all: _,
            exclude,
            chip,
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = crate::cli::Selection {
                scenario: None,
                tag: tag.clone(),
                chip: None,
            }
            .resolve(&catalog)?;
            let selected = excluding(selected, &exclude)?;
            let chip = orchestration::select_chip(&root, &selected, chip.as_deref())?;
            let lab = lab::config::LabConfig::load(&lab_path, &chip)?;
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
                orchestration::selection_description(&tag),
                hil_core::image::CurrentBuild {
                    network: image::Integration::default(),
                    layout_seed,
                    features: oer_hil_image_class::FeatureDelta::default(),
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

/// `selected` without the scenarios named in `exclude`, each reported.
fn excluding<'a>(
    selected: Vec<&'a crate::scenario::Scenario>,
    exclude: &[String],
) -> crate::Result<Vec<&'a crate::scenario::Scenario>> {
    let (left_out, kept): (Vec<_>, Vec<_>) = selected
        .into_iter()
        .partition(|scenario| exclude.iter().any(|id| id == scenario.id()));
    for scenario in left_out {
        eprintln!("skipping excluded scenario `{}`", scenario.id());
    }
    if kept.is_empty() {
        return Err("every selected HIL scenario is excluded".into());
    }
    Ok(kept)
}
