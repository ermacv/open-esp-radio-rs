//! Run one authenticated ESP32-S31 vendor-comparison scenario.
use clap::{Parser, Subcommand};
use oer_esp32s31_vendor_scenarios::session::evidence_index::{self, Entry, Index};
use oer_esp32s31_vendor_scenarios::{
    calibration_leaves, calibration_prefix, channel, coverage,
    gain::{Gain, Options},
    gain_state::{self, Unmet},
    harness::{Budget, Result},
    harness_edges, i2c, i2c_transport, mac, observation,
    phy::PhyOptions,
    research, rfpll, rx_gain, session, state, tracking, tx_dc,
};
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

#[derive(Parser)]
#[command(about = "Typed ESP32-S31 vendor-comparison scenarios over the Blobray CLI")]
struct Cli {
    #[command(subcommand)]
    scenario: Scenario,
}

#[derive(Subcommand)]
enum Scenario {
    /// Wi-Fi/BT gain arithmetic and publication, calibration storage and the
    /// RF-test power producer. Missing `--rftest` is an unmet obligation.
    Gain {
        #[command(flatten)]
        common: Common,
        /// Authenticated `librftest.a`; without it the producer obligation stays unmet.
        #[arg(long)]
        rftest: Option<PathBuf>,
    },
    /// Channel restoration over installed ROM callbacks: full-root channel,
    /// temperature prefix over every sensor range and stuck-readiness containment.
    Channel {
        #[command(flatten)]
        common: Common,
    },
    /// Complete RX-gain root: publication guards, DC calibration and
    /// failed-channel, minimum-search and shared-budget containment.
    RxGain {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long)]
        phy_sdk: PathBuf,
    },
    /// Complete TX-DC/PWDET root: Wi-Fi/BT DC rows over constant and
    /// alternating SAR samples, and PBus/SAR fault containment.
    TxDc {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long)]
        phy_sdk: PathBuf,
    },
    /// Every PHY comparison scenario concurrently under one budget, each in its
    /// own output directory; any failure fails the run.
    All {
        #[command(flatten)]
        common: Common,
        /// Authenticated bootloader SDK firmware (calibration leaves and prefix).
        #[arg(long)]
        sdk: PathBuf,
        /// Authenticated PHY SDK firmware (RFPLL and diagnostics symbols).
        #[arg(long)]
        phy_sdk: PathBuf,
        /// Authenticated `librftest.a` (RF-test power producer).
        #[arg(long)]
        rftest: PathBuf,
        /// Authenticated `libpp.a` (Wi-Fi MAC HAL leaves).
        #[arg(long)]
        libpp: PathBuf,
        /// Write the native evidence index qualification reads.
        #[arg(long)]
        index: Option<PathBuf>,
    },
    /// Wi-Fi MAC HAL leaves of `libpp.a` over every radio register fill.
    WifiMac {
        #[command(flatten)]
        common: Common,
        /// Authenticated `libpp.a`.
        #[arg(long)]
        libpp: PathBuf,
    },
    /// Combined calibration and parameter tracking parents with their real
    /// children, RFPLL corrections and failed-TX containment.
    Tracking {
        #[command(flatten)]
        common: Common,
        /// Authenticated SDK firmware supplying the never-executed diagnostics symbol.
        #[arg(long)]
        phy_sdk: PathBuf,
    },
    /// Captured PHY research, navigation, register/data review and
    /// source-free preservation of every retained result.
    Research {
        /// `blobray` executable.
        #[arg(long)]
        binary: PathBuf,
        /// Pinned `libphy.a`.
        #[arg(long)]
        library: PathBuf,
        /// Pinned ROM ELF.
        #[arg(long)]
        rom: PathBuf,
        /// External linker selected for image preparation.
        #[arg(long)]
        linker: PathBuf,
        /// Ignored output root; each run creates a new `run-*` directory.
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        budget: Budget,
    },
    /// PHY I2C command memory and transport, harness call edges, calibration
    /// leaves and PBus/DCODE prefix (`--sdk`) and RFPLL (`--phy-sdk`). Each
    /// missing optional input is an unmet obligation.
    I2c {
        #[command(flatten)]
        common: Common,
        /// Authenticated linked SDK firmware (crystal-clock symbol companion).
        #[arg(long)]
        sdk: Option<PathBuf>,
        /// Authenticated SDK firmware with the RFPLL diagnostics symbol; requires `--sdk`.
        #[arg(long, requires = "sdk")]
        phy_sdk: Option<PathBuf>,
    },
}

