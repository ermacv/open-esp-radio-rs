use std::{
    collections::BTreeMap,
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

mod bidirectional;
mod controlled_ap;
mod icmp_latency;
mod paced_tcp;
mod paced_udp;
mod packet_socket;
mod rx_traffic;
mod startup_artifact;
mod station_ap_absence;
mod station_ap_loss;
mod station_lifecycle;
mod station_rx_fault;
mod station_tx_fault;
mod tcp_traffic;
mod traffic_capture;
mod trigger;
mod tx_traffic;
mod udp_socket;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const QUALIFIED_PROFILE: &str = "psram-code-psram-data";
const TARGET: &str = "riscv32imafc-unknown-none-elf";
const RUNTIME_BIN: &str = "open-esp-radio-hil-esp32s31-runtime";
const BOOTSTRAP_BIN: &str = "open-esp-radio-hil-esp32s31-bootstrap";
const ORACLE_BIN: &str = "open-esp-radio-vendor-oracle-hil-esp32s31";
const RUNTIME_MAGIC: u32 = 0x3247_5453;
const RUNTIME_CRC_OFFSET: usize = 40;
const RUNTIME_HEADER_BYTES: usize = 44;
const PARTITION_TABLE_OFFSET: u32 = 0x8000;
const OTA_SELECTOR_OFFSET: u32 = 0xd000;
const OTA_0_OFFSET: u32 = 0x1_0000;
const OTA_DATA_SIZE: usize = 0x2000;

const SCENARIO_ENVIRONMENT: &[&str] = &[
    "OPEN_RADIO_BIDIRECTIONAL_BENCH",
    "OPEN_RADIO_RAW_MAC_BENCH",
    "OPEN_RADIO_TX_BENCH",
    "OPEN_RADIO_PERF_AP",
    "OPEN_RADIO_TCP_BENCH",
    "OPEN_RADIO_AMSDU_BENCH",
    "OPEN_RADIO_NETWORK_AMSDU_BENCH",
    "OPEN_RADIO_HE_MATRIX_HIL",
    "OPEN_RADIO_HE_LDPC_HIL",
    "OPEN_RADIO_HE_DCM_HIL",
    "OPEN_RADIO_HE_TB_HIL",
    "OPEN_RADIO_HE_DELIMITER_HIL",
    "OPEN_RADIO_HT_SGI",
];

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let root = repository_root()?;
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("profiles") if arguments.next().is_none() => {
            println!("{QUALIFIED_PROFILE}");
            Ok(())
        }
        Some("scenarios") if arguments.next().is_none() => {
            print_scenarios();
            Ok(())
        }
        Some("doctor") if arguments.next().is_none() => doctor(&root),
        Some("build") => {
            let scenario = arguments
                .next()
                .map(|value| Scenario::parse(&value))
                .transpose()?
                .unwrap_or(Scenario::BootSmoke);
            if arguments.next().is_some() {
                return Err("usage: cargo hil build [scenario]".into());
            }
            let artifacts = build(&root, scenario)?;
            println!("profile={QUALIFIED_PROFILE}");
            println!("scenario={}", scenario.name());
            println!("runtime_elf={}", artifacts.runtime_elf.display());
            println!("runtime_bin={}", artifacts.runtime_bin.display());
            println!("bootstrap_elf={}", artifacts.bootstrap_elf.display());
            println!(
                "application_image={}",
                artifacts.application_image.display()
            );
            Ok(())
        }
        Some("flash") => {
            let (scenario, port) = parse_flash_arguments(arguments)?;
            let artifacts = build(&root, scenario)?;
            flash(&root, &artifacts, &port)?;
            println!("profile={QUALIFIED_PROFILE}");
            println!("scenario={}", scenario.name());
            println!("port={}", port.display());
            println!("flash=PASS");
            Ok(())
        }
        Some("traffic") => match arguments.next().as_deref() {
            Some("bidirectional") => bidirectional::run(arguments.collect(), &root),
            Some("icmp") => icmp_latency::run(arguments.collect(), &root),
            Some("rx") => rx_traffic::run(arguments.collect(), &root),
            Some("tcp-rx") => tcp_traffic::run_rx(arguments.collect(), &root),
            Some("tcp-tx") => tcp_traffic::run_tx(arguments.collect(), &root),
            Some("tcp-bidirectional") => {
                tcp_traffic::run_bidirectional(arguments.collect(), &root)
            }
            Some("tx") => tx_traffic::run(arguments.collect(), &root),
            Some("trigger") => trigger::run(&arguments.collect::<Vec<_>>()),
            Some("trigger-hil") => trigger::run_hil(&arguments.collect::<Vec<_>>(), &root),
            _ => Err(
                "usage: cargo hil traffic <rx|tx|bidirectional|tcp-rx|tcp-tx|tcp-bidirectional|icmp|trigger|trigger-hil> ..."
                    .into(),
            ),
        },
        Some("station") => match arguments.next().as_deref() {
            Some("ap-absence") => station_ap_absence::run(arguments.collect(), &root),
            Some("ap-loss") => station_ap_loss::run(arguments.collect(), &root),
            Some("reconnect") => station_lifecycle::run(arguments.collect(), &root),
            Some("rx-fault") => station_rx_fault::run(arguments.collect(), &root),
            Some("tx-fault") => station_tx_fault::run(arguments.collect(), &root),
            _ => Err("usage: cargo hil station \
                 <reconnect|ap-loss|ap-absence|rx-fault|tx-fault> [options]"
                .into()),
        },
        Some("oracle") => oracle_command(&root, arguments.collect()),
        Some("help" | "--help" | "-h") | None => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown HIL command `{command}`").into()),
    }
}

