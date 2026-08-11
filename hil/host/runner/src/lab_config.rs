//! Typed, host-local description of the physical HIL cell.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use open_esp_radio_hil_protocol::{
    NetworkConfiguration, NetworkCredentials, NetworkIpv4Configuration,
};
use serde::Deserialize;
use zeroize::{Zeroize, Zeroizing};

use crate::{Result, repository_root};

pub(crate) struct LabConfig {
    path: PathBuf,
    pub(crate) device: DeviceConfig,
    pub(crate) station: StationConfig,
    pub(crate) openwrt: OpenWrtConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabConfig {
    device: RawDeviceConfig,
    station: RawStationConfig,
    openwrt: RawOpenWrtConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeviceConfig {
    serial: PathBuf,
    startup_artifact: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct DeviceConfig {
    pub(crate) serial: PathBuf,
    pub(crate) startup_artifact: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStationConfig {
    ssid: String,
    passphrase: String,
    ipv4: RawIpv4Config,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
enum RawIpv4Config {
    Dhcp,
    Static {
        address: String,
        gateway: Option<Ipv4Addr>,
    },
}

pub(crate) struct StationConfig {
    ssid: Zeroizing<String>,
    passphrase: Zeroizing<String>,
    ipv4: NetworkIpv4Configuration,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOpenWrtConfig {
    ssh_target: String,
    wireless_interface: String,
    ingress_interface: String,
    observe_ap: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenWrtConfig {
    pub(crate) ssh_target: String,
    pub(crate) wireless_interface: String,
    pub(crate) ingress_interface: String,
    pub(crate) observe_ap: bool,
}

impl LabConfig {
    pub(crate) fn default_path() -> Result<PathBuf> {
        Ok(repository_root()?.join("hil/local.toml"))
    }

    pub(crate) fn load(path: &Path) -> Result<Self> {
        require_private_permissions(path)?;
        let source = fs::read_to_string(path)
            .map_err(|error| format!("cannot read HIL lab config `{}`: {error}", path.display()))?;
        let mut raw: RawLabConfig = toml::from_str(&source)
            .map_err(|error| format!("invalid HIL lab config `{}`: {error}", path.display()))?;
        if raw.device.serial.as_os_str().is_empty() {
            return Err("HIL lab config defines an empty serial device".into());
        }
        if raw.station.ssid.is_empty() || raw.station.ssid.len() > 32 {
            return Err("HIL station SSID must contain 1..=32 bytes".into());
        }
        if raw.station.passphrase.len() < 8 || raw.station.passphrase.len() > 63 {
            return Err("HIL station passphrase must contain 8..=63 bytes".into());
        }
        let ipv4 = parse_ipv4(raw.station.ipv4.clone())?;
        for (name, value) in [
            ("openwrt.ssh_target", raw.openwrt.ssh_target.as_str()),
            (
                "openwrt.wireless_interface",
                raw.openwrt.wireless_interface.as_str(),
            ),
            (
                "openwrt.ingress_interface",
                raw.openwrt.ingress_interface.as_str(),
            ),
        ] {
            validate_shell_token(name, value)?;
        }
        let root = repository_root()?;
        let startup_artifact = raw.device.startup_artifact.map(|path| {
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        });
        let station = StationConfig {
            ssid: Zeroizing::new(std::mem::take(&mut raw.station.ssid)),
            passphrase: Zeroizing::new(std::mem::take(&mut raw.station.passphrase)),
            ipv4,
        };
        Ok(Self {
            path: path.to_owned(),
            device: DeviceConfig {
                serial: raw.device.serial,
                startup_artifact,
            },
            station,
            openwrt: OpenWrtConfig {
                ssh_target: raw.openwrt.ssh_target,
                wireless_interface: raw.openwrt.wireless_interface,
                ingress_interface: raw.openwrt.ingress_interface,
                observe_ap: raw.openwrt.observe_ap,
            },
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            path: PathBuf::from("hil/local.toml"),
            device: DeviceConfig {
                serial: PathBuf::from("/dev/ttyACM0"),
                startup_artifact: None,
            },
            station: StationConfig {
                ssid: Zeroizing::new(String::from("test-network")),
                passphrase: Zeroizing::new(String::from("test-password")),
                ipv4: NetworkIpv4Configuration::Dhcp,
            },
            openwrt: OpenWrtConfig {
                ssh_target: String::from("open-radio-ap"),
                wireless_interface: String::from("phy0-ap0"),
                ingress_interface: String::from("br-lan"),
                observe_ap: true,
            },
        }
    }
}

impl StationConfig {
    pub(crate) fn network_configuration(&self) -> Result<NetworkConfiguration> {
        let credentials =
            NetworkCredentials::try_new(self.ssid.as_bytes(), self.passphrase.as_bytes())
                .map_err(|error| format!("invalid HIL network credentials: {error}"))?;
        Ok(NetworkConfiguration {
            credentials,
            ipv4: self.ipv4,
        })
    }

    pub(crate) fn credentials(&self) -> (&str, &str) {
        (self.ssid.as_str(), self.passphrase.as_str())
    }
}

fn parse_ipv4(raw: RawIpv4Config) -> Result<NetworkIpv4Configuration> {
    let configuration = match raw {
        RawIpv4Config::Dhcp => NetworkIpv4Configuration::Dhcp,
        RawIpv4Config::Static { address, gateway } => {
            let (address, prefix) = address
                .split_once('/')
                .ok_or("station.ipv4.address must be an IPv4 CIDR")?;
            NetworkIpv4Configuration::Static {
                address: address
                    .parse::<Ipv4Addr>()
                    .map_err(|error| format!("invalid station IPv4 address: {error}"))?
                    .octets(),
                prefix_length: prefix
                    .parse::<u8>()
                    .map_err(|error| format!("invalid station IPv4 prefix: {error}"))?,
                gateway: gateway.map(|address| address.octets()),
            }
        }
    };
    if !configuration.validate() {
        return Err("invalid station IPv4 configuration".into());
    }
    Ok(configuration)
}

fn validate_shell_token(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'@'))
    {
        return Err(format!("{name} contains unsupported characters").into());
    }
    Ok(())
}

#[cfg(unix)]
fn require_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let metadata = fs::metadata(path).map_err(|error| {
        format!(
            "cannot inspect HIL lab config `{}`: {error}",
            path.display()
        )
    })?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode != 0o600 {
        return Err(format!(
            "HIL lab config `{}` contains credentials and must have mode 0600 (found {mode:04o})",
            path.display()
        )
        .into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

impl Drop for RawStationConfig {
    fn drop(&mut self) {
        self.ssid.zeroize();
        self.passphrase.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_static_ipv4() {
        let parsed = parse_ipv4(RawIpv4Config::Static {
            address: "192.168.1.182/24".into(),
            gateway: Some(Ipv4Addr::new(192, 168, 1, 1)),
        })
        .unwrap();
        assert_eq!(
            parsed,
            NetworkIpv4Configuration::Static {
                address: [192, 168, 1, 182],
                prefix_length: 24,
                gateway: Some([192, 168, 1, 1]),
            }
        );
    }

    #[test]
    fn rejects_unsafe_fixture_tokens() {
        assert!(validate_shell_token("iface", "phy0-ap0; reboot").is_err());
    }
}
