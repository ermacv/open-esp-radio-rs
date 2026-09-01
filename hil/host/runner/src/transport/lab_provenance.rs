//! Secret-free, pre-run observations of the physical HIL cell.

use std::{collections::BTreeMap, fs, net::Ipv4Addr, path::Path, process::Command};

use open_esp_radio_hil_protocol::{NetworkIpv4Configuration, WifiChannelWidth};
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    qualification::scenario::PhyExpectation,
    reporting::run::unix_millis,
    transport::lab_config::{LabConfig, OpenWrtConfig, StationFixtureConfig},
};

pub(crate) const LAB_PROVENANCE_SCHEMA: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LabProvenance {
    pub(crate) schema: u16,
    pub(crate) captured_unix_millis: u64,
    pub(crate) definition: LabDefinition,
    pub(crate) host: HostObservation,
    pub(crate) fixture: FixtureObservation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LabDefinition {
    pub(crate) cell_id: String,
    pub(crate) device_id: String,
    pub(crate) station_ipv4: StationIpv4Definition,
    pub(crate) access_point: AccessPointDefinition,
    pub(crate) station_fixture: StationFixtureDefinition,
    /// Network names, credentials and SSH endpoints are deliberately absent.
    pub(crate) sensitive_network_values: SensitiveValueDisposition,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SensitiveValueDisposition {
    Omitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StationIpv4Definition {
    Dhcp,
    Static {
        address: Ipv4Addr,
        prefix_length: u8,
        gateway: Option<Ipv4Addr>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccessPointDefinition {
    pub(crate) channel: u8,
    pub(crate) channel_width: WifiChannelWidth,
    pub(crate) client_limit: u8,
    pub(crate) target_address: Ipv4Addr,
    pub(crate) client_address: Ipv4Addr,
    pub(crate) secondary_client_address: Option<Ipv4Addr>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum StationFixtureDefinition {
    LocalLinux {
        interface: String,
        phys: Vec<PhyExpectation>,
    },
    OpenWrt {
        wireless_interface: String,
        ingress_interface: String,
        monitor_interface: Option<String>,
        phys: Vec<PhyExpectation>,
        independent_laptop_monitor: bool,
    },
    External {
        phys: Vec<PhyExpectation>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostObservation {
    pub(crate) kernel_release: String,
    pub(crate) machine: String,
    pub(crate) os_release: Option<String>,
    pub(crate) boot_id: Option<String>,
    pub(crate) interfaces: Vec<HostInterfaceObservation>,
    pub(crate) ipv4_routes: Vec<HostIpv4Route>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostInterfaceObservation {
    pub(crate) name: String,
    pub(crate) operstate: String,
    pub(crate) mac_address: Option<String>,
    pub(crate) master: Option<String>,
    pub(crate) wireless: bool,
    pub(crate) ipv4_addresses: Vec<String>,
    pub(crate) wireless_link: Option<HostWirelessLink>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostWirelessLink {
    pub(crate) interface_type: Option<String>,
    pub(crate) connected: bool,
    pub(crate) bssid: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostIpv4Route {
    pub(crate) destination: String,
    pub(crate) interface: Option<String>,
    pub(crate) gateway: Option<Ipv4Addr>,
    pub(crate) preferred_source: Option<Ipv4Addr>,
    pub(crate) metric: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum FixtureObservation {
    LocalLinux,
    OpenWrt(Box<OpenWrtObservation>),
    External { managed: bool },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenWrtObservation {
    pub(crate) release: String,
    pub(crate) revision: String,
    pub(crate) kernel_release: String,
    pub(crate) machine: String,
    pub(crate) boot_id: String,
    pub(crate) wireless_interface: String,
    pub(crate) ingress_interface: String,
    pub(crate) ingress_operstate: String,
    pub(crate) ingress_ipv4_addresses: Vec<String>,
    pub(crate) wiphy: u32,
    pub(crate) interface_type: String,
    pub(crate) channel: u8,
    pub(crate) frequency_mhz: u16,
    pub(crate) width_mhz: u16,
    pub(crate) center1_mhz: u16,
    pub(crate) tx_power_milli_dbm: i32,
    pub(crate) country: Option<String>,
    pub(crate) driver: String,
    pub(crate) firmware: Option<String>,
    pub(crate) associated_stations: u16,
    pub(crate) concurrent_interfaces: Vec<OpenWrtInterface>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenWrtInterface {
    pub(crate) name: String,
    pub(crate) interface_type: String,
}

impl LabProvenance {
    pub(crate) fn capture(lab: &LabConfig) -> Result<Self> {
        let definition = LabDefinition::from_config(lab);
        let host = capture_host()?;
        let fixture = match &lab.station_fixture {
            StationFixtureConfig::LocalLinux(_) => FixtureObservation::LocalLinux,
            StationFixtureConfig::OpenWrt(config) => {
                FixtureObservation::OpenWrt(Box::new(capture_openwrt(config)?))
            }
            StationFixtureConfig::External(_) => FixtureObservation::External { managed: false },
        };
        Ok(Self {
            schema: LAB_PROVENANCE_SCHEMA,
            captured_unix_millis: unix_millis()?,
            definition,
            host,
            fixture,
        })
    }

    pub(crate) fn validate_binding(
        &self,
        cell_id: &str,
        device_id: &str,
        run_started_unix_millis: u64,
        run_finished_unix_millis: Option<u64>,
    ) -> Result<()> {
        if self.schema != LAB_PROVENANCE_SCHEMA
            || self.definition.cell_id != cell_id
            || self.definition.device_id != device_id
            || self.captured_unix_millis < run_started_unix_millis
            || run_finished_unix_millis.is_some_and(|finished| self.captured_unix_millis > finished)
        {
            return Err("lab provenance is not bound to its containing HIL run".into());
        }
        let mut host_interfaces = self
            .host
            .interfaces
            .iter()
            .map(|interface| interface.name.as_str())
            .collect::<Vec<_>>();
        host_interfaces.sort_unstable();
        if host_interfaces.is_empty()
            || host_interfaces.windows(2).any(|pair| pair[0] == pair[1])
            || self.host.kernel_release.is_empty()
            || self.host.machine.is_empty()
        {
            return Err("lab provenance has an invalid host observation".into());
        }
        match (&self.definition.station_fixture, &self.fixture) {
            (
                StationFixtureDefinition::LocalLinux { interface, .. },
                FixtureObservation::LocalLinux,
            ) if self.host.interfaces.iter().any(|entry| {
                entry.name == *interface && entry.wireless && entry.wireless_link.is_some()
            }) => {}
            (
                StationFixtureDefinition::OpenWrt {
                    wireless_interface,
                    ingress_interface,
                    phys,
                    ..
                },
                FixtureObservation::OpenWrt(observation),
            ) => {
                let width_matches = phys.iter().any(|phy| match phy {
                    PhyExpectation::He20 | PhyExpectation::Ht20 => observation.width_mhz == 20,
                    PhyExpectation::Ht40 => observation.width_mhz == 40,
                });
                if observation.wireless_interface != *wireless_interface
                    || observation.ingress_interface != *ingress_interface
                    || observation.interface_type != "AP"
                    || !width_matches
                    || !observation.concurrent_interfaces.iter().any(|interface| {
                        interface.name == *wireless_interface
                            && interface.interface_type == observation.interface_type
                    })
                {
                    return Err("lab provenance has an inconsistent OpenWrt observation".into());
                }
            }
            (
                StationFixtureDefinition::External { .. },
                FixtureObservation::External { managed: false },
            ) => {}
            _ => return Err("lab provenance fixture definition and observation disagree".into()),
        }
        Ok(())
    }
}

impl LabDefinition {
    fn from_config(lab: &LabConfig) -> Self {
        let station_ipv4 = match lab.station.ipv4() {
            NetworkIpv4Configuration::Dhcp => StationIpv4Definition::Dhcp,
            NetworkIpv4Configuration::Static {
                address,
                prefix_length,
                gateway,
            } => StationIpv4Definition::Static {
                address: Ipv4Addr::from(address),
                prefix_length,
                gateway: gateway.map(Ipv4Addr::from),
            },
        };
        let station_fixture = match &lab.station_fixture {
            StationFixtureConfig::LocalLinux(config) => StationFixtureDefinition::LocalLinux {
                interface: config.interface.clone(),
                phys: config.phys.clone(),
            },
            StationFixtureConfig::OpenWrt(config) => StationFixtureDefinition::OpenWrt {
                wireless_interface: config.wireless_interface.clone(),
                ingress_interface: config.ingress_interface.clone(),
                monitor_interface: config.monitor_interface.clone(),
                phys: config.phys.clone(),
                independent_laptop_monitor: config.independent_laptop_monitor,
            },
            StationFixtureConfig::External(config) => StationFixtureDefinition::External {
                phys: config.phys.clone(),
            },
        };
        Self {
            cell_id: lab.cell_id().to_owned(),
            device_id: lab.device.id.clone(),
            station_ipv4,
            access_point: AccessPointDefinition {
                channel: lab.access_point.channel(),
                channel_width: lab.access_point.channel_width(),
                client_limit: lab.access_point.client_limit(),
                target_address: lab.access_point.target_address(),
                client_address: lab.access_point.client_address(),
                secondary_client_address: lab.access_point.secondary_client_address(),
            },
            station_fixture,
            sensitive_network_values: SensitiveValueDisposition::Omitted,
        }
    }
}

fn capture_host() -> Result<HostObservation> {
    let addresses = host_ipv4_addresses()?;
    let mut interfaces = Vec::new();
    let mut entries = fs::read_dir("/sys/class/net")?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let wireless = path.join("phy80211").exists() || path.join("wireless").exists();
        interfaces.push(HostInterfaceObservation {
            operstate: read_trimmed(path.join("operstate"))?.unwrap_or_default(),
            mac_address: read_trimmed(path.join("address"))?,
            master: fs::read_link(path.join("master")).ok().and_then(|master| {
                master
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            }),
            ipv4_addresses: addresses.get(&name).cloned().unwrap_or_default(),
            wireless_link: wireless
                .then(|| capture_host_wireless_link(&name))
                .transpose()?,
            wireless,
            name,
        });
    }
    Ok(HostObservation {
        kernel_release: command_stdout("uname", &["-r"])?,
        machine: command_stdout("uname", &["-m"])?,
        os_release: os_release(),
        boot_id: read_trimmed("/proc/sys/kernel/random/boot_id")?,
        interfaces,
        ipv4_routes: host_ipv4_routes()?,
    })
}

fn host_ipv4_addresses() -> Result<BTreeMap<String, Vec<String>>> {
    let output = command_output("ip", &["-o", "-4", "addr", "show"])?;
    let mut addresses = BTreeMap::<String, Vec<String>>::new();
    for line in output.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let Some(inet) = fields.iter().position(|field| *field == "inet") else {
            continue;
        };
        if inet < 2 || inet + 1 >= fields.len() {
            return Err(format!("invalid `ip -o -4 addr` record: {line}").into());
        }
        addresses
            .entry(fields[1].trim_end_matches(':').to_owned())
            .or_default()
            .push(fields[inet + 1].to_owned());
    }
    for values in addresses.values_mut() {
        values.sort();
    }
    Ok(addresses)
}

fn host_ipv4_routes() -> Result<Vec<HostIpv4Route>> {
    let output = command_output("ip", &["-o", "-4", "route", "show", "table", "main"])?;
    parse_host_ipv4_routes(&output)
}

fn parse_host_ipv4_routes(output: &str) -> Result<Vec<HostIpv4Route>> {
    let mut routes = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let value_after = |key: &str| {
            fields
                .windows(2)
                .find(|pair| pair[0] == key)
                .map(|pair| pair[1])
        };
        routes.push(HostIpv4Route {
            destination: fields
                .first()
                .ok_or("host IPv4 route has no destination")?
                .to_string(),
            interface: value_after("dev").map(str::to_owned),
            gateway: value_after("via").map(str::parse).transpose()?,
            preferred_source: value_after("src").map(str::parse).transpose()?,
            metric: value_after("metric").map(str::parse).transpose()?,
        });
    }
    Ok(routes)
}

fn capture_host_wireless_link(interface: &str) -> Result<HostWirelessLink> {
    let info = Command::new("iw")
        .args(["dev", interface, "info"])
        .output()?;
    let interface_type = info
        .status
        .success()
        .then(|| {
            let output = String::from_utf8_lossy(&info.stdout);
            tagged_iw_value(&output, "type").map(str::to_owned)
        })
        .flatten();
    let link = Command::new("iw")
        .args(["dev", interface, "link"])
        .output()?;
    if !link.status.success() {
        return Err(format!("cannot inspect wireless link `{interface}`").into());
    }
    let output = String::from_utf8(link.stdout)?;
    let bssid = output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("Connected to ")?
            .split_whitespace()
            .next()
            .map(str::to_owned)
    });
    Ok(HostWirelessLink {
        interface_type,
        connected: bssid.is_some(),
        bssid,
    })
}

fn capture_openwrt(config: &OpenWrtConfig) -> Result<OpenWrtObservation> {
    let script = format!(
        "set -eu; . /etc/openwrt_release; \
         info=$(iw dev {wireless} info); \
         printf 'release=%s\\n' \"$DISTRIB_DESCRIPTION\"; \
         printf 'revision=%s\\n' \"$DISTRIB_REVISION\"; \
         printf 'kernel=%s\\n' \"$(uname -r)\"; \
         printf 'machine=%s\\n' \"$(uname -m)\"; \
         printf 'boot_id=%s\\n' \"$(cat /proc/sys/kernel/random/boot_id)\"; \
         printf 'ingress_operstate=%s\\n' \"$(cat /sys/class/net/{ingress}/operstate)\"; \
         ip -o -4 addr show dev {ingress} | awk '{{print \"ingress_ipv4=\" $4}}'; \
         printf 'wiphy=%s\\n' \"$(printf '%s\\n' \"$info\" | awk '$1 == \"wiphy\" {{print $2; exit}}')\"; \
         printf 'interface_type=%s\\n' \"$(printf '%s\\n' \"$info\" | awk '$1 == \"type\" {{print $2; exit}}')\"; \
         printf 'channel=%s\\n' \"$(printf '%s\\n' \"$info\" | awk '$1 == \"channel\" {{print $2; exit}}')\"; \
         printf 'frequency_mhz=%s\\n' \"$(printf '%s\\n' \"$info\" | sed -n 's/.*(\\([0-9][0-9]*\\) MHz).*/\\1/p')\"; \
         printf 'width_mhz=%s\\n' \"$(printf '%s\\n' \"$info\" | sed -n 's/.*width: \\([0-9][0-9]*\\) MHz.*/\\1/p')\"; \
         printf 'center1_mhz=%s\\n' \"$(printf '%s\\n' \"$info\" | sed -n 's/.*center1: \\([0-9][0-9]*\\) MHz.*/\\1/p')\"; \
         printf 'tx_power_dbm=%s\\n' \"$(printf '%s\\n' \"$info\" | awk '$1 == \"txpower\" {{print $2; exit}}')\"; \
         printf 'country=%s\\n' \"$(iw reg get | sed -n 's/^country \\([A-Z0-9][A-Z0-9]\\):.*/\\1/p' | head -n1)\"; \
         printf 'driver=%s\\n' \"$(basename \"$(readlink -f /sys/class/net/{wireless}/device/driver 2>/dev/null)\" 2>/dev/null || true)\"; \
         printf 'firmware=%s\\n' \"$(ethtool -i {wireless} 2>/dev/null | sed -n 's/^firmware-version: //p' || true)\"; \
         printf 'associated_stations=%s\\n' \"$(iw dev {wireless} station dump | awk '/^Station / {{n++}} END {{print n+0}}')\"; \
         iw dev | awk '$1 == \"Interface\" {{name=$2}} $1 == \"type\" {{print \"vif=\" name \"|\" $2}}'",
        wireless = config.wireless_interface,
        ingress = config.ingress_interface,
    );
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot capture secret-free OpenWrt lab provenance: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let observation = parse_openwrt(
        &String::from_utf8(output.stdout)?,
        &config.wireless_interface,
        &config.ingress_interface,
    )?;
    let expected_widths = config
        .phys
        .iter()
        .map(|phy| match phy {
            PhyExpectation::He20 | PhyExpectation::Ht20 => 20,
            PhyExpectation::Ht40 => 40,
        })
        .collect::<Vec<_>>();
    if !expected_widths.contains(&observation.width_mhz) {
        return Err(format!(
            "OpenWrt pre-run width {} MHz is outside configured fixture PHY widths {:?}",
            observation.width_mhz, expected_widths
        )
        .into());
    }
    Ok(observation)
}

fn parse_openwrt(output: &str, wireless: &str, ingress: &str) -> Result<OpenWrtObservation> {
    let tagged = |key: &str| -> Result<&str> {
        output
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .ok_or_else(|| format!("OpenWrt lab snapshot omitted `{key}`").into())
    };
    let nonempty = |key: &str| -> Result<&str> {
        tagged(key).and_then(|value| {
            (!value.is_empty())
                .then_some(value)
                .ok_or_else(|| format!("OpenWrt lab snapshot reported an empty `{key}`").into())
        })
    };
    let optional = |key: &str| -> Result<Option<String>> {
        Ok(match tagged(key)? {
            "" => None,
            value => Some(value.to_owned()),
        })
    };
    let tx_power_dbm = nonempty("tx_power_dbm")?.parse::<f64>()?;
    if !tx_power_dbm.is_finite() {
        return Err("OpenWrt TX power is not finite".into());
    }
    let mut concurrent_interfaces = output
        .lines()
        .filter_map(|line| line.strip_prefix("vif="))
        .map(|value| -> Result<_> {
            let (name, interface_type) = value
                .split_once('|')
                .ok_or("invalid OpenWrt VIF observation")?;
            if name.is_empty() || interface_type.is_empty() {
                return Err("empty OpenWrt VIF name or type".into());
            }
            Ok(OpenWrtInterface {
                name: name.to_owned(),
                interface_type: interface_type.to_owned(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    concurrent_interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    let ingress_ipv4_addresses = output
        .lines()
        .filter_map(|line| line.strip_prefix("ingress_ipv4="))
        .map(str::to_owned)
        .collect();
    Ok(OpenWrtObservation {
        release: nonempty("release")?.to_owned(),
        revision: nonempty("revision")?.to_owned(),
        kernel_release: nonempty("kernel")?.to_owned(),
        machine: nonempty("machine")?.to_owned(),
        boot_id: nonempty("boot_id")?.to_owned(),
        wireless_interface: wireless.to_owned(),
        ingress_interface: ingress.to_owned(),
        ingress_operstate: nonempty("ingress_operstate")?.to_owned(),
        ingress_ipv4_addresses,
        wiphy: nonempty("wiphy")?.parse()?,
        interface_type: nonempty("interface_type")?.to_owned(),
        channel: nonempty("channel")?.parse()?,
        frequency_mhz: nonempty("frequency_mhz")?.parse()?,
        width_mhz: nonempty("width_mhz")?.parse()?,
        center1_mhz: nonempty("center1_mhz")?.parse()?,
        tx_power_milli_dbm: i32::try_from((tx_power_dbm * 1_000.0).round() as i64)?,
        country: optional("country")?,
        driver: nonempty("driver")?.to_owned(),
        firmware: optional("firmware")?,
        associated_stations: nonempty("associated_stations")?.parse()?,
        concurrent_interfaces,
    })
}

fn tagged_iw_value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next()? == key).then(|| fields.next()).flatten()
    })
}

fn command_stdout(program: &str, arguments: &[&str]) -> Result<String> {
    let output = command_output(program, arguments)?;
    let value = output.trim();
    if value.is_empty() {
        return Err(format!("`{program}` returned empty provenance").into());
    }
    Ok(value.to_owned())
}

fn command_output(program: &str, arguments: &[&str]) -> Result<String> {
    let output = Command::new(program).args(arguments).output()?;
    if !output.status.success() {
        return Err(format!("cannot capture host provenance with `{program}`").into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn read_trimmed(path: impl AsRef<Path>) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn os_release() -> Option<String> {
    let source = fs::read_to_string("/etc/os-release").ok()?;
    source.lines().find_map(|line| {
        line.strip_prefix("PRETTY_NAME=")
            .map(|value| value.trim_matches('"').to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_omits_credentials_and_transport_endpoint() {
        let definition = LabDefinition::from_config(&LabConfig::for_test());
        let json = serde_json::to_string(&definition).unwrap();

        assert!(!json.contains("test-password"));
        assert!(!json.contains("test-network"));
        assert!(!json.contains("open-radio-ap"));
        assert!(json.contains("phy0-ap0"));
        assert!(json.contains("sensitive_network_values"));
    }

    #[test]
    fn host_route_parser_keeps_replay_relevant_fields() {
        let routes = parse_host_ipv4_routes(
            "default via 192.0.2.1 dev enp0 proto dhcp src 192.0.2.10 metric 100\n\
             10.43.0.0/24 dev wlan0 proto kernel scope link src 10.43.0.2\n",
        )
        .unwrap();

        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].destination, "default");
        assert_eq!(routes[0].gateway, Some(Ipv4Addr::new(192, 0, 2, 1)));
        assert_eq!(routes[0].metric, Some(100));
        assert_eq!(routes[1].interface.as_deref(), Some("wlan0"));
        assert_eq!(
            routes[1].preferred_source,
            Some(Ipv4Addr::new(10, 43, 0, 2))
        );
    }

    #[test]
    fn openwrt_parser_records_radio_geometry_and_vifs() {
        let observation = parse_openwrt(
            "release=OpenWrt 24.10.2\n\
             revision=r28739-d9340319c6\n\
             kernel=6.6.93\n\
             machine=aarch64\n\
             boot_id=31e71ea2-7bf5-4f7f-9f24-725cbb6f86fd\n\
             ingress_operstate=up\n\
             ingress_ipv4=192.0.2.1/24\n\
             wiphy=0\n\
             interface_type=AP\n\
             channel=13\n\
             frequency_mhz=2472\n\
             width_mhz=40\n\
             center1_mhz=2462\n\
             tx_power_dbm=20.00\n\
             country=DE\n\
             driver=mt76x2e\n\
             firmware=0.0.00\n\
             associated_stations=1\n\
             vif=phy0-ap0|AP\n\
             vif=open-radio-mon|monitor\n",
            "phy0-ap0",
            "br-lan",
        )
        .unwrap();

        assert_eq!(observation.channel, 13);
        assert_eq!(observation.width_mhz, 40);
        assert_eq!(observation.tx_power_milli_dbm, 20_000);
        assert_eq!(observation.associated_stations, 1);
        assert_eq!(observation.concurrent_interfaces.len(), 2);
    }
}
