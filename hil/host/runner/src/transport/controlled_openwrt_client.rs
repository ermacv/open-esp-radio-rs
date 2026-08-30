//! Scoped controlled AP client on the laboratory OpenWrt radio.

use std::{
    io::Write as _,
    net::Ipv4Addr,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use zeroize::{Zeroize, Zeroizing};

use open_esp_radio_hil_protocol::WifiAccessPointSecurity;

use crate::{
    Result,
    qualification::scenario::HtGuardIntervalExpectation,
    transport::lab_config::{AccessPointConfig, OpenWrtConfig},
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
const FORWARD_TABLE: &str = "open_radio_hil";
const FORWARD_CHAIN: &str = "open_radio_hil_forward";

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct SecondaryClientProbeEvidence {
    pub(crate) transmitted: u32,
    pub(crate) received: u32,
}

pub(crate) struct ControlledOpenWrtClient {
    fixture: OpenWrtConfig,
    forward_address: Option<Ipv4Addr>,
    restored: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct OpenWrtClientLinkEvidence {
    pub(crate) rx_bytes: u64,
    pub(crate) rx_packets: u64,
    pub(crate) rx_duration_micros: Option<u64>,
    pub(crate) rx_bitrate: Option<String>,
    pub(crate) tx_bytes: u64,
    pub(crate) tx_packets: u64,
    pub(crate) tx_bitrate: Option<String>,
    pub(crate) tx_retries: u64,
    pub(crate) tx_failed: u64,
    pub(crate) tx_duration_micros: u64,
    pub(crate) tid0_aqm_drops: u64,
}

#[derive(Clone)]
struct OpenWrtClientLinkSnapshot {
    rx_bytes: u64,
    rx_packets: u64,
    rx_duration_micros: Option<u64>,
    rx_bitrate: Option<String>,
    tx_bytes: u64,
    tx_packets: u64,
    tx_bitrate: Option<String>,
    tx_retries: u64,
    tx_failed: u64,
    tx_duration_micros: u64,
    tid0_aqm_drops: u64,
}

pub(crate) struct OpenWrtClientLinkObservation {
    fixture: OpenWrtConfig,
    before: OpenWrtClientLinkSnapshot,
}

pub(crate) fn doctor(access_point: &AccessPointConfig, fixture: &OpenWrtConfig) -> Result<()> {
    let status = Command::new("sh")
        .args(["-c", "command -v wpa_passphrase >/dev/null"])
        .status()?;
    if !status.success() {
        return Err("wpa_passphrase is required for the controlled OpenWrt AP client".into());
    }
    let script = format!(
        "set -eu; \
         command -v iw >/dev/null; \
         command -v ip >/dev/null; \
         command -v ping >/dev/null; \
         command -v wpa_supplicant >/dev/null; \
         command -v nft >/dev/null; \
         command -v fw4 >/dev/null; \
         test \"$(sysctl -n net.ipv4.ip_forward)\" = 1; \
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
            "OpenWrt fixture cannot host a controlled AP client: {}",
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
        security: WifiAccessPointSecurity,
        fixed_ht_mcs: Option<u8>,
        fixed_guard_interval: HtGuardIntervalExpectation,
    ) -> Result<Self> {
        let address = access_point.secondary_client_cidr().ok_or(
            "the AP scenario requires a second client but secondary_client_address is absent",
        )?;
        let client_address = access_point.secondary_client_address().ok_or(
            "the AP scenario requires a second client but secondary_client_address is absent",
        )?;
        Self::connect_at(
            access_point,
            fixture,
            address,
            client_address,
            security,
            fixed_ht_mcs,
            fixed_guard_interval,
        )
    }

    pub(crate) fn connect_primary(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
        security: WifiAccessPointSecurity,
        fixed_ht_mcs: Option<u8>,
        fixed_guard_interval: HtGuardIntervalExpectation,
    ) -> Result<Self> {
        Self::connect_at(
            access_point,
            fixture,
            access_point.client_cidr(),
            access_point.client_address(),
            security,
            fixed_ht_mcs,
            fixed_guard_interval,
        )
    }

    fn connect_at(
        access_point: &AccessPointConfig,
        fixture: &OpenWrtConfig,
        address: String,
        client_address: Ipv4Addr,
        security: WifiAccessPointSecurity,
        fixed_ht_mcs: Option<u8>,
        fixed_guard_interval: HtGuardIntervalExpectation,
    ) -> Result<Self> {
        if fixed_ht_mcs.is_some_and(|mcs| mcs > 7) {
            return Err("controlled OpenWrt AP-client fixed HT MCS must be within 0..=7".into());
        }
        let (ssid, passphrase) = access_point.credentials();
        let target = access_point.target_address();
        let frequency_mhz = access_point.frequency_mhz();
        let ssid_hex = encode_hex(ssid.as_bytes());
        let security_config = match security {
            WifiAccessPointSecurity::Open => Zeroizing::new(String::from("key_mgmt=NONE")),
            WifiAccessPointSecurity::Wpa2Personal => {
                let psk = derive_psk(ssid, passphrase)?;
                Zeroizing::new(format!(
                    "psk={}\\n  key_mgmt=WPA-PSK\\n  proto=RSN\\n  pairwise=CCMP\\n  group=CCMP",
                    psk.as_str(),
                ))
            }
        };
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
             printf 'ctrl_interface=/var/run/wpa_supplicant\\nnetwork={{\\n  ssid={ssid_hex}\\n  {security_config}\\n}}\\n' > {CONFIG_FILE}; \
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
            security_config = security_config.as_str(),
        ));
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
        if fixed_ht_mcs.is_some() || fixed_guard_interval != HtGuardIntervalExpectation::Any {
            let mut rate_mask = format!("iw dev {INTERFACE} set bitrates");
            if let Some(mcs) = fixed_ht_mcs {
                rate_mask.push_str(&format!(" ht-mcs-2.4 {mcs}"));
            }
            match fixed_guard_interval {
                HtGuardIntervalExpectation::Any => {}
                HtGuardIntervalExpectation::Long => rate_mask.push_str(" lgi-2.4"),
                HtGuardIntervalExpectation::Short => rate_mask.push_str(" sgi-2.4"),
            }
            let output = ssh(fixture, &rate_mask)?;
            if !output.status.success() {
                let _ = restore(fixture);
                return Err(format!(
                    "cannot apply the controlled OpenWrt AP-client HT rate mask `{rate_mask}`: {}",
                    String::from_utf8_lossy(&output.stderr).trim(),
                )
                .into());
            }
        }
        // Association is visible before all bridge/driver counters settle.
        // Keep this outside the measured workload and give the target time to
        // publish its controlled-port state.
        thread::sleep(Duration::from_millis(250));
        let forward_address =
            match install_forwarding(fixture, access_point.target_address(), client_address) {
                Ok(address) => Some(address),
                Err(error) => {
                    let restore_error = restore(fixture).err();
                    return Err(match restore_error {
                        Some(restore_error) => {
                            format!("{error}; OpenWrt client cleanup also failed: {restore_error}")
                                .into()
                        }
                        None => error,
                    });
                }
            };
        Ok(Self {
            fixture: fixture.clone(),
            forward_address,
            restored: false,
        })
    }

    /// Address reached by the wired host while OpenWrt owns the Wi-Fi peer.
    ///
    /// The scoped forwarding rules translate this management address to the
    /// DUT AP address. The host therefore remains the traffic generator, but
    /// every measured radio frame still crosses the controlled OpenWrt peer.
    pub(crate) const fn forward_address(&self) -> Option<Ipv4Addr> {
        self.forward_address
    }

    pub(crate) fn begin_link_observation(&self) -> Result<OpenWrtClientLinkObservation> {
        Ok(OpenWrtClientLinkObservation {
            fixture: self.fixture.clone(),
            before: snapshot_link(&self.fixture)?,
        })
    }

    pub(crate) fn restore(mut self) -> Result<()> {
        restore(&self.fixture)?;
        self.restored = true;
        Ok(())
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

impl OpenWrtClientLinkObservation {
    pub(crate) fn finish(self) -> Result<OpenWrtClientLinkEvidence> {
        let after = snapshot_link(&self.fixture)?;
        Ok(OpenWrtClientLinkEvidence {
            rx_bytes: counter_delta("RX bytes", self.before.rx_bytes, after.rx_bytes)?,
            rx_packets: counter_delta("RX packets", self.before.rx_packets, after.rx_packets)?,
            rx_duration_micros: optional_counter_delta(
                "RX duration",
                self.before.rx_duration_micros,
                after.rx_duration_micros,
            )?,
            rx_bitrate: after.rx_bitrate,
            tx_bytes: counter_delta("TX bytes", self.before.tx_bytes, after.tx_bytes)?,
            tx_packets: counter_delta("TX packets", self.before.tx_packets, after.tx_packets)?,
            tx_bitrate: after.tx_bitrate,
            tx_retries: counter_delta("TX retries", self.before.tx_retries, after.tx_retries)?,
            tx_failed: counter_delta("TX failed", self.before.tx_failed, after.tx_failed)?,
            tx_duration_micros: counter_delta(
                "TX duration",
                self.before.tx_duration_micros,
                after.tx_duration_micros,
            )?,
            tid0_aqm_drops: counter_delta(
                "TID-0 AQM drops",
                self.before.tid0_aqm_drops,
                after.tid0_aqm_drops,
            )?,
        })
    }
}

impl Drop for ControlledOpenWrtClient {
    fn drop(&mut self) {
        if !self.restored {
            let _ = restore(&self.fixture);
        }
    }
}

fn install_forwarding(
    fixture: &OpenWrtConfig,
    target: Ipv4Addr,
    client: Ipv4Addr,
) -> Result<Ipv4Addr> {
    let cleanup = cleanup_forwarding_script();
    let script = format!(
        "set -eu; \
         cleanup_forwarding() {{ {cleanup}; }}; \
         cleanup_forwarding; \
         trap 'cleanup_forwarding' EXIT; \
         host_ip=${{SSH_CLIENT%% *}}; \
         route=$(ip -4 route get \"$host_ip\"); \
         host_if=$(printf '%s\\n' \"$route\" | awk '{{for (i=1; i<=NF; i++) if ($i == \"dev\") {{print $(i+1); exit}}}}'); \
         management_ip=$(printf '%s\\n' \"$route\" | awk '{{for (i=1; i<=NF; i++) if ($i == \"src\") {{print $(i+1); exit}}}}'); \
         test -n \"$host_if\"; \
         test -n \"$management_ip\"; \
         nft add chain inet fw4 {FORWARD_CHAIN}; \
         nft insert rule inet fw4 forward jump {FORWARD_CHAIN}; \
         nft add rule inet fw4 {FORWARD_CHAIN} iifname \"$host_if\" oifname \"{INTERFACE}\" ip saddr \"$host_ip\" ip daddr {target} accept; \
         nft add rule inet fw4 {FORWARD_CHAIN} iifname \"{INTERFACE}\" oifname \"$host_if\" ip saddr {target} ip daddr \"$host_ip\" accept; \
         nft add table inet {FORWARD_TABLE}; \
         nft 'add chain inet {FORWARD_TABLE} prerouting {{ type nat hook prerouting priority -101; policy accept; }}'; \
         nft 'add chain inet {FORWARD_TABLE} postrouting {{ type nat hook postrouting priority 101; policy accept; }}'; \
         nft add rule inet {FORWARD_TABLE} prerouting iifname \"$host_if\" ip saddr \"$host_ip\" ip daddr \"$management_ip\" udp dport {UDP_RX_PORT} dnat ip to {target}; \
         nft add rule inet {FORWARD_TABLE} prerouting iifname \"$host_if\" ip saddr \"$host_ip\" ip daddr \"$management_ip\" udp dport {UDP_TX_PORT} dnat ip to {target}; \
         nft add rule inet {FORWARD_TABLE} postrouting oifname \"{INTERFACE}\" ip saddr \"$host_ip\" ip daddr {target} udp dport {UDP_RX_PORT} snat ip to {client}; \
         nft add rule inet {FORWARD_TABLE} postrouting oifname \"{INTERFACE}\" ip saddr \"$host_ip\" ip daddr {target} udp dport {UDP_TX_PORT} snat ip to {client}; \
         nft add rule inet {FORWARD_TABLE} prerouting iifname \"$host_if\" ip saddr \"$host_ip\" ip daddr \"$management_ip\" icmp type echo-request dnat ip to {target}; \
         nft add rule inet {FORWARD_TABLE} postrouting oifname \"{INTERFACE}\" ip saddr \"$host_ip\" ip daddr {target} icmp type echo-request snat ip to {client}; \
         trap - EXIT; \
         printf 'forward_address=%s\\n' \"$management_ip\"",
        UDP_RX_PORT = 4_323,
        UDP_TX_PORT = 4_324,
    );
    let output = ssh(fixture, &script)?;
    if !output.status.success() {
        return Err(format!(
            "cannot install scoped OpenWrt AP forwarding: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        )
        .into());
    }
    tagged_ipv4(&String::from_utf8(output.stdout)?, "forward_address")
}

fn snapshot_link(fixture: &OpenWrtConfig) -> Result<OpenWrtClientLinkSnapshot> {
    let script = format!(
        "set -eu; \
         stats=$(iw dev {INTERFACE} station dump); \
         test -n \"$stats\"; \
         set -- /sys/kernel/debug/ieee80211/*/netdev:{INTERFACE}/stations/*/aqm; \
         test \"$#\" -eq 1; test -r \"$1\"; aqm=\"$1\"; \
         printf 'rx_bytes=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*rx bytes:/ {{print $3}}')\"; \
         printf 'rx_packets=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*rx packets:/ {{print $3}}')\"; \
         printf 'rx_duration=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*rx duration:/ {{print $3}}')\"; \
         printf 'rx_bitrate=%s\\n' \"$(printf '%s\\n' \"$stats\" | sed -n 's/^[[:space:]]*rx bitrate:[[:space:]]*//p')\"; \
         printf 'tx_bytes=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx bytes:/ {{print $3}}')\"; \
         printf 'tx_packets=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx packets:/ {{print $3}}')\"; \
         printf 'tx_bitrate=%s\\n' \"$(printf '%s\\n' \"$stats\" | sed -n 's/^[[:space:]]*tx bitrate:[[:space:]]*//p')\"; \
         printf 'tx_retries=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx retries:/ {{print $3}}')\"; \
         printf 'tx_failed=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx failed:/ {{print $3}}')\"; \
         printf 'tx_duration=%s\\n' \"$(printf '%s\\n' \"$stats\" | awk '/^[[:space:]]*tx duration:/ {{print $3}}')\"; \
         printf 'tid0_aqm_drops=%s\\n' \"$(awk '$1 == 0 {{print $6}}' \"$aqm\")\""
    );
    let output = ssh(fixture, &script)?;
    if !output.status.success() {
        return Err(format!(
            "cannot snapshot controlled OpenWrt AP-client link counters: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        )
        .into());
    }
    let output = String::from_utf8(output.stdout)?;
    Ok(OpenWrtClientLinkSnapshot {
        rx_bytes: tagged_u64(&output, "rx_bytes")?,
        rx_packets: tagged_u64(&output, "rx_packets")?,
        rx_duration_micros: tagged_optional_u64(&output, "rx_duration")?,
        rx_bitrate: tagged_optional_string(&output, "rx_bitrate"),
        tx_bytes: tagged_u64(&output, "tx_bytes")?,
        tx_packets: tagged_u64(&output, "tx_packets")?,
        tx_bitrate: tagged_optional_string(&output, "tx_bitrate"),
        tx_retries: tagged_u64(&output, "tx_retries")?,
        tx_failed: tagged_u64(&output, "tx_failed")?,
        tx_duration_micros: tagged_u64(&output, "tx_duration")?,
        tid0_aqm_drops: tagged_u64(&output, "tid0_aqm_drops")?,
    })
}

fn tagged_u64(output: &str, key: &str) -> Result<u64> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt output omitted `{key}`"))?
        .parse()
        .map_err(|error| format!("invalid OpenWrt `{key}` counter: {error}").into())
}

fn tagged_optional_u64(output: &str, key: &str) -> Result<Option<u64>> {
    tagged_optional_string(output, key)
        .map(|value| {
            value
                .parse()
                .map_err(|error| format!("invalid OpenWrt `{key}` counter: {error}").into())
        })
        .transpose()
}

fn tagged_optional_string(output: &str, key: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn counter_delta(name: &str, before: u64, after: u64) -> Result<u64> {
    after.checked_sub(before).ok_or_else(|| {
        format!(
            "controlled OpenWrt AP-client `{name}` counter reset during the workload: {before} -> {after}"
        )
        .into()
    })
}

fn optional_counter_delta(
    name: &str,
    before: Option<u64>,
    after: Option<u64>,
) -> Result<Option<u64>> {
    match (before, after) {
        (Some(before), Some(after)) => counter_delta(name, before, after).map(Some),
        (None, None) => Ok(None),
        _ => Err(format!(
            "controlled OpenWrt AP-client `{name}` counter availability changed during the workload"
        )
        .into()),
    }
}

fn cleanup_forwarding_script() -> String {
    format!(
        "nft delete table inet {FORWARD_TABLE} 2>/dev/null || true; \
         for handle in $(nft -a list chain inet fw4 forward 2>/dev/null | awk '/jump {FORWARD_CHAIN}/ {{print $NF}}'); do \
             nft delete rule inet fw4 forward handle \"$handle\" 2>/dev/null || true; \
         done; \
         nft flush chain inet fw4 {FORWARD_CHAIN} 2>/dev/null || true; \
         nft delete chain inet fw4 {FORWARD_CHAIN} 2>/dev/null || true"
    )
}

fn tagged_ipv4(output: &str, key: &str) -> Result<Ipv4Addr> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .ok_or_else(|| format!("OpenWrt output omitted `{key}`"))?
        .parse()
        .map_err(|error| format!("invalid OpenWrt `{key}` address: {error}").into())
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
    let cleanup = cleanup_forwarding_script();
    let script = format!(
        "set -eu; \
         {cleanup}; \
         if test -f {PID_FILE}; then kill $(cat {PID_FILE}) 2>/dev/null || true; fi; \
         iw dev {INTERFACE} del 2>/dev/null || true; \
         rm -f {PID_FILE} {CONFIG_FILE}",
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

    #[test]
    fn forwarding_address_is_typed() {
        assert_eq!(
            tagged_ipv4("forward_address=192.168.178.2\n", "forward_address").unwrap(),
            Ipv4Addr::new(192, 168, 178, 2),
        );
    }

    #[test]
    fn link_snapshot_parser_and_counter_delta_are_strict() {
        let output = "rx_bytes=200\nrx_packets=30\nrx_duration=88\nrx_bitrate=150.0 MBit/s MCS 7 40MHz short GI\ntx_bytes=100\ntx_packets=20\ntx_bitrate=135.0 MBit/s MCS 6 40MHz short GI\ntx_retries=3\ntx_failed=1\ntx_duration=77\ntid0_aqm_drops=0\n";
        assert_eq!(tagged_u64(output, "rx_packets").unwrap(), 30);
        assert_eq!(
            tagged_optional_u64(output, "rx_duration").unwrap(),
            Some(88)
        );
        assert_eq!(
            tagged_optional_string(output, "rx_bitrate").as_deref(),
            Some("150.0 MBit/s MCS 7 40MHz short GI"),
        );
        assert_eq!(tagged_u64(output, "tx_packets").unwrap(), 20);
        assert_eq!(
            tagged_optional_string(output, "tx_bitrate").as_deref(),
            Some("135.0 MBit/s MCS 6 40MHz short GI"),
        );
        assert_eq!(counter_delta("packets", 20, 27).unwrap(), 7);
        assert!(counter_delta("packets", 27, 20).is_err());
        assert_eq!(
            optional_counter_delta("duration", Some(20), Some(27)).unwrap(),
            Some(7),
        );
        assert!(optional_counter_delta("duration", Some(20), None).is_err());
    }
}
