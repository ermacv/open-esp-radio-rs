//! Boards of the stand, identified by the MAC address that Espressif USB
//! Serial/JTAG ports report as their USB serial number. The stand file
//! (`oer-hil-stand-schema`) describes them; the arbiter reads it and never
//! writes it.

use std::{fs, path::Path};

use serde::Serialize;

use crate::Arbiter;

/// A board of the stand file, as the arbiter controls it: its MAC (the USB
/// serial number in the ports' form), id, chip and, when its reset ladder
/// includes `power`, its hub port. Boards have no fixed role: a scenario or
/// other consumer chooses which board it uses in which role.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Device {
    pub mac: String,
    pub name: String,
    pub chip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<crate::control::PowerControl>,
}

impl Device {
    /// `name (chip)`.
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.chip)
    }
}

/// The boards of a stand file.
pub fn devices_of(file: &oer_hil_stand_schema::StandFile) -> crate::Result<Vec<Device>> {
    file.board
        .iter()
        .map(|board| {
            let hub = file.hub(&board.port.hub).ok_or_else(|| {
                format!(
                    "board `{}`: hub `{}` is not described",
                    board.id, board.port.hub
                )
            })?;
            Ok(Device {
                mac: normalize_mac(&board.usb_serial)?,
                name: board.id.clone(),
                chip: board.chip.clone(),
                power: board
                    .reset
                    .contains(&oer_hil_stand_schema::ResetStep::Power)
                    .then(|| crate::control::PowerControl {
                        via: crate::control::PowerVia::Uhubctl,
                        location: hub.usb2.clone(),
                        port: u32::from(board.port.port),
                    }),
            })
        })
        .collect()
}

/// A USB serial port attached now.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AttachedPort {
    pub port: String,
    pub mac: Option<String>,
    pub vid: u16,
    pub pid: u16,
    pub product: Option<String>,
}

/// USB serial ports currently attached to the host.
pub fn attached_ports() -> Vec<AttachedPort> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|port| match port.port_type {
            serialport::SerialPortType::UsbPort(usb) => Some(AttachedPort {
                port: port.port_name,
                mac: usb.serial_number.filter(|serial| !serial.is_empty()),
                vid: usb.vid,
                pid: usb.pid,
                product: usb.product,
            }),
            _ => None,
        })
        .collect()
}

/// The MAC of a board named by its stand-file id, by its chip when it is
/// the only board of that chip, or by its MAC.
pub fn board_mac(devices: &[Device], board: &str) -> crate::Result<String> {
    if let Some(device) = devices.iter().find(|device| device.name == board) {
        return Ok(device.mac.clone());
    }
    let of_chip = devices
        .iter()
        .filter(|device| device.chip == board)
        .collect::<Vec<_>>();
    match of_chip.as_slice() {
        [device] => return Ok(device.mac.clone()),
        [] => {}
        several => {
            let names = several
                .iter()
                .map(|device| device.name.as_str())
                .collect::<Vec<_>>();
            return Err(format!(
                "the stand file has several {board} boards ({}); name one of them",
                names.join(", ")
            )
            .into());
        }
    }
    normalize_mac(board).map_err(|_| {
        format!("board `{board}` is neither a board of the stand file, a chip nor a MAC; see the stand file")
            .into()
    })
}

/// The label of the board with `mac`, or the MAC.
pub fn device_label(devices: &[Device], mac: &str) -> String {
    crate::board::device_label(Some(mac), devices)
}

/// The USB serial number of the port behind `path`, following symlinks such
/// as `/dev/serial/by-id`.
pub fn port_mac(path: &Path) -> Option<String> {
    let canonical = fs::canonicalize(path).ok()?;
    attached_ports().into_iter().find_map(|port| {
        (fs::canonicalize(&port.port).ok().as_ref() == Some(&canonical))
            .then_some(port.mac)
            .flatten()
    })
}

impl Arbiter {
    /// The boards of the stand file.
    pub fn devices(&self) -> crate::Result<Vec<Device>> {
        let file = oer_hil_stand_schema::StandFile::load(self.stand_file())?;
        devices_of(&file)
    }
}