#[derive(Clone, clap::Args)]
struct Common {
    /// `blobray` executable.
    #[arg(long)]
    binary: PathBuf,
    /// Pinned `libphy.a`.
    #[arg(long)]
    library: PathBuf,
    /// Pinned ROM ELF.
    #[arg(long)]
    rom: PathBuf,
    /// Freshly built production probe ELF.
    #[arg(long)]
    production: PathBuf,
    /// External linker selected for image preparation.
    #[arg(long)]
    linker: PathBuf,
    /// Ignored output root; each run creates a new `run-*` directory.
    #[arg(long)]
    output: PathBuf,
    #[command(flatten)]
    budget: Budget,
    /// Point mutant of the loaded production probe image in every comparison,
    /// `ADDRESS:ORIGINAL:REPLACEMENT` in hexadecimal bytes; repeat for several.
    /// The probe is not rebuilt; a failing scenario kills the mutant.
    #[arg(long = "patch", value_parser = parse_patch)]
    patches: Vec<blobray_application::in_process::ImagePatch>,
}

/// `ADDRESS:ORIGINAL:REPLACEMENT`, the bytes as hexadecimal strings.
fn parse_patch(
    text: &str,
) -> std::result::Result<blobray_application::in_process::ImagePatch, String> {
    let bytes = |hex: &str| -> std::result::Result<Vec<u8>, String> {
        if hex.is_empty() || !hex.len().is_multiple_of(2) {
            return Err(format!(
                "`{hex}` is not a whole number of hexadecimal bytes"
            ));
        }
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
            .collect()
    };
    let parts: Vec<&str> = text.split(':').collect();
    let [address, original, replacement] = parts[..] else {
        return Err("expected ADDRESS:ORIGINAL:REPLACEMENT".into());
    };
    Ok(blobray_application::in_process::ImagePatch {
        address: u32::from_str_radix(address.trim_start_matches("0x"), 16)
            .map_err(|e| e.to_string())?,
        original: bytes(original)?,
        replacement: bytes(replacement)?,
    })
}

/// Every patch replaces bytes it names correctly in an executable segment of
/// the production ELF, so a failing run means a killed mutant, not a bad patch.
fn check_patches(
    production: &Path,
    patches: &[blobray_application::in_process::ImagePatch],
) -> Result<()> {
    use object::{Object, ObjectSegment, SegmentFlags};
    /// ELF program header flag of an executable segment.
    const PF_X: u32 = 1;
    let bytes = std::fs::read(production)?;
    let file = object::File::parse(&*bytes)?;
    for patch in patches {
        let start = u64::from(patch.address);
        let end = start + patch.original.len() as u64;
        let found = file.segments().find(|segment| {
            matches!(segment.flags(), SegmentFlags::Elf { p_flags } if p_flags & PF_X != 0)
                && segment.address() <= start
                && end <= segment.address() + segment.size()
        });
        let data = found
            .ok_or_else(|| format!("patch at {:#x} is outside executable code", patch.address))?
            .data_range(start, patch.original.len() as u64)?;
        if data != Some(&patch.original[..]) {
            return Err(format!(
                "patch at {:#x} does not match the probe bytes",
                patch.address
            )
            .into());
        }
    }
    Ok(())
}

/// Exit code of one scenario.
fn single(outcome: Result<Outcome>) -> Result<ExitCode> {
    Ok(outcome?.0)
}

/// Evidence entries and untriaged coverage of one scenario, alongside its
/// exit code.
type Outcome = (ExitCode, session::Claims);

