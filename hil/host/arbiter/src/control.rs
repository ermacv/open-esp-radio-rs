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
