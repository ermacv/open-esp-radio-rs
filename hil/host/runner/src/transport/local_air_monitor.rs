//! Independent passive 802.11 evidence from the laptop radio.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use crate::{
    Result,
    transport::openwrt_fixture::resolve_station_mac,
    transport::openwrt_tx_monitor::MacFrameKey,
    transport::{self, lab_config::OpenWrtConfig},
};

const MONITOR_INTERFACE: &str = "mon0";
const MAX_CAPTURE_BYTES: u64 = 128 * 1024 * 1024;
const RETRY_GROUP_SECONDS: f64 = 0.100;

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(crate) struct LocalAirMonitorEvidence {
    pub(crate) captured_frames: u64,
    pub(crate) kernel_dropped: u64,
    pub(crate) logical_data_units: u32,
    pub(crate) retry_attempts: u32,
    pub(crate) missing_mac_metadata: u32,
    #[serde(skip)]
    pub(crate) mac_units: BTreeMap<MacFrameKey, u32>,
    pub(crate) block_ack_frames: u32,
    pub(crate) full_block_ack_frames: u32,
    pub(crate) tail_block_ack_frames: u32,
    pub(crate) hole_block_ack_frames: u32,
    pub(crate) unique_block_acked_mpdus: u32,
    pub(crate) backward_block_ack_starts: u32,
}

pub(crate) struct LocalAirMonitorCapture {
    output: PathBuf,
    target_mac: String,
    child: Option<Child>,
    owns_monitor: bool,
}

impl LocalAirMonitorCapture {
    pub(crate) fn start(
        config: &OpenWrtConfig,
        target: Ipv4Addr,
        duration: Duration,
        output: &Path,
    ) -> Result<Self> {
        if !config.independent_laptop_monitor {
            return Err(
                "scenario requires the independent laptop monitor, but the fixture does not enable it"
                    .into(),
            );
        }
        let target_mac = resolve_station_mac(config, target)?;
        helper_action(resolve_observer_action(config)?)?;
        let capture_path = output.join("independent-air.pcapng");
        let timeout = duration.saturating_add(Duration::from_secs(3));
        let child = Command::new("dumpcap")
            // libpcap's `wlan host` capture filter drops the target's HT40
            // A-MPDU records on this radiotap interface. Capture the bounded
            // channel view and apply the exact target-MAC display filter in
            // `parse_capture` instead.
            .args(["-q", "-i", MONITOR_INTERFACE, "-s", "512"])
            .args(["-a", &format!("duration:{}", timeout.as_secs().max(1))])
            .arg("-w")
            .arg(&capture_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = helper_action("managed");
                return Err(format!("cannot start independent laptop capture: {error}").into());
            }
        };
        thread::sleep(Duration::from_millis(500));
        if let Some(status) = child.try_wait()? {
            let _ = helper_action("managed");
            return Err(format!(
                "independent laptop capture exited before the session started: {status}"
            )
            .into());
        }
        Ok(Self {
            output: capture_path,
            target_mac,
            child: Some(child),
            owns_monitor: true,
        })
    }

    pub(crate) fn finish(mut self) -> Result<LocalAirMonitorEvidence> {
        let output = self
            .child
            .take()
            .expect("independent monitor capture owns its child")
            .wait_with_output();
        let restore = self.restore_managed();
        let output = output?;
        restore?;
        if !output.status.success() {
            return Err(format!(
                "independent laptop capture failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let summary = String::from_utf8(output.stderr)?;
        let captured_frames = dumpcap_captured(&summary)?;
        let kernel_dropped = dumpcap_dropped(&summary)?;
        if kernel_dropped != 0 {
            return Err(format!(
                "independent laptop capture dropped {kernel_dropped} packets in its capture socket"
            )
            .into());
        }
        let size = fs::metadata(&self.output)?.len();
        if size == 0 || size > MAX_CAPTURE_BYTES {
            return Err(format!(
                "independent laptop capture size is outside 1..={MAX_CAPTURE_BYTES} bytes: {size}"
            )
            .into());
        }
        let mut evidence = parse_capture(&self.output, &self.target_mac)?;
        evidence.captured_frames = captured_frames;
        evidence.kernel_dropped = kernel_dropped;
        Ok(evidence)
    }

    fn restore_managed(&mut self) -> Result<()> {
        if self.owns_monitor {
            helper_action("managed")?;
            self.owns_monitor = false;
        }
        Ok(())
    }
}

fn resolve_observer_action(config: &OpenWrtConfig) -> Result<&'static str> {
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(format!("iw dev {} info", config.wireless_interface))
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot query OpenWrt channel for independent monitor: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    resolve_observer_action_from_iw(&String::from_utf8(output.stdout)?)
}

fn resolve_observer_action_from_iw(info: &str) -> Result<&'static str> {
    let channel = info.lines().find_map(|line| {
        line.trim()
            .strip_prefix("channel ")?
            .split_whitespace()
            .next()?
            .parse::<u8>()
            .ok()
    });
    match channel {
        Some(1) => Ok("observer-ht40-1"),
        Some(6) => Ok("observer-ht40-6"),
        Some(11) => Ok("observer-ht40-11"),
        Some(13) => Ok("observer-ht40-13"),
        Some(channel) => Err(format!(
            "independent HT40 monitor does not support OpenWrt primary channel {channel}"
        )
        .into()),
        None => Err("OpenWrt interface did not report its channel".into()),
    }
}

