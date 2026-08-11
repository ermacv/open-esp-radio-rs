//! Typed, host-local description of the physical HIL cell.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use open_esp_radio_hil_protocol::{NetworkCredentials, NetworkIpv4Configuration};
use serde::Deserialize;
use zeroize::{Zeroize, Zeroizing};

use crate::{Result, repository_root};

pub(crate) struct LabConfig {
    path: PathBuf,
    pub(crate) device: DeviceConfig,
    pub(crate) station: StationConfig,
    pub(crate) access_point: AccessPointConfig,
    pub(crate) openwrt: OpenWrtConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabConfig {
    device: RawDeviceConfig,
    station: RawStationConfig,
    access_point: RawAccessPointConfig,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccessPointConfig {
    ssid: String,
    passphrase: String,
    channel: u8,
    target_address: String,
    client_address: String,
}

pub(crate) struct AccessPointConfig {
    ssid: Zeroizing<String>,
    passphrase: Zeroizing<String>,
    channel: u8,
    target_address: Ipv4Addr,
    client_address: Ipv4Addr,
    prefix_length: u8,
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
        validate_credentials(
            "access-point",
            &raw.access_point.ssid,
            &raw.access_point.passphrase,
        )?;
        if !(1..=13).contains(&raw.access_point.channel) {
            return Err("HIL access-point channel must be in 1..=13".into());
        }
        let (target_address, target_prefix) = parse_cidr(
            "access_point.target_address",
            &raw.access_point.target_address,
        )?;
        let (client_address, client_prefix) = parse_cidr(
            "access_point.client_address",
            &raw.access_point.client_address,
        )?;
        if target_prefix != client_prefix
            || target_address == client_address
            || subnet(target_address, target_prefix) != subnet(client_address, client_prefix)
        {
            return Err(
                "HIL AP target/client addresses must be distinct hosts in one IPv4 subnet".into(),
            );
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
        let access_point = AccessPointConfig {
            ssid: Zeroizing::new(std::mem::take(&mut raw.access_point.ssid)),
            passphrase: Zeroizing::new(std::mem::take(&mut raw.access_point.passphrase)),
            channel: raw.access_point.channel,
            target_address,
            client_address,
            prefix_length: target_prefix,
        };
        Ok(Self {
            path: path.to_owned(),
            device: DeviceConfig {
                serial: raw.device.serial,
                startup_artifact,
            },
            station,
            access_point,
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
            access_point: AccessPointConfig {
                ssid: Zeroizing::new(String::from("test-device-ap")),
                passphrase: Zeroizing::new(String::from("test-password")),
                channel: 6,
                target_address: Ipv4Addr::new(10, 43, 0, 1),
                client_address: Ipv4Addr::new(10, 43, 0, 2),
                prefix_length: 24,
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
    pub(crate) const fn ipv4(&self) -> NetworkIpv4Configuration {
        self.ipv4
    }

    pub(crate) fn protocol_credentials(&self) -> Result<NetworkCredentials> {
        NetworkCredentials::try_new(self.ssid.as_bytes(), self.passphrase.as_bytes())
            .map_err(|error| format!("invalid HIL network credentials: {error}").into())
    }

    pub(crate) fn credentials(&self) -> (&str, &str) {
        (self.ssid.as_str(), self.passphrase.as_str())
    }
}

impl AccessPointConfig {
    pub(crate) fn protocol_request(
        &self,
    ) -> Result<open_esp_radio_hil_protocol::WifiAccessPointRequest> {
        Ok(open_esp_radio_hil_protocol::WifiAccessPointRequest {
            credentials: NetworkCredentials::try_new(
                self.ssid.as_bytes(),
                self.passphrase.as_bytes(),
            )
            .map_err(|error| format!("invalid HIL AP credentials: {error}"))?,
            channel: self.channel,
            ipv4: NetworkIpv4Configuration::Static {
                address: self.target_address.octets(),
                prefix_length: self.prefix_length,
                gateway: None,
            },
        })
    }

    pub(crate) fn credentials(&self) -> (&str, &str) {
        (self.ssid.as_str(), self.passphrase.as_str())
    }

    pub(crate) const fn channel(&self) -> u8 {
        self.channel
    }

    pub(crate) const fn target_address(&self) -> Ipv4Addr {
        self.target_address
    }

    pub(crate) const fn client_address(&self) -> Ipv4Addr {
        self.client_address
    }

    pub(crate) fn client_cidr(&self) -> String {
        format!("{}/{}", self.client_address, self.prefix_length)
    }
}

fn validate_credentials(name: &str, ssid: &str, passphrase: &str) -> Result<()> {
    if ssid.is_empty() || ssid.len() > 32 || ssid.chars().any(char::is_control) {
        return Err(format!("HIL {name} SSID must contain 1..=32 non-control bytes").into());
    }
    if !(8..=63).contains(&passphrase.len()) || passphrase.chars().any(char::is_control) {
        return Err(format!("HIL {name} passphrase must contain 8..=63 non-control bytes").into());
    }
    Ok(())
}

fn parse_cidr(name: &str, value: &str) -> Result<(Ipv4Addr, u8)> {
    let (address, prefix) = value
        .split_once('/')
        .ok_or_else(|| format!("{name} must be an IPv4 CIDR"))?;
    let address = address
        .parse::<Ipv4Addr>()
        .map_err(|error| format!("invalid {name} address: {error}"))?;
    let prefix = prefix
        .parse::<u8>()
        .map_err(|error| format!("invalid {name} prefix: {error}"))?;
    if prefix > 30 || address.is_unspecified() || address.is_broadcast() {
        return Err(format!("{name} must identify a host in a /0..=/30 subnet").into());
    }
    let host_mask = if prefix == 0 {
        u32::MAX
    } else {
        u32::MAX >> prefix
    };
    let host = u32::from(address) & host_mask;
    if host == 0 || host == host_mask {
        return Err(format!("{name} cannot be the subnet or broadcast address").into());
    }
    Ok((address, prefix))
}

fn subnet(address: Ipv4Addr, prefix: u8) -> u32 {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(address) & mask
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

impl Drop for RawAccessPointConfig {
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