/// Upper-case, colon-separated six-byte MAC, the form the ports report.
pub fn normalize_mac(text: &str) -> crate::Result<String> {
    let digits = text
        .chars()
        .filter(|character| !matches!(character, ':' | '-'))
        .collect::<String>();
    if digits.len() != 12 || !digits.chars().all(|digit| digit.is_ascii_hexdigit()) {
        return Err(format!("`{text}` is not a six-byte MAC address").into());
    }
    Ok(digits
        .to_ascii_uppercase()
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect::<Vec<_>>()
        .join(":"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macs_are_normalized_and_invalid_ones_refused() {
        assert_eq!(
            normalize_mac("38:44:be:aa:25:64").unwrap(),
            "38:44:BE:AA:25:64"
        );
        assert_eq!(normalize_mac("3844BEAA2564").unwrap(), "38:44:BE:AA:25:64");
        for invalid in ["", "38:44", "zz:44:be:aa:25:64", "/dev/ttyACM0"] {
            assert!(normalize_mac(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn boards_are_named_by_id_by_their_only_chip_or_by_mac() {
        let board = |name: &str, chip: &str, mac: &str| Device {
            mac: mac.into(),
            name: name.into(),
            chip: chip.into(),
            power: None,
        };
        let one = [board("c5-a", "esp32c5", "38:44:BE:AA:25:64")];
        assert_eq!(board_mac(&one, "c5-a").unwrap(), "38:44:BE:AA:25:64");
        // A chip names its only board, never one of several.
        assert_eq!(board_mac(&one, "esp32c5").unwrap(), "38:44:BE:AA:25:64");
        let two = [
            board("h2-a", "esp32h2", "AA:AA:AA:AA:AA:01"),
            board("h2-b", "esp32h2", "AA:AA:AA:AA:AA:02"),
        ];
        let error = board_mac(&two, "esp32h2").unwrap_err().to_string();
        assert!(error.contains("h2-a, h2-b"), "{error}");
        assert_eq!(
            board_mac(&one, "30:ed:a0:f3:f6:d0").unwrap(),
            "30:ED:A0:F3:F6:D0"
        );
        assert!(board_mac(&one, "s3").is_err());
    }

    #[test]
    fn the_stand_file_gives_a_board_its_hub_port_when_it_resets_by_power() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        assert!(arbiter.devices().is_err(), "no stand file, no boards");
        let file = oer_hil_stand_schema::StandFile::parse(STAND).unwrap();
        let devices = devices_of(&file).unwrap();
        assert_eq!(devices.len(), 2);
        let s31 = &devices[0];
        assert_eq!(
            (s31.mac.as_str(), s31.label()),
            ("30:ED:A0:F3:F6:D0", "s31-a (esp32s31)".into())
        );
        assert_eq!(s31.power, None, "no power step");
        assert_eq!(
            devices[1].power,
            Some(crate::control::PowerControl {
                via: crate::control::PowerVia::Uhubctl,
                location: "3-8.3".into(),
                port: 2,
            })
        );
        assert_eq!(board_mac(&devices, "c5-a").unwrap(), "38:44:BE:AA:25:64");
        assert_eq!(board_mac(&devices, "esp32c5").unwrap(), "38:44:BE:AA:25:64");
    }

    const STAND: &str = r#"
schema = 1
[stand]
id = "test"
air = "exclusive"
[[hub]]
id = "mid"
usb2 = "3-8.3"
switchable = [2]
[[board]]
id = "s31-a"
usb-serial = "30:ed:a0:f3:f6:d0"
chip = "esp32s31"
radios = ["wifi-2g4"]
roles = ["dut"]
port = { hub = "mid", port = 3 }
reset = ["usb-jtag-rts"]
[[board]]
id = "c5-a"
usb-serial = "38:44:BE:AA:25:64"
chip = "esp32c5"
radios = ["ieee802154"]
roles = ["dut", "peer"]
port = { hub = "mid", port = 2 }
reset = ["jtag", "power"]
"#;
}