/// Vendor roots each scenario claims against its compiled production entry.
const GAIN_CLAIMS: [(&str, &str, &str); 4] = [
    (
        "archive",
        "phy_wifi_get_tx_tab_new",
        "open_phy_channel_trace_calculate_tx_gain",
    ),
    (
        "archive",
        "phy_set_tx_gain_mem_new",
        "open_phy_channel_trace_publish_tx_gain",
    ),
    (
        "archive",
        "phy_bt_get_tx_tab_new",
        "open_phy_bluetooth_trace_calculate_gain",
    ),
    (
        "archive",
        "phy_bt_set_tx_gain_new",
        "open_phy_bluetooth_trace_tx_gain",
    ),
];
const I2C_CLAIMS: [(&str, &str, &str); 7] = [
    (
        "archive",
        "phy_i2c_master_cmd_mem_init",
        "open_phy_trace_command_memory",
    ),
    (
        "archive",
        "phy_get_i2c_hostid_new",
        "open_phy_trace_i2c_host",
    ),
    ("rom", "phy_i2c_readReg", "open_phy_trace_i2c_transfer"),
    ("rom", "phy_i2c_writeReg", "open_phy_trace_i2c_transfer"),
    ("rom", "phy_i2c_readReg_Mask", "open_phy_trace_i2c_transfer"),
    (
        "rom",
        "phy_i2c_writeReg_Mask",
        "open_phy_trace_i2c_transfer",
    ),
    ("rom", "phy_i2c_master_reset", "open_phy_trace_i2c_reset"),
];
const I2C_SDK_CLAIMS: [(&str, &str, &str); 5] = [
    (
        "rom",
        "phy_pbus_clear_reg",
        "open_phy_calibration_trace_pbus_clear",
    ),
    (
        "archive",
        "phy_txgain_comp_pacfg_new",
        "open_phy_calibration_leaf",
    ),
    ("archive", "phy_force_dig_gain", "open_phy_calibration_leaf"),
    (
        "archive",
        "phy_temp_to_power_new",
        "open_phy_calibration_leaf",
    ),
    ("archive", "phy_reg_update_new", "open_phy_calibration_leaf"),
];
const I2C_RFPLL_CLAIMS: [(&str, &str, &str); 4] = [
    (
        "archive",
        "phy_rfpll_cap_init_cal_new",
        "open_phy_rfpll_trace_search",
    ),
    (
        "archive",
        "phy_rfpll_cap_track_new",
        "open_phy_rfpll_trace_maintain",
    ),
    (
        "archive",
        "phy_rfpll_cap_track_new",
        "open_phy_rfpll_trace_track",
    ),
    ("rom", "phy_set_rfpll_freq", "open_phy_rfpll_trace_program"),
];
const CHANNEL_CLAIMS: [(&str, &str, &str); 2] = [
    (
        "archive",
        "phy_chip_set_chan",
        "open_phy_channel_trace_state",
    ),
    ("rom", channel::SENSOR_ROOT, channel::SENSOR_ENTRY),
];
const RX_GAIN_CLAIMS: [(&str, &str, &str); 1] = [(
    "archive",
    "phy_set_rx_gain_table",
    "open_phy_calibration_trace_rx_gain",
)];
const TX_DC_CLAIMS: [(&str, &str, &str); 1] = [(
    "archive",
    "phy_txdc_cal_pwdet_init",
    "open_phy_calibration_trace_tx_dc_pwdet",
)];
const TRACKING_CLAIMS: [(&str, &str, &str); 2] = [
    (
        "archive",
        "phy_cal_param_track",
        "open_phy_calibration_trace_combined",
    ),
    (
        "archive",
        "phy_param_track_tot",
        "open_phy_tracking_trace_parent",
    ),
];

fn finish(unmet: &[Unmet], message: &str, run: &std::path::Path) -> ExitCode {
    if unmet.is_empty() {
        println!("{message} {}", run.display());
        ExitCode::SUCCESS
    } else {
        let ids: Vec<_> = unmet.iter().map(|o| o.id).collect();
        println!(
            "INCOMPLETE: unmet obligations: {} {}",
            ids.join(", "),
            run.display()
        );
        ExitCode::from(2)
    }
}

fn gain(common: Common, rftest: Option<PathBuf>) -> Result<Outcome> {
    let options = Options {
        binary: common.binary,
        library: common.library,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        rftest,
        patches: common.patches,
    };
    let mut g = Gain::new(&options)?;
    let unmet = gain_state::exercise(&mut g, options.rftest.is_some())?;
    g.runner.doc("unmet-obligations", &unmet)?;
    g.coefficient_boundaries()?;
    g.characterize()?;
    g.wifi()?;
    g.publish_wifi()?;
    g.bluetooth()?;
    g.additive()?;
    g.negative()?;
    let claims = g
        .session
        .claims("gain", &g.roots, &GAIN_CLAIMS, coverage::DECISIONS)?;
    Ok((
        finish(
            &unmet,
            "authenticated gain arithmetic/publication, gain state passed",
            &g.run,
        ),
        claims,
    ))
}

