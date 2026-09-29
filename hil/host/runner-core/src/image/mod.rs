//! Reproducible HIL firmware construction and image auditing.

use std::num::NonZeroU32;
use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::Result;
use oer_hil_protocol::{DiagnosticFeature, DiagnosticFeatures, FeatureCapabilities};
use oer_process::CommandExt as _;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub use oer_esp32s31_firmware::network::Integration;

mod features;
pub use features::FeatureDelta;
mod class;
pub mod esp_idf;
pub use esp_idf::XTASK_ENV;
pub mod snapshot;
pub mod source_inputs;
pub mod stack;
pub use class::ImageClass;

pub const TARGET: &str = "riscv32imafc-unknown-none-elf";
const RUNTIME_BIN: &str = "oer-hil-esp32s31-runtime";
use oer_esp32s31_firmware::{BOOTSTRAP_BIN, audit_application_image, pack_runtime};

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
    memory_benchmark: bool,
}

pub fn classify_flashed_capabilities(
    features: &FeatureCapabilities,
) -> Option<crate::image::ImageClass> {
    if features.bluetooth_secure_gatt {
        let expected = FeatureCapabilities {
            bluetooth_secure_gatt: true,
            structured_evidence: true,
            psram_task_stack: true,
            ..FeatureCapabilities::default()
        };
        return (*features == expected).then_some(ImageClass::BluetoothSecureGatt);
    }
    if features.bluetooth_gatt && features.wifi_role_control {
        // The joint image is the correctness Wi-Fi image with the GATT
        // application beside it.
        let mut wifi = *features;
        wifi.bluetooth_gatt = false;
        return (classify_flashed_capabilities(&wifi) == Some(ImageClass::Correctness))
            .then_some(ImageClass::WifiBleCoex);
    }
    if features.bluetooth_gatt {
        let expected = FeatureCapabilities {
            bluetooth_gatt: true,
            structured_evidence: true,
            psram_task_stack: true,
            ..FeatureCapabilities::default()
        };
        return (*features == expected).then_some(ImageClass::BluetoothGatt);
    }
    if features.system_watchdog {
        let expected = FeatureCapabilities {
            system_watchdog: true,
            structured_evidence: true,
            psram_task_stack: true,
            ..FeatureCapabilities::default()
        };
        return (*features == expected).then_some(ImageClass::SystemWatchdog);
    }
    if features.bluetooth_dtm {
        let expected = FeatureCapabilities {
            bluetooth_dtm: true,
            bluetooth_hci: features.bluetooth_hci,
            // Sealed older Bluetooth images retain their original placement.
            phy_rx_hot_sram: features.phy_rx_hot_sram,
            structured_evidence: true,
            psram_task_stack: true,
            ..FeatureCapabilities::default()
        };
        return (*features == expected).then_some(ImageClass::BluetoothDtm);
    }
    if features.phy_fault_injection {
        return None;
    }
    // The program-counter sampler rides in the classes that compile it: the
    // image is its class without the feature.
    if features
        .diagnostic_features
        .contains(DiagnosticFeature::PcProfile)
    {
        let mut control = *features;
        control.diagnostic_features = features
            .diagnostic_features
            .with(DiagnosticFeature::PcProfile, false);
        return classify_flashed_capabilities(&control)
            .filter(|class| class.samples_program_counter());
    }
    // A diagnostic-feature class is the performance image plus exactly its
    // one feature.
    if !features.diagnostic_features.is_empty() {
        let mut control = *features;
        control.diagnostic_features = DiagnosticFeatures::empty();
        if classify_flashed_capabilities(&control) != Some(ImageClass::Performance) {
            return None;
        }
        let only = |feature| {
            features.diagnostic_features == DiagnosticFeatures::empty().with(feature, true)
        };
        return if only(DiagnosticFeature::RxOwnership) {
            Some(ImageClass::DiagnosticRxOwnership)
        } else if only(DiagnosticFeature::StationExit) {
            Some(ImageClass::DiagnosticStationExit)
        } else {
            None
        };
    }
    if features.ieee802154_route_probe {
        let mut control = *features;
        control.ieee802154_route_probe = false;
        return (classify_flashed_capabilities(&control) == Some(ImageClass::Performance))
            .then_some(ImageClass::DiagnosticIeee802154Route);
    }
    if features.ieee802154_air_check || features.ieee802154_session || features.ieee802154_thread {
        let mut control = *features;
        control.ieee802154_air_check = false;
        control.ieee802154_session = false;
        control.ieee802154_thread = false;
        let radio = features.ieee802154_air_check
            && features.ieee802154_session
            && classify_flashed_capabilities(&control) == Some(ImageClass::Performance);
        return radio.then_some(if features.ieee802154_thread {
            ImageClass::DiagnosticIeee802154Thread
        } else {
            ImageClass::DiagnosticIeee802154Radio
        });
    }
    if features.phy_rx_hot_sram {
        return None;
    }
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
        memory_benchmark: features.memory_benchmark,
    })
}

