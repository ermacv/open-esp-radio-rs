//! The one flash writer: an image bundle's [`Snapshot`] (its flash contents
//! read once into owned bytes) through one espflash connection to the ROM
//! stub, for the chip its profile names.
//!
//! Each segment whose flash contents already match is skipped after an MD5
//! comparison, so an unchanged bootloader or partition table costs one
//! checksum. Segments are written in the bundle's order, which puts the OTA
//! selection last: an interrupted write leaves the previous selection
//! pointing at an image whose checksum no longer validates instead of a
//! half-written one. A failure of the serial link is retried; a failure of
//! the image is not.
#![forbid(unsafe_code)]

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use std::path::Path;

use oer_image_bundle::Snapshot;

/// Where the writer leaves the chip after the last segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum After {
    /// Reset into the written image.
    HardReset,
    /// Stay in the ROM's download mode.
    StayInBootloader,
}

impl After {
    /// What a write followed by `start` leaves to the start.
    pub const fn of(start: oer_chip_profile::Start) -> Self {
        match start {
            oer_chip_profile::Start::Reset => Self::HardReset,
            oer_chip_profile::Start::PowerOn => Self::StayInBootloader,
        }
    }
}

/// Write the bytes of `snapshot` into the flash of the `chip` (its espflash
/// name) board at `port` through one connection, retrying a failed link.
pub fn write(port: &Path, chip: &str, snapshot: &Snapshot, after: After) -> crate::Result<()> {
    let chip = espflash_chip(chip)?;
    with_retries(port, || {
        write_once(port, chip, snapshot, after)
            .map_err(|error| format!("flash through {}: {error}", port.display()).into())
    })
}

/// The espflash name of the chip at `port`, from its ROM; the chip is reset
/// into its application afterwards.
pub fn detect(port: &Path) -> crate::Result<String> {
    with_retries(port, || {
        let mut flasher = connect(port, After::HardReset, None)?;
        let chip = flasher.chip();
        flasher.connection().reset_after(true, chip)?;
        Ok(chip.to_string())
    })
}

fn connect(
    port: &Path,
    after: After,
    chip: Option<espflash::target::Chip>,
) -> crate::Result<espflash::flasher::Flasher> {
    use espflash::{
        connection::{Connection, ResetAfterOperation, ResetBeforeOperation},
        flasher::Flasher,
    };
    use serialport::SerialPortType;

    let port_name = port.to_string_lossy().into_owned();
    let usb = serialport::available_ports()?
        .into_iter()
        .find(|info| {
            info.port_name == port_name
                || std::fs::canonicalize(&info.port_name).ok() == std::fs::canonicalize(port).ok()
        })
        .and_then(|info| match info.port_type {
            SerialPortType::UsbPort(usb) => Some(usb),
            _ => None,
        })
        .ok_or_else(|| format!("{port_name} is not an attached USB serial port"))?;
    let serial = oer_device_port::Port::open(
        port,
        oer_device_port::Settings::CONSOLE
            .lines(oer_device_port::Lines::Kept)
            .timeout(std::time::Duration::ZERO),
    )?
    .into_native();
    let connection = Connection::new(
        serial,
        usb,
        match after {
            After::HardReset => ResetAfterOperation::HardReset,
            After::StayInBootloader => ResetAfterOperation::NoReset,
        },
        ResetBeforeOperation::DefaultReset,
        115_200,
    );
    Ok(Flasher::connect(connection, true, false, true, chip, None)?)
}

fn write_once(
    port: &Path,
    chip: espflash::target::Chip,
    snapshot: &Snapshot,
    after: After,
) -> crate::Result<()> {
    use espflash::{image_format::Segment as Region, target::DefaultProgressCallback};

    let mut flasher = connect(port, after, Some(chip))?;
    let regions = snapshot
        .segments
        .iter()
        .map(|segment| Region::new(segment.offset, &segment.data))
        .collect::<Vec<_>>();
    flasher.write_bins_to_flash(&regions, &mut DefaultProgressCallback)?;
    flasher.connection().reset_after(true, chip)?;
    Ok(())
}

/// espflash's chip named `name` (a chip profile's `espflash-chip`).
pub fn espflash_chip(name: &str) -> crate::Result<espflash::target::Chip> {
    <espflash::target::Chip as std::str::FromStr>::from_str(name)
        .map_err(|_| format!("espflash knows no chip `{name}`").into())
}

/// How many times a write is attempted when espflash's link to the chip
/// fails on its way.
pub const ATTEMPTS: usize = 3;

/// Run `write` again when espflash's link to the chip failed on its way;
/// each retry is reported on stderr, which a job's log keeps.
fn with_retries<T>(port: &Path, mut write: impl FnMut() -> crate::Result<T>) -> crate::Result<T> {
    retry_transient(ATTEMPTS, &mut write, |attempt, error| {
        eprintln!(
            "flash attempt {attempt} of {ATTEMPTS} through {} failed: {error}; retrying",
            port.display()
        );
    })
}

fn retry_transient<T>(
    attempts: usize,
    operation: &mut impl FnMut() -> crate::Result<T>,
    mut between: impl FnMut(usize, &str),
) -> crate::Result<T> {
    let mut attempt = 1;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < attempts && transient_failure(&error.to_string()) => {
                between(attempt, &error.to_string());
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Whether a flash failure lies in the serial link to the chip, which a new
/// connection can clear, rather than in the image.
fn transient_failure(message: &str) -> bool {
    const LINK_FAILURES: [&str; 6] = [
        "Protocol error",
        "timed out",
        "Timeout",
        "Broken pipe",
        "Input/output error",
        "Failed to connect",
    ];
    LINK_FAILURES
        .iter()
        .any(|failure| message.contains(failure))
}

#[cfg(test)]
mod tests;