fn repository_root() -> Result<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find(|path| path.join(".git").is_dir() && path.join("Cargo.toml").is_file())
        .map(PathBuf::from)
        .ok_or_else(|| "HIL runner must live inside the repository".into())
}

fn doctor(root: &std::path::Path) -> Result<()> {
    let firmware = root.join("hil/targets/esp32s31/Cargo.toml");
    if !firmware.is_file() {
        return Err(format!("missing embedded HIL workspace: {}", firmware.display()).into());
    }
    println!("repository={}", root.display());
    println!("firmware_workspace={}", firmware.display());
    println!("target={TARGET}");
    println!("qualified_profile={QUALIFIED_PROFILE}");
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

fn print_help() {
    println!(
        "Open ESP radio HIL\n\n\
         cargo hil profiles\n\
         cargo hil scenarios\n\
         cargo hil doctor\n\
         cargo hil build [scenario]\n\n\
         cargo hil flash [scenario] [--port /dev/ttyACM0]\n\
         cargo hil traffic rx <device-ipv4> [options]\n\
         cargo hil traffic tx <device-ipv4> [options]\n\
         cargo hil traffic bidirectional <ipv4> [options]\n\
         cargo hil traffic tcp-rx <device-ipv4> [options]\n\
         cargo hil traffic tcp-tx <device-ipv4> [options]\n\
         cargo hil traffic tcp-bidirectional <device-ipv4> [options]\n\
         cargo hil traffic icmp <device-ipv4> [options]\n\
         cargo hil traffic trigger <monitor-interface> [options]\n\
         cargo hil traffic trigger-hil <monitor-interface> [options]\n\
         cargo hil station ap-absence [options]\n\
         cargo hil station ap-loss [options]\n\
         cargo hil station reconnect [options]\n\
         cargo hil station rx-fault [options]\n\
         cargo hil station tx-fault [options]\n\
         cargo hil oracle build\n\
         cargo hil oracle flash [--port /dev/ttyACM0]\n\n\
         The build command compiles and packs both HIL stages, audits the \
         PSRAM/SRAM placement contract, and emits an ESP application image.\n\
         Traffic commands provision Wi-Fi at runtime from \
         OPEN_RADIO_HIL_STA_SSID and OPEN_RADIO_HIL_STA_PASSWORD. Set \
         OPEN_RADIO_HIL_STA_IPV4_CIDR and, optionally, \
         OPEN_RADIO_HIL_STA_GATEWAY_IPV4 for an isolated static-IP cell; \
         otherwise the target uses DHCP.\n\
         Run `cargo hil scenarios` for the firmware scenario list."
    );
}

fn print_scenarios() {
    for scenario in Scenario::ALL {
        println!("{:<22} {}", scenario.argument(), scenario.description());
    }
}

fn scenario_manifest(scenario: Scenario) -> String {
    let mut manifest = format!(
        "profile={QUALIFIED_PROFILE}\nscenario={}\nartifact={}\nruntime_feature={}\n",
        scenario.argument(),
        scenario.name(),
        scenario.runtime_feature(),
    );
    for (variable, value) in scenario.environment() {
        manifest.push_str(variable);
        manifest.push('=');
        manifest.push_str(value);
        manifest.push('\n');
    }
    manifest
}

struct OracleArtifacts {
    output: PathBuf,
    elf: PathBuf,
    application_image: PathBuf,
}

fn oracle_command(root: &Path, arguments: Vec<String>) -> Result<()> {
    let mut arguments = arguments.into_iter();
    match arguments.next().as_deref() {
        Some("build") if arguments.next().is_none() => {
            let artifacts = build_oracle(root)?;
            println!("oracle_elf={}", artifacts.elf.display());
            println!(
                "application_image={}",
                artifacts.application_image.display()
            );
            println!("oracle_build=PASS");
            Ok(())
        }
        Some("flash") => {
            let port = parse_oracle_flash_arguments(arguments)?;
            let artifacts = build_oracle(root)?;
            flash_oracle(root, &artifacts, &port)?;
            println!("oracle_elf={}", artifacts.elf.display());
            println!(
                "application_image={}",
                artifacts.application_image.display()
            );
            println!("port={}", port.display());
            println!("oracle_flash=PASS");
            Ok(())
        }
        _ => Err("usage: cargo hil oracle <build|flash [--port /dev/ttyACM0]>".into()),
    }
}

fn build_oracle(root: &Path) -> Result<OracleArtifacts> {
    eprintln!("oracle_input_authentication=CALLER_OWNED");
    let manifest = root.join("verification/vendor/targets/esp32s31/oracle-firmware/Cargo.toml");
    let output = root.join("target/hil/vendor-oracle/esp32s31");
    let cargo_target = output.join("cargo");
    let elf = cargo_target.join(TARGET).join("release").join(ORACLE_BIN);
    let application_image = output.join("application.bin");
    fs::create_dir_all(&output)?;

    let libgcc = find_libgcc()?;
    let libgcc_dir = libgcc
        .parent()
        .ok_or_else(|| format!("libgcc path has no parent: {}", libgcc.display()))?;
    let mut cargo = cargo_command();
    cargo
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .args(["--release", "--target", TARGET, "--locked"])
        .env("CARGO_TARGET_DIR", &cargo_target)
        .env("ESP32S31_LIBGCC_DIR", libgcc_dir);
    run_command(&mut cargo, "build isolated vendor PHY oracle")?;
    require_file(&elf, "vendor-oracle ELF")?;

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
        .arg(&elf)
        .arg(&application_image);
    run_command(&mut save_image, "encode vendor-oracle application image")?;
    audit_application_image(&application_image)?;

    Ok(OracleArtifacts {
        output,
        elf,
        application_image,
    })
}

fn find_libgcc() -> Result<PathBuf> {
    if let Some(directory) = env::var_os("ESP32S31_LIBGCC_DIR") {
        let path = PathBuf::from(directory).join("libgcc.a");
        require_file(&path, "Espressif libgcc")?;
        return Ok(path);
    }

    let home = env::var_os("HOME").ok_or("HOME is not set; cannot locate Espressif GCC")?;
    let root = PathBuf::from(home).join(".espressif/tools/riscv32-esp-elf");
    let mut candidates = Vec::new();
    collect_named_files(&root, "riscv32-esp-elf-gcc", &mut candidates)?;
    candidates.sort();
    let gcc = candidates
        .pop()
        .ok_or_else(|| format!("riscv32-esp-elf-gcc was not found below {}", root.display()))?;
    let output = Command::new(gcc)
        .args([
            "-march=rv32imafc",
            "-mabi=ilp32f",
            "-print-libgcc-file-name",
        ])
        .output()?;
    if !output.status.success() {
        return Err("Espressif GCC failed to locate libgcc".into());
    }
    let path = PathBuf::from(String::from_utf8(output.stdout)?.trim());
    require_file(&path, "Espressif libgcc")?;
    Ok(path)
}

fn collect_named_files(root: &Path, name: &str, output: &mut Vec<PathBuf>) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_named_files(&path, name, output)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            output.push(path);
        }
    }
    Ok(())
}