fn classify_image_signature(
    signature: ImageCapabilitySignature,
) -> Option<crate::image::ImageClass> {
    use crate::image::ImageClass;

    if signature.memory_benchmark {
        let expected = ImageCapabilitySignature {
            driver_observation: false,
            task_poll: false,
            tx_architecture_probe: false,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: false,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
            memory_benchmark: true,
        };
        return (signature == expected).then_some(ImageClass::DiagnosticMemoryBenchmark);
    }

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
            memory_benchmark: false,
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
            memory_benchmark: false,
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
            memory_benchmark: false,
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
            memory_benchmark: false,
        } => Some(ImageClass::DiagnosticMacIrq),
        ImageCapabilitySignature {
            driver_observation: true,
            task_poll: true,
            tx_architecture_probe: false,
            core0_rx_cycles: false,
            rx_delivery: false,
            mac_irq: true,
            ieee802154_event_status: false,
            ieee802154_ed_event: false,
            psram_task_stack: true,
            memory_benchmark: false,
        } => Some(ImageClass::DiagnosticTxWait),
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
            memory_benchmark: false,
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
            memory_benchmark: false,
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
            memory_benchmark: false,
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
            memory_benchmark: false,
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
            memory_benchmark: false,
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
            memory_benchmark: false,
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
            memory_benchmark: false,
        } => Some(ImageClass::DiagnosticIeee802154EdEvent),
        _ => None,
    }
}

/// Version of the `image build`/`image flash` artifact report on stdout.
const ARTIFACT_REPORT_SCHEMA: u16 = 2;

#[derive(Serialize)]
struct ArtifactReport<'a> {
    schema: u16,
    image_class: &'a str,
    target: &'a str,
    network: &'a str,
    profile: &'a str,
    runtime_elf: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_bin: Option<String>,
    runtime_stack_report: String,
    placement_report: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    bootstrap_elf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bootstrap_stack_report: Option<String>,
    effective_embedded_lock: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_bootstrap_lock: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bootloader: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    partition_table: Option<String>,
    application_image: String,
    application_sha256: String,
    stack_frame_audit: &'a str,
    move_size_audit: &'a str,
    placement_audit: &'a str,
    application_audit: &'a str,
    autonomous_source_graph: &'a str,
    flashed: bool,
}