impl Drop for LocalAirMonitorCapture {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.restore_managed();
    }
}

pub(crate) fn doctor(config: &OpenWrtConfig) -> Result<()> {
    if !config.independent_laptop_monitor {
        return Ok(());
    }
    transport::network_helper::doctor()?;
    for tool in ["dumpcap", "tshark"] {
        let status = Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(format!("`{tool}` is required for independent air evidence").into());
        }
    }
    Ok(())
}

fn parse_capture(path: &Path, target_mac: &str) -> Result<LocalAirMonitorEvidence> {
    let output = Command::new("tshark")
        .args(["-r"])
        .arg(path)
        .args([
            "-Y",
            &format!("wlan.fc.type == 2 && wlan.da == {target_mac}"),
            "-T",
            "fields",
            "-E",
            "separator=\t",
            "-e",
            "frame.time_epoch",
            "-e",
            "wlan.seq",
            "-e",
            "wlan.frag",
            "-e",
            "wlan.qos.tid",
            "-e",
            "wlan.fc.retry",
        ])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot decode independent laptop capture: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let mut evidence = LocalAirMonitorEvidence::default();
    let mut last_observed = BTreeMap::<MacFrameKey, f64>::new();
    for line in String::from_utf8(output.stdout)?.lines() {
        let mut fields = line.splitn(5, '\t');
        let timestamp = fields.next().and_then(|value| value.parse::<f64>().ok());
        let sequence = fields.next().and_then(|value| value.parse::<u16>().ok());
        let fragment = fields.next().and_then(|value| value.parse::<u8>().ok());
        let tid = fields
            .next()
            .and_then(|value| value.parse::<u8>().ok())
            .or(Some(u8::MAX));
        let retry = fields.next().is_some_and(parse_retry_flag);
        let Some((timestamp, key)) = timestamp.zip(sequence.zip(fragment).zip(tid).map(
            |((sequence, fragment), tid)| MacFrameKey {
                tid,
                sequence,
                fragment,
            },
        )) else {
            evidence.missing_mac_metadata = evidence.missing_mac_metadata.saturating_add(1);
            continue;
        };
        if retry {
            evidence.retry_attempts = evidence.retry_attempts.saturating_add(1);
        }
        let is_new = !retry
            || last_observed
                .get(&key)
                .is_none_or(|previous| timestamp - previous > RETRY_GROUP_SECONDS);
        last_observed.insert(key, timestamp);
        if is_new {
            evidence.logical_data_units = evidence.logical_data_units.saturating_add(1);
            let count = evidence.mac_units.entry(key).or_default();
            *count = count.saturating_add(1);
        }
    }
    parse_block_ack_capture(path, target_mac, &mut evidence)?;
    Ok(evidence)
}

