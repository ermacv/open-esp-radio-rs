//! Bringing back a device under test that stopped answering.
//!
//! When a failed repetition's target does not answer the post-mortem query,
//! the runner climbs a ladder: it pulses EN through the board's registered
//! reset path, or RTS on the chip's own USB port for a board without one, and
//! asks again; a power cycle through a switchable hub port is
//! the next step once boards have one. A step that brings the board back is
//! journaled as a recovery; it counts as hardware-level when the port had
//! vanished or the ROM was waiting for a download, which firmware cannot
//! cause. A board no step brings back, or that needed hardware-level recovery
//! too often, is quarantined: it serves nobody until a person resets or
//! power-cycles it. What the stand saw is kept in the repetition's
//! `post-mortem/`, which the quarantine names.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use oer_hil_arbiter::{Arbiter, QuarantineTrigger, RecoveryStep};

use crate::post_mortem::{self, Finding};

/// What the ladder did.
pub enum Recovery {
    /// The board answered again after `step`.
    Recovered {
        step: RecoveryStep,
        hardware: bool,
        reset_line: Option<String>,
        finding: Box<Finding>,
    },
    /// The board was quarantined for `trigger`.
    Quarantined {
        trigger: QuarantineTrigger,
        reason: String,
    },
}

impl Recovery {
    /// One sentence for the repetition's failure message.
    pub fn describe(&self) -> String {
        match self {
            Self::Recovered {
                step,
                hardware,
                reset_line,
                ..
            } => format!(
                "the target did not answer; {step:?} brought it back ({} failure: {})",
                if *hardware {
                    "hardware"
                } else {
                    "firmware or unknown"
                },
                reset_line.as_deref().unwrap_or("no reset line")
            ),
            Self::Quarantined { reason, .. } => format!("the board was quarantined: {reason}"),
        }
    }

    pub fn quarantined(&self) -> bool {
        matches!(self, Self::Quarantined { .. })
    }
}

/// Recover the target at `port`, which did not answer after a failure in
/// the repetition whose output is `output`; `origin` names the run.
pub fn recover(
    port: &Path,
    mac: Option<&str>,
    output: &Path,
    elf: Option<&Path>,
    origin: &str,
) -> Option<Recovery> {
    let arbiter = Arbiter::open().ok()?;
    let mac = mac.map(str::to_owned).or_else(|| board_mac(port))?;
    let evidence = output.join("post-mortem");
    let hardware = post_mortem::current_port(port, Some(&mac), Duration::ZERO).is_none()
        || console_waits_for_download(output);
    let reset = arbiter
        .devices()
        .ok()?
        .into_iter()
        .find(|device| device.mac == mac)
        .and_then(|device| device.control?.reset);
    let (step, banner) = match reset {
        Some(reset) => (
            RecoveryStep::EnReset,
            reset.reset(oer_hil_arbiter::BootMode::Normal).ok(),
        ),
        None => (
            RecoveryStep::RtsReset,
            post_mortem::current_port(port, Some(&mac), Duration::from_secs(5))
                .and_then(|port| rts_reset(&port).ok()),
        ),
    };
    let reset_line = banner
        .as_deref()
        .and_then(oer_hil_arbiter::control::reset_line)
        .map(str::to_owned);
    let _ = std::fs::create_dir_all(&evidence);
    let _ = std::fs::write(
        evidence.join(format!("{step:?}-banner.txt").to_lowercase()),
        banner.as_deref().unwrap_or_default(),
    );
    let finding = post_mortem::inspect(port, Some(&mac), output, elf);
    let Some(finding) = finding else {
        return quarantine(
            &arbiter,
            &mac,
            QuarantineTrigger::Unreachable,
            format!(
                "it did not answer after {step:?} ({})",
                reset_line.as_deref().unwrap_or("no ROM line")
            ),
            &evidence,
        );
    };
    let hardware = hardware
        || reset_line
            .as_deref()
            .is_some_and(|line| line.contains("DOWNLOAD"));
    let _ = arbiter.record_board_by(
        String::from("stand"),
        Some(mac.clone()),
        oer_hil_arbiter::BoardEventKind::Recovered {
            step,
            hardware,
            reset_line: reset_line.clone(),
            origin: origin.to_owned(),
        },
    );
    if hardware
        && arbiter
            .recent_hardware_recoveries(&mac)
            .is_ok_and(|count| count >= oer_hil_arbiter::maintenance::FLAKY_RECOVERIES)
    {
        return quarantine(
            &arbiter,
            &mac,
            QuarantineTrigger::Flaky,
            format!(
                "it needed hardware-level recovery {} times within {} minutes",
                oer_hil_arbiter::maintenance::FLAKY_RECOVERIES,
                oer_hil_arbiter::maintenance::FLAKY_WINDOW.as_secs() / 60
            ),
            &evidence,
        );
    }
    Some(Recovery::Recovered {
        step,
        hardware,
        reset_line,
        finding: Box::new(finding),
    })
}

