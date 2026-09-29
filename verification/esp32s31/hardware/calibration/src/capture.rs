//! Cold calibrations of the vendor firmware and the production image on the
//! leased board, with their register state read after the bring-up or after
//! one further Wi-Fi radio restart.
//!
//! Run under the stand lease (`cargo hil --owner <name> lease -- ...`). The
//! vendor firmware comes from `cargo xtask vendor-firmware`; the production
//! image from `cargo hil image build`. Each round flashes and boots the
//! vendor firmware, then the production image, so both sides see the same
//! board temperature. Each production boot receives a fresh startup artifact
//! path, so it calibrates fully and publishes the new retained calibration.
use crate::registers::{Register, Space};
use crate::{Result, repository_root, vendor};
use oer_hil_runner_core::device::{EraseCalibration, Slot, booted_slot};
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
/// The raw console a production session capture keeps.
const SESSION_CONSOLE: &str = "uart.bin";
/// The two-slot layout's slots of the vendor firmware and the production
/// image: both are written once and a round selects one, so the rounds boot
/// them without flashing.
const VENDOR_SLOT: Slot = Slot::Ota0;
const PRODUCTION_SLOT: Slot = Slot::Ota1;
/// Bootstrap ELF the HIL image build leaves beside the application.
const BOOTSTRAP_ELF: &str = "bootstrap.elf";
/// Longest wait for one Wi-Fi radio restart on either side.
const RESTART_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest wait for one production register image window.
const REGISTER_IMAGE_TIMEOUT: Duration = Duration::from_secs(2);
/// Console characters an unanswered register read reports.
const CONSOLE_TAIL_CHARS: usize = 2000;
/// Extensions of the per-boot vendor consoles and register replies.
pub const CONSOLE_EXTENSION: &str = "log";
pub const REGISTER_EXTENSION: &str = "registers";
/// Vendor window reads, and the qualifier of the reads after a transmission.
const WINDOW_EXTENSION: &str = "windows";
const TRANSMITTED: &str = "transmitted";
pub const ANALOG_EXTENSION: &str = "analog";

/// Lifecycle point at which both sides report their state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Lifecycle {
    /// After the cold calibration and the Wi-Fi bring-up.
    #[default]
    Cold,
    /// After one further Wi-Fi radio restart: RF closed and woken.
    Restart,
    /// After the IEEE 802.15.4 radio is configured and receives, before
    /// any transmission.
    Ieee802154,
    /// As `Ieee802154`, after one IEEE 802.15.4 disable and enable (the
    /// vendor driver's OFF and ON, production's session stop and start).
    Ieee802154Restart,
}

impl Lifecycle {
    /// Whether the IEEE 802.15.4 radio, rather than Wi-Fi, holds the PHY.
    pub fn ieee802154(self) -> bool {
        matches!(self, Self::Ieee802154 | Self::Ieee802154Restart)
    }

    /// Whether the point restarts the radio that holds the PHY.
    fn restarts(self) -> bool {
        matches!(self, Self::Restart | Self::Ieee802154Restart)
    }

    /// The point's name, also its directory in a capture of several points.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cold => "cold",
            Self::Restart => "restart",
            Self::Ieee802154 => "ieee802154",
            Self::Ieee802154Restart => "ieee802154-restart",
        }
    }

    /// The vendor firmware project and HIL image class of the point.
    fn images(self) -> (&'static str, &'static str) {
        if self.ieee802154() {
            (IEEE802154_VENDOR_PROJECT, IEEE802154_PRODUCTION_IMAGE)
        } else {
            (CALIBRATION_VENDOR_PROJECT, CALIBRATION_PRODUCTION_IMAGE)
        }
    }
}

