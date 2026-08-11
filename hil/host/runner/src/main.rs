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

mod access_point_qualification;
mod bidirectional;
mod controlled_ap;
mod controlled_client;
mod fixture_lock;
mod icmp_latency;
mod lab_config;
mod openwrt_fixture;
mod paced_tcp;
mod paced_udp;
mod rx_delivery;
mod rx_traffic;
mod scenario;
mod stack_audit;
mod startup_artifact;
mod station_ap_absence;
mod station_ap_loss;
mod station_lifecycle;
mod tcp_traffic;
mod traffic_capture;
mod tx_traffic;
mod udp_socket;
mod wifi_capture;
mod wifi_control;

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

const QUALIFIED_PROFILE: &str = "psram-code-psram-data";
const TARGET: &str = "riscv32imafc-unknown-none-elf";
const RUNTIME_BIN: &str = "open-esp-radio-hil-esp32s31-runtime";
const BOOTSTRAP_BIN: &str = "open-esp-radio-hil-esp32s31-bootstrap";
const RUNTIME_MAGIC: u32 = 0x3247_5453;
const RUNTIME_CRC_OFFSET: usize = 40;
const RUNTIME_HEADER_BYTES: usize = 44;
const PARTITION_TABLE_OFFSET: u32 = 0x8000;
const OTA_SELECTOR_OFFSET: u32 = 0xd000;
const OTA_0_OFFSET: u32 = 0x1_0000;
const OTA_DATA_SIZE: usize = 0x2000;

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
    Build { class: scenario::ImageClass },
    Flash { class: scenario::ImageClass },
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
        .unwrap_or(lab_config::LabConfig::default_path()?);
    let catalog_path = root.join("hil/scenarios");
    match cli.command {
        CliCommand::Doctor => doctor(&root, &lab_config::LabConfig::load(&lab_path)?),
        CliCommand::Scenario { command } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
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
                            "schema": scenario::SCENARIO_SCHEMA,
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
            let artifacts = build(&root, class)?;
            if flash_requested {
                let lab = lab_config::LabConfig::load(&lab_path)?;
                let _fixture = fixture_lock::FixtureLock::acquire(&root)?;
                flash(&root, &artifacts, &lab.device.serial)?;
            }
            print_artifacts(class, &artifacts, flash_requested)
        }
        CliCommand::Device {
            command: DeviceCommand::Status,
        } => {
            let lab = lab_config::LabConfig::load(&lab_path)?;
            let _fixture = fixture_lock::FixtureLock::acquire(&root)?;
            device_status(&root, &lab)
        }
        CliCommand::Run { scenario: id } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            let selected = catalog.get(&id)?.clone();
            let lab = lab_config::LabConfig::load(&lab_path)?;
            let _fixture = fixture_lock::FixtureLock::acquire(&root)?;
            run_scenario(&root, &lab, &selected)
        }
        CliCommand::RunAll { tag } => {
            let catalog = scenario::Catalog::load(&catalog_path)?;
            let lab = lab_config::LabConfig::load(&lab_path)?;
            let _fixture = fixture_lock::FixtureLock::acquire(&root)?;
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

fn doctor(root: &std::path::Path, lab: &lab_config::LabConfig) -> Result<()> {
    let firmware = root.join("hil/targets/esp32s31/Cargo.toml");
    if !firmware.is_file() {
        return Err(format!("missing embedded HIL workspace: {}", firmware.display()).into());
    }
    println!("repository={}", root.display());
    println!("firmware_workspace={}", firmware.display());
    println!("target={TARGET}");
    println!("qualified_profile={QUALIFIED_PROFILE}");
    println!("lab_config={}", lab.path().display());
    if !lab.device.serial.exists() {
        return Err(format!(
            "serial device does not exist: {}",
            lab.device.serial.display()
        )
        .into());
    }
    println!("serial_device=PASS");
    openwrt_fixture::doctor(&lab.openwrt)?;
    println!("openwrt_fixture=PASS");
    controlled_client::doctor()?;
    println!("controlled_client=PASS");
    for program in ["cargo", "llvm-objcopy", "llvm-nm", "espflash"] {
        require_program(program)?;
        println!("tool_{program}=PASS");
    }
    ensure_no_old_application_dependency(root)?;
    println!("old_application_dependency=ABSENT");
    ensure_vendor_oracle_isolated(root)?;
    println!("vendor_oracle_default_graph=ABSENT");
    println!("result=PASS");
    Ok(())
}

#[derive(Serialize)]
struct ArtifactReport<'a> {
    image_class: &'a str,
    profile: &'a str,
    runtime_elf: String,
    runtime_bin: String,
    bootstrap_elf: String,
    application_image: String,
    application_sha256: String,
    flashed: bool,
}