/// Set once this process quarantined its device under test: the run's
/// remaining repetitions then record the quarantine without touching it.
static QUARANTINED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this process quarantined its device under test.
pub fn device_quarantined() -> bool {
    QUARANTINED.load(std::sync::atomic::Ordering::Relaxed)
}

fn quarantine(
    arbiter: &Arbiter,
    mac: &str,
    trigger: QuarantineTrigger,
    reason: String,
    evidence: &Path,
) -> Option<Recovery> {
    arbiter
        .quarantine(
            mac,
            trigger,
            reason.clone(),
            Some(evidence.display().to_string()),
        )
        .ok()?;
    QUARANTINED.store(true, std::sync::atomic::Ordering::Relaxed);
    Some(Recovery::Quarantined { trigger, reason })
}

/// Pulse RTS on the chip's USB Serial/JTAG port with the boot strap released
/// and return what the console printed within two seconds.
fn rts_reset(port: &Path) -> crate::Result<String> {
    use std::io::Read as _;
    let mut serial = serialport::new(port.to_string_lossy(), 115_200)
        .timeout(Duration::from_millis(100))
        .open()?;
    serial.write_data_terminal_ready(false)?;
    serial.write_request_to_send(true)?;
    std::thread::sleep(Duration::from_millis(200));
    serial.write_request_to_send(false)?;
    let started = std::time::Instant::now();
    let mut banner = Vec::new();
    let mut buffer = [0_u8; 1024];
    while started.elapsed() < Duration::from_secs(2) {
        match serial.read(&mut buffer) {
            Ok(read) => banner.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    Ok(String::from_utf8_lossy(&banner).into_owned())
}

/// The MAC of the board at `port`, or of the board whose port it was.
fn board_mac(port: &Path) -> Option<String> {
    oer_hil_arbiter::port_mac(port).or_else(|| {
        // A vanished port still names its board in /dev/serial/by-id.
        let name = port.file_name()?.to_string_lossy().into_owned();
        let mac = name.rsplit('_').next()?.strip_suffix("-if00")?;
        oer_hil_arbiter::normalize_mac(mac).ok()
    })
}

/// Whether the repetition's console ended in the ROM waiting for a download.
fn console_waits_for_download(output: &Path) -> bool {
    let tail = |path: PathBuf| {
        std::fs::read(path).ok().map(|bytes| {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            text.lines()
                .rev()
                .take(20)
                .any(|line| line.contains("DOWNLOAD(") || line.contains("waiting for download"))
        })
    };
    tail(output.join("uart.log")).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vanished_by_id_port_still_names_its_board() {
        assert_eq!(
            board_mac(Path::new(
                "/dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_38:44:BE:AA:25:64-if00"
            ))
            .as_deref(),
            Some("38:44:BE:AA:25:64")
        );
        assert_eq!(board_mac(Path::new("/dev/ttyACM9")), None);
    }

    #[test]
    fn a_console_that_ends_in_download_mode_is_a_hardware_failure() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("uart.log"),
            "app\nrst:0x18 (JTAG_CPU),boot:0x0 (DOWNLOAD(UART0/SDIO_FEI_FEO))\nwait uart0/sdio download\n",
        )
        .unwrap();
        assert!(console_waits_for_download(directory.path()));
        std::fs::write(directory.path().join("uart.log"), "app\n").unwrap();
        assert!(!console_waits_for_download(directory.path()));
    }
}
