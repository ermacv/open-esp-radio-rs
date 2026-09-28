//! Bringing back a device under test that stopped answering.
//!
//! When a failed repetition's target does not answer the post-mortem query,
//! the runner climbs a ladder: it pulses EN through the board's registered
//! reset path, or RTS on the chip's own USB port for a board without one, and
//! asks again. A step that brings the board back is journaled as a recovery;
//! it counts as hardware-level when the port had vanished or the ROM was
//! waiting for a download, which firmware cannot cause.
//!
//! A board is quarantined only when no script can bring it back to a state
//! in which firmware can be loaded: its ROM stays silent after the reset. A
//! ROM that answers, booting from flash or waiting for a download, means the
//! stand can reflash the board, however bad its firmware; such a board is
//! never quarantined, however often it needed recovering. A quarantined board
//! serves nobody until a person resets or power-cycles it. What the stand saw
//! is kept in the repetition's `post-mortem/`, which the quarantine names.

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
        /// Where core 0 was when the reset hit, from the ROM banner.
        core0: Option<String>,
        finding: Box<Finding>,
    },
    /// After `step` the ROM answered, but the firmware did not: a firmware
    /// or host fault. The stand can reflash the board, so it is not
    /// quarantined.
    BootedSilent {
        step: RecoveryStep,
        reset_line: String,
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
                core0,
                ..
            } => format!(
                "the target did not answer; {step:?} brought it back ({} failure: {}){}",
                if *hardware {
                    "hardware"
                } else {
                    "firmware or unknown"
                },
                reset_line.as_deref().unwrap_or("no reset line"),
                core0
                    .as_deref()
                    .map(|core0| format!("; core 0 was at {core0}"))
                    .unwrap_or_default()
            ),
            Self::BootedSilent { step, reset_line } => format!(
                "the target did not answer; after {step:?} its ROM answered ({reset_line}) \
                 but the firmware did not: a firmware or host fault, not a board fault"
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
    // A cancelled run stops rather than judges the board it leaves.
    if !judges_board(oer_process::cancellation_requested()) {
        return None;
    }
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
    let rts = || {
        post_mortem::current_port(port, Some(&mac), Duration::from_secs(5))
            .and_then(|port| rts_reset(&port).ok())
    };
    let line_of = |banner: &Option<String>| {
        banner
            .as_deref()
            .and_then(oer_hil_arbiter::control::reset_line)
            .map(str::to_owned)
    };
    let (mut step, mut banner) = match &reset {
        Some(reset) => (
            RecoveryStep::EnReset,
            reset.reset(oer_hil_arbiter::BootMode::Normal).ok(),
        ),
        None => (RecoveryStep::RtsReset, rts()),
    };
    // Every path is tried before a silent ROM is taken for a lost board.
    if line_of(&banner).is_none() && reset.is_some() {
        let retried = rts();
        if line_of(&retried).is_some() {
            (step, banner) = (RecoveryStep::RtsReset, retried);
        }
    }
    let reset_line = line_of(&banner);
    let core0 = banner.as_deref().and_then(saved_pc).map(|address| {
        let symbols = elf.and_then(|elf| addr2line::Loader::new(elf).ok());
        post_mortem::symbol(symbols.as_ref(), address)
    });
    let _ = std::fs::create_dir_all(&evidence);
    let _ = std::fs::write(
        evidence.join(format!("{step:?}-banner.txt").to_lowercase()),
        banner.as_deref().unwrap_or_default(),
    );
    let finding = post_mortem::inspect(port, Some(&mac), output, elf);
    let Some(finding) = finding else {
        if !judges_board(oer_process::cancellation_requested()) {
            eprintln!(
                "hil: the run was cancelled while the board was recovering; it is not \
                 quarantined"
            );
            return None;
        }
        // A ROM that answers can be reflashed: only a silent one needs a person.
        if let Some(line) = reset_line.as_deref().filter(|line| rom_answers(line)) {
            return Some(Recovery::BootedSilent {
                step,
                reset_line: line.to_owned(),
            });
        }
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
    Some(Recovery::Recovered {
        step,
        hardware,
        reset_line,
        core0,
        finding: Box::new(finding),
    })
}

/// Whether a ROM reset line shows a ROM that answers, booting from flash or
/// waiting for a download: the stand can load firmware into the board.
fn rom_answers(line: &str) -> bool {
    line.starts_with("rst:") && line.contains("boot:")
}

/// Whether an unanswered query after a reset judges the board. A cancelled
/// runner stops waiting for the answer early, which says nothing about it.
fn judges_board(cancelled: bool) -> bool {
    !cancelled
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

/// The program counter the ROM reports core 0 was at when the reset hit
/// (`Core0 Saved PC:0x...`), the last one printed.
fn saved_pc(banner: &str) -> Option<u32> {
    banner.lines().rev().find_map(|line| {
        let value = line.split_once("Core0 Saved PC:")?.1.trim();
        u32::from_str_radix(value.strip_prefix("0x")?, 16).ok()
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
    fn only_a_board_whose_rom_stays_silent_is_quarantined() {
        // A ROM that answers can be reflashed, whatever the firmware does.
        assert!(rom_answers(
            "rst:0x17 (CHIP_USB_UART_RESET),boot:0x5f (SPI_FAST_FLASH_BOOT)"
        ));
        assert!(rom_answers(
            "rst:0x1 (POWERON),boot:0x4 (DOWNLOAD(USB/UART0))"
        ));
        assert!(!rom_answers("garbled output"));
    }

    #[test]
    fn a_cancelled_run_never_quarantines_its_board() {
        assert!(!judges_board(true));
        assert!(judges_board(false));
    }

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
    fn the_rom_banner_names_where_core_zero_was() {
        let banner = "ESP-ROM:esp32s31-20251218\nrst:0x17 (CHIP_USB_UART_RESET),boot:0x5f (SPI_FAST_FLASH_BOOT)\nCore0 Saved PC:0x50050cd4\nSPI mode:DIO\n";
        assert_eq!(saved_pc(banner), Some(0x5005_0cd4));
        assert_eq!(saved_pc("rst:0x1 (POWERON)\n"), None);
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
