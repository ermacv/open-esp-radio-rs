//! Qualification preflights for the host-side IP topology.

use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::{Result, transport::lab_config::StationFixtureConfig};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BenchmarkIpv4Route {
    interface: String,
    source: Ipv4Addr,
    medium: RouteMedium,
    expected_medium: Option<RouteMedium>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum RouteMedium {
    Ethernet,
    Wireless,
}

#[derive(Debug, Serialize)]
struct HostRouteEvidence<'a> {
    schema: u16,
    target: Ipv4Addr,
    interface: &'a str,
    route_source: Ipv4Addr,
    socket_source: Ipv4Addr,
    medium: RouteMedium,
    expected_medium: Option<RouteMedium>,
    medium_assertion_passed: bool,
    socket_source_assertion_passed: bool,
}

impl BenchmarkIpv4Route {
    pub(crate) fn discover(device: Ipv4Addr, fixture: &StationFixtureConfig) -> Result<Self> {
        reject_overlapping_ipv4_links(device)?;
        let output = Command::new("ip")
            .args(["-4", "route", "get", &device.to_string()])
            .output()
            .map_err(|error| format!("cannot inspect the host route to {device}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cannot inspect the host route to {device}: `ip route get` exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let mut route = parse_ipv4_route(&String::from_utf8(output.stdout)?, device)?;
        route.medium = interface_medium(&route.interface);
        route.expected_medium = match fixture {
            StationFixtureConfig::OpenWrt(_) => Some(RouteMedium::Ethernet),
            StationFixtureConfig::LocalLinux(_) => Some(RouteMedium::Wireless),
            StationFixtureConfig::External(_) => None,
        };
        if route
            .expected_medium
            .is_some_and(|expected| route.medium != expected)
        {
            return Err(format!(
                "qualification route to {device} uses {:?} interface `{}`, expected {:?}",
                route.medium,
                route.interface,
                route.expected_medium.expect("checked above")
            )
            .into());
        }
        Ok(route)
    }

    pub(crate) fn verify_socket_source(&self, actual: Ipv4Addr) -> Result<()> {
        if actual != self.source {
            return Err(format!(
                "benchmark socket selected source {actual}, but the validated `{}` route selected {}",
                self.interface, self.source
            )
            .into());
        }
        Ok(())
    }

    pub(crate) fn record(
        &self,
        output: &Path,
        target: Ipv4Addr,
        socket_source: Ipv4Addr,
    ) -> Result<()> {
        self.verify_socket_source(socket_source)?;
        crate::reporting::run::atomic_json(
            &output.join("host-route.json"),
            &HostRouteEvidence {
                schema: 1,
                target,
                interface: &self.interface,
                route_source: self.source,
                socket_source,
                medium: self.medium,
                expected_medium: self.expected_medium,
                medium_assertion_passed: self
                    .expected_medium
                    .is_none_or(|expected| expected == self.medium),
                socket_source_assertion_passed: true,
            },
        )
    }
}

fn interface_medium(interface: &str) -> RouteMedium {
    let interface = PathBuf::from("/sys/class/net").join(interface);
    if interface.join("phy80211").exists() || interface.join("wireless").exists() {
        RouteMedium::Wireless
    } else {
        RouteMedium::Ethernet
    }
}

fn parse_ipv4_route(output: &str, device: Ipv4Addr) -> Result<BenchmarkIpv4Route> {
    let fields = output
        .lines()
        .next()
        .ok_or_else(|| format!("host route lookup for {device} returned no route"))?
        .split_whitespace()
        .collect::<Vec<_>>();
    let value_after = |key: &str| {
        fields
            .windows(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1])
    };
    let interface = value_after("dev")
        .ok_or_else(|| format!("host route lookup for {device} omitted its interface"))?;
    let source = value_after("src")
        .ok_or_else(|| format!("host route lookup for {device} omitted its source address"))?
        .parse::<Ipv4Addr>()?;
    Ok(BenchmarkIpv4Route {
        interface: interface.to_owned(),
        source,
        medium: RouteMedium::Ethernet,
        expected_medium: None,
    })
}

/// Reject a dual-homed host on the target's L2 subnet.
///
/// With Linux's default ARP-flux policy (`arp_ignore=0`), either interface may
/// answer for the source address selected by the benchmark socket. The target
/// can then legitimately update its neighbor cache to the other interface's
/// MAC. A TX flow addressed to the host's Ethernet IP may consequently return
/// through a co-channel WLAN interface, measuring a second radio hop instead
/// of the DUT-to-fixture link. A qualification cell must expose one
/// unambiguous L2 identity instead of silently measuring that topology.
pub(crate) fn reject_overlapping_ipv4_links(device: Ipv4Addr) -> Result<()> {
    let output = match Command::new("ip")
        .args(["-o", "-4", "addr", "show", "up", "scope", "global"])
        .output()
    {
        Ok(output) if output.status.success() => output.stdout,
        // The runner remains usable on non-Linux hosts. Linux qualification
        // cells have `ip` and therefore get the strict ARP-flux preflight.
        Ok(_) | Err(_) => return Ok(()),
    };
    let links = overlapping_ipv4_links(&String::from_utf8_lossy(&output), device);
    if links.len() > 1 {
        return Err(format!(
            "qualification has multiple active interfaces on the target subnet ({}); disable all but one to prevent ARP flux and a second radio hop",
            links.join(", ")
        )
        .into());
    }
    Ok(())
}

fn overlapping_ipv4_links(output: &str, device: Ipv4Addr) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _index = fields.next()?;
            let name = fields.next()?.trim_end_matches(':');
            fields.find(|field| *field == "inet")?;
            let (address, prefix) = fields.next()?.split_once('/')?;
            let address = address.parse::<Ipv4Addr>().ok()?;
            let prefix = prefix.parse::<u8>().ok()?;
            same_ipv4_subnet(address, device, prefix).then(|| format!("{name}={address}/{prefix}"))
        })
        .collect()
}

