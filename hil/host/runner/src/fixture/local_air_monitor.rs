//! Independent passive 802.11 evidence from the laptop radio.

use oer_process::CommandExt as _;
use oer_process::owned::Child;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use crate::{
    Result,
    fixture::{openwrt_fixture::resolve_station_mac, openwrt_tx_monitor::MacFrameKey},
    lab::config::OpenWrtConfig,
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
    /// Target-oriented egress timing. This is deliberately independent of
    /// whether the target is a station or an access point.
    pub(crate) target_egress: TargetEgressAirTimingEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct AirIntervalSummary {
    pub(crate) samples: u32,
    pub(crate) total_micros: u64,
    pub(crate) minimum_micros: u64,
    pub(crate) p50_micros: u64,
    pub(crate) p95_micros: u64,
    pub(crate) p99_micros: u64,
    pub(crate) maximum_micros: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub(crate) struct TargetEgressAirTimingEvidence {
    pub(crate) target_data_frames: u32,
    pub(crate) peer_block_ack_frames: u32,
    /// Whether the observer decoded enough target data records to pair every
    /// peer BlockAck with a target transmission. Pair-derived intervals stay
    /// absent when this is false; sparse target decoding must not manufacture
    /// apparently valid multi-millisecond gaps.
    pub(crate) target_data_pairing_available: bool,
    /// Direction-neutral cadence of peer BlockAck responses to the target.
    /// This remains useful when the observer cannot decode the target's HT40
    /// A-MPDU records, but it cannot separate peer response time from the
    /// target's post-BlockAck scheduling delay.
    pub(crate) peer_block_ack_interarrival: Option<AirIntervalSummary>,
    pub(crate) data_to_block_ack: Option<AirIntervalSummary>,
    pub(crate) block_ack_to_next_data: Option<AirIntervalSummary>,
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
            return Err(crate::fixture::Error::new("scenario requires the independent laptop monitor, but the fixture does not enable it").into());
        }
        let target_mac = resolve_station_mac(config, target)?;
        Self::start_for_target(
            target_mac,
            duration,
            output,
            resolve_observer_action(config).map_err(crate::fixture::Error::context)?,
            true,
        )
    }

    /// Observe an AP while the laptop's managed interface remains its traffic
    /// client. The compatibility monitor joins the existing channel instead
    /// of replacing `wlan0` with a standalone monitor wdev.
    pub(crate) fn start_associated(
        target_mac: String,
        duration: Duration,
        output: &Path,
    ) -> Result<Self> {
        Self::start_for_target(target_mac, duration, output, "monitor", false)
    }

    fn start_for_target(
        target_mac: String,
        duration: Duration,
        output: &Path,
        observer_action: &str,
        restore_managed: bool,
    ) -> Result<Self> {
        let mut owner = Self {
            output: output.join("independent-air.pcapng"),
            target_mac,
            child: None,
            owns_monitor: restore_managed,
        };
        helper_action(observer_action)?;
        let timeout = duration.saturating_add(Duration::from_secs(3));
        owner.child = Some(
            Command::new("dumpcap")
                // libpcap's `wlan host` capture filter drops the target's HT40
                // A-MPDU records on this radiotap interface. Capture the bounded
                // channel view and apply the exact target-MAC display filter in
                // `parse_capture` instead.
                .args(["-q", "-i", MONITOR_INTERFACE, "-s", "512"])
                .args(["-a", &format!("duration:{}", timeout.as_secs().max(1))])
                .arg("-w")
                .arg(&owner.output)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn_owned()?
                .with_timeout(timeout.saturating_add(Duration::from_secs(10))),
        );
        oer_process::sleep(Duration::from_millis(500))?;
        if let Some(status) = owner
            .child
            .as_mut()
            .expect("capture owns its child")
            .try_wait()?
        {
            return Err(crate::fixture::Error::new(format!(
                "independent laptop capture exited before the session started: {status}"
            ))
            .into());
        }
        Ok(owner)
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
            return Err(crate::fixture::Error::new(format!(
                "independent laptop capture failed with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
            .into());
        }
        let summary = String::from_utf8(output.stderr)?;
        let captured_frames = dumpcap_captured(&summary).map_err(crate::fixture::Error::context)?;
        let kernel_dropped = dumpcap_dropped(&summary).map_err(crate::fixture::Error::context)?;
        if kernel_dropped != 0 {
            return Err(crate::fixture::Error::new(format!(
                "independent laptop capture dropped {kernel_dropped} packets in its capture socket"
            ))
            .into());
        }
        let size = fs::metadata(&self.output)?.len();
        if size == 0 || size > MAX_CAPTURE_BYTES {
            return Err(crate::fixture::Error::new(format!(
                "independent laptop capture size is outside 1..={MAX_CAPTURE_BYTES} bytes: {size}"
            ))
            .into());
        }
        let mut evidence = parse_capture(&self.output, &self.target_mac)
            .map_err(crate::fixture::Error::context)?;
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
        .supervised_output()?;
    if !output.status.success() {
        return Err(crate::fixture::Error::new(format!(
            "cannot query OpenWrt channel for independent monitor: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
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
        Some(channel) => Err(crate::fixture::Error::new(format!(
            "independent HT40 monitor does not support OpenWrt primary channel {channel}"
        ))
        .into()),
        None => {
            Err(crate::fixture::Error::new("OpenWrt interface did not report its channel").into())
        }
    }
}

impl Drop for LocalAirMonitorCapture {
    fn drop(&mut self) {
        oer_process::cleanup(|| {
            if let Some(child) = &mut self.child {
                let _ = child.kill();
                let _ = child.wait();
            }
            crate::fixture::cleanup::record("restore monitor interface", || self.restore_managed());
        });
    }
}

pub(crate) fn doctor() -> Result<()> {
    crate::fixture::network_helper::doctor()?;
    for tool in ["dumpcap", "tshark"] {
        let status = Command::new(tool)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .supervised_status()?;
        if !status.success() {
            return Err(crate::fixture::Error::new(format!(
                "`{tool}` is required for independent air evidence"
            ))
            .into());
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
        .supervised_output()?;
    if !output.status.success() {
        return Err(crate::fixture::Error::new(format!(
            "cannot decode independent laptop capture: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
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
    evidence.target_egress = parse_target_egress_capture(path, target_mac)?;
    Ok(evidence)
}

fn parse_target_egress_capture(
    path: &Path,
    target_mac: &str,
) -> Result<TargetEgressAirTimingEvidence> {
    let output = Command::new("tshark")
        .args(["-r"])
        .arg(path)
        .args([
            "-Y",
            &format!(
                "(wlan.fc.type == 2 && wlan.ta == {target_mac}) || \
                 (wlan.fc.type == 1 && wlan.fc.subtype == 9 && wlan.ra == {target_mac})"
            ),
            "-T",
            "fields",
            "-E",
            "separator=\t",
            "-e",
            "frame.time_epoch",
            "-e",
            "wlan.fc.type",
            "-e",
            "wlan.fc.subtype",
        ])
        .supervised_output()?;
    if !output.status.success() {
        return Err(crate::fixture::Error::new(format!(
            "cannot decode target-egress air timing: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .into());
    }
    Ok(target_egress_timing_from_fields(&String::from_utf8(
        output.stdout,
    )?))
}

fn target_egress_timing_from_fields(fields: &str) -> TargetEgressAirTimingEvidence {
    let mut target_data_frames = 0_u32;
    let mut peer_block_ack_frames = 0_u32;
    let mut last_target_data = None;
    let mut previous_block_ack = None;
    let mut pending_block_ack = None;
    let mut peer_block_ack_interarrival = Vec::new();
    let mut data_to_block_ack = Vec::new();
    let mut block_ack_to_next_data = Vec::new();

    for line in fields.lines() {
        let mut fields = line.splitn(3, '\t');
        let Some(timestamp) = fields.next().and_then(epoch_micros) else {
            continue;
        };
        let frame_type = fields.next().and_then(parse_tshark_u8);
        let subtype = fields.next().and_then(parse_tshark_u8);
        match (frame_type, subtype) {
            (Some(2), _) => {
                target_data_frames = target_data_frames.saturating_add(1);
                if let Some(block_ack) = pending_block_ack.take()
                    && let Some(interval) = timestamp.checked_sub(block_ack)
                {
                    block_ack_to_next_data.push(interval);
                }
                // An A-MPDU can be exposed as multiple MPDU records. Keeping
                // the final data timestamp before the BlockAck measures from
                // the observed end of the PPDU, not from its first subframe.
                last_target_data = Some(timestamp);
            }
            (Some(1), Some(9)) => {
                peer_block_ack_frames = peer_block_ack_frames.saturating_add(1);
                if let Some(previous) = previous_block_ack
                    && let Some(interval) = timestamp.checked_sub(previous)
                {
                    peer_block_ack_interarrival.push(interval);
                }
                previous_block_ack = Some(timestamp);
                if let Some(target_data) = last_target_data.take()
                    && let Some(interval) = timestamp.checked_sub(target_data)
                {
                    data_to_block_ack.push(interval);
                    pending_block_ack = Some(timestamp);
                }
            }
            _ => {}
        }
    }

    let target_data_pairing_available = peer_block_ack_frames != 0
        && usize::try_from(peer_block_ack_frames).ok() == Some(data_to_block_ack.len());
    TargetEgressAirTimingEvidence {
        target_data_frames,
        peer_block_ack_frames,
        target_data_pairing_available,
        peer_block_ack_interarrival: summarize_intervals(peer_block_ack_interarrival),
        data_to_block_ack: target_data_pairing_available
            .then(|| summarize_intervals(data_to_block_ack))
            .flatten(),
        block_ack_to_next_data: target_data_pairing_available
            .then(|| summarize_intervals(block_ack_to_next_data))
            .flatten(),
    }
}

fn epoch_micros(value: &str) -> Option<u64> {
    let (seconds, fraction) = value.trim().split_once('.').unwrap_or((value.trim(), ""));
    let seconds = seconds.parse::<u64>().ok()?;
    let mut micros = 0_u64;
    let mut digits = 0_u8;
    for byte in fraction.bytes().take(6) {
        if !byte.is_ascii_digit() {
            return None;
        }
        micros = micros
            .checked_mul(10)?
            .checked_add(u64::from(byte - b'0'))?;
        digits += 1;
    }
    while digits < 6 {
        micros = micros.checked_mul(10)?;
        digits += 1;
    }
    seconds.checked_mul(1_000_000)?.checked_add(micros)
}

fn parse_tshark_u8(value: &str) -> Option<u8> {
    let value = value.trim();
    value.parse().ok().or_else(|| {
        value
            .strip_prefix("0x")
            .and_then(|value| u8::from_str_radix(value, 16).ok())
    })
}

fn summarize_intervals(mut intervals: Vec<u64>) -> Option<AirIntervalSummary> {
    if intervals.is_empty() {
        return None;
    }
    intervals.sort_unstable();
    let samples = u32::try_from(intervals.len()).unwrap_or(u32::MAX);
    Some(AirIntervalSummary {
        samples,
        total_micros: intervals.iter().copied().fold(0_u64, u64::saturating_add),
        minimum_micros: intervals[0],
        p50_micros: nearest_rank(&intervals, 50),
        p95_micros: nearest_rank(&intervals, 95),
        p99_micros: nearest_rank(&intervals, 99),
        maximum_micros: *intervals.last().expect("nonempty interval sample"),
    })
}

fn nearest_rank(sorted: &[u64], percentile: usize) -> u64 {
    let rank = sorted.len().saturating_mul(percentile).div_ceil(100).max(1);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
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
        .supervised_output()?;
    if !output.status.success() {
        return Err(crate::fixture::Error::new(format!(
            "cannot decode independent BlockAck capture: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
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
        .args(["-n", crate::fixture::network_helper::PATH, action])
        .supervised_status()?;
    if !status.success() {
        return Err(crate::fixture::Error::new(format!(
            "laptop radio helper `{action}` failed with {status}"
        ))
        .into());
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
mod tests;
