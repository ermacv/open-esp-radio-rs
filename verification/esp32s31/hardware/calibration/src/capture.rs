//! Cold calibrations of the vendor firmware and the production image on the
//! leased board.
//!
//! Run under the stand lease (`cargo hil --owner <name> lease -- ...`). The
//! vendor firmware comes from `cargo xtask vendor-firmware`; the production
//! image from `cargo hil image build`. Each round flashes and boots the
//! vendor firmware, then the production image, so both sides see the same
//! board temperature. Each production boot receives a fresh startup artifact
//! path, so it calibrates fully and publishes the new retained calibration.
use crate::{Result, repository_root, vendor};
use oer_hil_runner_core::lab::config::LabConfig;
use oer_hil_runner_core::session::{SerialCapture, Settings, Target};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Record of one capture directory.
pub const RECORD: &str = "capture.json";
/// File name prefixes of the per-boot vendor consoles and production
/// artifacts.
pub const VENDOR_PREFIX: &str = "vendor-";
pub const PRODUCTION_PREFIX: &str = "production-";
/// Vendor firmware outputs, relative to the repository root.
const VENDOR_FIRMWARE: &str = "target/vendor-firmware/esp32s31";
/// Name the board journal records for vendor firmware images.
const JOURNAL_PREFIX: &str = "vendor-";
/// Longest wait for one vendor boot's closed report.
const VENDOR_BOOT_TIMEOUT: Duration = Duration::from_secs(20);
/// Serial read poll interval; the USB-Serial/JTAG console ignores the rate.
const READ_TIMEOUT: Duration = Duration::from_millis(100);
const BAUD_RATE: u32 = 115_200;
/// Console characters an unanswered register read reports.
const CONSOLE_TAIL_CHARS: usize = 2000;
/// Extensions of the per-boot vendor consoles and register replies.
pub const CONSOLE_EXTENSION: &str = "log";
pub const REGISTER_EXTENSION: &str = "registers";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Capture {
    pub started_unix_seconds: u64,
    pub vendor_application_sha256: String,
    pub vendor_idf_revision: String,
    pub production_image: String,
    pub production_application_sha256: String,
}

#[derive(clap::Args)]
pub struct Arguments {
    /// New directory for the captures.
    #[arg(long)]
    output: PathBuf,
    /// Cold boots per side, alternating vendor and production.
    #[arg(long, default_value_t = 10)]
    boots: u32,
    /// Vendor firmware project of `verification/esp32s31/hil-vendor`.
    #[arg(long, default_value = "calibration")]
    vendor_project: String,
    /// HIL image class whose cold calibration is compared.
    #[arg(long, default_value = "correctness")]
    production_image: String,
    #[arg(long)]
    lab_config: Option<PathBuf>,
}

#[derive(Deserialize)]
struct VendorBuild {
    idf_revision: String,
    application: PathBuf,
    application_sha256: String,
}

