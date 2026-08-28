//! Reproducible HIL firmware construction and image auditing.

use crate::*;
use open_esp_radio_hil_protocol::FeatureCapabilities;

pub(crate) const QUALIFIED_PROFILE: &str = "psram-code-psram-data";
pub(crate) const TARGET: &str = "riscv32imafc-unknown-none-elf";
const RUNTIME_BIN: &str = "open-esp-radio-hil-esp32s31-runtime";
const BOOTSTRAP_BIN: &str = "open-esp-radio-hil-esp32s31-bootstrap";
const RUNTIME_MAGIC: u32 = 0x3247_5453;
const RUNTIME_CRC_OFFSET: usize = 40;
const RUNTIME_HEADER_BYTES: usize = 44;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ImageCapabilitySignature {
    driver_observation: bool,
    task_poll: bool,
    core0_rx_cycles: bool,
    rx_delivery: bool,
    mac_irq: bool,
    ieee802154_event_status: bool,
    ieee802154_ed_event: bool,
    psram_task_stack: bool,
}

pub(crate) fn classify_flashed_capabilities(
    features: &FeatureCapabilities,
) -> Option<qualification::scenario::ImageClass> {
    classify_image_signature(ImageCapabilitySignature {
        driver_observation: features.driver_observation_evidence,
        task_poll: features.task_poll_evidence,
        core0_rx_cycles: features.core0_rx_cycle_evidence,
        rx_delivery: features.rx_delivery_evidence,
        mac_irq: features.mac_irq_evidence,
        ieee802154_event_status: features.ieee802154_event_status_probe,
        ieee802154_ed_event: features.ieee802154_ed_event_probe,
        psram_task_stack: features.psram_task_stack,
    })
}

fn classify_image_signature(
    signature: ImageCapabilitySignature,
) -> Option<qualification::scenario::ImageClass> {
    use qualification::scenario::ImageClass;

    match signature {
        ImageCapabilitySignature {
            driver_observation: false,
            task_poll: false,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::Performance),
        ImageCapabilitySignature {
            driver_observation: true,
            task_poll: false,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::Correctness),
        ImageCapabilitySignature {
            driver_observation: true,
            task_poll: false,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: true,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::DiagnosticMacIrq),
        ImageCapabilitySignature {
            driver_observation: true,
            task_poll: true,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::DiagnosticTaskPoll),
        ImageCapabilitySignature {
            driver_observation: false,
            task_poll: true,
            core0_rx_cycles: true,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::DiagnosticCore0RxCoarse),
        ImageCapabilitySignature {
            driver_observation: true,
            task_poll: true,
            core0_rx_cycles: true,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::DiagnosticCore0RxCycles),
        ImageCapabilitySignature {
            driver_observation: false,
            task_poll: true,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::DiagnosticTaskResidence),
        ImageCapabilitySignature {
            driver_observation: true,
            task_poll: false,
            core0_rx_cycles: false,
            rx_delivery: true,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::DiagnosticRxDelivery),
        ImageCapabilitySignature {
            driver_observation: false,
            task_poll: false,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: true,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        } => Some(ImageClass::DiagnosticIeee802154EventStatus),
        ImageCapabilitySignature {
            driver_observation: false,
            task_poll: false,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: true,
            psram_task_stack: true,
        } => Some(ImageClass::DiagnosticIeee802154EdEvent),
        _ => None,
    }
}

#[derive(Serialize)]
struct ArtifactReport<'a> {
    schema: u16,
    image_class: &'a str,
    profile: &'a str,
    runtime_elf: String,
    runtime_bin: String,
    bootstrap_elf: String,
    application_image: String,
    application_sha256: String,
    flashed: bool,
}