pub fn print_artifacts(
    class: crate::image::ImageClass,
    artifacts: &Artifacts,
    flashed: bool,
) -> Result<()> {
    let staged = artifacts.staged();
    let esp_idf = match &artifacts.boot {
        BootArtifacts::EspIdf(boot) => Some(boot),
        BootArtifacts::Staged { .. } => None,
    };
    let report = ArtifactReport {
        schema: ARTIFACT_REPORT_SCHEMA,
        image_class: class.id(),
        target: TARGET,
        network: artifacts.network.id(),
        profile: class.runtime_profile(),
        runtime_elf: artifacts.runtime_elf.display().to_string(),
        runtime_bin: staged.map(|(runtime_bin, ..)| runtime_bin.display().to_string()),
        runtime_stack_report: artifacts
            .output
            .join("runtime-stack.txt")
            .display()
            .to_string(),
        placement_report: artifacts.output.join("placement.txt").display().to_string(),
        bootstrap_elf: staged.map(|(_, bootstrap_elf, _)| bootstrap_elf.display().to_string()),
        bootstrap_stack_report: staged.map(|_| {
            artifacts
                .output
                .join("bootstrap-stack.txt")
                .display()
                .to_string()
        }),
        effective_embedded_lock: artifacts.effective_embedded_lock.display().to_string(),
        effective_bootstrap_lock: staged.map(|(.., lock)| lock.display().to_string()),
        bootloader: esp_idf.map(|boot| boot.bootloader.display().to_string()),
        partition_table: esp_idf.map(|boot| boot.partition_table.display().to_string()),
        application_image: artifacts.application_image.display().to_string(),
        application_sha256: sha256_file(&artifacts.application_image)?,
        stack_frame_audit: "PASS",
        move_size_audit: "PASS",
        placement_audit: "PASS",
        application_audit: "PASS",
        autonomous_source_graph: "PASS",
        flashed,
    };
    crate::emit_json(&report, true)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(fs::read(path)?);
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Clone)]
pub struct Artifacts {
    /// The chip the image runs on and its Rust target triple.
    pub chip: String,
    pub rust_target: String,
    pub network: Integration,
    pub output: PathBuf,
    pub runtime_elf: PathBuf,
    pub effective_embedded_lock: PathBuf,
    pub application_image: PathBuf,
    /// What the chip's boot flow adds to the application.
    pub boot: BootArtifacts,
    /// `source-inputs.json`: the repository files the image was built from.
    pub source_inputs: Option<PathBuf>,
    /// Host tools that produced this build, recorded in its provenance.
    pub environment: crate::evidence::build::BuildEnvironment,
    /// The seed the runtime's code and read-only data were shuffled by;
    /// `None` is the linker's natural order.
    pub layout_seed: Option<NonZeroU32>,
    /// Runtime features added to or removed from the class's own.
    pub features: FeatureDelta,
}

/// The profile of `chip` in the repository this runner was built from.
pub fn chip_profile(chip: &str) -> Result<oer_chip_profile::Profile> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    oer_chip_profile::Profile::load(&root, chip).map_err(|error| error.to_string().into())
}

/// The files an image's boot flow writes or needs besides the application.
#[derive(Clone, Debug)]
pub enum BootArtifacts {
    /// The ROM loads the platform bootstrap, which stages the packed runtime.
    Staged {
        runtime_bin: PathBuf,
        bootstrap_elf: PathBuf,
        effective_bootstrap_lock: PathBuf,
    },
    /// The chip's catalog ESP-IDF bootloader loads the application from its
    /// partition.
    EspIdf(EspIdfBoot),
}

/// An ESP-IDF application's bootloader, partition table and flash layout.
#[derive(Clone, Debug)]
pub struct EspIdfBoot {
    /// The chip as `espflash` names it.
    pub espflash_chip: String,
    pub bootloader: PathBuf,
    pub partition_table: PathBuf,
    pub flash: oer_chip_profile::FlashLayout,
}

impl Artifacts {
    /// The staged boot files, for code that exists only for staged images.
    pub fn staged(&self) -> Option<(&Path, &Path, &Path)> {
        match &self.boot {
            BootArtifacts::Staged {
                runtime_bin,
                bootstrap_elf,
                effective_bootstrap_lock,
            } => Some((runtime_bin, bootstrap_elf, effective_bootstrap_lock)),
            BootArtifacts::EspIdf(_) => None,
        }
    }
}

/// The seed of a runtime image's link order: `None` keeps the linker's
/// natural order, a seed shuffles ordinary code and read-only data by it.
pub type LayoutSeed = Option<NonZeroU32>;

/// How a run builds its images from the current sources.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentBuild {
    pub network: Integration,
    pub layout_seed: LayoutSeed,
    /// Runtime features added to or removed from each class's own; empty
    /// outside an experiment.
    pub features: FeatureDelta,
}

/// The build environment's layout seed variable.
pub const LAYOUT_SEED_ENV: &str = oer_esp32s31_platform_layout::build::LAYOUT_SEED_ENV;

pub fn build(
    root: &Path,
    class: crate::image::ImageClass,
    network: Integration,
    layout_seed: LayoutSeed,
    features: &FeatureDelta,
) -> Result<Artifacts> {
    build_selected(root, class, network, layout_seed, features)
}

fn build_selected(
    root: &Path,
    class: crate::image::ImageClass,
    network: Integration,
    layout_seed: LayoutSeed,
    features: &FeatureDelta,
) -> Result<Artifacts> {
    let local_esp_hal = local_esp_hal_override()?;
    let local_embassy = local_embassy_override()?;
    let local_xarxa = local_xarxa_override()?;
    build_resolved(
        root,
        class,
        network,
        LocalOverrides {
            esp_hal: local_esp_hal.as_deref(),
            embassy: local_embassy.as_deref(),
            xarxa: local_xarxa.as_deref(),
        },
        BuildPlacement {
            output: None,
            cache: &shared_compile_cache(root, class, network),
            layout_seed,
            features,
        },
    )
}