/// The application image and digest `cargo hil image build <class>` reports.
fn build_production(root: &Path, class: &str) -> Result<(PathBuf, String)> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args(["hil", "image", "build", class])
        .output()?;
    if !output.status.success() {
        return Err(format!("cargo hil image build {class} failed").into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    let report = stdout
        .rfind("\n{")
        .map_or(stdout.as_str(), |start| &stdout[start + 1..]);
    let report: serde_json::Value = serde_json::from_str(report)?;
    let field = |key: &str| {
        report[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("image build report lacks {key}"))
    };
    Ok((
        PathBuf::from(field("application_image")?),
        field("application_sha256")?,
    ))
}

/// Record a flash outside the HIL runner in the board journal.
fn journal(root: &Path, image: &str, application: &Path, port: &Path) -> Result<()> {
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args([
            "hil",
            "board",
            "flashed",
            "--flashed",
            image,
            "--application",
        ])
        .arg(application)
        .arg("--port")
        .arg(port)
        .status()?;
    if !status.success() {
        return Err(format!("recording the {image} flash in the board journal failed").into());
    }
    Ok(())
}

/// The console of one reset vendor boot, up to its closed report, and the
/// open port the firmware keeps answering on.
fn vendor_boot(port: &Path) -> Result<(String, Box<dyn serialport::SerialPort>)> {
    let mut serial = serialport::new(port.to_string_lossy(), BAUD_RATE)
        .timeout(READ_TIMEOUT)
        .open()?;
    oer_hil_runner_core::session::reset::reset_usb_serial_jtag(&mut *serial)?;
    let started = Instant::now();
    let mut console = Vec::new();
    let mut buffer = [0; 1024];
    while started.elapsed() < VENDOR_BOOT_TIMEOUT {
        match serial.read(&mut buffer) {
            Ok(read) => console.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
        let text = String::from_utf8_lossy(&console);
        if vendor::complete(&text) {
            return Ok((text.into_owned(), serial));
        }
    }
    Err(format!(
        "no closed vendor report within {VENDOR_BOOT_TIMEOUT:?}; console:\n{}",
        String::from_utf8_lossy(&console)
    )
    .into())
}

/// The vendor firmware's replies to reads of `registers`. A read that
/// resets the chip, such as a register whose clock domain the calibration
/// leaves off, shows as a new boot report instead of a reply: the register
/// is recorded as unreadable in this state and the reads continue once the
/// new boot reports.
fn vendor_registers(
    serial: &mut dyn serialport::SerialPort,
    registers: &[crate::registers::Register],
) -> Result<String> {
    let mut replies = String::new();
    let mut buffer = [0; 256];
    for register in registers {
        serial.write_all(vendor::register_request(register.address).as_bytes())?;
        let boots = vendor::reports(&replies);
        let started = Instant::now();
        loop {
            if vendor::registers(&replies)?.contains_key(&register.address) {
                break;
            }
            if vendor::reports(&replies) > boots {
                replies.push_str(&vendor::unreadable_line(register.address));
                break;
            }
            if started.elapsed() > VENDOR_BOOT_TIMEOUT {
                let tail = replies
                    .char_indices()
                    .rev()
                    .nth(CONSOLE_TAIL_CHARS)
                    .map_or(replies.as_str(), |(start, _)| &replies[start..]);
                return Err(format!(
                    "no reply for {} ({:#010x}); console tail:\n{tail}",
                    register.name, register.address
                )
                .into());
            }
            match serial.read(&mut buffer) {
                Ok(read) => replies.push_str(&String::from_utf8_lossy(&buffer[..read])),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(replies)
}

/// One cold production boot publishing its retained calibration to
/// `artifact`.
fn production_boot(lab: &LabConfig, artifact: &Path, output: &Path) -> Result<()> {
    let mut lab = lab.clone();
    lab.device.startup_artifact = Some(artifact.to_owned());
    let capture = SerialCapture::start_with_reset(&lab.device.serial, output)?;
    let result = capture.prepare_startup(Target {
        lab: &lab,
        settings: Settings::default(),
    });
    let (_, status) = capture.finish_with(result)?;
    let status = status.ok_or("the production image published no startup artifact")?;
    if status.disposition != oer_hil_protocol::StartupArtifactDisposition::Created {
        return Err(format!(
            "the production boot did not calibrate cold: {:?}",
            status.disposition
        )
        .into());
    }
    Ok(())
}

pub fn run(arguments: &Arguments) -> Result<std::process::ExitCode> {
    let root = repository_root().canonicalize()?;
    let lab_path = match &arguments.lab_config {
        Some(path) => path.clone(),
        None => LabConfig::default_path()?,
    };
    let lab = LabConfig::load(&lab_path)?;
    let port = lab.device.serial.clone();
    let vendor_build: VendorBuild = serde_json::from_slice(&std::fs::read(
        root.join(VENDOR_FIRMWARE)
            .join(&arguments.vendor_project)
            .join("build.json"),
    )?)?;
    let (production, production_sha256) = build_production(&root, &arguments.production_image)?;
    let registers = crate::registers::partition(&root, crate::registers::PARTITION)?;
    if let Some(parent) = arguments.output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&arguments.output)
        .map_err(|error| format!("{}: {error}", arguments.output.display()))?;
    let output = arguments.output.canonicalize()?;
    let started_unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let _lease = oer_esp32s31_firmware::device::DeviceLease::acquire(&port)?;

    // Vendor and production boots alternate, so both sides see the same
    // board temperature drift.
    let journal_name = format!("{JOURNAL_PREFIX}{}", arguments.vendor_project);
    for boot in 1..=arguments.boots {
        oer_hil_runner_core::device::flash_application(
            &root,
            &vendor_build.application,
            &output.join("flash-vendor"),
            &port,
        )?;
        journal(&root, &journal_name, &vendor_build.application, &port)?;
        let (console, mut serial) = vendor_boot(&port)?;
        let replies = vendor_registers(&mut *serial, &registers)?;
        drop(serial);
        let stem = format!("{VENDOR_PREFIX}{boot:02}");
        std::fs::write(output.join(format!("{stem}.{CONSOLE_EXTENSION}")), console)?;
        std::fs::write(output.join(format!("{stem}.{REGISTER_EXTENSION}")), replies)?;
        println!(
            "vendor boot {boot}: report and {} registers captured",
            registers.len()
        );

        oer_hil_runner_core::device::flash_application(
            &root,
            &production,
            &output.join("flash-production"),
            &port,
        )?;
        journal(&root, &arguments.production_image, &production, &port)?;
        production_boot(
            &lab,
            &output.join(format!("{PRODUCTION_PREFIX}{boot:02}.bin")),
            &output.join(format!("session-production-{boot:02}")),
        )?;
        println!("production boot {boot}: retained calibration captured");
    }

    let record = Capture {
        started_unix_seconds,
        vendor_application_sha256: vendor_build.application_sha256,
        vendor_idf_revision: vendor_build.idf_revision,
        production_image: arguments.production_image.clone(),
        production_application_sha256: production_sha256,
    };
    let mut bytes = serde_json::to_vec_pretty(&record)?;
    bytes.push(b'\n');
    std::fs::write(output.join(RECORD), bytes)?;
    Ok(std::process::ExitCode::SUCCESS)
}