fn i2c(common: Common, sdk: Option<PathBuf>, phy_sdk: Option<PathBuf>) -> Result<Outcome> {
    let options = i2c::Options {
        binary: common.binary,
        library: common.library,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        sdk,
        phy_sdk,
        patches: common.patches,
    };
    let mut ctx = i2c::I2c::new(&options)?;
    let unmet = i2c::unmet(options.sdk.is_some(), options.phy_sdk.is_some());
    ctx.runner.doc("unmet-obligations", &unmet)?;
    if options.phy_sdk.is_some() {
        rfpll::exercise(&mut ctx)?;
    }
    if options.sdk.is_some() {
        calibration_prefix::exercise(&mut ctx)?;
    }
    ctx.command_memory()?;
    i2c_transport::exercise(&mut ctx)?;
    if options.sdk.is_some() {
        calibration_leaves::exercise(&mut ctx)?;
    }
    harness_edges::exercise(&mut ctx)?;
    // All original source copies were deleted before linking/execution. Preserve
    // the full project closure, including probe/ROM bytes and negative evidence.
    let mut list = I2C_CLAIMS.to_vec();
    if options.sdk.is_some() {
        list.extend(I2C_SDK_CLAIMS);
    }
    if options.phy_sdk.is_some() {
        list.extend(I2C_RFPLL_CLAIMS);
    }
    let claims = ctx
        .session
        .claims("i2c", &ctx.roots, &list, coverage::DECISIONS)?;
    Ok((
        finish(&unmet, "authenticated PHY comparisons passed", &ctx.run),
        claims,
    ))
}

impl Common {
    fn phy(self) -> PhyOptions {
        PhyOptions {
            binary: self.binary,
            library: self.library,
            rom: self.rom,
            production: self.production,
            linker: self.linker,
            output: self.output,
            budget: self.budget,
            patches: self.patches,
        }
    }
}

fn channel(common: Common) -> Result<Outcome> {
    let options = common.phy();
    let mut ctx = channel::Channel::new(&options)?;
    channel::exercise(&mut ctx)?;
    let claims = ctx
        .session
        .claims("channel", &ctx.roots, &CHANNEL_CLAIMS, coverage::DECISIONS)?;
    Ok((
        finish(
            &[],
            "authenticated channel restoration, temperature prefix, containment passed",
            &ctx.run,
        ),
        claims,
    ))
}

fn wifi_mac(common: Common, libpp: PathBuf) -> Result<Outcome> {
    let options = mac::MacOptions {
        binary: common.binary,
        libpp,
        rom: common.rom,
        production: common.production,
        linker: common.linker,
        output: common.output,
        budget: common.budget,
        patches: common.patches,
    };
    let mut ctx = mac::Mac::new(&options)?;
    mac::exercise(&mut ctx)?;
    let claims = ctx
        .session
        .claims("wifi-mac", &ctx.roots, &mac::claims(), coverage::DECISIONS)?;
    Ok((
        finish(&[], "authenticated Wi-Fi MAC HAL leaves passed", &ctx.run),
        claims,
    ))
}

fn rx_gain(common: Common, phy_sdk: PathBuf) -> Result<Outcome> {
    let mut ctx = rx_gain::RxGain::new(&common.phy(), &phy_sdk)?;
    rx_gain::exercise(&mut ctx)?;
    let claims = ctx
        .session
        .claims("rx-gain", &ctx.roots, &RX_GAIN_CLAIMS, coverage::DECISIONS)?;
    Ok((
        finish(
            &[],
            "authenticated RX gain publication, calibration, containment passed",
            &ctx.run,
        ),
        claims,
    ))
}

fn tx_dc(common: Common, phy_sdk: PathBuf) -> Result<Outcome> {
    let mut ctx = tx_dc::TxDc::new(&common.phy(), &phy_sdk)?;
    tx_dc::exercise(&mut ctx)?;
    let claims = ctx
        .session
        .claims("tx-dc", &ctx.roots, &TX_DC_CLAIMS, coverage::DECISIONS)?;
    Ok((
        finish(
            &[],
            "authenticated TX-DC/PWDET calibration, fault containment passed",
            &ctx.run,
        ),
        claims,
    ))
}