/// Where one image build writes its artifacts and keeps compiled units.
#[derive(Clone, Copy)]
pub(crate) struct BuildPlacement<'a> {
    /// Artifact directory; the class/network default when absent.
    pub(crate) output: Option<&'a Path>,
    /// The compile cache shared by every build of one image class and
    /// network. Cargo fingerprints decide reuse: registry packages are
    /// reused, while packages from a freshly materialized source tree always
    /// rebuild.
    pub(crate) cache: &'a Path,
    /// The runtime's layout seed. It is part of the default artifact
    /// directory; a shared compile cache only relinks for it.
    pub(crate) layout_seed: LayoutSeed,
    /// Runtime features added to or removed from the class's own; part of
    /// the default artifact directory too.
    pub(crate) features: &'a FeatureDelta,
}

/// Type-check the runtime of `class` against the committed pins, with the
/// image build's target, features and compiler configuration but without
/// code generation, in the class's shared compile cache. A pre-push check:
/// lints that need monomorphization (`large_assignments`) and the link-time
/// placement and stack audits still need `cargo hil image build`.
pub fn check(root: &Path, class: crate::image::ImageClass, network: Integration) -> Result<()> {
    let cache = shared_compile_cache(root, class, network);
    let lock = oer_esp32s31_firmware::network::BuildLock::prepare(
        &root.join("hil/targets/esp32s31"),
        &cache.join("check-lock"),
    )?;
    let stack_budget =
        oer_memory_report::StackBudget::load(&root.join("hil/targets/esp32s31/stack.toml"))?;
    let mut command = cargo_command();
    command
        .current_dir(root)
        .arg("check")
        .arg("--manifest-path")
        .arg(root.join("hil/targets/esp32s31/Cargo.toml"))
        .args([
            "-p",
            RUNTIME_BIN,
            "--release",
            "--target",
            TARGET,
            "--locked",
        ])
        .args([
            "--no-default-features",
            "--features",
            &class.build_features(network),
        ])
        .env("CARGO_TARGET_DIR", cache.join("runtime"));
    lock.configure(&mut command);
    network.configure(&mut command, root);
    crate::image::stack::configure_image_compiler(&mut command, &stack_budget);
    let status = command.status()?;
    if !status.success() {
        return Err(format!("the {} runtime does not type-check", class.id()).into());
    }
    Ok(())
}

/// The shared compile cache of the repository at `root`.
/// Overrides the directory of the shared compile caches, so a baseline
/// worktree compiles into its checkout's warm caches: registry packages are
/// reused, while the worktree's own packages, at other paths, are units of
/// their own.
pub const BUILD_CACHE_ENV: &str = "OER_HIL_BUILD_CACHE";

/// The directory of the shared compile caches of the repository at `root`.
fn compile_cache_base(root: &Path, overridden: Option<std::ffi::OsString>) -> PathBuf {
    overridden
        .filter(|directory| !directory.is_empty())
        .map_or_else(
            || root.join("target/hil/esp32s31/build-cache"),
            PathBuf::from,
        )
}

pub(crate) fn shared_compile_cache(
    root: &Path,
    class: crate::image::ImageClass,
    network: Integration,
) -> PathBuf {
    compile_cache_base(root, std::env::var_os(BUILD_CACHE_ENV)).join(format!(
        "{}-{}-{}",
        class.runtime_profile(),
        class.id(),
        network.id()
    ))
}

#[derive(Default)]
struct LocalOverrides<'a> {
    esp_hal: Option<&'a Path>,
    embassy: Option<&'a Path>,
    xarxa: Option<&'a Path>,
}

