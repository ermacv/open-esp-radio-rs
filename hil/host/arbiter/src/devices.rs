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
    /// `name (chip)`, or the MAC when nothing else is known.
    pub fn label(&self) -> String {
        let name = self.name.as_deref().unwrap_or(&self.mac);
        match &self.chip {
            Some(chip) => format!("{name} ({chip})"),
            None => name.to_owned(),
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
            let device = device.clone();
            registry.devices.sort_by(|a, b| a.mac.cmp(&b.mac));
            let temporary = path.with_extension("json.tmp");
            fs::write(&temporary, serde_json::to_vec_pretty(&registry)?)?;
            fs::rename(&temporary, &path)?;
            Ok(device)
        })
    }
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
                name: Some("s31".into()),
            })
            .unwrap();
        assert_eq!(device.chip.as_deref(), Some("esp32s31"));
        assert_eq!(device.label(), "s31 (esp32s31)");
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
        assert_eq!(devices[0].name.as_deref(), Some("s31"));
    }
}