fn same_ipv4_subnet(left: Ipv4Addr, right: Ipv4Addr, prefix: u8) -> bool {
    if prefix > 32 {
        return false;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(left) & mask == u32::from(right) & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_link_parser_exposes_arp_flux_risk() {
        let links = overlapping_ipv4_links(
            "2: enp0: <UP> inet 192.168.178.129/24 scope global enp0\n\
             3: wlan0: <UP> inet 192.168.178.107/24 scope global wlan0\n\
             4: tailscale0: <UP> inet 100.64.0.1/32 scope global tailscale0\n",
            Ipv4Addr::new(192, 168, 178, 131),
        );

        assert_eq!(
            links,
            ["enp0=192.168.178.129/24", "wlan0=192.168.178.107/24"]
        );
    }

    #[test]
    fn subnet_comparison_handles_boundary_prefixes() {
        assert!(same_ipv4_subnet(
            Ipv4Addr::new(1, 2, 3, 4),
            Ipv4Addr::new(203, 0, 113, 9),
            0
        ));
        assert!(!same_ipv4_subnet(
            Ipv4Addr::new(1, 2, 3, 4),
            Ipv4Addr::new(1, 2, 3, 5),
            32
        ));
        assert!(!same_ipv4_subnet(
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::LOCALHOST,
            33
        ));
    }

    #[test]
    fn route_parser_binds_interface_and_source() {
        let route = parse_ipv4_route(
            "192.168.178.127 dev enp0s20f0u2u4c2 src 192.168.178.129 uid 1000\n    cache\n",
            Ipv4Addr::new(192, 168, 178, 127),
        )
        .unwrap();

        assert_eq!(
            route,
            BenchmarkIpv4Route {
                interface: String::from("enp0s20f0u2u4c2"),
                source: Ipv4Addr::new(192, 168, 178, 129),
                medium: RouteMedium::Ethernet,
                expected_medium: None,
            }
        );
        route
            .verify_socket_source(Ipv4Addr::new(192, 168, 178, 129))
            .unwrap();
        assert!(
            route
                .verify_socket_source(Ipv4Addr::new(192, 168, 178, 107))
                .is_err()
        );
    }

    #[test]
    fn route_parser_rejects_incomplete_kernel_output() {
        let device = Ipv4Addr::new(192, 168, 178, 127);

        assert!(parse_ipv4_route("", device).is_err());
        assert!(parse_ipv4_route("192.168.178.127 src 192.168.178.129", device).is_err());
        assert!(parse_ipv4_route("192.168.178.127 dev enp0", device).is_err());
    }

    #[test]
    fn route_evidence_is_typed_and_requires_the_bound_socket_source() {
        let output =
            std::env::temp_dir().join(format!("open-radio-host-route-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&output);
        std::fs::create_dir(&output).unwrap();
        let route = BenchmarkIpv4Route {
            interface: String::from("enp0"),
            source: Ipv4Addr::new(192, 0, 2, 10),
            medium: RouteMedium::Ethernet,
            expected_medium: Some(RouteMedium::Ethernet),
        };

        route
            .record(
                &output,
                Ipv4Addr::new(192, 0, 2, 20),
                Ipv4Addr::new(192, 0, 2, 10),
            )
            .unwrap();
        let evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("host-route.json")).unwrap())
                .unwrap();
        assert_eq!(evidence["medium"], "ethernet");
        assert_eq!(evidence["expected_medium"], "ethernet");
        assert_eq!(evidence["socket_source_assertion_passed"], true);
        assert!(
            route
                .record(
                    &output,
                    Ipv4Addr::new(192, 0, 2, 20),
                    Ipv4Addr::new(192, 0, 2, 11),
                )
                .is_err()
        );
        std::fs::remove_dir_all(output).unwrap();
    }
}