fn build_resolved(
    root: &Path,
    class: crate::image::ImageClass,
    network: Integration,
    local: LocalOverrides<'_>,
    placement: BuildPlacement<'_>,
) -> Result<Artifacts> {
    let LocalOverrides {
        esp_hal: local_esp_hal,
        embassy: local_embassy,
        xarxa: local_xarxa,
    } = local;
    ensure_no_old_application_dependency(root)?;
    let manifest = root.join("hil/targets/esp32s31/Cargo.toml");
    let BuildPlacement {
        output: output_override,
        cache,
        layout_seed,
        features,
    } = placement;
    let output = output_override.map_or_else(
        || {
            root.join("target/hil/esp32s31").join(format!(
                "{}-{}-{}{}{}",
                class.runtime_profile(),
                class.id(),
                network.id(),
                seed_suffix(layout_seed),
                features.suffix()
            ))
        },
        Path::to_owned,
    );
    let runtime_target = cache.join("runtime");
    let bootstrap_target = cache.join("bootstrap");
    fs::create_dir_all(&output)?;
    fs::write(output.join("image-class.txt"), format!("{}\n", class.id()))?;
    let log = BuildLog::create(&output.join("build.log"))?;
    // Private copies of both committed catalogs: patched networks and local
    // overrides resolve into them, never into the source tree.
    let runtime_lock = oer_esp32s31_firmware::network::BuildLock::prepare(
        &root.join("hil/targets/esp32s31"),
        &output.join("locks/runtime"),
    )?;
    let bootstrap_lock = oer_esp32s31_firmware::network::BuildLock::prepare(
        &root.join("platform/esp32s31"),
        &output.join("locks/bootstrap"),
    )?;
    let overridden = local_esp_hal.is_some() || local_embassy.is_some() || local_xarxa.is_some();

    let compiled_runtime_elf = runtime_target
        .join(TARGET)
        .join("release")
        .join(RUNTIME_BIN);
    // Artifacts are copied out of the compile cache, which a later build of
    // the same class may overwrite.
    let runtime_elf = output.join("runtime.elf");
    let runtime_bin = output.join("runtime.bin");
    let compiled_bootstrap_elf = bootstrap_target
        .join(TARGET)
        .join("release")
        .join(BOOTSTRAP_BIN);
    let bootstrap_elf = output.join("bootstrap.elf");
    let effective_embedded_lock = output.join("effective-Cargo.lock");
    let effective_bootstrap_lock = output.join("bootstrap-Cargo.lock");
    let application_image = output.join("application.bin");

    let runtime_features = features.apply(&class.build_features(network));
    let stack_policy_path = root.join("hil/targets/esp32s31/stack.toml");
    let stack_budget = oer_memory_report::StackBudget::load(&stack_policy_path)?;
    let mut runtime = cargo_command();
    runtime
        .current_dir(root)
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["-p", RUNTIME_BIN, "--release", "--target", TARGET])
        .args(["--no-default-features", "--features", &runtime_features])
        .env("CARGO_TARGET_DIR", &runtime_target)
        .env("CARGO_INCREMENTAL", "0");
    // Only a recorded seed reaches the link: `cargo_command` removed any
    // inherited one.
    if let Some(seed) = layout_seed {
        runtime.env(LAYOUT_SEED_ENV, seed.to_string());
    }
    if !overridden {
        runtime.arg("--locked");
    }
    runtime_lock.configure(&mut runtime);
    network.configure(&mut runtime, root);
    add_local_esp_hal_patches(&mut runtime, local_esp_hal);
    add_local_embassy_patches(&mut runtime, local_embassy);
    add_local_xarxa_patches(&mut runtime, local_xarxa);
    crate::image::stack::configure_image_compiler(&mut runtime, &stack_budget);
    log.run(&mut runtime, "build stage-two runtime")?;
    require_file(&compiled_runtime_elf, "runtime ELF")?;
    fs::copy(&compiled_runtime_elf, &runtime_elf)?;
    // Local overrides deliberately resolve path packages; only a network
    // selection has a fixed expected pin change.
    if !overridden {
        runtime_lock.validate(root, network)?;
    }

    let stack_report = crate::image::stack::analyze_elf_stack(&runtime_elf, &stack_budget)?;
    let stack_report_path = output.join("runtime-stack.txt");
    fs::write(
        &stack_report_path,
        oer_memory_report::render_stack_report(&stack_report),
    )?;
    eprintln!("stack_report={}", stack_report_path.display());
    oer_memory_report::audit_stack(&stack_report)
        .map_err(|error| log.failed("runtime stack audit", error.into()))?;

    let mut objcopy = Command::new(program_from_env("LLVM_OBJCOPY", "llvm-objcopy"));
    objcopy
        .args(["-O", "binary"])
        .arg(&runtime_elf)
        .arg(&runtime_bin);
    log.run(&mut objcopy, "flatten stage-two runtime")?;
    let crc =
        pack_runtime(&runtime_bin).map_err(|error| -> Box<dyn Error + Send + Sync> { error })?;
    let placement = audit_runtime(&runtime_elf, &runtime_bin, class)
        .map_err(|error| log.failed("runtime placement audit", error))?;
    fs::write(output.join("placement.txt"), placement)?;

    let mut bootstrap = cargo_command();
    bootstrap.current_dir(root);
    oer_esp32s31_firmware::bootstrap_command(
        &mut bootstrap,
        root,
        &absolute(&runtime_bin)?,
        &bootstrap_target,
    );
    if local_esp_hal.is_none() {
        bootstrap.arg("--locked");
    }
    bootstrap_lock.configure(&mut bootstrap);
    add_bootstrap_patches(&mut bootstrap, local_esp_hal);
    crate::image::stack::configure_image_compiler(&mut bootstrap, &stack_budget);
    log.run(&mut bootstrap, "build Flash/SRAM bootstrap")?;
    require_file(&compiled_bootstrap_elf, "bootstrap ELF")?;
    fs::copy(&compiled_bootstrap_elf, &bootstrap_elf)?;
    let bootstrap_stack_report =
        crate::image::stack::analyze_elf_stack(&bootstrap_elf, &stack_budget)?;
    let bootstrap_stack_report_path = output.join("bootstrap-stack.txt");
    fs::write(
        &bootstrap_stack_report_path,
        oer_memory_report::render_stack_report(&bootstrap_stack_report),
    )?;
    eprintln!(
        "bootstrap_stack_report={}",
        bootstrap_stack_report_path.display()
    );
    oer_memory_report::audit_stack(&bootstrap_stack_report)
        .map_err(|error| log.failed("bootstrap stack audit", error.into()))?;

    let mut save_image = Command::new(program_from_env("ESPFLASH", "espflash"));
    oer_esp32s31_firmware::save_image_command(
        &mut save_image,
        root,
        &bootstrap_elf,
        &application_image,
    );
    log.run(&mut save_image, "encode ESP application image")?;
    audit_application_image(&application_image)
        .map_err(|error| log.failed("application image audit", error))?;
    fs::copy(runtime_lock.path(), &effective_embedded_lock)?;
    fs::copy(bootstrap_lock.path(), &effective_bootstrap_lock)?;
    let source_inputs = source_inputs::write(
        &output,
        &source_inputs::collect(
            root,
            &[
                (&runtime_target.join(TARGET).join("release"), RUNTIME_BIN),
                (
                    &bootstrap_target.join(TARGET).join("release"),
                    BOOTSTRAP_BIN,
                ),
            ],
        )?,
    )?;

    eprintln!("runtime_crc32={crc:08x}");
    eprintln!("placement_audit=PASS");
    eprintln!("stack_frame_audit=PASS");
    eprintln!("autonomous_source_graph=PASS");
    Ok(Artifacts {
        chip: String::from("esp32s31"),
        rust_target: String::from(TARGET),
        network,
        output,
        runtime_elf,
        effective_embedded_lock,
        application_image,
        boot: BootArtifacts::Staged {
            runtime_bin,
            bootstrap_elf,
            effective_bootstrap_lock,
        },
        source_inputs: Some(source_inputs),
        environment: crate::evidence::build::BuildEnvironment::capture(),
        layout_seed,
        features: features.clone(),
    })
}

