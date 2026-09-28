//! Typed, host-local description of the physical HIL cell.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use oer_hil_protocol::{
    NetworkCredentials, NetworkIpv4Configuration, WifiAccessPointSecurity, WifiChannelWidth,
};
use serde::Deserialize;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    Result,
    lab::link::{PhyExpectation, WifiLabUse},
    repository_root,
};

#[derive(Clone)]
pub struct LabConfig {
    path: PathBuf,
    cell_id: String,
    /// The chip of `device`.
    target: String,
    /// Every device under test by chip, resolved by [`Self::for_target`].
    targets: std::collections::BTreeMap<String, RawDeviceConfig>,
    /// The device under test of `target`.
    pub device: DeviceConfig,
    pub bluetooth_adapter: Option<oer_hil_fixture::bluetooth::model::Adapter>,
    /// The IEEE 802.15.4 reference peer (`hil/peers/esp32c5-ieee802154`).
    pub peer: Option<PeerBoardConfig>,
    pub station: StationConfig,
    pub access_point: AccessPointConfig,
    pub station_fixture: StationFixtureConfig,
    pub air_observer: Option<AirObserverConfig>,
    pub legacy_bss: Option<LegacyBssConfig>,
}

/// The laptop radio may host a non-ERP (802.11b) BSS on the laboratory AP's
/// channel. Its regulatory domain is explicit because the laboratory AP
/// configuration carries none.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyBssConfig {
    pub country: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabConfig {
    lab: RawLabIdentity,
    /// The esp32s31 device under test; the same as `[targets.esp32s31]`.
    device: Option<RawDeviceConfig>,
    /// Devices under test by chip id.
    #[serde(default)]
    targets: std::collections::BTreeMap<String, RawDeviceConfig>,
    bluetooth: Option<RawBluetoothConfig>,
    /// The reference peer board; `[ieee802154_peer]`, its earlier name, is
    /// read the same.
    #[serde(alias = "ieee802154_peer")]
    peer: Option<RawPeerBoardConfig>,
    station: RawStationConfig,
    access_point: RawAccessPointConfig,
    station_fixture: RawStationFixtureConfig,
    air_observer: Option<AirObserverConfig>,
    legacy_bss: Option<LegacyBssConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBluetoothConfig {
    adapter: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPeerBoardConfig {
    /// Defaults to `board`.
    id: Option<String>,
    serial: Option<PathBuf>,
    /// A registered board name or MAC, resolved to its attached port.
    board: Option<String>,
}

/// The reference peer board: a stable identity and its serial port. Each
/// scenario that uses it names the catalog image it must carry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerBoardConfig {
    pub id: String,
    /// The peer's port, or why its board has none: a board that is not
    /// attached fails only the runs that use the peer.
    port: std::result::Result<PathBuf, String>,
}

impl PeerBoardConfig {
    pub fn new(id: String, serial: PathBuf) -> Self {
        Self {
            id,
            port: Ok(serial),
        }
    }

    /// The peer's serial port; an error when its board is not attached.
    pub fn serial(&self) -> Result<PathBuf> {
        self.port
            .clone()
            .map_err(|error| format!("peer board: {error}").into())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLabIdentity {
    id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeviceConfig {
    id: String,
    serial: Option<PathBuf>,
    /// A registered board name or MAC, resolved to its attached port.
    board: Option<String>,
    startup_artifact: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct DeviceConfig {
    pub id: String,
    pub serial: PathBuf,
    pub startup_artifact: Option<PathBuf>,
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

#[derive(Clone)]
pub struct StationConfig {
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

#[derive(Clone)]
pub struct AccessPointConfig {
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
        country: String,
        channel: u8,
        address: String,
        #[serde(default)]
        coexistence: Coexistence,
    },
    OpenWrt {
        radio: String,
        ap_section: String,
        channel: u8,
        ssh_target: String,
        wireless_interface: String,
        ingress_interface: String,
        monitor_interface: Option<String>,
        phys: Vec<PhyExpectation>,
        #[serde(default)]
        independent_laptop_monitor: bool,
        #[serde(default)]
        read_only: bool,
    },
    External {
        phys: Vec<PhyExpectation>,
    },
}

#[derive(Clone, Debug)]
pub enum StationFixtureConfig {
    LocalLinux(LocalLinuxConfig),
    OpenWrt(OpenWrtConfig),
    External(ExternalConfig),
}

#[derive(Clone, Debug)]
pub struct LocalLinuxConfig {
    pub interface: String,
    pub phys: Vec<PhyExpectation>,
    pub country: String,
    pub channel: u8,
    pub ht40_above: bool,
    pub address: Ipv4Addr,
    pub prefix_length: u8,
    pub coexistence: Coexistence,
}

#[derive(Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Coexistence {
    #[default]
    Respect,
    ForceHt40,
}

#[derive(Clone, Debug)]
pub struct OpenWrtConfig {
    pub ht40_above: bool,
    pub radio: String,
    pub ap_section: String,
    pub channel: u8,
    pub ssh_target: String,
    pub wireless_interface: String,
    pub ingress_interface: String,
    pub monitor_interface: Option<String>,
    pub phys: Vec<PhyExpectation>,
    pub independent_laptop_monitor: bool,
    pub read_only: bool,
}

/// Dedicated idle OpenWrt radio used for independent over-the-air evidence.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AirObserverConfig {
    pub ssh_target: String,
    pub phy: String,
    pub interface: String,
}

#[derive(Clone, Debug)]
pub struct ExternalConfig {
    pub phys: Vec<PhyExpectation>,
}

/// `$XDG_CONFIG_HOME/open-esp-radio/lab.toml`, or `~/.config/...`.
pub fn host_config_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => PathBuf::from(
            std::env::var_os("HOME").ok_or("HOME is required to locate the host lab config")?,
        )
        .join(".config"),
    };
    Ok(base.join("open-esp-radio/lab.toml"))
}

/// A section's serial port: an explicit `serial`, or the attached port of
/// its `board`.
fn board_port(
    section: &str,
    serial: Option<PathBuf>,
    board: Option<&str>,
    chip: Option<&str>,
    resolve: &dyn Fn(&str, Option<&str>) -> Result<PathBuf>,
) -> Result<PathBuf> {
    match (serial, board) {
        (Some(serial), None) if !serial.as_os_str().is_empty() => Ok(serial),
        (Some(_), None) => Err(format!("HIL lab config [{section}] has an empty serial").into()),
        (None, Some(board)) => resolve(board, chip),
        (Some(_), Some(_)) => {
            Err(format!("HIL lab config [{section}] names both serial and board").into())
        }
        (None, None) => Err(format!("HIL lab config [{section}] needs serial or board").into()),
    }
}

/// The chip whose device a lab configuration uses unless a run names another.
pub const DEFAULT_TARGET: &str = "esp32s31";

/// The serial port, id and startup artifact of `chip`'s device under test.
fn resolve_target(
    targets: &std::collections::BTreeMap<String, RawDeviceConfig>,
    chip: &str,
    resolve: &dyn Fn(&str, Option<&str>) -> Result<PathBuf>,
) -> Result<(PathBuf, String, Option<PathBuf>)> {
    let device = targets.get(chip).cloned().ok_or_else(|| {
        format!(
            "HIL lab config has no device under test for {chip}; add [targets.{chip}] (configured: {})",
            targets.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    let serial = board_port(
        &format!("targets.{chip}"),
        device.serial,
        device.board.as_deref(),
        Some(chip),
        resolve,
    )?;
    Ok((serial, device.id, device.startup_artifact))
}

/// Resolve a registered board name or MAC to its attached port, requiring
/// `chip` when the registry knows the board's chip.
fn resolve_board(board: &str, chip: Option<&str>) -> Result<PathBuf> {
    let devices = oer_hil_arbiter::Arbiter::open()?.devices()?;
    let mac = oer_hil_arbiter::board_mac(&devices, board)?;
    if let (Some(required), Some(known)) = (
        chip,
        devices
            .iter()
            .find(|device| device.mac == mac)
            .and_then(|device| device.chip.as_deref()),
    ) && required != known
    {
        return Err(format!("board `{board}` is an {known}; this role needs an {required}").into());
    }
    oer_hil_arbiter::attached_ports()
        .into_iter()
        .find(|port| port.mac.as_deref() == Some(mac.as_str()))
        .map(|port| PathBuf::from(port.port))
        .ok_or_else(|| format!("board `{board}` ({mac}) is not attached").into())
}

impl LabConfig {
    /// This checkout's `hil/local.toml` when present, otherwise the host's
    /// shared lab configuration, which every checkout reads.
    pub fn default_path() -> Result<PathBuf> {
        let local = repository_root()?.join("hil/local.toml");
        let host = host_config_path()?;
        if local.exists() {
            if host.exists() {
                eprintln!(
                    "hil: {} overrides the host lab config {}",
                    local.display(),
                    host.display()
                );
            }
            return Ok(local);
        }
        Ok(host)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::load_resolving(path, &resolve_board)
    }

    /// Load with `resolve` mapping a `board` reference and its required chip
    /// to the board's serial port.
    pub(crate) fn load_resolving(
        path: &Path,
        resolve: &dyn Fn(&str, Option<&str>) -> Result<PathBuf>,
    ) -> Result<Self> {
        require_private_permissions(path)?;
        let source = fs::read_to_string(path)
            .map_err(|error| format!("cannot read HIL lab config `{}`: {error}", path.display()))?;
        let mut raw: RawLabConfig = toml::from_str(&source)
            .map_err(|error| format!("invalid HIL lab config `{}`: {error}", path.display()))?;
        if let Some(observer) = &raw.air_observer {
            for (name, value) in [
                ("air_observer.ssh_target", &observer.ssh_target),
                ("air_observer.phy", &observer.phy),
                ("air_observer.interface", &observer.interface),
            ] {
                validate_shell_token(name, value)?;
            }
            if !matches!(raw.station_fixture, RawStationFixtureConfig::OpenWrt { .. }) {
                return Err("independent OpenWrt observer requires a managed OpenWrt AP".into());
            }
        }
        validate_identifier("lab.id", &raw.lab.id)?;
        let root = repository_root()?;
        let mut targets = std::mem::take(&mut raw.targets);
        if let Some(device) = raw.device.take() {
            if targets.contains_key(DEFAULT_TARGET) {
                return Err(format!(
                    "HIL lab config names both [device] and [targets.{DEFAULT_TARGET}]; keep one"
                )
                .into());
            }
            targets.insert(DEFAULT_TARGET.to_owned(), device);
        }
        let supported = oer_chip_profile::supported(&root)?;
        for (chip, device) in &targets {
            if !supported.contains(chip) {
                return Err(format!(
                    "HIL lab config [targets.{chip}] names no supported chip; supported: {}",
                    supported.join(", ")
                )
                .into());
            }
            validate_identifier(&format!("targets.{chip}.id"), &device.id)?;
        }
        let target = match targets.len() {
            _ if targets.contains_key(DEFAULT_TARGET) => DEFAULT_TARGET.to_owned(),
            1 => targets.keys().next().cloned().unwrap_or_default(),
            0 => return Err("HIL lab config needs [device] or a [targets.<chip>] table".into()),
            _ => {
                return Err(format!(
                    "HIL lab config [targets] must include {DEFAULT_TARGET} or name exactly one chip"
                )
                .into());
            }
        };
        let (device_serial, device_id, device_startup_artifact) =
            resolve_target(&targets, &target, resolve)?;
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
            RawStationFixtureConfig::LocalLinux {
                interface,
                phys,
                country,
                channel,
                address,
                ..
            } => {
                validate_shell_token("station_fixture.interface", interface)?;
                if interface != "wlan0" {
                    return Err("the installed local HIL helper currently owns only `wlan0`".into());
                }
                validate_phys("local-linux", phys)?;
                if country.len() != 2 || !country.bytes().all(|byte| byte.is_ascii_uppercase()) {
                    return Err(
                        "local AP country must be a two-letter uppercase country code".into(),
                    );
                }
                if !(1..=13).contains(channel) {
                    return Err("local AP channel must be in 1..=13".into());
                }
                let (address, prefix) = parse_cidr("station_fixture.address", address)?;
                if !(1..=30).contains(&prefix) || address.is_unspecified() || address.is_multicast()
                {
                    return Err(
                        "local AP address requires a unicast subnet with host addresses".into(),
                    );
                }
            }
            RawStationFixtureConfig::OpenWrt {
                radio,
                ap_section,
                channel,
                ssh_target,
                wireless_interface,
                ingress_interface,
                monitor_interface,
                phys,
                independent_laptop_monitor: _,
                read_only: _,
            } => {
                if !(1..=13).contains(channel) {
                    return Err("OpenWrt channel must be in 1..=13".into());
                }
                for (name, value) in [
                    ("station_fixture.radio", radio.as_str()),
                    ("station_fixture.ap_section", ap_section.as_str()),
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
            }
            RawStationFixtureConfig::External { phys } => validate_phys("external", phys)?,
        }
        let startup_artifact = device_startup_artifact.map(|path| {
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
        if let Some(legacy) = &raw.legacy_bss
            && (legacy.country.len() != 2
                || !legacy.country.bytes().all(|byte| byte.is_ascii_uppercase()))
        {
            return Err("legacy BSS country must be a two-letter uppercase country code".into());
        }
        Ok(Self {
            path: path.to_owned(),
            air_observer: raw.air_observer,
            legacy_bss: raw.legacy_bss,
            cell_id: raw.lab.id,
            bluetooth_adapter: raw
                .bluetooth
                .map(|config| config.adapter.parse())
                .transpose()?,
            peer: raw
                .peer
                .map(|config| -> Result<_> {
                    let id = config
                        .id
                        .clone()
                        .or_else(|| config.board.clone())
                        .unwrap_or_default();
                    if id.trim().is_empty() {
                        return Err("IEEE 802.15.4 peer id is empty".into());
                    }
                    // Only the board's attachment is deferred to the runs
                    // that use the peer; a malformed section still fails.
                    let port = match (config.serial, config.board.as_deref()) {
                        (None, Some(board)) => resolve(board, None).map_err(|e| e.to_string()),
                        (serial, board) => Ok(board_port("peer", serial, board, None, resolve)?),
                    };
                    Ok(PeerBoardConfig { id, port })
                })
                .transpose()?,
            target,
            targets,
            device: DeviceConfig {
                id: device_id,
                serial: device_serial,
                startup_artifact,
            },
            station,
            access_point,
            station_fixture: match raw.station_fixture {
                RawStationFixtureConfig::LocalLinux {
                    interface,
                    phys,
                    country,
                    channel,
                    address,
                    coexistence,
                } => {
                    let (address, prefix_length) = parse_cidr("station_fixture.address", &address)?;
                    StationFixtureConfig::LocalLinux(LocalLinuxConfig {
                        interface,
                        phys,
                        country,
                        channel,
                        ht40_above: channel <= 9,
                        address,
                        prefix_length,
                        coexistence,
                    })
                }
                RawStationFixtureConfig::OpenWrt {
                    radio,
                    ap_section,
                    channel,
                    ssh_target,
                    wireless_interface,
                    ingress_interface,
                    monitor_interface,
                    phys,
                    independent_laptop_monitor,
                    read_only,
                } => StationFixtureConfig::OpenWrt(OpenWrtConfig {
                    ht40_above: channel <= 9,
                    radio,
                    ap_section,
                    channel,
                    ssh_target,
                    wireless_interface,
                    ingress_interface,
                    monitor_interface,
                    phys,
                    independent_laptop_monitor,
                    read_only,
                }),
                RawStationFixtureConfig::External { phys } => {
                    StationFixtureConfig::External(ExternalConfig { phys })
                }
            },
        })
    }

    /// The station-fixture link a scenario requires: its own link or, for a
    /// scenario without one, the access point's channel width.
    pub fn fixture_phy(&self, wifi: WifiLabUse) -> PhyExpectation {
        wifi.link.unwrap_or_else(|| {
            if self.access_point.bandwidth_mhz() == 40 {
                PhyExpectation::Ht40
            } else {
                PhyExpectation::Ht20
            }
        })
    }

    /// The frequency range, in kHz, the Wi-Fi link of `wifi` occupies: the
    /// fixture's primary channel and, for HT40, its secondary channel above
    /// or below, with the spectral mask's 1 MHz beyond each 20 MHz channel.
    pub fn wifi_range_khz(&self, wifi: WifiLabUse) -> (u64, u64) {
        let lab = self.resolve(wifi);
        let (channel, above) = match &lab.station_fixture {
            StationFixtureConfig::OpenWrt(config) => (config.channel, config.ht40_above),
            StationFixtureConfig::LocalLinux(config) => (config.channel, config.ht40_above),
            StationFixtureConfig::External(_) => (
                lab.access_point.channel,
                lab.access_point.channel_width == WifiChannelWidth::Mhz40Above,
            ),
        };
        let center: u64 = if channel == 14 {
            2_484_000
        } else {
            2_407_000 + 5_000 * u64::from(channel)
        };
        match lab.fixture_phy(wifi) {
            PhyExpectation::Ht40 if above => (center - 11_000, center + 31_000),
            PhyExpectation::Ht40 => (center - 31_000, center + 11_000),
            PhyExpectation::Ht20 | PhyExpectation::He20 => (center - 11_000, center + 11_000),
        }
    }

    /// Resolve the channel geometry of an access-point scenario: the target
    /// AP adopts the requested link and the fixture follows its channel.
    pub fn resolve(&self, wifi: WifiLabUse) -> Self {
        let mut lab = self.clone();
        if wifi.access_point {
            if let Some(link) = wifi.link {
                lab.access_point.channel_width = match link {
                    PhyExpectation::Ht20 | PhyExpectation::He20 => WifiChannelWidth::Mhz20,
                    PhyExpectation::Ht40
                        if self.access_point.channel_width.bandwidth_mhz() == 40 =>
                    {
                        self.access_point.channel_width
                    }
                    PhyExpectation::Ht40 if self.access_point.channel <= 9 => {
                        WifiChannelWidth::Mhz40Above
                    }
                    PhyExpectation::Ht40 => WifiChannelWidth::Mhz40Below,
                };
            }
            if let StationFixtureConfig::OpenWrt(config) = &mut lab.station_fixture {
                config.channel = lab.access_point.channel;
                config.ht40_above = lab.access_point.channel_width == WifiChannelWidth::Mhz40Above;
            }
            if let StationFixtureConfig::LocalLinux(config) = &mut lab.station_fixture {
                config.channel = lab.access_point.channel;
                config.ht40_above = lab.access_point.channel_width == WifiChannelWidth::Mhz40Above;
            }
        }
        lab
    }

    /// The chip of the device under test this configuration uses.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The chips with a device under test.
    pub fn targets(&self) -> Vec<&str> {
        self.targets.keys().map(String::as_str).collect()
    }

    /// This configuration with `chip`'s device under test, resolving its
    /// board now so an absent board of another chip never fails a load.
    pub fn for_target(&self, chip: &str) -> Result<Self> {
        self.for_target_resolving(chip, &resolve_board)
    }

    pub(crate) fn for_target_resolving(
        &self,
        chip: &str,
        resolve: &dyn Fn(&str, Option<&str>) -> Result<PathBuf>,
    ) -> Result<Self> {
        if chip == self.target {
            return Ok(self.clone());
        }
        let (serial, id, startup_artifact) = resolve_target(&self.targets, chip, resolve)?;
        let root = repository_root()?;
        let mut lab = self.clone();
        lab.target = chip.to_owned();
        lab.device = DeviceConfig {
            id,
            serial,
            startup_artifact: startup_artifact.map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                }
            }),
        };
        Ok(lab)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cell_id(&self) -> &str {
        &self.cell_id
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn for_test() -> Self {
        Self {
            path: PathBuf::from("hil/local.toml"),
            air_observer: None,
            legacy_bss: None,
            cell_id: String::from("test-cell"),
            bluetooth_adapter: None,
            peer: None,
            target: DEFAULT_TARGET.to_owned(),
            targets: std::collections::BTreeMap::new(),
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
                ht40_above: true,
                radio: String::from("radio0"),
                ap_section: String::from("default_radio0"),
                channel: 6,
                ssh_target: String::from("open-radio-ap"),
                wireless_interface: String::from("phy0-ap0"),
                ingress_interface: String::from("br-lan"),
                monitor_interface: Some(String::from("open-radio-mon")),
                phys: vec![PhyExpectation::Ht40],
                independent_laptop_monitor: true,
                read_only: false,
            }),
        }
    }
}

impl StationFixtureConfig {
    pub fn require_phy(&self, phy: PhyExpectation) -> Result<()> {
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
    pub const fn ipv4(&self) -> NetworkIpv4Configuration {
        self.ipv4
    }

    pub fn protocol_credentials(&self) -> Result<NetworkCredentials> {
        NetworkCredentials::try_new(self.ssid.as_bytes(), self.passphrase.as_bytes())
            .map_err(|error| format!("invalid HIL network credentials: {error}").into())
    }

    pub fn credentials(&self) -> (&str, &str) {
        (self.ssid.as_str(), self.passphrase.as_str())
    }
}

impl AccessPointConfig {
    pub fn protocol_request(
        &self,
        security: WifiAccessPointSecurity,
    ) -> Result<oer_hil_protocol::WifiAccessPointRequest> {
        Ok(oer_hil_protocol::WifiAccessPointRequest {
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

    pub fn credentials(&self) -> (&str, &str) {
        (self.ssid.as_str(), self.passphrase.as_str())
    }

    pub const fn channel(&self) -> u8 {
        self.channel
    }

    pub const fn bandwidth_mhz(&self) -> u16 {
        self.channel_width.bandwidth_mhz()
    }

    pub const fn channel_width(&self) -> WifiChannelWidth {
        self.channel_width
    }

    /// Exact primary-channel frequency used to constrain client scanning.
    pub const fn frequency_mhz(&self) -> u16 {
        2_407 + self.channel as u16 * 5
    }

    pub const fn client_limit(&self) -> u8 {
        self.client_limit
    }

    pub const fn target_address(&self) -> Ipv4Addr {
        self.target_address
    }

    pub const fn client_address(&self) -> Ipv4Addr {
        self.client_address
    }

    pub const fn secondary_client_address(&self) -> Option<Ipv4Addr> {
        self.secondary_client_address
    }

    pub fn client_cidr(&self) -> String {
        format!("{}/{}", self.client_address, self.prefix_length)
    }

    pub fn secondary_client_cidr(&self) -> Option<String> {
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
mod tests;
