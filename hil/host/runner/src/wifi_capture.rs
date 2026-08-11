//! Finite normalized 802.11 capture exported through the typed HIL protocol.

mod assembly;
mod pcapng;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use open_esp_radio_hil_protocol::WifiMonitorCaptureRequest;

use crate::{
    Result,
    controlled_ap::{ControlledAp, require_station_credentials},
    lab_config::LabConfig,
    traffic_capture::{MonitorCaptureEvidence, SerialCapture},
    wifi_control::{scan, start_station, stop_station},
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_CAPTURE_DURATION: Duration = Duration::from_secs(3);

struct Options {
    serial: PathBuf,
    output: PathBuf,
    timeout: Duration,
    duration: Duration,
    channel: Option<u8>,
    snapshot_length: u16,
    external_ap: bool,
}

pub(crate) fn run(arguments: Vec<String>, artifact_dir: &Path, lab: &LabConfig) -> Result<()> {
    let options = parse_options(&arguments, lab)?;
    let _access_point = if options.external_ap {
        require_station_credentials(&lab.station)?;
        None
    } else {
        Some(ControlledAp::start(&lab.station, &lab.openwrt)?)
    };
    fs::create_dir_all(&artifact_dir)?;
    if let Some(parent) = options.output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let serial = SerialCapture::start_with_reset(&options.serial);
    let result = qualify(&serial, lab, &options);
    serial.finish_to(artifact_dir)?;
    let result = result?;
    pcapng::write_capture(
        &options.output,
        &result.assembly.packets,
        result.host_anchor_micros,
    )?;
    if result.assembly.incomplete_frames != 0
        || result.capture.summary.exported_frames as usize != result.assembly.packets.len()
    {
        return Err(format!(
            "monitor export was incomplete: target_exported={} reconstructed={} incomplete={}",
            result.capture.summary.exported_frames,
            result.assembly.packets.len(),
            result.assembly.incomplete_frames,
        )
        .into());
    }
    println!("wifi_capture=PASS");
    println!("pcapng={}", options.output.display());
    println!(
        "generation={} frames={} bytes={} full_drops={} oversized_drops={} discarded={}",
        result.capture.summary.generation,
        result.capture.summary.exported_frames,
        result.capture.summary.captured_bytes,
        result.capture.summary.full_drops,
        result.capture.summary.oversized_drops,
        result.capture.summary.discarded_frames,
    );
    println!("uart_log={}", artifact_dir.join("uart.log").display());
    Ok(())
}

struct CaptureResult {
    capture: MonitorCaptureEvidence,
    assembly: assembly::AssemblyReport,
    host_anchor_micros: u64,
}

fn qualify(serial: &SerialCapture, lab: &LabConfig, options: &Options) -> Result<CaptureResult> {
    let capabilities = serial.prepare_station(lab, options.timeout)?;
    if !capabilities.features.wifi_monitor_capture {
        return Err("firmware does not advertise typed monitor capture".into());
    }
    serial.wait_for_connected_station(options.timeout)?;
    stop_station(serial, options.timeout)?;
    let channel = match options.channel {
        Some(channel) => channel,
        None => {
            let evidence = scan(serial, options.timeout)?;
            if !(1..=13).contains(&evidence.configured_ssid_channel) {
                start_station(serial, lab, options.timeout)?;
                return Err("scan did not provide a capture channel".into());
            }
            evidence.configured_ssid_channel
        }
    };
    let duration_millis = u32::try_from(options.duration.as_millis())
        .map_err(|_| "capture duration does not fit the HIL protocol")?;
    let host_anchor_micros = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_micros()
        .try_into()
        .map_err(|_| "host wall clock does not fit PCAPNG timestamp")?;
    let handle = serial.request_monitor_capture(WifiMonitorCaptureRequest {
        channel,
        snapshot_length: options.snapshot_length,
        duration_millis,
    })?;
    let capture = serial.wait_monitor_capture(handle, options.timeout + options.duration);
    let restart = start_station(serial, lab, options.timeout);
    let capture = match (capture, restart) {
        (Ok(capture), Ok(())) => capture,
        (Err(capture), Ok(())) => return Err(capture),
        (Ok(_), Err(restart)) => {
            return Err(format!("station restart after capture failed: {restart}").into());
        }
        (Err(capture), Err(restart)) => {
            return Err(format!(
                "monitor capture failed ({capture}); station restart also failed ({restart})"
            )
            .into());
        }
    };
    let stack = serial.query_stack_usage(options.timeout)?;
    println!(
        "stack_stage=monitor-capture cpu0_free={}/{} cpu1_free={}/{} required_percent={}",
        stack.cpu0.free_bytes,
        stack.cpu0.capacity_bytes,
        stack.cpu1.free_bytes,
        stack.cpu1.capacity_bytes,
        stack.minimum_free_percent,
    );
    validate_summary(&capture, channel, options.duration)?;
    let assembly = assembly::assemble(capture.chunks.clone())?;
    Ok(CaptureResult {
        capture,
        assembly,
        host_anchor_micros,
    })
}

fn validate_summary(
    capture: &MonitorCaptureEvidence,
    channel: u8,
    requested_duration: Duration,
) -> Result<()> {
    let summary = capture.summary;
    if summary.channel != channel
        || summary.generation_mismatches != 0
        || summary.channel_mismatches != 0
    {
        return Err(format!("monitor returned inconsistent capture evidence: {summary:?}").into());
    }
    if summary.exported_frames == 0 || summary.captured_bytes == 0 {
        return Err("finite monitor capture exported no frames".into());
    }
    let requested_micros = requested_duration.as_micros().min(u128::from(u64::MAX)) as u64;
    let minimum = requested_micros * 95 / 100;
    let maximum = requested_micros * 110 / 100;
    if summary.elapsed_micros < minimum || summary.elapsed_micros > maximum {
        return Err(format!(
            "finite monitor timing is outside the qualified range: requested_us={requested_micros} observed_us={} range={minimum}..={maximum}",
            summary.elapsed_micros,
        )
        .into());
    }
    if summary.exported_frames != summary.captured_frames {
        return Err(format!(
            "target exported {} frames after consuming {}",
            summary.exported_frames, summary.captured_frames
        )
        .into());
    }
    if summary.published_frames
        != summary
            .exported_frames
            .saturating_add(summary.discarded_frames)
    {
        return Err(format!(
            "capture ownership accounting mismatch: published={} exported={} discarded={}",
            summary.published_frames, summary.exported_frames, summary.discarded_frames,
        )
        .into());
    }
    Ok(())
}

fn parse_options(arguments: &[String], lab: &LabConfig) -> Result<Options> {
    let serial = lab.device.serial.clone();
    let mut output = None;
    let mut timeout = DEFAULT_TIMEOUT;
    let mut duration = DEFAULT_CAPTURE_DURATION;
    let mut channel = None;
    let mut snapshot_length = 256_u16;
    let mut external_ap = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--output" => {
                index += 1;
                output = Some(PathBuf::from(
                    arguments.get(index).ok_or("--output requires a path")?,
                ));
            }
            "--timeout-seconds" => {
                index += 1;
                let seconds = arguments
                    .get(index)
                    .ok_or("--timeout-seconds requires a value")?
                    .parse::<u64>()?;
                if !(10..=180).contains(&seconds) {
                    return Err("--timeout-seconds must be in 10..=180".into());
                }
                timeout = Duration::from_secs(seconds);
            }
            "--seconds" => {
                index += 1;
                let seconds = arguments
                    .get(index)
                    .ok_or("--seconds requires a value")?
                    .parse::<u64>()?;
                if !(1..=30).contains(&seconds) {
                    return Err("--seconds must be in 1..=30".into());
                }
                duration = Duration::from_secs(seconds);
            }
            "--channel" => {
                index += 1;
                let selected = arguments
                    .get(index)
                    .ok_or("--channel requires a value")?
                    .parse::<u8>()?;
                if !(1..=13).contains(&selected) {
                    return Err("--channel must be in 1..=13".into());
                }
                channel = Some(selected);
            }
            "--snapshot-length" => {
                index += 1;
                snapshot_length = arguments
                    .get(index)
                    .ok_or("--snapshot-length requires a value")?
                    .parse::<u16>()?;
                if snapshot_length > 2_304 {
                    return Err("--snapshot-length must be in 0..=2304".into());
                }
            }
            "--external-ap" => external_ap = true,
            argument => return Err(format!("unknown Wi-Fi capture option `{argument}`").into()),
        }
        index += 1;
    }
    Ok(Options {
        serial,
        output: output.ok_or("--output is required")?,
        timeout,
        duration,
        channel,
        snapshot_length,
        external_ap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_requires_output_and_bounds_capture() {
        let lab = LabConfig::for_test();
        assert!(parse_options(&[], &lab).is_err());
        let options = parse_options(
            &[
                "--output".into(),
                "capture.pcapng".into(),
                "--seconds".into(),
                "5".into(),
                "--channel".into(),
                "11".into(),
                "--snapshot-length".into(),
                "512".into(),
            ],
            &lab,
        )
        .unwrap();
        assert_eq!(options.output, PathBuf::from("capture.pcapng"));
        assert_eq!(options.duration, Duration::from_secs(5));
        assert_eq!(options.channel, Some(11));
        assert_eq!(options.snapshot_length, 512);
    }

    #[test]
    fn parser_rejects_unbounded_values() {
        assert!(
            parse_options(
                &[
                    "--output".into(),
                    "capture.pcapng".into(),
                    "--seconds".into(),
                    "31".into(),
                ],
                &LabConfig::for_test()
            )
            .is_err()
        );
    }
}