pub(crate) fn print_artifacts(
    class: qualification::scenario::ImageClass,
    artifacts: &Artifacts,
    flashed: bool,
) -> Result<()> {
    let report = ArtifactReport {
        schema: reporting::run::RUN_SCHEMA,
        image_class: class.id(),
        profile: class.runtime_profile(),
        runtime_elf: artifacts.runtime_elf.display().to_string(),
        runtime_bin: artifacts.runtime_bin.display().to_string(),
        bootstrap_elf: artifacts.bootstrap_elf.display().to_string(),
        application_image: artifacts.application_image.display().to_string(),
        application_sha256: sha256_file(&artifacts.application_image)?,
        flashed,
    };
    crate::emit_json(&report, true)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(fs::read(path)?);
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) struct Artifacts {
    pub(crate) output: PathBuf,
    pub(crate) runtime_elf: PathBuf,
    pub(crate) runtime_bin: PathBuf,
    pub(crate) bootstrap_elf: PathBuf,
    pub(crate) application_image: PathBuf,
}

pub(crate) fn build(root: &Path, class: qualification::scenario::ImageClass) -> Result<Artifacts> {
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
    class: qualification::scenario::ImageClass,
    local_esp_hal: Option<&Path>,
) -> Result<Artifacts> {
    ensure_no_old_application_dependency(root)?;
    ensure_no_competing_log_writers(root)?;
    let manifest = root.join("hil/targets/esp32s31/Cargo.toml");
    let output = root.join("target/hil/esp32s31").join(format!(
        "{}-{}",
        class.runtime_profile(),
        class.id()
    ));
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
    evidence::stack_audit::enable_stack_checks(&mut runtime, &stack_budget);
    run_command(&mut runtime, "build stage-two runtime")?;
    require_file(&runtime_elf, "runtime ELF")?;

    let stack_report = evidence::stack_audit::analyze_elf_stack(&runtime_elf, &stack_budget)?;
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
    let placement = audit_runtime(&runtime_elf, &runtime_bin, class)?;
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
    evidence::stack_audit::enable_stack_checks(&mut bootstrap, &stack_budget);
    run_command(&mut bootstrap, "build Flash/SRAM bootstrap")?;
    require_file(&bootstrap_elf, "bootstrap ELF")?;
    let bootstrap_stack_report =
        evidence::stack_audit::analyze_elf_stack(&bootstrap_elf, &stack_budget)?;
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
    eprintln!("serialized_log_writer_audit=PASS");
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

fn cargo_command() -> Command {
    Command::new(program_from_env("CARGO", "cargo"))
}

pub(crate) fn program_from_env(variable: &str, fallback: &str) -> OsString {
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

pub(crate) fn run_command(command: &mut Command, description: &str) -> Result<()> {
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

pub(crate) fn require_program(program: &str) -> Result<()> {
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

pub(crate) fn ensure_no_old_application_dependency(root: &Path) -> Result<()> {
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

pub(crate) fn ensure_vendor_dependencies_absent(root: &Path) -> Result<()> {
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
    Ok(())
}

fn ensure_no_competing_log_writers(root: &Path) -> Result<()> {
    let scopes = [
        root.join("driver"),
        root.join("hil/targets/esp32s31/runtime/src"),
        root.join("hil/targets/esp32s31/bootstrap/src"),
    ];
    let mut sources = Vec::new();
    for scope in scopes {
        collect_rust_sources(&scope, &mut sources)?;
    }
    for source in sources {
        let relative = source.strip_prefix(root).unwrap_or(&source);
        let text = fs::read_to_string(&source)?;
        for (line_index, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for token in [
                "esp_println",
                "ets_printf",
                "emergency_log",
                "UsbSerialJtag",
            ] {
                if code.contains(token) && !log_writer_token_allowed(relative, token) {
                    return Err(format!(
                        "{}:{} uses competing log writer token `{token}`",
                        relative.display(),
                        line_index + 1,
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

fn collect_rust_sources(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_sources(&path, output)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            output.push(path);
        }
    }
    Ok(())
}

fn log_writer_token_allowed(path: &Path, token: &str) -> bool {
    match path.to_string_lossy().as_ref() {
        "hil/targets/esp32s31/runtime/src/boot_smoke_console.rs" => token == "UsbSerialJtag",
        "hil/targets/esp32s31/runtime/src/console.rs" => token != "esp_println",
        "hil/targets/esp32s31/runtime/src/main.rs" => {
            matches!(token, "ets_printf" | "emergency_log")
        }
        "hil/targets/esp32s31/bootstrap/src/main.rs" => token == "ets_printf",
        _ => false,
    }
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

fn audit_runtime(
    elf: &Path,
    binary: &Path,
    class: qualification::scenario::ImageClass,
) -> Result<String> {
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
    let critical_data_end = symbol("__runtime_critical_data_end")?;
    let critical_bss_end = symbol("__runtime_critical_bss_end")?;
    let dma_start = symbol("__runtime_dma_data_start")?;
    let dma_end = symbol("__runtime_dma_bss_end")?;
    let stack_bottom = symbol("_stack_end")?;
    let stack_top = symbol("_stack_start")?;
    let binary_bytes = fs::metadata(binary)?.len();
    let initialized_observers_valid = if class == qualification::scenario::ImageClass::BootSmoke {
        true
    } else {
        ["RX_PIPELINE", "AGGREGATE_TX", "MAC_IRQ", "TASK_POLLS"]
            .into_iter()
            .all(|expected| {
                symbols.iter().any(|(name, address)| {
                    name.contains(expected)
                        && *address >= critical_start
                        && *address < critical_data_end
                })
            })
    };

    let in_sram = |start: u64, end: u64| start >= 0x2f00_0000 && end >= start && end <= 0x2f07_afc0;
    let stack_placement_valid = if class.uses_psram_task_stack() {
        let cpu0_irq_bottom = symbol("__runtime_cpu0_irq_stack_bottom")?;
        let cpu0_irq_top = symbol("__runtime_cpu0_irq_stack_top")?;
        let cpu1_irq_bottom = symbol("__runtime_cpu1_irq_stack_bottom")?;
        let cpu1_irq_top = symbol("__runtime_cpu1_irq_stack_top")?;
        let trap_entry = symbol("_start_trap")?;
        let irq_entry_first = symbol("_runtime_psram_irq_entry_1")?;
        let irq_entry_last = symbol("_runtime_psram_irq_entry_47")?;
        let mtvt_source = symbol("_runtime_psram_mtvt_source")?;
        let cpu0_mtvt = symbol("_mtvt_table")?;
        let cpu1_mtvt = symbol("_mtvt_table2")?;
        let all_irq_entries_in_sram = (1..=47).all(|number| {
            symbols
                .get(&format!("_runtime_psram_irq_entry_{number}"))
                .is_some_and(|entry| in_sram(*entry, *entry + 4))
        });
        stack_bottom >= 0x5000_0000
            && stack_top <= 0x5100_0000
            && stack_top.saturating_sub(stack_bottom) == 0x3_0000
            && in_sram(cpu0_irq_bottom, cpu0_irq_top)
            && in_sram(cpu1_irq_bottom, cpu1_irq_top)
            && cpu0_irq_top.saturating_sub(cpu0_irq_bottom) == 0x8000
            && cpu1_irq_top.saturating_sub(cpu1_irq_bottom) == 0x8000
            && in_sram(trap_entry, trap_entry + 4)
            && in_sram(irq_entry_first, irq_entry_first + 4)
            && in_sram(irq_entry_last, irq_entry_last + 4)
            && in_sram(mtvt_source, mtvt_source + 48 * 4)
            && in_sram(cpu0_mtvt, cpu0_mtvt + 48 * 4)
            && in_sram(cpu1_mtvt, cpu1_mtvt + 48 * 4)
            && all_irq_entries_in_sram
    } else {
        stack_top == 0x2f07_afc0 && stack_top.saturating_sub(stack_bottom) >= 0x1_0000
    };
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
        || !initialized_observers_valid
        || !stack_placement_valid
    {
        return Err("runtime ELF violates the PSRAM/PSRAM placement contract".into());
    }
    if class.uses_psram_task_stack() {
        audit_psram_stack_entry_instructions(elf)?;
    }

    Ok(format!(
        "profile={}\n\
         image={image_start:#010x}..{payload_end:#010x}\n\
         text={text_start:#010x}..{text_end:#010x}\n\
         data_start={data_start:#010x}\n\
         bss_end={bss_end:#010x}\n\
         isr={isr_start:#010x}..{isr_end:#010x}\n\
         critical={critical_start:#010x}..{critical_bss_end:#010x}\n\
         dma={dma_start:#010x}..{dma_end:#010x}\n\
         stack={stack_bottom:#010x}..{stack_top:#010x}\n\
         result=PASS\n",
        class.runtime_profile()
    ))
}

fn audit_psram_stack_entry_instructions(elf: &Path) -> Result<()> {
    let mut names = vec!["_start_trap".to_owned()];
    names.extend((1..=47).map(|number| format!("_runtime_psram_irq_entry_{number}")));
    let output = Command::new(program_from_env("LLVM_OBJDUMP", "llvm-objdump"))
        .arg("-d")
        .arg(format!("--disassemble-symbols={}", names.join(",")))
        .arg(elf)
        .output()?;
    if !output.status.success() {
        return Err("llvm-objdump failed while auditing PSRAM stack entries".into());
    }
    let text = String::from_utf8(output.stdout)?;
    for name in names {
        let marker = format!("<{name}>:");
        let tail = text
            .split_once(&marker)
            .map(|(_, tail)| tail)
            .ok_or_else(|| format!("runtime disassembly lacks `{name}`"))?;
        let instruction = tail
            .lines()
            .find_map(|line| line.trim().split_once(':').map(|(_, body)| body.trim()))
            .filter(|body| !body.is_empty())
            .ok_or_else(|| format!("runtime disassembly has no instruction for `{name}`"))?;
        if !instruction.contains("csrrw") || !instruction.contains("sp, mscratch, sp") {
            return Err(format!(
                "`{name}` touches the interrupted stack before swapping to SRAM: `{instruction}`"
            )
            .into());
        }
    }
    Ok(())
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

    fn image_signature(
        driver_observation: bool,
        task_poll: bool,
        rx_delivery: bool,
        mac_irq: bool,
        ieee802154_event_status: bool,
        ieee802154_ed_event: bool,
    ) -> ImageCapabilitySignature {
        ImageCapabilitySignature {
            driver_observation,
            task_poll,
            core0_rx_cycles: false,
            rx_delivery,
            mac_irq,
            ieee802154_event_status,
            ieee802154_ed_event,
            psram_task_stack: true,
        }
    }

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
        ensure_vendor_dependencies_absent(&root).unwrap();
        ensure_no_competing_log_writers(&root).unwrap();
    }

    #[test]
    fn image_classes_are_stable_and_do_not_use_workload_environment() {
        assert_eq!(qualification::scenario::ImageClass::ALL.len(), 11);
        assert!(
            qualification::scenario::ImageClass::ALL
                .into_iter()
                .all(qualification::scenario::ImageClass::uses_psram_task_stack)
        );
        assert_eq!(
            qualification::scenario::ImageClass::Performance.id(),
            "performance"
        );
        assert_eq!(
            qualification::scenario::ImageClass::Correctness.id(),
            "correctness"
        );
        assert_eq!(
            qualification::scenario::ImageClass::Correctness.runtime_features(),
            "open-radio-hil,driver-observation,psram-task-stack,code-psram,profile-psram-data"
        );
        assert_eq!(
            qualification::scenario::ImageClass::DiagnosticMacIrq.runtime_features(),
            "open-radio-hil,psram-task-stack,mac-irq-telemetry,code-psram,profile-psram-data"
        );
        assert_eq!(
            qualification::scenario::ImageClass::DiagnosticTaskResidence.runtime_features(),
            "open-radio-hil,psram-task-stack,task-residence-telemetry,code-psram,profile-psram-data"
        );
        assert_eq!(
            qualification::scenario::ImageClass::DiagnosticTaskPoll.runtime_features(),
            "open-radio-hil,psram-task-stack,task-poll-telemetry,code-psram,profile-psram-data"
        );
        assert_eq!(
            qualification::scenario::ImageClass::DiagnosticCore0RxCoarse.runtime_features(),
            "open-radio-hil,psram-task-stack,core0-rx-coarse-telemetry,code-psram,profile-psram-data"
        );
        assert_eq!(
            qualification::scenario::ImageClass::DiagnosticCore0RxCycles.runtime_features(),
            "open-radio-hil,psram-task-stack,core0-rx-cycle-telemetry,code-psram,profile-psram-data"
        );
        assert_eq!(
            qualification::scenario::ImageClass::DiagnosticRxDelivery.runtime_features(),
            "open-radio-hil,psram-task-stack,rx-delivery-telemetry,code-psram,profile-psram-data"
        );
        assert_eq!(
            qualification::scenario::ImageClass::DiagnosticIeee802154EventStatus.runtime_features(),
            "open-radio-hil,ieee802154-event-status-probe,psram-task-stack,code-psram,profile-psram-data"
        );
        assert_eq!(
            qualification::scenario::ImageClass::DiagnosticIeee802154EdEvent.runtime_features(),
            "open-radio-hil,ieee802154-ed-event-probe,psram-task-stack,code-psram,profile-psram-data"
        );
    }

    #[test]
    fn image_capability_classifier_preserves_every_exclusive_class() {
        use qualification::scenario::ImageClass;

        for (signals, expected) in [
            (
                image_signature(false, false, false, false, false, false),
                ImageClass::Performance,
            ),
            (
                image_signature(true, false, false, false, false, false),
                ImageClass::Correctness,
            ),
            (
                image_signature(true, false, false, true, false, false),
                ImageClass::DiagnosticMacIrq,
            ),
            (
                image_signature(false, true, false, false, false, false),
                ImageClass::DiagnosticTaskResidence,
            ),
            (
                image_signature(true, true, false, false, false, false),
                ImageClass::DiagnosticTaskPoll,
            ),
            (
                image_signature(true, false, true, false, false, false),
                ImageClass::DiagnosticRxDelivery,
            ),
            (
                image_signature(false, false, false, false, true, false),
                ImageClass::DiagnosticIeee802154EventStatus,
            ),
            (
                image_signature(false, false, false, false, false, true),
                ImageClass::DiagnosticIeee802154EdEvent,
            ),
        ] {
            assert_eq!(classify_image_signature(signals), Some(expected));
        }
        let mut core0_rx_cycles = image_signature(true, true, false, false, false, false);
        core0_rx_cycles.core0_rx_cycles = true;
        assert_eq!(
            classify_image_signature(core0_rx_cycles),
            Some(ImageClass::DiagnosticCore0RxCycles),
        );
        let mut core0_rx_coarse = image_signature(false, true, false, false, false, false);
        core0_rx_coarse.core0_rx_cycles = true;
        assert_eq!(
            classify_image_signature(core0_rx_coarse),
            Some(ImageClass::DiagnosticCore0RxCoarse),
        );
    }

    #[test]
    fn image_capability_classifier_rejects_mixed_or_non_psram_images() {
        assert_eq!(
            classify_image_signature(image_signature(true, false, false, false, true, false)),
            None
        );
        assert_eq!(
            classify_image_signature(image_signature(false, true, false, false, true, false)),
            None
        );
        assert_eq!(
            classify_image_signature(image_signature(false, false, false, false, true, true)),
            None
        );

        let mut performance = image_signature(false, false, false, false, false, false);
        performance.psram_task_stack = false;
        assert_eq!(classify_image_signature(performance), None);
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
