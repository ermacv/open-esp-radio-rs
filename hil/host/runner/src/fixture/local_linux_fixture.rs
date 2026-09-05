//! Session-bounded evidence from the laptop-owned nl80211 AP.

use oer_process::CommandExt as _;
use oer_process::owned::Child;
use std::{
    net::Ipv4Addr,
    process::{Command, Stdio},
    time::Duration,
};

use crate::{Result, lab::config::LocalLinuxConfig, scenario::PhyExpectation};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalLinuxRxEvidence {
    pub(crate) udp_packets: u64,
    pub(crate) station_tx_packets: u64,
    pub(crate) station_tx_retries: u64,
    pub(crate) station_tx_failed: u64,
    pub(crate) interface_tx_packets: u64,
    pub(crate) station_tx_duration_micros: u64,
    pub(crate) channel_width_mhz: u8,
    pub(crate) tx_bitrate: String,
    pub(crate) rx_bitrate: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalLinuxTxEvidence {
    pub(crate) udp_packets: u64,
    pub(crate) channel_width_mhz: u8,
    pub(crate) tx_bitrate: String,
    pub(crate) rx_bitrate: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    station_tx_packets: u64,
    station_tx_retries: u64,
    station_tx_failed: u64,
    interface_tx_packets: u64,
    station_tx_duration_micros: u64,
    channel_width_mhz: u8,
    tx_bitrate: String,
    rx_bitrate: String,
}

pub(crate) struct LocalLinuxRxCapture {
    config: LocalLinuxConfig,
    target: Ipv4Addr,
    expected_phy: PhyExpectation,
    before: Snapshot,
    capture: LocalPacketCapture,
}

pub(crate) struct LocalLinuxTxCapture {
    config: LocalLinuxConfig,
    target: Ipv4Addr,
    expected_phy: PhyExpectation,
    capture: LocalPacketCapture,
}

struct LocalPacketCapture {
    child: Option<Child>,
}

impl LocalLinuxRxCapture {
    pub(crate) fn start(
        config: &LocalLinuxConfig,
        target: Ipv4Addr,
        port: u16,
        traffic_duration: Duration,
        expected_phy: PhyExpectation,
    ) -> Result<Self> {
        let before = snapshot(config, target)?;
        require_width(expected_phy, before.channel_width_mhz)?;
        let filter = format!("udp and dst host {target} and dst port {port}");
        let capture = LocalPacketCapture::start(&config.interface, &filter, traffic_duration)?;
        Ok(Self {
            config: config.clone(),
            target,
            expected_phy,
            before,
            capture,
        })
    }

    pub(crate) fn finish(self) -> Result<LocalLinuxRxEvidence> {
        let udp_packets = self.capture.finish()?;
        let after = snapshot(&self.config, self.target)?;
        require_width(self.expected_phy, after.channel_width_mhz)?;
        Ok(LocalLinuxRxEvidence {
            udp_packets,
            station_tx_packets: delta(
                "station TX packets",
                self.before.station_tx_packets,
                after.station_tx_packets,
            )?,
            station_tx_retries: delta(
                "station TX retries",
                self.before.station_tx_retries,
                after.station_tx_retries,
            )?,
            station_tx_failed: delta(
                "station TX failed",
                self.before.station_tx_failed,
                after.station_tx_failed,
            )?,
            interface_tx_packets: delta(
                "interface TX packets",
                self.before.interface_tx_packets,
                after.interface_tx_packets,
            )?,
            station_tx_duration_micros: delta(
                "station TX duration",
                self.before.station_tx_duration_micros,
                after.station_tx_duration_micros,
            )?,
            channel_width_mhz: after.channel_width_mhz,
            tx_bitrate: after.tx_bitrate,
            rx_bitrate: after.rx_bitrate,
        })
    }
}

impl LocalLinuxTxCapture {
    pub(crate) fn start(
        config: &LocalLinuxConfig,
        target: Ipv4Addr,
        source_port: u16,
        destination_port: u16,
        traffic_duration: Duration,
        expected_phy: PhyExpectation,
    ) -> Result<Self> {
        let before = snapshot(config, target)?;
        require_width(expected_phy, before.channel_width_mhz)?;
        let filter = format!(
            "udp and src host {target} and src port {source_port} and dst port {destination_port}"
        );
        Ok(Self {
            config: config.clone(),
            target,
            expected_phy,
            capture: LocalPacketCapture::start(&config.interface, &filter, traffic_duration)?,
        })
    }

    pub(crate) fn finish(self) -> Result<LocalLinuxTxEvidence> {
        let udp_packets = self.capture.finish()?;
        let after = snapshot(&self.config, self.target)?;
        require_width(self.expected_phy, after.channel_width_mhz)?;
        Ok(LocalLinuxTxEvidence {
            udp_packets,
            channel_width_mhz: after.channel_width_mhz,
            tx_bitrate: after.tx_bitrate,
            rx_bitrate: after.rx_bitrate,
        })
    }
}

impl LocalPacketCapture {
    fn start(interface: &str, filter: &str, traffic_duration: Duration) -> Result<Self> {
        let timeout = traffic_duration.saturating_add(Duration::from_secs(3));
        let mut child = Command::new("dumpcap")
            .args([
                "-q",
                "-i",
                interface,
                "-f",
                filter,
                "-a",
                &format!("duration:{}", timeout.as_secs().max(1)),
                "-w",
                "/dev/null",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn_owned()
            .map_err(|error| crate::error::context("start packet capture", error))?
            .with_timeout(timeout.saturating_add(Duration::from_secs(10)));
        oer_process::sleep(Duration::from_millis(500))?;
        if child.try_wait()?.is_some() {
            return Err("local AP packet capture exited before the HIL session started".into());
        }
        Ok(Self { child: Some(child) })
    }

    fn finish(mut self) -> Result<u64> {
        let output = self
            .child
            .take()
            .expect("local fixture owns packet capture")
            .wait_with_output()?;
        if !output.status.success() {
            return Err(format!(
                "local AP packet capture exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let summary = String::from_utf8(output.stderr)?;
        let udp_packets = dumpcap_captured(&summary)?;
        let dropped = dumpcap_dropped(&summary)?;
        if dropped != 0 {
            return Err(format!("local AP packet capture dropped {dropped} packets").into());
        }
        Ok(udp_packets)
    }
}

impl Drop for LocalPacketCapture {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            if let Some(capture) = &mut self.child {
                let _ = capture.kill();
                let _ = capture.wait();
            }
        });
    }
}

fn dumpcap_captured(summary: &str) -> Result<u64> {
    summary
        .lines()
        .find_map(|line| line.trim().strip_prefix("Packets captured:"))
        .ok_or("local AP packet capture omitted its packet count")?
        .trim()
        .parse()
        .map_err(|error| format!("invalid local AP captured packet count: {error}").into())
}

fn dumpcap_dropped(summary: &str) -> Result<u64> {
    let counts = summary
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("Packets received/dropped on interface")?
                .split_once(':')
                .map(|(_, counts)| counts.trim())
        })
        .ok_or("local AP packet capture omitted its drop count")?;
    counts
        .split_once('/')
        .and_then(|(_, dropped)| dropped.split_whitespace().next())
        .ok_or("local AP packet capture reported a malformed drop count")?
        .parse()
        .map_err(|error| format!("invalid local AP dropped packet count: {error}").into())
}

fn snapshot(config: &LocalLinuxConfig, target: Ipv4Addr) -> Result<Snapshot> {
    let neighbor = Command::new("ip")
        .args([
            "neigh",
            "show",
            &target.to_string(),
            "dev",
            &config.interface,
        ])
        .supervised_output()?;
    if !neighbor.status.success() {
        return Err("cannot resolve the local-AP station neighbor".into());
    }
    let neighbor = String::from_utf8(neighbor.stdout)?;
    let mac = neighbor
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|pair| (pair[0] == "lladdr").then_some(pair[1]))
        .ok_or("local AP has no resolved station MAC")?;
    let output = Command::new("iw")
        .args(["dev", &config.interface, "station", "get", mac])
        .supervised_output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot snapshot local AP station counters: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let station = String::from_utf8(output.stdout)?;
    let interface = Command::new("iw")
        .args(["dev", &config.interface, "info"])
        .supervised_output()?;
    if !interface.status.success() {
        return Err("cannot read the local AP channel width".into());
    }
    let interface = String::from_utf8(interface.stdout)?;
    let interface_tx_packets = std::fs::read_to_string(format!(
        "/sys/class/net/{}/statistics/tx_packets",
        config.interface
    ))?
    .trim()
    .parse()?;
    Ok(Snapshot {
        station_tx_packets: tagged_u64(&station, "tx packets:")?,
        station_tx_retries: tagged_u64(&station, "tx retries:")?,
        station_tx_failed: tagged_u64(&station, "tx failed:")?,
        interface_tx_packets,
        station_tx_duration_micros: tagged_u64(&station, "tx duration:")?,
        channel_width_mhz: channel_width(&interface)?,
        tx_bitrate: tagged_text(&station, "tx bitrate:")?,
        rx_bitrate: tagged_text(&station, "rx bitrate:")?,
    })
}