fn parse_block_ack_capture(
    path: &Path,
    target_mac: &str,
    evidence: &mut LocalAirMonitorEvidence,
) -> Result<()> {
    let output = Command::new("tshark")
        .args(["-r"])
        .arg(path)
        .args([
            "-Y",
            &format!("wlan.fc.type == 1 && wlan.fc.subtype == 9 && wlan.ta == {target_mac}"),
            "-T",
            "fields",
            "-E",
            "separator=\t",
            "-e",
            "wlan.fixed.ssc.sequence",
            "-e",
            "wlan.ba.bm",
        ])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot decode independent BlockAck capture: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let mut tracker = BlockAckTracker::default();
    for line in String::from_utf8(output.stdout)?.lines() {
        let Some((sequence, bitmap)) = line.split_once('\t') else {
            continue;
        };
        let Some(sequence) = sequence.parse::<u16>().ok() else {
            continue;
        };
        let Some(bitmap) = decode_block_ack_bitmap(bitmap) else {
            continue;
        };
        tracker.observe(sequence, bitmap);
    }
    evidence.block_ack_frames = tracker.frames;
    evidence.full_block_ack_frames = tracker.full_frames;
    evidence.tail_block_ack_frames = tracker.tail_frames;
    evidence.hole_block_ack_frames = tracker.hole_frames;
    evidence.unique_block_acked_mpdus =
        u32::try_from(tracker.acknowledged.len()).unwrap_or(u32::MAX);
    evidence.backward_block_ack_starts = tracker.backward_starts;
    Ok(())
}

#[derive(Default)]
struct BlockAckTracker {
    frames: u32,
    full_frames: u32,
    tail_frames: u32,
    hole_frames: u32,
    backward_starts: u32,
    previous_start: Option<u16>,
    unwrapped_start: u64,
    acknowledged: BTreeSet<u64>,
}

impl BlockAckTracker {
    fn observe(&mut self, start: u16, bitmap: [u8; 8]) {
        let start = start & 0x0fff;
        if let Some(previous) = self.previous_start {
            let advance = start.wrapping_sub(previous) & 0x0fff;
            if advance > 0x0800 {
                self.backward_starts = self.backward_starts.saturating_add(1);
                return;
            }
            self.unwrapped_start = self.unwrapped_start.saturating_add(u64::from(advance));
        } else {
            self.unwrapped_start = u64::from(start);
        }
        self.previous_start = Some(start);
        self.frames = self.frames.saturating_add(1);
        if bitmap == [u8::MAX; 8] {
            self.full_frames = self.full_frames.saturating_add(1);
        } else if block_ack_bitmap_has_internal_hole(bitmap) {
            self.hole_frames = self.hole_frames.saturating_add(1);
        } else {
            self.tail_frames = self.tail_frames.saturating_add(1);
        }
        for (byte_index, byte) in bitmap.into_iter().enumerate() {
            for bit in 0..8_u8 {
                if byte & (1_u8 << bit) != 0 {
                    self.acknowledged.insert(
                        self.unwrapped_start
                            .saturating_add((byte_index * 8 + usize::from(bit)) as u64),
                    );
                }
            }
        }
    }
}

/// Return true when a later MPDU is acknowledged after an earlier zero bit.
///
/// A non-full compressed BlockAck bitmap is not itself loss evidence. During
/// window growth or at the tail of a transfer it normally contains a prefix
/// of set bits followed by zeroes. Only a set bit after the first zero proves
/// that the receiver observed a hole inside the represented sequence range.
fn block_ack_bitmap_has_internal_hole(bitmap: [u8; 8]) -> bool {
    let mut observed_zero = false;
    for byte in bitmap {
        for bit in 0..8_u8 {
            if byte & (1_u8 << bit) == 0 {
                observed_zero = true;
            } else if observed_zero {
                return true;
            }
        }
    }
    false
}

