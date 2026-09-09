//! Managed-interface packet counts around the AP's station-to-host forwarding.
//!
//! These are capture counts, not per-packet ACK evidence. Capture overflow is
//! an error; matching counts alone do not establish matching packet identities.

use super::{OpenWrtConfig, Result};
use crate::fixture::openwrt_capture::RemoteCapture;
use oer_process::CommandExt as _;
use std::{fs, net::Ipv4Addr, path::Path, process::Command, time::Duration};

pub(crate) struct OpenWrtTxCapture {
    wireless: RemoteCapture,
    uplink: RemoteCapture,
    payload_bytes: usize,
}

impl OpenWrtTxCapture {
    pub(crate) fn start(
        config: &OpenWrtConfig,
        target: Ipv4Addr,
        host: Ipv4Addr,
        ports: (u16, u16),
        payload_bytes: usize,
        duration: Duration,
        output: &Path,
    ) -> Result<Self> {
        let filter = data_filter(target, host, ports.0, ports.1, payload_bytes)?;
        let wireless = RemoteCapture::start_managed(
            config,
            &config.wireless_interface,
            output.join("openwrt-wireless-ingress.pcap"),
            &filter,
            duration,
        )?;
        let uplink = RemoteCapture::start_managed(
            config,
            &config.ingress_interface,
            output.join("openwrt-host-egress.pcap"),
            &filter,
            duration,
        )?;
        Ok(Self {
            wireless,
            uplink,
            payload_bytes,
        })
    }

    pub(crate) fn finish(
        mut self,
        output: &Path,
        host_unique_datagrams: Option<u64>,
    ) -> Result<()> {
        let (wireless, wireless_drops) = self.wireless.finish_capture()?;
        let (uplink, uplink_drops) = self.uplink.finish_capture()?;
        let wireless_units = inspect_capture(
            &output.join("openwrt-wireless-ingress.pcap"),
            wireless,
            self.payload_bytes,
        )
        .map_err(crate::fixture::Error::context)?;
        let uplink_units = inspect_capture(
            &output.join("openwrt-host-egress.pcap"),
            uplink,
            self.payload_bytes,
        )
        .map_err(crate::fixture::Error::context)?;
        let coverage = validate_coverage(
            wireless_units.payload_units,
            uplink_units.payload_units,
            host_unique_datagrams,
        );
        fs::write(
            output.join("openwrt-tx-delivery.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": 2,
                "payload_bytes_per_datagram": self.payload_bytes,
                "wireless": wireless_units,
                "host_facing": uplink_units,
                "host_unique_datagrams": host_unique_datagrams,
                "coverage_consistent": coverage.is_ok(),
                "failure": coverage.as_ref().err().map(|error| error.to_string()),
                "wireless_capture_kernel_drops": wireless_drops,
                "host_facing_capture_kernel_drops": uplink_drops,
            }))?,
        )?;
        coverage
    }
}

fn validate_coverage(wireless: u64, uplink: u64, host_unique: Option<u64>) -> Result<()> {
    let host_unique =
        host_unique.ok_or("host delivery unavailable for OpenWrt capture validation")?;
    if wireless < host_unique || uplink < host_unique {
        return Err(crate::fixture::Error::new(format!(
            "OpenWrt capture is incomplete: wireless={wireless} host_facing={uplink} host_unique={host_unique}; a zero capture drop count does not prove complete capture"
        )).into());
    }
    Ok(())
}

fn data_filter(
    target: Ipv4Addr,
    host: Ipv4Addr,
    source_port: u16,
    destination_port: u16,
    payload_bytes: usize,
) -> Result<String> {
    // Validate the configured workload, but retain all sizes on this flow so
    // the PCAP can expose GRO or unexpected control packets instead of hiding them.
    if !(64..=1472).contains(&payload_bytes) {
        return Err("station delivery capture requires a 64..=1472 byte payload".into());
    }
    Ok(format!(
        "udp and src host {target} and dst host {host} and src port {source_port} and dst port {destination_port} and udp[4:2] >= 72"
    ))
}

#[derive(Debug, Default, serde::Serialize)]
struct CaptureUnits {
    captured_packets: u64,
    payload_units: u64,
    coalesced_packets: u64,
    maximum_payload_units_per_packet: u64,
}

fn inspect_capture(
    path: &Path,
    expected_packets: u64,
    payload_bytes: usize,
) -> Result<CaptureUnits> {
    let result = Command::new("tshark")
        .arg("-r")
        .arg(path)
        .args([
            "-T",
            "fields",
            "-e",
            "udp.length",
            "-e",
            "ip.flags.mf",
            "-e",
            "ip.frag_offset",
        ])
        .supervised_output()?;
    if !result.status.success() {
        return Err(format!(
            "cannot decode OpenWrt UDP capture: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    let units = parse_units(&String::from_utf8(result.stdout)?, payload_bytes)?;
    if units.captured_packets != expected_packets {
        return Err("OpenWrt PCAP records do not match the capture process count".into());
    }
    Ok(units)
}

fn parse_units(fields: &str, payload_bytes: usize) -> Result<CaptureUnits> {
    if !(64..=1472).contains(&payload_bytes) {
        return Err("unsupported UDP capture payload size".into());
    }
    let mut result = CaptureUnits::default();
    for line in fields.lines() {
        let mut fields = line.split('\t');
        let length = fields.next().ok_or("missing UDP length")?.parse::<u16>()?;
        if !matches!(fields.next(), Some("0" | "False"))
            || fields.next() != Some("0")
            || fields.next().is_some()
        {
            return Err(
                "fragmented or undecodable UDP capture cannot establish payload units".into(),
            );
        }
        let payload = usize::from(length.checked_sub(8).ok_or("short UDP header")?);
        if payload == 0 || !payload.is_multiple_of(payload_bytes) {
            return Err(
                "captured UDP payload is not a whole number of configured datagrams".into(),
            );
        }
        let units = (payload / payload_bytes) as u64;
        result.captured_packets += 1;
        result.payload_units += units;
        result.coalesced_packets += u64::from(units > 1);
        result.maximum_payload_units_per_packet =
            result.maximum_payload_units_per_packet.max(units);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
