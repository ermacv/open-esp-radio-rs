//! Independent passive 802.11 evidence from the laptop radio.

use std::{
    collections::BTreeMap,
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use crate::{Result, lab_config::OpenWrtConfig, network_helper, openwrt_tx_monitor::MacFrameKey};

const MONITOR_INTERFACE: &str = "mon0";
const MAX_CAPTURE_BYTES: u64 = 128 * 1024 * 1024;
const RETRY_GROUP_SECONDS: f64 = 0.100;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LocalAirMonitorEvidence {
    pub(crate) captured_frames: u64,
    pub(crate) kernel_dropped: u64,
    pub(crate) logical_data_units: u32,
    pub(crate) retry_attempts: u32,
    pub(crate) missing_mac_metadata: u32,
    pub(crate) mac_units: BTreeMap<MacFrameKey, u32>,
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
        let target_mac = resolve_target_mac(config, target)?;
        helper_action("observer-ht40-1")?;
        let capture_path = output.join("independent-air.pcapng");
        let timeout = duration.saturating_add(Duration::from_secs(3));
        let filter = format!("wlan host {target_mac}");
        let child = Command::new("dumpcap")
            .args(["-q", "-i", MONITOR_INTERFACE, "-f", &filter, "-s", "128"])
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
    network_helper::doctor()?;
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

fn resolve_target_mac(config: &OpenWrtConfig, target: Ipv4Addr) -> Result<String> {
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(&config.ssh_target)
        .arg(format!("ip neigh show {target} | awk 'NR==1 {{print $5}}'"))
        .output()?;
    if !output.status.success() {
        return Err("cannot resolve target MAC through the OpenWrt fixture".into());
    }
    let mac = String::from_utf8(output.stdout)?
        .trim()
        .to_ascii_lowercase();
    let valid = mac.len() == 17
        && mac.split(':').count() == 6
        && mac
            .split(':')
            .all(|octet| octet.len() == 2 && octet.chars().all(|value| value.is_ascii_hexdigit()));
    if !valid {
        return Err(format!("OpenWrt returned an invalid target MAC `{mac}`").into());
    }
    Ok(mac)
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
    Ok(evidence)
}

fn parse_retry_flag(value: &str) -> bool {
    matches!(value, "1" | "true" | "True")
}

fn helper_action(action: &str) -> Result<()> {
    let status = Command::new("sudo")
        .args(["-n", network_helper::PATH, action])
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
}