struct Artifacts {
    output: PathBuf,
    runtime_elf: PathBuf,
    runtime_bin: PathBuf,
    bootstrap_elf: PathBuf,
    application_image: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    BootSmoke,
    Radio,
    RadioPollProfile,
    RadioRxOrderProfile,
    UdpTx,
    StationTxFault,
    Bidirectional,
    Tcp,
}

impl Scenario {
    const ALL: [Self; 8] = [
        Self::BootSmoke,
        Self::Radio,
        Self::RadioPollProfile,
        Self::RadioRxOrderProfile,
        Self::UdpTx,
        Self::StationTxFault,
        Self::Bidirectional,
        Self::Tcp,
    ];

    fn parse(value: &str) -> Result<Self> {
        match value {
            "boot-smoke" => Ok(Self::BootSmoke),
            "radio" | "open-radio-hil" => Ok(Self::Radio),
            "radio-poll-profile" | "open-radio-poll-profile" => Ok(Self::RadioPollProfile),
            "radio-rx-order-profile" | "open-radio-rx-order-profile" => {
                Ok(Self::RadioRxOrderProfile)
            }
            "udp-tx" | "open-radio-udp-tx" => Ok(Self::UdpTx),
            "station-tx-fault" | "open-radio-station-tx-fault" => Ok(Self::StationTxFault),
            "bidirectional" | "open-radio-bidirectional" => Ok(Self::Bidirectional),
            "tcp" | "tcp-rx" | "open-radio-tcp" | "open-radio-tcp-rx" => Ok(Self::Tcp),
            _ => Err(format!(
                "unsupported production-runner HIL scenario `{value}`; run `cargo hil scenarios` for the list"
            )
            .into()),
        }
    }