/// The points one capture reads in each boot, in order: one point, or a
/// radio's bring-up followed by its restart in the same boot.
fn boot_points(requested: &[Lifecycle]) -> Result<Vec<Lifecycle>> {
    match requested {
        [point] => Ok(vec![*point]),
        [Lifecycle::Cold, Lifecycle::Restart]
        | [Lifecycle::Ieee802154, Lifecycle::Ieee802154Restart] => Ok(requested.to_vec()),
        _ => Err(format!(
            "lifecycle points {requested:?}: one point, cold,restart or ieee802154,ieee802154-restart"
        )
        .into()),
    }
}

/// The capture directories under `captures`: itself when it records one
/// point, otherwise its point directories in lifecycle order.
pub fn point_directories(captures: &Path) -> Result<Vec<PathBuf>> {
    if captures.join(RECORD).is_file() {
        return Ok(vec![captures.to_owned()]);
    }
    let directories: Vec<PathBuf> = [
        Lifecycle::Cold,
        Lifecycle::Restart,
        Lifecycle::Ieee802154,
        Lifecycle::Ieee802154Restart,
    ]
    .into_iter()
    .map(|point| captures.join(point.name()))
    .filter(|directory| directory.join(RECORD).is_file())
    .collect();
    if directories.is_empty() {
        return Err(format!("{} holds no capture", captures.display()).into());
    }
    Ok(directories)
}

/// Vendor firmware projects of `verification/esp32s31/hil-vendor` and HIL
/// image classes of the Wi-Fi and IEEE 802.15.4 points.
const CALIBRATION_VENDOR_PROJECT: &str = "calibration";
const CALIBRATION_PRODUCTION_IMAGE: &str = "correctness";
const IEEE802154_VENDOR_PROJECT: &str = "ieee802154-reference";
const IEEE802154_PRODUCTION_IMAGE: &str = "diagnostic-ieee802154-radio";
/// Longest wait for one IEEE 802.15.4 session command of production.
const SESSION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Capture {
    pub started_unix_seconds: u64,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    pub vendor_application_sha256: String,
    pub vendor_idf_revision: String,
    /// The production image class and digest; absent for a vendor-only
    /// capture.
    #[serde(default)]
    pub production_image: Option<String>,
    #[serde(default)]
    pub production_application_sha256: Option<String>,
}