/// The artifact directory suffix of a seeded build, so a seed never reuses
/// another layout's artifacts.
pub(crate) fn seed_suffix(layout_seed: LayoutSeed) -> String {
    layout_seed.map_or_else(String::new, |seed| format!("-seed{seed}"))
}

fn cargo_command() -> Command {
    let mut command = Command::new(program_from_env("CARGO", "cargo"));
    for variable in inherited_build_overrides(env::vars_os().map(|(name, _)| name)) {
        command.env_remove(variable);
    }
    // A layout seed reaches a build only as the build's recorded seed, never
    // from the caller's environment.
    command.env_remove(LAYOUT_SEED_ENV);
    command
}

/// Inherited Cargo variables that would change a firmware image without
/// appearing in the checkout: profiles, build and target settings. The build
/// relies on the repository's Cargo configuration instead. `RUSTFLAGS` stays
/// and is recorded in the build provenance; job count and target directory do
/// not change the image.
fn inherited_build_overrides(
    names: impl Iterator<Item = std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    names
        .filter(|name| {
            let name = name.to_string_lossy();
            ["CARGO_PROFILE_", "CARGO_BUILD_", "CARGO_TARGET_"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
                && name != "CARGO_BUILD_JOBS"
                && name != "CARGO_TARGET_DIR"
        })
        .collect()
}

pub fn program_from_env(variable: &str, fallback: &str) -> OsString {
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

/// The local overrides the bootstrap build takes: esp-hal's, the only one
/// it depends on. A patch it does not use would be recorded in its lock
/// file, which `--locked` refuses.
fn add_bootstrap_patches(command: &mut Command, local_esp_hal: Option<&Path>) {
    add_local_esp_hal_patches(command, local_esp_hal);
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

/// The log of one image build: every step's standard error, as the terminal
/// also shows it, so a failure's cause survives the caller's terminal.
pub struct BuildLog {
    path: PathBuf,
}

/// A build step that failed, with the line that says why when one does, and
/// the build log holding its whole output.
#[derive(Debug)]
pub struct BuildStepFailed {
    pub step: String,
    pub cause: String,
    pub log: PathBuf,
}

impl std::fmt::Display for BuildStepFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.step, self.cause)
    }
}