fn print_artifacts(
    class: scenario::ImageClass,
    artifacts: &Artifacts,
    flashed: bool,
) -> Result<()> {
    let report = ArtifactReport {
        image_class: class.id(),
        profile: QUALIFIED_PROFILE,
        runtime_elf: artifacts.runtime_elf.display().to_string(),
        runtime_bin: artifacts.runtime_bin.display().to_string(),
        bootstrap_elf: artifacts.bootstrap_elf.display().to_string(),
        application_image: artifacts.application_image.display().to_string(),
        application_sha256: sha256_file(&artifacts.application_image)?,
        flashed,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(fs::read(path)?);
    Ok(format!("{:x}", digest.finalize()))
}

fn device_status(root: &Path, lab: &lab_config::LabConfig) -> Result<()> {
    device_status_at(&root.join("target/hil/esp32s31/device-status"), lab)
}

fn device_status_at(output: &Path, lab: &lab_config::LabConfig) -> Result<()> {
    let capture = traffic_capture::SerialCapture::start_with_reset(&lab.device.serial);
    let result = (|| -> Result<_> {
        let capabilities = capture.prepare_protocol(lab)?;
        let operation = capture.query_operation_status(std::time::Duration::from_secs(10))?;
        let stack = capture.query_stack_usage(std::time::Duration::from_secs(10))?;
        Ok(serde_json::json!({
            "protocol_version": open_esp_radio_hil_protocol::PROTOCOL_VERSION,
            "capabilities": capabilities,
            "operation": operation,
            "stack": stack,
            "uart_log": output.join("uart.log"),
        }))
    })();
    let capture_result = capture.finish_to(output);
    let report = result?;
    capture_result?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn boot_smoke(output: &Path, lab: &lab_config::LabConfig) -> Result<()> {
    let capture = traffic_capture::SerialCapture::start_with_reset(&lab.device.serial);
    let result = capture.wait_for_boot_smoke(std::time::Duration::from_secs(10));
    capture.finish_to(output)?;
    result
}

fn run_all(
    root: &Path,
    lab: &lab_config::LabConfig,
    catalog: &scenario::Catalog,
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
    for class in scenario::ImageClass::ALL {
        let class_scenarios = selected
            .iter()
            .copied()
            .filter(|entry| entry.image == class)
            .collect::<Vec<_>>();
        if class_scenarios.is_empty() {
            continue;
        }
        let artifacts = build(root, class)?;
        flash(root, &artifacts, &lab.device.serial)?;
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
    lab: &lab_config::LabConfig,
    selected: &scenario::Scenario,
) -> Result<()> {
    let output = root.join("target/hil/esp32s31/runs").join(&selected.id);
    fs::create_dir_all(&output)?;
    fs::write(
        output.join("resolved-scenario.json"),
        serde_json::to_vec_pretty(selected)?,
    )?;
    let result = execute_workload(root, lab, selected, &output);
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
    println!("{}", serde_json::to_string(&result_document)?);
    Ok(())
}

fn execute_workload(
    root: &Path,
    lab: &lab_config::LabConfig,
    selected: &scenario::Scenario,
    output: &Path,
) -> Result<()> {
    use scenario::{Direction, Workload};

    match &selected.workload {
        Workload::BootSmoke => boot_smoke(output, lab),
        Workload::Udp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            payload_bytes,
            phy,
        } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--seconds", duration_seconds);
            push_option(&mut arguments, "--payload", payload_bytes);
            push_option(&mut arguments, "--phy", phy.id());
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
                    rx_traffic::run(
                        arguments,
                        output,
                        lab,
                        selected.criteria.require_no_beacon_loss,
                    )
                }
                Direction::Tx => {
                    if let Some(rate) = tx_rate_bps {
                        push_option(&mut arguments, "--rate", rate);
                    }
                    if let Some(floor) = selected.criteria.minimum_tx_bps {
                        push_option(&mut arguments, "--floor", floor);
                    }
                    tx_traffic::run(
                        arguments,
                        output,
                        lab,
                        selected.criteria.require_no_beacon_loss,
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
                    bidirectional::run(
                        arguments,
                        output,
                        lab,
                        selected.criteria.require_no_beacon_loss,
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
                Direction::Rx => tcp_traffic::run_rx(
                    arguments,
                    output,
                    lab,
                    selected.criteria.require_no_beacon_loss,
                ),
                Direction::Tx => tcp_traffic::run_tx(
                    arguments,
                    output,
                    lab,
                    selected.criteria.require_no_beacon_loss,
                ),
                Direction::Bidirectional => tcp_traffic::run_bidirectional(
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
            icmp_latency::run(
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
            station_lifecycle::run(
                arguments,
                output,
                lab,
                selected.criteria.require_no_beacon_loss,
            )
        }
        Workload::StationApLoss { timeout_seconds } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            station_ap_loss::run(arguments, output, lab)
        }
        Workload::StationApAbsence { timeout_seconds } => {
            let mut arguments = Vec::new();
            push_option(&mut arguments, "--timeout-seconds", timeout_seconds);
            station_ap_absence::run(arguments, output, lab)
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
            wifi_control::run(operation.id(), arguments, output, lab)
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
            wifi_capture::run(arguments, output, lab)
        }
        Workload::AccessPoint {
            cycles,
            boots,
            timeout_seconds,
            traffic,
        } => access_point_qualification::run(
            access_point_qualification::Config {
                cycles: *cycles,
                boots: *boots,
                timeout: std::time::Duration::from_secs(u64::from(*timeout_seconds)),
                traffic: traffic.clone(),
                criteria: selected.criteria.clone(),
            },
            output,
            lab,
        ),
    }
}

fn push_option(arguments: &mut Vec<String>, name: &str, value: impl ToString) {
    arguments.push(name.to_owned());
    arguments.push(value.to_string());
}

struct Artifacts {
    output: PathBuf,
    runtime_elf: PathBuf,
    runtime_bin: PathBuf,
    bootstrap_elf: PathBuf,
    application_image: PathBuf,
}

fn build(root: &Path, class: scenario::ImageClass) -> Result<Artifacts> {
    let local_esp_hal = local_esp_hal_override()?;
    if local_esp_hal.is_none() {
        return build_resolved(root, class, None);
    }

    let lockfile = root.join("hil/targets/esp32s31/Cargo.lock");
    let mut snapshot = TrackedFileSnapshot::capture(lockfile)?;
    let result = build_resolved(root, class, local_esp_hal.as_deref());
    let restore = snapshot.restore();
    match (result, restore) {
        (Ok(artifacts), Ok(())) => Ok(artifacts),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(format!("restore embedded Cargo.lock: {error}").into()),
        (Err(build_error), Err(restore_error)) => Err(format!(
            "{build_error}; additionally failed to restore embedded Cargo.lock: {restore_error}"
        )
        .into()),
    }
}

fn build_resolved(
    root: &Path,
    class: scenario::ImageClass,
    local_esp_hal: Option<&Path>,
) -> Result<Artifacts> {
    ensure_no_old_application_dependency(root)?;
    let manifest = root.join("hil/targets/esp32s31/Cargo.toml");
    let output =
        root.join("target/hil/esp32s31")
            .join(format!("{}-{}", QUALIFIED_PROFILE, class.id()));
    let runtime_target = output.join("cargo/runtime");
    let bootstrap_target = output.join("cargo/bootstrap");
    fs::create_dir_all(&output)?;
    fs::write(output.join("image-class.txt"), format!("{}\n", class.id()))?;

    let runtime_elf = runtime_target
        .join(TARGET)
        .join("release")
        .join(RUNTIME_BIN);
    let runtime_bin = output.join("runtime.bin");
    let bootstrap_elf = bootstrap_target
        .join(TARGET)
        .join("release")
        .join(BOOTSTRAP_BIN);
    let application_image = output.join("application.bin");

    let runtime_features = class.runtime_features();
    let stack_policy_path = root.join("hil/targets/esp32s31/stack.toml");
    let stack_budget = open_esp_radio_memory_report::StackBudget::load(&stack_policy_path)?;
    let mut runtime = cargo_command();
    runtime
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["-p", RUNTIME_BIN, "--release", "--target", TARGET])
        .args(["--no-default-features", "--features", runtime_features])
        .env("CARGO_TARGET_DIR", &runtime_target);
    if local_esp_hal.is_none() {
        runtime.arg("--locked");
    }
    add_local_esp_hal_patches(&mut runtime, local_esp_hal);
    stack_audit::enable_stack_checks(&mut runtime, &stack_budget);
    run_command(&mut runtime, "build stage-two runtime")?;
    require_file(&runtime_elf, "runtime ELF")?;

    let stack_report = stack_audit::analyze_elf_stack(&runtime_elf, &stack_budget)?;
    let stack_report_path = output.join("runtime-stack.txt");
    fs::write(
        &stack_report_path,
        open_esp_radio_memory_report::render_stack_report(&stack_report),
    )?;
    eprintln!("stack_report={}", stack_report_path.display());
    open_esp_radio_memory_report::audit_stack(&stack_report)?;

    let mut objcopy = Command::new(program_from_env("LLVM_OBJCOPY", "llvm-objcopy"));
    objcopy
        .args(["-O", "binary"])
        .arg(&runtime_elf)
        .arg(&runtime_bin);
    run_command(&mut objcopy, "flatten stage-two runtime")?;
    let crc = pack_runtime(&runtime_bin)?;
    let placement = audit_runtime(&runtime_elf, &runtime_bin)?;
    fs::write(output.join("placement.txt"), placement)?;

    let mut bootstrap = cargo_command();
    bootstrap
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["-p", BOOTSTRAP_BIN, "--release", "--target", TARGET])
        .env("CARGO_TARGET_DIR", &bootstrap_target)
        .env("PSRAM_RUNTIME_BIN", absolute(&runtime_bin)?);
    if local_esp_hal.is_none() {
        bootstrap.arg("--locked");
    }
    add_local_esp_hal_patches(&mut bootstrap, local_esp_hal);
    stack_audit::enable_stack_checks(&mut bootstrap, &stack_budget);
    run_command(&mut bootstrap, "build Flash/SRAM bootstrap")?;
    require_file(&bootstrap_elf, "bootstrap ELF")?;
    let bootstrap_stack_report = stack_audit::analyze_elf_stack(&bootstrap_elf, &stack_budget)?;
    let bootstrap_stack_report_path = output.join("bootstrap-stack.txt");
    fs::write(
        &bootstrap_stack_report_path,
        open_esp_radio_memory_report::render_stack_report(&bootstrap_stack_report),
    )?;
    eprintln!(
        "bootstrap_stack_report={}",
        bootstrap_stack_report_path.display()
    );
    open_esp_radio_memory_report::audit_stack(&bootstrap_stack_report)?;

    let partition_table = root.join("hil/targets/esp32s31/partitions/hil.csv");
    let mut save_image = Command::new(program_from_env("ESPFLASH", "espflash"));
    save_image
        .args([
            "save-image",
            "--chip",
            "esp32s31",
            "--flash-mode",
            "qio",
            "--flash-freq",
            "80mhz",
            "--flash-size",
            "16mb",
            "--mmu-page-size",
            "65536",
            "--partition-table",
        ])
        .arg(&partition_table)
        .args(["--target-app-partition", "ota_0"])
        .arg(&bootstrap_elf)
        .arg(&application_image);
    run_command(&mut save_image, "encode ESP application image")?;
    audit_application_image(&application_image)?;

    eprintln!("runtime_crc32={crc:08x}");
    eprintln!("placement_audit=PASS");
    eprintln!("stack_frame_audit=PASS");
    eprintln!("autonomous_source_graph=PASS");
    Ok(Artifacts {
        output,
        runtime_elf,
        runtime_bin,
        bootstrap_elf,
        application_image,
    })
}

/// Byte-exact restoration guard for a caller-owned tracked file.
///
/// An explicitly requested local Cargo override legitimately resolves the
/// embedded workspace against path packages and therefore rewrites package
/// `source` fields in its lock file. That resolution is a build fixture, not a
/// repository mutation. The explicit `restore` reports failures; `Drop` is
/// the fallback for every early return and panic path.
struct TrackedFileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
    restored: bool,
}

impl TrackedFileSnapshot {
    fn capture(path: PathBuf) -> std::io::Result<Self> {
        let contents = match fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(Self {
            path,
            contents,
            restored: false,
        })
    }

    fn restore(&mut self) -> std::io::Result<()> {
        if self.restored {
            return Ok(());
        }
        match &self.contents {
            Some(contents) => {
                let unchanged = fs::read(&self.path)
                    .is_ok_and(|current| current.as_slice() == contents.as_slice());
                if !unchanged {
                    fs::write(&self.path, contents)?;
                }
            }
            None => match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            },
        }
        self.restored = true;
        Ok(())
    }
}