    const fn argument(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke",
            Self::Radio => "radio",
            Self::RadioPollProfile => "radio-poll-profile",
            Self::RadioRxOrderProfile => "radio-rx-order-profile",
            Self::UdpTx => "udp-tx",
            Self::StationTxFault => "station-tx-fault",
            Self::Bidirectional => "bidirectional",
            Self::Tcp => "tcp",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke",
            Self::Radio => "open-radio-hil",
            Self::RadioPollProfile => "open-radio-poll-profile",
            Self::RadioRxOrderProfile => "open-radio-rx-order-profile",
            Self::UdpTx => "open-radio-udp-tx",
            Self::StationTxFault => "open-radio-station-tx-fault",
            Self::Bidirectional => "open-radio-bidirectional",
            Self::Tcp => "open-radio-tcp",
        }
    }

    const fn runtime_feature(self) -> &'static str {
        match self {
            Self::BootSmoke => "boot-smoke",
            Self::Radio => "open-radio-hil",
            Self::RadioPollProfile => "open-radio-hil,task-poll-telemetry",
            Self::RadioRxOrderProfile => "open-radio-hil,rx-order-telemetry",
            Self::UdpTx | Self::StationTxFault | Self::Bidirectional | Self::Tcp => {
                "open-radio-hil"
            }
        }
    }

    const fn environment(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::BootSmoke | Self::Radio | Self::RadioPollProfile | Self::RadioRxOrderProfile => {
                &[]
            }
            Self::UdpTx => &[("OPEN_RADIO_TX_BENCH", "1"), ("OPEN_RADIO_HT_SGI", "1")],
            Self::StationTxFault => &[
                ("OPEN_RADIO_TX_BENCH", "1"),
                ("OPEN_RADIO_PERF_AP", "1"),
                ("OPEN_RADIO_HT_SGI", "1"),
            ],
            Self::Bidirectional => &[
                ("OPEN_RADIO_TX_BENCH", "1"),
                ("OPEN_RADIO_BIDIRECTIONAL_BENCH", "1"),
                ("OPEN_RADIO_HT_SGI", "1"),
            ],
            Self::Tcp => &[("OPEN_RADIO_TCP_BENCH", "1"), ("OPEN_RADIO_HT_SGI", "1")],
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::BootSmoke => "bootstrap, Flash/PSRAM and runtime smoke test",
            Self::Radio => "production ConnectedRunner PHY/MAC/STA/WPA2 HIL",
            Self::RadioPollProfile => {
                "radio HIL with diagnostic Embassy Future::poll residence telemetry"
            }
            Self::RadioRxOrderProfile => {
                "radio HIL correlating UDP and 802.11 receive sequence order"
            }
            Self::UdpTx => "production ConnectedRunner embassy-net UDP throughput",
            Self::StationTxFault => {
                "connected TX reset frontier against the repository-controlled AP"
            }
            Self::Bidirectional => "production ConnectedRunner simultaneous RX/TX throughput",
            Self::Tcp => "production ConnectedRunner embassy-net TCP RX/TX/full-duplex throughput",
        }
    }
}

