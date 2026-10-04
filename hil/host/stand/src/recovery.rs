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
    let (step, banner) = (RecoveryStep::RtsReset, rts());
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

/// A bootloader that keeps resetting the same way: the ROM answers, but no
/// image starts, whatever the stand flashes.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BootLoop {
    /// The repeated ROM reset line.
    pub reset_line: String,
    /// How many consecutive resets of that kind the console shows.
    pub resets: usize,
}

/// The reset code of a ROM line, `rst:0x7` of `rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x..`.
fn reset_code(line: &str) -> Option<&str> {
    line.split([' ', '(', ','])
        .next()
        .filter(|code| code.starts_with("rst:"))
}

/// The boot loop `console` ends in, if any: at least two consecutive ROM
/// reset lines with the same reset code at its end.
pub fn boot_loop(console: &str) -> Option<BootLoop> {
    let lines = console
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("rst:") && line.contains("boot:"))
        .collect::<Vec<_>>();
    let last = *lines.last()?;
    let code = reset_code(last)?;
    let resets = lines
        .iter()
        .rev()
        .take_while(|line| reset_code(line) == Some(code))
        .count();
    (resets >= 2).then(|| BootLoop {
        reset_line: last.to_owned(),
        resets,
    })
}

/// One reset the stand tried against a boot loop.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EscalationStep {
    pub step: RecoveryStep,
    /// The ROM line after it, or why the step could not run.
    pub outcome: std::result::Result<Option<String>, String>,
    /// Whether the console stopped looping after it.
    pub cleared: bool,
}

/// What the stand did about a boot loop, recorded as the repetition's
/// `reset-escalation.json`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResetEscalation {
    pub boot_loop: BootLoop,
    pub steps: Vec<EscalationStep>,
    /// Whether a step cleared the loop; when none did, the board is
    /// quarantined for a person.
    pub cleared: bool,
}

/// The file a repetition records its reset escalation in.
pub const RESET_ESCALATION_FILE: &str = "reset-escalation.json";

/// How long the console is read for the ROM's line after an escalation step
/// that does not return one itself.
const BANNER_WATCH: Duration = Duration::from_secs(2);

