//! Session-bounded, read-only evidence from the laboratory OpenWrt AP.

use std::{
    io::Read as _,
    net::Ipv4Addr,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use crate::{
    Result, qualification::scenario::PhyExpectation, transport::lab_config::OpenWrtConfig,
};

const PRE_WORKLOAD_CHANNEL_SAMPLE: Duration = Duration::from_secs(12);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ChannelUtilization {
    pub(crate) active_millis: u64,
    pub(crate) busy_millis: u64,
    pub(crate) scaled_255: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OpenWrtRxEvidence {
    pub(crate) ingress_packets: u64,
    pub(crate) wireless_packets: u64,
    pub(crate) ingress_interface_rx_packets: u64,
    pub(crate) wireless_interface_tx_packets: u64,
    pub(crate) station_tx_packets: u64,
    pub(crate) station_tx_retries: u64,
    pub(crate) station_tx_failed: u64,
    pub(crate) station_tid0_aqm_drops: u64,
    pub(crate) pre_workload_channel_utilization: Option<ChannelUtilization>,
    pub(crate) channel_width_mhz: u8,
    pub(crate) tx_bitrate: String,
    pub(crate) rx_bitrate: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    station_mac: [u8; 6],
    ingress_rx_packets: u64,
    wireless_tx_packets: u64,
    station_tx_packets: u64,
    station_tx_retries: u64,
    station_tx_failed: u64,
    station_tid0_aqm_drops: u64,
    channel_width_mhz: u8,
    tx_bitrate: String,
    rx_bitrate: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OpenWrtStationLinkEvidence {
    pub(crate) channel_width_mhz: u8,
    pub(crate) tx_bitrate: String,
    pub(crate) rx_bitrate: String,
}

struct Capture {
    name: &'static str,
    child: Child,
}

pub(crate) struct OpenWrtRxCapture {
    config: OpenWrtConfig,
    target: Ipv4Addr,
    expected_phy: PhyExpectation,
    before: Snapshot,
    pre_workload_channel_utilization: Option<ChannelUtilization>,
    ingress: Option<Capture>,
    wireless: Option<Capture>,
}

impl OpenWrtRxCapture {
    pub(crate) fn start(
        config: &OpenWrtConfig,
        target: Ipv4Addr,
        port: u16,
        traffic_duration: Duration,
        expected_phy: PhyExpectation,
        maximum_idle_channel_utilization_255: Option<u8>,
    ) -> Result<Self> {
        let pre_workload_channel_utilization = maximum_idle_channel_utilization_255
            .map(|maximum| -> Result<_> {
                let utilization = measure_channel_utilization(config)?;
                require_pre_workload_channel_utilization(Some(maximum), utilization.scaled_255)?;
                Ok(utilization)
            })
            .transpose()?;
        let before = snapshot(config, target, None)?;
        require_width(expected_phy, before.channel_width_mhz)?;
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
        let early_exit = capture_early_exit(&mut ingress)?.or(capture_early_exit(&mut wireless)?);
        if let Some(error) = early_exit {
            let _ = ingress.child.kill();
            let _ = ingress.child.wait();
            let _ = wireless.child.kill();
            let _ = wireless.child.wait();
            return Err(error.into());
        }
        Ok(Self {
            config: config.clone(),
            target,
            expected_phy,
            before,
            pre_workload_channel_utilization,
            ingress: Some(ingress),
            wireless: Some(wireless),
        })
    }

    pub(crate) fn finish(mut self) -> Result<OpenWrtRxEvidence> {
        let ingress = finish_capture(self.ingress.take().expect("capture owns ingress"))?;
        let wireless = finish_capture(self.wireless.take().expect("capture owns wireless"))?;
        let after = snapshot(&self.config, self.target, Some(self.before.station_mac))?;
        require_width(self.expected_phy, after.channel_width_mhz)?;
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
            station_tid0_aqm_drops: delta(
                "station TID-0 AQM drops",
                self.before.station_tid0_aqm_drops,
                after.station_tid0_aqm_drops,
            )?,
            pre_workload_channel_utilization: self.pre_workload_channel_utilization,
            channel_width_mhz: after.channel_width_mhz,
            tx_bitrate: after.tx_bitrate,
            rx_bitrate: after.rx_bitrate,
        })
    }
}

/// Snapshot the final AP/STA link vector after one measured workload.
pub(crate) fn station_link(
    config: &OpenWrtConfig,
    target: Ipv4Addr,
) -> Result<OpenWrtStationLinkEvidence> {
    let snapshot = snapshot(config, target, None)?;
    Ok(OpenWrtStationLinkEvidence {
        channel_width_mhz: snapshot.channel_width_mhz,
        tx_bitrate: snapshot.tx_bitrate,
        rx_bitrate: snapshot.rx_bitrate,
    })
}

fn snapshot(
    config: &OpenWrtConfig,
    target: Ipv4Addr,
    station_mac: Option<[u8; 6]>,
) -> Result<Snapshot> {
    let mac_assignment = match station_mac {
        Some(mac) => format!(
            "mac='{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}'; ",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        ),
        None => format!(
            "mac=$(ip neigh show {target} | awk 'NR==1 {{print $5}}'); \
             if test -z \"$mac\"; then \
               set -- $(iw dev {} station dump | awk '/^Station / {{print $2}}'); \
               test \"$#\" -eq 1; mac=\"$1\"; \
             fi; ",
            config.wireless_interface
        ),
    };
    let script = format!(
        "set -eu; \
         {mac_assignment}\
         test -n \"$mac\"; \
         stats=$(iw dev {wireless} station get \"$mac\"); \
         printf 'station_mac=%s\n' \"$mac\"; \
         printf 'ingress_rx=%s\\n' \"$(cat /sys/class/net/{ingress}/statistics/rx_packets)\"; \
         printf 'wireless_tx=%s\\n' \"$(cat /sys/class/net/{wireless}/statistics/tx_packets)\"; \
         printf 'station_tx=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx packets:/ {{print $3}}')\"; \
         printf 'station_retries=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx retries:/ {{print $3}}')\"; \
         printf 'station_failed=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/tx failed:/ {{print $3}}')\"; \
         printf 'tx_bitrate=%s\\n' \"$(printf '%s\\n' \"$stats\" | sed -n 's/^[[:space:]]*tx bitrate:[[:space:]]*//p')\"; \
         printf 'rx_bitrate=%s\\n' \"$(printf '%s\\n' \"$stats\" | sed -n 's/^[[:space:]]*rx bitrate:[[:space:]]*//p')\"; \
         set -- /sys/kernel/debug/ieee80211/*/netdev:{wireless}/stations/$mac/aqm; \
         test \"$#\" -eq 1; test -r \"$1\"; \
         printf 'station_tid0_aqm_drops=%s\\n' \"$(awk '$1 == 0 {{print $6}}' \"$1\")\"; \
         printf 'channel_width=%s\\n' \"$(iw dev {wireless} info | sed -n 's/.*width: \\([0-9][0-9]*\\) MHz.*/\\1/p')\"",
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
        station_mac: parse_mac(tagged_text(&stdout, "station_mac")?)?,
        ingress_rx_packets: tagged(&stdout, "ingress_rx")?,
        wireless_tx_packets: tagged(&stdout, "wireless_tx")?,
        station_tx_packets: tagged(&stdout, "station_tx")?,
        station_tx_retries: tagged(&stdout, "station_retries")?,
        station_tx_failed: tagged(&stdout, "station_failed")?,
        station_tid0_aqm_drops: tagged(&stdout, "station_tid0_aqm_drops")?,
        channel_width_mhz: u8::try_from(tagged(&stdout, "channel_width")?)?,
        tx_bitrate: nonempty_tagged_text(&stdout, "tx_bitrate")?.to_owned(),
        rx_bitrate: nonempty_tagged_text(&stdout, "rx_bitrate")?.to_owned(),
    })
}