fn channel_width(output: &str) -> Result<u8> {
    output
        .lines()
        .find_map(|line| line.split_once("width:").map(|(_, value)| value.trim()))
        .and_then(|value| value.split_whitespace().next())
        .ok_or("local AP interface snapshot omitted channel width")?
        .parse()
        .map_err(|error| format!("invalid local AP channel width: {error}").into())
}

fn require_width(phy: PhyExpectation, observed: u8) -> Result<()> {
    let expected = match phy {
        PhyExpectation::He20 | PhyExpectation::Ht20 => 20,
        PhyExpectation::Ht40 => 40,
    };
    if observed != expected {
        return Err(format!(
            "local AP link width changed during the HIL session: expected={expected} observed={observed} MHz"
        )
        .into());
    }
    Ok(())
}

fn tagged_u64(output: &str, key: &str) -> Result<u64> {
    tagged_text(output, key)?
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("local AP counter `{key}` is empty"))?
        .parse()
        .map_err(|error| format!("invalid local AP counter `{key}`: {error}").into())
}

fn tagged_text(output: &str, key: &str) -> Result<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix(key).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("local AP station snapshot omitted `{key}`").into())
}

fn delta(name: &str, before: u64, after: u64) -> Result<u64> {
    after.checked_sub(before).ok_or_else(|| {
        format!("local AP `{name}` counter reset during the HIL session: {before} -> {after}")
            .into()
    })
}

#[cfg(test)]
mod tests;