impl Error for BuildStepFailed {}

impl BuildLog {
    /// Start an empty log at `path`.
    pub fn create(path: &Path) -> Result<Self> {
        fs::write(path, b"")?;
        Ok(Self {
            path: path.to_owned(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn append(&self, text: &str) -> Result<()> {
        use std::io::Write as _;
        let mut file = fs::OpenOptions::new().append(true).open(&self.path)?;
        file.write_all(text.as_bytes())?;
        Ok(())
    }

    /// Run `command` as the step `description`, copying its standard error
    /// to the terminal and the log.
    pub fn run(&self, command: &mut Command, description: &str) -> Result<()> {
        use std::io::BufRead as _;
        eprintln!("==> {description}");
        self.append(&format!("==> {description}\n"))?;
        command.stderr(Stdio::piped());
        let mut child = oer_process::owned::Child::spawn(command)?;
        let stderr = child
            .take_stderr()
            .ok_or("the build step has no standard error")?;
        let log = self.path.clone();
        let copier = std::thread::spawn(move || -> Vec<String> {
            use std::io::Write as _;
            let mut file = fs::OpenOptions::new().append(true).open(&log).ok();
            let mut lines = Vec::new();
            for line in std::io::BufReader::new(stderr)
                .lines()
                .map_while(std::io::Result::ok)
            {
                eprintln!("{line}");
                if let Some(file) = file.as_mut() {
                    let _ = writeln!(file, "{line}");
                }
                lines.push(line);
            }
            lines
        });
        let status = child.wait_timeout(Some(std::time::Duration::from_secs(30 * 60)))?;
        let lines = copier.join().unwrap_or_default();
        if !status.success() {
            return Err(BuildStepFailed {
                step: description.to_owned(),
                cause: decisive_line(&lines).unwrap_or_else(|| format!("exit {status}")),
                log: self.path.clone(),
            }
            .into());
        }
        Ok(())
    }

    /// Record an in-process step's failure in the log and name the log.
    pub fn failed(
        &self,
        step: &str,
        error: Box<dyn Error + Send + Sync>,
    ) -> Box<dyn Error + Send + Sync> {
        let _ = self.append(&format!("==> {step}\n{error}\n"));
        BuildStepFailed {
            step: step.to_owned(),
            cause: error.to_string(),
            log: self.path.clone(),
        }
        .into()
    }
}

/// The line of a failed step's standard error that says why: an espflash
/// error id, else the last `error` line, else the last non-empty line.
pub fn decisive_line(lines: &[String]) -> Option<String> {
    let trimmed = || {
        lines
            .iter()
            .rev()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
    };
    trimmed()
        .find(|line| line.contains("espflash::"))
        .or_else(|| {
            trimmed().find(|line| {
                let lower = line.to_ascii_lowercase();
                lower.starts_with("error") || lower.contains(" error:") || lower.contains("× ")
            })
        })
        .or_else(|| trimmed().next())
        .map(str::to_owned)
}

pub fn run_command(command: &mut Command, description: &str) -> Result<()> {
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

pub fn require_program(program: &std::ffi::OsStr) -> Result<()> {
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

pub fn ensure_no_old_application_dependency(root: &Path) -> Result<()> {
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

pub fn ensure_vendor_dependencies_absent(root: &Path) -> Result<()> {
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
    let report = oer_esp32s31_firmware::audit_runtime(elf, binary)
        .map_err(|error| -> Box<dyn Error + Send + Sync> { error })?;
    use object::{Object, ObjectSection, ObjectSymbol};
    let bytes = fs::read(elf)?;
    let object = object::File::parse(bytes.as_slice())?;
    let critical = object
        .section_by_name(".critical.data")
        .map(|section| section.address()..section.address() + section.size());
    audit_radio_observers(
        class,
        critical,
        object
            .symbols()
            .filter_map(|symbol| symbol.name().ok().map(|name| (name, symbol.address()))),
    )?;
    Ok(report)
}

/// Radio compositions retain their observer state in critical SRAM. Images
/// without a radio composition still pass the shared firmware/stack audits,
/// but have no radio observer storage to require.
fn audit_radio_observers<'a>(
    class: ImageClass,
    critical: Option<std::ops::Range<u64>>,
    symbols: impl Iterator<Item = (&'a str, u64)>,
) -> Result<()> {
    if matches!(
        class,
        ImageClass::SystemWatchdog
            | ImageClass::BluetoothGatt
            | ImageClass::BluetoothSecureGatt
            | ImageClass::BluetoothDtm
            | ImageClass::BootSmoke
            | ImageClass::DiagnosticMemoryBenchmark
    ) {
        return Ok(());
    }
    let aggregate_required = matches!(
        class,
        ImageClass::Correctness
            | ImageClass::DiagnosticMacIrq
            | ImageClass::DiagnosticTxWait
            | ImageClass::DiagnosticTaskPoll
            | ImageClass::DiagnosticCore0RxCycles
            | ImageClass::DiagnosticRxDelivery
    );
    let range = critical.ok_or("missing critical data for HIL radio observers")?;
    let symbols: Vec<_> = symbols.collect();
    for expected in ["RX_PIPELINE", "AGGREGATE_TX", "MAC_IRQ", "TASK_POLLS"] {
        // Without driver-observation the linker may discard aggregate state.
        // If retained, its clock pointer still requires initialized SRAM.
        if expected == "AGGREGATE_TX"
            && !aggregate_required
            && !symbols.iter().any(|(name, _)| name.contains(expected))
        {
            continue;
        }
        if !symbols
            .iter()
            .any(|(name, address)| name.contains(expected) && range.contains(address))
        {
            return Err(
                format!("HIL observer {expected} is missing or outside critical data").into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod build_log_tests {
    use super::*;

    #[test]
    fn the_decisive_line_prefers_the_espflash_error_id() {
        let lines = [
            "   Compiling x",
            "Error: espflash::image_too_big",
            "  × Image size 4210448B exceeds partition size 4194304B",
            "",
        ]
        .map(String::from);
        assert_eq!(
            decisive_line(&lines).as_deref(),
            Some("Error: espflash::image_too_big")
        );
        let cargo = ["warning: x", "error: could not compile `y`", "  note"].map(String::from);
        assert_eq!(
            decisive_line(&cargo).as_deref(),
            Some("error: could not compile `y`")
        );
        assert_eq!(decisive_line(&["tail".into()]).as_deref(), Some("tail"));
        assert_eq!(decisive_line(&[]), None);
    }

    #[test]
    fn a_failed_step_keeps_its_output_in_the_log() {
        let directory = tempfile::tempdir().unwrap();
        let log = BuildLog::create(&directory.path().join("build.log")).unwrap();
        let error = log
            .run(
                Command::new("sh").args(["-c", "echo noise >&2; echo 'error: broken' >&2; exit 3"]),
                "a step",
            )
            .unwrap_err();
        let failed = error.downcast_ref::<BuildStepFailed>().unwrap();
        assert_eq!(failed.cause, "error: broken");
        let text = fs::read_to_string(log.path()).unwrap();
        assert!(
            text.contains("==> a step\nnoise\nerror: broken\n"),
            "{text}"
        );
    }
}