/// Climb past the RTS reset that did not clear `found`: a system reset
/// through the builtin USB-JTAG, then EN and the hub port's power when the
/// board has them, stopping at the first step after which `boots` sees the
/// image answer. When none clears it the board is quarantined for a person.
pub fn escalate_boot_loop(
    port: &Path,
    mac: Option<&str>,
    chip: &str,
    found: BootLoop,
    output: &Path,
    origin: &str,
    mut boots: impl FnMut() -> bool,
) -> ResetEscalation {
    let mac = mac.map(str::to_owned).or_else(|| board_mac(port));
    let arbiter = Arbiter::open().ok();
    let power = mac.as_deref().and_then(|mac| {
        arbiter
            .as_ref()?
            .devices()
            .ok()?
            .into_iter()
            .find(|device| device.mac == mac)?
            .power
    });
    let mut steps = Vec::new();
    let mut try_step = |step: RecoveryStep, reset: &dyn Fn() -> crate::Result<Option<String>>| {
        let outcome = reset().map_err(|error| error.to_string());
        let console = match &outcome {
            Ok(None) => post_mortem::current_port(port, mac.as_deref(), Duration::from_secs(10))
                .and_then(|port| read_console(&port, BANNER_WATCH).ok())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let reset_line = outcome.clone().map(|banner| {
            banner
                .as_deref()
                .and_then(oer_hil_arbiter::control::reset_line)
                .or_else(|| oer_hil_arbiter::control::reset_line(&console))
                .map(str::to_owned)
        });
        // Only an image that answers shows the loop is gone: a console read
        // across the port's re-enumeration may simply have missed it.
        let cleared = outcome.is_ok() && boots();
        steps.push(EscalationStep {
            step,
            outcome: reset_line,
            cleared,
        });
        cleared
    };
    let openocd = oer_hil_arbiter::control::Openocd::from_environment();
    let mut cleared = match (&openocd, mac.as_deref()) {
        (Some(openocd), Some(mac)) => try_step(RecoveryStep::JtagReset, &|| {
            openocd.reset(chip, mac, Duration::from_secs(60))?;
            Ok(None)
        }),
        _ => try_step(RecoveryStep::JtagReset, &|| {
            Err("no OpenOCD was passed to the runner or the board's MAC is unknown".into())
        }),
    };
    if !cleared && let Some(power) = power.clone() {
        cleared = try_step(RecoveryStep::PowerCycle, &|| {
            power.cycle()?;
            Ok(None)
        });
    }
    let escalation = ResetEscalation {
        boot_loop: found,
        steps,
        cleared,
    };
    let _ = oer_hil_durable::atomic_json(&output.join(RESET_ESCALATION_FILE), &escalation);
    if let (Some(arbiter), Some(mac)) = (&arbiter, mac.as_deref()) {
        match escalation.steps.iter().find(|step| step.cleared) {
            Some(step) => {
                let _ = arbiter.record_board_by(
                    String::from("stand"),
                    Some(mac.to_owned()),
                    oer_hil_arbiter::BoardEventKind::Recovered {
                        step: step.step,
                        hardware: true,
                        reset_line: step.outcome.clone().ok().flatten(),
                        origin: origin.to_owned(),
                    },
                );
            }
            None => {
                let _ = quarantine(
                    arbiter,
                    mac,
                    QuarantineTrigger::BootLoop,
                    format!(
                        "its bootloader resets in a loop ({}) that {} did not clear",
                        escalation.boot_loop.reset_line,
                        escalation
                            .steps
                            .iter()
                            .map(|step| format!("{:?}", step.step))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    output,
                );
            }
        }
    }
    escalation
}

/// Read the console at `port` for `watch` without resetting the chip.
fn read_console(port: &Path, watch: Duration) -> crate::Result<String> {
    use std::io::Read as _;
    let mut serial = serialport::new(port.to_string_lossy(), 115_200)
        .timeout(Duration::from_millis(100))
        .open()?;
    serial.write_data_terminal_ready(false)?;
    serial.write_request_to_send(false)?;
    let started = std::time::Instant::now();
    let mut console = Vec::new();
    let mut buffer = [0_u8; 1024];
    while started.elapsed() < watch {
        match serial.read(&mut buffer) {
            Ok(read) => console.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    Ok(String::from_utf8_lossy(&console).into_owned())
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

/// Image classes whose firmware did not answer after its ROM booted it,
/// earlier in this process: the run's remaining repetitions of them cannot
/// pass, so they are recorded without holding the board.
static SILENT_IMAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Record that `image`'s firmware did not answer after a boot.
pub fn mark_image_silent(image: &str) {
    if let Ok(mut images) = SILENT_IMAGES.lock()
        && !images.iter().any(|known| known == image)
    {
        images.push(image.to_owned());
    }
}

/// Whether `image`'s firmware did not answer after a boot in this process.
pub fn image_silent(image: &str) -> bool {
    SILENT_IMAGES
        .lock()
        .is_ok_and(|images| images.iter().any(|known| known == image))
}

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
    fn an_image_that_did_not_answer_after_a_boot_is_remembered() {
        assert!(!image_silent("test-silent-image"));
        mark_image_silent("test-silent-image");
        mark_image_silent("test-silent-image");
        assert!(image_silent("test-silent-image"));
        assert!(!image_silent("test-other-image"));
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
    fn a_bootloader_that_resets_the_same_way_again_is_a_boot_loop() {
        let looping = "ESP-ROM:esp32s31\nrst:0x1 (POWERON),boot:0x58 (SPI_FAST_FLASH_BOOT)\n\
            I (48) boot: Multicore bootloader\n\
            rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)\n\
            Core0 Saved PC:0x2f06f5f6\nI (48) boot: Multicore bootloader\n\
            rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)\n\
            I (48) boot: Multicore bootloader\n";
        assert_eq!(
            boot_loop(looping),
            Some(BootLoop {
                reset_line: String::from(
                    "rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)"
                ),
                resets: 2,
            })
        );
        // One reset, then the image: no loop.
        assert_eq!(
            boot_loop("rst:0x3 (SW_SYS_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)\nOER_BOOT\n"),
            None
        );
        // A different reset before the last one: no loop yet.
        assert_eq!(
            boot_loop(
                "rst:0x3 (SW_SYS_RESET),boot:0x58\nrst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58\n"
            ),
            None
        );
        assert_eq!(boot_loop(""), None);
    }

    #[test]
    fn a_reset_escalation_records_every_step_it_tried() {
        let escalation = ResetEscalation {
            boot_loop: BootLoop {
                reset_line: String::from("rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58"),
                resets: 3,
            },
            steps: vec![EscalationStep {
                step: RecoveryStep::JtagReset,
                outcome: Ok(Some(String::from("rst:0x3 (SW_SYS_RESET),boot:0x58"))),
                cleared: true,
            }],
            cleared: true,
        };
        let json = serde_json::to_value(&escalation).unwrap();
        assert_eq!(json["steps"][0]["step"], "jtag-reset");
        assert_eq!(
            serde_json::from_value::<ResetEscalation>(json).unwrap(),
            escalation
        );
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
