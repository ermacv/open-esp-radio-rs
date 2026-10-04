//! Board support: how the stand writes an image into one chip's flash,
//! starts it and resets it.
//!
//! A chip's boot flow decides what an image adds to its application and how
//! the flash is written; [`Board`] is that flow. The ESP-IDF bootloader flow,
//! [`EspIdf`], is driven by the chip profile's flash layout alone; a chip
//! whose ROM loads its own platform bootstrap implements [`Board`] in its own
//! board-support package. The composition that knows the chips chooses the
//! implementation from the profile's `boot`. [`reset`] holds the console
//! and reset primitives every board shares.

use std::path::{Path, PathBuf};

mod esp_idf;
pub mod reset;

pub use esp_idf::EspIdf;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// What an image adds to its application for the chip's boot flow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Companions {
    /// The ROM loads the platform bootstrap, whose ELF the flow encodes
    /// into the bootloader region; without one the board keeps its current
    /// bootloader.
    Staged { bootstrap_elf: Option<PathBuf> },
    /// The chip's ESP-IDF bootloader loads the application from its
    /// partition.
    EspIdf {
        bootloader: PathBuf,
        partition_table: PathBuf,
    },
}

/// An image as the board writes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlashImage {
    pub application: PathBuf,
    pub companions: Companions,
    /// Where the flow writes the flash contents it encodes.
    pub work: PathBuf,
}

/// One chip's boot flow.
pub trait Board {
    /// Write `image` into the flash of the board at `port` and start it.
    /// A failure of the serial link is retried; a failure of the image is
    /// not.
    fn flash(&self, image: &FlashImage, port: &Path) -> Result<()>;
}

/// How many times a flash is attempted when espflash's link to the chip
/// fails on its way.
pub const FLASH_ATTEMPTS: usize = 3;

/// Run `write` again when espflash's link to the chip failed on its way,
/// with a power-on reset between the attempts where the board has one; each
/// retry is reported on stderr, which the job log keeps.
pub fn with_flash_retries(port: &Path, mut write: impl FnMut() -> Result<()>) -> Result<()> {
    retry_transient(FLASH_ATTEMPTS, &mut write, |attempt, error| {
        eprintln!(
            "hil: flash attempt {attempt} of {FLASH_ATTEMPTS} through {} failed: {error}; \
             retrying",
            port.display()
        );
    })
}

fn retry_transient<T>(
    attempts: usize,
    operation: &mut impl FnMut() -> Result<T>,
    mut between: impl FnMut(usize, &str),
) -> Result<T> {
    let mut attempt = 1;
    loop {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if attempt < attempts && transient_flash_failure(&error.to_string()) => {
                between(attempt, &error.to_string());
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Whether a flash failure lies in the serial link to the chip, which a
/// reset and a new connection can clear, rather than in the image.
fn transient_flash_failure(message: &str) -> bool {
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

/// `espflash`, or the program the `ESPFLASH` variable names.
pub fn espflash() -> std::process::Command {
    std::process::Command::new(std::env::var_os("ESPFLASH").unwrap_or_else(|| "espflash".into()))
}

/// Run `command` to completion within half an hour; `description` names it
/// on stderr and in the error.
pub fn run(command: &mut std::process::Command, description: &str) -> Result<()> {
    eprintln!("==> {description}");
    let status = oer_process::owned::Child::spawn(command)?
        .wait_timeout(Some(std::time::Duration::from_secs(30 * 60)))?;
    if !status.success() {
        return Err(format!("{description} failed with {status}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
