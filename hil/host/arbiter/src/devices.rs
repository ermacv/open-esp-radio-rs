//! Boards of the stand, identified by the MAC address that Espressif USB
//! Serial/JTAG ports report as their USB serial number.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::Arbiter;

const REGISTRY_SCHEMA: u32 = 1;

/// A registered board. Unset fields are unknown. Boards have no fixed role:
/// a scenario or other consumer chooses which board it uses in which role.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Device {
    /// USB serial number, the chip's MAC address for USB Serial/JTAG.
    pub mac: String,
    pub chip: Option<String>,
    pub name: Option<String>,
}

impl Device {
    /// `name (chip)`, the name alone when it is the chip, or the MAC when
    /// nothing else is known.
    pub fn label(&self) -> String {
        let name = self.name.as_deref().unwrap_or(&self.mac);
        match &self.chip {
            Some(chip) if chip != name => format!("{name} ({chip})"),
            _ => name.to_owned(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    schema: u32,
    devices: Vec<Device>,
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

/// The MAC of a board named by its registered name, by its chip when it is
/// the only registered board of that chip, or by its MAC.
pub fn board_mac(devices: &[Device], board: &str) -> crate::Result<String> {
    if let Some(device) = devices
        .iter()
        .find(|device| device.name.as_deref() == Some(board))
    {
        return Ok(device.mac.clone());
    }
    let of_chip = devices
        .iter()
        .filter(|device| device.chip.as_deref() == Some(board))
        .collect::<Vec<_>>();
    match of_chip.as_slice() {
        [device] => return Ok(device.mac.clone()),
        [] => {}
        several => {
            let names = several
                .iter()
                .map(|device| device.name.as_deref().unwrap_or(&device.mac))
                .collect::<Vec<_>>();
            return Err(format!(
                "several {board} boards are registered ({}); name one of them",
                names.join(", ")
            )
            .into());
        }
    }
    normalize_mac(board).map_err(|_| {
        format!("board `{board}` is neither a registered name, a chip nor a MAC; see `cargo hil devices`")
            .into()
    })
}

/// The registered label of the board with `mac`, or the MAC.
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
    fn registry_path(&self) -> std::path::PathBuf {
        self.directory().join("devices.json")
    }

    pub fn devices(&self) -> crate::Result<Vec<Device>> {
        let path = self.registry_path();
        self.locked(|| read_registry(&path).map(|registry| registry.devices))
    }

    /// Merge the set fields of `update` into the device with its MAC.
    pub fn set_device(&self, update: Device) -> crate::Result<Device> {
        self.update_device(update, true)
    }

    /// Fill only fields that are still unknown.
    pub fn register_device(&self, update: Device) -> crate::Result<Device> {
        self.update_device(update, false)
    }

    fn update_device(&self, update: Device, overwrite: bool) -> crate::Result<Device> {
        let mac = normalize_mac(&update.mac)?;
        let path = self.registry_path();
        self.locked(|| {
            let mut registry = read_registry(&path)?;
            let index = match registry.devices.iter().position(|device| device.mac == mac) {
                Some(index) => index,
                None => {
                    registry.devices.push(Device {
                        mac: mac.clone(),
                        ..Device::default()
                    });
                    registry.devices.len() - 1
                }
            };
            let device = &mut registry.devices[index];
            for (field, value) in [
                (&mut device.chip, update.chip),
                (&mut device.name, update.name),
            ] {
                if value.is_some() && (overwrite || field.is_none()) {
                    *field = value;
                }
            }
            name_by_chip(&mut registry.devices);
            let device = registry.devices[index].clone();
            registry.devices.sort_by(|a, b| a.mac.cmp(&b.mac));
            let temporary = path.with_extension("json.tmp");
            fs::write(&temporary, serde_json::to_vec_pretty(&registry)?)?;
            fs::rename(&temporary, &path)?;
            Ok(device)
        })
    }
}

/// Name boards after their chip: the only board of a chip is the chip, and
/// several boards of one chip are the chip with the last four hexadecimal
/// digits of their MAC, which is also what to write on the board. Names an
/// operator chose otherwise are kept.
fn name_by_chip(devices: &mut [Device]) {
    let chips = devices
        .iter()
        .filter_map(|device| device.chip.clone())
        .collect::<std::collections::BTreeSet<_>>();
    for chip in chips {
        let several = devices
            .iter()
            .filter(|device| device.chip.as_deref() == Some(chip.as_str()))
            .count()
            > 1;
        for device in devices
            .iter_mut()
            .filter(|device| device.chip.as_deref() == Some(chip.as_str()))
        {
            let generated = device
                .name
                .as_deref()
                .is_none_or(|name| name == chip || is_chip_tail_name(name, &chip));
            if generated {
                device.name = Some(if several {
                    format!("{chip}-{}", mac_tail(&device.mac))
                } else {
                    chip.clone()
                });
            }
        }
    }
}

/// The last four hexadecimal digits of a MAC, lower case.
fn mac_tail(mac: &str) -> String {
    let digits = mac.replace(':', "").to_lowercase();
    digits[digits.len().saturating_sub(4)..].to_owned()
}

fn is_chip_tail_name(name: &str, chip: &str) -> bool {
    name.strip_prefix(chip)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|tail| tail.len() == 4 && tail.chars().all(|c| c.is_ascii_hexdigit()))
}

fn read_registry(path: &Path) -> crate::Result<Registry> {
    match fs::read(path) {
        Ok(bytes) => {
            let registry: Registry = serde_json::from_slice(&bytes)?;
            if registry.schema != REGISTRY_SCHEMA {
                return Err(format!(
                    "device registry {} has schema {}; update this checkout",
                    path.display(),
                    registry.schema
                )
                .into());
            }
            Ok(registry)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Registry {
            schema: REGISTRY_SCHEMA,
            devices: Vec::new(),
        }),
        Err(error) => Err(error.into()),
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
    fn boards_are_named_by_registered_name_or_mac() {
        let devices = [Device {
            mac: "38:44:BE:AA:25:64".into(),
            chip: None,
            name: Some("esp32c5".into()),
        }];
        assert_eq!(board_mac(&devices, "esp32c5").unwrap(), "38:44:BE:AA:25:64");
        // A chip names its only registered board, never one of several.
        let chip_only = |name: Option<&str>, mac: &str| Device {
            mac: mac.into(),
            chip: Some("esp32h2".into()),
            name: name.map(Into::into),
        };
        let one = [chip_only(None, "AA:AA:AA:AA:AA:01")];
        assert_eq!(board_mac(&one, "esp32h2").unwrap(), "AA:AA:AA:AA:AA:01");
        let two = [
            chip_only(Some("esp32h2-dut"), "AA:AA:AA:AA:AA:01"),
            chip_only(Some("esp32h2-peer"), "AA:AA:AA:AA:AA:02"),
        ];
        let error = board_mac(&two, "esp32h2").unwrap_err().to_string();
        assert!(error.contains("esp32h2-dut, esp32h2-peer"), "{error}");
        assert_eq!(
            board_mac(&devices, "30:ed:a0:f3:f6:d0").unwrap(),
            "30:ED:A0:F3:F6:D0"
        );
        assert!(board_mac(&devices, "s3").is_err());
    }

    #[test]
    fn boards_are_named_by_chip_and_by_mac_tail_once_a_chip_has_several() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        let register = |mac: &str, chip: &str| {
            arbiter
                .register_device(Device {
                    mac: mac.into(),
                    chip: Some(chip.into()),
                    name: None,
                })
                .unwrap()
        };
        assert_eq!(
            register("38:44:BE:AA:25:64", "esp32c5").name.as_deref(),
            Some("esp32c5")
        );
        arbiter
            .set_device(Device {
                mac: "30:ED:A0:F3:F6:D0".into(),
                chip: Some("esp32s31".into()),
                name: Some("bench-dut".into()),
            })
            .unwrap();
        assert_eq!(
            register("38:44:BE:AA:99:0F", "esp32c5").name.as_deref(),
            Some("esp32c5-990f")
        );
        let names = arbiter
            .devices()
            .unwrap()
            .into_iter()
            .map(|device| device.name.unwrap())
            .collect::<Vec<_>>();
        // An operator's own name stays.
        assert_eq!(names, ["bench-dut", "esp32c5-2564", "esp32c5-990f"]);
    }

    #[test]
    fn set_overwrites_and_register_only_fills_unknown_fields() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        arbiter
            .register_device(Device {
                mac: "30:ed:a0:f3:f6:d0".into(),
                chip: Some("esp32s31".into()),
                name: None,
            })
            .unwrap();
        let device = arbiter
            .register_device(Device {
                mac: "30:ED:A0:F3:F6:D0".into(),
                chip: Some("other".into()),
                name: Some("esp32s31".into()),
            })
            .unwrap();
        assert_eq!(device.chip.as_deref(), Some("esp32s31"));
        assert_eq!(device.label(), "esp32s31");
        arbiter
            .set_device(Device {
                mac: "30:ED:A0:F3:F6:D0".into(),
                chip: Some("esp32c5".into()),
                name: None,
            })
            .unwrap();
        let devices = arbiter.devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].chip.as_deref(), Some("esp32c5"));
        assert_eq!(devices[0].name.as_deref(), Some("esp32s31"));
    }
}
