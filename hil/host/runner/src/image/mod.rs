//! Reproducible HIL firmware construction and image auditing.

use crate::*;
use oer_process::CommandExt as _;
use open_esp_radio_hil_protocol::FeatureCapabilities;

mod class;
mod reproducibility;
pub(crate) mod stack;
pub(crate) use class::ImageClass;

pub(crate) use reproducibility::verify_rebuild;

pub(crate) const TARGET: &str = "riscv32imafc-unknown-none-elf";
const RUNTIME_BIN: &str = "open-esp-radio-hil-esp32s31-runtime";
use oer_firmware::{BOOTSTRAP_BIN, audit_application_image, pack_runtime};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ImageCapabilitySignature {
    driver_observation: bool,
    task_poll: bool,
    tx_architecture_probe: bool,
    core0_rx_cycles: bool,
    rx_delivery: bool,
    mac_irq: bool,
    ieee802154_event_status: bool,
    ieee802154_ed_event: bool,
    psram_task_stack: bool,
}

pub(crate) fn classify_flashed_capabilities(
    features: &FeatureCapabilities,
) -> Option<crate::image::ImageClass> {
    classify_image_signature(ImageCapabilitySignature {
        driver_observation: features.driver_observation_evidence,
        task_poll: features.task_poll_evidence,
        tx_architecture_probe: features.tx_architecture_probe,
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
) -> Option<crate::image::ImageClass> {
    use crate::image::ImageClass;

    if signature.tx_architecture_probe {
        let expected = ImageCapabilitySignature {
            driver_observation: false,
            task_poll: true,
            tx_architecture_probe: true,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
        };
        return (signature == expected).then_some(ImageClass::DiagnosticTxArchitecture);
    }

    match signature {
        ImageCapabilitySignature {
            driver_observation: false,
            task_poll: false,
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
            tx_architecture_probe: false,
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
    effective_embedded_lock: String,
    effective_bootstrap_lock: String,
    application_image: String,
    application_sha256: String,
    flashed: bool,
}

pub(crate) fn print_artifacts(
    class: crate::image::ImageClass,
    artifacts: &Artifacts,
    flashed: bool,
) -> Result<()> {
    let report = ArtifactReport {
        schema: crate::evidence::run::RUN_SCHEMA,
        image_class: class.id(),
        profile: class.runtime_profile(),
        runtime_elf: artifacts.runtime_elf.display().to_string(),
        runtime_bin: artifacts.runtime_bin.display().to_string(),
        bootstrap_elf: artifacts.bootstrap_elf.display().to_string(),
        effective_embedded_lock: artifacts.effective_embedded_lock.display().to_string(),
        effective_bootstrap_lock: artifacts.effective_bootstrap_lock.display().to_string(),
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
    pub(crate) effective_embedded_lock: PathBuf,
    pub(crate) effective_bootstrap_lock: PathBuf,
    pub(crate) application_image: PathBuf,
}

pub(crate) fn build(root: &Path, class: crate::image::ImageClass) -> Result<Artifacts> {
    let local_esp_hal = local_esp_hal_override()?;
    let local_embassy = local_embassy_override()?;
    let local_xarxa = local_xarxa_override()?;
    if local_esp_hal.is_none() && local_embassy.is_none() && local_xarxa.is_none() {
        return build_resolved(root, class, None, None, None, None, false);
    }

    let lockfile = root.join("hil/targets/esp32s31/Cargo.lock");
    let mut snapshot = TrackedFileSnapshot::capture(lockfile)?;
    let mut platform_snapshot =
        TrackedFileSnapshot::capture(root.join("platform/esp32s31/Cargo.lock"))?;
    let result = build_resolved(
        root,
        class,
        local_esp_hal.as_deref(),
        local_embassy.as_deref(),
        local_xarxa.as_deref(),
        None,
        false,
    );
    let restore = snapshot
        .restore()
        .and_then(|()| platform_snapshot.restore());
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
    class: crate::image::ImageClass,
    local_esp_hal: Option<&Path>,
    local_embassy: Option<&Path>,
    local_xarxa: Option<&Path>,
    output_override: Option<&Path>,
    trim_paths: bool,
) -> Result<Artifacts> {
    ensure_no_old_application_dependency(root)?;
    let manifest = root.join("hil/targets/esp32s31/Cargo.toml");
    let output = output_override.map_or_else(
        || {
            root.join("target/hil/esp32s31").join(format!(
                "{}-{}",
                class.runtime_profile(),
                class.id()
            ))
        },
        Path::to_owned,
    );
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
    let effective_embedded_lock = output.join("effective-Cargo.lock");
    let effective_bootstrap_lock = output.join("bootstrap-Cargo.lock");
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
        .env("CARGO_TARGET_DIR", &runtime_target)
        .env("CARGO_INCREMENTAL", "0");
    if local_esp_hal.is_none() {
        runtime.arg("--locked");
    }
    add_local_esp_hal_patches(&mut runtime, local_esp_hal);
    add_local_embassy_patches(&mut runtime, local_embassy);
    add_local_xarxa_patches(&mut runtime, local_xarxa);
    enable_experimental_path_trimming(&mut runtime, trim_paths);
    crate::image::stack::enable_stack_checks(&mut runtime, &stack_budget);
    run_command(&mut runtime, "build stage-two runtime")?;
    require_file(&runtime_elf, "runtime ELF")?;

    let stack_report = crate::image::stack::analyze_elf_stack(&runtime_elf, &stack_budget)?;
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
    let crc =
        pack_runtime(&runtime_bin).map_err(|error| -> Box<dyn Error + Send + Sync> { error })?;
    let placement = audit_runtime(&runtime_elf, &runtime_bin, class)?;
    fs::write(output.join("placement.txt"), placement)?;

    let mut bootstrap = cargo_command();
    oer_firmware::bootstrap_command(
        &mut bootstrap,
        root,
        &absolute(&runtime_bin)?,
        &bootstrap_target,
    );
    if local_esp_hal.is_none() {
        bootstrap.arg("--locked");
    }
    add_local_esp_hal_patches(&mut bootstrap, local_esp_hal);
    add_local_embassy_patches(&mut bootstrap, local_embassy);
    add_local_xarxa_patches(&mut bootstrap, local_xarxa);
    enable_experimental_path_trimming(&mut bootstrap, trim_paths);
    crate::image::stack::enable_stack_checks(&mut bootstrap, &stack_budget);
    run_command(&mut bootstrap, "build Flash/SRAM bootstrap")?;
    require_file(&bootstrap_elf, "bootstrap ELF")?;
    let bootstrap_stack_report =
        crate::image::stack::analyze_elf_stack(&bootstrap_elf, &stack_budget)?;
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

    let mut save_image = Command::new(program_from_env("ESPFLASH", "espflash"));
    oer_firmware::save_image_command(&mut save_image, root, &bootstrap_elf, &application_image);
    run_command(&mut save_image, "encode ESP application image")?;
    audit_application_image(&application_image)
        .map_err(|error| -> Box<dyn Error + Send + Sync> { error })?;
    fs::copy(
        root.join("hil/targets/esp32s31/Cargo.lock"),
        &effective_embedded_lock,
    )?;

    fs::copy(
        root.join("platform/esp32s31/Cargo.lock"),
        &effective_bootstrap_lock,
    )?;

    eprintln!("runtime_crc32={crc:08x}");
    eprintln!("placement_audit=PASS");
    eprintln!("stack_frame_audit=PASS");
    eprintln!("autonomous_source_graph=PASS");
    Ok(Artifacts {
        output,
        runtime_elf,
        runtime_bin,
        bootstrap_elf,
        effective_embedded_lock,
        effective_bootstrap_lock,
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

fn local_embassy_override() -> Result<Option<PathBuf>> {
    let Some(local) = env::var_os("EMBASSY_ROOT").map(PathBuf::from) else {
        return Ok(None);
    };
    let packages = ["embassy-net", "embassy-net-driver"];
    let missing = packages
        .iter()
        .filter_map(|path| (!local.join(path).is_dir()).then_some(*path))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "EMBASSY_ROOT={} is missing required package directories: {}",
            local.display(),
            missing.join(", ")
        )
        .into());
    }
    Ok(Some(local))
}

fn local_xarxa_override() -> Result<Option<PathBuf>> {
    // Do not use an XARXA_* name here: Xarxa's build script owns that prefix
    // for compile-time protocol configuration and rejects unknown variables.
    let Some(local) = env::var_os("OPEN_RADIO_XARXA_ROOT").map(PathBuf::from) else {
        return Ok(None);
    };
    let packages = ["xarxa-driver"];
    let missing = packages
        .iter()
        .filter_map(|path| (!local.join(path).is_dir()).then_some(*path))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "OPEN_RADIO_XARXA_ROOT={} is missing required package directories: {}",
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

fn add_local_embassy_patches(command: &mut Command, local: Option<&Path>) {
    let Some(local) = local else {
        return;
    };
    for package in ["embassy-net", "embassy-net-driver"] {
        command.arg("--config").arg(format!(
            "patch.\"https://github.com/ermacv/embassy.git\".{package}.path=\"{}\"",
            local.join(package).display()
        ));
    }
}

fn add_local_xarxa_patches(command: &mut Command, local: Option<&Path>) {
    let Some(local) = local else {
        return;
    };
    for (package, package_root) in [
        ("xarxa", local.to_owned()),
        ("xarxa-driver", local.join("xarxa-driver")),
    ] {
        command.arg("--config").arg(format!(
            "patch.\"https://github.com/ermacv/xarxa.git\".{package}.path=\"{}\"",
            package_root.display()
        ));
    }
}

fn enable_experimental_path_trimming(command: &mut Command, enabled: bool) {
    if enabled {
        // This remains an opt-in build experiment. The normal recipe must not
        // acquire an unstable Cargo setting until a same-source HIL A/B proves
        // both byte identity and equivalent performance.
        command
            .args(["-Z", "trim-paths"])
            .args(["--config", "profile.release.trim-paths=\"object\""]);
    }
}

pub(crate) fn run_command(command: &mut Command, description: &str) -> Result<()> {
    eprintln!("==> {description}");
    let status = oer_process::owned::Child::spawn(command)?
        .wait_timeout(Some(std::time::Duration::from_secs(30 * 60)))?;
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

pub(crate) fn require_program(program: &std::ffi::OsStr) -> Result<()> {
    let status = Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .supervised_status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "required program `{}` is unavailable",
            program.to_string_lossy()
        )
        .into())
    }
}

pub(crate) fn ensure_no_old_application_dependency(root: &Path) -> Result<()> {
    for relative in [
        "hil/targets/esp32s31/Cargo.toml",
        "platform/esp32s31/bootstrap/Cargo.toml",
        "hil/targets/esp32s31/runtime/Cargo.toml",
        "platform/esp32s31/board/Cargo.toml",
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

fn audit_runtime(elf: &Path, binary: &Path, class: crate::image::ImageClass) -> Result<String> {
    let report = oer_firmware::audit_runtime(elf, binary, class.uses_psram_task_stack())
        .map_err(|error| -> Box<dyn Error + Send + Sync> { error })?;
    if class != crate::image::ImageClass::BootSmoke {
        use object::{Object, ObjectSection, ObjectSymbol};
        let bytes = fs::read(elf)?;
        let object = object::File::parse(bytes.as_slice())?;
        let section = object
            .section_by_name(".critical.data")
            .ok_or("missing critical data")?;
        let range = section.address()..section.address() + section.size();
        for expected in ["RX_PIPELINE", "AGGREGATE_TX", "MAC_IRQ", "TASK_POLLS"] {
            if !object.symbols().any(|symbol| {
                symbol.name().is_ok_and(|name| name.contains(expected))
                    && range.contains(&symbol.address())
            }) {
                return Err(format!("HIL observer {expected} is outside critical data").into());
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
