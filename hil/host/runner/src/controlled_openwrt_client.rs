//! Scoped second AP client on the laboratory OpenWrt radio.

use std::{
    io::Write as _,
    net::Ipv4Addr,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use zeroize::{Zeroize, Zeroizing};

use crate::{
    Result,
    lab_config::{AccessPointConfig, OpenWrtConfig},
};

// Linux network-interface names contain at most IFNAMSIZ-1 (15) bytes.
// Keeping the fixture name below that boundary avoids an opaque nl80211
// `Attribute failed policy validation` error before association even starts.
const INTERFACE: &str = "or-ap-client";
// A virtual managed interface otherwise inherits the active AP interface's
// address. mt76 accepts the wdev creation but refuses to bring the duplicate
// address up. This locally administered identity belongs only to the scoped
// laboratory client and disappears together with the interface.
const CLIENT_MAC: &str = "02:00:00:00:43:03";
const PID_FILE: &str = "/var/run/open-radio-client.pid";
const CONFIG_FILE: &str = "/var/run/open-radio-client.conf";

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SecondaryClientProbeEvidence {
    pub(crate) transmitted: u32,
    pub(crate) received: u32,
}

pub(crate) struct ControlledOpenWrtClient {
    fixture: OpenWrtConfig,
    restored: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OpenWrtUdpTransmission {
    pub(crate) bytes: u64,
    pub(crate) datagrams: u64,
    pub(crate) elapsed: Duration,
    pub(crate) station_tx_packets: u64,
    pub(crate) station_tx_retries: u64,
    pub(crate) station_tx_failed: u64,
    /// Frames received with an invalid FCS by the radio hosting the client.
    ///
    /// In an AP RX workload this includes control responses sent by the DUT,
    /// most importantly Block ACK frames, even though the UDP payload travels
    /// in the opposite direction.
    pub(crate) radio_rx_fcs_errors: u64,
}

pub(crate) fn doctor(access_point: &AccessPointConfig, fixture: &OpenWrtConfig) -> Result<()> {
    if access_point.secondary_client_cidr().is_none() {
        return Ok(());
    }
    let status = Command::new("sh")
        .args(["-c", "command -v wpa_passphrase >/dev/null"])
        .status()?;
    if !status.success() {
        return Err("wpa_passphrase is required for the second AP client".into());
    }
    let script = format!(
        "set -eu; \
         command -v iw >/dev/null; \
         command -v ip >/dev/null; \
         command -v ping >/dev/null; \
         command -v wpa_supplicant >/dev/null; \
         test -r /sys/class/net/{}/phy80211/name; \
         phy=$(cat /sys/class/net/{}/phy80211/name); \
         iw phy \"$phy\" info | grep -q '#{{ managed }}'; \
         iw dev {} info | grep -q 'channel {} '",
        fixture.wireless_interface,
        fixture.wireless_interface,
        fixture.wireless_interface,
        access_point.channel(),
    );
    let output = ssh(fixture, &script)?;
    if !output.status.success() {
        return Err(format!(
            "OpenWrt fixture cannot host the second AP client: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        )
        .into());
    }
    Ok(())
}

impl ControlledOpenWrtClient {
    pub(crate) fn connect(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
    ) -> Result<Self> {
        let address = access_point.secondary_client_cidr().ok_or(
            "the AP scenario requires a second client but secondary_client_address is absent",
        )?;
        Self::connect_at(access_point, fixture, address)
    }

    pub(crate) fn connect_primary(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
    ) -> Result<Self> {
        Self::connect_at(access_point, fixture, access_point.client_cidr())
    }

    fn connect_at(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
        address: String,
    ) -> Result<Self> {
        let (ssid, passphrase) = access_point.credentials();
        let target = access_point.target_address();
        let frequency_mhz = access_point.frequency_mhz();
        let ssid_hex = encode_hex(ssid.as_bytes());
        let mut psk = derive_psk(ssid, passphrase)?;
        let phy = format!(
            "/sys/class/net/{}/phy80211/name",
            fixture.wireless_interface
        );
        let script = Zeroizing::new(format!(
            "set -eu; \
             phy=$(cat {phy}); \
             if test -f {PID_FILE}; then old_pid=$(cat {PID_FILE}); kill \"$old_pid\" 2>/dev/null || true; for attempt in $(seq 1 5); do kill -0 \"$old_pid\" 2>/dev/null || break; sleep 1; done; fi; \
             iw dev {INTERFACE} del 2>/dev/null || true; \
             rm -f {PID_FILE} {CONFIG_FILE}; \
             iw phy \"$phy\" interface add {INTERFACE} type managed addr {CLIENT_MAC}; \
             ip link set {INTERFACE} up; \
             ip addr add {address} dev {INTERFACE}; \
             umask 077; \
             printf 'ctrl_interface=/var/run/wpa_supplicant\\nnetwork={{\\n  ssid={ssid_hex}\\n  psk={psk}\\n  key_mgmt=WPA-PSK\\n  proto=RSN\\n  pairwise=CCMP\\n  group=CCMP\\n}}\\n' > {CONFIG_FILE}; \
             sed -i '/^}}$/i\\  scan_freq={frequency_mhz}' {CONFIG_FILE}; \
             wpa_supplicant -B -P {PID_FILE} -i {INTERFACE} -c {CONFIG_FILE}; \
             if command -v wpa_cli >/dev/null; then wpa_cli -i {INTERFACE} scan >/dev/null; fi; \
             for attempt in $(seq 1 20); do \
                 if ping -4 -q -I {INTERFACE} -c 1 -W 1 {target} >/dev/null 2>&1; then \
                     exit 0; \
                 fi; \
                 sleep 1; \
             done; \
             iw dev {INTERFACE} link >&2 || true; \
             if command -v wpa_cli >/dev/null; then wpa_cli -i {INTERFACE} status >&2 || true; fi; \
             exit 1",
            psk = psk.as_str(),
        ));
        psk.zeroize();
        let output = ssh(fixture, script.as_str())?;
        if !output.status.success() {
            let _ = restore(fixture);
            return Err(format!(
                "OpenWrt AP client did not associate: status={} stdout={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim(),
            )
            .into());
        }
        // Association is visible before all bridge/driver counters settle.
        // Keep this outside the measured workload and give the target time to
        // publish its controlled-port state.
        thread::sleep(Duration::from_millis(250));
        Ok(Self {
            fixture: fixture.clone(),
            restored: false,
        })
    }

    pub(crate) fn restore(mut self) -> Result<()> {
        restore(&self.fixture)?;
        self.restored = true;
        Ok(())
    }

    pub(crate) fn send_udp(
        &self,
        target: Ipv4Addr,
        port: u16,
        rate_bps: u64,
        duration: Duration,
        payload_bytes: usize,
    ) -> Result<OpenWrtUdpTransmission> {
        let duration_seconds = duration.as_secs().max(1);
        let script = format!(
            "set -eu; \
             command -v iperf >/dev/null; \
             phy=$(cat /sys/class/net/{INTERFACE}/phy80211/name); \
             fcs_counter=/sys/kernel/debug/ieee80211/$phy/statistics/dot11FCSErrorCount; \
             test -r \"$fcs_counter\"; \
             mac=$(iw dev {INTERFACE} link | awk '/Connected to/ {{print $3}}'); \
             test -n \"$mac\"; \
             before=$(iw dev {INTERFACE} station get \"$mac\"); \
             before_fcs=$(cat \"$fcs_counter\"); \
             before_packets=$(printf '%s\\n' \"$before\" | awk '/tx packets:/ {{print $3}}'); \
             before_retries=$(printf '%s\\n' \"$before\" | awk '/tx retries:/ {{print $3}}'); \
             before_failed=$(printf '%s\\n' \"$before\" | awk '/tx failed:/ {{print $3}}'); \
             output=$(iperf -c {target} -p {port} -u -b {rate_bps} -t {duration_seconds} -l {payload_bytes} --no-udp-fin -e); \
             datagrams=$(printf '%s\\n' \"$output\" | awk '/Sent [0-9]+ datagrams/ {{for (i=1; i<NF; i++) if ($i == \"Sent\") value=$(i+1)}} END {{print value}}'); \
             test -n \"$datagrams\"; \
             after=$(iw dev {INTERFACE} station get \"$mac\"); \
             after_fcs=$(cat \"$fcs_counter\"); \
             after_packets=$(printf '%s\\n' \"$after\" | awk '/tx packets:/ {{print $3}}'); \
             after_retries=$(printf '%s\\n' \"$after\" | awk '/tx retries:/ {{print $3}}'); \
             after_failed=$(printf '%s\\n' \"$after\" | awk '/tx failed:/ {{print $3}}'); \
             printf 'datagrams=%s\\n' \"$datagrams\"; \
             printf 'station_tx_packets=%s\\n' \"$((after_packets-before_packets))\"; \
             printf 'station_tx_retries=%s\\n' \"$((after_retries-before_retries))\"; \
             printf 'station_tx_failed=%s\\n' \"$((after_failed-before_failed))\"; \
             printf 'radio_rx_fcs_errors=%s\\n' \"$((after_fcs-before_fcs))\""
        );
        let started = Instant::now();
        let output = ssh(&self.fixture, &script)?;
        let elapsed = started.elapsed();
        if !output.status.success() {
            return Err(format!(
                "OpenWrt UDP source failed: status={} stdout={} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim(),
            )
            .into());
        }
        let output = String::from_utf8(output.stdout)?;
        let datagrams = tagged_u64(&output, "datagrams")?;
        Ok(OpenWrtUdpTransmission {
            bytes: datagrams.saturating_mul(u64::try_from(payload_bytes)?),
            datagrams,
            elapsed,
            station_tx_packets: tagged_u64(&output, "station_tx_packets")?,
            station_tx_retries: tagged_u64(&output, "station_tx_retries")?,
            station_tx_failed: tagged_u64(&output, "station_tx_failed")?,
            radio_rx_fcs_errors: tagged_u64(&output, "radio_rx_fcs_errors")?,
        })
    }

    pub(crate) fn spawn_udp_rx_probe(
        &self,
        target: Ipv4Addr,
        port: u16,
    ) -> thread::JoinHandle<std::result::Result<(), String>> {
        let fixture = self.fixture.clone();
        thread::spawn(move || {
            let script = format!(
                "set -eu; command -v socat >/dev/null; \
                 for attempt in $(seq 1 5); do \
                   {{ printf '\\377\\377\\377\\377'; dd if=/dev/zero bs=60 count=1 2>/dev/null; }} \
                     | socat -u STDIN UDP-DATAGRAM:{target}:{port}; \
                   sleep 1; \
                 done"
            );
            let output = ssh(&fixture, &script).map_err(|error| error.to_string())?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "OpenWrt UDP readiness probe failed: status={} stdout={} stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout).trim(),
                    String::from_utf8_lossy(&output.stderr).trim(),
                ))
            }
        })
    }

    /// Exercise this peer throughout the primary saturation workload.
    ///
    /// The probe is deliberately low-bandwidth: it proves that AP scheduling,
    /// pairwise key selection and the network handoff continue to serve a
    /// second peer without contaminating the primary throughput measurement.
    pub(crate) fn spawn_probe(
        &self,
        target: Ipv4Addr,
        duration: Duration,
    ) -> thread::JoinHandle<std::result::Result<SecondaryClientProbeEvidence, String>> {
        let fixture = self.fixture.clone();
        thread::spawn(move || probe(&fixture, target, duration))
    }
}

