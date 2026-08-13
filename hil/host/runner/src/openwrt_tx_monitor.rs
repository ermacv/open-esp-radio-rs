//! Opt-in packet evidence from the OpenWrt AP's own TX monitor tap.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use crate::{Result, lab_config::OpenWrtConfig};

const REMOTE_CAPTURE: &str = "/tmp/open-radio-ap-tx-monitor.pcap";
const MAX_CAPTURE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct MacFrameKey {
    pub(crate) tid: u8,
    pub(crate) sequence: u16,
    pub(crate) fragment: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct OpenWrtTxMonitorEvidence {
    pub(crate) captured_frames: u64,
    pub(crate) kernel_dropped: u64,
    pub(crate) data_units: u32,
    pub(crate) unique_units: u32,
    pub(crate) duplicates: u32,
    pub(crate) gap_events: u32,
    pub(crate) forward_missing: u32,
    pub(crate) late_recovered: u32,
    pub(crate) unrecovered: u32,
    pub(crate) out_of_range: u32,
    pub(crate) terminal_markers: u32,
    pub(crate) mac_retry_publications: u32,
    pub(crate) missing_mac_metadata: u32,
    pub(crate) first_anomaly: Option<u32>,
    pub(crate) mac_units: BTreeMap<MacFrameKey, u32>,
}

pub(crate) struct OpenWrtTxMonitorCapture {
    config: OpenWrtConfig,
    monitor: String,
    target: Ipv4Addr,
    port: u16,
    output: PathBuf,
    child: Option<Child>,
}

impl OpenWrtTxMonitorCapture {
    pub(crate) fn start(
        config: &OpenWrtConfig,
        target: Ipv4Addr,
        port: u16,
        duration: Duration,
        output: &Path,
    ) -> Result<Self> {
        let monitor = config
            .monitor_interface
            .clone()
            .ok_or("OpenWrt TX-monitor evidence requires `station_fixture.monitor_interface`")?;
        let seconds = duration.saturating_add(Duration::from_secs(3)).as_secs();
        let script = format!(
            "set -eu; \
             ! iw dev {monitor} info >/dev/null 2>&1; \
             wiphy=$(iw dev {wireless} info | awk '/wiphy/ {{print \"phy\" $2; exit}}'); \
             mac=$(ip neigh show {target} | awk 'NR==1 {{print $5}}'); \
             test -n \"$wiphy\"; test -n \"$mac\"; \
             rm -f {remote}; \
             iw phy \"$wiphy\" interface add {monitor} type monitor; \
             cleanup() {{ iw dev {monitor} del >/dev/null 2>&1 || true; }}; \
             trap cleanup EXIT HUP INT TERM; \
             ip link set {monitor} up; \
             set +e; timeout -s INT {seconds} tcpdump -i {monitor} -n -s 128 -U \
                 -w {remote} \"wlan host $mac\"; status=$?; set -e; \
             test \"$status\" -eq 0 -o \"$status\" -eq 124",
            wireless = config.wireless_interface,
            remote = REMOTE_CAPTURE,
        );
        let mut child = ssh(config, &script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start OpenWrt TX-monitor capture: {error}"))?;
        thread::sleep(Duration::from_secs(1));
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "OpenWrt TX-monitor capture exited before the session started: {status}"
            )
            .into());
        }
        Ok(Self {
            config: config.clone(),
            monitor,
            target,
            port,
            output: output.join("ap-tx-monitor.pcap"),
            child: Some(child),
        })
    }

    pub(crate) fn finish(mut self, expected_units: u64) -> Result<OpenWrtTxMonitorEvidence> {
        let child = self
            .child
            .take()
            .expect("TX-monitor capture owns its child");
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(format!(
                "OpenWrt TX-monitor capture failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let summary = String::from_utf8(output.stderr)?;
        let captured_frames = summary_value(&summary, "packets captured")
            .ok_or("OpenWrt TX-monitor capture omitted its packet count")?;
        let kernel_dropped = summary_value(&summary, "packets dropped by kernel").unwrap_or(0);
        if kernel_dropped != 0 {
            return Err(format!(
                "OpenWrt TX-monitor capture dropped {kernel_dropped} packets in its capture socket"
            )
            .into());
        }
        copy_remote(&self.config, &self.output)?;
        remove_remote(&self.config);
        let size = fs::metadata(&self.output)?.len();
        if size == 0 || size > MAX_CAPTURE_BYTES {
            return Err(format!(
                "OpenWrt TX-monitor capture size is outside 1..={MAX_CAPTURE_BYTES} bytes: {size}"
            )
            .into());
        }
        let mut evidence = parse_capture(&self.output, self.target, self.port, expected_units)?;
        evidence.captured_frames = captured_frames;
        evidence.kernel_dropped = kernel_dropped;
        Ok(evidence)
    }
}

impl Drop for OpenWrtTxMonitorCapture {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let cleanup = format!(
            "iw dev {} del >/dev/null 2>&1 || true; rm -f {}",
            self.monitor, REMOTE_CAPTURE
        );
        let _ = ssh(&self.config, &cleanup).status();
    }
}

