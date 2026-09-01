//! Typed, host-local description of the physical HIL cell.

use std::{
    cell::Cell,
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use open_esp_radio_hil_protocol::{
    NetworkCredentials, NetworkIpv4Configuration, WifiAccessPointSecurity, WifiChannelWidth,
    WifiDataPlanePlacement, WifiRxAdmissionPolicy, WifiRxChecksumPolicy, WifiRxContinuationPolicy,
    WifiRxDispatchPolicy, WifiTxBufferPolicy, WifiTxUdpChecksumPolicy,
};
use serde::Deserialize;
use zeroize::{Zeroize, Zeroizing};

use crate::{Result, qualification::scenario::PhyExpectation, repository_root};

pub(crate) struct LabConfig {
    path: PathBuf,
    cell_id: String,
    pub(crate) device: DeviceConfig,
    pub(crate) station: StationConfig,
    pub(crate) access_point: AccessPointConfig,
    pub(crate) station_fixture: StationFixtureConfig,
    data_plane: Cell<WifiDataPlanePlacement>,
    rx_checksum: Cell<WifiRxChecksumPolicy>,
    tx_udp_checksum: Cell<WifiTxUdpChecksumPolicy>,
    tx_buffer: Cell<WifiTxBufferPolicy>,
    rx_admission: Cell<WifiRxAdmissionPolicy>,
    rx_dispatch: Cell<WifiRxDispatchPolicy>,
    rx_continuation: Cell<WifiRxContinuationPolicy>,
    l1_cache_counters: Cell<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabConfig {
    lab: RawLabIdentity,
    device: RawDeviceConfig,
    station: RawStationConfig,
    access_point: RawAccessPointConfig,
    station_fixture: RawStationFixtureConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabIdentity {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeviceConfig {
    id: String,
    serial: PathBuf,
    startup_artifact: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct DeviceConfig {
    pub(crate) id: String,
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
    channel_width: WifiChannelWidth,
    #[serde(default = "default_ap_client_limit")]
    client_limit: u8,
    target_address: String,
    client_address: String,
    secondary_client_address: Option<String>,
}

const fn default_ap_client_limit() -> u8 {
    4
}

pub(crate) struct AccessPointConfig {
    ssid: Zeroizing<String>,
    passphrase: Zeroizing<String>,
    channel: u8,
    channel_width: WifiChannelWidth,
    client_limit: u8,
    target_address: Ipv4Addr,
    client_address: Ipv4Addr,
    secondary_client_address: Option<Ipv4Addr>,
    prefix_length: u8,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum RawStationFixtureConfig {
    LocalLinux {
        interface: String,
        phys: Vec<PhyExpectation>,
    },
    OpenWrt {
        ssh_target: String,
        wireless_interface: String,
        ingress_interface: String,
        monitor_interface: Option<String>,
        phys: Vec<PhyExpectation>,
        #[serde(default)]
        independent_laptop_monitor: bool,
    },
    External {
        phys: Vec<PhyExpectation>,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum StationFixtureConfig {
    LocalLinux(LocalLinuxConfig),
    OpenWrt(OpenWrtConfig),
    External(ExternalConfig),
}

#[derive(Clone, Debug)]
pub(crate) struct LocalLinuxConfig {
    pub(crate) interface: String,
    pub(crate) phys: Vec<PhyExpectation>,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenWrtConfig {
    pub(crate) ssh_target: String,
    pub(crate) wireless_interface: String,
    pub(crate) ingress_interface: String,
    pub(crate) monitor_interface: Option<String>,
    pub(crate) phys: Vec<PhyExpectation>,
    pub(crate) independent_laptop_monitor: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ExternalConfig {
    pub(crate) phys: Vec<PhyExpectation>,
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
        validate_identifier("lab.id", &raw.lab.id)?;
        validate_identifier("device.id", &raw.device.id)?;
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
        if !raw
            .access_point
            .channel_width
            .admits_primary(raw.access_point.channel)
        {
            return Err("HIL access-point channel geometry is invalid".into());
        }
        if !(1..=15).contains(&raw.access_point.client_limit) {
            return Err("HIL access-point client_limit must be in 1..=15".into());
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
        let secondary_client_address = raw
            .access_point
            .secondary_client_address
            .as_deref()
            .map(|address| parse_cidr("access_point.secondary_client_address", address))
            .transpose()?;
        if let Some((secondary, prefix)) = secondary_client_address
            && (prefix != target_prefix
                || secondary == target_address
                || secondary == client_address
                || subnet(secondary, prefix) != subnet(target_address, target_prefix))
        {
            return Err(
                "HIL secondary AP client must be a distinct host in the target AP subnet".into(),
            );
        }
        let ipv4 = parse_ipv4(raw.station.ipv4.clone())?;
        match &raw.station_fixture {
            RawStationFixtureConfig::LocalLinux { interface, phys } => {
                validate_shell_token("station_fixture.interface", interface)?;
                if interface != "wlan0" {
                    return Err("the installed local HIL helper currently owns only `wlan0`".into());
                }
                validate_phys("local-linux", phys)?;
            }
            RawStationFixtureConfig::OpenWrt {
                ssh_target,
                wireless_interface,
                ingress_interface,
                monitor_interface,
                phys,
                independent_laptop_monitor: _,
            } => {
                for (name, value) in [
                    ("station_fixture.ssh_target", ssh_target.as_str()),
                    (
                        "station_fixture.wireless_interface",
                        wireless_interface.as_str(),
                    ),
                    (
                        "station_fixture.ingress_interface",
                        ingress_interface.as_str(),
                    ),
                ] {
                    validate_shell_token(name, value)?;
                }
                if let Some(interface) = monitor_interface {
                    validate_shell_token("station_fixture.monitor_interface", interface)?;
                }
                validate_phys("open-wrt", phys)?;
                if phys.as_slice() != [PhyExpectation::Ht40] {
                    return Err(
                        "the laboratory OpenWrt fixture is qualified only for `phys = [\"ht40\"]`; 20 MHz width alone does not prove HE"
                            .into(),
                    );
                }
            }
            RawStationFixtureConfig::External { phys } => validate_phys("external", phys)?,
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
            channel_width: raw.access_point.channel_width,
            client_limit: raw.access_point.client_limit,
            target_address,
            client_address,
            secondary_client_address: secondary_client_address.map(|(address, _)| address),
            prefix_length: target_prefix,
        };
        Ok(Self {
            path: path.to_owned(),
            cell_id: raw.lab.id,
            device: DeviceConfig {
                id: raw.device.id,
                serial: raw.device.serial,
                startup_artifact,
            },
            station,
            access_point,
            station_fixture: match raw.station_fixture {
                RawStationFixtureConfig::LocalLinux { interface, phys } => {
                    StationFixtureConfig::LocalLinux(LocalLinuxConfig { interface, phys })
                }
                RawStationFixtureConfig::OpenWrt {
                    ssh_target,
                    wireless_interface,
                    ingress_interface,
                    monitor_interface,
                    phys,
                    independent_laptop_monitor,
                } => StationFixtureConfig::OpenWrt(OpenWrtConfig {
                    ssh_target,
                    wireless_interface,
                    ingress_interface,
                    monitor_interface,
                    phys,
                    independent_laptop_monitor,
                }),
                RawStationFixtureConfig::External { phys } => {
                    StationFixtureConfig::External(ExternalConfig { phys })
                }
            },
            data_plane: Cell::new(WifiDataPlanePlacement::SplitRadioNetwork),
            rx_checksum: Cell::new(WifiRxChecksumPolicy::Software),
            tx_udp_checksum: Cell::new(WifiTxUdpChecksumPolicy::Software),
            tx_buffer: Cell::new(WifiTxBufferPolicy::DirectDma),
            rx_admission: Cell::new(WifiRxAdmissionPolicy::SynchronousShared),
            rx_dispatch: Cell::new(WifiRxDispatchPolicy::Asynchronous),
            rx_continuation: Cell::new(WifiRxContinuationPolicy::ImmediateSoftwareProbe),
            l1_cache_counters: Cell::new(false),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn cell_id(&self) -> &str {
        &self.cell_id
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            path: PathBuf::from("hil/local.toml"),
            cell_id: String::from("test-cell"),
            device: DeviceConfig {
                id: String::from("test-device"),
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
                channel_width: WifiChannelWidth::Mhz40Above,
                client_limit: 4,
                target_address: Ipv4Addr::new(10, 43, 0, 1),
                client_address: Ipv4Addr::new(10, 43, 0, 2),
                secondary_client_address: Some(Ipv4Addr::new(10, 43, 0, 3)),
                prefix_length: 24,
            },
            station_fixture: StationFixtureConfig::OpenWrt(OpenWrtConfig {
                ssh_target: String::from("open-radio-ap"),
                wireless_interface: String::from("phy0-ap0"),
                ingress_interface: String::from("br-lan"),
                monitor_interface: Some(String::from("open-radio-mon")),
                phys: vec![PhyExpectation::Ht40],
                independent_laptop_monitor: true,
            }),
            data_plane: Cell::new(WifiDataPlanePlacement::SplitRadioNetwork),
            rx_checksum: Cell::new(WifiRxChecksumPolicy::Software),
            tx_udp_checksum: Cell::new(WifiTxUdpChecksumPolicy::Software),
            tx_buffer: Cell::new(WifiTxBufferPolicy::DirectDma),
            rx_admission: Cell::new(WifiRxAdmissionPolicy::SynchronousShared),
            rx_dispatch: Cell::new(WifiRxDispatchPolicy::Asynchronous),
            rx_continuation: Cell::new(WifiRxContinuationPolicy::ImmediateSoftwareProbe),
            l1_cache_counters: Cell::new(false),
        }
    }

    pub(crate) fn set_data_plane(&self, placement: WifiDataPlanePlacement) {
        self.data_plane.set(placement);
    }

    pub(crate) fn data_plane(&self) -> WifiDataPlanePlacement {
        self.data_plane.get()
    }

    pub(crate) fn set_rx_checksum(&self, policy: WifiRxChecksumPolicy) {
        self.rx_checksum.set(policy);
    }

    pub(crate) fn rx_checksum(&self) -> WifiRxChecksumPolicy {
        self.rx_checksum.get()
    }

    pub(crate) fn set_tx_udp_checksum(&self, policy: WifiTxUdpChecksumPolicy) {
        self.tx_udp_checksum.set(policy);
    }

    pub(crate) fn tx_udp_checksum(&self) -> WifiTxUdpChecksumPolicy {
        self.tx_udp_checksum.get()
    }

    pub(crate) fn set_tx_buffer(&self, policy: WifiTxBufferPolicy) {
        self.tx_buffer.set(policy);
    }

    pub(crate) fn tx_buffer(&self) -> WifiTxBufferPolicy {
        self.tx_buffer.get()
    }

    pub(crate) fn set_rx_admission(&self, policy: WifiRxAdmissionPolicy) {
        self.rx_admission.set(policy);
    }

    pub(crate) fn rx_admission(&self) -> WifiRxAdmissionPolicy {
        self.rx_admission.get()
    }

    pub(crate) fn set_rx_dispatch(&self, policy: WifiRxDispatchPolicy) {
        self.rx_dispatch.set(policy);
    }

    pub(crate) fn rx_dispatch(&self) -> WifiRxDispatchPolicy {
        self.rx_dispatch.get()
    }

    pub(crate) fn set_rx_continuation(&self, policy: WifiRxContinuationPolicy) {
        self.rx_continuation.set(policy);
    }

    pub(crate) fn rx_continuation(&self) -> WifiRxContinuationPolicy {
        self.rx_continuation.get()
    }

    pub(crate) fn set_l1_cache_counters(&self, enabled: bool) {
        self.l1_cache_counters.set(enabled);
    }

    pub(crate) fn l1_cache_counters(&self) -> bool {
        self.l1_cache_counters.get()
    }
}

impl StationFixtureConfig {
    pub(crate) fn require_phy(&self, phy: PhyExpectation) -> Result<()> {
        let phys = match self {
            Self::LocalLinux(config) => &config.phys,
            Self::OpenWrt(config) => &config.phys,
            Self::External(config) => &config.phys,
        };
        if !phys.contains(&phy) {
            return Err(format!(
                "station fixture does not advertise the required `{}` PHY; configured capabilities: {:?}",
                phy.id(), phys
            )
            .into());
        }
        Ok(())
    }
}

fn validate_phys(owner: &str, phys: &[PhyExpectation]) -> Result<()> {
    if phys.is_empty() {
        return Err(format!("{owner} station fixture must advertise at least one PHY").into());
    }
    for (index, phy) in phys.iter().enumerate() {
        if phys[..index].contains(phy) {
            return Err(format!("{owner} station fixture advertises `{}` twice", phy.id()).into());
        }
    }
    Ok(())
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
        security: WifiAccessPointSecurity,
    ) -> Result<open_esp_radio_hil_protocol::WifiAccessPointRequest> {
        Ok(open_esp_radio_hil_protocol::WifiAccessPointRequest {
            credentials: NetworkCredentials::try_new(
                self.ssid.as_bytes(),
                self.passphrase.as_bytes(),
            )
            .map_err(|error| format!("invalid HIL AP credentials: {error}"))?,
            security,
            channel: self.channel,
            channel_width: self.channel_width,
            client_limit: self.client_limit,
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

    pub(crate) const fn bandwidth_mhz(&self) -> u16 {
        self.channel_width.bandwidth_mhz()
    }

    pub(crate) const fn channel_width(&self) -> WifiChannelWidth {
        self.channel_width
    }

    /// Exact primary-channel frequency used to constrain client scanning.
    pub(crate) const fn frequency_mhz(&self) -> u16 {
        2_407 + self.channel as u16 * 5
    }

    pub(crate) const fn client_limit(&self) -> u8 {
        self.client_limit
    }

    pub(crate) const fn target_address(&self) -> Ipv4Addr {
        self.target_address
    }

    pub(crate) const fn client_address(&self) -> Ipv4Addr {
        self.client_address
    }

    pub(crate) const fn secondary_client_address(&self) -> Option<Ipv4Addr> {
        self.secondary_client_address
    }

    pub(crate) fn client_cidr(&self) -> String {
        format!("{}/{}", self.client_address, self.prefix_length)
    }

    pub(crate) fn secondary_client_cidr(&self) -> Option<String> {
        self.secondary_client_address
            .map(|address| format!("{address}/{}", self.prefix_length))
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

fn validate_identifier(name: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!(
            "{name} must contain 1..=64 lowercase ASCII letters, digits or hyphens"
        )
        .into());
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

    #[test]
    fn physical_identity_is_stable_and_path_safe() {
        assert!(validate_identifier("lab.id", "berlin-s31-01").is_ok());
        assert!(validate_identifier("lab.id", "Berlin/S31").is_err());
        assert!(validate_identifier("device.id", "").is_err());
    }
}