impl Drop for TrackedFileSnapshot {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn flash(root: &Path, artifacts: &Artifacts, port: &Path) -> Result<()> {
    let partition_csv = root.join("hil/targets/esp32s31/partitions/hil.csv");
    let partition_bin = artifacts.output.join("partitions.bin");
    let selector_bin = artifacts.output.join("otadata-ota0-valid.bin");

    let mut partition = Command::new(program_from_env("ESPFLASH", "espflash"));
    partition
        .args(["partition-table", "--to-binary", "--output"])
        .arg(&partition_bin)
        .arg(&partition_csv);
    run_command(&mut partition, "encode HIL partition table")?;
    fs::write(&selector_bin, ota0_selector_image())?;

    write_flash_binary(
        port,
        PARTITION_TABLE_OFFSET,
        &partition_bin,
        "no-reset",
        "write HIL partition table",
    )?;
    write_flash_binary(
        port,
        OTA_0_OFFSET,
        &artifacts.application_image,
        "no-reset",
        "write HIL application",
    )?;
    write_flash_binary(
        port,
        OTA_SELECTOR_OFFSET,
        &selector_bin,
        "hard-reset",
        "select HIL ota_0 image",
    )
}

fn write_flash_binary(
    port: &Path,
    address: u32,
    image: &Path,
    after: &str,
    description: &str,
) -> Result<()> {
    let mut command = Command::new(program_from_env("ESPFLASH", "espflash"));
    command
        .args([
            "write-bin",
            "--chip",
            "esp32s31",
            "--non-interactive",
            "--port",
        ])
        .arg(port)
        .args(["--after", after])
        .arg(format!("{address:#x}"))
        .arg(image);
    run_command(&mut command, description)
}

fn ota0_selector_image() -> [u8; OTA_DATA_SIZE] {
    let sequence = 1_u32;
    let mut image = [0xff; OTA_DATA_SIZE];
    image[0..4].copy_from_slice(&sequence.to_le_bytes());
    image[24..28].copy_from_slice(&2_u32.to_le_bytes());
    image[28..32].copy_from_slice(&crc32_idf(&sequence.to_le_bytes()).to_le_bytes());
    image
}

fn crc32_idf(bytes: &[u8]) -> u32 {
    let mut crc = 0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    crc ^ u32::MAX
}

fn cargo_command() -> Command {
    Command::new(program_from_env("CARGO", "cargo"))
}

fn program_from_env(variable: &str, fallback: &str) -> OsString {
    env::var_os(variable).unwrap_or_else(|| fallback.into())
}

fn local_esp_hal_override() -> Result<Option<PathBuf>> {
    let Some(local) = env::var_os("ESP_HAL_ROOT").map(PathBuf::from) else {
        return Ok(None);
    };
    let packages = [
        ("esp-bootloader-esp-idf", "esp-bootloader-esp-idf"),
        ("esp-hal", "esp-hal"),
        ("esp-sync", "esp-sync"),
    ];
    let missing = packages
        .iter()
        .filter_map(|(_, path)| (!local.join(path).is_dir()).then_some(*path))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "ESP_HAL_ROOT={} is missing required package directories: {}",
            local.display(),
            missing.join(", ")
        )
        .into());
    }
    Ok(Some(local))
}

