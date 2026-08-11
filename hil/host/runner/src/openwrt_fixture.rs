//! Session-bounded, read-only evidence from the laboratory OpenWrt AP.

use std::{
    net::Ipv4Addr,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use crate::{Result, lab_config::OpenWrtConfig};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OpenWrtRxEvidence {
    pub(crate) ingress_packets: u64,
    pub(crate) wireless_packets: u64,
    pub(crate) ingress_interface_rx_packets: u64,
    pub(crate) wireless_interface_tx_packets: u64,
    pub(crate) station_tx_packets: u64,
    pub(crate) station_tx_retries: u64,
    pub(crate) station_tx_failed: u64,
    pub(crate) firmware_msdu_queued: u64,
    pub(crate) firmware_msdu_dropped: u64,
    pub(crate) firmware_mpdu_requeued: u64,
    pub(crate) firmware_sw_retry_dropped: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Snapshot {
    ingress_rx_packets: u64,
    wireless_tx_packets: u64,
    station_tx_packets: u64,
    station_tx_retries: u64,
    station_tx_failed: u64,
    firmware_msdu_queued: u64,
    firmware_msdu_dropped: u64,
    firmware_mpdu_requeued: u64,
    firmware_sw_retry_dropped: u64,
}

struct Capture {
    name: &'static str,
    child: Child,
}

pub(crate) struct OpenWrtRxCapture {
    config: OpenWrtConfig,
    target: Ipv4Addr,
    before: Snapshot,
    ingress: Option<Capture>,
    wireless: Option<Capture>,
}

impl OpenWrtRxCapture {
    pub(crate) fn start(
        config: &OpenWrtConfig,
        target: Ipv4Addr,
        port: u16,
        traffic_duration: Duration,
    ) -> Result<Self> {
        let before = snapshot(config, target)?;
        let timeout = traffic_duration.saturating_add(Duration::from_secs(3));
        let mut ingress = spawn_capture(
            config,
            "OpenWrt Ethernet ingress",
            &config.ingress_interface,
            target,
            port,
            timeout,
        )?;
        let mut wireless = match spawn_capture(
            config,
            "OpenWrt Wi-Fi egress",
            &config.wireless_interface,
            target,
            port,
            timeout,
        ) {
            Ok(capture) => capture,
            Err(error) => {
                let mut ingress = ingress;
                let _ = ingress.child.kill();
                let _ = ingress.child.wait();
                return Err(error);
            }
        };
        // tcpdump opens the packet socket before entering its capture loop.
        // This guard is outside the measured session and avoids admitting a
        // prefix before both independently owned sockets exist.
        thread::sleep(Duration::from_secs(1));
        if ingress.child.try_wait()?.is_some() || wireless.child.try_wait()?.is_some() {
            return Err("OpenWrt tcpdump exited before the HIL session started".into());
        }
        Ok(Self {
            config: config.clone(),
            target,
            before,
            ingress: Some(ingress),
            wireless: Some(wireless),
        })
    }

    pub(crate) fn finish(mut self) -> Result<OpenWrtRxEvidence> {
        let ingress = finish_capture(self.ingress.take().expect("capture owns ingress"))?;
        let wireless = finish_capture(self.wireless.take().expect("capture owns wireless"))?;
        let after = snapshot(&self.config, self.target)?;
        Ok(OpenWrtRxEvidence {
            ingress_packets: ingress,
            wireless_packets: wireless,
            ingress_interface_rx_packets: delta(
                "ingress interface RX",
                self.before.ingress_rx_packets,
                after.ingress_rx_packets,
            )?,
            wireless_interface_tx_packets: delta(
                "wireless interface TX",
                self.before.wireless_tx_packets,
                after.wireless_tx_packets,
            )?,
            station_tx_packets: delta(
                "station TX",
                self.before.station_tx_packets,
                after.station_tx_packets,
            )?,
            station_tx_retries: delta(
                "station retries",
                self.before.station_tx_retries,
                after.station_tx_retries,
            )?,
            station_tx_failed: delta(
                "station failed",
                self.before.station_tx_failed,
                after.station_tx_failed,
            )?,
            firmware_msdu_queued: delta(
                "firmware MSDU queued",
                self.before.firmware_msdu_queued,
                after.firmware_msdu_queued,
            )?,
            firmware_msdu_dropped: delta(
                "firmware MSDU dropped",
                self.before.firmware_msdu_dropped,
                after.firmware_msdu_dropped,
            )?,
            firmware_mpdu_requeued: delta(
                "firmware MPDU requeued",
                self.before.firmware_mpdu_requeued,
                after.firmware_mpdu_requeued,
            )?,
            firmware_sw_retry_dropped: delta(
                "firmware SW-retry dropped",
                self.before.firmware_sw_retry_dropped,
                after.firmware_sw_retry_dropped,
            )?,
        })
    }
}

fn snapshot(config: &OpenWrtConfig, target: Ipv4Addr) -> Result<Snapshot> {
    let script = format!(
        "set -eu; \
         mac=$(ip neigh show {target} | awk 'NR==1 {{print $5}}'); \
         test -n \"$mac\"; \
         stats=$(iw dev {wireless} station get \"$mac\"); \
         printf 'ingress_rx=%s\\n' \"$(cat /sys/class/net/{ingress}/statistics/rx_packets)\"; \
         printf 'wireless_tx=%s\\n' \"$(cat /sys/class/net/{wireless}/statistics/tx_packets)\"; \
         printf 'station_tx=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx packets:/ {{print $3}}')\"; \
         printf 'station_retries=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx retries:/ {{print $3}}')\"; \
         printf 'station_failed=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx failed:/ {{print $3}}')\"; \
         wiphy=$(iw dev {wireless} info | awk '/wiphy/ {{print \"phy\" $2; exit}}'); \
         test -n \"$wiphy\"; \
         fw=/sys/kernel/debug/ieee80211/$wiphy/ath10k/fw_stats; \
         printf 'fw_msdu_queued=%s\\n' \"$(awk '/^[[:space:]]*MSDU queued/ {{print $3; exit}}' \"$fw\")\"; \
         printf 'fw_msdu_dropped=%s\\n' \"$(awk '/^[[:space:]]*MSDUs dropped/ {{print $3; exit}}' \"$fw\")\"; \
         printf 'fw_mpdu_requeued=%s\\n' \"$(awk '/^[[:space:]]*MPDUs requeued/ {{print $3; exit}}' \"$fw\")\"; \
         printf 'fw_sw_retry_dropped=%s\\n' \"$(awk '/Dropped due to SW retries/ {{print $6; exit}}' \"$fw\")\"",
        ingress = config.ingress_interface,
        wireless = config.wireless_interface,
    );
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot snapshot OpenWrt station counters: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(Snapshot {
        ingress_rx_packets: tagged(&stdout, "ingress_rx")?,
        wireless_tx_packets: tagged(&stdout, "wireless_tx")?,
        station_tx_packets: tagged(&stdout, "station_tx")?,
        station_tx_retries: tagged(&stdout, "station_retries")?,
        station_tx_failed: tagged(&stdout, "station_failed")?,
        firmware_msdu_queued: tagged(&stdout, "fw_msdu_queued")?,
        firmware_msdu_dropped: tagged(&stdout, "fw_msdu_dropped")?,
        firmware_mpdu_requeued: tagged(&stdout, "fw_mpdu_requeued")?,
        firmware_sw_retry_dropped: tagged(&stdout, "fw_sw_retry_dropped")?,
    })
}

fn tagged(output: &str, key: &str) -> Result<u64> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt snapshot omitted `{key}`"))?
        .parse()
        .map_err(|error| format!("invalid OpenWrt `{key}` counter: {error}").into())
}

fn delta(name: &str, before: u64, after: u64) -> Result<u64> {
    after.checked_sub(before).ok_or_else(|| {
        format!("OpenWrt `{name}` counter reset during the HIL session: {before} -> {after}").into()
    })
}

impl Drop for OpenWrtRxCapture {
    fn drop(&mut self) {
        for capture in [&mut self.ingress, &mut self.wireless] {
            if let Some(capture) = capture {
                let _ = capture.child.kill();
                let _ = capture.child.wait();
            }
        }
    }
}

pub(crate) fn doctor(config: &OpenWrtConfig) -> Result<()> {
    let script = format!(
        "set -eu; command -v tcpdump >/dev/null; \
         test -d /sys/class/net/{ingress}; \
         test -d /sys/class/net/{wireless}; \
         wiphy=$(iw dev {wireless} info | awk '/wiphy/ {{print \"phy\" $2; exit}}'); \
         test -n \"$wiphy\"; \
         test -r /sys/kernel/debug/ieee80211/$wiphy/ath10k/fw_stats",
        ingress = config.ingress_interface,
        wireless = config.wireless_interface,
    );
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "OpenWrt fixture doctor failed over noninteractive SSH: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(())
}

fn spawn_capture(
    config: &OpenWrtConfig,
    name: &'static str,
    interface: &str,
    target: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<Capture> {
    let seconds = timeout.as_secs().max(1);
    let filter = format!("udp and dst host {target} and dst port {port}");
    let script = format!(
        "exec timeout -s INT {seconds} tcpdump -i {interface} -n -q -U -w /dev/null '{filter}'"
    );
    let child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start {name}: {error}"))?;
    Ok(Capture { name, child })
}

fn finish_capture(capture: Capture) -> Result<u64> {
    let output = capture.child.wait_with_output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let captured = parse_summary_value(&stderr, "packets captured")
        .ok_or_else(|| format!("{} did not report a packet count: {stderr}", capture.name))?;
    let dropped = parse_summary_value(&stderr, "packets dropped by kernel").unwrap_or(0);
    if dropped != 0 {
        return Err(format!("{} dropped {dropped} captured packets", capture.name).into());
    }
    // BusyBox timeout returns 124 after delivering SIGINT. tcpdump may also
    // translate SIGINT to a normal exit. Any other terminal status is a lost
    // fixture, not valid qualification evidence.
    if !output.status.success() && output.status.code() != Some(124) {
        return Err(format!("{} exited with {}: {stderr}", capture.name, output.status).into());
    }
    Ok(captured)
}

fn parse_summary_value(stderr: &str, suffix: &str) -> Option<u64> {
    stderr.lines().find_map(|line| {
        let line = line.trim();
        let number = line.strip_suffix(suffix)?.trim();
        number.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcpdump_summary_without_using_received_by_filter() {
        let summary =
            "12 packets captured\n25 packets received by filter\n0 packets dropped by kernel\n";
        assert_eq!(parse_summary_value(summary, "packets captured"), Some(12));
        assert_eq!(
            parse_summary_value(summary, "packets dropped by kernel"),
            Some(0)
        );
    }

    #[test]
    fn counter_reset_is_not_misreported_as_a_wrapping_delta() {
        assert_eq!(delta("counter", 10, 14).unwrap(), 4);
        assert!(delta("counter", 14, 10).is_err());
    }
}