fn tagged_text<'a>(output: &'a str, key: &str) -> Result<&'a str> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt snapshot omitted `{key}`").into())
}

fn nonempty_tagged_text<'a>(output: &'a str, key: &str) -> Result<&'a str> {
    tagged_text(output, key).and_then(|value| {
        (!value.is_empty())
            .then_some(value)
            .ok_or_else(|| format!("OpenWrt snapshot reported an empty `{key}`").into())
    })
}

fn parse_mac(value: &str) -> Result<[u8; 6]> {
    let mut bytes = [0_u8; 6];
    let mut octets = value.split(':');
    for byte in &mut bytes {
        let octet = octets
            .next()
            .filter(|octet| octet.len() == 2)
            .ok_or_else(|| format!("invalid OpenWrt station MAC `{value}`"))?;
        *byte = u8::from_str_radix(octet, 16)
            .map_err(|_| format!("invalid OpenWrt station MAC `{value}`"))?;
    }
    if octets.next().is_some() {
        return Err(format!("invalid OpenWrt station MAC `{value}`").into());
    }
    Ok(bytes)
}

pub(crate) fn resolve_station_mac(config: &OpenWrtConfig, target: Ipv4Addr) -> Result<String> {
    let script = format!(
        "set -eu; \
         mac=$(ip neigh show {target} | awk 'NR==1 {{print $5}}'); \
         if test -z \"$mac\"; then \
           set -- $(iw dev {} station dump | awk '/^Station / {{print $2}}'); \
           test \"$#\" -eq 1; mac=\"$1\"; \
         fi; \
         printf '%s\\n' \"$mac\"",
        config.wireless_interface
    );
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot resolve the sole associated OpenWrt station: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let mac = String::from_utf8(output.stdout)?
        .trim()
        .to_ascii_lowercase();
    parse_mac(&mac)?;
    Ok(mac)
}

fn require_width(phy: PhyExpectation, observed: u8) -> Result<()> {
    let expected = match phy {
        PhyExpectation::He20 | PhyExpectation::Ht20 => 20,
        PhyExpectation::Ht40 => 40,
    };
    if observed != expected {
        return Err(format!(
            "OpenWrt AP link width changed during the HIL session: expected={expected} observed={observed} MHz"
        )
        .into());
    }
    Ok(())
}