#[derive(clap::Args)]
pub struct Arguments {
    /// New directory for the captures.
    #[arg(long)]
    output: PathBuf,
    /// Cold boots per side, alternating vendor and production.
    #[arg(long, default_value_t = 10)]
    boots: u32,
    /// Vendor firmware project of `verification/esp32s31/hil-vendor`; the
    /// lifecycle point's own by default.
    #[arg(long)]
    vendor_project: Option<String>,
    /// HIL image class whose calibration is compared; the lifecycle point's
    /// own by default.
    #[arg(long)]
    production_image: Option<String>,
    /// Lifecycle points at which the registers are read: one point, or
    /// `cold,restart` and `ieee802154,ieee802154-restart`, which read both
    /// points in each boot into one directory per point.
    #[arg(long, value_enum, value_delimiter = ',', default_value = "cold")]
    lifecycle: Vec<Lifecycle>,
    /// Further device windows the vendor reference firmware reads with
    /// `PEEK` at an IEEE 802.15.4 point, as `ADDRESS:WORDS` (hexadecimal
    /// address, decimal word count), recorded in `vendor-NN.windows`.
    /// Production publishes only its register partition, so the windows are
    /// a vendor-side investigation, not a comparison.
    #[arg(long = "vendor-window", value_parser = parse_window)]
    vendor_windows: Vec<(u32, u32)>,
    /// After the reads, transmit one frame on the vendor side and read its
    /// analog image and windows again (`vendor-NN.transmitted.*`).
    #[arg(long)]
    vendor_transmit: bool,
    /// Boot only the vendor firmware.
    #[arg(long)]
    vendor_only: bool,
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

/// The vendor firmware of `project`, built first when this checkout has
/// none yet (a new worktree shares the IDF tree but not its builds).
fn vendor_build(root: &Path, project: &str) -> Result<VendorBuild> {
    let report = root.join(VENDOR_FIRMWARE).join(project).join("build.json");
    if let Some(build) = existing_vendor_build(&report) {
        return Ok(build);
    }
    let command = format!("cargo xtask vendor-firmware --chip esp32s31 {project}");
    eprintln!("vendor firmware {project} is not built in this checkout; running `{command}`");
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args(["xtask", "vendor-firmware", "--chip", "esp32s31", project])
        .status()?;
    if !status.success() {
        return Err(format!("`{command}` failed; run it and retry the capture").into());
    }
    existing_vendor_build(&report)
        .ok_or_else(|| format!("`{command}` left no application for {project}").into())
}

/// The build `report` describes, when it and its application image exist.
fn existing_vendor_build(report: &Path) -> Option<VendorBuild> {
    let build: VendorBuild = serde_json::from_slice(&std::fs::read(report).ok()?).ok()?;
    build.application.is_file().then_some(build)
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

/// The console port of the board, just reset. The port is opened through
/// the stand's opener, which never resets the chip on its own.
fn reset_console(port: &Path) -> Result<Box<dyn serialport::SerialPort>> {
    use oer_hil_runner_core::session::reset::{open_without_reset, reset_usb_serial_jtag};
    let mut serial = open_without_reset(port)?;
    serial.set_timeout(READ_TIMEOUT)?;
    reset_usb_serial_jtag(&mut *serial)?;
    Ok(serial)
}

/// The console of one reset vendor boot, up to its closed report, and the
/// open port the firmware keeps answering on.
fn vendor_boot(port: &Path) -> Result<(String, Box<dyn serialport::SerialPort>)> {
    let mut serial = reset_console(port)?;
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

/// Restart the vendor Wi-Fi radio and require that stopping the client
/// released every PHY modem, which closes RF. `replies` accumulates the
/// console.
fn vendor_restart(serial: &mut dyn serialport::SerialPort, replies: &mut String) -> Result<()> {
    let done = vendor::restarts(replies)?.len();
    serial.write_all(vendor::RESTART_REQUEST.as_bytes())?;
    let started = Instant::now();
    let mut buffer = [0; 256];
    loop {
        if let Some(&flags) = vendor::restarts(replies)?.get(done) {
            if flags != 0 {
                return Err(format!(
                    "the stopped vendor Wi-Fi client left PHY modem flags {flags:#x}, so RF stayed open"
                )
                .into());
            }
            return Ok(());
        }
        if started.elapsed() > RESTART_TIMEOUT {
            return Err(format!("no vendor restart reply within {RESTART_TIMEOUT:?}").into());
        }
        match serial.read(&mut buffer) {
            Ok(read) => replies.push_str(&String::from_utf8_lossy(&buffer[..read])),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
    }
}

/// The vendor firmware's replies to reads of `registers`. A read that
/// resets the chip, such as a register whose clock domain the calibration
/// leaves off, shows as a new boot report instead of a reply: the register
/// is recorded as unreadable in this state and the reads continue once the
/// new boot reports, after restarting the radio again at the
/// [`Lifecycle::Restart`] point. `replies` holds the console so far, and
/// its restarts.
fn vendor_registers(
    serial: &mut dyn serialport::SerialPort,
    space: Space,
    registers: &[crate::registers::Register],
    lifecycle: Lifecycle,
    replies: &mut String,
) -> Result<()> {
    let mut buffer = [0; 256];
    for register in registers {
        serial.write_all(space.request(register.address).as_bytes())?;
        let boots = vendor::reports(replies);
        let started = Instant::now();
        loop {
            if space.replies(replies)?.contains_key(&register.address) {
                break;
            }
            if vendor::reports(replies) > boots {
                replies.push_str(&vendor::unreadable_line(register.address));
                if lifecycle == Lifecycle::Restart {
                    vendor_restart(serial, replies)?;
                }
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
    Ok(())
}

/// Restart the radio that holds production's PHY at a restarting `point`
/// and require that RF was closed.
fn production_restart(capture: &SerialCapture, point: Lifecycle) -> Result<()> {
    if point.ieee802154() {
        // The session stops and starts its radio client, then configures
        // and receives again without transmitting.
        let restarted = capture.restart_ieee802154_session_radio(SESSION_TIMEOUT)?;
        if !restarted.rf_closed {
            return Err(
                format!("the production session restart kept RF open: {restarted:?}").into(),
            );
        }
    } else {
        let evidence =
            capture.wait_wifi_radio_restart(capture.request_radio_restart()?, RESTART_TIMEOUT)?;
        if evidence.rf != oer_hil_protocol::wifi::WifiRadioRestartRf::ClosedAndWoken {
            return Err(format!("the production restart kept RF open: {evidence:?}").into());
        }
    }
    Ok(())
}

/// Production's register replies at the `readable` image indices and its
/// whole analog image, in the vendor line formats.
fn production_registers(
    capture: &SerialCapture,
    registers: &[Register],
    readable: &[usize],
    analog: &[Register],
) -> Result<(String, String)> {
    let mut replies = String::new();
    for window in windows(readable) {
        let words = capture.read_phy_register_image(window, REGISTER_IMAGE_TIMEOUT)?;
        for (offset, value) in words.values.iter().enumerate() {
            let register = &registers[usize::from(words.first) + offset];
            replies.push_str(&Space::Mmio.line(register.address, *value));
        }
    }
    let mut analog_replies = String::new();
    let every: Vec<usize> = (0..analog.len()).collect();
    for window in windows(&every) {
        let bytes = capture.read_phy_analog_image(window, REGISTER_IMAGE_TIMEOUT)?;
        if usize::from(bytes.length) != analog.len() {
            return Err(format!(
                "production reads {} analog registers, the published model {}",
                bytes.length,
                analog.len()
            )
            .into());
        }
        for (offset, value) in bytes.values.iter().enumerate() {
            let register = &analog[usize::from(bytes.first) + offset];
            analog_replies.push_str(&Space::Analog.line(register.address, u32::from(*value)));
        }
    }
    Ok((replies, analog_replies))
}

/// One cold production boot publishing its retained calibration to
/// `artifact`, then, at each of `points` in turn, reading the register image
/// at that point's `readable` indices and the whole analog image; the
/// replies use the vendor line formats. The boot's console must show the
/// bootloader loading `slot`.
#[allow(clippy::too_many_arguments)]
fn production_boot(
    lab: &LabConfig,
    artifact: &Path,
    output: &Path,
    registers: &[Register],
    readable: &[Vec<usize>],
    analog: &[Register],
    points: &[Lifecycle],
    slot: Slot,
) -> Result<Vec<(String, String)>> {
    let mut lab = lab.clone();
    lab.dut.startup_artifact = Some(artifact.to_owned());
    let bring_up = points[0];
    let capture = SerialCapture::start_with_reset(&lab.dut.serial, output)?;
    let result = (|| {
        // The IEEE 802.15.4 session replaces the image's own initialization:
        // it is admitted only before it, and publishes no startup artifact.
        let status = if bring_up.ieee802154() {
            capture.request_image_keys(SESSION_TIMEOUT)?;
            None
        } else {
            let (_, status) = capture.prepare_startup(Target {
                lab: &lab,
                settings: Settings::default(),
            })?;
            status
        };
        if bring_up.ieee802154() {
            ieee802154_session(&capture)?;
        }
        let mut replies = Vec::with_capacity(points.len());
        for (point, readable) in points.iter().zip(readable) {
            if point.restarts() {
                production_restart(&capture, *point)?;
            }
            replies.push(production_registers(&capture, registers, readable, analog)?);
        }
        Ok((status, replies))
    })();
    let (status, replies) = capture.finish_with(result)?;
    let console = std::fs::read(output.join(SESSION_CONSOLE))?;
    if booted_slot(&console) != Some(slot) {
        return Err(format!(
            "the production boot did not load slot {slot:?}; its console is {}",
            output.join(SESSION_CONSOLE).display()
        )
        .into());
    }
    if bring_up.ieee802154() {
        return Ok(replies);
    }
    let status = status.ok_or("the production image published no startup artifact")?;
    if status.disposition != oer_hil_protocol::phy::StartupArtifactDisposition::Created {
        return Err(format!(
            "the production boot did not calibrate cold: {:?}",
            status.disposition
        )
        .into());
    }
    Ok(replies)
}

/// Start production's IEEE 802.15.4 session on the reference firmware's
/// channel and receive; the session's default PIB transmits at the same
/// power.
fn ieee802154_session(capture: &SerialCapture) -> Result<()> {
    capture.start_ieee802154_session(
        oer_hil_protocol::ieee802154::Ieee802154SessionConfig {
            channel: crate::peer::CHANNEL,
            pan_id: crate::peer::PAN_ID,
            short_address: crate::peer::SHORT_ADDRESS,
            extended_address: crate::peer::EXTENDED_ADDRESS,
            promiscuous: false,
            maintenance_policy:
                oer_hil_protocol::ieee802154::Ieee802154SessionMaintenancePolicy::Vendor,
            background_maintenance: false,
            enhanced_ack: false,
            wifi_coexistence: false,
        },
        SESSION_TIMEOUT,
    )?;
    capture.receive_ieee802154_session()
}

/// Requests covering the ascending `indices` in runs of at most one reply.
fn windows(indices: &[usize]) -> Vec<oer_hil_protocol::phy::PhyRegisterImageRequest> {
    let mut windows: Vec<oer_hil_protocol::phy::PhyRegisterImageRequest> = vec![];
    for &index in indices {
        match windows.last_mut() {
            Some(window)
                if usize::from(window.first) + usize::from(window.count) == index
                    && usize::from(window.count)
                        < oer_hil_protocol::phy::PHY_REGISTER_IMAGE_WORDS =>
            {
                window.count += 1
            }
            _ => windows.push(oer_hil_protocol::phy::PhyRegisterImageRequest {
                first: index as u16,
                count: 1,
            }),
        }
    }
    windows
}

/// `ADDRESS:WORDS` of one vendor window.
fn parse_window(text: &str) -> std::result::Result<(u32, u32), String> {
    let (address, words) = text
        .split_once(':')
        .ok_or_else(|| format!("{text}: expected ADDRESS:WORDS"))?;
    let address = u32::from_str_radix(address.trim_start_matches("0x"), 16)
        .map_err(|error| format!("{address}: {error}"))?;
    let words: u32 = words.parse().map_err(|error| format!("{words}: {error}"))?;
    if address % 4 != 0 || words == 0 {
        return Err(format!(
            "{text}: a word-aligned address and at least one word"
        ));
    }
    Ok((address, words))
}

/// The registers of vendor `windows`, one per word.
fn window_registers(windows: &[(u32, u32)]) -> Vec<Register> {
    windows
        .iter()
        .flat_map(|&(address, words)| (0..words).map(move |word| address + 4 * word))
        .map(|address| Register {
            name: format!("window.{address:08x}"),
            address,
        })
        .collect()
}

/// The vendor calibration firmware's boot console, from the reset, and its
/// register and analog replies at each of `points` in turn: the Wi-Fi
/// bring-up, then its restart in the same boot.
fn vendor_wifi_boot(
    port: &Path,
    points: &[Lifecycle],
    registers: &[Register],
    analog: &[Register],
) -> Result<(String, Vec<(String, String)>)> {
    let (console, mut serial) = vendor_boot(port)?;
    let mut replies = Vec::with_capacity(points.len());
    for &point in points {
        let mut register_replies = String::new();
        if point.restarts() {
            vendor_restart(&mut *serial, &mut register_replies)?;
        }
        vendor_registers(
            &mut *serial,
            Space::Mmio,
            registers,
            point,
            &mut register_replies,
        )?;
        let mut analog_replies = String::new();
        vendor_registers(
            &mut *serial,
            Space::Analog,
            analog,
            point,
            &mut analog_replies,
        )?;
        replies.push((register_replies, analog_replies));
    }
    Ok((console, replies))
}

pub fn run(arguments: &Arguments) -> Result<std::process::ExitCode> {
    let root = repository_root().canonicalize()?;
    let points = boot_points(&arguments.lifecycle)?;
    let bring_up = points[0];
    let investigates = !arguments.vendor_windows.is_empty() || arguments.vendor_transmit;
    if investigates && !(bring_up.ieee802154() && points.len() == 1) {
        return Err("vendor windows and transmission need one IEEE 802.15.4 point".into());
    }
    let windows = window_registers(&arguments.vendor_windows);
    let lab_path = match &arguments.lab_config {
        Some(path) => path.clone(),
        None => LabConfig::default_path()?,
    };
    let lab = LabConfig::load(&lab_path, "esp32s31")?;
    let port = lab.dut.serial.clone();
    let (vendor_project, production_image) = bring_up.images();
    let vendor_project = arguments
        .vendor_project
        .as_deref()
        .unwrap_or(vendor_project);
    let production_image = arguments
        .production_image
        .as_deref()
        .unwrap_or(production_image);
    let vendor_build = vendor_build(&root, vendor_project)?;
    let built = if arguments.vendor_only {
        None
    } else {
        Some(build_production(&root, production_image)?)
    };
    // The image's bootstrap ELF, from which the runner writes the HIL
    // bootloader with the production slot.
    let production_bootstrap = built
        .as_ref()
        .map(|(production, _)| production.with_file_name(BOOTSTRAP_ELF))
        .filter(|path| path.is_file());
    let registers = crate::registers::partition(&root, crate::registers::PARTITION)?;
    let analog = crate::registers::analog(&root, crate::registers::ANALOG_DOMAIN)?;
    if let Some(parent) = arguments.output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&arguments.output)
        .map_err(|error| format!("{}: {error}", arguments.output.display()))?;
    let output = arguments.output.canonicalize()?;
    // A boot that reads several points writes one capture directory each.
    let directories: Vec<PathBuf> = if points.len() == 1 {
        vec![output.clone()]
    } else {
        points
            .iter()
            .map(|point| output.join(point.name()))
            .collect()
    };
    for directory in &directories {
        std::fs::create_dir_all(directory)?;
    }
    let started_unix_seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let _lease = oer_esp32s31_firmware::device::DeviceLease::acquire(&port)?;

    // Both firmwares are written once, each into its slot of the two-slot
    // layout. Each round selects a slot and erases the calibration the other
    // firmware left, so every boot calibrates cold without a flash.
    let production_slot = match &built {
        Some((production, _)) => Some(oer_hil_runner_core::device::flash_slot(
            &root,
            production,
            production_bootstrap.as_deref(),
            PRODUCTION_SLOT,
            &output.join("flash-production"),
            &port,
        )?),
        None => None,
    };
    let vendor_slot = oer_hil_runner_core::device::flash_slot(
        &root,
        &vendor_build.application,
        // The vendor image keeps the board's HIL bootloader.
        None,
        VENDOR_SLOT,
        &output.join("flash-vendor"),
        &port,
    )?;

    // Vendor and production boots alternate, so both sides see the same
    // board temperature drift.
    let journal_name = format!("{JOURNAL_PREFIX}{vendor_project}");
    for boot in 1..=arguments.boots {
        oer_hil_runner_core::device::select_slot(&port, &vendor_slot, EraseCalibration::Yes)?;
        journal(&root, &journal_name, &vendor_build.application, &port)?;
        let stem = format!("{VENDOR_PREFIX}{boot:02}");
        let (console, replies) = if bring_up.ieee802154() {
            let mut peer = crate::peer::Peer::boot(reset_console(&port)?, bring_up.restarts())?;
            let mut replies = Vec::with_capacity(points.len());
            for index in 0..points.len() {
                if index > 0 {
                    peer.restart()?;
                }
                replies.push((
                    peer.registers(Space::Mmio, &registers)?,
                    peer.registers(Space::Analog, &analog)?,
                ));
            }
            let stem = directories[0].join(&stem);
            if !windows.is_empty() {
                let lines = peer.registers(Space::Mmio, &windows)?;
                std::fs::write(stem.with_extension(WINDOW_EXTENSION), lines)?;
            }
            if arguments.vendor_transmit {
                peer.transmit()?;
                let analog = peer.registers(Space::Analog, &analog)?;
                std::fs::write(
                    stem.with_extension(format!("{TRANSMITTED}.{ANALOG_EXTENSION}")),
                    analog,
                )?;
                if !windows.is_empty() {
                    let lines = peer.registers(Space::Mmio, &windows)?;
                    std::fs::write(
                        stem.with_extension(format!("{TRANSMITTED}.{WINDOW_EXTENSION}")),
                        lines,
                    )?;
                }
            }
            (peer.console, replies)
        } else {
            vendor_wifi_boot(&port, &points, &registers, &analog)?
        };
        if booted_slot(console.as_bytes()) != Some(VENDOR_SLOT) {
            return Err(format!("vendor boot {boot} did not load slot {VENDOR_SLOT:?}").into());
        }
        for (directory, (register_replies, analog_replies)) in directories.iter().zip(&replies) {
            std::fs::write(
                directory.join(format!("{stem}.{CONSOLE_EXTENSION}")),
                &console,
            )?;
            std::fs::write(
                directory.join(format!("{stem}.{REGISTER_EXTENSION}")),
                register_replies,
            )?;
            std::fs::write(
                directory.join(format!("{stem}.{ANALOG_EXTENSION}")),
                analog_replies,
            )?;
        }
        println!(
            "vendor boot {boot}: report, {} registers and {} analog registers captured at {} point(s)",
            registers.len(),
            analog.len(),
            points.len()
        );
        let (Some((production, _)), Some(production_slot)) = (&built, &production_slot) else {
            continue;
        };

        oer_hil_runner_core::device::select_slot(&port, production_slot, EraseCalibration::Yes)?;
        journal(&root, production_image, production, &port)?;
        let readable = replies
            .iter()
            .map(|(register_replies, _)| {
                let answered = vendor::registers(register_replies)?;
                Ok(registers
                    .iter()
                    .enumerate()
                    .filter(|(_, register)| answered.contains_key(&register.address))
                    .map(|(index, _)| index)
                    .collect())
            })
            .collect::<Result<Vec<Vec<usize>>>>()?;
        let artifact_name = format!("{PRODUCTION_PREFIX}{boot:02}.bin");
        let artifact = directories[0].join(&artifact_name);
        let production_replies = production_boot(
            &lab,
            &artifact,
            &output.join(format!("session-production-{boot:02}")),
            &registers,
            &readable,
            &analog,
            &points,
            PRODUCTION_SLOT,
        )?;
        for (index, (directory, (register_replies, analog_replies))) in
            directories.iter().zip(&production_replies).enumerate()
        {
            // Every point of the boot compares the calibration it made.
            if index > 0 && artifact.is_file() {
                std::fs::copy(&artifact, directory.join(&artifact_name))?;
            }
            std::fs::write(
                directory.join(format!("{PRODUCTION_PREFIX}{boot:02}.{REGISTER_EXTENSION}")),
                register_replies,
            )?;
            std::fs::write(
                directory.join(format!("{PRODUCTION_PREFIX}{boot:02}.{ANALOG_EXTENSION}")),
                analog_replies,
            )?;
        }
        println!(
            "production boot {boot}: retained calibration and {} registers captured at {} point(s)",
            readable[0].len(),
            points.len()
        );
    }

    for (directory, point) in directories.iter().zip(&points) {
        let record = Capture {
            started_unix_seconds,
            lifecycle: *point,
            vendor_application_sha256: vendor_build.application_sha256.clone(),
            vendor_idf_revision: vendor_build.idf_revision.clone(),
            production_image: built.as_ref().map(|_| production_image.to_owned()),
            production_application_sha256: built.as_ref().map(|(_, sha256)| sha256.clone()),
        };
        let mut bytes = serde_json::to_vec_pretty(&record)?;
        bytes.push(b'\n');
        std::fs::write(directory.join(RECORD), bytes)?;
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vendor_build_counts_only_with_its_report_and_application() {
        let directory =
            std::env::temp_dir().join(format!("oer-vendor-build-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let report = directory.join("build.json");
        let application = directory.join("app.bin");

        assert!(existing_vendor_build(&report).is_none());
        std::fs::write(
            &report,
            serde_json::json!({
                "idf_revision": "rev",
                "application": application,
                "application_sha256": "00",
            })
            .to_string(),
        )
        .unwrap();
        assert!(existing_vendor_build(&report).is_none());
        std::fs::write(&application, b"image").unwrap();
        assert!(existing_vendor_build(&report).is_some());

        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn vendor_windows_parse_into_word_registers() {
        assert_eq!(parse_window("2010fc00:2"), Ok((0x2010_fc00, 2)));
        assert!(parse_window("2010fc02:2").is_err());
        assert!(parse_window("2010fc00").is_err());
        let registers = window_registers(&[(0x2010_fc00, 2), (0x2010_08b8, 1)]);
        let addresses: Vec<u32> = registers.iter().map(|r| r.address).collect();
        assert_eq!(addresses, [0x2010_fc00, 0x2010_fc04, 0x2010_08b8]);
    }

    #[test]
    fn a_boot_reads_one_point_or_a_bring_up_and_its_restart() {
        use Lifecycle::*;
        assert_eq!(boot_points(&[Restart]).unwrap(), [Restart]);
        assert_eq!(boot_points(&[Cold, Restart]).unwrap(), [Cold, Restart]);
        assert_eq!(
            boot_points(&[Ieee802154, Ieee802154Restart]).unwrap(),
            [Ieee802154, Ieee802154Restart]
        );
        assert!(boot_points(&[Restart, Cold]).is_err());
        assert!(boot_points(&[Cold, Ieee802154Restart]).is_err());
        assert!(boot_points(&[]).is_err());
    }

    #[test]
    fn a_capture_of_several_points_compares_each_point_directory() {
        let directory =
            std::env::temp_dir().join(format!("oer-capture-points-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        assert!(point_directories(&directory).is_err());
        for point in [Lifecycle::Restart, Lifecycle::Cold] {
            std::fs::create_dir_all(directory.join(point.name())).unwrap();
            std::fs::write(directory.join(point.name()).join(RECORD), "{}").unwrap();
        }
        assert_eq!(
            point_directories(&directory).unwrap(),
            [directory.join("cold"), directory.join("restart")]
        );
        std::fs::write(directory.join(RECORD), "{}").unwrap();
        assert_eq!(
            point_directories(&directory).unwrap(),
            std::slice::from_ref(&directory)
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn readable_indices_group_into_reply_windows() {
        let indices: Vec<usize> = (0..20).chain([30, 31]).collect();
        let windows: Vec<_> = windows(&indices)
            .iter()
            .map(|w| (w.first, w.count))
            .collect();
        assert_eq!(windows, [(0, 16), (16, 4), (30, 2)]);
    }
}
