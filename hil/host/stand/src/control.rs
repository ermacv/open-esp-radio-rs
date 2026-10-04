//! A board's ways back: the rungs of its stand-file reset ladder, the
//! automatic entry into its ROM's download mode, and [`climb`], the one
//! ladder the stand's recoveries take.
//!
//! The rungs are the board's `reset` steps in the stand file's order: an RTS
//! pulse on its USB Serial/JTAG port, a system reset through its builtin
//! USB-JTAG with OpenOCD, and its hub port's power. After them, a board that
//! resets by power can be put into the ROM's download mode: its port is
//! powered off and on and, as soon as its USB returns, reset into download
//! through the USB Serial/JTAG, before a flashed image that switches the
//! USB Serial/JTAG off runs. A ROM in download mode serves the USB
//! Serial/JTAG, so the stand can load firmware into the board.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use oer_hil_arbiter::{
    RecoveryStep,
    control::{Openocd, PowerControl},
};
use oer_hil_stand_schema::ResetStep;
use serde::{Deserialize, Serialize};

use crate::post_mortem;

/// How long a reset's console is read for the ROM's line.
const BANNER: Duration = Duration::from_secs(2);
/// How long a board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);
/// How long a returned port may take until the stand's user may open it.
const PORT_ACCESS: Duration = Duration::from_secs(3);
/// How long the JTAG reset may take.
const JTAG: Duration = Duration::from_secs(60);

/// One way to reset a board.
pub trait Rung {
    fn step(&self) -> RecoveryStep;
    /// Reset the board; the console its reset read, when the reset read it.
    fn reset(&self) -> crate::Result<Option<String>>;
}

struct Rts {
    port: PathBuf,
    mac: String,
}

impl Rung for Rts {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::RtsReset
    }

    fn reset(&self) -> crate::Result<Option<String>> {
        let port = post_mortem::current_port(&self.port, Some(&self.mac), Duration::from_secs(5))
            .ok_or("the board's port is gone")?;
        let serial = oer_hil_board::reset::reset_into_application(&port)?;
        Ok(Some(read(serial, BANNER)))
    }
}

struct Jtag {
    openocd: Option<Openocd>,
    chip: String,
    mac: String,
}

impl Rung for Jtag {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::JtagReset
    }

    fn reset(&self) -> crate::Result<Option<String>> {
        let openocd = self
            .openocd
            .as_ref()
            .ok_or("no OpenOCD was passed to the runner")?;
        openocd.reset(&self.chip, &self.mac, JTAG)?;
        Ok(None)
    }
}

struct Power {
    power: PowerControl,
}

impl Rung for Power {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::PowerCycle
    }

    fn reset(&self) -> crate::Result<Option<String>> {
        self.power.cycle()?;
        Ok(None)
    }
}

/// A board's rungs and its download entry, from the stand file.
pub struct BoardControl {
    port: PathBuf,
    mac: String,
    chip: String,
    rungs: Vec<Box<dyn Rung>>,
    power: Option<PowerControl>,
}

impl BoardControl {
    /// The board whose USB serial number is `mac`, last seen at `port`.
    pub fn of_board(port: &Path, mac: &str) -> crate::Result<Self> {
        let arbiter = oer_hil_arbiter::Arbiter::open()?;
        let stand = oer_hil_stand_schema::StandFile::load(arbiter.stand_file())?;
        let board = stand
            .board_by_serial(mac)
            .ok_or_else(|| format!("board {mac} is not in the stand file"))?;
        let device = arbiter
            .devices()?
            .into_iter()
            .find(|device| device.mac == mac)
            .ok_or_else(|| format!("board {mac} is not in the stand file"))?;
        let openocd = Openocd::from_environment();
        let rungs = board
            .reset
            .iter()
            .filter_map(|step| -> Option<Box<dyn Rung>> {
                match step {
                    ResetStep::UsbJtagRts => Some(Box::new(Rts {
                        port: port.to_owned(),
                        mac: mac.to_owned(),
                    })),
                    ResetStep::Jtag => Some(Box::new(Jtag {
                        openocd: openocd.clone(),
                        chip: board.chip.clone(),
                        mac: mac.to_owned(),
                    })),
                    ResetStep::Power => device
                        .power
                        .clone()
                        .map(|power| Box::new(Power { power }) as Box<dyn Rung>),
                }
            })
            .collect();
        Ok(Self {
            port: port.to_owned(),
            mac: mac.to_owned(),
            chip: board.chip.clone(),
            rungs,
            power: device.power,
        })
    }

    pub fn chip(&self) -> &str {
        &self.chip
    }

    /// The rungs in the stand file's order.
    pub fn rungs(&self) -> Vec<&dyn Rung> {
        self.rungs.iter().map(Box::as_ref).collect()
    }

    /// The board's console read for `watch` without resetting it, once its
    /// port is back.
    pub fn console(&self, watch: Duration) -> String {
        post_mortem::current_port(&self.port, Some(&self.mac), REATTACH)
            .and_then(|port| oer_hil_board::reset::open_without_reset(&port).ok())
            .map(|serial| read(serial, watch))
            .unwrap_or_default()
    }