pub(crate) fn doctor(config: &OpenWrtConfig) -> Result<()> {
    let Some(monitor) = &config.monitor_interface else {
        return Ok(());
    };
    let output = ssh(
        config,
        &format!(
            "set -eu; ! iw dev {monitor} info >/dev/null 2>&1; \
             wiphy=$(iw dev {} info | awk '/wiphy/ {{print \"phy\" $2; exit}}'); \
             iw phy \"$wiphy\" info | grep -q '^[[:space:]]*\\* monitor$'",
            config.wireless_interface
        ),
    )
    .output()?;
    if !output.status.success() {
        return Err(format!(
            "OpenWrt monitor evidence is unavailable or `{monitor}` already exists: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let status = Command::new("tshark")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err("local tshark is required for OpenWrt TX-monitor evidence".into());
    }
    Ok(())
}

fn parse_capture(
    path: &Path,
    target: Ipv4Addr,
    port: u16,
    expected_units: u64,
) -> Result<OpenWrtTxMonitorEvidence> {
    let output = Command::new("tshark")
        .args(["-r"])
        .arg(path)
        .args([
            "-Y",
            "wlan.fc.type == 2",
            "-T",
            "fields",
            "-E",
            "separator=\t",
            "-e",
            "wlan.seq",
            "-e",
            "wlan.frag",
            "-e",
            "wlan.qos.tid",
            "-e",
            "wlan.fc.retry",
            "-e",
            "radiotap.data_retries",
            "-e",
            "data.data",
        ])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot decode OpenWrt TX-monitor capture: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let expected_units = u32::try_from(expected_units)?;
    let mut tracker = SequenceTracker::default();
    for line in String::from_utf8(output.stdout)?.lines() {
        let mut fields = line.splitn(6, '\t');
        let mac_sequence = fields.next().and_then(|value| value.parse::<u16>().ok());
        let mac_fragment = fields.next().and_then(|value| value.parse::<u8>().ok());
        let mac_tid = fields
            .next()
            .and_then(|value| value.parse::<u8>().ok())
            .or(Some(u8::MAX));
        let _retry = fields.next();
        let retries = fields
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let Some(raw) = fields.next().and_then(decode_hex) else {
            continue;
        };
        let Some(sequence) = udp_sequence(&raw, target, port) else {
            continue;
        };
        let mac = mac_sequence
            .zip(mac_fragment)
            .zip(mac_tid)
            .map(|((sequence, fragment), tid)| MacFrameKey {
                tid,
                sequence,
                fragment,
            });
        tracker.observe(sequence, mac, retries, expected_units);
    }
    Ok(tracker.finish(expected_units))
}

#[derive(Default)]
struct SequenceTracker {
    seen: BTreeSet<u32>,
    evidence: OpenWrtTxMonitorEvidence,
    highest: Option<u32>,
}

impl SequenceTracker {
    fn observe(&mut self, sequence: i32, mac: Option<MacFrameKey>, retries: u32, limit: u32) {
        self.evidence.mac_retry_publications =
            self.evidence.mac_retry_publications.saturating_add(retries);
        if sequence < 0 {
            self.evidence.terminal_markers = self.evidence.terminal_markers.saturating_add(1);
            return;
        }
        let sequence = sequence as u32;
        self.evidence.data_units = self.evidence.data_units.saturating_add(1);
        if let Some(mac) = mac {
            let count = self.evidence.mac_units.entry(mac).or_default();
            *count = count.saturating_add(1);
        } else {
            self.evidence.missing_mac_metadata =
                self.evidence.missing_mac_metadata.saturating_add(1);
        }
        if sequence >= limit {
            self.evidence.out_of_range = self.evidence.out_of_range.saturating_add(1);
            self.evidence.first_anomaly.get_or_insert(sequence);
            return;
        }
        if !self.seen.insert(sequence) {
            self.evidence.duplicates = self.evidence.duplicates.saturating_add(1);
            self.evidence.first_anomaly.get_or_insert(sequence);
            return;
        }
        self.evidence.unique_units = self.evidence.unique_units.saturating_add(1);
        match self.highest {
            None => self.highest = Some(sequence),
            Some(highest) if sequence > highest => {
                let missing = sequence.saturating_sub(highest).saturating_sub(1);
                if missing != 0 {
                    self.evidence.gap_events = self.evidence.gap_events.saturating_add(1);
                    self.evidence.forward_missing =
                        self.evidence.forward_missing.saturating_add(missing);
                    self.evidence.first_anomaly.get_or_insert(sequence);
                }
                self.highest = Some(sequence);
            }
            Some(_) => {
                self.evidence.late_recovered = self.evidence.late_recovered.saturating_add(1);
                self.evidence.first_anomaly.get_or_insert(sequence);
            }
        }
    }

    fn finish(mut self, expected: u32) -> OpenWrtTxMonitorEvidence {
        self.evidence.unrecovered = expected.saturating_sub(self.evidence.unique_units);
        self.evidence
    }
}

fn udp_sequence(raw: &[u8], target: Ipv4Addr, port: u16) -> Option<i32> {
    for offset in 0..raw.len().min(32) {
        let version_ihl = *raw.get(offset)?;
        if version_ihl >> 4 != 4 || version_ihl & 0x0f < 5 {
            continue;
        }
        let header_len = usize::from(version_ihl & 0x0f) * 4;
        if *raw.get(offset + 9)? != 17 || raw.get(offset + 16..offset + 20)? != target.octets() {
            continue;
        }
        let udp = offset + header_len;
        if u16::from_be_bytes(raw.get(udp + 2..udp + 4)?.try_into().ok()?) != port {
            continue;
        }
        return Some(i32::from_be_bytes(
            raw.get(udp + 8..udp + 12)?.try_into().ok()?,
        ));
    }
    None
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

fn copy_remote(config: &OpenWrtConfig, local: &Path) -> Result<()> {
    let file = File::create(local)?;
    let status = ssh(config, &format!("cat {REMOTE_CAPTURE}"))
        .stdout(Stdio::from(file))
        .status()?;
    if !status.success() {
        return Err("cannot copy OpenWrt TX-monitor capture to the run directory".into());
    }
    Ok(())
}

fn remove_remote(config: &OpenWrtConfig) {
    let _ = ssh(config, &format!("rm -f {REMOTE_CAPTURE}")).status();
}

fn ssh(config: &OpenWrtConfig, script: &str) -> Command {
    let mut command = Command::new("ssh");
    command
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(script);
    command
}

fn summary_value(summary: &str, suffix: &str) -> Option<u64> {
    summary
        .lines()
        .find_map(|line| line.trim().strip_suffix(suffix)?.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_sequence_from_truncated_plaintext_ap_tx() {
        let mut packet = vec![0, 0, 0x08, 0x00];
        packet.extend_from_slice(&[
            0x45, 0, 0x05, 0xdc, 0, 0, 0x40, 0, 64, 17, 0, 0, 192, 168, 1, 129, 192, 168, 1, 182,
            0x83, 0xcb, 0x10, 0xe3, 0x05, 0xc8, 0, 0, 0, 0, 0, 42,
        ]);
        assert_eq!(
            udp_sequence(&packet, Ipv4Addr::new(192, 168, 1, 182), 4323),
            Some(42)
        );
    }

    #[test]
    fn sequence_tracker_separates_late_recovery_from_final_loss() {
        let mut tracker = SequenceTracker::default();
        for sequence in [0, 2, 1, 4, -1] {
            tracker.observe(sequence, None, 0, 5);
        }
        let evidence = tracker.finish(5);
        assert_eq!(evidence.forward_missing, 2);
        assert_eq!(evidence.late_recovered, 1);
        assert_eq!(evidence.unrecovered, 1);
        assert_eq!(evidence.terminal_markers, 1);
    }
}
