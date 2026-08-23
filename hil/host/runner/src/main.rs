use std::{
    collections::BTreeMap,
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use clap::{Parser, Subcommand};
use serde::Serialize;
use sha2::{Digest, Sha256};

mod device;
mod evidence;
mod image;
mod qualification;
mod reporting;
mod traffic;
mod transport;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Invalidates a previous successful qualification before a new run starts.
///
/// UART evidence is still overwritten on every terminal path, but a failed
/// run must never leave an older `report.md` looking like its result.
fn invalidate_previous_report(output: &Path) -> Result<()> {
    match fs::remove_file(output.join("report.md")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Starts one scenario from an empty generated-artifact directory.
///
/// A failed attempt is itself evidence. Keeping an older report beside the
/// new `result.json` makes that evidence ambiguous, especially for workloads
/// whose report is written only after every boot completes.
fn reset_scenario_output(output: &Path) -> Result<()> {
    match fs::remove_dir_all(output) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::create_dir_all(output)?;
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
    /// Execute one catalog scenario against the matching flashed image.
    Run { scenario: String },
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
        class: qualification::scenario::ImageClass,
    },
    Flash {
        class: qualification::scenario::ImageClass,
    },
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    Status,
}

fn run() -> Result<()> {
    let root = repository_root()?;
    let cli = Cli::parse();
    let lab_path = cli
        .lab_config
        .unwrap_or(transport::lab_config::LabConfig::default_path()?);
    let catalog_path = root.join("hil/scenarios");
    match cli.command {
        CliCommand::Doctor => doctor(&root, &transport::lab_config::LabConfig::load(&lab_path)?),
        CliCommand::Scenario { command } => {
            let catalog = qualification::scenario::Catalog::load(&catalog_path)?;
            match command {
                ScenarioCommand::List => {
                    println!("{}", serde_json::to_string_pretty(catalog.all())?);
                    Ok(())
                }
                ScenarioCommand::Validate { scenario } => {
                    if let Some(id) = scenario {
                        let _ = catalog.get(&id)?;
                    }
                    println!(
                        "{}",
                        serde_json::json!({
                            "schema": qualification::scenario::SCENARIO_SCHEMA,
                            "scenarios": catalog.all().len(),
                            "status": "valid"
                        })
                    );
                    Ok(())
                }
            }
        }
        CliCommand::Image { command } => {
            let (class, flash_requested) = match command {
                ImageCommand::Build { class } => (class, false),
                ImageCommand::Flash { class } => (class, true),
            };
            let artifacts = image::build(&root, class)?;
            if flash_requested {
                let lab = transport::lab_config::LabConfig::load(&lab_path)?;
                let _fixture = transport::fixture_lock::FixtureLock::acquire(&root)?;
                device::flash(&root, &artifacts, &lab.device.serial)?;
            }
            image::print_artifacts(class, &artifacts, flash_requested)
        }
        CliCommand::Device {
            command: DeviceCommand::Status,
        } => {
            let lab = transport::lab_config::LabConfig::load(&lab_path)?;
            let _fixture = transport::fixture_lock::FixtureLock::acquire(&root)?;
            device::status(&root, &lab)
        }
        CliCommand::Run { scenario: id } => {
            let catalog = qualification::scenario::Catalog::load(&catalog_path)?;
            let selected = catalog.get(&id)?.clone();
            let lab = transport::lab_config::LabConfig::load(&lab_path)?;
            let _fixture = transport::fixture_lock::FixtureLock::acquire(&root)?;
            run_scenario(&root, &lab, &selected)
        }
        CliCommand::RunAll { tag } => {
            let catalog = qualification::scenario::Catalog::load(&catalog_path)?;
            let lab = transport::lab_config::LabConfig::load(&lab_path)?;
            let _fixture = transport::fixture_lock::FixtureLock::acquire(&root)?;
            run_all(&root, &lab, &catalog, &tag)
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

fn doctor(root: &std::path::Path, lab: &transport::lab_config::LabConfig) -> Result<()> {
    let firmware = root.join("hil/targets/esp32s31/Cargo.toml");
    if !firmware.is_file() {
        return Err(format!("missing embedded HIL workspace: {}", firmware.display()).into());
    }
    println!("repository={}", root.display());
    println!("firmware_workspace={}", firmware.display());
    println!("target={}", image::TARGET);
    println!("qualified_profile={}", image::QUALIFIED_PROFILE);
    println!("lab_config={}", lab.path().display());
    if !lab.device.serial.exists() {
        return Err(format!(
            "serial device does not exist: {}",
            lab.device.serial.display()
        )
        .into());
    }
    println!("serial_device=PASS");
    match &lab.station_fixture {
        transport::lab_config::StationFixtureConfig::LocalLinux(_) => {
            transport::controlled_ap::doctor_local()?;
            println!("station_fixture=local-linux status=PASS");
        }
        transport::lab_config::StationFixtureConfig::OpenWrt(config) => {
            transport::openwrt_fixture::doctor(config)?;
            transport::openwrt_tx_monitor::doctor(config)?;
            transport::local_air_monitor::doctor(config)?;
            transport::controlled_openwrt_client::doctor(&lab.access_point, config)?;
            println!("station_fixture=openwrt status=PASS");
        }
        transport::lab_config::StationFixtureConfig::External(_) => {
            println!("station_fixture=external status=UNMANAGED");
        }
    }
    transport::controlled_client::doctor()?;
    println!("controlled_client=PASS");
    for program in [
        "cargo",
        "llvm-objcopy",
        "llvm-objdump",
        "llvm-nm",
        "espflash",
    ] {
        image::require_program(program)?;
        println!("tool_{program}=PASS");
    }
    image::ensure_no_old_application_dependency(root)?;
    println!("old_application_dependency=ABSENT");
    image::ensure_vendor_oracle_isolated(root)?;
    println!("vendor_oracle_default_graph=ABSENT");
    println!("result=PASS");
    Ok(())
}

fn boot_smoke(output: &Path, lab: &transport::lab_config::LabConfig) -> Result<()> {
    let capture = evidence::traffic_capture::SerialCapture::start_with_reset(&lab.device.serial);
    let result = capture.wait_for_boot_smoke(std::time::Duration::from_secs(10));
    capture.finish_to(output)?;
    result
}

fn run_all(
    root: &Path,
    lab: &transport::lab_config::LabConfig,
    catalog: &qualification::scenario::Catalog,
    tags: &[String],
) -> Result<()> {
    let selected = catalog
        .all()
        .iter()
        .filter(|entry| tags.iter().all(|tag| entry.tags.contains(tag)))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err("no HIL scenarios match the requested tags".into());
    }
    for entry in &selected {
        if let Some(link) = entry.link {
            lab.station_fixture.require_phy(link.phy)?;
        }
    }
    for class in qualification::scenario::ImageClass::ALL {
        let class_scenarios = selected
            .iter()
            .copied()
            .filter(|entry| entry.image == class)
            .collect::<Vec<_>>();
        if class_scenarios.is_empty() {
            continue;
        }
        let artifacts = image::build(root, class)?;
        device::flash(root, &artifacts, &lab.device.serial)?;
        for entry in class_scenarios {
            run_scenario(root, lab, entry)?;
        }
    }
    println!(
        "{}",
        serde_json::json!({"status": "passed", "scenarios": selected.len()})
    );
    Ok(())
}

fn run_scenario(
    root: &Path,
    lab: &transport::lab_config::LabConfig,
    selected: &qualification::scenario::Scenario,
) -> Result<()> {
    if let Some(link) = selected.link {
        lab.station_fixture.require_phy(link.phy)?;
    }
    let output = root.join("target/hil/esp32s31/runs").join(&selected.id);
    reset_scenario_output(&output)?;
    fs::write(
        output.join("resolved-scenario.json"),
        serde_json::to_vec_pretty(selected)?,
    )?;
    if selected.repetitions == 1 {
        return run_scenario_attempt(root, lab, selected, &output);
    }
    let mut attempts = Vec::with_capacity(usize::from(selected.repetitions));
    let mut failures = Vec::new();
    for number in 1..=selected.repetitions {
        let attempt_output = output.join(format!("attempt-{number:02}"));
        fs::create_dir_all(&attempt_output)?;
        fs::write(
            attempt_output.join("resolved-scenario.json"),
            serde_json::to_vec_pretty(selected)?,
        )?;
        let result = run_scenario_attempt(root, lab, selected, &attempt_output);
        match result {
            Ok(()) => attempts.push(serde_json::json!({
                "attempt": number,
                "status": "passed",
                "output": attempt_output,
            })),
            Err(error) => {
                let error = error.to_string();
                attempts.push(serde_json::json!({
                    "attempt": number,
                    "status": "failed",
                    "error": error,
                    "output": attempt_output,
                }));
                failures.push(format!("attempt {number}: {error}"));
            }
        }
    }
    let passed = failures.is_empty();
    let result_document = serde_json::json!({
        "schema": 2,
        "scenario": selected.id,
        "image": selected.image,
        "status": if passed { "passed" } else { "failed" },
        "required_repetitions": selected.repetitions,
        "attempts": attempts,
    });
    fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&result_document)?,
    )?;
    if !passed {
        return Err(format!(
            "scenario `{}` failed {}/{} repetitions: {}",
            selected.id,
            failures.len(),
            selected.repetitions,
            failures.join("; ")
        )
        .into());
    }
    println!("{}", serde_json::to_string(&result_document)?);
    Ok(())
}

fn run_scenario_attempt(
    root: &Path,
    lab: &transport::lab_config::LabConfig,
    selected: &qualification::scenario::Scenario,
    output: &Path,
) -> Result<()> {
    let result = validate_flashed_image(lab, selected.image, output)
        .and_then(|()| execute_workload(root, lab, selected, output));
    let result_document = match &result {
        Ok(()) => serde_json::json!({
            "schema": 1,
            "scenario": selected.id,
            "image": selected.image,
            "status": "passed"
        }),
        Err(error) => serde_json::json!({
            "schema": 1,
            "scenario": selected.id,
            "image": selected.image,
            "status": "failed",
            "error": error.to_string()
        }),
    };
    fs::write(
        output.join("result.json"),
        serde_json::to_vec_pretty(&result_document)?,
    )?;
    result?;
    if selected.repetitions == 1 {
        println!("{}", serde_json::to_string(&result_document)?);
    }
    Ok(())
}

fn validate_flashed_image(
    lab: &transport::lab_config::LabConfig,
    expected: qualification::scenario::ImageClass,
    output: &Path,
) -> Result<()> {
    if expected == qualification::scenario::ImageClass::BootSmoke {
        return Ok(());
    }
    let capture = evidence::traffic_capture::SerialCapture::start_with_reset(&lab.device.serial);
    let capabilities = capture.request_capabilities(std::time::Duration::from_secs(10));
    let capture_result = capture.finish_to(&output.join("image-preflight"));
    let capabilities = capabilities?;
    capture_result?;

    let observed = match (
        capabilities.features.driver_observation_evidence,
        capabilities.features.task_poll_evidence,
        capabilities.features.rx_delivery_evidence,
        capabilities.features.mac_irq_evidence,
        capabilities.features.psram_task_stack,
    ) {
        (false, false, false, false, true) => qualification::scenario::ImageClass::Performance,
        (true, false, false, false, true) => qualification::scenario::ImageClass::Correctness,
        (true, false, false, true, true) => qualification::scenario::ImageClass::DiagnosticMacIrq,
        (true, true, false, false, true) => qualification::scenario::ImageClass::DiagnosticTaskPoll,
        (true, false, true, false, true) => {
            qualification::scenario::ImageClass::DiagnosticRxDelivery
        }
        _ => {
            return Err(
                "flashed image advertises mutually exclusive diagnostic capabilities".into(),
            );
        }
    };
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

fn execute_workload(
    root: &Path,
    lab: &transport::lab_config::LabConfig,
    selected: &qualification::scenario::Scenario,
    output: &Path,
) -> Result<()> {
    use qualification::scenario::{Direction, Workload};

    lab.set_data_plane(selected.data_plane);

    // Ordinary station workloads own their AP fixture for the complete run.
    // Loss/absence and Wi-Fi-role workloads manage that lifetime internally;
    // target-AP and timebase workloads must not materialize a station AP.
    let _station_ap = matches!(
        &selected.workload,
        Workload::Udp { .. }
            | Workload::Tcp { .. }
            | Workload::Icmp { .. }
            | Workload::StationReconnect { .. }
            | Workload::StationAccessPoint { .. }
    )
    .then(|| {
        transport::controlled_ap::ControlledAp::start(
            &lab.station,
            &lab.station_fixture,
            selected
                .link
                .expect("validated station workload has a link expectation")
                .phy,
        )
    })
    .transpose()?;

    match &selected.workload {
        Workload::BootSmoke => boot_smoke(output, lab),
        Workload::Timebase {
            boots,
            intervals,
            period_millis,
        } => transport::timebase::run(
            transport::timebase::Config {
                boots: *boots,
                intervals: *intervals,
                period_millis: *period_millis,
            },
            output,
            lab,
        ),
        Workload::Udp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            payload_bytes,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--seconds", duration_seconds);
            push_option(&mut arguments, "--payload", payload_bytes);
            push_option(
                &mut arguments,
                "--phy",
                selected
                    .link
                    .expect("validated station workload has a link expectation")
                    .phy
                    .id(),
            );
            match direction {
                Direction::Rx => {
                    push_option(
                        &mut arguments,
                        "--rate",
                        rx_rate_bps.expect("validated RX rate"),
                    );
                    if let Some(floor) = selected.criteria.minimum_rx_bps {
                        push_option(&mut arguments, "--floor", floor);
                    }
                    traffic::rx_traffic::run(
                        arguments,
                        output,
                        lab,
                        selected.criteria.exact_delivery,
                        selected.criteria.require_no_beacon_loss,
                        selected.image != qualification::scenario::ImageClass::Performance,
                    )
                }
                Direction::Tx => {
                    if let Some(rate) = tx_rate_bps {
                        push_option(&mut arguments, "--rate", rate);
                    }
                    if let Some(floor) = selected.criteria.minimum_tx_bps {
                        push_option(&mut arguments, "--floor", floor);
                    }
                    traffic::tx_traffic::run(
                        arguments,
                        output,
                        lab,
                        selected.criteria.exact_delivery,
                        selected.criteria.require_no_beacon_loss,
                        selected.image != qualification::scenario::ImageClass::Performance,
                    )
                }
                Direction::Bidirectional => {
                    push_option(
                        &mut arguments,
                        "--rate",
                        rx_rate_bps.expect("validated RX rate"),
                    );
                    if let Some(rate) = tx_rate_bps {
                        push_option(&mut arguments, "--tx-rate", rate);
                    }
                    if let Some(floor) = selected.criteria.minimum_tx_bps {
                        push_option(&mut arguments, "--tx-floor", floor);
                    }
                    if let Some(floor) = selected.criteria.minimum_rx_bps {
                        push_option(&mut arguments, "--rx-floor", floor);
                    }
                    if let Some(floor) = selected.criteria.minimum_combined_bps {
                        push_option(&mut arguments, "--combined-floor", floor);
                    }
                    traffic::bidirectional::run(
                        arguments,
                        output,
                        lab,
                        traffic::bidirectional::RunPolicy {
                            require_exact_delivery: selected.criteria.exact_delivery,
                            require_no_beacon_loss: selected.criteria.require_no_beacon_loss,
                            capture_openwrt_tx_monitor_rx: selected.evidence.openwrt_tx_monitor_rx,
                            capture_independent_laptop_monitor_rx: selected
                                .evidence
                                .independent_laptop_monitor_rx,
                            require_driver_observation: selected.image
                                != qualification::scenario::ImageClass::Performance,
                        },
                    )
                }
            }
        }
        Workload::Tcp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            chunk_bytes,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--seconds", duration_seconds);
            push_option(&mut arguments, "--chunk", chunk_bytes);
            if let Some(rate) = rx_rate_bps {
                push_option(&mut arguments, "--rx-rate", rate);
            }
            if let Some(rate) = tx_rate_bps {
                push_option(&mut arguments, "--tx-rate", rate);
            }
            if let Some(floor) = selected.criteria.minimum_tx_bps {
                push_option(&mut arguments, "--tx-floor", floor);
            }
            if let Some(floor) = selected.criteria.minimum_rx_bps {
                push_option(&mut arguments, "--rx-floor", floor);
            }
            match direction {
                Direction::Rx => traffic::tcp_traffic::run_rx(
                    arguments,
                    output,
                    lab,
                    selected.criteria.require_no_beacon_loss,
                ),
                Direction::Tx => traffic::tcp_traffic::run_tx(
                    arguments,
                    output,
                    lab,
                    selected.criteria.require_no_beacon_loss,
                ),
                Direction::Bidirectional => traffic::tcp_traffic::run_bidirectional(
                    arguments,
                    output,
                    lab,
                    selected.criteria.require_no_beacon_loss,
                ),
            }
        }
        Workload::Icmp {
            count,
            interval_ms,
            timeout_ms,
            payload_bytes,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--count", count);
            push_option(&mut arguments, "--interval-ms", interval_ms);
            push_option(&mut arguments, "--timeout-ms", timeout_ms);
            push_option(&mut arguments, "--payload", payload_bytes);
            if let Some(maximum) = selected.criteria.maximum_lost {
                push_option(&mut arguments, "--max-lost", maximum);
            }
            if let Some(maximum) = selected.criteria.maximum_p95_ms {
                push_option(&mut arguments, "--max-p95-ms", maximum);
            }
            traffic::icmp_latency::run(
                arguments,
                output,
                lab,
                selected.criteria.require_no_beacon_loss,
            )
        }
        Workload::StationReconnect {
            cycles,
            boots,
            timeout_seconds,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--cycles", cycles);
            push_option(&mut arguments, "--boots", boots);
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            qualification::station_lifecycle::run(
                arguments,
                output,
                lab,
                selected.criteria.require_no_beacon_loss,
            )
        }
        Workload::StationApLoss { timeout_seconds } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            qualification::station_ap_loss::run(
                arguments,
                output,
                lab,
                selected
                    .link
                    .expect("validated AP-loss workload has a link expectation")
                    .phy,
            )
        }
        Workload::StationApAbsence { timeout_seconds } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            qualification::station_ap_absence::run(
                arguments,
                output,
                lab,
                selected
                    .link
                    .expect("validated AP-absence workload has a link expectation")
                    .phy,
            )
        }
        Workload::WifiRole {
            operation,
            timeout_seconds,
            channel,
            dwell_seconds,
            snapshot_length,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            if let Some(channel) = channel {
                push_option(&mut arguments, "--channel", channel);
            }
            if let Some(seconds) = dwell_seconds {
                push_option(&mut arguments, "--monitor-seconds", seconds);
            }
            if let Some(length) = snapshot_length {
                push_option(&mut arguments, "--snapshot-length", length);
            }
            transport::wifi_control::run(
                operation.id(),
                arguments,
                output,
                lab,
                selected
                    .link
                    .expect("validated Wi-Fi role has a link expectation")
                    .phy,
            )
        }
        Workload::MonitorCapture {
            timeout_seconds,
            duration_seconds,
            channel,
            snapshot_length,
        } => {
            let mut arguments = Vec::new();
            push_option(
                &mut arguments,
                "--output",
                root.join("target/hil/esp32s31/runs")
                    .join(&selected.id)
                    .join("capture.pcapng")
                    .display(),
            );
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            push_option(&mut arguments, "--seconds", duration_seconds);
            if let Some(channel) = channel {
                push_option(&mut arguments, "--channel", channel);
            }
            push_option(&mut arguments, "--snapshot-length", snapshot_length);
            transport::wifi_capture::run(
                arguments,
                output,
                lab,
                selected
                    .link
                    .expect("validated monitor capture has a link expectation")
                    .phy,
            )
        }
        Workload::AccessPoint {
            cycles,
            boots,
            timeout_seconds,
            client,
            traffic,
        } => qualification::access_point::run(
            qualification::access_point::Config {
                cycles: *cycles,
                boots: *boots,
                timeout: std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                client: *client,
                traffic: traffic.clone(),
                criteria: selected.criteria.clone(),
                expected_link: selected.link,
                require_driver_observation: selected.image
                    != qualification::scenario::ImageClass::Performance,
                require_rx_delivery_evidence: selected.image
                    == qualification::scenario::ImageClass::DiagnosticRxDelivery,
            },
            output,
            lab,
        ),
        Workload::StationAccessPoint {
            timeout_seconds,
            duration_seconds,
            rate_bps_per_flow,
            minimum_bps_per_flow,
            maximum_fairness_skew_percent,
            payload_bytes,
        } => qualification::station_access_point::run(
            qualification::station_access_point::Config {
                timeout: std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                duration: std::time::Duration::from_secs(u64::from(*duration_seconds)),
                rate_bps_per_flow: *rate_bps_per_flow,
                minimum_bps_per_flow: *minimum_bps_per_flow,
                maximum_fairness_skew_percent: *maximum_fairness_skew_percent,
                payload_bytes: usize::from(*payload_bytes),
            },
            output,
            lab,
        ),
        Workload::StationAccessPointReconnect { timeout_seconds } => {
            qualification::station_access_point_reconnect::run(
                std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                output,
                lab,
                selected
                    .link
                    .expect("validated paired reconnect has a link expectation")
                    .phy,
            )
        }
    }
}

fn push_option(arguments: &mut Vec<String>, name: &str, value: impl ToString) {
    arguments.push(name.to_owned());
    arguments.push(value.to_string());
}