fn build(root: &Path, scenario: Scenario) -> Result<Artifacts> {
    let local_esp_hal = local_esp_hal_override()?;
    if local_esp_hal.is_none() {
        return build_resolved(root, scenario, None);
    }

    let lockfile = root.join("hil/targets/esp32s31/Cargo.lock");
    let mut snapshot = TrackedFileSnapshot::capture(lockfile)?;
    let result = build_resolved(root, scenario, local_esp_hal.as_deref());
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
    scenario: Scenario,
    local_esp_hal: Option<&Path>,
) -> Result<Artifacts> {
    ensure_no_old_application_dependency(root)?;
    let manifest = root.join("hil/targets/esp32s31/Cargo.toml");
    let output =
        root.join("target/hil/esp32s31")
            .join(format!("{}-{}", QUALIFIED_PROFILE, scenario.name()));
    let runtime_target = output.join("cargo/runtime");
    let bootstrap_target = output.join("cargo/bootstrap");
    fs::create_dir_all(&output)?;
    fs::write(output.join("scenario.txt"), scenario_manifest(scenario))?;

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

    let runtime_features = format!(
        "{},code-psram,profile-psram-data",
        scenario.runtime_feature()
    );
    let mut runtime = cargo_command();
    runtime
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["-p", RUNTIME_BIN, "--release", "--target", TARGET])
        .args(["--no-default-features", "--features", &runtime_features])
        .env("CARGO_TARGET_DIR", &runtime_target);
    if local_esp_hal.is_none() {
        runtime.arg("--locked");
    }
    for variable in SCENARIO_ENVIRONMENT {
        runtime.env_remove(variable);
    }
    for (variable, value) in scenario.environment() {
        runtime.env(variable, value);
    }
    add_local_esp_hal_patches(&mut runtime, local_esp_hal);
    run_command(&mut runtime, "build stage-two runtime")?;
    require_file(&runtime_elf, "runtime ELF")?;

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
    run_command(&mut bootstrap, "build Flash/SRAM bootstrap")?;
    require_file(&bootstrap_elf, "bootstrap ELF")?;

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

fn parse_flash_arguments(
    mut arguments: impl Iterator<Item = String>,
) -> Result<(Scenario, PathBuf)> {
    let mut scenario = Scenario::BootSmoke;
    let mut port = env::var_os("ESPFLASH_PORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/dev/ttyACM0"));
    let mut scenario_seen = false;
    while let Some(argument) = arguments.next() {
        if argument == "--port" {
            port = arguments
                .next()
                .map(PathBuf::from)
                .ok_or("--port requires a serial device")?;
        } else if !scenario_seen {
            scenario = Scenario::parse(&argument)?;
            scenario_seen = true;
        } else {
            return Err(format!("unknown flash argument `{argument}`").into());
        }
    }
    Ok((scenario, port))
}

fn parse_oracle_flash_arguments(mut arguments: impl Iterator<Item = String>) -> Result<PathBuf> {
    let mut port = env::var_os("ESPFLASH_PORT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/dev/ttyACM0"));
    while let Some(argument) = arguments.next() {
        if argument != "--port" {
            return Err(format!("unknown oracle flash argument `{argument}`").into());
        }
        port = arguments
            .next()
            .map(PathBuf::from)
            .ok_or("--port requires a serial device")?;
    }
    Ok(port)
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