fn tracking(common: Common, phy_sdk: PathBuf) -> Result<Outcome> {
    let mut ctx = tracking::Tracking::new(&common.phy(), &phy_sdk)?;
    tracking::exercise(&mut ctx)?;
    let claims = ctx.session.claims(
        "tracking",
        &ctx.roots,
        &TRACKING_CLAIMS,
        coverage::DECISIONS,
    )?;
    Ok((
        finish(
            &[],
            "authenticated tracking parents, failed-TX containment passed",
            &ctx.run,
        ),
        claims,
    ))
}

/// Evidence index builders over the repository sources the verdicts depend on.
mod evidence {
    use super::*;

    use oer_esp32s31_vendor_scenarios::{PROBES_MANIFEST, PROBES_PACKAGE, PROBES_TARGET};

    /// Qualification target of this scenario package.
    const TARGET: &str = "esp32s31";
    /// Blobray workspace and this scenario package, whose path dependencies
    /// are the scenario code and the Blobray engine behind every verdict.
    const TOOL_MANIFEST: &str = "tools/blobray/Cargo.toml";
    const TOOL_PACKAGE: &str = env!("CARGO_PKG_NAME");
    /// Shared schema sources the scenarios include by path.
    const SCHEMA_SOURCES: &str = "verification/vendor/schema";

    pub use oer_esp32s31_vendor_scenarios::observation::root;

    fn sha256(path: &Path) -> Result<String> {
        use sha2::{Digest, Sha256};
        Ok(format!("{:x}", Sha256::digest(std::fs::read(path)?)))
    }