fn require_pre_workload_channel_utilization(maximum: Option<u8>, observed: u8) -> Result<()> {
    if maximum.is_some_and(|maximum| observed > maximum) {
        return Err(format!(
            "OpenWrt pre-workload channel utilization is too high for a ceiling run: observed={observed}/255 maximum={}/255",
            maximum.expect("checked above"),
        )
        .into());
    }
    Ok(())
}

fn measure_channel_utilization(config: &OpenWrtConfig) -> Result<ChannelUtilization> {
    let script = format!(
        "set -eu; \
         before=$(ubus -S call hostapd.{wireless} get_status); \
         active_before=$(jsonfilter -s \"$before\" -e '@.airtime.time'); \
         busy_before=$(jsonfilter -s \"$before\" -e '@.airtime.time_busy'); \
         sleep {seconds}; \
         after=$(ubus -S call hostapd.{wireless} get_status); \
         printf 'active_before=%s\\n' \"$active_before\"; \
         printf 'busy_before=%s\\n' \"$busy_before\"; \
         printf 'active_after=%s\\n' \"$(jsonfilter -s \"$after\" -e '@.airtime.time')\"; \
         printf 'busy_after=%s\\n' \"$(jsonfilter -s \"$after\" -e '@.airtime.time_busy')\"",
        wireless = config.wireless_interface,
        seconds = PRE_WORKLOAD_CHANNEL_SAMPLE.as_secs(),
    );
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot measure OpenWrt pre-workload channel utilization: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    let active_millis = delta(
        "channel active time",
        tagged(&stdout, "active_before")?,
        tagged(&stdout, "active_after")?,
    )?;
    let busy_millis = delta(
        "channel busy time",
        tagged(&stdout, "busy_before")?,
        tagged(&stdout, "busy_after")?,
    )?;
    let scaled_255 = scale_channel_utilization(active_millis, busy_millis)?;
    Ok(ChannelUtilization {
        active_millis,
        busy_millis,
        scaled_255,
    })
}

fn scale_channel_utilization(active_millis: u64, busy_millis: u64) -> Result<u8> {
    if active_millis == 0 || busy_millis > active_millis {
        return Err(format!(
            "invalid OpenWrt channel survey delta: active={active_millis} ms busy={busy_millis} ms"
        )
        .into());
    }
    let scaled = (u128::from(busy_millis) * 255).div_ceil(u128::from(active_millis));
    Ok(u8::try_from(scaled)?)
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
        for capture in [&mut self.ingress, &mut self.wireless]
            .into_iter()
            .flatten()
        {
            let _ = capture.child.kill();
            let _ = capture.child.wait();
        }
    }
}

pub(crate) fn doctor(config: &OpenWrtConfig) -> Result<()> {
    let script = format!(
        "set -eu; command -v tcpdump >/dev/null; command -v timeout >/dev/null; \
         command -v ubus >/dev/null; command -v jsonfilter >/dev/null; \
         test -d /sys/kernel/debug/ieee80211; \
         test -d /sys/class/net/{ingress}; \
         test -d /sys/class/net/{wireless}; \
         iw dev {wireless} info | grep -q 'type AP'; \
         iw dev {wireless} info | grep -q 'width: [24]0 MHz'",
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

fn capture_early_exit(capture: &mut Capture) -> Result<Option<String>> {
    let Some(status) = capture.child.try_wait()? else {
        return Ok(None);
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = capture.child.stderr.take() {
        pipe.read_to_string(&mut stderr)?;
    }
    Ok(Some(format!(
        "{} exited before the HIL session started with {status}: {}",
        capture.name,
        stderr.trim()
    )))
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

    #[test]
    fn station_mac_is_strictly_parsed_for_bridge_capture_reuse() {
        assert_eq!(
            parse_mac("30:ed:a0:f3:f6:d0").unwrap(),
            [0x30, 0xed, 0xa0, 0xf3, 0xf6, 0xd0]
        );
        assert!(parse_mac("30:ed:a0:f3:f6").is_err());
        assert!(parse_mac("30:ed:a0:f3:f6:d0:00").is_err());
        assert!(parse_mac("30:ed:a0:f3:f6:zz").is_err());
    }

    #[test]
    fn ceiling_rejects_busy_pre_workload_channel() {
        require_pre_workload_channel_utilization(Some(64), 64).unwrap();
        assert!(require_pre_workload_channel_utilization(Some(64), 65).is_err());
        require_pre_workload_channel_utilization(None, 255).unwrap();
        assert_eq!(scale_channel_utilization(12_002, 2_837).unwrap(), 61);
        assert_eq!(scale_channel_utilization(12_002, 3_790).unwrap(), 81);
        assert!(scale_channel_utilization(0, 0).is_err());
        assert!(scale_channel_utilization(10, 11).is_err());
    }
}
