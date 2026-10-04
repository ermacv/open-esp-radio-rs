//! Out-of-band control of a board: how the stand resets it or cuts its power
//! without the chip's own USB port.
//!
//! A board's registry entry may name a reset path, such as a DevKit's
//! USB-to-UART bridge whose modem lines drive the chip's EN and BOOT pins,
//! and a power switch. Both are absent unless an operator registers them.
//! Resetting through EN restarts the whole chip, like the board's RST button,
//! and recovers a chip whose USB Serial/JTAG port no longer answers.

use std::{
    io::Read as _,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

/// How the stand controls a board beyond its USB Serial/JTAG port.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<ResetControl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<PowerControl>,
}

/// A reset path through a USB serial bridge's modem lines.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResetControl {
    pub via: ResetVia,
    /// USB serial number of the bridge.
    pub serial: String,
    /// The modem line that pulls the chip's EN low.
    pub en: Line,
    /// The modem line that pulls the chip's boot strap low.
    pub boot: Line,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ResetVia {
    #[serde(rename = "uart-rts-dtr")]
    UartRtsDtr,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Line {
    Rts,
    Dtr,
}

impl std::str::FromStr for Line {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "rts" => Ok(Self::Rts),
            "dtr" => Ok(Self::Dtr),
            _ => Err(format!("`{text}` is not a modem line; use rts or dtr")),
        }
    }
}

/// A switchable USB hub port powering the board.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PowerControl {
    pub via: PowerVia,
    /// The hub's `uhubctl` location.
    pub location: String,
    pub port: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PowerVia {
    #[serde(rename = "uhubctl")]
    Uhubctl,
}

/// What a reset asked the chip to boot into.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootMode {
    /// Boot from flash, as the RST button does.
    Normal,
    /// Hold the boot strap low across the reset: the ROM waits for a download.
    Download,
}

/// How long the ROM's banner is read after the reset.
const BANNER: Duration = Duration::from_millis(1500);