fn flash_oracle(root: &Path, artifacts: &OracleArtifacts, port: &Path) -> Result<()> {
    let partition_csv = root.join("hil/targets/esp32s31/partitions/hil.csv");
    let partition_bin = artifacts.output.join("partitions.bin");
    let selector_bin = artifacts.output.join("otadata-ota0-valid.bin");
    let mut partition = Command::new(program_from_env("ESPFLASH", "espflash"));
    partition
        .args(["partition-table", "--to-binary", "--output"])
        .arg(&partition_bin)
        .arg(&partition_csv);
    run_command(&mut partition, "encode vendor-oracle partition table")?;
    fs::write(&selector_bin, ota0_selector_image())?;
    write_flash_binary(
        port,
        PARTITION_TABLE_OFFSET,
        &partition_bin,
        "no-reset",
        "write vendor-oracle partition table",
    )?;
    write_flash_binary(
        port,
        OTA_0_OFFSET,
        &artifacts.application_image,
        "no-reset",
        "write vendor-oracle application",
    )?;
    write_flash_binary(
        port,
        OTA_SELECTOR_OFFSET,
        &selector_bin,
        "hard-reset",
        "select vendor-oracle ota_0 image",
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
    fn scenario_names_are_stable() {
        assert_eq!(Scenario::parse("boot-smoke").unwrap(), Scenario::BootSmoke);
        assert_eq!(Scenario::parse("radio").unwrap(), Scenario::Radio);
        assert_eq!(Scenario::Radio.name(), "open-radio-hil");
        assert_eq!(
            Scenario::parse("radio-poll-profile").unwrap(),
            Scenario::RadioPollProfile
        );
        assert_eq!(
            Scenario::RadioPollProfile.runtime_feature(),
            "open-radio-hil,task-poll-telemetry"
        );
        assert_eq!(
            Scenario::parse("radio-rx-order-profile").unwrap(),
            Scenario::RadioRxOrderProfile
        );
        assert_eq!(
            Scenario::RadioRxOrderProfile.runtime_feature(),
            "open-radio-hil,rx-order-telemetry"
        );
        assert_eq!(Scenario::parse("udp-tx").unwrap(), Scenario::UdpTx);
        assert_eq!(
            Scenario::parse("station-tx-fault").unwrap(),
            Scenario::StationTxFault
        );
        assert_eq!(
            Scenario::parse("bidirectional").unwrap(),
            Scenario::Bidirectional
        );
        assert_eq!(Scenario::parse("tcp-rx").unwrap(), Scenario::Tcp);
        assert_eq!(Scenario::parse("tcp").unwrap(), Scenario::Tcp);
        assert!(Scenario::parse("he-dcm-matrix").is_err());
    }

    #[test]
    fn scenario_arguments_and_artifact_names_are_unique() {
        for (index, scenario) in Scenario::ALL.iter().enumerate() {
            for other in &Scenario::ALL[index + 1..] {
                assert_ne!(scenario.argument(), other.argument());
                assert_ne!(scenario.name(), other.name());
            }
        }
    }

    #[test]
    fn scenario_environment_selects_one_reproducible_mode() {
        assert!(Scenario::Radio.environment().is_empty());
        assert!(Scenario::RadioPollProfile.environment().is_empty());
        assert!(Scenario::RadioRxOrderProfile.environment().is_empty());
        assert_eq!(
            Scenario::UdpTx.environment(),
            &[("OPEN_RADIO_TX_BENCH", "1"), ("OPEN_RADIO_HT_SGI", "1"),]
        );
        assert_eq!(
            Scenario::StationTxFault.environment(),
            &[
                ("OPEN_RADIO_TX_BENCH", "1"),
                ("OPEN_RADIO_PERF_AP", "1"),
                ("OPEN_RADIO_HT_SGI", "1"),
            ]
        );
        assert_eq!(
            Scenario::Bidirectional.environment(),
            &[
                ("OPEN_RADIO_TX_BENCH", "1"),
                ("OPEN_RADIO_BIDIRECTIONAL_BENCH", "1"),
                ("OPEN_RADIO_HT_SGI", "1"),
            ]
        );
        assert_eq!(
            Scenario::Tcp.environment(),
            &[("OPEN_RADIO_TCP_BENCH", "1"), ("OPEN_RADIO_HT_SGI", "1"),]
        );
        for scenario in Scenario::ALL {
            for (variable, _) in scenario.environment() {
                if variable.starts_with("OPEN_RADIO_") {
                    assert!(SCENARIO_ENVIRONMENT.contains(variable));
                }
            }
        }
    }

    #[test]
    fn scenario_manifest_records_identity_without_external_configuration() {
        let manifest = scenario_manifest(Scenario::UdpTx);
        assert!(manifest.contains("scenario=udp-tx\n"));
        assert!(manifest.contains("artifact=open-radio-udp-tx\n"));
        assert!(manifest.contains("OPEN_RADIO_TX_BENCH=1\n"));
        assert!(!manifest.contains("OPEN_RADIO_STA_PASSWORD"));
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