fn decode_block_ack_bitmap(value: &str) -> Option<[u8; 8]> {
    if value.len() != 16 {
        return None;
    }
    let mut bitmap = [0_u8; 8];
    for (index, byte) in bitmap.iter_mut().enumerate() {
        *byte = u8::from_str_radix(value.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bitmap)
}

fn parse_retry_flag(value: &str) -> bool {
    matches!(value, "1" | "true" | "True")
}

fn helper_action(action: &str) -> Result<()> {
    let status = Command::new("sudo")
        .args(["-n", transport::network_helper::PATH, action])
        .status()?;
    if !status.success() {
        return Err(format!("laptop radio helper `{action}` failed with {status}").into());
    }
    Ok(())
}

fn dumpcap_captured(summary: &str) -> Result<u64> {
    summary
        .lines()
        .find_map(|line| line.trim().strip_prefix("Packets captured:"))
        .ok_or("independent capture omitted its packet count")?
        .trim()
        .parse()
        .map_err(|error| format!("invalid independent captured packet count: {error}").into())
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
        .ok_or("independent capture omitted its drop count")?;
    counts
        .split_once('/')
        .and_then(|(_, dropped)| dropped.split_whitespace().next())
        .ok_or("independent capture reported a malformed drop count")?
        .parse()
        .map_err(|error| format!("invalid independent dropped packet count: {error}").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_grouping_counts_one_logical_mpdu() {
        let key = MacFrameKey {
            tid: 0,
            sequence: 12,
            fragment: 0,
        };
        let mut last = BTreeMap::new();
        last.insert(key, 1.0_f64);
        assert!(1.02 - last[&key] <= RETRY_GROUP_SECONDS);
        assert!(1.2 - last[&key] > RETRY_GROUP_SECONDS);
    }

    #[test]
    fn accepts_tshark_boolean_and_numeric_retry_values() {
        assert!(parse_retry_flag("1"));
        assert!(parse_retry_flag("True"));
        assert!(parse_retry_flag("true"));
        assert!(!parse_retry_flag("0"));
        assert!(!parse_retry_flag("False"));
        assert!(!parse_retry_flag(""));
    }

    #[test]
    fn monitor_action_follows_openwrt_primary_channel() {
        assert_eq!(
            resolve_observer_action_from_iw(
                "type AP\n\tchannel 6 (2437 MHz), width: 40 MHz, center1: 2447 MHz\n"
            )
            .unwrap(),
            "observer-ht40-6"
        );
        assert_eq!(
            resolve_observer_action_from_iw(
                "type AP\n\tchannel 13 (2472 MHz), width: 40 MHz, center1: 2462 MHz\n"
            )
            .unwrap(),
            "observer-ht40-13"
        );
        assert!(resolve_observer_action_from_iw("channel 3 (2422 MHz)").is_err());
    }

    #[test]
    fn block_ack_tracker_unwraps_windows_and_deduplicates_overlap() {
        let mut tracker = BlockAckTracker::default();
        tracker.observe(4_090, [u8::MAX; 8]);
        tracker.observe(10, [u8::MAX; 8]);
        assert_eq!(tracker.frames, 2);
        assert_eq!(tracker.full_frames, 2);
        assert_eq!(tracker.tail_frames, 0);
        assert_eq!(tracker.hole_frames, 0);
        assert_eq!(tracker.backward_starts, 0);
        assert_eq!(tracker.acknowledged.len(), 80);

        tracker.observe(9, [1; 8]);
        assert_eq!(tracker.backward_starts, 1);
        assert_eq!(tracker.frames, 2);
    }

    #[test]
    fn decodes_little_bit_order_block_ack_bitmap_bytes() {
        let bitmap = decode_block_ack_bitmap("7f00000000000000").unwrap();
        let mut tracker = BlockAckTracker::default();
        tracker.observe(1, bitmap);
        assert_eq!(tracker.tail_frames, 1);
        assert_eq!(tracker.hole_frames, 0);
        assert_eq!(tracker.acknowledged.len(), 7);
    }

    #[test]
    fn distinguishes_normal_block_ack_tail_from_internal_loss_hole() {
        assert!(!block_ack_bitmap_has_internal_hole([
            0xff, 0x7f, 0, 0, 0, 0, 0, 0,
        ]));
        assert!(block_ack_bitmap_has_internal_hole([
            0xff, 0xf7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        ]));

        let mut tracker = BlockAckTracker::default();
        tracker.observe(1, [0xff, 0x7f, 0, 0, 0, 0, 0, 0]);
        tracker.observe(16, [0xff, 0xf7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(tracker.full_frames, 0);
        assert_eq!(tracker.tail_frames, 1);
        assert_eq!(tracker.hole_frames, 1);
    }
}