    /// Power the board off and on and, as soon as its USB returns, reset it
    /// into its ROM's download mode through the USB Serial/JTAG; the
    /// console the ROM printed. Only a board that resets by power has it.
    pub fn download_entry(&self) -> Option<impl Fn() -> crate::Result<String> + '_> {
        let power = self.power.as_ref()?;
        Some(move || {
            let mac = self.mac.clone();
            let cycle = power.cycle_observed(&|| {
                oer_hil_arbiter::attached_ports()
                    .iter()
                    .any(|port| port.mac.as_deref() == Some(mac.as_str()))
            })?;
            cycle.verdict()?;
            let port = post_mortem::current_port(&self.port, Some(&self.mac), REATTACH)
                .ok_or("the board's port did not return after its power")?;
            // udev hands the returned port to the stand's group a moment
            // after it appears: retry the open briefly, then reset at once.
            let started = std::time::Instant::now();
            let serial = loop {
                match oer_hil_board::reset::reset_into_download(&port) {
                    Ok(serial) => break serial,
                    Err(_) if started.elapsed() < PORT_ACCESS => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            Ok(read(serial, BANNER))
        })
    }
}

fn read(mut serial: Box<dyn serialport::SerialPort>, watch: Duration) -> String {
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
    String::from_utf8_lossy(&console).into_owned()
}

/// One rung the ladder tried.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LadderStep {
    pub step: RecoveryStep,
    /// The ROM's reset line after the rung, or why the rung failed.
    pub outcome: Result<Option<String>, String>,
    /// Where the ROM's banner says core 0 was when the reset hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_pc: Option<u32>,
    /// Whether the firmware answered after the rung.
    pub cleared: bool,
}

/// Where the ladder ended.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "end", rename_all = "kebab-case")]
pub enum LadderEnd {
    /// The firmware answered after the last step.
    Cleared,
    /// The firmware never answered, but the ROM did, booting from flash or
    /// waiting for a download: the stand can load firmware into the board.
    Loadable { reset_line: String },
    /// Neither the firmware nor the ROM answered: a person is needed.
    Silent,
}

/// What the ladder did.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Ladder {
    pub steps: Vec<LadderStep>,
    pub end: LadderEnd,
}

/// Climb `rungs`, then the download entry when the board has one, stopping
/// at the first step after which `answers` sees the firmware answer.
/// `console` reads the board's console after a rung that read none itself.
pub fn climb(
    rungs: &[&dyn Rung],
    download_entry: Option<&dyn Fn() -> crate::Result<String>>,
    console: &dyn Fn() -> String,
    answers: &mut dyn FnMut() -> bool,
) -> Ladder {
    let mut steps = Vec::new();
    for rung in rungs {
        let outcome = rung
            .reset()
            .map(|banner| banner.unwrap_or_else(console))
            .map_err(|error| error.to_string());
        let cleared = outcome.is_ok() && answers();
        steps.push(LadderStep {
            step: rung.step(),
            saved_pc: outcome.as_deref().ok().and_then(saved_pc),
            outcome: outcome.map(|banner| reset_line(&banner)),
            cleared,
        });
        if cleared {
            return Ladder {
                steps,
                end: LadderEnd::Cleared,
            };
        }
    }
    if let Some(entry) = download_entry {
        let outcome = entry().map_err(|error| error.to_string());
        steps.push(LadderStep {
            step: RecoveryStep::DownloadEntry,
            saved_pc: outcome.as_deref().ok().and_then(saved_pc),
            outcome: outcome.map(|banner| reset_line(&banner)),
            cleared: false,
        });
    }
    // The download entry's line, when it reached the ROM, else the latest
    // line of a ROM that answered.
    let end = steps
        .iter()
        .rev()
        .find_map(|step| {
            step.outcome
                .as_ref()
                .ok()
                .cloned()
                .flatten()
                .filter(|line| rom_answers(line))
        })
        .map_or(LadderEnd::Silent, |reset_line| LadderEnd::Loadable {
            reset_line,
        });
    Ladder { steps, end }
}

/// The program counter the ROM reports core 0 was at when the reset hit
/// (`Core0 Saved PC:0x...`), the last one printed.
pub fn saved_pc(banner: &str) -> Option<u32> {
    banner.lines().rev().find_map(|line| {
        let value = line.split_once("Core0 Saved PC:")?.1.trim();
        u32::from_str_radix(value.strip_prefix("0x")?, 16).ok()
    })
}

/// The ROM's last reset line in `console`.
fn reset_line(console: &str) -> Option<String> {
    oer_hil_arbiter::control::reset_line(console).map(str::to_owned)
}

/// Whether a ROM reset line shows a ROM that answers, booting from flash or
/// waiting for a download: the stand can load firmware into the board.
pub fn rom_answers(line: &str) -> bool {
    line.starts_with("rst:") && line.contains("boot:")
}

#[cfg(test)]
mod tests;