fn tagged_u64(output: &str, key: &str) -> Result<u64> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt output omitted `{key}`"))?
        .parse()
        .map_err(|error| format!("invalid OpenWrt `{key}` counter: {error}").into())
}

impl Drop for ControlledOpenWrtClient {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore(&self.fixture);
        }
    }
}

fn derive_psk(ssid: &str, passphrase: &str) -> Result<Zeroizing<String>> {
    let mut child = Command::new("wpa_passphrase")
        .arg(ssid)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("wpa_passphrase has no stdin")?
        .write_all(passphrase.as_bytes())?;
    let mut output = child.wait_with_output()?;
    if !output.status.success() {
        output.stdout.zeroize();
        output.stderr.zeroize();
        return Err("wpa_passphrase failed for the secondary AP client".into());
    }
    let text = Zeroizing::new(String::from_utf8(core::mem::take(&mut output.stdout))?);
    let psk = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("psk="))
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("wpa_passphrase did not return a 256-bit PSK")?;
    Ok(Zeroizing::new(psk.to_owned()))
}

fn restore(fixture: &OpenWrtConfig) -> Result<()> {
    let script = format!(
        "set -eu; \
         if test -f {PID_FILE}; then kill $(cat {PID_FILE}) 2>/dev/null || true; fi; \
         iw dev {INTERFACE} del 2>/dev/null || true; \
         rm -f {PID_FILE} {CONFIG_FILE}"
    );
    let output = ssh(fixture, &script)?;
    if !output.status.success() {
        return Err(format!(
            "cannot restore OpenWrt secondary AP client: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        )
        .into());
    }
    Ok(())
}

fn probe(
    fixture: &OpenWrtConfig,
    target: Ipv4Addr,
    duration: Duration,
) -> std::result::Result<SecondaryClientProbeEvidence, String> {
    // OpenWrt's compact BusyBox ping accepts integer-second intervals only.
    // One packet per second is sufficient for this liveness stream and keeps
    // it active for the complete primary saturation interval.
    let packets = duration.as_secs().clamp(2, 300) as u32;
    let deadline = duration.as_secs().saturating_add(5).max(5);
    let script =
        format!("ping -4 -q -I {INTERFACE} -c {packets} -i 1 -W 2 -w {deadline} {target}",);
    let output = ssh(fixture, &script).map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let evidence = parse_ping_summary(&stdout).ok_or_else(|| {
        format!(
            "cannot parse secondary-client ping evidence: stdout={} stderr={} status={}",
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
            output.status,
        )
    })?;
    if !output.status.success() || evidence.transmitted != evidence.received {
        return Err(format!(
            "secondary AP client lost traffic: transmitted={} received={} status={}",
            evidence.transmitted, evidence.received, output.status,
        ));
    }
    Ok(evidence)
}

fn parse_ping_summary(output: &str) -> Option<SecondaryClientProbeEvidence> {
    let summary = output
        .lines()
        .find(|line| line.contains("packets transmitted") && line.contains("packets received"))?;
    let mut fields = summary.split(',');
    let transmitted = fields.next()?.split_whitespace().next()?.parse().ok()?;
    let received = fields.next()?.split_whitespace().next()?.parse().ok()?;
    Some(SecondaryClientProbeEvidence {
        transmitted,
        received,
    })
}

fn ssh(fixture: &OpenWrtConfig, script: &str) -> Result<std::process::Output> {
    Ok(Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&fixture.ssh_target)
        .arg(script)
        .output()?)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssid_is_encoded_without_shell_or_wpa_quoting() {
        assert_eq!(encode_hex(b"lab \\\" ap"), "6c6162205c22206170");
    }

    #[test]
    fn busybox_ping_summary_is_typed() {
        assert_eq!(
            parse_ping_summary(
                "10 packets transmitted, 10 packets received, 0% packet loss\nround-trip min/avg/max = 1.0/2.0/3.0 ms",
            ),
            Some(SecondaryClientProbeEvidence {
                transmitted: 10,
                received: 10,
            }),
        );
    }
}