fn add_local_esp_hal_patches(command: &mut Command, local: Option<&Path>) {
    let Some(local) = local else {
        return;
    };
    let packages = [
        ("esp-bootloader-esp-idf", "esp-bootloader-esp-idf"),
        ("esp-hal", "esp-hal"),
        ("esp-sync", "esp-sync"),
    ];
    for (package, path) in packages {
        command.arg("--config").arg(format!(
            "patch.\"https://github.com/ermacv/esp-hal\".{package}.path=\"{}\"",
            local.join(path).display()
        ));
    }
}

fn run_command(command: &mut Command, description: &str) -> Result<()> {
    eprintln!("==> {description}");
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{description} failed with {status}").into());
    }
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    })
}

fn require_file(path: &Path, description: &str) -> Result<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("missing {description}: {}", path.display()).into())
    }
}

fn require_program(program: &str) -> Result<()> {
    let status = Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("required program `{program}` is unavailable").into())
    }
}

fn ensure_no_old_application_dependency(root: &Path) -> Result<()> {
    for relative in [
        "hil/targets/esp32s31/Cargo.toml",
        "hil/targets/esp32s31/bootstrap/Cargo.toml",
        "hil/targets/esp32s31/runtime/Cargo.toml",
        "hil/targets/esp32s31/board/Cargo.toml",
    ] {
        let path = root.join(relative);
        let contents = fs::read_to_string(&path)?;
        if contents.contains("esp32s31_rust") {
            return Err(format!("{} still depends on esp32s31_rust", path.display()).into());
        }
    }
    Ok(())
}