impl ResetControl {
    /// Reset the chip through EN into `mode` and return what its ROM printed
    /// on the bridge, whose first `rst:` line names the observed reset reason
    /// and boot mode.
    pub fn reset(&self, mode: BootMode) -> crate::Result<String> {
        let ResetVia::UartRtsDtr = self.via;
        if self.en == self.boot {
            return Err("a reset path needs different EN and BOOT lines".into());
        }
        let path = crate::attached_ports()
            .into_iter()
            .find(|port| port.mac.as_deref() == Some(self.serial.as_str()))
            .ok_or_else(|| format!("reset bridge {} is not attached", self.serial))?
            .port;
        let mut port = serialport::new(&path, 115_200)
            .timeout(Duration::from_millis(50))
            .open()
            .map_err(|error| format!("cannot open reset bridge {path}: {error}"))?;
        // Opening raises both lines, which the auto-program circuit ignores.
        // Releasing EN's line first passes through BOOT-only, which does not
        // reset; releasing BOOT's line first would pull EN low.
        let set = |port: &mut Box<dyn serialport::SerialPort>,
                   line: Line,
                   asserted: bool|
         -> crate::Result<()> {
            match line {
                Line::Rts => port.write_request_to_send(asserted)?,
                Line::Dtr => port.write_data_terminal_ready(asserted)?,
            }
            Ok(())
        };
        set(&mut port, self.en, false)?;
        set(&mut port, self.boot, false)?;
        // Drop what the bridge buffered before this reset, such as the banner
        // of an earlier one.
        std::thread::sleep(Duration::from_millis(50));
        port.clear(serialport::ClearBuffer::Input)?;
        match mode {
            BootMode::Normal => {
                set(&mut port, self.en, true)?;
                std::thread::sleep(Duration::from_millis(100));
                set(&mut port, self.en, false)?;
            }
            BootMode::Download => {
                set(&mut port, self.boot, true)?;
                std::thread::sleep(Duration::from_millis(50));
                set(&mut port, self.en, true)?;
                set(&mut port, self.boot, false)?;
                std::thread::sleep(Duration::from_millis(100));
                set(&mut port, self.boot, true)?;
                set(&mut port, self.en, false)?;
                std::thread::sleep(Duration::from_millis(50));
                set(&mut port, self.boot, false)?;
            }
        }
        let started = Instant::now();
        let mut banner = Vec::new();
        let mut buffer = [0_u8; 1024];
        while started.elapsed() < BANNER {
            match port.read(&mut buffer) {
                Ok(read) => banner.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(String::from_utf8_lossy(&banner).into_owned())
    }
}

impl PowerControl {
    /// Whether the board's hub port is powered.
    pub fn is_on(&self) -> crate::Result<bool> {
        let PowerVia::Uhubctl = self.via;
        let output = oer_process::output(
            std::process::Command::new("uhubctl")
                .args(["--location", &self.location, "--ports"])
                .arg(self.port.to_string()),
            Some(Duration::from_secs(30)),
        )?;
        port_powered(
            &String::from_utf8_lossy(&output.stdout),
            &self.location,
            self.port,
        )
        .ok_or_else(|| {
            format!(
                "uhubctl reports no port {} of hub {}",
                self.port, self.location
            )
            .into()
        })
    }

    /// Power the board's hub port on: only the arbiter does, to return a port
    /// to its working state when its lease changes hands.
    pub(crate) fn switch_on(&self) -> crate::Result<()> {
        self.action("on")
    }

    /// Power the board's hub port off and on again with `uhubctl`.
    pub fn cycle(&self) -> crate::Result<()> {
        self.action("cycle")
    }

    /// Power the hub port off for `off`, then on again: long enough for a
    /// person to see which button's light goes out.
    pub fn cycle_holding(&self, off: Duration) -> crate::Result<()> {
        self.action_delayed("cycle", off.as_secs().max(1))
    }

    fn action(&self, action: &str) -> crate::Result<()> {
        self.action_delayed(action, 2)
    }

    fn action_delayed(&self, action: &str, delay_secs: u64) -> crate::Result<()> {
        let PowerVia::Uhubctl = self.via;
        let output = oer_process::output(
            std::process::Command::new("uhubctl")
                .args(["--location", &self.location, "--ports"])
                .arg(self.port.to_string())
                .args(["--action", action, "--delay"])
                .arg(delay_secs.to_string()),
            Some(Duration::from_secs(30 + delay_secs)),
        )?;
        if !output.status.success() {
            return Err(format!(
                "uhubctl could not {action} {} port {}: {}",
                self.location,
                self.port,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        Ok(())
    }
}

/// Whether `uhubctl`'s report shows port `port` of hub `location` powered:
/// the port line of that hub's section, not its USB 3 companion's.
fn port_powered(report: &str, location: &str, port: u32) -> Option<bool> {
    let header = format!("Current status for hub {location} ");
    let section = report.split(&header).nth(1)?;
    let section = section.split("Current status for hub").next()?;
    let line = section
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("Port {port}:")))?;
    Some(line.split_whitespace().any(|word| word == "power"))
}

/// Names the OpenOCD executable of the ESP-IDF tools; the stand's wrapper
/// passes it to the runner, which cannot locate the tools itself.
pub const OPENOCD_ENV: &str = "OER_HIL_OPENOCD";
/// Names OpenOCD's script directory.
pub const OPENOCD_SCRIPTS_ENV: &str = "OER_HIL_OPENOCD_SCRIPTS";

/// The OpenOCD build that reaches a chip's builtin USB-JTAG.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Openocd {
    pub program: std::path::PathBuf,
    pub scripts: std::path::PathBuf,
}

impl Openocd {
    /// The build the stand's wrapper named, if any.
    pub fn from_environment() -> Option<Self> {
        Some(Self {
            program: std::env::var_os(OPENOCD_ENV)?.into(),
            scripts: std::env::var_os(OPENOCD_SCRIPTS_ENV)?.into(),
        })
    }

    /// The arguments of a `reset run` of the board of `chip` whose USB
    /// serial number is `mac`.
    pub fn reset_arguments(&self, chip: &str, mac: &str) -> Vec<String> {
        vec![
            String::from("-s"),
            self.scripts.display().to_string(),
            String::from("-f"),
            format!("board/{chip}-builtin.cfg"),
            String::from("-c"),
            format!("adapter serial {mac}"),
            String::from("-c"),
            String::from("init"),
            String::from("-c"),
            String::from("reset run"),
            String::from("-c"),
            String::from("shutdown"),
        ]
    }

    /// Reset the chip through its builtin USB-JTAG and let it run. On the
    /// esp32s31 this is a software system reset (`rst:0x3`) that also clears
    /// low-power and PMU state which an RTS reset through the USB
    /// Serial/JTAG port, and a reflash, leave in place (a powered-down MPLL
    /// kept the second-stage bootloader in a watchdog loop until it).
    pub fn reset(&self, chip: &str, mac: &str, timeout: Duration) -> crate::Result<()> {
        let output = oer_process::output(
            std::process::Command::new(&self.program).args(self.reset_arguments(chip, mac)),
            Some(timeout),
        )
        .map_err(|error| {
            if error.is::<oer_process::owned::DeadlineExceeded>() {
                format!(
                    "OpenOCD did not finish within {}s and was stopped",
                    timeout.as_secs()
                )
                .into()
            } else {
                error
            }
        })?;
        if !output.status.success() {
            let log = String::from_utf8_lossy(&output.stderr);
            let tail = log.lines().rev().take(20).collect::<Vec<_>>();
            return Err(format!(
                "OpenOCD failed with {}:\n{}",
                output.status,
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            )
            .into());
        }
        Ok(())
    }
}

impl Openocd {
    /// The arguments that halt the chip of `chip` whose USB serial number is
    /// `mac`, print `registers`, and let it run on, with no reset and no
    /// debugger server listening.
    pub fn register_arguments(&self, chip: &str, mac: &str, registers: &[&str]) -> Vec<String> {
        let mut commands = String::from("init; halt");
        for register in registers {
            commands.push_str(&format!("; echo \"{register} [reg {register}]\""));
        }
        commands.push_str("; resume; shutdown");
        vec![
            String::from("-s"),
            self.scripts.display().to_string(),
            String::from("-c"),
            String::from("gdb_port disabled; telnet_port disabled; tcl_port disabled"),
            String::from("-f"),
            format!("board/{chip}-builtin.cfg"),
            String::from("-c"),
            format!("adapter serial {mac}"),
            String::from("-c"),
            commands,
        ]
    }

    /// Halt the chip, read `registers` of its current hart and let it run
    /// on: what a target that stopped answering was doing, without the reset
    /// that would erase it.
    pub fn registers(
        &self,
        chip: &str,
        mac: &str,
        registers: &[&str],
        timeout: Duration,
    ) -> crate::Result<Vec<(String, u32)>> {
        let output = oer_process::output(
            std::process::Command::new(&self.program)
                .args(self.register_arguments(chip, mac, registers)),
            Some(timeout),
        )?;
        let log = String::from_utf8_lossy(&output.stderr).into_owned()
            + &String::from_utf8_lossy(&output.stdout);
        let values = parse_registers(&log, registers);
        if values.is_empty() {
            let tail = log.lines().rev().take(12).collect::<Vec<_>>();
            return Err(format!(
                "OpenOCD read no register ({}):\n{}",
                output.status,
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            )
            .into());
        }
        Ok(values)
    }
}

/// The `NAME name (/32): 0x…` lines OpenOCD echoed for `registers`.
fn parse_registers(log: &str, registers: &[&str]) -> Vec<(String, u32)> {
    registers
        .iter()
        .filter_map(|register| {
            let line = log
                .lines()
                .find(|line| line.trim_start().starts_with(&format!("{register} ")))?;
            let value = line.rsplit(':').next()?.trim();
            let value = u32::from_str_radix(value.strip_prefix("0x")?, 16).ok()?;
            Some((String::from(*register), value))
        })
        .collect()
}

/// The ROM's last `rst:... boot:...` line in `banner`.
pub fn reset_line(banner: &str) -> Option<&str> {
    banner
        .lines()
        .map(str::trim)
        .rfind(|line| line.starts_with("rst:") && line.contains("boot:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_entry_round_trips_and_is_optional() {
        let text = r#"{"reset":{"via":"uart-rts-dtr","serial":"5B90165754","en":"rts","boot":"dtr"},
            "power":{"via":"uhubctl","location":"3-1","port":2}}"#;
        let control: Control = serde_json::from_str(text).unwrap();
        assert_eq!(control.reset.as_ref().unwrap().en, Line::Rts);
        assert_eq!(control.power.as_ref().unwrap().port, 2);
        let back: Control =
            serde_json::from_str(&serde_json::to_string(&control).unwrap()).unwrap();
        assert_eq!(back, control);
        assert_eq!(
            serde_json::from_str::<Control>("{}").unwrap(),
            Control::default()
        );
        assert!(serde_json::from_str::<Control>(r#"{"reset":{"via":"gpio"}}"#).is_err());
    }

    #[test]
    fn echoed_registers_are_read_and_a_missing_one_is_left_out() {
        let log = "Info : [esp32c5] Target halted, PC=0x42010736\n\
                   pc pc (/32): 0x42010736\n\
                   mcause mcause (/32): 0x30000007\n\
                   Info : shutdown command invoked\n";
        assert_eq!(
            parse_registers(log, &["pc", "mcause", "mtval"]),
            [
                (String::from("pc"), 0x4201_0736),
                (String::from("mcause"), 0x3000_0007)
            ]
        );
        let openocd = Openocd {
            program: "openocd".into(),
            scripts: "/scripts".into(),
        };
        let arguments = openocd.register_arguments("esp32c5", "38:44:BE:AA:25:64", &["pc"]);
        let commands = arguments.last().unwrap();
        // It reads without resetting and leaves the chip running.
        assert!(commands.contains("halt") && commands.ends_with("resume; shutdown"));
        assert!(!commands.contains("reset"));
    }

    #[test]
    fn the_rom_reset_line_is_found_in_its_banner() {
        let banner = "ESP-ROM:esp32c5-eco2-20250121\r\nBuild:Jan 21 2025\r\n\
                      rst:0x1 (POWERON),boot:0x18 (SPI_FAST_FLASH_BOOT)\r\nSPI mode:DIO\r\n";
        assert_eq!(
            reset_line(banner),
            Some("rst:0x1 (POWERON),boot:0x18 (SPI_FAST_FLASH_BOOT)")
        );
        assert_eq!(reset_line("garbage"), None);
    }
}

#[cfg(test)]
mod power_tests {
    use super::port_powered;

    const REPORT: &str =
        "Current status for hub 4-8.3 [0bda:0411 Generic USB3.2 Hub, USB 3.20, 4 ports, ppps]
  Port 2: 02a0 power 5gbps Rx.Detect
Current status for hub 3-8.3 [0bda:5411 Generic USB2.1 Hub, USB 2.10, 4 ports, ppps]
  Port 1: 0100 power
  Port 2: 0000 off
";

    #[test]
    fn a_port_is_read_from_its_own_hub_not_its_companion() {
        assert_eq!(port_powered(REPORT, "3-8.3", 2), Some(false));
        assert_eq!(port_powered(REPORT, "3-8.3", 1), Some(true));
        assert_eq!(port_powered(REPORT, "4-8.3", 2), Some(true));
        assert_eq!(port_powered(REPORT, "3-8.3", 4), None);
    }
}