    /// Path packages in the resolved dependency closure of `package`.
    fn path_closure(
        root: &Path,
        manifest: &str,
        package: &str,
        platform: Option<&str>,
    ) -> Result<Vec<PathBuf>> {
        let mut command = std::process::Command::new("cargo");
        command
            .current_dir(root)
            .args(["metadata", "--format-version", "1", "--offline", "--locked"])
            .args(["--manifest-path", manifest]);
        if let Some(platform) = platform {
            command.args(["--filter-platform", platform]);
        }
        let output = command.output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let packages = metadata["packages"]
            .as_array()
            .ok_or("cargo metadata packages")?;
        let id_of = |name: &str| {
            packages
                .iter()
                .find(|p| p["name"] == name)
                .and_then(|p| p["id"].as_str())
                .map(str::to_owned)
        };
        let nodes = metadata["resolve"]["nodes"]
            .as_array()
            .ok_or("cargo metadata resolve")?;
        let mut pending = vec![id_of(package).ok_or_else(|| format!("package {package} missing"))?];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let node = nodes
                .iter()
                .find(|n| n["id"] == id.as_str())
                .ok_or("unresolved package")?;
            for dependency in node["dependencies"].as_array().into_iter().flatten() {
                pending.push(dependency.as_str().ok_or("dependency id")?.to_owned());
            }
        }
        let mut directories = std::collections::BTreeSet::new();
        for package in packages {
            let local = package["source"].is_null();
            if local && seen.contains(package["id"].as_str().unwrap_or_default()) {
                let manifest = Path::new(package["manifest_path"].as_str().ok_or("manifest path")?);
                let directory = manifest.parent().ok_or("manifest directory")?;
                directories.insert(directory.canonicalize()?.strip_prefix(root)?.to_path_buf());
            }
        }
        Ok(directories.into_iter().collect())
    }

    pub fn index(
        common: &Common,
        optional: &[&PathBuf; 4],
        entries: Vec<Entry>,
        untriaged: Vec<evidence_index::Location>,
        unobserved: Vec<evidence_index::SourceLine>,
        unprojected: Vec<evidence_index::StateRange>,
    ) -> Result<Index> {
        let root = root()?;
        let mut inputs = std::collections::BTreeMap::new();
        for (role, path) in [
            ("archive", &common.library),
            ("rom", &common.rom),
            ("production", &common.production),
            ("sdk", optional[0]),
            ("phy-sdk", optional[1]),
            ("rftest", optional[2]),
            ("libpp", optional[3]),
        ] {
            inputs.insert(role.to_owned(), sha256(path)?);
        }
        let mut directories =
            path_closure(&root, PROBES_MANIFEST, PROBES_PACKAGE, Some(PROBES_TARGET))?;
        directories.extend(path_closure(&root, TOOL_MANIFEST, TOOL_PACKAGE, None)?);
        directories.push(PathBuf::from(SCHEMA_SOURCES));
        directories.sort();
        directories.dedup();
        let sources = directories
            .into_iter()
            .map(|path| {
                Ok(evidence_index::SourceDigest {
                    sha256: evidence_index::digest_directory(&root, &path)?,
                    path,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let index = Index {
            schema: evidence_index::SCHEMA,
            command: evidence_index::COMMAND.into(),
            target: TARGET.into(),
            inputs,
            sources,
            entries,
            untriaged,
            unobserved,
            unprojected,
        };
        index.validate(TARGET)?;
        Ok(index)
    }
}

/// Run every PHY comparison scenario; each must pass with no unmet obligation.
fn all(
    common: Common,
    sdk: PathBuf,
    phy_sdk: PathBuf,
    rftest: PathBuf,
    libpp: PathBuf,
    index: Option<PathBuf>,
) -> Result<ExitCode> {
    if !common.patches.is_empty() {
        if index.is_some() {
            return Err("a point-mutant run writes no evidence index".into());
        }
        check_patches(&common.production, &common.patches)?;
        println!(
            "{} point-mutant patches applied to every production comparison",
            common.patches.len()
        );
    }
    let within = |name: &str| Common {
        output: common.output.join(name),
        ..common.clone()
    };
    type Run<'a> = Box<dyn FnOnce() -> Result<Outcome> + Send + 'a>;
    let scenarios: Vec<(&str, Run<'_>)> = vec![
        (
            "gain",
            Box::new(|| gain(within("gain"), Some(rftest.clone()))),
        ),
        (
            "i2c",
            Box::new(|| i2c(within("i2c"), Some(sdk.clone()), Some(phy_sdk.clone()))),
        ),
        ("channel", Box::new(|| channel(within("channel")))),
        (
            "rx-gain",
            Box::new(|| rx_gain(within("rx-gain"), phy_sdk.clone())),
        ),
        (
            "tx-dc",
            Box::new(|| tx_dc(within("tx-dc"), phy_sdk.clone())),
        ),
        (
            "tracking",
            Box::new(|| tracking(within("tracking"), phy_sdk.clone())),
        ),
        (
            "wifi-mac",
            Box::new(|| wifi_mac(within("wifi-mac"), libpp.clone())),
        ),
    ];
    // Scenarios share no state: each owns its session and output directory.
    // They run concurrently and their results join in the declared order.
    let outcomes: Vec<(&str, f64, Result<Outcome>)> = std::thread::scope(|scope| {
        let running: Vec<_> = scenarios
            .into_iter()
            .map(|(name, run)| {
                (
                    name,
                    scope.spawn(move || {
                        let start = std::time::Instant::now();
                        let outcome = run();
                        (start.elapsed().as_secs_f64(), outcome)
                    }),
                )
            })
            .collect();
        running
            .into_iter()
            .map(|(name, handle)| {
                let (seconds, outcome) = handle
                    .join()
                    .unwrap_or_else(|_| (0.0, Err(format!("scenario {name} panicked").into())));
                (name, seconds, outcome)
            })
            .collect()
    });
    let mut elapsed = vec![];
    let mut entries = vec![];
    let mut untriaged = std::collections::BTreeSet::new();
    let mut closures = vec![];
    let mut lines = observation::Lines::default();
    let mut unprojected = std::collections::BTreeSet::new();
    let mut failed = None;
    for (name, seconds, outcome) in outcomes {
        elapsed.push((name, seconds));
        match outcome {
            Ok((code, claims)) if code == ExitCode::SUCCESS => {
                entries.extend(claims.entries);
                untriaged.extend(claims.untriaged);
                closures.extend(claims.closures);
                lines.extend(&claims.lines);
                unprojected.extend(claims.unprojected);
            }
            Ok((code, _)) => {
                println!("scenario {name} did not pass");
                failed.get_or_insert(Ok(code));
            }
            Err(error) => {
                println!("scenario {name} failed: {error}");
                failed.get_or_insert(Err(error));
            }
        }
    }
    if let Some(failed) = failed {
        return failed;
    }
    // A line is unobserved when no scenario observes it; every decision must
    // still review one.
    let root = observation::root()?;
    let unobserved_lines = lines.unobserved();
    let mut sources = observation::Sources::default();
    sources.check(&root, observation::DECISIONS, &unobserved_lines)?;
    let (reviewed, unobserved) =
        sources.classify(&root, observation::DECISIONS, &unobserved_lines)?;
    let effect = unobserved
        .iter()
        .filter(|l| lines.effect.contains(*l))
        .count();
    let state = unobserved
        .iter()
        .filter(|l| !lines.effect.contains(*l) && lines.state.contains(*l))
        .count();
    println!(
        "{} of {} executed production PHY lines observed; {} unobserved reviewed, {} untriaged \
         ({effect} reach uncompared effects, {state} only final state, {} nothing)",
        lines.observed.len(),
        lines.executed.len(),
        reviewed.len(),
        unobserved.len(),
        unobserved.len() - effect - state,
    );
    for (name, seconds) in &elapsed {
        println!("{name} {seconds:.1}s");
    }
    // Decisions are shared by every scenario, so one is stale only when no
    // scenario's closures leave a location it excludes uncovered.
    coverage::Observed::of(&closures).check("all", coverage::DECISIONS)?;
    // A state decision is stale when no claim writes a byte it reviews
    // without comparing it.
    state::check(state::DECISIONS, &unprojected)?;
    let (mut reviewed_state, mut untriaged_state) = (0, std::collections::BTreeSet::new());
    for (root, production, byte) in &unprojected {
        let (reviewed, untriaged) = state::classify(
            state::DECISIONS,
            (root, production),
            &std::collections::BTreeSet::from([byte.clone()]),
        );
        reviewed_state += reviewed.len();
        untriaged_state.extend(untriaged);
    }
    let unprojected = untriaged_state;
    println!(
        "{reviewed_state} unprojected vendor state bytes reviewed, {} untriaged",
        unprojected.len()
    );
    if let Some(path) = index {
        let index = evidence::index(
            &common,
            &[&sdk, &phy_sdk, &rftest, &libpp],
            entries,
            coverage::uncovered_everywhere(&closures, untriaged)
                .into_iter()
                .collect(),
            unobserved
                .into_iter()
                .map(|(path, line)| evidence_index::SourceLine { path, line })
                .collect(),
            state::ranges(&unprojected)
                .into_iter()
                .map(|(symbol, offset, length)| evidence_index::StateRange {
                    symbol,
                    offset,
                    length,
                })
                .collect(),
        )?;
        let mut bytes = serde_json::to_vec_pretty(&index)?;
        bytes.push(b'\n');
        std::fs::write(&path, bytes)?;
        println!("evidence index {}", path.display());
    }
    println!("all PHY comparison scenarios passed");
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let result = match Cli::parse().scenario {
        Scenario::Gain { common, rftest } => single(gain(common, rftest)),
        Scenario::Channel { common } => single(channel(common)),
        Scenario::RxGain { common, phy_sdk } => single(rx_gain(common, phy_sdk)),
        Scenario::TxDc { common, phy_sdk } => single(tx_dc(common, phy_sdk)),
        Scenario::Tracking { common, phy_sdk } => single(tracking(common, phy_sdk)),
        Scenario::WifiMac { common, libpp } => single(wifi_mac(common, libpp)),
        Scenario::All {
            common,
            sdk,
            phy_sdk,
            rftest,
            libpp,
            index,
        } => all(common, sdk, phy_sdk, rftest, libpp, index),
        Scenario::Research {
            binary,
            library,
            rom,
            linker,
            output,
            budget,
        } => research::exercise(&research::Options {
            binary,
            library,
            rom,
            linker,
            output,
            budget,
        })
        .map(|run| {
            println!(
                "authenticated PHY research, review and source-free preservation passed {}",
                run.display()
            );
            ExitCode::SUCCESS
        }),
        Scenario::I2c {
            common,
            sdk,
            phy_sdk,
        } => single(i2c(common, sdk, phy_sdk)),
    };
    result.unwrap_or_else(|error| {
        eprintln!("error: {error}");
        ExitCode::FAILURE
    })
}