fn ensure_vendor_oracle_isolated(root: &Path) -> Result<()> {
    for relative in [
        "Cargo.lock",
        "hil/targets/esp32s31/Cargo.toml",
        "hil/targets/esp32s31/Cargo.lock",
    ] {
        let path = root.join(relative);
        let contents = fs::read_to_string(&path)?;
        for forbidden in [
            "name = \"esp-phy\"",
            "name = \"esp-rtos\"",
            "name = \"esp-wifi-sys-esp32s31\"",
        ] {
            if contents.contains(forbidden) {
                return Err(format!(
                    "{} pulls `{forbidden}` into the source-only graph",
                    path.display()
                )
                .into());
            }
        }
    }
    require_file(
        &root.join("verification/vendor/targets/esp32s31/oracle-firmware/Cargo.toml"),
        "isolated vendor-oracle workspace",
    )
}

fn pack_runtime(path: &Path) -> Result<u32> {
    let mut bytes = fs::read(path)?;
    if bytes.len() < RUNTIME_HEADER_BYTES {
        return Err("runtime image is shorter than its header".into());
    }
    if u32::from_le_bytes(bytes[0..4].try_into()?) != RUNTIME_MAGIC {
        return Err("runtime image has the wrong stage-two magic".into());
    }
    if u32::from_le_bytes(bytes[28..32].try_into()?) as usize != RUNTIME_HEADER_BYTES {
        return Err("runtime image has an incompatible header size".into());
    }
    bytes[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
    let crc = crc32(&bytes);
    bytes[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    fs::write(path, bytes)?;
    let packed = fs::read(path)?;
    let stored = u32::from_le_bytes(packed[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].try_into()?);
    let mut verified = packed;
    verified[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
    if stored != crc || crc32(&verified) != crc {
        return Err("runtime CRC did not survive packing".into());
    }
    Ok(crc)
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn audit_runtime(elf: &Path, binary: &Path) -> Result<String> {
    let output = Command::new(program_from_env("LLVM_NM", "llvm-nm"))
        .args(["--defined-only", "--numeric-sort"])
        .arg(elf)
        .output()?;
    if !output.status.success() {
        return Err("llvm-nm failed while auditing runtime placement".into());
    }
    let text = String::from_utf8(output.stdout)?;
    let mut symbols = BTreeMap::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(address) = words.next() else {
            continue;
        };
        let Some(_kind) = words.next() else { continue };
        let Some(name) = words.next() else { continue };
        if let Ok(address) = u64::from_str_radix(address, 16) {
            symbols.insert(name.to_owned(), address);
        }
    }
    let symbol = |name: &str| -> Result<u64> {
        symbols
            .get(name)
            .copied()
            .ok_or_else(|| format!("runtime ELF lacks `{name}`").into())
    };

    let image_start = symbol("__runtime_image_start")?;
    let payload_end = symbol("__runtime_payload_end")?;
    let text_start = symbol("__runtime_text_start")?;
    let text_end = symbol("__runtime_text_end")?;
    let entry = symbol("_runtime_start")?;
    let data_start = symbol("__runtime_data_start")?;
    let bss_end = symbol("__runtime_data_bss_end")?;
    let isr_start = symbol("__runtime_isr_start")?;
    let isr_end = symbol("__runtime_isr_end")?;
    let critical_start = symbol("__runtime_critical_data_start")?;
    let critical_bss_end = symbol("__runtime_critical_bss_end")?;
    let dma_start = symbol("__runtime_dma_data_start")?;
    let dma_end = symbol("__runtime_dma_bss_end")?;
    let stack_bottom = symbol("_stack_end")?;
    let stack_top = symbol("_stack_start")?;
    let binary_bytes = fs::metadata(binary)?.len();

    let in_sram = |start: u64, end: u64| start >= 0x2f00_0000 && end >= start && end <= 0x2f07_afc0;
    if image_start != 0x5001_0000
        || payload_end <= image_start
        || payload_end - image_start != binary_bytes
        || entry < text_start
        || entry >= text_end
        || data_start < 0x5000_0000
        || bss_end > 0x5100_0000
        || !in_sram(isr_start, isr_end)
        || !in_sram(critical_start, critical_bss_end)
        || !in_sram(dma_start, dma_end)
        || stack_top != 0x2f07_afc0
        || stack_top.saturating_sub(stack_bottom) < 0x1_0000
    {
        return Err("runtime ELF violates the PSRAM/PSRAM placement contract".into());
    }

    Ok(format!(
        "profile={QUALIFIED_PROFILE}\n\
         image={image_start:#010x}..{payload_end:#010x}\n\
         text={text_start:#010x}..{text_end:#010x}\n\
         data_start={data_start:#010x}\n\
         bss_end={bss_end:#010x}\n\
         isr={isr_start:#010x}..{isr_end:#010x}\n\
         critical={critical_start:#010x}..{critical_bss_end:#010x}\n\
         dma={dma_start:#010x}..{dma_end:#010x}\n\
         stack={stack_bottom:#010x}..{stack_top:#010x}\n\
         result=PASS\n"
    ))
}

fn audit_application_image(path: &Path) -> Result<()> {
    const APP_DESC_OFFSET: usize = 0x20;
    const APP_DESC_MMU_PAGE_LOG2_OFFSET: usize = 180;
    let bytes = fs::read(path)?;
    let end = APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET + 1;
    if bytes.len() < end
        || bytes[APP_DESC_OFFSET..APP_DESC_OFFSET + 4] != 0xabcd_5432_u32.to_le_bytes()
        || bytes[APP_DESC_OFFSET + APP_DESC_MMU_PAGE_LOG2_OFFSET] != 16
    {
        return Err("ESP application image has an invalid app descriptor or MMU page size".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch_directory(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = env::temp_dir().join(format!(
            "open-esp-radio-hil-runner-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn qualified_profile_name_is_stable() {
        assert_eq!(QUALIFIED_PROFILE, "psram-code-psram-data");
        assert_eq!(TARGET, "riscv32imafc-unknown-none-elf");
    }

    #[test]
    fn repository_layout_contains_the_embedded_workspace() {
        let root = repository_root().unwrap();
        assert!(root.join("hil/targets/esp32s31/Cargo.toml").is_file());
        ensure_vendor_oracle_isolated(&root).unwrap();
    }

    #[test]
    fn image_classes_are_stable_and_do_not_use_workload_environment() {
        assert_eq!(scenario::ImageClass::ALL.len(), 4);
        assert_eq!(scenario::ImageClass::Qualification.id(), "qualification");
        assert_eq!(
            scenario::ImageClass::DiagnosticRxDelivery.runtime_features(),
            "open-radio-hil,rx-delivery-telemetry,code-psram,profile-psram-data"
        );
    }

    #[test]
    fn runtime_crc_treats_the_checksum_field_as_zero() {
        let mut image = vec![0x5a; 128];
        image[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
        let expected = crc32(&image);
        image[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].copy_from_slice(&expected.to_le_bytes());
        image[RUNTIME_CRC_OFFSET..RUNTIME_CRC_OFFSET + 4].fill(0);
        assert_eq!(crc32(&image), expected);
    }

    #[test]
    fn ota0_selector_uses_valid_idf_entry() {
        let image = ota0_selector_image();
        assert_eq!(u32::from_le_bytes(image[0..4].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(image[24..28].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(image[28..32].try_into().unwrap()),
            crc32_idf(&1_u32.to_le_bytes())
        );
        assert!(image[32..].iter().all(|byte| *byte == 0xff));
    }

    #[test]
    fn tracked_file_snapshot_restores_exact_contents() {
        let directory = scratch_directory("restore");
        let lockfile = directory.join("Cargo.lock");
        let original = b"version = 4\n\n[[package]]\nname = \"fixture\"\n";
        fs::write(&lockfile, original).unwrap();

        let mut snapshot = TrackedFileSnapshot::capture(lockfile.clone()).unwrap();
        fs::write(&lockfile, b"rewritten by cargo\n").unwrap();
        snapshot.restore().unwrap();

        assert_eq!(fs::read(&lockfile).unwrap(), original);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn tracked_file_snapshot_drop_removes_new_file() {
        let directory = scratch_directory("drop");
        let lockfile = directory.join("Cargo.lock");
        {
            let _snapshot = TrackedFileSnapshot::capture(lockfile.clone()).unwrap();
            fs::write(&lockfile, b"generated by cargo\n").unwrap();
        }

        assert!(!lockfile.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
